// src/ffmpeg_worker.rs
// 負責調用 FFmpeg 進行 4 象限裁切與 v360 魚眼拉直處理，以及產生示範樣品影片

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::io::{BufRead, BufReader};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use crossbeam_channel::Sender;
use egui::Pos2;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

/// 單一鏡頭之廣角角度 (FOV)、縮放倍率、三軸角度修正 (Pitch/Yaw/Roll) 與 X/Y 軸平移控制
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CameraParam {
    pub in_fov: i32,   // 輸入魚眼 FOV (120° ~ 230°，預設 180° / 195°)
    pub zoom: f32,     // 縮放比例 (0.5x ~ 3.0x，預設 1.0x)
    pub out_hfov: i32, // 展開水平 FOV (60° ~ 140°，預設 110° / 120°)
    pub out_vfov: i32, // 展開垂直 FOV (50° ~ 120°，預設 80° / 85°)
    pub pitch: i32,    // 俯仰角度 (-45° ~ +45°，負值朝下、正值朝上)
    pub yaw: i32,      // 水平偏航 (-45° ~ +45°，負值偏左、正值偏右)
    pub roll: i32,     // 滾轉傾斜 (-30° ~ +30°，水平旋轉調平)
    pub offset_x: f32, // X 軸平移微調 (-1.00m ~ +1.00m，步進 0.01m，預設 0.00m)
    pub offset_y: f32, // Y 軸平移微調 (-1.00m ~ +1.00m，步進 0.01m，預設 0.00m)
}

impl Default for CameraParam {
    fn default() -> Self {
        Self::default_for_cam(0)
    }
}

#[allow(dead_code)]
impl CameraParam {
    /// 依據 CAM 專案標定參數校正後之最佳預設參數
    /// 俯仰 (Pitch)、偏航 (Yaw)、滾轉 (Roll) 預設為 0° 微調偏量，代表 100% 套用 CAM 實車標定物理角度
    pub fn default_for_cam(cam_idx: usize) -> Self {
        match cam_idx {
            0 => Self {
                // 前視鏡頭 (TL): CAM 基準 pos=[0.11, 2.33, 0.82], yaw=4°, pitch=26.4°, roll=2.2°
                in_fov: 185,
                zoom: 1.00,
                out_hfov: 110,
                out_vfov: 80,
                pitch: 0, // 微調偏量 (預設 0°)
                yaw: 0,   // 微調偏量 (預設 0°)
                roll: 0,  // 微調偏量 (預設 0°)
                offset_x: 0.0,
                offset_y: 0.0,
            },
            1 => Self {
                // 後視鏡頭 (TR): CAM 基準 pos=[-0.04, -2.47, 0.75], yaw=180°, pitch=18.9°, roll=4.0°
                in_fov: 185,
                zoom: 1.00,
                out_hfov: 110,
                out_vfov: 80,
                pitch: 0, // 微調偏量 (預設 0°)
                yaw: 0,   // 微調偏量 (預設 0°)
                roll: 0,  // 微調偏量 (預設 0°)
                offset_x: 0.0,
                offset_y: 0.0,
            },
            2 => Self {
                // 左側鏡頭 (BL): CAM 基準 pos=[-1.03, 0.55, 1.02], yaw=-90°, pitch=85.0°, roll=-7.6°
                in_fov: 195,
                zoom: 1.00,
                out_hfov: 120,
                out_vfov: 85,
                pitch: 0, // 微調偏量 (預設 0°)
                yaw: 0,   // 微調偏量 (預設 0°)
                roll: 0,  // 微調偏量 (預設 0°)
                offset_x: 0.0,
                offset_y: 0.0,
            },
            _ => Self {
                // 右側鏡頭 (BR): CAM 基準 pos=[0.93, 0.55, 1.02], yaw=90°, pitch=83.1°, roll=2.2°
                in_fov: 195,
                zoom: 1.00,
                out_hfov: 120,
                out_vfov: 85,
                pitch: 0, // 微調偏量 (預設 0°)
                yaw: 0,   // 微調偏量 (預設 0°)
                roll: 0,  // 微調偏量 (預設 0°)
                offset_x: 0.0,
                offset_y: 0.0,
            },
        }
    }

