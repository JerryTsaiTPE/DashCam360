// src/renderer3d.rs
// 專業車載 3D 無縫環景立體碗面 (Surround Bowl & Seamless Panorama) 渲染模組

use egui::{Color32, Mesh, Painter, Pos2, Rect, Stroke, TextureId};
use glam::{Mat4, Vec3, Vec4};

pub struct Camera3D {
    pub yaw: f32,       // 水平環視角度 (度)
    pub pitch: f32,     // 俯仰角度 (度)
    pub distance: f32,  // 相機距離
    pub look_at: Vec3,  // 觀察焦點目標
}

impl Default for Camera3D {
    fn default() -> Self {
        Self {
            yaw: 45.0,
            pitch: 35.0,
            distance: 7.6,
            look_at: Vec3::new(0.0, 0.5, 0.0),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ViewMode {
    SurroundBowl,     // 3D 車載無縫立體碗面 (地面平鋪 + 向上弧形碗壁)
    SeamlessCylinder, // 360° 無縫全景圓柱面
}

pub struct Surround3DViewer {
    pub camera: Camera3D,
    pub view_mode: ViewMode,
    pub floor_radius: f32, // 地面延伸半徑 (預設 2.6m)
    pub wall_radius: f32,  // 碗壁頂部半徑 (預設 4.4m)
    pub wall_height: f32,  // 碗壁揚起高度 (預設 2.2m)
    pub show_seam_lines: bool, // 是否標示 4 方位無縫接縫線
}

impl Default for Surround3DViewer {
    fn default() -> Self {
        Self {
            camera: Camera3D::default(),
            view_mode: ViewMode::SurroundBowl,
            floor_radius: 2.6,
            wall_radius: 4.4,
            wall_height: 2.2,
            show_seam_lines: true,
        }
    }
}

impl Surround3DViewer {
    pub fn handle_input(&mut self, response: &egui::Response, ui: &egui::Ui) {
        if response.dragged_by(egui::PointerButton::Primary) {
            let delta = response.drag_delta();
            self.camera.yaw += delta.x * 0.45;
            self.camera.pitch = (self.camera.pitch + delta.y * 0.45).clamp(5.0, 88.0);
        } else if response.dragged_by(egui::PointerButton::Secondary) {
            let delta = response.drag_delta();
            let rad_yaw = self.camera.yaw.to_radians();
            self.camera.look_at.x -= delta.x * 0.015 * rad_yaw.cos();
            self.camera.look_at.z += delta.x * 0.015 * rad_yaw.sin();
            self.camera.look_at.y += delta.y * 0.015;
        }

        let scroll_delta = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll_delta != 0.0 {
            self.camera.distance = (self.camera.distance - scroll_delta * 0.008).clamp(2.0, 25.0);
        }
    }

    pub fn render(
        &self,
        painter: &Painter,
        rect: Rect,
        textures: &[Option<TextureId>; 4], // [0: front, 1: rear, 2: left, 3: right]
    ) {
        let width = rect.width();
        let height = rect.height();
        if width <= 10.0 || height <= 10.0 {
            return;
        }

        // 1. 計算相機 MVP 透視矩陣
        let rad_pitch = self.camera.pitch.to_radians();
        let rad_yaw = self.camera.yaw.to_radians();

        let cam_x = self.camera.look_at.x + self.camera.distance * rad_pitch.cos() * rad_yaw.sin();
        let cam_y = self.camera.look_at.y + self.camera.distance * rad_pitch.sin();
        let cam_z = self.camera.look_at.z + self.camera.distance * rad_pitch.cos() * rad_yaw.cos();
        let cam_pos = Vec3::new(cam_x, cam_y, cam_z);

        let aspect = width / height;
        let proj = Mat4::perspective_rh(48.0_f32.to_radians(), aspect, 0.1, 100.0);
        let view = Mat4::look_at_rh(cam_pos, self.camera.look_at, Vec3::Y);
        let mvp = proj * view;

        // 3D -> 2D 螢幕頂點投影閉包
        let project = |p: Vec3| -> Option<Pos2> {
            let clip = mvp * Vec4::new(p.x, p.y, p.z, 1.0);
            if clip.w <= 0.05 {
                return None;
            }
            let ndc = Vec3::new(clip.x / clip.w, clip.y / clip.w, clip.z / clip.w);
            if ndc.z > 1.0 || ndc.z < -1.0 {
                return None;
            }
            let screen_x = rect.left() + (ndc.x + 1.0) * 0.5 * width;
            let screen_y = rect.top() + (1.0 - ndc.y) * 0.5 * height;
            Some(Pos2::new(screen_x, screen_y))
        };

        // 2. 繪製地面導航格線
        self.render_ground_grid(painter, &project);

        // 3. 繪製雷達同心距離標線 (1.5m, 2.5m, 3.5m, 4.5m)
        self.render_radar_circles(painter, &project);

        // 4. 繪製核心車身模型 (車輛本體幾何、輪胎與車頭指向箭頭)
        self.render_car(painter, &project);

        // 5. 核心：繪製 3D 無縫環景立體碗面 (Surround Bowl)
        self.render_seamless_surround_bowl(painter, &project, textures);
    }

    fn render_ground_grid<F>(&self, painter: &Painter, project: &F)
    where
        F: Fn(Vec3) -> Option<Pos2>,
    {
        let grid_size = 7;
        let grid_stroke = Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(30, 42, 60, 80));
        for i in -grid_size..=grid_size {
            let fi = i as f32;
            if let (Some(p1), Some(p2)) = (project(Vec3::new(fi, 0.0, -7.0)), project(Vec3::new(fi, 0.0, 7.0))) {
                painter.line_segment([p1, p2], grid_stroke);
            }
            if let (Some(p1), Some(p2)) = (project(Vec3::new(-7.0, 0.0, fi)), project(Vec3::new(7.0, 0.0, fi))) {
                painter.line_segment([p1, p2], grid_stroke);
            }
        }
    }

    fn render_radar_circles<F>(&self, painter: &Painter, project: &F)
    where
        F: Fn(Vec3) -> Option<Pos2>,
    {
        let ring_configs = [
            (1.5, Color32::from_rgb(255, 60, 80)),   // 1.5m 危險區 (紅)
            (2.5, Color32::from_rgb(255, 170, 0)),  // 2.5m 警戒區 (橘)
            (3.5, Color32::from_rgb(0, 230, 140)),  // 3.5m 安全區 (綠)
            (4.5, Color32::from_rgb(0, 210, 255)),  // 4.5m 外圍區 (青)
        ];

        for (radius, color) in ring_configs {
            let mut points = Vec::new();
            for deg in (0..=360).step_by(8) {
                let rad = (deg as f32).to_radians();
                let p = Vec3::new(radius * rad.cos(), 0.02, radius * rad.sin());
                if let Some(pos) = project(p) {
                    points.push(pos);
                }
            }
            if points.len() > 10 {
                for pair in points.windows(2) {
                    painter.line_segment([pair[0], pair[1]], Stroke::new(1.5_f32, color));
                }
            }
        }
    }

    fn render_car<F>(&self, painter: &Painter, project: &F)
    where
        F: Fn(Vec3) -> Option<Pos2>,
    {
        let (sx, sy, sz) = (1.8, 0.75, 4.0);
        let cy = 0.65;
        let car_stroke = Stroke::new(1.8_f32, Color32::from_rgb(0, 220, 255));

        let p_fl = project(Vec3::new(-sx / 2.0, cy - sy / 2.0, -sz / 2.0));
        let p_fr = project(Vec3::new(sx / 2.0, cy - sy / 2.0, -sz / 2.0));
        let p_rl = project(Vec3::new(-sx / 2.0, cy - sy / 2.0, sz / 2.0));
        let p_rr = project(Vec3::new(sx / 2.0, cy - sy / 2.0, sz / 2.0));

        let p_fl_top = project(Vec3::new(-sx / 2.0, cy + sy / 2.0, -sz / 2.0));
        let p_fr_top = project(Vec3::new(sx / 2.0, cy + sy / 2.0, -sz / 2.0));
        let p_rl_top = project(Vec3::new(-sx / 2.0, cy + sy / 2.0, sz / 2.0));
        let p_rr_top = project(Vec3::new(sx / 2.0, cy + sy / 2.0, sz / 2.0));

        let edges = [
            (&p_fl, &p_fr), (&p_fr, &p_rr), (&p_rr, &p_rl), (&p_rl, &p_fl),
            (&p_fl_top, &p_fr_top), (&p_fr_top, &p_rr_top), (&p_rr_top, &p_rl_top), (&p_rl_top, &p_fl_top),
            (&p_fl, &p_fl_top), (&p_fr, &p_fr_top), (&p_rl, &p_rl_top), (&p_rr, &p_rr_top),
        ];

        for (e1, e2) in edges {
            if let (Some(a), Some(b)) = (e1, e2) {
                painter.line_segment([*a, *b], car_stroke);
            }
        }

        // 前方車頭指向箭頭 (向前方 -Z 指示)
        if let (Some(tip), Some(left), Some(right)) = (
            project(Vec3::new(0.0, 1.4, -1.2)),
            project(Vec3::new(-0.35, 1.4, -0.4)),
            project(Vec3::new(0.35, 1.4, -0.4)),
        ) {
            let arrow_stroke = Stroke::new(2.5_f32, Color32::from_rgb(0, 255, 180));
            painter.line_segment([tip, left], arrow_stroke);
            painter.line_segment([tip, right], arrow_stroke);
            painter.line_segment([left, right], arrow_stroke);
        }
    }

    /// 核心：渲染 360 度連續無縫環景立體碗面
    fn render_seamless_surround_bowl<F>(
        &self,
        painter: &Painter,
        project: &F,
        textures: &[Option<TextureId>; 4],
    ) where
        F: Fn(Vec3) -> Option<Pos2>,
    {
        // 4 個相機象限的角度區間（嚴格頭尾相接，構成 360° 完整圓周）：
        // Front:  -45° ~ +45°   (對應前視，以 -Z 軸為中心 0°)
        // Right:  +45° ~ +135°  (對應右視，以 +X 軸為中心 90°)
        // Rear:   +135° ~ +225° (對應後視，以 +Z 軸為中心 180°)
        // Left:   +225° ~ +315° (即 -45°，對應左視，以 -X 軸為中心 270°)
        let sectors = [
            (-45.0_f32, 45.0_f32, textures[0], "FRONT (前)"),
            (45.0_f32, 135.0_f32, textures[3], "RIGHT (右)"),
            (135.0_f32, 225.0_f32, textures[1], "REAR (後)"),
            (225.0_f32, 315.0_f32, textures[2], "LEFT (左)"),
        ];

        let num_radial_segments = 8; // 每個扇區細分 8 段弧形面片

        let r_inner = 1.35_f32; // 車身內緣半徑
        let (r_mid, y_mid, r_outer, y_top) = match self.view_mode {
            ViewMode::SurroundBowl => (
                self.floor_radius,      // 地面延伸半徑
                0.05_f32,               // 地面平鋪微升高度
                self.wall_radius,       // 碗壁頂部外擴半徑
                self.wall_height,       // 碗壁揚起高度
            ),
            ViewMode::SeamlessCylinder => (
                self.floor_radius,
                self.wall_height * 0.5,
                self.floor_radius,      // 垂直圓柱：半徑上下一致
                self.wall_height,
            ),
        };

        let v_mid_split = 0.55_f32; // 紋理切分：下半部 55%~100% 作為地面，0%~55% 作為立體立壁

        for (start_deg, end_deg, tex_opt, label) in sectors {
            // 每個扇區構建帶動態材質的立體連續網格
            if let Some(tex) = tex_opt {
                let mut mesh = Mesh::with_texture(tex);

                for seg in 0..num_radial_segments {
                    let t0 = seg as f32 / num_radial_segments as f32;
                    let t1 = (seg + 1) as f32 / num_radial_segments as f32;

                    let deg0 = start_deg + t0 * (end_deg - start_deg);
                    let deg1 = start_deg + t1 * (end_deg - start_deg);

                    let rad0 = deg0.to_radians();
                    let rad1 = deg1.to_radians();

                    // 沿圓周的 3D 座標 (X: 橫向 sin, Z: 縱向 -cos)
                    // 1. 車身內圈頂點 (Inner)
                    let p_in_0 = Vec3::new(r_inner * rad0.sin(), 0.02, -r_inner * rad0.cos());
                    let p_in_1 = Vec3::new(r_inner * rad1.sin(), 0.02, -r_inner * rad1.cos());

                    // 2. 地面邊緣頂點 (Middle Floor)
                    let p_mid_0 = Vec3::new(r_mid * rad0.sin(), y_mid, -r_mid * rad0.cos());
                    let p_mid_1 = Vec3::new(r_mid * rad1.sin(), y_mid, -r_mid * rad1.cos());

                    // 3. 碗壁外緣頂點 (Outer Top Wall)
                    let p_top_0 = Vec3::new(r_outer * rad0.sin(), y_top, -r_outer * rad0.cos());
                    let p_top_1 = Vec3::new(r_outer * rad1.sin(), y_top, -r_outer * rad1.cos());

                    // 投影至 2D 螢幕
                    if let (Some(pt_in_0), Some(pt_in_1), Some(pt_mid_0), Some(pt_mid_1), Some(pt_top_0), Some(pt_top_1)) = (
                        project(p_in_0), project(p_in_1),
                        project(p_mid_0), project(p_mid_1),
                        project(p_top_0), project(p_top_1),
                    ) {
                        let u0 = t0;
                        let u1 = t1;
                        let tint = Color32::WHITE;

                        // ====== 第一段：地面過渡平鋪面片 (Inner -> Middle) ======
                        let base_idx_floor = mesh.vertices.len() as u32;
                        mesh.vertices.push(egui::epaint::Vertex { pos: pt_mid_0, uv: Pos2::new(u0, v_mid_split), color: tint });
                        mesh.vertices.push(egui::epaint::Vertex { pos: pt_mid_1, uv: Pos2::new(u1, v_mid_split), color: tint });
                        mesh.vertices.push(egui::epaint::Vertex { pos: pt_in_1,  uv: Pos2::new(u1, 1.0),          color: tint });
                        mesh.vertices.push(egui::epaint::Vertex { pos: pt_in_0,  uv: Pos2::new(u0, 1.0),          color: tint });

                        mesh.add_triangle(base_idx_floor, base_idx_floor + 1, base_idx_floor + 2);
                        mesh.add_triangle(base_idx_floor, base_idx_floor + 2, base_idx_floor + 3);

                        // ====== 第二段：立體碗壁/立面面片 (Middle -> Top) ======
                        let base_idx_wall = mesh.vertices.len() as u32;
                        mesh.vertices.push(egui::epaint::Vertex { pos: pt_top_0, uv: Pos2::new(u0, 0.0),         color: tint });
                        mesh.vertices.push(egui::epaint::Vertex { pos: pt_top_1, uv: Pos2::new(u1, 0.0),         color: tint });
                        mesh.vertices.push(egui::epaint::Vertex { pos: pt_mid_1, uv: Pos2::new(u1, v_mid_split), color: tint });
                        mesh.vertices.push(egui::epaint::Vertex { pos: pt_mid_0, uv: Pos2::new(u0, v_mid_split), color: tint });

                        mesh.add_triangle(base_idx_wall, base_idx_wall + 1, base_idx_wall + 2);
                        mesh.add_triangle(base_idx_wall, base_idx_wall + 2, base_idx_wall + 3);
                    }
                }
                painter.add(mesh);
            }

            // 繪製交界無縫接縫線 (Seam Line) 與方位標記
            if self.show_seam_lines {
                let rad_seam = start_deg.to_radians();
                let seam_p_in = Vec3::new(r_inner * rad_seam.sin(), 0.02, -r_inner * rad_seam.cos());
                let seam_p_top = Vec3::new(r_outer * rad_seam.sin(), y_top, -r_outer * rad_seam.cos());

                if let (Some(p1), Some(p2)) = (project(seam_p_in), project(seam_p_top)) {
                    painter.line_segment([p1, p2], Stroke::new(1.5_f32, Color32::from_rgba_unmultiplied(0, 240, 255, 140)));
                }

                // 扇區中間方位文字標記
                let center_deg = (start_deg + end_deg) * 0.5;
                let center_rad = center_deg.to_radians();
                let label_pos_3d = Vec3::new((r_outer + 0.3) * center_rad.sin(), y_top + 0.2, -(r_outer + 0.3) * center_rad.cos());
                if let Some(pos_2d) = project(label_pos_3d) {
                    painter.text(
                        pos_2d,
                        egui::Align2::CENTER_CENTER,
                        label,
                        egui::FontId::proportional(12.0),
                        Color32::from_rgb(0, 230, 255),
                    );
                }
            }
        }
    }
}
