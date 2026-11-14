// src/birdview.rs
// 2D 車載 360° 鳥瞰地面全景 (Around View Monitoring - AVM Top View) 無縫無死角拼接模組
// 移植自 CAM 專案 (C:\Users\HPi\Desktop\CAM) 實車等距魚眼 (Equidistant Fisheye) 幾何投影與權重羽化融合演算法：
// - 車體物理坐標系：X ∈ [-3.6, +3.6] m, Y ∈ [-6.0, +5.0] m (地面 Z = 0)
// - 相機等距魚眼投影模型：r = f * (theta + k1 * theta^3), u = cx + r*cos(phi), v = cy + r*sin(phi)
// - 實車 4 路鏡頭標定參數 (Front: pitch 26.4°, Rear: pitch 18.9°, Left: pitch 85.0°, Right: pitch 83.1°)
// - 3D 車身幾何 AABB 遮蔽消除 (ego_boxes)，徹底排除車門、保險桿反光破圖
// - 物理入射角與距離權重融合 (GSD dens * fade)，四路鏡頭無縫平滑羽化，停止線 100% 水平、雙白線 100% 垂直筆直

use egui::{Color32, Mesh, Painter, Pos2, Rect, Stroke, TextureHandle, TextureId, Vec2};
use crate::ffmpeg_worker::CameraParam;
use crate::player::{FRAME_H, FRAME_W};

pub const CAR_TOPVIEW_BYTES: &[u8] = include_bytes!("../car_topview.png");

/// CAM 專案之相機實車物理標定與幾何投影結構體
#[derive(Clone, Copy, Debug)]
#[allow(dead_code)]
pub struct CamCalib {
    pub name: &'static str,
    pub pos: [f32; 3],        // 相機安裝位置 (x, y, z) 公尺 (車體坐標系)
    pub yaw_deg: f32,         // 水平偏航角 (0 = 朝前, 180 = 朝後, -90 = 朝左, 90 = 朝右)
    pub pitch_deg: f32,       // 下俯角度
    pub roll_deg: f32,        // 沿光軸滾轉角
    pub f: f32,               // 等距魚眼焦距 (px/rad)
    pub cx: f32,              // 魚眼光心 X (以 960 象限寬為基準)
    pub cy: f32,              // 魚眼光心 Y (以 540 象限高為基準)
    pub k1: f32,              // 徑向畸變微調係數
    pub theta_max_deg: f32,   // 最大有效可視夾角 (預設 97°)
    pub r_max: f32,           // 成像圓最大半徑 (430 px)
    pub d_min: f32,           // 最小取樣距離
    pub d_max: f32,           // 最大取樣距離
    pub gain: f32,            // 曝光增益
    pub scale_h: f32,         // 水平展開視場比例 (由 out_hfov 控制)
    pub scale_v: f32,         // 垂直展開視場比例 (由 out_vfov 控制)
}

impl CamCalib {
    /// 取得 CAM 專案 calib.json 之預設實車標定參數
    pub fn default_for_cam(idx: usize) -> Self {
        match idx {
            0 => Self {
                name: "front",
                pos: [0.1066667, 2.3333333, 0.82],
                yaw_deg: 4.0,
                pitch_deg: 26.4444,
                roll_deg: 2.2222,
                f: 322.2222,
                cx: 460.0,
                cy: 193.3333,
                k1: 0.0,
                theta_max_deg: 97.0,
                r_max: 430.0,
                d_min: 0.6,
                d_max: 6.5,
                gain: 1.0,
                scale_h: 1.0,
                scale_v: 1.0,
            },
            1 => Self {
                name: "rear",
                pos: [-0.0355556, -2.4666667, 0.75],
                yaw_deg: 180.0,
                pitch_deg: 18.8889,
                roll_deg: 4.0,
                f: 197.5556,
                cx: 482.0,
                cy: 259.0,
                k1: 0.0,
                theta_max_deg: 97.0,
                r_max: 430.0,
                d_min: 0.6,
                d_max: 6.0,
                gain: 1.0,
                scale_h: 1.0,
                scale_v: 1.0,
            },
            2 => Self {
                name: "left",
                pos: [-1.0311111, 0.55, 1.02],
                yaw_deg: -90.0,
                pitch_deg: 85.0,
                roll_deg: -7.5556,
                f: 270.0,
                cx: 500.0,
                cy: 258.0,
                k1: 0.0,
                theta_max_deg: 97.0,
                r_max: 430.0,
                d_min: 0.6,
                d_max: 4.2,
                gain: 1.0,
                scale_h: 1.0,
                scale_v: 1.0,
            },
            _ => Self {
                name: "right",
                pos: [0.93, 0.55, 1.02],
                yaw_deg: 90.0,
                pitch_deg: 83.1111,
                roll_deg: 2.2222,
                f: 270.0,
                cx: 454.0,
                cy: 268.0,
                k1: 0.0,
                theta_max_deg: 97.0,
                r_max: 430.0,
                d_min: 0.6,
                d_max: 4.2,
                gain: 1.0,
                scale_h: 1.0,
                scale_v: 1.0,
            },
        }
    }

    /// 套用使用者在 UI 面板上的即時滑桿微調增量 (Zoom / FOV / Pitch / Yaw / Roll / Offset X / Offset Y / Out H-V FOV)
    pub fn apply_param(&mut self, idx: usize, param: &CameraParam) {
        self.pitch_deg += param.pitch as f32;
        self.yaw_deg += param.yaw as f32;
        self.roll_deg += param.roll as f32;

        // X/Y 軸物理平移微調 (公尺，直接作用於相機車體安裝位置)
        self.pos[0] += param.offset_x;
        self.pos[1] += param.offset_y;

        // 縮放與輸入魚眼 FOV
        let safe_in_fov = (param.in_fov as f32).clamp(100.0, 250.0);
        let def_in_fov = if idx < 2 { 185.0_f32 } else { 195.0_f32 };
        let safe_zoom = param.zoom.clamp(0.3, 4.0);
        self.f = self.f * safe_zoom * (def_in_fov / safe_in_fov);

        // 展開水平 FOV 與垂直 FOV (動態伸縮橫向/縱向展開比例)
        let def_hfov = if idx < 2 { 110.0_f32 } else { 120.0_f32 };
        let def_vfov = if idx < 2 { 80.0_f32 } else { 85.0_f32 };
        self.scale_h = (param.out_hfov as f32 / def_hfov).clamp(0.35, 3.0);
        self.scale_v = (param.out_vfov as f32 / def_vfov).clamp(0.35, 3.0);
    }