    /// 取得前後左右 4 路鏡頭之完整預設校正參數組
    pub fn default_presets() -> [Self; 4] {
        [
            Self::default_for_cam(0),
            Self::default_for_cam(1),
            Self::default_for_cam(2),
            Self::default_for_cam(3),
        ]
    }

    /// 【第一階段 Stage 1：魚眼鏡頭物理去畸變模型 (Fisheye Radial Barrel Undistortion)】
    /// 將去畸變後的理想針孔透視平面座標 `(xu, yu)` (以鏡頭真實光心 `(u0, v0)` 為原點)
    /// 逆向映射回原始魚眼感測器 UV 座標 `(u, v)`：
    /// `r^2 = xu^2 + yu^2`, `D(r^2) = 1 / sqrt(1 + k1 * r^2 + k2 * r^4)`
    #[inline]
    pub fn undistort_pinhole_to_fisheye(
        xu: f32,
        yu: f32,
        u0: f32,
        v0: f32,
        k1: f32,
        k2: f32,
    ) -> Pos2 {
        let r2 = xu * xu + yu * yu;
        let d = 1.0_f32 / (1.0_f32 + k1 * r2 + k2 * r2 * r2).max(0.05_f32).sqrt();
        Pos2::new(u0 + xu * d, v0 + yu * d)
    }

    /// 在鳥瞰圖已經過實車地面透視與魚眼拉直的基準 UV 上，疊加使用者的即時微調增量 (0ms 即時預覽)
    /// 當參數處於預設值時，100% 保持最佳無縫拼接與筆直雙白線；當拖動滑桿時即時反應縮放與三軸旋轉
    pub fn apply_birdview_delta(&self, cam_idx: usize, base_uv: Pos2, pivot: Pos2) -> Pos2 {
        let def = Self::default_for_cam(cam_idx);

        let safe_in_fov = (self.in_fov as f32).clamp(100.0, 250.0);
        let def_in_fov = (def.in_fov as f32).clamp(100.0, 250.0);
        let safe_zoom = self.zoom.clamp(0.3, 4.0);

        let rel_mag = (safe_zoom / def.zoom) * (def_in_fov / safe_in_fov);
        let scale_x = (self.out_hfov as f32 / def.out_hfov as f32) / rel_mag;
        let scale_y = (self.out_vfov as f32 / def.out_vfov as f32) / rel_mag;

        let mut dx = (base_uv.x - pivot.x) * scale_x;
        let mut dy = (base_uv.y - pivot.y) * scale_y;

        let d_pitch = ((self.pitch - def.pitch) as f32).to_radians();
        let d_yaw = ((self.yaw - def.yaw) as f32).to_radians();
        let d_roll = ((self.roll - def.roll) as f32).to_radians();

        // 俯仰與偏航微調位移
        dy -= d_pitch.sin() * 0.45_f32;
        dx += d_yaw.sin() * 0.45_f32;

        // 俯仰梯形透視微調
        let keystone = (1.0_f32 + dy * d_pitch.sin() * 0.6_f32).clamp(0.5_f32, 2.0_f32);
        dx *= keystone;

        // 滾轉 (Roll) 旋轉
        let cos_r = d_roll.cos();
        let sin_r = d_roll.sin();
        let rot_x = dx * cos_r - dy * sin_r;
        let rot_y = dx * sin_r + dy * cos_r;

        Pos2::new(
            (rot_x + pivot.x).clamp(0.002, 0.998),
            (rot_y + pivot.y).clamp(0.002, 0.998),
        )
    }

