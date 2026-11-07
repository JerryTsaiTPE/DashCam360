// src/main.rs
// DashCam360 Native (Rust) 入口點

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod birdview;
mod ffmpeg_worker;
mod player;
mod renderer3d;

use app::DashCamApp;
use eframe::egui::{self, Vec2};

fn setup_custom_chinese_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();

    // 優先搜尋 Windows 系統原生繁體中文字型 (微軟正黑體)
    let candidates = [
        "C:\\Windows\\Fonts\\msjh.ttc",   // 微軟正黑體 (Microsoft JhengHei)
        "C:\\Windows\\Fonts\\msyh.ttc",   // 微軟雅黑
        "C:\\Windows\\Fonts\\msjhbd.ttc", // 微軟正黑粗體
        "C:\\Windows\\Fonts\\mingliu.ttc",// 新細明體
        "C:\\Windows\\Fonts\\simsun.ttc", // 中易宋體
    ];

    let mut loaded = false;
    for path in candidates {
        if let Ok(bytes) = std::fs::read(path) {
            fonts.font_data.insert(
                "custom_chinese".to_owned(),
                egui::FontData::from_owned(bytes),
            );

            // 優先置於常規比例字型首位
            fonts
                .families
                .entry(egui::FontFamily::Proportional)
                .or_default()
                .insert(0, "custom_chinese".to_owned());

            // 亦加入等寬字型備用
            fonts
                .families
                .entry(egui::FontFamily::Monospace)
                .or_default()
                .push("custom_chinese".to_owned());

            loaded = true;
            break;
        }
    }

    if loaded {
        ctx.set_fonts(fonts);
    }
}

fn main() -> eframe::Result<()> {
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("DashCam360 Native - 汽車環景魚眼校正與 3D 空間投影系統 (Rust 高效原生版)")
            .with_inner_size(Vec2::new(1440.0, 880.0))
            .with_min_inner_size(Vec2::new(1024.0, 640.0)),
        ..Default::default()
    };

    eframe::run_native(
        "DashCam360 Native",
        native_options,
        Box::new(|cc| {
            // 載入繁體中文字型，徹底解決字形亂碼方塊問題
            setup_custom_chinese_fonts(&cc.egui_ctx);

            // 啟用現代暗黑科技風格
            let mut visuals = egui::Visuals::dark();
            visuals.panel_fill = egui::Color32::from_rgb(15, 20, 28);
            visuals.window_fill = egui::Color32::from_rgb(18, 24, 34);
            visuals.override_text_color = Some(egui::Color32::from_rgb(235, 242, 250));
            cc.egui_ctx.set_visuals(visuals);

            Ok(Box::new(DashCamApp::new(cc)))
        }),
    )
}