    /// 計算相機坐標系在車體坐標系下的三個正交軸向量 (x: 畫面右方, yy: 畫面下方, z: 光軸朝向)
    #[inline]
    pub fn axes(&self) -> ([f32; 3], [f32; 3], [f32; 3]) {
        let y = self.yaw_deg.to_radians();
        let p = self.pitch_deg.to_radians();
        let r = self.roll_deg.to_radians();

        let z = [y.sin() * p.cos(), y.cos() * p.cos(), -p.sin()];
        let x0 = [y.cos(), -y.sin(), 0.0];
        let y0 = [
            z[1] * x0[2] - z[2] * x0[1],
            z[2] * x0[0] - z[0] * x0[2],
            z[0] * x0[1] - z[1] * x0[0],
        ];
        let x = [
            x0[0] * r.cos() + y0[0] * r.sin(),
            x0[1] * r.cos() + y0[1] * r.sin(),
            x0[2] * r.cos() + y0[2] * r.sin(),
        ];
        let yy = [
            -x0[0] * r.sin() + y0[0] * r.cos(),
            -x0[1] * r.sin() + y0[1] * r.cos(),
            -x0[2] * r.sin() + y0[2] * r.cos(),
        ];
        (x, yy, z)
    }

    /// 將地面物理點 (X, Y, 0) 投影至本鏡頭之歸一化 UV 坐標與計算物理融合權重
    #[inline]
    pub fn project_and_weight(
        &self,
        axes: &([f32; 3], [f32; 3], [f32; 3]),
        x_w: f32,
        y_w: f32,
    ) -> Option<(Pos2, f32)> {
        self.project_and_weight_z(axes, x_w, y_w, 0.0)
    }

    /// 將 3D 碗狀地面點 (X, Y, Z) 投影至本鏡頭之歸一化 UV 坐標與計算物理融合權重
    #[inline]
    pub fn project_and_weight_z(
        &self,
        axes: &([f32; 3], [f32; 3], [f32; 3]),
        x_w: f32,
        y_w: f32,
        z_w: f32,
    ) -> Option<(Pos2, f32)> {
        let (xa, ya, za) = axes;
        let d = [x_w - self.pos[0], y_w - self.pos[1], z_w - self.pos[2]];
        let xc = d[0] * xa[0] + d[1] * xa[1] + d[2] * xa[2];
        let yc = d[0] * ya[0] + d[1] * ya[1] + d[2] * ya[2];
        let zc = d[0] * za[0] + d[1] * za[1] + d[2] * za[2];

        // 必須在光軸正前方
        if zc <= 0.01 {
            return None;
        }

        let n = (xc * xc + yc * yc + zc * zc).sqrt();
        if n < self.d_min || n > self.d_max {
            return None;
        }

        let cos_t = (zc / n).clamp(-1.0, 1.0);
        let theta = cos_t.acos();
        if theta > self.theta_max_deg.to_radians() {
            return None;
        }

        let phi = yc.atan2(xc);
        let r = self.f * (theta + self.k1 * theta.powi(3));
        if r > self.r_max {
            return None;
        }

        // 透過 scale_h / scale_v 進行水平與垂直展開視場角即時縮放
        let u_px = self.cx + (r * phi.cos()) / self.scale_h;
        let v_px = self.cy + (r * phi.sin()) / self.scale_v;
        if u_px < 0.0 || u_px > 959.0 || v_px < 0.0 || v_px > 539.0 {
            return None;
        }

        // 3D 車身 AABB 遮蔽檢測 (排除自身車門與保險桿反光)
        if hit_ego_boxes(&self.pos, &[x_w, y_w, z_w]) {
            return None;
        }

        // CAM 專案之 GSD 地面密度權重 (p = 3.0) 與邊緣漸變羽化
        let dz = (self.pos[2] - z_w).abs().max(0.05);
        let dens = (dz / n).clamp(0.0, 1.0).powi(3);
        let fade = ((self.r_max - r) / 45.0).clamp(0.0, 1.0);
        let w = dens * fade * self.gain;

        Some((Pos2::new(u_px / 960.0, v_px / 540.0), w))
    }
}

/// 計算 3D 碗狀曲面地面高度 Z(x, y)：
/// 車身周圍近場維持平坦 (Z = 0.0) 確保停車線/車道線筆直，遠處平滑向上彎曲減少放射狀拉伸。
#[inline]
pub fn bowl_height(x_w: f32, y_w: f32, bowl_strength: f32) -> f32 {
    if bowl_strength <= 0.001 {
        return 0.0;
    }
    let dx = (x_w.abs() - 1.8).max(0.0);
    let dy = (y_w.abs() - 3.6).max(0.0);
    let d_excess = (dx * dx + dy * dy).sqrt();
    (bowl_strength * 0.07 * d_excess * d_excess).min(0.9)
}

/// 雙線性採樣 RGBA 影像像素 (回傳歸一化 [R, G, B] 於 0.0..=1.0)
#[inline]
fn sample_rgb_bilinear(pixels: &[u8], w: usize, h: usize, uv: Pos2) -> Option<[f32; 3]> {
    if pixels.len() < w * h * 4 || w < 2 || h < 2 {
        return None;
    }
    let fx = uv.x.clamp(0.0, 1.0) * (w - 1) as f32;
    let fy = uv.y.clamp(0.0, 1.0) * (h - 1) as f32;
    let x0 = (fx.floor() as usize).min(w - 2);
    let y0 = (fy.floor() as usize).min(h - 2);
    let tx = fx - x0 as f32;
    let ty = fy - y0 as f32;

    let idx00 = (y0 * w + x0) * 4;
    let idx10 = idx00 + 4;
    let idx01 = ((y0 + 1) * w + x0) * 4;
    let idx11 = idx01 + 4;

    let mut out = [0.0_f32; 3];
    for c in 0..3 {
        let v00 = pixels[idx00 + c] as f32;
        let v10 = pixels[idx10 + c] as f32;
        let v01 = pixels[idx01 + c] as f32;
        let v11 = pixels[idx11 + c] as f32;
        let v0 = v00 + (v10 - v00) * tx;
        let v1 = v01 + (v11 - v01) * tx;
        out[c] = (v0 + (v1 - v0) * ty) / 255.0;
    }
    Some(out)
}

/// 建構 4 路 CamCalib 實例與正交軸
fn build_cams_and_axes(params: &[CameraParam; 4]) -> ([CamCalib; 4], [([f32; 3], [f32; 3], [f32; 3]); 4]) {
    let mut cams = [
        CamCalib::default_for_cam(0),
        CamCalib::default_for_cam(1),
        CamCalib::default_for_cam(2),
        CamCalib::default_for_cam(3),
    ];
    for i in 0..4 {
        cams[i].apply_param(i, &params[i]);
    }
    let axes = [cams[0].axes(), cams[1].axes(), cams[2].axes(), cams[3].axes()];
    (cams, axes)
}

/// 一鍵自動對齊結果報告
#[derive(Clone, Copy, Debug)]
pub struct AutoAlignReport {
    pub before_rmse: f32,
    pub after_rmse: f32,
    pub improved_pct: f32,
    pub overlap_samples: usize,
}