    /// 根據當前廣角、縮放與三軸角度修正，將原始 UV 進行即時投影轉換 (供中央單路主畫面 0ms 即時預覽)
    /// - 左側視角 (cam_idx == 2): 自動逆時針旋轉 -90° (讓車頭朝上、車尾朝下、左側車道在左)
    /// - 右側視角 (cam_idx == 3): 自動順時針旋轉 +90° (讓車頭朝上、車尾朝下、右側車道在右)
    /// - 前後視角 (cam_idx == 0, 1): 寬廣自然透視展開，完整保留左右雙白線與前方車道
    pub fn transform_uv_for_cam(&self, cam_idx: usize, raw_uv: Pos2) -> Pos2 {
        let def = Self::default_for_cam(cam_idx);
        let safe_in_fov = (self.in_fov as f32).clamp(100.0, 250.0);
        let def_in_fov = (def.in_fov as f32).clamp(100.0, 250.0);
        let safe_zoom = self.zoom.clamp(0.3, 4.0);

        let rel_mag = (safe_zoom / def.zoom) * (def_in_fov / safe_in_fov);
        let fov_scale_x = (self.out_hfov as f32 / def.out_hfov as f32) / rel_mag;
        let fov_scale_y = (self.out_vfov as f32 / def.out_vfov as f32) / rel_mag;

        let mut nx = (raw_uv.x - 0.5_f32) * 2.0_f32;
        let mut ny = (raw_uv.y - 0.5_f32) * 2.0_f32;

        // 輕度魚眼桶狀反畸變 (將彎曲車道線拉直，同時保留寬廣車道視野)
        let r2 = nx * nx + ny * ny;
        let dewarp = 1.0_f32 - 0.08_f32 * r2;
        nx *= dewarp * fov_scale_x;
        ny *= dewarp * fov_scale_y;

        // 增量俯仰/偏航/滾轉 (預設值時保持最自然寬廣的標準車道構圖)
        let d_pitch = ((self.pitch - def.pitch) as f32).to_radians();
        let d_yaw = ((self.yaw - def.yaw) as f32).to_radians();
        let d_roll = ((self.roll - def.roll) as f32).to_radians();

        ny -= d_pitch.sin() * 0.65_f32;
        nx += d_yaw.sin() * 0.65_f32;

        let keystone = (1.0_f32 + ny * d_pitch.sin() * 0.45_f32).clamp(0.5_f32, 2.0_f32);
        nx *= keystone;

        let cos_r = d_roll.cos();
        let sin_r = d_roll.sin();
        let rx = nx * cos_r - ny * sin_r + self.offset_x * 0.40_f32;
        let ry = nx * sin_r + ny * cos_r - self.offset_y * 0.40_f32;

        let (u, v) = match cam_idx {
            0 => {
                // 前視鏡頭 (TL): 寬廣展開，完整呈現前方車道與左右雙白線
                (0.50_f32 + rx * 0.43_f32, 0.50_f32 + ry * 0.42_f32)
            }
            1 => {
                // 後視鏡頭 (TR): 寬廣展開，完整呈現後方車道與左右雙白線
                (0.525_f32 + rx * 0.43_f32, 0.50_f32 + ry * 0.42_f32)
            }
            2 => {
                // 左側鏡頭 (BL): 轉 -90 度 (逆時針 90 度)
                // 螢幕上方 (ry = -1) 對應車頭 (u 較大)，螢幕下方 (ry = +1) 對應車尾 (u 較小)
                // 螢幕左方 (rx = -1) 對應左側外車道 (v 較小)，螢幕右方 (rx = +1) 對應車身側 (v 較大)
                let u_rot = 0.48_f32 - ry * 0.42_f32;
                let v_rot = 0.46_f32 + rx * 0.42_f32;
                (u_rot, v_rot)
            }
            _ => {
                // 右側鏡頭 (BR): 轉 +90 度 (順時針 90 度)
                // 螢幕上方 (ry = -1) 對應車頭 (u 較小)，螢幕下方 (ry = +1) 對應車尾 (u 較大)
                // 螢幕左方 (rx = -1) 對應車身側 (v 較大)，螢幕右方 (rx = +1) 對應右側外車道 (v 較小)
                let u_rot = 0.51_f32 + ry * 0.42_f32;
                let v_rot = 0.46_f32 - rx * 0.42_f32;
                (u_rot, v_rot)
            }
        };

        Pos2::new(u.clamp(0.002, 0.998), v.clamp(0.002, 0.998))
    }

    pub fn transform_uv(&self, raw_uv: Pos2) -> Pos2 {
        self.transform_uv_for_cam(0, raw_uv)
    }
}

