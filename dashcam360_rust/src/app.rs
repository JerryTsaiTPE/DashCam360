// src/app.rs
// DashCam360 核心應用狀態機與原生視窗介面
// 支援雙分頁模式：
// 1. 🎬 360° AVM 同步播放分頁 (左側鳥瞰全景、中間同步主視角、最右側視角切換與完整控制)
// 2. 🦅 鳥瞰圖拼接專用校正分頁 (大鳥瞰圖、直接點選扇區跳出小拖動 bar 微調縮放/俯仰/偏航/滾轉，0ms 即時預覽)

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use crossbeam_channel::{unbounded, Receiver, Sender};
use eframe::egui::{self, Color32, Mesh, Pos2, Rect, RichText, Stroke, Vec2};

use crate::birdview::{auto_align_extrinsics, BirdViewRenderer};
use crate::ffmpeg_worker::{
    generate_demo_sample, probe_video_duration, start_processing, CameraParam, ProcessConfig,
    WorkerMessage,
};
use crate::player::QuadPlayer;
use crate::renderer3d::Surround3DViewer;

fn format_time_mm_ss(sec: f32) -> String {
    let s = sec.max(0.0);
    let mins = (s as u32) / 60;
    let rem_s = s - (mins as f32 * 60.0);
    format!("{:02}:{:04.1}", mins, rem_s)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AppTab {
    Playback360,   // 360 同步播放與視角切換
    BirdViewCalib, // 鳥瞰圖拼接專用校正
    RawVideo,      // 未裁切/未調整之原始影片播放
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CenterViewMode {
    Front, // 前視畫面
    Rear,  // 後視畫面
    Left,  // 左側畫面
    Right, // 右側畫面
}

pub struct DashCamApp {
    // 當前主分頁
    pub current_tab: AppTab,

    // 原始影片播放分頁之單鏡頭放大模式 (None: 2x2 四畫面全覽, Some(0..3): 單路放大)
    pub raw_selected_cam: Option<usize>,

    // 檔案與路徑
    pub input_path: Option<PathBuf>,
    pub input_duration_sec: Option<f32>,
    pub output_dir: PathBuf,

    // 象限映射 (0: TL, 1: TR, 2: BL, 3: BR)
    pub mapping: [String; 4],

    // 前後左右 4 顆鏡頭之獨立廣角、縮放與角度修正參數 (單路預覽專用)
    pub camera_params: [CameraParam; 4],
    // 鳥瞰圖專用拼接校正參數 (鳥瞰圖拼接專用校正分頁獨立控制，不影響單路預覽，也不受單路微調干擾)
    pub birdview_camera_params: [CameraParam; 4],
    pub selected_cam_idx: usize,

    // 鳥瞰圖校正專用小拖動 bar 狀態
    pub show_calib_floating_bar: bool,

    // 播放與視角模式
    pub center_view: CenterViewMode,
    pub show_horizon_crosshair: bool, // 是否在主視角顯示水平校準十字準星

    // 處理進度
    pub is_processing: bool,
    pub progress_pct: f32,
    pub status_message: String,
    cancel_flag: Arc<AtomicBool>,
    tx_worker: Sender<WorkerMessage>,
    rx_worker: Receiver<WorkerMessage>,

    // 渲染器與播放器
    pub birdview: BirdViewRenderer,
    pub viewer: Surround3DViewer,
    pub player: QuadPlayer,
}

impl DashCamApp {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        let (tx, rx) = unbounded();

        let mut app = Self {
            current_tab: AppTab::Playback360,
            raw_selected_cam: None,
            input_path: None,
            input_duration_sec: None,
            output_dir: PathBuf::from("output"),
            mapping: [
                "front".to_string(), // TL (左上: 前方)
                "rear".to_string(),  // TR (右上: 後方)
                "left".to_string(),  // BL (左下: 左側)
                "right".to_string(), // BR (右下: 右側)
            ],
            camera_params: CameraParam::default_presets(),
            birdview_camera_params: CameraParam::default_presets(),
            selected_cam_idx: 0,
            show_calib_floating_bar: true,
            center_view: CenterViewMode::Front,
            show_horizon_crosshair: false,
            is_processing: false,
            progress_pct: 0.0,
            status_message: "就緒 (已預設套用 default.json 校正參數)".to_string(),
            cancel_flag: Arc::new(AtomicBool::new(false)),
            tx_worker: tx,
            rx_worker: rx,
            birdview: BirdViewRenderer::default(),
            viewer: Surround3DViewer::default(),
            player: QuadPlayer::new(),
        };

        // 啟動時預設套用 default.json 校正參數
        if app.load_default_profile() {
            app.status_message = "就緒 (已預設套用 default.json 校正參數)".to_string();
        }

        // 優先載入四路原始魚眼影片進行即時 4 象限串流，確保鳥瞰圖與即時預覽不會被二次扭曲
        let demo = PathBuf::from("sample_videos/demo_quad_fisheye.mp4");
        if demo.exists() {
            app.player.load_quad_video(demo.clone(), app.mapping.clone());
            app.input_path = Some(demo);
            app.status_message = "已載入實車四路魚眼串流並套用 default.json 預設參數".to_string();
        } else {
            let f = app.output_dir.join("front.mp4");
            let r = app.output_dir.join("rear.mp4");
            let l = app.output_dir.join("left.mp4");
            let ri = app.output_dir.join("right.mp4");
            if f.exists() && r.exists() && l.exists() && ri.exists() {
                app.player.load_and_play([f, r, l, ri]);
            }
        }

        app
    }

    fn check_worker_messages(&mut self) {
        while let Ok(msg) = self.rx_worker.try_recv() {
            match msg {
                WorkerMessage::Progress(pct, msg_text) => {
                    self.progress_pct = pct;
                    self.status_message = msg_text;
                    if pct >= 100.0 {
                        if let Some(p) = &self.input_path {
                            if self.input_duration_sec.is_none() {
                                self.input_duration_sec = probe_video_duration(p);
                            }
                            if !self.player.is_playing && p.exists() {
                                self.player.load_quad_video(p.clone(), self.mapping.clone());
                            }
                        }
                    }
                }
                WorkerMessage::Complete(_f, _r, _l, _ri) => {
                    self.is_processing = false;
                    // 保持播放器使用原始 4 象限魚眼串流，避免鳥瞰圖與即時預覽發生二次變形
                    if let Some(p) = &self.input_path {
                        if !self.player.is_playing && p.exists() {
                            self.player.load_quad_video(p.clone(), self.mapping.clone());
                        }
                    }
                    self.status_message = format!(
                        "校正影片已成功匯出至 output 資料夾！(時長: {})",
                        format_time_mm_ss(self.player.duration_sec)
                    );
                }
                WorkerMessage::Error(err) => {
                    self.is_processing = false;
                    self.status_message = format!("錯誤: {}", err);
                }
            }
        }
    }

    /// 匯出全車完整參數檔 (包含單路預覽角度、鳥瞰圖拼接校正、視角展開張角、路面裁切高度、3D車體幾何等)
    pub fn export_calibration_profile(&mut self) {
        if let Some(file) = rfd::FileDialog::new()
            .set_title("匯出 DashCam360 校正參數檔")
            .add_filter("DashCam360 參數設定檔 (*.json)", &["json"])
            .set_file_name("dashcam360_calibration.json")
            .save_file()
        {
            let profile = CalibrationProfile {
                profile_version: "2.0".to_string(),
                mapping: self.mapping.clone(),
                preview_camera_params: self.camera_params,
                birdview_camera_params: self.birdview_camera_params,
                birdview_seam_spread: self.birdview.seam_spread,
                birdview_uv_road_split: self.birdview.uv_road_split,
                birdview_show_guides: self.birdview.show_guides,
                birdview_seam_lines: self.birdview.seam_lines,
                birdview_bowl_strength: self.birdview.bowl_strength,
                birdview_seam_sharpness: self.birdview.seam_sharpness,
                birdview_auto_gain: self.birdview.auto_gain,
                viewer_mirror_offset_z: self.viewer.mirror_offset_z,
                viewer_floor_radius: self.viewer.floor_radius,
                viewer_wall_radius: self.viewer.wall_radius,
                viewer_wall_height: self.viewer.wall_height,
            };
            match serde_json::to_string_pretty(&profile) {
                Ok(json_str) => {
                    if let Err(e) = std::fs::write(&file, json_str) {
                        self.status_message = format!("匯出失敗: {}", e);
                    } else {
                        self.status_message = format!(
                            "已成功匯出參數檔: {}",
                            file.file_name().unwrap_or_default().to_string_lossy()
                        );
                    }
                }
                Err(e) => {
                    self.status_message = format!("序列化失敗: {}", e);
                }
            }
        }
    }

    fn apply_calibration_profile(&mut self, profile: CalibrationProfile) {
        self.mapping = profile.mapping;
        self.camera_params = profile.preview_camera_params;
        self.birdview_camera_params = profile.birdview_camera_params;
        self.birdview.seam_spread = profile.birdview_seam_spread;
        self.birdview.uv_road_split = profile.birdview_uv_road_split;
        self.birdview.show_guides = profile.birdview_show_guides;
        self.birdview.seam_lines = profile.birdview_seam_lines;
        self.birdview.bowl_strength = profile.birdview_bowl_strength;
        self.birdview.seam_sharpness = profile.birdview_seam_sharpness;
        self.birdview.auto_gain = profile.birdview_auto_gain;
        self.viewer.mirror_offset_z = profile.viewer_mirror_offset_z;
        self.viewer.floor_radius = profile.viewer_floor_radius;
        self.viewer.wall_radius = profile.viewer_wall_radius;
        self.viewer.wall_height = profile.viewer_wall_height;
    }

    /// 啟動時搜尋並套用 default.json，若磁碟上找不到則回退使用內嵌的 sample/default.json
    fn load_default_profile(&mut self) -> bool {
        let mut candidates = vec![
            PathBuf::from("default.json"),
            PathBuf::from("sample/default.json"),
        ];
        if let Ok(exe_path) = std::env::current_exe() {
            if let Some(exe_dir) = exe_path.parent() {
                candidates.push(exe_dir.join("default.json"));
                candidates.push(exe_dir.join("sample/default.json"));
                if let Some(p1) = exe_dir.parent() {
                    candidates.push(p1.join("sample/default.json"));
                    if let Some(p2) = p1.parent() {
                        candidates.push(p2.join("sample/default.json"));
                        if let Some(p3) = p2.parent() {
                            candidates.push(p3.join("sample/default.json"));
                        }
                    }
                }
            }
        }

        for path in candidates {
            if path.is_file() {
                if let Ok(json_str) = std::fs::read_to_string(&path) {
                    if let Ok(profile) = serde_json::from_str::<CalibrationProfile>(&json_str) {
                        self.apply_calibration_profile(profile);
                        return true;
                    }
                }
            }
        }

        const EMBEDDED_DEFAULT_JSON: &str = include_str!("../default.json");
        if let Ok(profile) = serde_json::from_str::<CalibrationProfile>(EMBEDDED_DEFAULT_JSON) {
            self.apply_calibration_profile(profile);
            return true;
        }
        false
    }

    fn load_video_file(&mut self, file: PathBuf) {
        let d = probe_video_duration(&file);
        self.input_duration_sec = d;
        self.input_path = Some(file.clone());
        self.player.load_quad_video(file.clone(), self.mapping.clone());
        let fname = file.file_name().unwrap_or_default().to_string_lossy();
        self.status_message = format!("已載入影片四路完整魚眼串流: {}", fname);
    }

    fn is_supported_video_file(path: &std::path::Path) -> bool {
        let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
            return false;
        };
        matches!(
            ext.to_ascii_lowercase().as_str(),
            "mp4" | "mkv" | "avi" | "mov" | "ts" | "m4v" | "webm"
        )
    }

    fn handle_drag_and_drop(&mut self, ctx: &egui::Context) {
        let (hovered, dropped) = ctx.input(|i| (i.raw.hovered_files.clone(), i.raw.dropped_files.clone()));

        if !hovered.is_empty() {
            let screen_rect = ctx.screen_rect();
            let painter = ctx.layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::new("drag_drop_overlay"),
            ));
            painter.rect_filled(
                screen_rect,
                0.0,
                Color32::from_rgba_unmultiplied(10, 18, 28, 195),
            );
            painter.rect_stroke(
                screen_rect.shrink(18.0),
                12.0,
                Stroke::new(2.5_f32, Color32::from_rgb(0, 220, 255)),
            );

            let label = hovered
                .first()
                .and_then(|f| f.path.as_ref())
                .and_then(|p| p.file_name())
                .map(|n| format!("📂 放開滑鼠以載入影片：{}", n.to_string_lossy()))
                .unwrap_or_else(|| "📂 放開滑鼠以載入影片檔案".to_string());

            painter.text(
                screen_rect.center(),
                egui::Align2::CENTER_CENTER,
                label,
                egui::FontId::proportional(24.0),
                Color32::from_rgb(0, 235, 255),
            );
        }

        if !dropped.is_empty() {
            let mut loaded = false;
            for item in &dropped {
                if let Some(path) = &item.path {
                    if Self::is_supported_video_file(path) {
                        self.load_video_file(path.clone());
                        loaded = true;
                        break;
                    }
                }
            }
            if !loaded {
                self.status_message = "不支援的檔案格式，請拖曳影片檔案 (.mp4, .mkv, .avi, .mov, .ts)".to_string();
            }
        }
    }

    /// 匯入全車完整參數檔
    pub fn import_calibration_profile(&mut self) {
        if let Some(file) = rfd::FileDialog::new()
            .set_title("匯入 DashCam360 校正參數檔")
            .add_filter("DashCam360 參數設定檔 (*.json)", &["json"])
            .pick_file()
        {
            match std::fs::read_to_string(&file) {
                Ok(json_str) => match serde_json::from_str::<CalibrationProfile>(&json_str) {
                    Ok(profile) => {
                        self.apply_calibration_profile(profile);
                        self.status_message = format!(
                            "已成功匯入參數檔: {}",
                            file.file_name().unwrap_or_default().to_string_lossy()
                        );
                    }
                    Err(e) => {
                        self.status_message = format!("解析參數檔失敗: {}", e);
                    }
                },
                Err(e) => {
                    self.status_message = format!("讀取檔案失敗: {}", e);
                }
            }
        }
    }
}