/// 預先生成「本車道走廊」重疊區候選取樣點 (|x| <= 2.4m，完全排除相鄰車道車輛與護欄干擾)
fn generate_overlap_candidates() -> Vec<(f32, f32)> {
    let mut pts = Vec::with_capacity(420);
    let nx = 16;
    let ny = 30;
    for iy in 0..=ny {
        let y = -5.5 + (iy as f32 / ny as f32) * 10.2;
        for ix in 0..=nx {
            let x = -2.4 + (ix as f32 / nx as f32) * 4.8;
            if x.abs() <= 0.95 && y.abs() <= 2.40 {
                continue;
            }
            if x.abs() >= 0.75 && y.abs() >= 1.15 {
                pts.push((x, y));
            }
        }
    }
    pts
}

/// 計算「本車道重疊區光度誤差 + 本車道左右標線筆直度與跨鏡頭對齊誤差」：
/// 1) 僅看 |x| <= 2.4m 本車道走廊，完全忽略相鄰車道車輛
/// 2) 以左右鏡頭車側標線為基準 X，要求前後鏡頭遠近車道線保持垂直筆直且無縫對齊
fn eval_alignment_loss(
    frames: &[&[u8]; 4],
    params: &[CameraParam; 4],
    candidates: &[(f32, f32)],
) -> (f32, usize) {
    let (cams, axes) = build_cams_and_axes(params);
    const PAIRS: [(usize, usize); 4] = [(0, 2), (0, 3), (1, 2), (1, 3)];

    let mut total_sq_err = 0.0_f32;
    let mut total_cnt = 0_usize;

    for &(ca, cb) in &PAIRS {
        let mut samples_a = Vec::with_capacity(64);
        let mut samples_b = Vec::with_capacity(64);
        let mut sum_a = 0.0_f32;
        let mut sum_b = 0.0_f32;

        for &(xw, yw) in candidates {
            if let (Some((uva, _)), Some((uvb, _))) = (
                cams[ca].project_and_weight(&axes[ca], xw, yw),
                cams[cb].project_and_weight(&axes[cb], xw, yw),
            ) {
                if let (Some(rgba), Some(rgbb)) = (
                    sample_rgb_bilinear(frames[ca], FRAME_W, FRAME_H, uva),
                    sample_rgb_bilinear(frames[cb], FRAME_W, FRAME_H, uvb),
                ) {
                    let la = 0.299 * rgba[0] + 0.587 * rgba[1] + 0.114 * rgba[2];
                    let lb = 0.299 * rgbb[0] + 0.587 * rgbb[1] + 0.114 * rgbb[2];
                    samples_a.push(la);
                    samples_b.push(lb);
                    sum_a += la;
                    sum_b += lb;
                }
            }
        }

        let n = samples_a.len();
        if n >= 6 {
            let mean_a = sum_a / n as f32;
            let mean_b = sum_b / n as f32;
            for i in 0..n {
                let da = samples_a[i] - mean_a;
                let db = samples_b[i] - mean_b;
                let diff = da - db;
                total_sq_err += diff * diff;
            }
            total_cnt += n;
        }
    }

    if total_cnt < 24 {
        return (1e6, total_cnt);
    }

    let overlap_rmse = (total_sq_err / total_cnt as f32).sqrt() * 255.0;

    // 分區追蹤本車道左右標線 (|x| ∈ [1.05, 2.50]) 並要求前後鏡頭筆直對齊車側基準
    let find_cam_line_xs = |cam_idx: usize, y_start: f32, y_end: f32, steps: usize, side: f32| -> Vec<(f32, f32)> {
        let mut pts = Vec::new();
        for iy in 0..steps {
            let yw = y_start + (iy as f32 / (steps - 1).max(1) as f32) * (y_end - y_start);
            let mut xs_row = [0.0_f32; 21];
            let mut lums_row = [0.0_f32; 21];
            let mut row_sum = 0.0_f32;
            let mut row_cnt = 0_usize;
            for ix in 0..21 {
                let xw = side * (1.05 + (ix as f32 / 20.0) * 1.45);
                if let Some((uv, _)) = cams[cam_idx].project_and_weight(&axes[cam_idx], xw, yw) {
                    if let Some(rgb) = sample_rgb_bilinear(frames[cam_idx], FRAME_W, FRAME_H, uv) {
                        let lum = 0.299 * rgb[0] + 0.587 * rgb[1] + 0.114 * rgb[2];
                        xs_row[ix] = xw;
                        lums_row[ix] = lum;
                        row_sum += lum;
                        row_cnt += 1;
                    }
                }
            }
            if row_cnt >= 10 {
                let mean_l = row_sum / row_cnt as f32;
                let mut best_c = 0.0_f32;
                let mut best_x = 0.0_f32;
                for ix in 0..21 {
                    let c = lums_row[ix] - mean_l;
                    if xs_row[ix] != 0.0 && c > best_c {
                        best_c = c;
                        best_x = xs_row[ix];
                    }
                }
                if best_c > 0.07 {
                    pts.push((yw, best_x));
                }
            }
        }
        pts
    };

    let mut straight_pen = 0.0_f32;
    for &(side, side_cam) in &[(-1.0_f32, 2_usize), (1.0_f32, 3_usize)] {
        let side_pts = find_cam_line_xs(side_cam, -1.8, 1.8, 12, side);
        let ref_x = if side_pts.len() >= 3 {
            side_pts.iter().map(|p| p.1).sum::<f32>() / side_pts.len() as f32
        } else {
            side * 1.65
        };

        if side_pts.len() >= 4 {
            let var = side_pts.iter().map(|p| (p.1 - ref_x).powi(2)).sum::<f32>() / side_pts.len() as f32;
            straight_pen += var.sqrt() * 80.0;
        }

        for &(cam_idx, y0, y1) in &[(0_usize, 2.2_f32, 4.9_f32), (1_usize, -5.5_f32, -2.2_f32)] {
            let pts = find_cam_line_xs(cam_idx, y0, y1, 14, side);
            if pts.len() >= 4 {
                let err_to_ref = (pts.iter().map(|p| (p.1 - ref_x).powi(2)).sum::<f32>() / pts.len() as f32).sqrt();
                straight_pen += err_to_ref * 110.0;
            } else {
                straight_pen += 25.0;
            }
        }
    }

    (overlap_rmse + straight_pen, total_cnt)
}

