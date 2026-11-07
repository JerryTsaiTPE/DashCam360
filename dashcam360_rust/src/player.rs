// src/player.rs
// 四路高畫質即時串流解碼播放器 (純原生 Rust + FFmpeg Pipe，支援真實影片長度與暫停/續播/跳轉)

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use crossbeam_channel::{bounded, Receiver, Sender};
use egui::{ColorImage, Context, TextureHandle, TextureOptions};

use crate::ffmpeg_worker::{get_executable, probe_video_duration};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

const FRAME_W: usize = 640;
const FRAME_H: usize = 360;
const FRAME_BYTES: usize = FRAME_W * FRAME_H * 4; // RGBA
const PLAYBACK_FPS: f32 = 25.0;

pub struct FrameData {
    pub role_idx: usize, // 0: front, 1: rear, 2: left, 3: right
    pub frame_idx: u64,
    pub pixels: Vec<u8>,
}

#[derive(Clone, Debug)]
pub enum PlayerSource {
    SeparateFiles([PathBuf; 4]),
    SingleQuadVideo(PathBuf, [String; 4]),
}

pub struct QuadPlayer {
    pub is_playing: bool,
    pub is_paused: bool,
    pub stop_signal: Arc<AtomicBool>,
    pub pause_signal: Arc<AtomicBool>,
    pub rx_frames: Receiver<FrameData>,
    tx_frames: Sender<FrameData>,
    pub textures: [Option<TextureHandle>; 4],
    pub loaded_paths: Option<[PathBuf; 4]>,
    pub current_source: Option<PlayerSource>,
    pub current_time_sec: f32,
    pub duration_sec: f32,
    start_offset_sec: f32,
}

impl QuadPlayer {
    pub fn new() -> Self {
        let (tx, rx) = bounded(16);
        Self {
            is_playing: false,
            is_paused: false,
            stop_signal: Arc::new(AtomicBool::new(false)),
            pause_signal: Arc::new(AtomicBool::new(false)),
            rx_frames: rx,
            tx_frames: tx,
            textures: [None, None, None, None],
            loaded_paths: None,
            current_source: None,
            current_time_sec: 0.0,
            duration_sec: 0.0,
            start_offset_sec: 0.0,
        }
    }

    pub fn load_and_play(&mut self, paths: [PathBuf; 4]) {
        self.load_and_play_at(paths, 0.0);
    }