pub struct ProcessConfig {
    pub input_path: PathBuf,
    pub output_dir: PathBuf,
    pub mapping: [String; 4], // [TL, TR, BL, BR] -> roles e.g. "front", "rear", "left", "right"
    pub camera_params: [CameraParam; 4], // 0: front, 1: rear, 2: left, 3: right
}

#[derive(Clone, Debug)]
pub enum WorkerMessage {
    Progress(f32, String),
    Complete(PathBuf, PathBuf, PathBuf, PathBuf), // front, rear, left, right
    Error(String),
}

pub fn get_executable(name: &str) -> PathBuf {
    // 優先檢查同目錄下的 exe
    if let Ok(current_exe) = std::env::current_exe() {
        if let Some(dir) = current_exe.parent() {
            let candidate = dir.join(format!("{}.exe", name));
            if candidate.exists() {
                return candidate;
            }
        }
    }
    // 檢查當前工作目錄
    let candidate = PathBuf::from(format!("{}.exe", name));
    if candidate.exists() {
        return candidate;
    }
    PathBuf::from(name)
}

/// 透過 ffprobe (或備援 ffmpeg -i) 偵測影片真實秒數
pub fn probe_video_duration(path: &Path) -> Option<f32> {
    if !path.exists() {
        return None;
    }

    // 1. 優先使用 ffprobe 讀取 format=duration 與 stream=duration
    let ffprobe_bin = get_executable("ffprobe");
    for entry in ["format=duration", "stream=duration"] {
        let mut cmd = Command::new(&ffprobe_bin);
        #[cfg(windows)]
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW

        cmd.args([
            "-v",
            "error",
            "-show_entries",
            entry,
            "-of",
            "default=noprint_wrappers=1:nokey=1",
        ])
        .arg(path)
        .stdout(Stdio::piped())
        .stderr(Stdio::null());

        if let Ok(output) = cmd.output() {
            if output.status.success() {
                let s = String::from_utf8_lossy(&output.stdout);
                for line in s.lines() {
                    if let Ok(dur) = line.trim().parse::<f32>() {
                        if dur > 0.05 && dur.is_finite() {
                            return Some(dur);
                        }
                    }
                }
            }
        }
    }

    // 2. 備援：若無 ffprobe，使用 ffmpeg -i 從 stderr 解析 "Duration: HH:MM:SS.xx"
    let ffmpeg_bin = get_executable("ffmpeg");
    let mut cmd_ff = Command::new(&ffmpeg_bin);
    #[cfg(windows)]
    cmd_ff.creation_flags(0x08000000); // CREATE_NO_WINDOW

    cmd_ff
        .arg("-i")
        .arg(path)
        .stdout(Stdio::null())
        .stderr(Stdio::piped());

    if let Ok(output) = cmd_ff.output() {
        let stderr_str = String::from_utf8_lossy(&output.stderr);
        if let Some(idx) = stderr_str.find("Duration:") {
            let rest = &stderr_str[idx + "Duration:".len()..].trim_start();
            let time_part = rest.split(',').next().unwrap_or("").trim();
            let parts: Vec<&str> = time_part.split(':').collect();
            if parts.len() == 3 {
                if let (Ok(h), Ok(m), Ok(s)) = (
                    parts[0].trim().parse::<f32>(),
                    parts[1].trim().parse::<f32>(),
                    parts[2].trim().parse::<f32>(),
                ) {
                    let total = h * 3600.0 + m * 60.0 + s;
                    if total > 0.05 && total.is_finite() {
                        return Some(total);
                    }
                }
            }
        }
    }

    None
}

