// src/app.rs
// DashCam360 核心應用狀態機與原生視窗介面 (3D 無縫環景立體碗面強化版)

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use crossbeam_channel::{unbounded, Receiver, Sender};
use eframe::egui::{self, Color32, RichText, Vec2};
use glam::Vec3;

use crate::ffmpeg_worker::{generate_demo_sample, start_processing, ProcessConfig, WorkerMessage};
use crate::player::QuadPlayer;
use crate::renderer3d::{Surround3DViewer, ViewMode};

pub struct DashCamApp {
    // 檔案與路徑
    pub input_path: Option<PathBuf>,
    pub output_dir: PathBuf,

    // 象限映射 (0: TL, 1: TR, 2: BL, 3: BR)
    pub mapping: [String; 4],

    // FFmpeg 參數
    pub in_fov: i32,
    pub out_hfov: i32,

    // 處理進度
    pub is_processing: bool,
    pub progress_pct: f32,
    pub status_message: String,
    cancel_flag: Arc<AtomicBool>,
    tx_worker: Sender<WorkerMessage>,
    rx_worker: Receiver<WorkerMessage>,

    // 3D 檢視器與播放器
    pub viewer: Surround3DViewer,
    pub player: QuadPlayer,
}

impl DashCamApp {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        let (tx, rx) = unbounded();

        let mut app = Self {
            input_path: None,
            output_dir: PathBuf::from("output"),
            mapping: [
                "front".to_string(), // TL
                "rear".to_string(),  // TR
                "left".to_string(),  // BL
                "right".to_string(), // BR
            ],
            in_fov: 180,
            out_hfov: 110,
            is_processing: false,
            progress_pct: 0.0,
            status_message: "就緒".to_string(),
            cancel_flag: Arc::new(AtomicBool::new(false)),
            tx_worker: tx,
            rx_worker: rx,
            viewer: Surround3DViewer::default(),
            player: QuadPlayer::new(),
        };

        // 若 output 目錄已存在上次校正的影片，自動加載體驗
        let f = app.output_dir.join("front.mp4");
        let r = app.output_dir.join("rear.mp4");
        let l = app.output_dir.join("left.mp4");
        let ri = app.output_dir.join("right.mp4");
        if f.exists() && r.exists() && l.exists() && ri.exists() {
            app.player.load_and_play([f, r, l, ri]);
        }

        app
    }

    fn check_worker_messages(&mut self) {
        while let Ok(msg) = self.rx_worker.try_recv() {
            match msg {
                WorkerMessage::Progress(pct, msg_text) => {
                    self.progress_pct = pct;
                    self.status_message = msg_text;
                }
                WorkerMessage::Complete(f, r, l, ri) => {
                    self.is_processing = false;
                    self.status_message = "校正完成！已掛載 4 路動態材質至 3D 空間。".to_string();
                    self.player.load_and_play([f, r, l, ri]);
                }
                WorkerMessage::Error(err) => {
                    self.is_processing = false;
                    self.status_message = format!("錯誤: {}", err);
                }
            }
        }
    }
}