    pub fn load_and_play_at(&mut self, paths: [PathBuf; 4], start_sec: f32) {
        self.stop();

        // 透過 ffprobe / ffmpeg 偵測真實影片時長
        let mut detected_dur = None;
        for p in &paths {
            if let Some(d) = probe_video_duration(p) {
                detected_dur = Some(d);
                break;
            }
        }
        if let Some(d) = detected_dur {
            self.duration_sec = d.max(0.1);
        } else if self.duration_sec <= 0.1 {
            self.duration_sec = 5.0;
        }

        let clamped_start = if self.duration_sec > 0.1 {
            start_sec.clamp(0.0, (self.duration_sec - 0.05).max(0.0))
        } else {
            0.0
        };

        // 清空殘留影格佇列
        while self.rx_frames.try_recv().is_ok() {}

        let stop_flag = Arc::new(AtomicBool::new(false));
        let pause_flag = Arc::new(AtomicBool::new(false));
        self.stop_signal = stop_flag.clone();
        self.pause_signal = pause_flag.clone();

        self.is_playing = true;
        self.is_paused = false;
        self.start_offset_sec = clamped_start;
        self.current_time_sec = clamped_start;
        self.loaded_paths = Some(paths.clone());
        self.current_source = Some(PlayerSource::SeparateFiles(paths.clone()));

        let ffmpeg_bin = get_executable("ffmpeg");

        for (idx, path) in paths.into_iter().enumerate() {
            if !path.exists() {
                continue;
            }

            let thread_stop = stop_flag.clone();
            let thread_pause = pause_flag.clone();
            let tx = self.tx_frames.clone();
            let bin = ffmpeg_bin.clone();

            std::thread::spawn(move || {
                let mut cmd = Command::new(&bin);
                #[cfg(windows)]
                cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW: 避免彈出任何 CMD 視窗

                if clamped_start > 0.05 {
                    cmd.arg("-ss").arg(format!("{:.2}", clamped_start));
                }

                cmd.arg("-stream_loop")
                    .arg("-1") // 無限循環播放
                    .arg("-i")
                    .arg(&path)
                    .args([
                        "-f",
                        "rawvideo",
                        "-pix_fmt",
                        "rgba",
                        "-s",
                        &format!("{}x{}", FRAME_W, FRAME_H),
                        "-r",
                        "25",
                        "pipe:1",
                    ])
                    .stdout(Stdio::piped())
                    .stderr(Stdio::null());

                if let Ok(mut child) = cmd.spawn() {
                    if let Some(mut stdout) = child.stdout.take() {
                        let mut buffer = vec![0u8; FRAME_BYTES];
                        let mut frame_idx: u64 = 0;
                        let frame_interval = Duration::from_millis(40); // 25 fps = 40ms
                        let mut next_tick = Instant::now() + frame_interval;

                        while !thread_stop.load(Ordering::Relaxed) {
                            // 若處於暫停狀態，等待續播並重置時間基準
                            if thread_pause.load(Ordering::Relaxed) {
                                std::thread::sleep(Duration::from_millis(20));
                                next_tick = Instant::now() + frame_interval;
                                continue;
                            }

                            if stdout.read_exact(&mut buffer).is_ok() {
                                let _ = tx.send(FrameData {
                                    role_idx: idx,
                                    frame_idx,
                                    pixels: buffer.clone(),
                                });
                                frame_idx = frame_idx.wrapping_add(1);

                                let now = Instant::now();
                                if next_tick > now {
                                    std::thread::sleep(next_tick - now);
                                }
                                next_tick += frame_interval;
                                if Instant::now().saturating_duration_since(next_tick) > Duration::from_millis(200) {
                                    next_tick = Instant::now() + frame_interval;
                                }
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

    /// 直接載入單一 4 分割/原始行車記錄器影片，透過 FFmpeg 動態裁切 4 象限並串流解碼
    /// 讓使用者無須等待漫長後台轉碼，即可立即載入完整的完整魚眼影像進行即時預覽與校正！
    pub fn load_quad_video(&mut self, video_path: PathBuf, mapping: [String; 4]) {
        self.load_quad_video_at(video_path, mapping, 0.0);
    }

    pub fn load_quad_video_at(&mut self, video_path: PathBuf, mapping: [String; 4], start_sec: f32) {
        self.stop();

        if let Some(dur) = probe_video_duration(&video_path) {
            self.duration_sec = dur.max(0.1);
        } else if self.duration_sec <= 0.1 {
            self.duration_sec = 5.0;
        }

        let clamped_start = if self.duration_sec > 0.1 {
            start_sec.clamp(0.0, (self.duration_sec - 0.05).max(0.0))
        } else {
            0.0
        };

        while self.rx_frames.try_recv().is_ok() {}

        let stop_flag = Arc::new(AtomicBool::new(false));
        let pause_flag = Arc::new(AtomicBool::new(false));
        self.stop_signal = stop_flag.clone();
        self.pause_signal = pause_flag.clone();

        self.is_playing = true;
        self.is_paused = false;
        self.start_offset_sec = clamped_start;
        self.current_time_sec = clamped_start;
        self.current_source = Some(PlayerSource::SingleQuadVideo(video_path.clone(), mapping.clone()));

        let ffmpeg_bin = get_executable("ffmpeg");

        let quad_crops = [
            "crop=iw/2:ih/2:0:0,scale=640:360",
            "crop=iw/2:ih/2:iw/2:0,scale=640:360",
            "crop=iw/2:ih/2:0:ih/2,scale=640:360",
            "crop=iw/2:ih/2:iw/2:ih/2,scale=640:360",
        ];
        let roles = ["front", "rear", "left", "right"];

        for role_idx in 0..4 {
            let role = roles[role_idx];
            let q_idx = mapping.iter().position(|m| m == role).unwrap_or(role_idx);
            let filter = quad_crops[q_idx];

            let thread_stop = stop_flag.clone();
            let thread_pause = pause_flag.clone();
            let tx = self.tx_frames.clone();
            let bin = ffmpeg_bin.clone();
            let path = video_path.clone();

            std::thread::spawn(move || {
                let mut cmd = Command::new(&bin);
                #[cfg(windows)]
                cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW

                if clamped_start > 0.05 {
                    cmd.arg("-ss").arg(format!("{:.2}", clamped_start));
                }

                cmd.arg("-stream_loop")
                    .arg("-1")
                    .arg("-i")
                    .arg(&path)
                    .args([
                        "-vf",
                        filter,
                        "-f",
                        "rawvideo",
                        "-pix_fmt",
                        "rgba",
                        "-r",
                        "25",
                        "pipe:1",
                    ])
                    .stdout(Stdio::piped())
                    .stderr(Stdio::null());

                if let Ok(mut child) = cmd.spawn() {
                    if let Some(mut stdout) = child.stdout.take() {
                        let mut buffer = vec![0u8; FRAME_BYTES];
                        let mut frame_idx: u64 = 0;
                        let frame_interval = Duration::from_millis(40);
                        let mut next_tick = Instant::now() + frame_interval;

                        while !thread_stop.load(Ordering::Relaxed) {
                            if thread_pause.load(Ordering::Relaxed) {
                                std::thread::sleep(Duration::from_millis(20));
                                next_tick = Instant::now() + frame_interval;
                                continue;
                            }

                            if stdout.read_exact(&mut buffer).is_ok() {
                                let _ = tx.send(FrameData {
                                    role_idx,
                                    frame_idx,
                                    pixels: buffer.clone(),
                                });
                                frame_idx = frame_idx.wrapping_add(1);

                                let now = Instant::now();
                                if next_tick > now {
                                    std::thread::sleep(next_tick - now);
                                }
                                next_tick += frame_interval;
                                if Instant::now().saturating_duration_since(next_tick) > Duration::from_millis(200) {
                                    next_tick = Instant::now() + frame_interval;
                                }
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

    pub fn toggle_pause(&mut self) {
        if !self.is_playing {
            if let Some(src) = self.current_source.clone() {
                match src {
                    PlayerSource::SeparateFiles(paths) => self.load_and_play_at(paths, 0.0),
                    PlayerSource::SingleQuadVideo(path, mapping) => self.load_quad_video_at(path, mapping, 0.0),
                }
            }
            return;
        }
        self.is_paused = !self.is_paused;
        self.pause_signal.store(self.is_paused, Ordering::SeqCst);
    }

    pub fn seek_to(&mut self, target_sec: f32) {
        if let Some(src) = self.current_source.clone() {
            let was_paused = self.is_paused;
            match src {
                PlayerSource::SeparateFiles(paths) => self.load_and_play_at(paths, target_sec),
                PlayerSource::SingleQuadVideo(path, mapping) => self.load_quad_video_at(path, mapping, target_sec),
            }
            if was_paused {
                self.is_paused = true;
                self.pause_signal.store(true, Ordering::SeqCst);
            }
        }
    }

    pub fn stop(&mut self) {
        self.stop_signal.store(true, Ordering::SeqCst);
        self.pause_signal.store(false, Ordering::SeqCst);
        self.is_playing = false;
        self.is_paused = false;
    }

    pub fn update_textures(&mut self, ctx: &Context) {
        let mut received_any = false;
        // 從解碼隊列提取最新影格並更新 GPU 紋理
        while let Ok(frame) = self.rx_frames.try_recv() {
            received_any = true;
            if frame.role_idx == 0 && self.duration_sec > 0.1 {
                let raw_sec = self.start_offset_sec + (frame.frame_idx as f32 / PLAYBACK_FPS);
                self.current_time_sec = raw_sec % self.duration_sec;
            }

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

        if self.is_playing && (!self.is_paused || received_any) {
            ctx.request_repaint_after(Duration::from_millis(16));
        }
    }
}