/// 為單一鏡頭產生指定輸出解析度 (out_w, out_h) 的 16-bit 像素重映射表 (gray16le)
/// rawvideo 格式：每像素 2 bytes (little-endian u16)，值為原象限裁切影像 (960x540) 中的 X 與 Y 坐標
fn generate_remap_tables(
    xmap_path: &Path,
    ymap_path: &Path,
    out_w: u32,
    out_h: u32,
    src_w: f32, // 960.0
    src_h: f32, // 540.0
    param: &CameraParam,
    cam_idx: usize,
) -> std::io::Result<()> {
    use std::io::Write;
    let mut bx = std::io::BufWriter::new(std::fs::File::create(xmap_path)?);
    let mut by = std::io::BufWriter::new(std::fs::File::create(ymap_path)?);

    let max_x = (src_w - 1.0).max(0.0);
    let max_y = (src_h - 1.0).max(0.0);

    for y in 0..out_h {
        let raw_y = (y as f32 + 0.5) / (out_h as f32);
        for x in 0..out_w {
            let raw_x = (x as f32 + 0.5) / (out_w as f32);
            let src_uv = param.transform_uv_for_cam(cam_idx, Pos2::new(raw_x, raw_y));
            let sx = (src_uv.x * src_w).clamp(0.0, max_x).round() as u16;
            let sy = (src_uv.y * src_h).clamp(0.0, max_y).round() as u16;
            bx.write_all(&sx.to_le_bytes())?;
            by.write_all(&sy.to_le_bytes())?;
        }
    }
    bx.flush()?;
    by.flush()?;
    Ok(())
}