/// 稀疏直接法 + 本車道直線度優化之自動外參對齊：
/// 忽略相鄰車道車輛干擾，專注讓本車道左右標線在跨鏡頭交界與遠近處皆保持平行筆直。
pub fn auto_align_extrinsics(
    frames: &[Option<Vec<u8>>; 4],
    params: &mut [CameraParam; 4],
) -> Result<AutoAlignReport, String> {
    let f0 = frames[0].as_deref().ok_or("前視鏡頭尚未有畫面影格")?;
    let f1 = frames[1].as_deref().ok_or("後視鏡頭尚未有畫面影格")?;
    let f2 = frames[2].as_deref().ok_or("左側鏡頭尚未有畫面影格")?;
    let f3 = frames[3].as_deref().ok_or("右側鏡頭尚未有畫面影格")?;
    let frame_refs = [f0, f1, f2, f3];

    let candidates = generate_overlap_candidates();
    let (before_loss, before_cnt) = eval_alignment_loss(&frame_refs, params, &candidates);
    if before_cnt < 24 {
        return Err(format!(
            "重疊區有效取樣點不足 ({} 點)，請先調整基礎 FOV 或確認四路畫面已載入",
            before_cnt
        ));
    }

    let mut best_params = *params;
    let mut best_loss = before_loss;
    let mut best_cnt = before_cnt;

    // 多尺度座標下降：角度 (多輪收斂) + 縮放 + 平移
    let angle_steps: &[i32] = &[8, 4, 2, 1];
    let zoom_steps: &[f32] = &[0.08, 0.03];
    let offset_steps: &[f32] = &[0.08, 0.03, 0.01];

    for &ang_step in angle_steps {
        for cam_idx in 0..4 {
            for field in 0..3 {
                loop {
                    let mut improved = false;
                    for &dir in &[-1_i32, 1_i32] {
                        let mut trial = best_params;
                        match field {
                            0 => trial[cam_idx].yaw = (trial[cam_idx].yaw + dir * ang_step).clamp(-10, 10),
                            1 => trial[cam_idx].pitch = (trial[cam_idx].pitch + dir * ang_step).clamp(-35, 35),
                            _ => trial[cam_idx].roll = (trial[cam_idx].roll + dir * ang_step).clamp(-10, 10),
                        }
                        if trial[cam_idx].yaw == best_params[cam_idx].yaw
                            && trial[cam_idx].pitch == best_params[cam_idx].pitch
                            && trial[cam_idx].roll == best_params[cam_idx].roll
                        {
                            continue;
                        }
                        let (loss, cnt) = eval_alignment_loss(&frame_refs, &trial, &candidates);
                        if cnt >= (before_cnt / 2).max(24) && loss < best_loss - 0.02 {
                            best_loss = loss;
                            best_params = trial;
                            best_cnt = cnt;
                            improved = true;
                        }
                    }
                    if !improved {
                        break;
                    }
                }
            }
        }
    }

    for &zoom_step in zoom_steps {
        for cam_idx in 0..4 {
            for &dir in &[-1.0_f32, 1.0_f32] {
                let mut trial = best_params;
                trial[cam_idx].zoom = (trial[cam_idx].zoom + dir * zoom_step).clamp(0.70, 1.40);
                let (loss, cnt) = eval_alignment_loss(&frame_refs, &trial, &candidates);
                if cnt >= (before_cnt / 2).max(24) && loss < best_loss {
                    best_loss = loss;
                    best_params = trial;
                    best_cnt = cnt;
                }
            }
        }
    }

    for &off_step in offset_steps {
        for cam_idx in 0..4 {
            for field in 0..2 {
                for &dir in &[-1.0_f32, 1.0_f32] {
                    let mut trial = best_params;
                    match field {
                        0 => trial[cam_idx].offset_x = (trial[cam_idx].offset_x + dir * off_step).clamp(-0.6, 0.6),
                        _ => trial[cam_idx].offset_y = (trial[cam_idx].offset_y + dir * off_step).clamp(-0.6, 0.6),
                    }
                    let (loss, cnt) = eval_alignment_loss(&frame_refs, &trial, &candidates);
                    if cnt >= (before_cnt / 2).max(24) && loss < best_loss {
                        best_loss = loss;
                        best_params = trial;
                        best_cnt = cnt;
                    }
                }
            }
        }
    }

    *params = best_params;
    let improved_pct = if before_loss > 1e-4 {
        ((before_loss - best_loss) / before_loss * 100.0).max(0.0)
    } else {
        0.0
    };

    Ok(AutoAlignReport {
        before_rmse: before_loss,
        after_rmse: best_loss,
        improved_pct,
        overlap_samples: best_cnt,
    })
}