impl eframe::App for DashCamApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.check_worker_messages();
        self.player.update_textures(ctx);

        // 頂部快捷列
        egui::TopBottomPanel::top("top_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("🚘 DashCam360 (Rust 3D 無縫環景)").strong().color(Color32::from_rgb(0, 230, 255)).size(16.0));
                ui.separator();

                ui.label("快捷視角:");
                if ui.button("🔄 360° 自由環視").clicked() {
                    self.viewer.camera.pitch = 35.0;
                    self.viewer.camera.yaw = 45.0;
                    self.viewer.camera.distance = 7.6;
                    self.viewer.camera.look_at = Vec3::new(0.0, 0.5, 0.0);
                }
                if ui.button("🦅 鳥瞰地面全景").clicked() {
                    self.viewer.camera.pitch = 86.0;
                    self.viewer.camera.yaw = 0.0;
                    self.viewer.camera.distance = 9.5;
                    self.viewer.camera.look_at = Vec3::ZERO;
                }
                if ui.button("⬆️ 車前視角").clicked() {
                    self.viewer.camera.pitch = 18.0;
                    self.viewer.camera.yaw = 180.0;
                    self.viewer.camera.distance = 5.2;
                    self.viewer.camera.look_at = Vec3::new(0.0, 0.8, -2.5);
                }
                if ui.button("⬇️ 車後視角").clicked() {
                    self.viewer.camera.pitch = 18.0;
                    self.viewer.camera.yaw = 0.0;
                    self.viewer.camera.distance = 5.2;
                    self.viewer.camera.look_at = Vec3::new(0.0, 0.8, 2.5);
                }
                if ui.button("⬅️ 車左視角").clicked() {
                    self.viewer.camera.pitch = 18.0;
                    self.viewer.camera.yaw = 90.0;
                    self.viewer.camera.distance = 5.2;
                    self.viewer.camera.look_at = Vec3::new(-2.5, 0.8, 0.0);
                }
                if ui.button("➡️ 車右視角").clicked() {
                    self.viewer.camera.pitch = 18.0;
                    self.viewer.camera.yaw = 270.0;
                    self.viewer.camera.distance = 5.2;
                    self.viewer.camera.look_at = Vec3::new(2.5, 0.8, 0.0);
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new("滑鼠左鍵拖曳 360° 旋轉 | 滾輪縮放 | 右鍵平移").color(Color32::GRAY).size(11.0));
                });
            });
        });

        // 底部播放控制面板
        egui::TopBottomPanel::bottom("bottom_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                let btn_text = if self.player.is_playing { "⏸ 暫停" } else { "▶ 播放" };
                if ui.button(RichText::new(btn_text).strong()).clicked() {
                    if self.player.is_playing {
                        self.player.stop();
                    } else {
                        let f = self.output_dir.join("front.mp4");
                        let r = self.output_dir.join("rear.mp4");
                        let l = self.output_dir.join("left.mp4");
                        let ri = self.output_dir.join("right.mp4");
                        self.player.load_and_play([f, r, l, ri]);
                    }
                }

                let cur_s = self.player.current_time_sec as u32;
                let dur_s = self.player.duration_sec as u32;
                ui.label(format!("{:02}:{:02} / {:02}:{:02}", cur_s / 60, cur_s % 60, dur_s / 60, dur_s % 60));

                let progress = self.player.current_time_sec / self.player.duration_sec.max(0.1);
                ui.add(egui::ProgressBar::new(progress).desired_width(ui.available_width() - 360.0));

                // 3D 模式切換按鈕
                ui.label("3D投影模式:");
                if ui.selectable_label(self.viewer.view_mode == ViewMode::SurroundBowl, "立體碗面").clicked() {
                    self.viewer.view_mode = ViewMode::SurroundBowl;
                }
                if ui.selectable_label(self.viewer.view_mode == ViewMode::SeamlessCylinder, "無縫圓柱").clicked() {
                    self.viewer.view_mode = ViewMode::SeamlessCylinder;
                }
                ui.checkbox(&mut self.viewer.show_seam_lines, "接縫線");
            });
        });

        // 右側控制側邊欄
        egui::SidePanel::right("control_panel")
            .min_width(340.0)
            .default_width(360.0)
            .show(ctx, |ui| {
                ui.heading(RichText::new("⚙️ 車載環景校正與 3D 碗面").color(Color32::from_rgb(0, 210, 255)));
                ui.separator();

                // 檔案匯入
                ui.group(|ui| {
                    ui.label(RichText::new("📁 匯入 4 分割魚眼影片").strong());
                    if let Some(path) = &self.input_path {
                        ui.label(format!("檔案: {}", path.file_name().unwrap_or_default().to_string_lossy()));
                    } else {
                        ui.label(RichText::new("未選取檔案 (支援 MP4/MKV/AVI)").color(Color32::GRAY));
                    }

                    ui.horizontal(|ui| {
                        if ui.button("選取本地檔案").clicked() {
                            if let Some(file) = rfd::FileDialog::new()
                                .add_filter("Video", &["mp4", "mkv", "avi", "mov", "ts"])
                                .pick_file()
                            {
                                self.input_path = Some(file);
                            }
                        }
                        if ui.button("🎲 產生測試樣品").clicked() {
                            let demo_path = PathBuf::from("sample_videos/demo_quad_fisheye.mp4");
                            generate_demo_sample(&demo_path, self.tx_worker.clone());
                            self.input_path = Some(demo_path);
                            self.status_message = "正在產生測試樣品...".to_string();
                        }
                    });
                });

                ui.add_space(6.0);

                // 象限映射
                ui.group(|ui| {
                    ui.label(RichText::new("🧭 象限方位映射指派").strong());
                    let roles = ["front", "rear", "left", "right"];
                    let role_names = ["前視 (Front)", "後視 (Rear)", "左側 (Left)", "右側 (Right)"];

                    let labels = ["↖ 左上象限:", "↗ 右上象限:", "↙ 左下象限:", "↘ 右下象限:"];
                    for i in 0..4 {
                        ui.horizontal(|ui| {
                            ui.label(labels[i]);
                            egui::ComboBox::from_id_salt(format!("quad_cb_{}", i))
                                .selected_text(match self.mapping[i].as_str() {
                                    "front" => "前視 (Front)",
                                    "rear" => "後視 (Rear)",
                                    "left" => "左側 (Left)",
                                    _ => "右側 (Right)",
                                })
                                .show_ui(ui, |ui| {
                                    for (r, r_name) in roles.iter().zip(role_names.iter()) {
                                        ui.selectable_value(&mut self.mapping[i], r.to_string(), *r_name);
                                    }
                                });
                        });
                    }
                });

                ui.add_space(6.0);

                // 3D 環景碗面幾何微調
                ui.group(|ui| {
                    ui.label(RichText::new("🌐 3D 環景碗面幾何形狀").strong());
                    ui.horizontal(|ui| {
                        ui.label("地面平鋪半徑:");
                        ui.add(egui::Slider::new(&mut self.viewer.floor_radius, 1.8..=4.0).suffix("m"));
                    });
                    ui.horizontal(|ui| {
                        ui.label("碗壁外擴半徑:");
                        ui.add(egui::Slider::new(&mut self.viewer.wall_radius, 3.0..=6.5).suffix("m"));
                    });
                    ui.horizontal(|ui| {
                        ui.label("碗壁揚起高度:");
                        ui.add(egui::Slider::new(&mut self.viewer.wall_height, 1.2..=3.5).suffix("m"));
                    });
                });

                ui.add_space(6.0);

                // FFmpeg v360 校正參數
                ui.group(|ui| {
                    ui.label(RichText::new("📐 FFmpeg v360 魚眼展開參數").strong());
                    ui.horizontal(|ui| {
                        ui.label("輸入魚眼 FOV:");
                        ui.add(egui::Slider::new(&mut self.in_fov, 140..=220).suffix("°"));
                    });
                    ui.horizontal(|ui| {
                        ui.label("拉直水平 FOV:");
                        ui.add(egui::Slider::new(&mut self.out_hfov, 70..=140).suffix("°"));
                    });
                });

                ui.add_space(6.0);

                // 執行狀態與進度條
                ui.group(|ui| {
                    ui.label(RichText::new("⚡ 處理進度").strong());
                    ui.add(egui::ProgressBar::new(self.progress_pct / 100.0).text(format!("{:.1}%", self.progress_pct)));
                    ui.label(RichText::new(&self.status_message).size(12.0).color(Color32::from_rgb(0, 230, 255)));
                });

                ui.add_space(8.0);

                // 一鍵執行按鈕
                let btn_run = egui::Button::new(RichText::new("🚀 一鍵自動裁切與 v360 魚眼校正").strong().size(14.0))
                    .min_size(Vec2::new(ui.available_width(), 42.0))
                    .fill(Color32::from_rgb(0, 120, 210));

                if ui.add_enabled(!self.is_processing, btn_run).clicked() {
                    if let Some(input) = &self.input_path {
                        self.is_processing = true;
                        self.progress_pct = 2.0;
                        self.status_message = "正在調用 FFmpeg 進行四象限裁切與拉直...".to_string();

                        let config = ProcessConfig {
                            input_path: input.clone(),
                            output_dir: self.output_dir.clone(),
                            mapping: self.mapping.clone(),
                            in_fov: self.in_fov,
                            out_hfov: self.out_hfov,
                            out_vfov: 80,
                        };

                        start_processing(config, self.cancel_flag.clone(), self.tx_worker.clone());
                    } else {
                        self.status_message = "請先匯入影片檔案或點擊產生測試樣品！".to_string();
                    }
                }
            });

        // 中央主 3D 畫布
        egui::CentralPanel::default().show(ctx, |ui| {
            let available_rect = ui.available_rect_before_wrap();
            let response = ui.allocate_rect(available_rect, egui::Sense::drag());

            // 處理滑鼠旋轉與縮放
            self.viewer.handle_input(&response, ui);

            // 繪製純原生 3D 環景
            let painter = ui.painter_at(available_rect);
            painter.rect_filled(available_rect, 0.0, Color32::from_rgb(12, 16, 23));

            let tex_ids = [
                self.player.textures[0].as_ref().map(|t| t.id()),
                self.player.textures[1].as_ref().map(|t| t.id()),
                self.player.textures[2].as_ref().map(|t| t.id()),
                self.player.textures[3].as_ref().map(|t| t.id()),
            ];

            self.viewer.render(&painter, available_rect, &tex_ids);
        });
    }
}