pub fn start_processing(
    config: ProcessConfig,
    cancel_flag: Arc<AtomicBool>,
    tx: Sender<WorkerMessage>,
) {
    std::thread::spawn(move || {
        let ffmpeg_bin = get_executable("ffmpeg");
        let _ = std::fs::create_dir_all(&config.output_dir);

        // 先偵測輸入影片實際長度，避免進度條固定除以 5 秒
        let total_duration = probe_video_duration(&config.input_path).unwrap_or(5.0).max(0.1);

        let front_out = config.output_dir.join("front.mp4");
        let rear_out = config.output_dir.join("rear.mp4");
        let left_out = config.output_dir.join("left.mp4");
        let right_out = config.output_dir.join("right.mp4");
        let quad_out = config.output_dir.join("quad_rectified.mp4");

        // 建立儲存 remap 映射表的暫存目錄
        let temp_map_dir = std::env::temp_dir().join(format!(
            "dashcam360_remap_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0)
        ));
        let _ = std::fs::create_dir_all(&temp_map_dir);

        let roles = ["front", "rear", "left", "right"];

        // 1. 生成 4 路單路輸出 (960x540) 的 remap 表
        let mut single_map_files = Vec::new();
        for (idx, role) in roles.iter().enumerate() {
            let xmap = temp_map_dir.join(format!("xmap_{}_single.raw", role));
            let ymap = temp_map_dir.join(format!("ymap_{}_single.raw", role));
            if let Err(e) = generate_remap_tables(&xmap, &ymap, 960, 540, 960.0, 540.0, &config.camera_params[idx], idx) {
                let _ = tx.send(WorkerMessage::Error(format!("無法生成單路 {} remap 坐標表: {}", role, e)));
                let _ = std::fs::remove_dir_all(&temp_map_dir);
                return;
            }
            single_map_files.push((xmap, ymap));
        }

        // 2. 生成 4 路四象限拼版輸出 (960x510，配合頂欄 60px 組成標準 1920x1080) 的 remap 表
        let mut quad_map_files = Vec::new();
        for (idx, role) in roles.iter().enumerate() {
            let xmap = temp_map_dir.join(format!("xmap_{}_quad.raw", role));
            let ymap = temp_map_dir.join(format!("ymap_{}_quad.raw", role));
            if let Err(e) = generate_remap_tables(&xmap, &ymap, 960, 510, 960.0, 540.0, &config.camera_params[idx], idx) {
                let _ = tx.send(WorkerMessage::Error(format!("無法生成四象限 {} remap 坐標表: {}", role, e)));
                let _ = std::fs::remove_dir_all(&temp_map_dir);
                return;
            }
            quad_map_files.push((xmap, ymap));
        }

        // 象限裁切
        let quad_crops = [
            "crop=iw/2:ih/2:0:0",
            "crop=iw/2:ih/2:iw/2:0",
            "crop=iw/2:ih/2:0:ih/2",
            "crop=iw/2:ih/2:iw/2:ih/2",
        ];

        let mut filter_parts = Vec::new();

        // 裁切原影片各象限並分流 (split=2) 給單路 remap 與拼版 remap
        for i in 0..4 {
            let crop = quad_crops[i];
            let role = &config.mapping[i];
            filter_parts.push(format!("[0:v]{crop},split=2[{role}_raw_s][{role}_raw_q]"));
        }

        // 單路 remap (輸入 1~8)
        for (idx, role) in roles.iter().enumerate() {
            let in_x = 1 + idx * 2;
            let in_y = 2 + idx * 2;
            filter_parts.push(format!(
                "[{role}_raw_s][{in_x}:v][{in_y}:v]remap,format=yuv420p[{role}]"
            ));
        }

        // 四象限 remap (輸入 9~16)
        for (idx, role) in roles.iter().enumerate() {
            let in_x = 9 + idx * 2;
            let in_y = 10 + idx * 2;
            filter_parts.push(format!(
                "[{role}_raw_q][{in_x}:v][{in_y}:v]remap,format=yuv420p[{role}_q]"
            ));
        }

        // 2x2 四象限排版：
        // 左上: 前視 (front_q)
        // 右上: 後視 (rear_q)
        // 左下: 左視 (left_q)
        // 右下: 右視 (right_q)
        // 四畫面各 960x510，hstack + vstack 為 1920x1020
        // 透過 pad 於上方補 60px 純黑資訊列，達成精準標準 1920x1080 且完全同步長度
        filter_parts.push("[front_q][rear_q]hstack[top_row]".to_string());
        filter_parts.push("[left_q][right_q]hstack[bot_row]".to_string());
        filter_parts.push("[top_row][bot_row]vstack,pad=1920:1080:0:60:black[canvas]".to_string());

        // 擷取原始後視畫面右上角之時間戳記 (完整保留頭尾字元 440x56)，精確疊加於最右上角黑色頂欄，0% 遮擋畫面
        let rear_quad_idx = config.mapping.iter().position(|m| m == "rear").unwrap_or(1);
        let (ts_crop_x, ts_crop_y) = match rear_quad_idx {
            0 => ("iw/2-445", "0"),
            1 => ("iw-445", "0"),
            2 => ("iw/2-445", "ih/2"),
            _ => ("iw-445", "ih/2"),
        };
        filter_parts.push(format!(
            "[0:v]crop=440:56:{ts_crop_x}:{ts_crop_y},format=yuva420p[ts_crop]"
        ));
        filter_parts.push(
            "[canvas][ts_crop]overlay=x=W-w-16:y=2:shortest=1,format=yuv420p[quad_out]".to_string()
        );

        let filter_complex = filter_parts.join(";");

        let _ = tx.send(WorkerMessage::Progress(
            2.0,
            format!("啟動 FFmpeg 像素級精準 remap 轉檔 (影片長度: {:.1}s)...", total_duration),
        ));

        let mut cmd = Command::new(&ffmpeg_bin);
        #[cfg(windows)]
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW

        cmd.arg("-y")
            .arg("-i")
            .arg(&config.input_path);

        // 加入單路 960x540 remap 表輸入 (循環對齊輸入影片長度)
        for (xmap, ymap) in &single_map_files {
            cmd.args(["-stream_loop", "-1", "-f", "rawvideo", "-pixel_format", "gray16le", "-video_size", "960x540", "-i"])
                .arg(xmap)
                .args(["-stream_loop", "-1", "-f", "rawvideo", "-pixel_format", "gray16le", "-video_size", "960x540", "-i"])
                .arg(ymap);
        }

        // 加入四象限 960x510 remap 表輸入 (循環對齊輸入影片長度)
        for (xmap, ymap) in &quad_map_files {
            cmd.args(["-stream_loop", "-1", "-f", "rawvideo", "-pixel_format", "gray16le", "-video_size", "960x510", "-i"])
                .arg(xmap)
                .args(["-stream_loop", "-1", "-f", "rawvideo", "-pixel_format", "gray16le", "-video_size", "960x510", "-i"])
                .arg(ymap);
        }

        cmd.arg("-filter_complex")
            .arg(&filter_complex)
            .args(["-map", "[front]", "-c:v", "libx264", "-preset", "veryfast", "-crf", "20", "-pix_fmt", "yuv420p", "-shortest", "-movflags", "+faststart"])
            .arg(&front_out)
            .args(["-map", "[rear]",  "-c:v", "libx264", "-preset", "veryfast", "-crf", "20", "-pix_fmt", "yuv420p", "-shortest", "-movflags", "+faststart"])
            .arg(&rear_out)
            .args(["-map", "[left]",  "-c:v", "libx264", "-preset", "veryfast", "-crf", "20", "-pix_fmt", "yuv420p", "-shortest", "-movflags", "+faststart"])
            .arg(&left_out)
            .args(["-map", "[right]", "-c:v", "libx264", "-preset", "veryfast", "-crf", "20", "-pix_fmt", "yuv420p", "-shortest", "-movflags", "+faststart"])
            .arg(&right_out)
            .args(["-map", "[quad_out]", "-c:v", "libx264", "-preset", "veryfast", "-crf", "22", "-pix_fmt", "yuv420p", "-shortest", "-movflags", "+faststart"])
            .arg(&quad_out)
            .arg("-progress")
            .arg("pipe:1")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                let _ = tx.send(WorkerMessage::Error(format!("無法啟動 FFmpeg ({}): {}", ffmpeg_bin.display(), e)));
                let _ = std::fs::remove_dir_all(&temp_map_dir);
                return;
            }
        };

        let stderr_lines = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let stderr_lines_clone = stderr_lines.clone();
        if let Some(stderr) = child.stderr.take() {
            std::thread::spawn(move || {
                let reader = BufReader::new(stderr);
                for line in reader.lines().flatten() {
                    let mut lock = stderr_lines_clone.lock().unwrap();
                    if lock.len() >= 12 {
                        lock.remove(0);
                    }
                    lock.push(line);
                }
            });
        }

        if let Some(stdout) = child.stdout.take() {
            let reader = BufReader::new(stdout);
            let mut curr_fps = "30".to_string();

            for line in reader.lines().flatten() {
                if cancel_flag.load(Ordering::SeqCst) {
                    let _ = child.kill();
                    let _ = std::fs::remove_dir_all(&temp_map_dir);
                    let _ = tx.send(WorkerMessage::Progress(0.0, "已取消處理".to_string()));
                    return;
                }

                if line.starts_with("fps=") {
                    curr_fps = line.trim_start_matches("fps=").trim().to_string();
                } else if line.starts_with("out_time_us=") {
                    if let Ok(us) = line.trim_start_matches("out_time_us=").trim().parse::<u64>() {
                        let sec = us as f32 / 1_000_000.0;
                        let pct = (sec / total_duration * 100.0).clamp(2.0, 99.0);
                        let _ = tx.send(WorkerMessage::Progress(
                            pct,
                            format!("處理中: {:.1}% ({:.1}s / {:.1}s, @{}fps)", pct, sec, total_duration, curr_fps),
                        ));
                    }
                }
            }
        }

        let wait_res = child.wait();
        let _ = std::fs::remove_dir_all(&temp_map_dir);

        match wait_res {
            Ok(status) if status.success() => {
                let _ = tx.send(WorkerMessage::Progress(100.0, "像素級一致校正與 4 路分離固化完成！".to_string()));
                let _ = tx.send(WorkerMessage::Complete(front_out, rear_out, left_out, right_out));
            }
            Ok(status) => {
                let err_detail = stderr_lines.lock().unwrap().join("\n");
                let _ = tx.send(WorkerMessage::Error(format!("FFmpeg 執行結束但代碼異常: {:?}\n{}", status.code(), err_detail)));
            }
            Err(e) => {
                let _ = tx.send(WorkerMessage::Error(format!("等待 FFmpeg 錯誤: {}", e)));
            }
        }
    });
}