/// 離線 CLI 驗證輔助：讀取 1920x1080 四象限魚眼影格並輸出升級前後鳥瞰圖對比
pub fn run_verify_birdview_cli(input_path: &str, out_dir: &str) {
    use image::{imageops::FilterType, GenericImageView, Rgba, RgbaImage};

    let src = image::open(input_path).expect("無法開啟輸入圖片");
    let (w, h) = src.dimensions();
    let hw = w / 2;
    let hh = h / 2;

    let rects = [
        (0, 0, hw, hh),   // 0: Front (TL)
        (hw, 0, hw, hh),  // 1: Rear (TR)
        (0, hh, hw, hh),  // 2: Left (BL)
        (hw, hh, hw, hh), // 3: Right (BR)
    ];

    let mut frames_vec: Vec<Vec<u8>> = Vec::with_capacity(4);
    for &(x, y, cw, ch) in &rects {
        let crop = src.crop_imm(x, y, cw, ch);
        let resized = crop.resize_exact(FRAME_W as u32, FRAME_H as u32, FilterType::Triangle);
        frames_vec.push(resized.to_rgba8().into_raw());
    }

    let frames_opt: [Option<Vec<u8>>; 4] = [
        Some(frames_vec[0].clone()),
        Some(frames_vec[1].clone()),
        Some(frames_vec[2].clone()),
        Some(frames_vec[3].clone()),
    ];
    let frame_refs: [&[u8]; 4] = [
        &frames_vec[0],
        &frames_vec[1],
        &frames_vec[2],
        &frames_vec[3],
    ];

    let car_img = image::load_from_memory(CAR_TOPVIEW_BYTES)
        .expect("無法載入 car_topview.png")
        .to_rgba8();

    let render_img = |params: &[CameraParam; 4],
                      gains: &[[f32; 3]; 4],
                      bowl_strength: f32,
                      seam_sharpness: f32|
     -> RgbaImage {
        let out_w = 540_u32;
        let out_h = 825_u32;
        let mut img = RgbaImage::from_pixel(out_w, out_h, Rgba([26, 28, 34, 255]));
        let (cams, axes) = build_cams_and_axes(params);

        let world_w = 7.2_f32;
        let half_w = world_w * 0.5;
        let y_max = 5.0_f32;
        let y_min = -6.0_f32;
        let world_h = y_max - y_min;

        for py in 0..out_h {
            let ty = py as f32 / (out_h - 1) as f32;
            let y_world = y_max - ty * world_h;
            for px in 0..out_w {
                let tx = px as f32 / (out_w - 1) as f32;
                let x_world = -half_w + tx * world_w;
                let z_world = bowl_height(x_world, y_world, bowl_strength);

                let mut acc = [0.0_f32; 3];
                let mut total_w = 0.0_f32;
                for k in 0..4 {
                    if let Some((uv, w)) = cams[k].project_and_weight_z(&axes[k], x_world, y_world, z_world) {
                        if let Some(rgb) = sample_rgb_bilinear(frame_refs[k], FRAME_W, FRAME_H, uv) {
                            let ws = w.powf(seam_sharpness);
                            for c in 0..3 {
                                acc[c] += rgb[c] * 255.0 * gains[k][c] * ws;
                            }
                            total_w += ws;
                        }
                    }
                }
                if total_w > 1e-12 {
                    let r = (acc[0] / total_w).round().clamp(0.0, 255.0) as u8;
                    let g = (acc[1] / total_w).round().clamp(0.0, 255.0) as u8;
                    let b = (acc[2] / total_w).round().clamp(0.0, 255.0) as u8;
                    img.put_pixel(px, py, Rgba([r, g, b, 255]));
                }
            }
        }

        let car_w = ((1.82 / world_w) * out_w as f32).round() as u32;
        let car_h = ((4.70 / world_h) * out_h as f32).round() as u32;
        let car_resized = image::imageops::resize(&car_img, car_w, car_h, FilterType::Triangle);
        let cx = (out_w - car_w) / 2;
        let cy = ((y_max / world_h) * out_h as f32).round() as u32 - car_h / 2;
        image::imageops::overlay(&mut img, &car_resized, cx as i64, cy as i64);
        img
    };

    let mut params = CameraParam::default_presets();
    let before_img = render_img(&params, &[[1.0; 3]; 4], 0.0, 1.0);
    let before_path = format!("{}/road_bev_before.png", out_dir);
    before_img.save(&before_path).expect("儲存 before 失敗");
    println!("Saved {}", before_path);

    match auto_align_extrinsics(&frames_opt, &mut params) {
        Ok(report) => {
            println!(
                "AutoAlignReport: before_rmse={:.3}, after_rmse={:.3}, improved={:.2}%, samples={}",
                report.before_rmse, report.after_rmse, report.improved_pct, report.overlap_samples
            );
            for (i, p) in params.iter().enumerate() {
                println!(
                    "  Cam {}: yaw={}, pitch={}, roll={}, offset_x={:.2}, offset_y={:.2}",
                    i, p.yaw, p.pitch, p.roll, p.offset_x, p.offset_y
                );
            }
        }
        Err(e) => println!("AutoAlign failed: {}", e),
    }

    let mut renderer = BirdViewRenderer::default();
    for g in 1..=12 {
        renderer.update_auto_gain(&frames_opt, &params, g);
    }
    println!("AutoGain RGB: {:?}", renderer.cam_rgb_gains);

    let after_img = render_img(
        &params,
        &renderer.cam_rgb_gains,
        renderer.bowl_strength,
        renderer.seam_sharpness,
    );
    let after_path = format!("{}/road_bev_after.png", out_dir);
    after_img.save(&after_path).expect("儲存 after 失敗");
    println!("Saved {}", after_path);
}

/// 射線與車體 AABB 包圍盒相交檢測 (檢測光線在到達地面 target 之前是否被車體 origin->target 遮蔽)
#[inline]
fn hit_box(origin: &[f32; 3], target: &[f32; 3], box_min: &[f32; 3], box_max: &[f32; 3]) -> bool {
    let mut t0 = 0.0_f32;
    let mut t1 = 1.0_f32;
    for k in 0..3 {
        let d = target[k] - origin[k];
        let lo = box_min[k];
        let hi = box_max[k];
        if d.abs() < 1e-9 {
            if origin[k] < lo || origin[k] > hi {
                return false;
            }
        } else {
            let ta = (lo - origin[k]) / d;
            let tb = (hi - origin[k]) / d;
            let tmin = ta.min(tb);
            let tmax = ta.max(tb);
            t0 = t0.max(tmin);
            t1 = t1.min(tmax);
        }
    }
    t0 < t1 && t0 > 0.02 && t0 < 1.0 && t1 > 0.0
}

/// 實車 3D 車體遮蔽檢測 (車身底盤與車頂兩個幾何盒)
#[inline]
fn hit_ego_boxes(origin: &[f32; 3], target: &[f32; 3]) -> bool {
    let b1_min = [-0.91, -2.35, 0.3];
    let b1_max = [0.91, 2.35, 1.0];
    if hit_box(origin, target, &b1_min, &b1_max) {
        return true;
    }
    let b2_min = [-0.8, -1.35, 1.0];
    let b2_max = [0.8, 0.75, 1.45];
    if hit_box(origin, target, &b2_min, &b2_max) {
        return true;
    }
    false
}

pub struct BirdViewRenderer {
    pub car_texture: Option<TextureHandle>,
    pub show_guides: bool,
    pub seam_lines: bool,
    pub uv_road_split: f32, // 地面遠端取景比率 (預設 0.44)
    pub seam_spread: f32,   // 前後視角扇區展開角度比例 (預設 1.00)
    pub bowl_strength: f32, // 3D 碗狀曲面強度 (0.0 = 純平面, 0.35 = 預設柔和碗狀)
    pub seam_sharpness: f32, // 接縫銳利度 (1.0 = 寬羽化, 2.5 = 預設緊緻融合)
    pub auto_gain: bool,    // 自動亮度/色差增益補償
    pub cam_rgb_gains: [[f32; 3]; 4],
    last_gain_frame_gen: u64,
}

impl Default for BirdViewRenderer {
    fn default() -> Self {
        Self {
            car_texture: None,
            show_guides: false, // 預設隱藏輔助線以展現最自然無縫的實景道路拼接
            seam_lines: false,  // 預設隱藏接縫線以達到無縫視覺效果
            uv_road_split: 0.44,
            seam_spread: 1.00,  // 預設展開至鳥瞰圖外框四個角落 (最大前後視野角度)
            bowl_strength: 0.0, // 預設純平面 Z=0，確保車道線筆直平行不彎曲
            seam_sharpness: 2.5,
            auto_gain: true,
            cam_rgb_gains: [[1.0; 3]; 4],
            last_gain_frame_gen: 0,
        }
    }
}

impl BirdViewRenderer {
    pub fn ensure_car_texture(&mut self, ctx: &egui::Context) -> TextureId {
        if let Some(tex) = &self.car_texture {
            return tex.id();
        }

        let img = image::load_from_memory(CAR_TOPVIEW_BYTES)
            .expect("無法解析內嵌 car_topview.png")
            .to_rgba8();
        let size = [img.width() as _, img.height() as _];
        let pixels = img.into_raw();
        let color_image = egui::ColorImage::from_rgba_unmultiplied(size, &pixels);
        let tex = ctx.load_texture("car_topview_embedded", color_image, egui::TextureOptions::LINEAR);
        let id = tex.id();
        self.car_texture = Some(tex);
        id
    }