fn default_bowl_strength() -> f32 { 0.0 }
fn default_seam_sharpness() -> f32 { 2.5 }
fn default_auto_gain() -> bool { true }

/// 匯出/匯入用的全車參數設定結構
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct CalibrationProfile {
    pub profile_version: String,
    pub mapping: [String; 4],
    pub preview_camera_params: [CameraParam; 4],
    pub birdview_camera_params: [CameraParam; 4],
    pub birdview_seam_spread: f32,
    pub birdview_uv_road_split: f32,
    pub birdview_show_guides: bool,
    pub birdview_seam_lines: bool,
    #[serde(default = "default_bowl_strength")]
    pub birdview_bowl_strength: f32,
    #[serde(default = "default_seam_sharpness")]
    pub birdview_seam_sharpness: f32,
    #[serde(default = "default_auto_gain")]
    pub birdview_auto_gain: bool,
    pub viewer_mirror_offset_z: f32,
    pub viewer_floor_radius: f32,
    pub viewer_wall_radius: f32,
    pub viewer_wall_height: f32,
}

impl eframe::App for DashCamApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.check_worker_messages();
        self.handle_drag_and_drop(ctx);
        self.player.update_textures(ctx);
        self.birdview.update_auto_gain(
            &self.player.latest_frames,
            &self.birdview_camera_params,
            self.player.frame_gen,
        );

        let tex_ids = [
            self.player.textures[0].as_ref().map(|t| t.id()),
            self.player.textures[1].as_ref().map(|t| t.id()),
            self.player.textures[2].as_ref().map(|t| t.id()),
            self.player.textures[3].as_ref().map(|t| t.id()),
        ];

        // 頂部導航列 (分頁切換與狀態)
        egui::TopBottomPanel::top("top_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading(
                    RichText::new("🚗 DashCam360")
                        .color(Color32::from_rgb(0, 220, 255))
                        .size(15.0),
                );
                ui.separator();

                // 核心分頁切換按鈕
                let tab_play_sel = self.current_tab == AppTab::Playback360;
                let tab_calib_sel = self.current_tab == AppTab::BirdViewCalib;
                let tab_raw_sel = self.current_tab == AppTab::RawVideo;

                if ui
                    .selectable_label(
                        tab_play_sel,
                        RichText::new("🎬 360° AVM 同步播放").strong().size(13.0),
                    )
                    .clicked()
                {
                    self.current_tab = AppTab::Playback360;
                }
                if ui
                    .selectable_label(
                        tab_calib_sel,
                        RichText::new("🦅 鳥瞰圖拼接專用校正").strong().size(13.0),
                    )
                    .clicked()
                {
                    self.current_tab = AppTab::BirdViewCalib;
                    self.show_calib_floating_bar = true;
                }
                if ui
                    .selectable_label(
                        tab_raw_sel,
                        RichText::new("📹 原始四路魚眼影像 (未裁切)").strong().size(13.0),
                    )
                    .clicked()
                {
                    self.current_tab = AppTab::RawVideo;
                }

                ui.separator();
                ui.label(
                    RichText::new("⚡ 60 FPS 零延遲即時預覽（拖動滑桿即刻反應）")
                        .color(Color32::from_rgb(0, 255, 180))
                        .size(12.0),
                );

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let status_col = if self.is_processing {
                        Color32::from_rgb(255, 190, 0)
                    } else {
                        Color32::from_rgb(0, 220, 255)
                    };
                    ui.label(RichText::new(&self.status_message).color(status_col).size(12.0));
                });
            });
        });

        // 底部播放控制面板
        egui::TopBottomPanel::bottom("bottom_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                let is_active_playing = self.player.is_playing && !self.player.is_paused;
                let btn_text = if is_active_playing { "⏸ 暫停" } else { "▶ 播放" };
                if ui.button(RichText::new(btn_text).strong().size(13.0)).clicked() {
                    self.player.toggle_pause();
                }

                if ui.button("⏮ 重播").on_hover_text("從 00:00 重新播放").clicked() {
                    self.player.seek_to(0.0);
                }

                let dur_s = if self.player.duration_sec > 0.1 {
                    self.player.duration_sec
                } else {
                    self.input_duration_sec.unwrap_or(0.0)
                };
                let cur_s = self.player.current_time_sec.clamp(0.0, dur_s.max(0.0));

                ui.label(
                    RichText::new(format!(
                        "{} / {}",
                        format_time_mm_ss(cur_s),
                        format_time_mm_ss(dur_s)
                    ))
                    .monospace()
                    .strong(),
                );

                // 可拖曳與即時進度條
                let max_dur = dur_s.max(0.1);
                let mut seek_val = cur_s;
                let slider_width = (ui.available_width() - 360.0).max(120.0);
                ui.spacing_mut().slider_width = slider_width;
                let slider_resp = ui.add_enabled(
                    self.player.is_playing && dur_s > 0.1,
                    egui::Slider::new(&mut seek_val, 0.0..=max_dur)
                        .show_value(false)
                        .trailing_fill(true),
                );
                if slider_resp.drag_stopped() || (slider_resp.changed() && !slider_resp.dragged()) {
                    self.player.seek_to(seek_val);
                }

                ui.separator();
                ui.checkbox(&mut self.birdview.show_guides, "倒車導引線");
                ui.checkbox(&mut self.birdview.seam_lines, "接縫線");
                ui.checkbox(&mut self.show_horizon_crosshair, "校準地平十字線");
            });
        });

        // 根據當前分頁渲染不同介面
        match self.current_tab {
            AppTab::Playback360 => {
                self.render_playback_tab(ctx, &tex_ids);
            }
            AppTab::BirdViewCalib => {
                self.render_birdview_calib_tab(ctx, &tex_ids);
            }
            AppTab::RawVideo => {
                self.render_raw_video_tab(ctx, &tex_ids);
            }
        }
    }
}

