// src/ffmpeg_worker.rs
// 負責調用 FFmpeg 進行 4 象限裁切與 v360 魚眼拉直處理，以及產生示範樣品影片

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::io::{BufRead, BufReader};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use crossbeam_channel::Sender;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

pub struct ProcessConfig {
    pub input_path: PathBuf,
    pub output_dir: PathBuf,
    pub mapping: [String; 4], // [TL, TR, BL, BR] -> roles e.g. "front", "rear", "left", "right"
    pub in_fov: i32,
    pub out_hfov: i32,
    pub out_vfov: i32,
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

pub fn start_processing(
    config: ProcessConfig,
    cancel_flag: Arc<AtomicBool>,
    tx: Sender<WorkerMessage>,
) {
    std::thread::spawn(move || {
        let ffmpeg_bin = get_executable("ffmpeg");
        let _ = std::fs::create_dir_all(&config.output_dir);

        let front_out = config.output_dir.join("front.mp4");
        let rear_out = config.output_dir.join("rear.mp4");
        let left_out = config.output_dir.join("left.mp4");
        let right_out = config.output_dir.join("right.mp4");
        let quad_out = config.output_dir.join("quad_rectified.mp4");

        // 象限裁切
        let quad_crops = [
            "crop=iw/2:ih/2:0:0",
            "crop=iw/2:ih/2:iw/2:0",
            "crop=iw/2:ih/2:0:ih/2",
            "crop=iw/2:ih/2:iw/2:ih/2",
        ];

        let mut filter_parts = Vec::new();

        for i in 0..4 {
            let crop = quad_crops[i];
            let role = &config.mapping[i];
            filter_parts.push(format!(
                "[0:v]{crop},v360=input=fisheye:output=rectilinear:ih_fov={}:iv_fov={}:h_fov={}:v_fov={},format=yuv420p,split=2[{role}][{role}_q]",
                config.in_fov, config.in_fov, config.out_hfov, config.out_vfov
            ));
        }

        filter_parts.push("[front_q][rear_q]hstack[top_row]".to_string());
        filter_parts.push("[left_q][right_q]hstack[bot_row]".to_string());
        filter_parts.push("[top_row][bot_row]vstack,format=yuv420p[quad_out]".to_string());

        let filter_complex = filter_parts.join(";");

        let _ = tx.send(WorkerMessage::Progress(2.0, "啟動 FFmpeg 並行裁切與 v360 魚眼展開拉直...".to_string()));

        let mut cmd = Command::new(&ffmpeg_bin);
        #[cfg(windows)]
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW

        cmd.arg("-y")
            .arg("-i")
            .arg(&config.input_path)
            .arg("-filter_complex")
            .arg(&filter_complex)
            .args(["-map", "[front]", "-c:v", "libx264", "-preset", "veryfast", "-crf", "20", "-pix_fmt", "yuv420p", "-movflags", "+faststart"])
            .arg(&front_out)
            .args(["-map", "[rear]",  "-c:v", "libx264", "-preset", "veryfast", "-crf", "20", "-pix_fmt", "yuv420p", "-movflags", "+faststart"])
            .arg(&rear_out)
            .args(["-map", "[left]",  "-c:v", "libx264", "-preset", "veryfast", "-crf", "20", "-pix_fmt", "yuv420p", "-movflags", "+faststart"])
            .arg(&left_out)
            .args(["-map", "[right]", "-c:v", "libx264", "-preset", "veryfast", "-crf", "20", "-pix_fmt", "yuv420p", "-movflags", "+faststart"])
            .arg(&right_out)
            .args(["-map", "[quad_out]", "-c:v", "libx264", "-preset", "veryfast", "-crf", "22", "-pix_fmt", "yuv420p", "-movflags", "+faststart"])
            .arg(&quad_out)
            .arg("-progress")
            .arg("pipe:1")
            .stdout(Stdio::piped())
            .stderr(Stdio::null());

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                let _ = tx.send(WorkerMessage::Error(format!("無法啟動 FFmpeg ({}): {}", ffmpeg_bin.display(), e)));
                return;
            }
        };

        if let Some(stdout) = child.stdout.take() {
            let reader = BufReader::new(stdout);
            let mut curr_fps = "30".to_string();

            for line in reader.lines().flatten() {
                if cancel_flag.load(Ordering::SeqCst) {
                    let _ = child.kill();
                    let _ = tx.send(WorkerMessage::Progress(0.0, "已取消處理".to_string()));
                    return;
                }

                if line.starts_with("fps=") {
                    curr_fps = line.trim_start_matches("fps=").trim().to_string();
                } else if line.starts_with("out_time_us=") {
                    if let Ok(us) = line.trim_start_matches("out_time_us=").trim().parse::<u64>() {
                        let sec = us as f32 / 1_000_000.0;
                        let pct = (sec / 5.0 * 100.0).clamp(5.0, 98.0);
                        let _ = tx.send(WorkerMessage::Progress(pct, format!("處理中: {:.1}% ({:.1}s, @{}fps)", pct, sec, curr_fps)));
                    }
                }
            }
        }

        match child.wait() {
            Ok(status) if status.success() => {
                let _ = tx.send(WorkerMessage::Progress(100.0, "v360 魚眼校正與 4 路分離完成！".to_string()));
                let _ = tx.send(WorkerMessage::Complete(front_out, rear_out, left_out, right_out));
            }
            Ok(status) => {
                let _ = tx.send(WorkerMessage::Error(format!("FFmpeg 執行結束但代碼異常: {:?}", status.code())));
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