    /// 根據 4 路重疊扇區的平均 RGB 統計，自動計算平滑曝光與白平衡增益補償
    pub fn update_auto_gain(
        &mut self,
        frames: &[Option<Vec<u8>>; 4],
        camera_params: &[CameraParam; 4],
        frame_gen: u64,
    ) {
        if !self.auto_gain {
            self.cam_rgb_gains = [[1.0; 3]; 4];
            return;
        }
        if frame_gen == self.last_gain_frame_gen {
            return;
        }
        let (Some(f0), Some(f1), Some(f2), Some(f3)) = (
            frames[0].as_deref(),
            frames[1].as_deref(),
            frames[2].as_deref(),
            frames[3].as_deref(),
        ) else {
            return;
        };
        self.last_gain_frame_gen = frame_gen;

        let frame_refs = [f0, f1, f2, f3];
        let (cams, axes) = build_cams_and_axes(camera_params);
        let candidates = generate_overlap_candidates();

        let mut sum_rgb = [[0.0_f32; 3]; 4];
        let mut counts = [0_usize; 4];

        // 僅在任兩顆鏡頭有重疊的區域統計 RGB，避免因各自看到不同方向物體導致誤判
        for &(xw, yw) in candidates.iter().step_by(2) {
            let mut active = [None; 4];
            let mut n_active = 0_usize;
            for k in 0..4 {
                if let Some((uv, _)) = cams[k].project_and_weight(&axes[k], xw, yw) {
                    active[k] = Some(uv);
                    n_active += 1;
                }
            }
            if n_active >= 2 {
                for k in 0..4 {
                    if let Some(uv) = active[k] {
                        if let Some(rgb) = sample_rgb_bilinear(frame_refs[k], FRAME_W, FRAME_H, uv) {
                            for c in 0..3 {
                                sum_rgb[k][c] += rgb[c];
                            }
                            counts[k] += 1;
                        }
                    }
                }
            }
        }

        let mut mean_rgb = [[0.5_f32; 3]; 4];
        for k in 0..4 {
            if counts[k] < 8 {
                return;
            }
            for c in 0..3 {
                mean_rgb[k][c] = (sum_rgb[k][c] / counts[k] as f32).max(0.05);
            }
        }

        let mut global_c = [0.0_f32; 3];
        for c in 0..3 {
            global_c[c] = (mean_rgb[0][c] + mean_rgb[1][c] + mean_rgb[2][c] + mean_rgb[3][c]) * 0.25;
        }

        let mut target = [[1.0_f32; 3]; 4];
        let mut max_g = 1.0_f32;
        for k in 0..4 {
            for c in 0..3 {
                target[k][c] = (global_c[c] / mean_rgb[k][c]).clamp(0.72, 1.32);
                max_g = max_g.max(target[k][c]);
            }
        }
        // 歸一化使最大值 <= 1.0 (egui Vertex::color 為 0..255)
        let inv_max = 1.0 / max_g;
        for k in 0..4 {
            for c in 0..3 {
                let g = (target[k][c] * inv_max).clamp(0.60, 1.0);
                self.cam_rgb_gains[k][c] = self.cam_rgb_gains[k][c] * 0.75 + g * 0.25;
            }
        }
    }

    /// 檢測滑鼠在鳥瞰圖點擊了哪一個鏡頭扇區 (0: 前視 Front, 1: 後視 Rear, 2: 左側 Left, 3: 右側 Right)
    pub fn check_sector_click(&self, click_pos: Pos2, rect: Rect) -> Option<usize> {
        let half_w = 3.6_f32;
        let y_max = 5.0_f32;
        let y_min = -6.0_f32;

        let norm_x = (click_pos.x - rect.center().x) / (rect.width() * 0.5);
        let x_world = norm_x * half_w;

        let ty = ((click_pos.y - rect.top()) / rect.height()).clamp(0.0, 1.0);
        let y_world = y_max - ty * (y_max - y_min);

        // 車身本體區域 (長 4.7m, 寬 1.82m) 點擊不切換鏡頭
        if x_world.abs() <= 0.91 && y_world.abs() <= 2.35 {
            return None;
        }

        // 依據地面點與車輛中心之方位角與位置選取最近扇區
        let angle = y_world.atan2(x_world).to_degrees(); // 0 = 右, 90 = 前, 180/-180 = 左, -90 = 後
        if (45.0..=135.0).contains(&angle) {
            Some(0) // 前視 Front
        } else if (-135.0..=-45.0).contains(&angle) {
            Some(1) // 後視 Rear
        } else if angle > 135.0 || angle < -135.0 {
            Some(2) // 左側 Left
        } else {
            Some(3) // 右側 Right
        }
    }