impl DashCamApp {
    /// 渲染 分頁 1: 360° AVM 同步播放 (左側鳥瞰、中間主視角、右側視角切換與面板)
    fn render_playback_tab(&mut self, ctx: &egui::Context, tex_ids: &[Option<egui::TextureId>; 4]) {
        // 右側控制面板
        egui::SidePanel::right("right_panel")
            .min_width(290.0)
            .default_width(320.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| {
                    ui.add_space(4.0);
                    ui.heading(
                        RichText::new("🎯 主視角切換 (方位對應)")
                            .color(Color32::from_rgb(0, 220, 255))
                            .size(15.0),
                    );
                    ui.add_space(2.0);

                    // 前後左右對應幾何十字導航佈局
                    ui.vertical_centered(|ui| {
                        let total_w = ui.available_width();
                        let btn_w_half = ((total_w - 8.0) / 2.0).max(120.0);
                        let btn_w_center = (total_w * 0.72).clamp(160.0, 240.0);
                        let btn_h = 36.0;

                        // 1. 上方：前視視角 (Front)
                        let front_sel = self.center_view == CenterViewMode::Front;
                        if ui.add_sized(
                            [btn_w_center, btn_h],
                            egui::SelectableLabel::new(
                                front_sel,
                                RichText::new("⬆️ 🚗 前視視角 (Front)").strong().size(13.5),
                            ),
                        ).clicked() {
                            self.center_view = CenterViewMode::Front;
                            self.selected_cam_idx = 0;
                        }

                        ui.add_space(4.0);

                        // 2. 中間排：左側視角 (Left) 與 右側視角 (Right)
                        ui.horizontal(|ui| {
                            let left_sel = self.center_view == CenterViewMode::Left;
                            if ui.add_sized(
                                [btn_w_half, btn_h],
                                egui::SelectableLabel::new(
                                    left_sel,
                                    RichText::new("⬅️ 👈 左側 (Left)").strong().size(13.0),
                                ),
                            ).clicked() {
                                self.center_view = CenterViewMode::Left;
                                self.selected_cam_idx = 2;
                            }

                            let right_sel = self.center_view == CenterViewMode::Right;
                            if ui.add_sized(
                                [btn_w_half, btn_h],
                                egui::SelectableLabel::new(
                                    right_sel,
                                    RichText::new("👉 右側 (Right) ➡️").strong().size(13.0),
                                ),
                            ).clicked() {
                                self.center_view = CenterViewMode::Right;
                                self.selected_cam_idx = 3;
                            }
                        });

                        ui.add_space(4.0);

                        // 3. 下方：後視視角 (Rear)
                        let rear_sel = self.center_view == CenterViewMode::Rear;
                        if ui.add_sized(
                            [btn_w_center, btn_h],
                            egui::SelectableLabel::new(
                                rear_sel,
                                RichText::new("⬇️ 🔙 後視視角 (Rear)").strong().size(13.5),
                            ),
                        ).clicked() {
                            self.center_view = CenterViewMode::Rear;
                            self.selected_cam_idx = 1;
                        }
                    });

                    ui.add_space(8.0);
                    ui.separator();

                    // 前後左右鏡頭廣角、縮放與角度修正 (僅作用於單路視角預覽)
                    ui.collapsing(
                        RichText::new("📐 前後左右廣角與即時角度修正")
                            .color(Color32::from_rgb(0, 230, 255))
                            .strong(),
                        |ui| {
                            ui.label(
                                RichText::new("🟢 單路視角微調：滑桿僅即時反應於右側主視角，鳥瞰圖由校正分頁獨立控制")
                                    .color(Color32::from_rgb(0, 255, 180))
                                    .size(11.0),
                            );

                            ui.horizontal_wrapped(|ui| {
                                ui.selectable_value(&mut self.selected_cam_idx, 0, "前視");
                                ui.selectable_value(&mut self.selected_cam_idx, 1, "後視");
                                ui.selectable_value(&mut self.selected_cam_idx, 2, "左側");
                                ui.selectable_value(&mut self.selected_cam_idx, 3, "右側");
                            });

                            let cam_names = ["前視鏡頭 (Front)", "後視鏡頭 (Rear)", "左後視鏡頭 (Left)", "右後視鏡頭 (Right)"];
                            let idx = self.selected_cam_idx;
                            let p = &mut self.camera_params[idx];

                            ui.add_space(2.0);
                            ui.label(RichText::new(format!("當前調整: {}", cam_names[idx])).color(Color32::from_rgb(255, 215, 0)).strong());

                            ui.label(RichText::new("【縮放與魚眼廣角 (FOV)】").strong());
                            ui.horizontal(|ui| {
                                ui.label("🔍 縮放 (Zoom):");
                                ui.add(egui::Slider::new(&mut p.zoom, 0.4..=3.0).step_by(0.05).suffix("x"))
                                    .on_hover_text("完整縮放畫面 (0.4x 全視野 ~ 3.0x 局部放大)");
                            });
                            ui.horizontal(|ui| {
                                ui.label("🐟 輸入魚眼 FOV:");
                                ui.add(egui::Slider::new(&mut p.in_fov, 120..=230).suffix("°"))
                                    .on_hover_text("原始魚眼鏡頭視場角，數值越大邊緣越完整收納");
                            });
                            ui.horizontal(|ui| {
                                ui.label("展開水平 FOV:");
                                ui.add(egui::Slider::new(&mut p.out_hfov, 60..=140).suffix("°"));
                            });
                            ui.horizontal(|ui| {
                                ui.label("展開垂直 FOV:");
                                ui.add(egui::Slider::new(&mut p.out_vfov, 50..=120).suffix("°"));
                            });

                            ui.add_space(2.0);
                            let cam_base_info = match idx {
                                0 => "CAM 實車基準: 俯仰 26.4°, 偏航 4.0°, 滾轉 2.2°",
                                1 => "CAM 實車基準: 俯仰 18.9°, 偏航 180°, 滾轉 4.0°",
                                2 => "CAM 實車基準: 俯仰 85.0° (垂直俯視), 偏航 -90°, 滾轉 -7.6°",
                                _ => "CAM 實車基準: 俯仰 83.1° (垂直俯視), 偏航 90°, 滾轉 2.2°",
                            };
                            ui.label(RichText::new("【鏡頭角度修正 (即調即見)】").strong());
                            ui.label(RichText::new(cam_base_info).size(11.0).color(Color32::from_rgb(0, 220, 255)));
                            ui.horizontal(|ui| {
                                ui.label("俯仰微調 (Pitch):");
                                ui.add(egui::Slider::new(&mut p.pitch, -45..=45).suffix("°"))
                                    .on_hover_text("上下仰角/俯角修正：0° 為 CAM 實車標定值，負值往下看、正值往上看");
                            });
                            ui.horizontal(|ui| {
                                ui.label("偏航微調 (Yaw):");
                                ui.add(egui::Slider::new(&mut p.yaw, -45..=45).suffix("°"))
                                    .on_hover_text("左右偏航修正：0° 為 CAM 實車標定值，負值往左偏、正值往右偏");
                            });
                            ui.horizontal(|ui| {
                                ui.label("滾轉微調 (Roll):");
                                ui.add(egui::Slider::new(&mut p.roll, -30..=30).suffix("°"))
                                    .on_hover_text("水平旋轉調平：0° 為 CAM 實車標定值");
                            });

                            ui.add_space(3.0);
                            ui.label(RichText::new("【X/Y 軸平移微調 (即調即見)】").strong());
                            ui.horizontal(|ui| {
                                ui.label("X 軸平移 (左右):");
                                ui.add(egui::Slider::new(&mut p.offset_x, -1.00..=1.00).step_by(0.01).suffix(" m"))
                                    .on_hover_text("相機左右橫向平移微調：負值往左、正值往右 (步進 1 cm)");
                                if ui.small_button("0").on_hover_text("歸零").clicked() {
                                    p.offset_x = 0.0;
                                }
                            });
                            ui.horizontal(|ui| {
                                ui.label("Y 軸平移 (前後):");
                                ui.add(egui::Slider::new(&mut p.offset_y, -1.00..=1.00).step_by(0.01).suffix(" m"))
                                    .on_hover_text("相機前後縱向平移微調：負值往後、正值往前 (步進 1 cm)");
                                if ui.small_button("0").on_hover_text("歸零").clicked() {
                                    p.offset_y = 0.0;
                                }
                            });

                            ui.horizontal(|ui| {
                                if ui.button("🔄 重設此鏡頭").clicked() {
                                    self.camera_params[idx] = CameraParam::default_for_cam(idx);
                                }
                                if ui.button("✨ 恢復全部預設").clicked() {
                                    self.camera_params = CameraParam::default_presets();
                                }
                                if ui.button("📋 從鳥瞰校正同步").on_hover_text("複製鳥瞰圖校正專用參數至此單路預覽").clicked() {
                                    self.camera_params[idx] = self.birdview_camera_params[idx];
                                }
                            });
                        },
                    );

                    ui.add_space(4.0);

                    // 參數檔匯出/匯入管理面板
                    ui.collapsing(RichText::new("💾 參數設定檔管理 (匯出/匯入)").strong(), |ui| {
                        ui.label(RichText::new("儲存或載入四路相機角度、鳥瞰圖拼接與 3D 幾何設定").size(11.0).color(Color32::GRAY));
                        ui.horizontal(|ui| {
                            if ui.button("📤 匯出參數檔 (.json)").on_hover_text("將目前的所有校正參數儲存為 JSON 檔案").clicked() {
                                self.export_calibration_profile();
                            }
                            if ui.button("📥 匯入參數檔 (.json)").on_hover_text("從先前儲存的 JSON 檔案載入參數").clicked() {
                                self.import_calibration_profile();
                            }
                        });
                    });

                    ui.add_space(4.0);

                    // 檔案匯入面板
                    ui.collapsing(RichText::new("📁 影片檔案與示範樣品").strong(), |ui| {
                        if let Some(path) = &self.input_path {
                            let fname = path.file_name().unwrap_or_default().to_string_lossy();
                            if let Some(d) = self.input_duration_sec {
                                ui.label(format!("檔案: {} ({:.1}s)", fname, d));
                            } else {
                                ui.label(format!("檔案: {}", fname));
                            }
                        } else {
                            ui.label(RichText::new("未選取檔案").color(Color32::GRAY));
                        }

                        ui.horizontal(|ui| {
                            if ui.button("選取本地檔案").clicked() {
                                if let Some(file) = rfd::FileDialog::new()
                                    .add_filter("Video", &["mp4", "mkv", "avi", "mov", "ts"])
                                    .pick_file()
                                {
                                    self.load_video_file(file);
                                }
                            }
                            if ui.button("🎲 產生測試樣品").clicked() {
                                let demo_path = PathBuf::from("sample_videos/demo_quad_fisheye.mp4");
                                self.input_duration_sec = Some(5.0);
                                generate_demo_sample(&demo_path, self.tx_worker.clone());
                                self.input_path = Some(demo_path);
                                self.status_message = "正在產生示範樣品...".to_string();
                            }
                        });
                    });

                    ui.add_space(4.0);

                    // 象限方位映射指派
                    ui.collapsing(RichText::new("🧭 象限方位映射指派").strong(), |ui| {
                        let roles = ["front", "rear", "left", "right"];
                        let role_names = ["前視 (Front)", "後視 (Rear)", "左後視鏡 (Left)", "右後視鏡 (Right)"];
                        let labels = ["↖ 左上象限:", "↗ 右上象限:", "↙ 左下象限:", "↘ 右下象限:"];
                        for i in 0..4 {
                            ui.horizontal(|ui| {
                                ui.label(labels[i]);
                                egui::ComboBox::from_id_salt(format!("quad_cb_{}", i))
                                    .selected_text(match self.mapping[i].as_str() {
                                        "front" => "前視",
                                        "rear" => "後視",
                                        "left" => "左視",
                                        _ => "右視",
                                    })
                                    .show_ui(ui, |ui| {
                                        for (r, r_name) in roles.iter().zip(role_names.iter()) {
                                            ui.selectable_value(&mut self.mapping[i], r.to_string(), *r_name);
                                        }
                                    });
                            });
                        }
                    });

                    ui.add_space(4.0);


                    ui.add_space(8.0);
                    ui.separator();

                    // 一鍵固化輸出校正影片 (FFmpeg)
                    let btn_run = egui::Button::new(
                        RichText::new("💾 固化輸出校正影片 (FFmpeg)")
                            .strong()
                            .size(13.5),
                    )
                    .min_size(Vec2::new(ui.available_width(), 40.0))
                    .fill(Color32::from_rgb(0, 120, 210));

                    if ui.add_enabled(!self.is_processing, btn_run).on_hover_text("將上方調好的廣角與角度修正永久固化編碼至 front.mp4 等影片").clicked() {
                        if let Some(input) = &self.input_path {
                            self.is_processing = true;
                            self.progress_pct = 2.0;
                            self.status_message = "正在調用 FFmpeg 進行四象限裁切與拉直...".to_string();

                            let config = ProcessConfig {
                                input_path: input.clone(),
                                output_dir: self.output_dir.clone(),
                                mapping: self.mapping.clone(),
                                camera_params: self.camera_params,
                            };

                            start_processing(config, self.cancel_flag.clone(), self.tx_worker.clone());
                        } else {
                            self.status_message = "請先匯入影片檔案或產生測試樣品！".to_string();
                        }
                    }

                    if self.is_processing {
                        ui.add_space(4.0);
                        ui.add(egui::ProgressBar::new(self.progress_pct / 100.0).text(format!("{:.1}%", self.progress_pct)));
                    }

                    ui.add_space(10.0);
                });
            });

        // 中央畫布：左側鳥瞰全景 + 中間主視角
        egui::CentralPanel::default().show(ctx, |ui| {
            let total_rect = ui.available_rect_before_wrap();
            let total_w = total_rect.width();
            let total_h = total_rect.height();

            if total_w < 100.0 || total_h < 100.0 {
                return;
            }

            let bird_w = (total_w * 0.33).clamp(280.0, 480.0);
            let center_w = total_w - bird_w;

            let left_rect = Rect::from_min_size(total_rect.min, Vec2::new(bird_w, total_h));
            let center_rect = Rect::from_min_size(
                Pos2::new(total_rect.min.x + bird_w, total_rect.min.y),
                Vec2::new(center_w, total_h),
            );

            // 1. 左側：360° 車載鳥瞰全景 (Bird's Eye View)
            // 獨立使用鳥瞰圖專用校正參數，完全不受單路主視角調整干擾
            let painter_left = ui.painter_at(left_rect);
            self.birdview.render(&painter_left, left_rect, ctx, tex_ids, &self.birdview_camera_params, None);

            // 支援在左側鳥瞰圖點選扇區直接切換主視角
            let bird_resp = ui.allocate_rect(left_rect, egui::Sense::click());
            if bird_resp.clicked() {
                if let Some(pos) = bird_resp.interact_pointer_pos() {
                    if let Some(cam) = self.birdview.check_sector_click(pos, left_rect) {
                        self.selected_cam_idx = cam;
                        self.center_view = match cam {
                            0 => CenterViewMode::Front,
                            1 => CenterViewMode::Rear,
                            2 => CenterViewMode::Left,
                            _ => CenterViewMode::Right,
                        };
                    }
                }
            }

            // 分割線
            let sep_line_x = left_rect.right();
            painter_left.line_segment(
                [Pos2::new(sep_line_x, total_rect.top()), Pos2::new(sep_line_x, total_rect.bottom())],
                Stroke::new(2.0_f32, Color32::from_rgb(30, 40, 56)),
            );

            // 2. 中間：同步主視角播放畫面 (Center Main View)
            let painter_center = ui.painter_at(center_rect);
            painter_center.rect_filled(center_rect, 0.0, Color32::from_rgb(10, 14, 20));

            let (cam_idx, cam_title) = match self.center_view {
                CenterViewMode::Front => (0, "🚗 車前即時主視角 (Front Camera)"),
                CenterViewMode::Rear => (1, "🔙 車後即時主視角 (Rear Camera)"),
                CenterViewMode::Left => (2, "👈 車左側即時視角 (-90° 旋轉對齊)"),
                CenterViewMode::Right => (3, "👉 車右側即時視角 (+90° 旋轉對齊)"),
            };

            let active_tex = tex_ids[cam_idx];
            let param = &self.camera_params[cam_idx];

                    if let Some(tex) = active_tex {
                        let target_aspect = 16.0 / 9.0;
                        let panel_aspect = center_rect.width() / center_rect.height();

                        let (video_w, video_h) = if panel_aspect > target_aspect {
                            (center_rect.height() * target_aspect, center_rect.height())
                        } else {
                            (center_rect.width(), center_rect.width() / target_aspect)
                        };

                        let video_rect = Rect::from_center_size(center_rect.center(), Vec2::new(video_w, video_h));

                        // 使用 16x12 網格進行魚眼反畸變與即時角度預覽 (支援左側 -90°、右側 +90° 旋轉)
                        let mut mesh = Mesh::with_texture(tex);
                        let gx: usize = 16;
                        let gy: usize = 12;
                        for iy in 0..=gy {
                            let ty = iy as f32 / gy as f32;
                            let py = video_rect.top() + ty * video_rect.height();
                            for ix in 0..=gx {
                                let tx = ix as f32 / gx as f32;
                                let px = video_rect.left() + tx * video_rect.width();
                                let uv = param.transform_uv_for_cam(cam_idx, Pos2::new(tx, ty));
                                mesh.vertices.push(egui::epaint::Vertex {
                                    pos: Pos2::new(px, py),
                                    uv,
                                    color: Color32::WHITE,
                                });
                            }
                        }
                        let stride = (gx + 1) as u32;
                        for iy in 0..gy as u32 {
                            for ix in 0..gx as u32 {
                                let i00 = iy * stride + ix;
                                let i10 = i00 + 1;
                                let i01 = (iy + 1) * stride + ix;
                                let i11 = i01 + 1;
                                mesh.add_triangle(i00, i10, i11);
                                mesh.add_triangle(i00, i11, i01);
                            }
                        }
                        painter_center.add(mesh);

                        painter_center.rect_stroke(
                            video_rect,
                            0.0,
                            Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(0, 210, 255, 60)),
                        );

                        if self.show_horizon_crosshair {
                            let vc = video_rect.center();
                            let cross_stroke = Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(0, 230, 255, 75));
                            painter_center.line_segment([Pos2::new(video_rect.left() + 20.0, vc.y), Pos2::new(video_rect.right() - 20.0, vc.y)], cross_stroke);
                            painter_center.line_segment([Pos2::new(vc.x, video_rect.top() + 20.0), Pos2::new(vc.x, video_rect.bottom() - 20.0)], cross_stroke);
                        }

                        let badge_pos = Pos2::new(video_rect.left() + 14.0, video_rect.top() + 14.0);
                        painter_center.text(
                            badge_pos,
                            egui::Align2::LEFT_TOP,
                            format!("🟢 {}", cam_title),
                            egui::FontId::proportional(13.0),
                            Color32::from_rgb(0, 255, 200),
                        );

                        let status_text = format!(
                            "即時預覽中 (Zoom: {:.2}x, FOV: {}°, Pitch: {}°, Yaw: {}°, Roll: {}°)",
                            param.zoom, param.in_fov, param.pitch, param.yaw, param.roll
                        );
                        painter_center.text(
                            Pos2::new(video_rect.right() - 14.0, video_rect.top() + 14.0),
                            egui::Align2::RIGHT_TOP,
                            status_text,
                            egui::FontId::proportional(11.5),
                            Color32::from_rgb(0, 230, 255),
                        );
                    } else {
                        painter_center.text(
                            center_rect.center(),
                            egui::Align2::CENTER_CENTER,
                            format!("{}\n\n請在右側點選「選取本地檔案」或「🎲 產生測試樣品」", cam_title),
                            egui::FontId::proportional(15.0),
                            Color32::from_rgb(120, 150, 180),
                        );
                    }
                });
    }

    /// 渲染 分頁 2: 🦅 鳥瞰圖拼接專用校正分頁
    /// - 大畫面鳥瞰圖，點選前後左右扇區即跳出浮動微調面板 (小拖動 bar)
    /// - 支援縮放、魚眼 FOV、俯仰、偏航、滾轉 60 FPS 零延遲即時預覽
    fn render_birdview_calib_tab(&mut self, ctx: &egui::Context, tex_ids: &[Option<egui::TextureId>; 4]) {
        // 右側輔助微調面板 (相機即時原圖 + 地面裁切高度)
        egui::SidePanel::right("calib_side_panel")
            .min_width(320.0)
            .default_width(340.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.add_space(4.0);
                    ui.heading(
                        RichText::new("🦅 鳥瞰扇區專用校正")
                            .color(Color32::from_rgb(0, 220, 255))
                            .size(15.0),
                    );
                    ui.add_space(4.0);

                    ui.label(
                        RichText::new("💡 提示：點擊中央鳥瞰圖的任一扇區，即可選取並跳出專屬微調面板！")
                            .color(Color32::from_rgb(0, 255, 180))
                            .size(12.0),
                    );

                    ui.add_space(6.0);

                    // 快速切換選取按鈕
                    let cam_names = ["🚗 前視 (Front)", "🔙 後視 (Rear)", "👈 左側 (Left)", "👉 右側 (Right)"];
                    ui.label(RichText::new("【切換當前選取鏡頭】").strong());
                    ui.horizontal_wrapped(|ui| {
                        for i in 0..4 {
                            let is_sel = self.selected_cam_idx == i;
                            if ui.selectable_label(is_sel, cam_names[i]).clicked() {
                                self.selected_cam_idx = i;
                                self.show_calib_floating_bar = true;
                            }
                        }
                    });

                    ui.add_space(8.0);
                    ui.separator();

                    // 當前鏡頭即時畫面縮圖 (預覽展開形變)
                    let cur_idx = self.selected_cam_idx;
                    ui.label(RichText::new(format!("📷 {} 畫面預覽", cam_names[cur_idx])).color(Color32::from_rgb(255, 215, 0)).strong());

                    let thumb_rect = ui.allocate_space(Vec2::new(ui.available_width(), 160.0)).1;
                    let painter_thumb = ui.painter_at(thumb_rect);
                    painter_thumb.rect_filled(thumb_rect, 4.0, Color32::from_rgb(14, 18, 26));

                    if let Some(tex) = tex_ids[cur_idx] {
                        let param = &self.birdview_camera_params[cur_idx];
                        let mut mesh = Mesh::with_texture(tex);
                        let gx: usize = 8;
                        let gy: usize = 6;
                        for iy in 0..=gy {
                            let ty = iy as f32 / gy as f32;
                            let py = thumb_rect.top() + ty * thumb_rect.height();
                            for ix in 0..=gx {
                                let tx = ix as f32 / gx as f32;
                                let px = thumb_rect.left() + tx * thumb_rect.width();
                                let uv = param.transform_uv_for_cam(cur_idx, Pos2::new(tx, ty));
                                mesh.vertices.push(egui::epaint::Vertex {
                                    pos: Pos2::new(px, py),
                                    uv,
                                    color: Color32::WHITE,
                                });
                            }
                        }
                        let stride = (gx + 1) as u32;
                        for iy in 0..gy as u32 {
                            for ix in 0..gx as u32 {
                                let i00 = iy * stride + ix;
                                let i10 = i00 + 1;
                                let i01 = (iy + 1) * stride + ix;
                                let i11 = i01 + 1;
                                mesh.add_triangle(i00, i10, i11);
                                mesh.add_triangle(i00, i11, i01);
                            }
                        }
                        painter_thumb.add(mesh);

                        // 十字準心
                        let c = thumb_rect.center();
                        let cs = Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(0, 230, 255, 80));
                        painter_thumb.line_segment([Pos2::new(thumb_rect.left(), c.y), Pos2::new(thumb_rect.right(), c.y)], cs);
                        painter_thumb.line_segment([Pos2::new(c.x, thumb_rect.top()), Pos2::new(c.x, thumb_rect.bottom())], cs);
                    } else {
                        painter_thumb.text(
                            thumb_rect.center(),
                            egui::Align2::CENTER_CENTER,
                            "尚無影像訊號",
                            egui::FontId::proportional(13.0),
                            Color32::GRAY,
                        );
                    }

                    ui.add_space(8.0);
                    ui.separator();

                    // 鳥瞰拼接輔助微調
                    ui.label(RichText::new("🌿 鳥瞰扇區展開角度與 3D 碗狀融合").strong());
                    ui.horizontal(|ui| {
                        ui.label("前後扇區張角:");
                        ui.add(egui::Slider::new(&mut self.birdview.seam_spread, 0.40..=1.35).suffix("x"))
                            .on_hover_text("調整前後視角在鳥瞰圖中的展開角度寬度 (1.00x = 直達四角，>1.0x 超廣角)");
                    });
                    ui.horizontal(|ui| {
                        ui.label("地面切分高度:");
                        ui.add(egui::Slider::new(&mut self.birdview.uv_road_split, 0.2..=0.85).suffix(" ratio"))
                            .on_hover_text("調整路面在鳥瞰圖中的有效取景高度 (避免遠方天空進入地面拼接)");
                    });
                    ui.horizontal(|ui| {
                        ui.label("3D 碗狀曲面:");
                        ui.add(egui::Slider::new(&mut self.birdview.bowl_strength, 0.0..=1.0))
                            .on_hover_text("近處維持平坦 Z=0 確保車道線筆直，遠處平滑上彎以減少放射狀拉伸 (0 = 純平面)");
                    });
                    ui.horizontal(|ui| {
                        ui.label("接縫銳利度:");
                        ui.add(egui::Slider::new(&mut self.birdview.seam_sharpness, 1.0..=6.0))
                            .on_hover_text("提高數值可收窄重疊過渡帶，減少鄰車或路緣重影");
                    });
                    ui.checkbox(&mut self.birdview.auto_gain, "自動亮度與白平衡增益補償 (Auto Gain)")
                        .on_hover_text("依據四路重疊區亮度與色溫自動均衡四路畫面，消除十字色差");

                    ui.add_space(6.0);
                    let has_all_frames = self.player.latest_frames.iter().all(|f| f.is_some());
                    let btn_auto_align = egui::Button::new(
                        RichText::new("✨ 一鍵自動對齊四路外參 (Auto Align)")
                            .strong()
                            .size(13.0),
                    )
                    .min_size(Vec2::new(ui.available_width(), 32.0))
                    .fill(Color32::from_rgb(0, 145, 110));
                    if ui
                        .add_enabled(has_all_frames, btn_auto_align)
                        .on_hover_text("利用目前畫面的重疊區域地面紋理，自動優化四路 Yaw/Pitch/Roll 與 X/Y 偏移")
                        .clicked()
                    {
                        match auto_align_extrinsics(
                            &self.player.latest_frames,
                            &mut self.birdview_camera_params,
                        ) {
                            Ok(report) => {
                                self.status_message = format!(
                                    "✨ 自動對齊完成！重疊誤差降低 {:.1}% (RMSE {:.1} → {:.1}, {} 取樣點)",
                                    report.improved_pct,
                                    report.before_rmse,
                                    report.after_rmse,
                                    report.overlap_samples
                                );
                            }
                            Err(e) => {
                                self.status_message = format!("自動對齊失敗: {}", e);
                            }
                        }
                    }

                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("📌 呼叫/重開懸浮調整小 Bar").clicked() {
                            self.show_calib_floating_bar = true;
                        }
                        if ui.button("📋 同步至單路視角").on_hover_text("將目前鳥瞰圖校正參數複製給單路預覽").clicked() {
                            self.camera_params[cur_idx] = self.birdview_camera_params[cur_idx];
                        }
                    });

                    ui.add_space(8.0);
                    ui.separator();

                    // 參數檔匯出/匯入管理
                    ui.label(RichText::new("💾 參數設定檔管理 (匯出/匯入)").strong());
                    ui.horizontal(|ui| {
                        if ui.button("📤 匯出參數檔 (.json)").on_hover_text("將目前的所有校正參數儲存為 JSON 檔案").clicked() {
                            self.export_calibration_profile();
                        }
                        if ui.button("📥 匯入參數檔 (.json)").on_hover_text("從先前儲存的 JSON 檔案載入參數").clicked() {
                            self.import_calibration_profile();
                        }
                    });

                    ui.add_space(10.0);
                    ui.separator();

                    // 固化按鈕
                    let btn_bake = egui::Button::new(
                        RichText::new("💾 固化輸出校正影片 (FFmpeg)")
                            .strong()
                            .size(13.5),
                    )
                    .min_size(Vec2::new(ui.available_width(), 38.0))
                    .fill(Color32::from_rgb(0, 120, 210));

                    if ui.add_enabled(!self.is_processing, btn_bake).clicked() {
                        if let Some(input) = &self.input_path {
                            self.is_processing = true;
                            self.progress_pct = 2.0;
                            self.status_message = "正在調用 FFmpeg 進行四象限裁切與拉直...".to_string();

                            let config = ProcessConfig {
                                input_path: input.clone(),
                                output_dir: self.output_dir.clone(),
                                mapping: self.mapping.clone(),
                                camera_params: self.birdview_camera_params,
                            };

                            start_processing(config, self.cancel_flag.clone(), self.tx_worker.clone());
                        } else {
                            self.status_message = "請先匯入影片檔案或產生測試樣品！".to_string();
                        }
                    }

                    if self.is_processing {
                        ui.add_space(4.0);
                        ui.add(egui::ProgressBar::new(self.progress_pct / 100.0).text(format!("{:.1}%", self.progress_pct)));
                    }
                });
            });

        // 中央大畫面鳥瞰圖畫布
        egui::CentralPanel::default().show(ctx, |ui| {
            let total_rect = ui.available_rect_before_wrap();
            let painter = ui.painter_at(total_rect);

            // 繪製高解析度大鳥瞰圖 (並高亮選中的扇區)
            self.birdview.render(&painter, total_rect, ctx, tex_ids, &self.birdview_camera_params, Some(self.selected_cam_idx));

            // 檢測滑鼠點擊扇區
            let bird_resp = ui.allocate_rect(total_rect, egui::Sense::click());
            if bird_resp.clicked() {
                if let Some(pos) = bird_resp.interact_pointer_pos() {
                    if let Some(cam) = self.birdview.check_sector_click(pos, total_rect) {
                        self.selected_cam_idx = cam;
                        self.show_calib_floating_bar = true;
                    }
                }
            }
        });

        // 懸浮小拖動 Bar (Floating Calibration HUD)
        if self.show_calib_floating_bar {
            let cam_names = ["前視鏡頭 (Front)", "後視鏡頭 (Rear)", "左後視鏡 (Left)", "右後視鏡 (Right)"];
            let idx = self.selected_cam_idx;
            let mut is_open = self.show_calib_floating_bar;

            egui::Window::new(format!("🔧 {} 視角微調 (即時預覽)", cam_names[idx]))
                .open(&mut is_open)
                .default_width(320.0)
                .resizable(false)
                .collapsible(true)
                .show(ctx, |ui| {
                    let p = &mut self.birdview_camera_params[idx];

                    ui.label(
                        RichText::new("🔍 縮放與魚眼廣角 (即動即見)")
                            .strong()
                            .color(Color32::from_rgb(0, 230, 255)),
                    );

                    ui.horizontal(|ui| {
                        ui.label("🔍 縮放 (Zoom):");
                        ui.add(egui::Slider::new(&mut p.zoom, 0.4..=3.0).step_by(0.05).suffix("x"))
                            .on_hover_text("縮放完整魚眼畫面");
                    });

                    ui.horizontal(|ui| {
                        ui.label("🐟 輸入魚眼 FOV:");
                        ui.add(egui::Slider::new(&mut p.in_fov, 120..=230).suffix("°"))
                            .on_hover_text("原始魚眼視場角 (越大收納越多邊緣)");
                    });

                    ui.add_space(4.0);
                    let cam_base_info = match idx {
                        0 => "CAM 實車基準: 俯仰 26.4°, 偏航 4.0°, 滾轉 2.2°",
                        1 => "CAM 實車基準: 俯仰 18.9°, 偏航 180°, 滾轉 4.0°",
                        2 => "CAM 實車基準: 俯仰 85.0° (垂直俯視), 偏航 -90°, 滾轉 -7.6°",
                        _ => "CAM 實車基準: 俯仰 83.1° (垂直俯視), 偏航 90°, 滾轉 2.2°",
                    };
                    ui.label(
                        RichText::new("📐 三軸角度校正 (即時預覽)")
                            .strong()
                            .color(Color32::from_rgb(0, 230, 255)),
                    );
                    ui.label(RichText::new(cam_base_info).size(11.0).color(Color32::from_rgb(180, 220, 255)));

                    ui.horizontal(|ui| {
                        ui.label("俯仰微調 (Pitch):");
                        ui.add(egui::Slider::new(&mut p.pitch, -45..=45).suffix("°"))
                            .on_hover_text("上下仰角/俯角修正：0° 為 CAM 實車標定值，負值往下看、正值往上看");
                    });

                    ui.horizontal(|ui| {
                        ui.label("偏航微調 (Yaw):");
                        ui.add(egui::Slider::new(&mut p.yaw, -45..=45).suffix("°"))
                            .on_hover_text("左右偏航修正：0° 為 CAM 實車標定值，負值往左偏、正值往右偏");
                    });

                    ui.horizontal(|ui| {
                        ui.label("滾轉微調 (Roll):");
                        ui.add(egui::Slider::new(&mut p.roll, -30..=30).suffix("°"))
                            .on_hover_text("水平旋轉調平：0° 為 CAM 實車標定值");
                    });

                    ui.add_space(4.0);
                    ui.label(
                        RichText::new("↔️↕️ X/Y 軸平移控制 (即調即見)")
                            .strong()
                            .color(Color32::from_rgb(0, 230, 255)),
                    );
                    ui.horizontal(|ui| {
                        ui.label("X 軸平移 (左右):");
                        ui.add(egui::Slider::new(&mut p.offset_x, -1.00..=1.00).step_by(0.01).suffix(" m"))
                            .on_hover_text("相機左右橫向平移微調：負值往左、正值往右 (步進 1 cm)");
                        if ui.small_button("0").on_hover_text("歸零").clicked() {
                            p.offset_x = 0.0;
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label("Y 軸平移 (前後):");
                        ui.add(egui::Slider::new(&mut p.offset_y, -1.00..=1.00).step_by(0.01).suffix(" m"))
                            .on_hover_text("相機前後縱向平移微調：負值往後、正值往前 (步進 1 cm)");
                        if ui.small_button("0").on_hover_text("歸零").clicked() {
                            p.offset_y = 0.0;
                        }
                    });

                    ui.add_space(4.0);
                    ui.label(RichText::new("展開視場角:").size(12.0));
                    ui.horizontal(|ui| {
                        ui.label("展開 H-FOV:");
                        ui.add(egui::Slider::new(&mut p.out_hfov, 60..=140).suffix("°"));
                    });
                    ui.horizontal(|ui| {
                        ui.label("展開 V-FOV:");
                        ui.add(egui::Slider::new(&mut p.out_vfov, 50..=120).suffix("°"));
                    });

                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui.button("🔄 重設此視角").clicked() {
                            self.birdview_camera_params[idx] = CameraParam::default_for_cam(idx);
                        }
                        if ui.button("✨ 恢復全部預設").clicked() {
                            self.birdview_camera_params = CameraParam::default_presets();
                        }
                        if ui.button("📋 同步至單路視角").on_hover_text("將目前鳥瞰圖校正參數複製給單路預覽").clicked() {
                            self.camera_params[idx] = self.birdview_camera_params[idx];
                        }
                    });
                });

            self.show_calib_floating_bar = is_open;
        }
    }

    /// 渲染 分頁 3: 📹 原始未裁切/未調整四路影片播放分頁
    /// - 支援 2x2 四畫面全覽（100% 原始魚眼鏡頭畫面，無任何幾何拉直或去畸變）
    /// - 支援點擊任一象限放大單路全螢幕播放
    /// - 支援前後左右方位對應切換
    fn render_raw_video_tab(&mut self, ctx: &egui::Context, tex_ids: &[Option<egui::TextureId>; 4]) {
        let cam_names = ["前視 (Front)", "後視 (Rear)", "左側 (Left)", "右側 (Right)"];
        let cam_icons = ["🚗", "🔙", "👈", "👉"];

        // 右側控制面板
        egui::SidePanel::right("raw_side_panel")
            .min_width(290.0)
            .default_width(320.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.add_space(4.0);
                    ui.heading(
                        RichText::new("📹 原始影片檢視")
                            .color(Color32::from_rgb(0, 220, 255))
                            .size(15.0),
                    );
                    ui.add_space(4.0);

                    ui.label(
                        RichText::new("💡 說明：此頁面播放行車記錄器四路原始光學魚眼影像，100% 未經裁切、拉直與任何去畸變處理，真實呈現相機完整全視場。")
                            .color(Color32::from_rgb(0, 255, 180))
                            .size(11.5),
                    );

                    ui.add_space(8.0);
                    ui.separator();

                    ui.heading(
                        RichText::new("🎯 檢視模式與方位切換")
                            .color(Color32::from_rgb(0, 220, 255))
                            .size(13.5),
                    );
                    ui.add_space(4.0);

                    // 1. 一鍵 2x2 四路全覽大按鈕
                    let is_quad = self.raw_selected_cam.is_none();
                    let btn_quad = egui::SelectableLabel::new(
                        is_quad,
                        RichText::new("🔲 2x2 四路原始畫面全覽").strong().size(13.5),
                    );
                    if ui.add_sized([ui.available_width(), 38.0], btn_quad).clicked() {
                        self.raw_selected_cam = None;
                    }

                    ui.add_space(6.0);
                    ui.label(RichText::new("【單路全螢幕放大 (方位對應)】").size(12.0).color(Color32::GRAY));

                    // 2. 前後左右方位對應十字佈局
                    ui.vertical_centered(|ui| {
                        let total_w = ui.available_width();
                        let btn_w_half = ((total_w - 8.0) / 2.0).max(110.0);
                        let btn_w_center = (total_w * 0.72).clamp(150.0, 230.0);
                        let btn_h = 34.0;

                        // 上方：前視
                        let is_f = self.raw_selected_cam == Some(0);
                        if ui.add_sized(
                            [btn_w_center, btn_h],
                            egui::SelectableLabel::new(
                                is_f,
                                RichText::new("⬆️ 🚗 前視 (Front)").strong().size(13.0),
                            ),
                        ).clicked() {
                            self.raw_selected_cam = Some(0);
                        }

                        ui.add_space(3.0);

                        // 中間排：左側與右側
                        ui.horizontal(|ui| {
                            let is_l = self.raw_selected_cam == Some(2);
                            if ui.add_sized(
                                [btn_w_half, btn_h],
                                egui::SelectableLabel::new(
                                    is_l,
                                    RichText::new("⬅️ 👈 左側 (Left)").strong().size(12.5),
                                ),
                            ).clicked() {
                                self.raw_selected_cam = Some(2);
                            }

                            let is_r = self.raw_selected_cam == Some(3);
                            if ui.add_sized(
                                [btn_w_half, btn_h],
                                egui::SelectableLabel::new(
                                    is_r,
                                    RichText::new("👉 右側 (Right) ➡️").strong().size(12.5),
                                ),
                            ).clicked() {
                                self.raw_selected_cam = Some(3);
                            }
                        });

                        ui.add_space(3.0);

                        // 下方：後視
                        let is_rear = self.raw_selected_cam == Some(1);
                        if ui.add_sized(
                            [btn_w_center, btn_h],
                            egui::SelectableLabel::new(
                                is_rear,
                                RichText::new("⬇️ 🔙 後視 (Rear)").strong().size(13.0),
                            ),
                        ).clicked() {
                            self.raw_selected_cam = Some(1);
                        }
                    });

                    ui.add_space(10.0);
                    ui.separator();

                    // 檔案資訊與載入
                    ui.label(RichText::new("📁 影片檔案狀態").strong());
                    if let Some(path) = &self.input_path {
                        let fname = path.file_name().unwrap_or_default().to_string_lossy();
                        if let Some(d) = self.input_duration_sec {
                            ui.label(format!("檔案: {} ({:.1}s)", fname, d));
                        } else {
                            ui.label(format!("檔案: {}", fname));
                        }
                    } else {
                        ui.label(RichText::new("未選取檔案").color(Color32::GRAY));
                    }

                    ui.horizontal(|ui| {
                        if ui.button("選取本地檔案").clicked() {
                            if let Some(file) = rfd::FileDialog::new()
                                .add_filter("Video", &["mp4", "mkv", "avi", "mov", "ts"])
                                .pick_file()
                            {
                                self.load_video_file(file);
                            }
                        }
                        if ui.button("🎲 產生測試樣品").clicked() {
                            let demo_path = PathBuf::from("sample_videos/demo_quad_fisheye.mp4");
                            self.input_duration_sec = Some(5.0);
                            generate_demo_sample(&demo_path, self.tx_worker.clone());
                            self.input_path = Some(demo_path);
                            self.status_message = "正在產生示範樣品...".to_string();
                        }
                    });

                    ui.add_space(8.0);
                    ui.label(
                        RichText::new("💡 提示：使用視窗底部的播放/暫停鍵與時間拉桿，可隨時控制四路同步播放或精確跳轉秒數。")
                            .size(11.0)
                            .color(Color32::GRAY),
                    );
                });
            });

        // 中央畫布：2x2 四畫面全覽 或 單路放大
        egui::CentralPanel::default().show(ctx, |ui| {
            let total_rect = ui.available_rect_before_wrap();
            let painter = ui.painter_at(total_rect);
            painter.rect_filled(total_rect, 0.0, Color32::from_rgb(10, 14, 20));

            match self.raw_selected_cam {
                None => {
                    // 2x2 四畫面全覽模式
                    let pad = 6.0;
                    let w = ((total_rect.width() - pad) / 2.0).max(10.0);
                    let h = ((total_rect.height() - pad) / 2.0).max(10.0);

                    let sub_rects = [
                        Rect::from_min_size(Pos2::new(total_rect.left(), total_rect.top()), Vec2::new(w, h)),
                        Rect::from_min_size(Pos2::new(total_rect.left() + w + pad, total_rect.top()), Vec2::new(w, h)),
                        Rect::from_min_size(Pos2::new(total_rect.left(), total_rect.top() + h + pad), Vec2::new(w, h)),
                        Rect::from_min_size(Pos2::new(total_rect.left() + w + pad, total_rect.top() + h + pad), Vec2::new(w, h)),
                    ];

                    let quadrant_labels = [
                        "↖ 左上: 🚗 前視 (Front) - 原始未裁切魚眼",
                        "↗ 右上: 🔙 後視 (Rear) - 原始未裁切魚眼",
                        "↙ 左下: 👈 左側 (Left) - 原始未裁切魚眼",
                        "↘ 右下: 👉 右側 (Right) - 原始未裁切魚眼",
                    ];

                    for i in 0..4 {
                        let rect = sub_rects[i];
                        painter.rect_filled(rect, 4.0, Color32::from_rgb(14, 18, 26));

                        if let Some(tex) = tex_ids[i] {
                            let target_aspect = 16.0 / 9.0;
                            let panel_aspect = rect.width() / rect.height();
                            let (video_w, video_h) = if panel_aspect > target_aspect {
                                (rect.height() * target_aspect, rect.height())
                            } else {
                                (rect.width(), rect.width() / target_aspect)
                            };
                            let video_rect = Rect::from_center_size(rect.center(), Vec2::new(video_w, video_h));

                            // 繪製 100% 原始未經任何形變拉伸的魚眼紋理
                            let mut mesh = Mesh::with_texture(tex);
                            mesh.vertices.push(egui::epaint::Vertex { pos: video_rect.left_top(), uv: Pos2::new(0.0, 0.0), color: Color32::WHITE });
                            mesh.vertices.push(egui::epaint::Vertex { pos: video_rect.right_top(), uv: Pos2::new(1.0, 0.0), color: Color32::WHITE });
                            mesh.vertices.push(egui::epaint::Vertex { pos: video_rect.right_bottom(), uv: Pos2::new(1.0, 1.0), color: Color32::WHITE });
                            mesh.vertices.push(egui::epaint::Vertex { pos: video_rect.left_bottom(), uv: Pos2::new(0.0, 1.0), color: Color32::WHITE });
                            mesh.add_triangle(0, 1, 2);
                            mesh.add_triangle(0, 2, 3);
                            painter.add(mesh);
                        } else {
                            painter.text(
                                rect.center(),
                                egui::Align2::CENTER_CENTER,
                                format!("{}\n\n尚無影像訊號", quadrant_labels[i]),
                                egui::FontId::proportional(13.0),
                                Color32::GRAY,
                            );
                        }

                        // 外框與高亮
                        painter.rect_stroke(
                            rect,
                            4.0,
                            Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(0, 210, 255, 60)),
                        );

                        // 左上角標籤
                        let badge_pos = Pos2::new(rect.left() + 10.0, rect.top() + 8.0);
                        painter.text(
                            badge_pos,
                            egui::Align2::LEFT_TOP,
                            quadrant_labels[i],
                            egui::FontId::proportional(12.0),
                            Color32::from_rgb(0, 255, 180),
                        );

                        // 右上角提示
                        painter.text(
                            Pos2::new(rect.right() - 10.0, rect.top() + 8.0),
                            egui::Align2::RIGHT_TOP,
                            "🔍 點擊放大",
                            egui::FontId::proportional(11.0),
                            Color32::from_rgb(0, 220, 255),
                        );

                        // 點擊此象限放大
                        let resp = ui.allocate_rect(rect, egui::Sense::click());
                        if resp.clicked() {
                            self.raw_selected_cam = Some(i);
                        }
                    }
                }
                Some(cam_idx) => {
                    // 單路原始大畫面全螢幕模式
                    let target_aspect = 16.0 / 9.0;
                    let panel_aspect = total_rect.width() / total_rect.height();
                    let (video_w, video_h) = if panel_aspect > target_aspect {
                        (total_rect.height() * target_aspect, total_rect.height())
                    } else {
                        (total_rect.width(), total_rect.width() / target_aspect)
                    };
                    let video_rect = Rect::from_center_size(total_rect.center(), Vec2::new(video_w, video_h));

                    if let Some(tex) = tex_ids[cam_idx] {
                        let mut mesh = Mesh::with_texture(tex);
                        mesh.vertices.push(egui::epaint::Vertex { pos: video_rect.left_top(), uv: Pos2::new(0.0, 0.0), color: Color32::WHITE });
                        mesh.vertices.push(egui::epaint::Vertex { pos: video_rect.right_top(), uv: Pos2::new(1.0, 0.0), color: Color32::WHITE });
                        mesh.vertices.push(egui::epaint::Vertex { pos: video_rect.right_bottom(), uv: Pos2::new(1.0, 1.0), color: Color32::WHITE });
                        mesh.vertices.push(egui::epaint::Vertex { pos: video_rect.left_bottom(), uv: Pos2::new(0.0, 1.0), color: Color32::WHITE });
                        mesh.add_triangle(0, 1, 2);
                        mesh.add_triangle(0, 2, 3);
                        painter.add(mesh);
                    } else {
                        painter.text(
                            total_rect.center(),
                            egui::Align2::CENTER_CENTER,
                            format!("{} 尚無影像訊號", cam_names[cam_idx]),
                            egui::FontId::proportional(16.0),
                            Color32::GRAY,
                        );
                    }

                    // 外框
                    painter.rect_stroke(
                        video_rect,
                        0.0,
                        Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(0, 210, 255, 80)),
                    );

                    // 頂部浮動工具列
                    let bar_rect = Rect::from_min_size(
                        Pos2::new(video_rect.left() + 14.0, video_rect.top() + 12.0),
                        Vec2::new(video_rect.width() - 28.0, 36.0),
                    );
                    painter.rect_filled(
                        bar_rect,
                        4.0,
                        Color32::from_rgba_unmultiplied(10, 14, 20, 210),
                    );

                    painter.text(
                        Pos2::new(bar_rect.left() + 12.0, bar_rect.center().y),
                        egui::Align2::LEFT_CENTER,
                        format!("🟢 原始光學全視場: {} {} (100% 未裁切 / 未調整)", cam_icons[cam_idx], cam_names[cam_idx]),
                        egui::FontId::proportional(13.0),
                        Color32::from_rgb(0, 255, 180),
                    );

                    // 返回 2x2 按鈕
                    let back_btn_rect = Rect::from_min_size(
                        Pos2::new(bar_rect.right() - 170.0, bar_rect.top() + 3.0),
                        Vec2::new(162.0, 30.0),
                    );
                    let back_resp = ui.allocate_rect(back_btn_rect, egui::Sense::click());
                    let btn_bg = if back_resp.hovered() {
                        Color32::from_rgb(0, 140, 220)
                    } else {
                        Color32::from_rgb(0, 100, 170)
                    };
                    painter.rect_filled(back_btn_rect, 4.0, btn_bg);
                    painter.text(
                        back_btn_rect.center(),
                        egui::Align2::CENTER_CENTER,
                        "🔙 返回 2x2 四畫面全覽",
                        egui::FontId::proportional(12.5),
                        Color32::WHITE,
                    );
                    if back_resp.clicked() {
                        self.raw_selected_cam = None;
                    }
                }
            }
        });
    }
}