pub fn generate_demo_sample(out_path: &Path, tx: Sender<WorkerMessage>) {
    let out_file = out_path.to_path_buf();
    std::thread::spawn(move || {
        let ffmpeg_bin = get_executable("ffmpeg");
        let _ = std::fs::create_dir_all(out_file.parent().unwrap_or(Path::new(".")));

        let real_jpg = out_file
            .parent()
            .unwrap_or(Path::new("sample_videos"))
            .join("real_quad_frame.jpg");

        if real_jpg.exists() {
            let _ = tx.send(WorkerMessage::Progress(15.0, "正在載入實車四路魚眼道路樣品...".to_string()));
            let mut cmd = Command::new(&ffmpeg_bin);
            #[cfg(windows)]
            cmd.creation_flags(0x08000000);

            cmd.arg("-y")
                .args(["-loop", "1", "-i"])
                .arg(&real_jpg)
                .args([
                    "-c:v", "libx264", "-preset", "veryfast", "-t", "6", "-r", "25",
                    "-pix_fmt", "yuv420p", "-vf", "scale=1920:1080", "-movflags", "+faststart",
                ])
                .arg(&out_file);

            if let Ok(output) = cmd.output() {
                if output.status.success() {
                    let _ = tx.send(WorkerMessage::Progress(100.0, "實車四路魚眼樣品載入完成！".to_string()));
                    return;
                }
            }
        }

        let _ = tx.send(WorkerMessage::Progress(10.0, "正在生成合成測試影片...".to_string()));

        // 使用 FFmpeg 生成 4 分割魚眼樣品
        let filter = "\
            testsrc=size=960x540:rate=30:duration=5[tl_raw];\
            [tl_raw]drawbox=x=0:y=0:w=960:h=540:color=0x112233@1:t=fill,drawbox=x=440:y=200:w=80:h=340:color=yellow@0.8:t=fill,drawgrid=w=100:h=60:c=cyan@0.2,v360=input=rectilinear:output=fisheye:ih_fov=110:iv_fov=80:h_fov=180:v_fov=180[tl];\
            testsrc=size=960x540:rate=30:duration=5[tr_raw];\
            [tr_raw]drawbox=x=0:y=0:w=960:h=540:color=0x331122@1:t=fill,drawbox=x=300:y=360:w=360:h=30:color=red@0.8:t=fill,drawgrid=w=100:h=50:c=red@0.25,v360=input=rectilinear:output=fisheye:ih_fov=110:iv_fov=80:h_fov=180:v_fov=180[tr];\
            testsrc=size=960x540:rate=30:duration=5[bl_raw];\
            [bl_raw]drawbox=x=0:y=0:w=960:h=540:color=0x113322@1:t=fill,drawbox=x=700:y=200:w=260:h=340:color=0x444444@0.9:t=fill,drawgrid=w=80:h=80:c=green@0.2,v360=input=rectilinear:output=fisheye:ih_fov=110:iv_fov=80:h_fov=180:v_fov=180[bl];\
            testsrc=size=960x540:rate=30:duration=5[br_raw];\
            [br_raw]drawbox=x=0:y=0:w=960:h=540:color=0x222211@1:t=fill,drawbox=x=0:y=200:w=260:h=340:color=0x444444@0.9:t=fill,drawgrid=w=80:h=80:c=yellow@0.2,v360=input=rectilinear:output=fisheye:ih_fov=110:iv_fov=80:h_fov=180:v_fov=180[br];\
            [tl][tr]hstack[top];\
            [bl][br]hstack[bot];\
            [top][bot]vstack,format=yuv420p[out]\
        ";

        let mut cmd = Command::new(&ffmpeg_bin);
        #[cfg(windows)]
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW

        cmd.arg("-y")
            .arg("-filter_complex")
            .arg(filter)
            .args(["-map", "[out]", "-c:v", "libx264", "-preset", "veryfast", "-crf", "20", "-pix_fmt", "yuv420p", "-movflags", "+faststart"])
            .arg(&out_file);

        match cmd.output() {
            Ok(output) if output.status.success() => {
                let _ = tx.send(WorkerMessage::Progress(100.0, "示範樣品建立成功！".to_string()));
            }
            Ok(output) => {
                let err = String::from_utf8_lossy(&output.stderr);
                let _ = tx.send(WorkerMessage::Error(format!("示範生成失敗: {}", err)));
            }
            Err(e) => {
                let _ = tx.send(WorkerMessage::Error(format!("執行 FFmpeg 失敗: {}", e)));
            }
        }
    });
}
