// src/player.rs
// 四路高畫質即時串流解碼播放器 (純原生 Rust + FFmpeg Pipe)

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use crossbeam_channel::{bounded, Receiver, Sender};
use egui::{ColorImage, Context, TextureHandle, TextureOptions};

use crate::ffmpeg_worker::get_executable;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

const FRAME_W: usize = 640;
const FRAME_H: usize = 360;
const FRAME_BYTES: usize = FRAME_W * FRAME_H * 4; // RGBA

pub struct FrameData {
    pub role_idx: usize, // 0: front, 1: rear, 2: left, 3: right
    pub pixels: Vec<u8>,
}

pub struct QuadPlayer {
    pub is_playing: bool,
    pub stop_signal: Arc<AtomicBool>,
    pub rx_frames: Receiver<FrameData>,
    tx_frames: Sender<FrameData>,
    pub textures: [Option<TextureHandle>; 4],
    children: Vec<Child>,
    pub current_time_sec: f32,
    pub duration_sec: f32,
    last_tick: Instant,
}

impl QuadPlayer {
    pub fn new() -> Self {
        let (tx, rx) = bounded(16);
        Self {
            is_playing: false,
            stop_signal: Arc::new(AtomicBool::new(false)),
            rx_frames: rx,
            tx_frames: tx,
            textures: [None, None, None, None],
            children: Vec::new(),
            current_time_sec: 0.0,
            duration_sec: 5.0,
            last_tick: Instant::now(),
        }
    }

    pub fn load_and_play(&mut self, paths: [PathBuf; 4]) {
        self.stop();
        self.stop_signal.store(false, Ordering::SeqCst);
        self.is_playing = true;
        self.current_time_sec = 0.0;
        self.last_tick = Instant::now();

        let ffmpeg_bin = get_executable("ffmpeg");

        for (idx, path) in paths.into_iter().enumerate() {
            if !path.exists() {
                continue;
            }

            let stop_flag = self.stop_signal.clone();
            let tx = self.tx_frames.clone();
            let bin = ffmpeg_bin.clone();

            std::thread::spawn(move || {
                let mut cmd = Command::new(&bin);
                #[cfg(windows)]
                cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW: 避免彈出任何 CMD 視窗

                cmd.arg("-stream_loop")
                    .arg("-1") // 無限循環播放
                    .arg("-i")
                    .arg(&path)
                    .args(["-f", "rawvideo", "-pix_fmt", "rgba", "-s", &format!("{}x{}", FRAME_W, FRAME_H), "-r", "25", "pipe:1"])
                    .stdout(Stdio::piped())
                    .stderr(Stdio::null());

                if let Ok(mut child) = cmd.spawn() {
                    if let Some(mut stdout) = child.stdout.take() {
                        let mut buffer = vec![0u8; FRAME_BYTES];

                        while !stop_flag.load(Ordering::Relaxed) {
                            if let Ok(_) = stdout.read_exact(&mut buffer) {
                                let _ = tx.send(FrameData {
                                    role_idx: idx,
                                    pixels: buffer.clone(),
                                });
                                // 限制約 25fps
                                std::thread::sleep(Duration::from_millis(38));
                            } else {
                                break;
                            }
                        }
                    }
                    let _ = child.kill();
                }
            });
        }
    }

    pub fn stop(&mut self) {
        self.stop_signal.store(true, Ordering::SeqCst);
        self.is_playing = false;
        self.children.clear();
    }

    pub fn update_textures(&mut self, ctx: &Context) {
        // 從解碼隊列提取最新影格並更新 GPU 紋理
        while let Ok(frame) = self.rx_frames.try_recv() {
            let color_image = ColorImage::from_rgba_unmultiplied(
                [FRAME_W, FRAME_H],
                &frame.pixels,
            );

            if let Some(tex) = &mut self.textures[frame.role_idx] {
                tex.set(color_image, TextureOptions::LINEAR);
            } else {
                let tex = ctx.load_texture(
                    format!("cam_texture_{}", frame.role_idx),
                    color_image,
                    TextureOptions::LINEAR,
                );
                self.textures[frame.role_idx] = Some(tex);
            }
        }

        if self.is_playing {
            let elapsed = self.last_tick.elapsed().as_secs_f32();
            self.last_tick = Instant::now();
            self.current_time_sec += elapsed;
            if self.current_time_sec > self.duration_sec {
                self.current_time_sec = 0.0;
            }
            ctx.request_repaint();
        }
    }
}