    /// 繪製 360° 車載鳥瞰全景圖 (基於 CAM 專案物理幾何投影與權重羽化融合)
    pub fn render(
        &mut self,
        painter: &Painter,
        rect: Rect,
        ctx: &egui::Context,
        textures: &[Option<TextureId>; 4],
        camera_params: &[CameraParam; 4],
        highlight_cam: Option<usize>,
    ) {
        if rect.width() < 50.0 || rect.height() < 50.0 {
            return;
        }

        // 1. 深色瀝青地面底色 (與 CAM 專案 background=[45,45,45] 完美呼應)
        painter.rect_filled(rect, 0.0, Color32::from_rgb(26, 28, 34));

        // 2. 建立 CAM 實車 4 路相機物理校正實例並疊加使用者滑桿即時增量
        let mut cams = [
            CamCalib::default_for_cam(0),
            CamCalib::default_for_cam(1),
            CamCalib::default_for_cam(2),
            CamCalib::default_for_cam(3),
        ];
        for i in 0..4 {
            cams[i].apply_param(i, &camera_params[i]);
        }
        let axes = [cams[0].axes(), cams[1].axes(), cams[2].axes(), cams[3].axes()];

        // 3. CAM 專案車體物理幾何與地面網格映射：
        //    X ∈ [-3.6, +3.6] m (總寬 7.2m)
        //    Y ∈ [-6.0, +5.0] m (總長 11.0m)
        //    車身長 L = 4.7m, 寬 W = 1.82m, 車中心位於地面 (0, 0)
        let world_w = 7.2_f32;
        let world_h = 11.0_f32;
        let half_w = world_w * 0.5;
        let y_max = 5.0_f32;
        let y_min = -6.0_f32;

        let car_w_px = (1.82_f32 / world_w) * rect.width();
        let car_h_px = (4.70_f32 / world_h) * rect.height();
        let car_center_px = Pos2::new(
            rect.center().x,
            rect.top() + (y_max / world_h) * rect.height(),
        );
        let car_rect = Rect::from_center_size(car_center_px, Vec2::new(car_w_px, car_h_px));

        // 4. 構建高密度地面節點網格 (NX=64, NY=88) 進行高精度物理投影與權重融合
        let nx: usize = 64;
        let ny: usize = 88;
        let stride = (nx + 1) as u32;

        // 預先計算各網格節點在各鏡頭下的 (UV, 權重 w)
        struct GridNode {
            pos: Pos2,
            uvs: [Pos2; 4],
            weights: [f32; 4],
            total_w: f32,
        }

        let mut nodes: Vec<GridNode> = Vec::with_capacity((nx + 1) * (ny + 1));

        for iy in 0..=ny {
            let ty = iy as f32 / ny as f32;
            let y_world = y_max - ty * (y_max - y_min);
            let py = rect.top() + ty * rect.height();

            for ix in 0..=nx {
                let tx = ix as f32 / nx as f32;
                let x_world = -half_w + tx * world_w;
                let px = rect.left() + tx * rect.width();

                let mut uvs = [Pos2::ZERO; 4];
                let mut weights = [0.0_f32; 4];
                let mut total_w = 0.0_f32;
                let z_world = bowl_height(x_world, y_world, self.bowl_strength);
                let sharp = self.seam_sharpness.clamp(1.0, 6.0);

                for k in 0..4 {
                    if let Some((uv, w)) = cams[k].project_and_weight_z(&axes[k], x_world, y_world, z_world) {
                        let w_sharp = w.powf(sharp);
                        uvs[k] = uv;
                        weights[k] = w_sharp;
                        total_w += w_sharp;
                    }
                }

                nodes.push(GridNode {
                    pos: Pos2::new(px, py),
                    uvs,
                    weights,
                    total_w,
                });
            }
        }

        // =========================================================================
        // 5. 分層繪製各鏡頭網格 (底層: 左右兩側 Left cam 2 / Right cam 3；頂層: 前後 Front cam 0 / Rear cam 1)
        //    套用接縫銳利化權重歸一化與自動 RGB 曝光/白平衡增益補償
        // =========================================================================
        for &cam_idx in &[2_usize, 3, 0, 1] {
            let Some(tex_id) = textures[cam_idx] else {
                continue;
            };
            let gain = self.cam_rgb_gains[cam_idx];
            let mut mesh = Mesh::with_texture(tex_id);
            for node in &nodes {
                let wk = node.weights[cam_idx];
                let alpha = if node.total_w > 1e-12 {
                    (wk / node.total_w).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let r = (alpha * gain[0] * 255.0).round().clamp(0.0, 255.0) as u8;
                let g = (alpha * gain[1] * 255.0).round().clamp(0.0, 255.0) as u8;
                let b = (alpha * gain[2] * 255.0).round().clamp(0.0, 255.0) as u8;
                let a = (alpha * 255.0).round().clamp(0.0, 255.0) as u8;
                mesh.vertices.push(egui::epaint::Vertex {
                    pos: node.pos,
                    uv: node.uvs[cam_idx],
                    color: Color32::from_rgba_premultiplied(r, g, b, a),
                });
            }
            for iy in 0..ny as u32 {
                for ix in 0..nx as u32 {
                    let i00 = iy * stride + ix;
                    let i10 = i00 + 1;
                    let i01 = (iy + 1) * stride + ix;
                    let i11 = i01 + 1;
                    if nodes[i00 as usize].weights[cam_idx] > 0.0
                        || nodes[i10 as usize].weights[cam_idx] > 0.0
                        || nodes[i01 as usize].weights[cam_idx] > 0.0
                        || nodes[i11 as usize].weights[cam_idx] > 0.0
                    {
                        mesh.add_triangle(i00, i10, i11);
                        mesh.add_triangle(i00, i11, i01);
                    }
                }
            }
            painter.add(mesh);
        }

        // ====== 6. 繪製四角接縫輔助線 (僅於勾選時顯示) ======
        let c_tl = car_rect.left_top();
        let c_tr = car_rect.right_top();
        let c_bl = car_rect.left_bottom();
        let c_br = car_rect.right_bottom();

        if self.seam_lines {
            let seam_stroke = Stroke::new(1.2_f32, Color32::from_rgba_unmultiplied(0, 230, 255, 120));
            painter.line_segment([c_tl, rect.left_top()], seam_stroke);
            painter.line_segment([c_tr, rect.right_top()], seam_stroke);
            painter.line_segment([c_bl, rect.left_bottom()], seam_stroke);
            painter.line_segment([c_br, rect.right_bottom()], seam_stroke);
        }

        // ====== 7. 繪製雷達倒車距離輔助導引線 (僅於勾選時顯示) ======
        if self.show_guides {
            let guide_colors = [
                Color32::from_rgba_unmultiplied(255, 60, 80, 180),  // 0.5m 紅色警戒線
                Color32::from_rgba_unmultiplied(255, 180, 0, 170), // 1.0m 橙黃輔助線
                Color32::from_rgba_unmultiplied(0, 230, 140, 160), // 2.0m 綠色安全線
            ];
            let guide_dists_m = [0.5_f32, 1.0_f32, 2.0_f32];

            for (col, dist_m) in guide_colors.iter().zip(guide_dists_m.iter()) {
                let dist_px = (dist_m / world_h) * rect.height();
                let ry = c_bl.y + dist_px;
                if ry < rect.bottom() - 10.0 {
                    let rx_left = c_bl.x - dist_px * 0.15;
                    let rx_right = c_br.x + dist_px * 0.15;
                    painter.line_segment(
                        [Pos2::new(rx_left, ry), Pos2::new(rx_right, ry)],
                        Stroke::new(1.5_f32, *col),
                    );
                }

                let fy = c_tl.y - dist_px;
                if fy > rect.top() + 10.0 {
                    let fx_left = c_tl.x - dist_px * 0.15;
                    let fx_right = c_tr.x + dist_px * 0.15;
                    painter.line_segment(
                        [Pos2::new(fx_left, fy), Pos2::new(fx_right, fy)],
                        Stroke::new(1.5_f32, *col),
                    );
                }
            }
        }

        // ====== 8. 核心：在正中間繪製高解析度轎車頂視圖案 (Car Top-View) ======
        let car_tex_id = self.ensure_car_texture(ctx);
        painter.rect_filled(car_rect, 8.0, Color32::from_rgba_unmultiplied(14, 18, 26, 255));
        painter.image(
            car_tex_id,
            car_rect,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            Color32::WHITE,
        );

        painter.rect_stroke(
            car_rect,
            8.0,
            Stroke::new(1.2_f32, Color32::from_rgba_unmultiplied(0, 220, 255, 110)),
        );

        // ====== 9. 高亮顯示當前選取的校正扇區 (Selected Sector Highlight) ======
        if let Some(h_idx) = highlight_cam {
            let neon_color = Color32::from_rgb(0, 255, 220);
            let highlight_stroke = Stroke::new(2.5_f32, neon_color);
            let o_tl = rect.left_top();
            let o_tr = rect.right_top();
            let o_bl = rect.left_bottom();
            let o_br = rect.right_bottom();

            match h_idx {
                0 => {
                    painter.line_segment([c_tl, o_tl], highlight_stroke);
                    painter.line_segment([o_tl, o_tr], highlight_stroke);
                    painter.line_segment([o_tr, c_tr], highlight_stroke);
                    painter.line_segment([c_tr, c_tl], highlight_stroke);
                    painter.text(
                        Pos2::new(rect.center().x, rect.top() + 18.0),
                        egui::Align2::CENTER_CENTER,
                        "✏️ 前視鏡頭 (CAM 等距魚眼物理幾何微調中)",
                        egui::FontId::proportional(13.0),
                        neon_color,
                    );
                }
                1 => {
                    painter.line_segment([c_bl, o_bl], highlight_stroke);
                    painter.line_segment([o_bl, o_br], highlight_stroke);
                    painter.line_segment([o_br, c_br], highlight_stroke);
                    painter.line_segment([c_br, c_bl], highlight_stroke);
                    painter.text(
                        Pos2::new(rect.center().x, rect.bottom() - 18.0),
                        egui::Align2::CENTER_CENTER,
                        "✏️ 後視鏡頭 (CAM 等距魚眼物理幾何微調中)",
                        egui::FontId::proportional(13.0),
                        neon_color,
                    );
                }
                2 => {
                    painter.line_segment([c_tl, o_tl], highlight_stroke);
                    painter.line_segment([o_tl, o_bl], highlight_stroke);
                    painter.line_segment([o_bl, c_bl], highlight_stroke);
                    painter.line_segment([c_bl, c_tl], highlight_stroke);
                    painter.text(
                        Pos2::new(o_tl.x + 14.0, rect.center().y),
                        egui::Align2::LEFT_CENTER,
                        "✏️ 左側鏡頭 (CAM 等距魚眼物理幾何微調中)",
                        egui::FontId::proportional(13.0),
                        neon_color,
                    );
                }
                3 => {
                    painter.line_segment([c_tr, o_tr], highlight_stroke);
                    painter.line_segment([o_tr, o_br], highlight_stroke);
                    painter.line_segment([o_br, c_br], highlight_stroke);
                    painter.line_segment([c_br, c_tr], highlight_stroke);
                    painter.text(
                        Pos2::new(o_tr.x - 14.0, rect.center().y),
                        egui::Align2::RIGHT_CENTER,
                        "✏️ 右側鏡頭 (CAM 等距魚眼物理幾何微調中)",
                        egui::FontId::proportional(13.0),
                        neon_color,
                    );
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bowl_height_flat_near_car_and_curved_far() {
        assert_eq!(bowl_height(0.0, 0.0, 0.5), 0.0);
        assert_eq!(bowl_height(1.2, 2.5, 0.5), 0.0);
        assert_eq!(bowl_height(3.5, 4.5, 0.0), 0.0);
        let h_far = bowl_height(3.5, 4.5, 0.5);
        assert!(h_far > 0.0 && h_far <= 1.35);
    }

    #[test]
    fn test_auto_gain_balances_brightness() {
        // 建立 4 張純色假影格，cam 0 較暗 (80), cam 1..3 較亮 (180)
        let make_frame = |val: u8| -> Vec<u8> {
            let mut v = vec![0_u8; FRAME_W * FRAME_H * 4];
            for px in v.chunks_exact_mut(4) {
                px[0] = val;
                px[1] = val;
                px[2] = val;
                px[3] = 255;
            }
            v
        };
        let frames = [
            Some(make_frame(80)),
            Some(make_frame(180)),
            Some(make_frame(180)),
            Some(make_frame(180)),
        ];
        let params = CameraParam::default_presets();
        let mut renderer = BirdViewRenderer::default();
        for gen in 1..=10 {
            renderer.update_auto_gain(&frames, &params, gen);
        }
        // cam 0 的增益應該高於 cam 1
        assert!(renderer.cam_rgb_gains[0][0] > renderer.cam_rgb_gains[1][0]);
    }

    #[test]
    fn test_auto_align_extrinsics_reduces_error_on_synthetic_ground() {
        // 用預設參數在地面畫一個棋盤格紋理，再把 cam 0 yaw 偏移 +4° 後測試能否自動對齊降誤差
        let true_params = CameraParam::default_presets();
        let (cams, axes) = build_cams_and_axes(&true_params);

        let mut raw_frames = [
            vec![128_u8; FRAME_W * FRAME_H * 4],
            vec![128_u8; FRAME_W * FRAME_H * 4],
            vec![128_u8; FRAME_W * FRAME_H * 4],
            vec![128_u8; FRAME_W * FRAME_H * 4],
        ];

        // 將世界地面方格圖案反投影寫入 4 個鏡頭的假影格
        for iy in 0..120 {
            let yw = -5.5 + (iy as f32 / 119.0) * 10.0;
            for ix in 0..90 {
                let xw = -3.4 + (ix as f32 / 89.0) * 6.8;
                let checker = (((xw * 1.5).floor() as i32 + (yw * 1.5).floor() as i32) & 1) == 0;
                let val = if checker { 220_u8 } else { 40_u8 };
                for k in 0..4 {
                    if let Some((uv, _)) = cams[k].project_and_weight(&axes[k], xw, yw) {
                        let px = (uv.x * (FRAME_W - 1) as f32).round() as usize;
                        let py = (uv.y * (FRAME_H - 1) as f32).round() as usize;
                        for dy in -1_isize..=1 {
                            for dx in -1_isize..=1 {
                                let nx = (px as isize + dx).clamp(0, FRAME_W as isize - 1) as usize;
                                let ny = (py as isize + dy).clamp(0, FRAME_H as isize - 1) as usize;
                                let idx = (ny * FRAME_W + nx) * 4;
                                raw_frames[k][idx] = val;
                                raw_frames[k][idx + 1] = val;
                                raw_frames[k][idx + 2] = val;
                                raw_frames[k][idx + 3] = 255;
                            }
                        }
                    }
                }
            }
        }

        let frames = [
            Some(raw_frames[0].clone()),
            Some(raw_frames[1].clone()),
            Some(raw_frames[2].clone()),
            Some(raw_frames[3].clone()),
        ];

        let mut perturbed = true_params;
        perturbed[0].yaw = 4;
        perturbed[2].pitch = -3;

        let report = auto_align_extrinsics(&frames, &mut perturbed).expect("auto align should succeed");
        assert!(report.after_rmse < report.before_rmse);
        assert!(report.overlap_samples >= 24);
    }
}
