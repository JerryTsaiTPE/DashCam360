#![allow(dead_code)]
// src/renderer3d.rs
// 專業車載 3D 無縫環景立體碗面 (Surround Bowl & Seamless Panorama) 渲染模組
// 支援：
// 1. 左右鏡頭安裝於汽車後視鏡下方（車側偏前方位置，而非車身正中間）的非對稱光軸投影與接縫
// 2. 碗面內圈精準貼合車身長方形邊界，且車身於碗面之後繪製，鳥瞰與各視角絕不遮蓋汽車長方形方塊

use egui::{Color32, Mesh, Painter, Pos2, Rect, Stroke, TextureId};
use glam::{Mat4, Vec3, Vec4};

pub const CAR_WIDTH: f32 = 1.8;  // 車寬 1.8m (X: -0.9 ~ +0.9)
pub const CAR_HEIGHT: f32 = 0.75; // 車高 0.75m
pub const CAR_LENGTH: f32 = 4.0; // 車長 4.0m (Z: -2.0 ~ +2.0，車頭朝 -Z)

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
    pub floor_radius: f32,      // 地面橫向延伸半徑 (預設 2.6m，縱向自動依車長比例延伸)
    pub wall_radius: f32,       // 碗壁頂部半徑 (預設 4.4m)
    pub wall_height: f32,       // 碗壁揚起高度 (預設 2.2m)
    pub mirror_offset_z: f32,   // 左右後視鏡鏡頭相對於車身中心向前偏移量 (預設 0.85m 偏前)
    pub show_seam_lines: bool,  // 是否標示 4 方位無縫接縫線與鏡頭軸線
}

impl Default for Surround3DViewer {
    fn default() -> Self {
        Self {
            camera: Camera3D::default(),
            view_mode: ViewMode::SurroundBowl,
            floor_radius: 2.6,
            wall_radius: 4.4,
            wall_height: 2.2,
            mirror_offset_z: 0.85,
            show_seam_lines: true,
        }
    }
}

impl Surround3DViewer {
    pub fn handle_input(&mut self, response: &egui::Response, ui: &egui::Ui) {
        if response.dragged_by(egui::PointerButton::Primary) {
            let delta = response.drag_delta();
            self.camera.yaw += delta.x * 0.45;
            self.camera.pitch = (self.camera.pitch + delta.y * 0.45).clamp(5.0, 89.2);
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

        // 2. 繪製底層地面導航格線
        self.render_ground_grid(painter, &project);

        // 3. 繪製 3D 無縫環景立體碗面 (內圈嚴格貼合車身長方形外緣，不進入車體範圍)
        self.render_seamless_surround_bowl(painter, &project, textures);

        // 4. 繪製雷達同心距離標線 (位於車身外圍)
        self.render_radar_circles(painter, &project);

        // 5. 最後繪製核心車身模型 (確保鳥瞰或任何視角下，汽車長方形方塊與後視鏡鏡頭標記絕不被影片蓋住)
        self.render_car(painter, &project);
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
        let z_elongation = (CAR_LENGTH - CAR_WIDTH) * 0.5; // 1.1m 縱向延伸，貼合長方形車身比例
        let ring_configs = [
            (1.5, Color32::from_rgba_unmultiplied(255, 60, 80, 160)),   // 危險區 (紅)
            (2.5, Color32::from_rgba_unmultiplied(255, 170, 0, 150)),   // 警戒區 (橘)
            (3.5, Color32::from_rgba_unmultiplied(0, 230, 140, 140)),   // 安全區 (綠)
            (4.5, Color32::from_rgba_unmultiplied(0, 210, 255, 130)),   // 外圍區 (青)
        ];

        for (radius_x, color) in ring_configs {
            let radius_z = radius_x + z_elongation;
            let mut points = Vec::new();
            for deg in (0..=360).step_by(8) {
                let rad = (deg as f32).to_radians();
                let p = Vec3::new(radius_x * rad.sin(), 0.03, -radius_z * rad.cos());
                if let Some(pos) = project(p) {
                    points.push(pos);
                }
            }
            if points.len() > 10 {
                for pair in points.windows(2) {
                    painter.line_segment([pair[0], pair[1]], Stroke::new(1.2_f32, color));
                }
            }
        }
    }

    fn render_car<F>(&self, painter: &Painter, project: &F)
    where
        F: Fn(Vec3) -> Option<Pos2>,
    {
        let (sx, sy, sz) = (CAR_WIDTH, CAR_HEIGHT, CAR_LENGTH);
        let hx = sx * 0.5; // 0.9m
        let hz = sz * 0.5; // 2.0m
        let y_bot = 0.03;
        let y_top = y_bot + sy;

        // 1. 先繪製車身底座與車頂的不透明深色填充面，徹底隔絕下方格線或背景，保持車身長方形方塊清晰乾淨
        let p_fl = project(Vec3::new(-hx, y_bot, -hz));
        let p_fr = project(Vec3::new(hx, y_bot, -hz));
        let p_rl = project(Vec3::new(-hx, y_bot, hz));
        let p_rr = project(Vec3::new(hx, y_bot, hz));

        let p_fl_top = project(Vec3::new(-hx, y_top, -hz));
        let p_fr_top = project(Vec3::new(hx, y_top, -hz));
        let p_rl_top = project(Vec3::new(-hx, y_top, hz));
        let p_rr_top = project(Vec3::new(hx, y_top, hz));

        let chassis_fill = Color32::from_rgba_unmultiplied(14, 22, 34, 245);
        let side_fill = Color32::from_rgba_unmultiplied(18, 30, 46, 225);
        let top_fill = Color32::from_rgba_unmultiplied(20, 34, 52, 235);

        // 繪製四個立體側面與底面、頂面填充
        let fill_quad = |painter: &Painter, a: Option<Pos2>, b: Option<Pos2>, c: Option<Pos2>, d: Option<Pos2>, col: Color32| {
            if let (Some(p0), Some(p1), Some(p2), Some(p3)) = (a, b, c, d) {
                let mut m = Mesh::default();
                m.colored_vertex(p0, col);
                m.colored_vertex(p1, col);
                m.colored_vertex(p2, col);
                m.colored_vertex(p3, col);
                m.add_triangle(0, 1, 2);
                m.add_triangle(0, 2, 3);
                painter.add(m);
            }
        };

        fill_quad(painter, p_fl, p_fr, p_rr, p_rl, chassis_fill);
        fill_quad(painter, p_fl, p_fr, p_fr_top, p_fl_top, side_fill);
        fill_quad(painter, p_rl, p_rr, p_rr_top, p_rl_top, side_fill);
        fill_quad(painter, p_fl, p_rl, p_rl_top, p_fl_top, side_fill);
        fill_quad(painter, p_fr, p_rr, p_rr_top, p_fr_top, side_fill);
        fill_quad(painter, p_fl_top, p_fr_top, p_rr_top, p_rl_top, top_fill);

        // 2. 繪製長方形車體外框線條
        let car_stroke = Stroke::new(2.0_f32, Color32::from_rgb(0, 220, 255));
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

        // 3. 繪製前擋風玻璃與座艙輪廓（幫助直覺辨識前座 A 柱與後視鏡相對位置）
        let z_windshield = -self.mirror_offset_z.clamp(0.2, hz - 0.3);
        let z_rear_window = 1.05_f32;
        let cabin_stroke = Stroke::new(1.3_f32, Color32::from_rgba_unmultiplied(0, 190, 235, 160));
        if let (Some(w_l), Some(w_r), Some(rw_l), Some(rw_r)) = (
            project(Vec3::new(-hx * 0.82, y_top + 0.02, z_windshield)),
            project(Vec3::new(hx * 0.82, y_top + 0.02, z_windshield)),
            project(Vec3::new(-hx * 0.82, y_top + 0.02, z_rear_window)),
            project(Vec3::new(hx * 0.82, y_top + 0.02, z_rear_window)),
        ) {
            painter.line_segment([w_l, w_r], cabin_stroke);
            painter.line_segment([rw_l, rw_r], cabin_stroke);
            painter.line_segment([w_l, rw_l], cabin_stroke);
            painter.line_segment([w_r, rw_r], cabin_stroke);
        }

        // 4. 繪製左右後視鏡 (位於車身左右側偏前方 A 柱位置 z = -mirror_offset_z) 與 4 顆鏡頭安裝點
        let mirror_z = -self.mirror_offset_z.clamp(0.0, hz - 0.2);
        let mirror_y = y_top * 0.82;
        let mirror_stroke = Stroke::new(2.5_f32, Color32::from_rgb(255, 200, 40));

        // 左後視鏡支架與鏡體
        if let (Some(m_in), Some(m_out), Some(m_back)) = (
            project(Vec3::new(-hx, mirror_y, mirror_z)),
            project(Vec3::new(-hx - 0.24, mirror_y, mirror_z - 0.04)),
            project(Vec3::new(-hx - 0.24, mirror_y, mirror_z + 0.14)),
        ) {
            painter.line_segment([m_in, m_out], mirror_stroke);
            painter.line_segment([m_out, m_back], mirror_stroke);
            painter.line_segment([m_back, m_in], mirror_stroke);
            painter.circle_filled(m_out, 4.5, Color32::from_rgb(255, 190, 0));
        }

        // 右後視鏡支架與鏡體
        if let (Some(m_in), Some(m_out), Some(m_back)) = (
            project(Vec3::new(hx, mirror_y, mirror_z)),
            project(Vec3::new(hx + 0.24, mirror_y, mirror_z - 0.04)),
            project(Vec3::new(hx + 0.24, mirror_y, mirror_z + 0.14)),
        ) {
            painter.line_segment([m_in, m_out], mirror_stroke);
            painter.line_segment([m_out, m_back], mirror_stroke);
            painter.line_segment([m_back, m_in], mirror_stroke);
            painter.circle_filled(m_out, 4.5, Color32::from_rgb(255, 190, 0));
        }

        // 前視與後視鏡頭安裝點標記
        if let Some(cam_f) = project(Vec3::new(0.0, y_top * 0.6, -hz)) {
            painter.circle_filled(cam_f, 4.0, Color32::from_rgb(0, 255, 180));
        }
        if let Some(cam_r) = project(Vec3::new(0.0, y_top * 0.6, hz)) {
            painter.circle_filled(cam_r, 4.0, Color32::from_rgb(0, 255, 180));
        }

        // 5. 前方車頭指向箭頭 (向前方 -Z 指示)
        if let (Some(tip), Some(left), Some(right)) = (
            project(Vec3::new(0.0, y_top + 0.05, -1.55)),
            project(Vec3::new(-0.38, y_top + 0.05, -0.85)),
            project(Vec3::new(0.38, y_top + 0.05, -0.85)),
        ) {
            let arrow_stroke = Stroke::new(2.6_f32, Color32::from_rgb(0, 255, 180));
            painter.line_segment([tip, left], arrow_stroke);
            painter.line_segment([tip, right], arrow_stroke);
            painter.line_segment([left, right], arrow_stroke);
        }
    }

    /// 核心：渲染 360 度連續無縫環景立體碗面
    /// - 內圈頂點嚴格沿著汽車長方形外框 `[-hx, +hx] x [-hz, +hz]` 分佈，絕不進入或遮擋車身方塊
    /// - 左右鏡頭光軸中心 (`u = 0.5`) 錨定於汽車左右後視鏡下方位置 (`z = -mirror_offset_z`，偏車頭前方)
    fn render_seamless_surround_bowl<F>(
        &self,
        painter: &Painter,
        project: &F,
        textures: &[Option<TextureId>; 4],
    ) where
        F: Fn(Vec3) -> Option<Pos2>,
    {
        let hx = CAR_WIDTH * 0.5;  // 0.9m
        let hz = CAR_LENGTH * 0.5; // 2.0m
        let z_elongation = hz - hx; // 1.1m 縱向延伸，確保前後方地面與左右方地面等寬，絕不向內摺疊蓋住車身

        let rx_mid = self.floor_radius.max(hx + 0.5);
        let rz_mid = rx_mid + z_elongation;

        let (y_mid, rx_outer, rz_outer, y_top) = match self.view_mode {
            ViewMode::SurroundBowl => {
                let wx = self.wall_radius.max(rx_mid + 0.4);
                let wz = wx + z_elongation;
                (0.05_f32, wx, wz, self.wall_height)
            }
            ViewMode::SeamlessCylinder => (
                self.wall_height * 0.5,
                rx_mid,
                rz_mid,
                self.wall_height,
            ),
        };

        // 後視鏡下方鏡頭位置 (車側偏前方：z = -mirror_offset_z)
        let mirror_z = -self.mirror_offset_z.clamp(0.0, hz - 0.2);

        // 四個車角接縫對應的外圍橢圓方位角：
        // 前左 (-38° / 322°)、前右 (+38°)、後右 (+142°)、後左 (+218°)
        let deg_fl_neg = -38.0_f32;
        let deg_fr = 38.0_f32;
        let deg_rr = 142.0_f32;
        let deg_rl = 218.0_f32;
        let deg_fl_pos = 322.0_f32;

        // 左右後視鏡鏡頭光軸中心 (u = 0.5) 對應的外圍橢圓方位角（隨 mirror_offset_z 向前偏移）
        let mirror_angle_shift = (self.mirror_offset_z / hz) * 26.0;
        let deg_mirror_r = 90.0_f32 - mirror_angle_shift;  // 右後視鏡光軸 (約 79°，偏前)
        let deg_mirror_l = 270.0_f32 + mirror_angle_shift; // 左後視鏡光軸 (約 281°，偏前)

        let num_segments = 12; // 每個扇區細分 12 段，確保前段與後段平滑過渡
        let v_mid_split = 0.55_f32; // 紋理切分：下半部 55%~100% 作為地面，0%~55% 作為立體碗壁
        let y_in = 0.02_f32;

        // 定義四個扇區：(role_id, 貼圖, 標籤文字, 標籤角度)
        // role_id: 0 = Front, 1 = Right (右後視鏡), 2 = Rear, 3 = Left (左後視鏡)
        let sector_defs = [
            (0usize, textures[0], "FRONT (車前)", 0.0_f32),
            (1usize, textures[3], "RIGHT (右後視鏡)", deg_mirror_r),
            (2usize, textures[1], "REAR (車後)", 180.0_f32),
            (3usize, textures[2], "LEFT (左後視鏡)", deg_mirror_l),
        ];

        // 根據扇區 role_id 與紋理水平參數 t ∈ [0.0, 1.0]，計算：
        // (車身矩形邊緣內圈點 p_in, 外圈橢圓角度 deg_out)
        let eval_sector_point = |role_id: usize, t: f32| -> (Vec3, f32) {
            match role_id {
                0 => {
                    // FRONT: 內緣沿車頭前保桿 (-hx -> +hx, z = -hz)
                    let x = -hx + t * (2.0 * hx);
                    let deg = deg_fl_neg + t * (deg_fr - deg_fl_neg);
                    (Vec3::new(x, y_in, -hz), deg)
                }
                1 => {
                    // RIGHT (右後視鏡鏡頭):
                    // t = 0.0 -> 前右車角 (+hx, -hz)
                    // t = 0.5 -> 右後視鏡下方鏡頭中心 (+hx, mirror_z) [偏前方]
                    // t = 1.0 -> 後右車角 (+hx, +hz)
                    if t <= 0.5 {
                        let a = t / 0.5;
                        let z = -hz + a * (mirror_z - (-hz));
                        let deg = deg_fr + a * (deg_mirror_r - deg_fr);
                        (Vec3::new(hx, y_in, z), deg)
                    } else {
                        let b = (t - 0.5) / 0.5;
                        let z = mirror_z + b * (hz - mirror_z);
                        let deg = deg_mirror_r + b * (deg_rr - deg_mirror_r);
                        (Vec3::new(hx, y_in, z), deg)
                    }
                }
                2 => {
                    // REAR: 內緣沿車尾後保桿 (+hx -> -hx, z = +hz)
                    let x = hx - t * (2.0 * hx);
                    let deg = deg_rr + t * (deg_rl - deg_rr);
                    (Vec3::new(x, y_in, hz), deg)
                }
                _ => {
                    // LEFT (左後視鏡鏡頭):
                    // t = 0.0 -> 後左車角 (-hx, +hz)
                    // t = 0.5 -> 左後視鏡下方鏡頭中心 (-hx, mirror_z) [偏前方]
                    // t = 1.0 -> 前左車角 (-hx, -hz)
                    if t <= 0.5 {
                        let a = t / 0.5;
                        let z = hz + a * (mirror_z - hz);
                        let deg = deg_rl + a * (deg_mirror_l - deg_rl);
                        (Vec3::new(-hx, y_in, z), deg)
                    } else {
                        let b = (t - 0.5) / 0.5;
                        let z = mirror_z + b * (-hz - mirror_z);
                        let deg = deg_mirror_l + b * (deg_fl_pos - deg_mirror_l);
                        (Vec3::new(-hx, y_in, z), deg)
                    }
                }
            }
        };

        for (role_id, tex_opt, label, label_deg) in sector_defs {
            if let Some(tex) = tex_opt {
                let mut mesh = Mesh::with_texture(tex);

                for seg in 0..num_segments {
                    let t0 = seg as f32 / num_segments as f32;
                    let t1 = (seg + 1) as f32 / num_segments as f32;

                    let (p_in_0, deg0) = eval_sector_point(role_id, t0);
                    let (p_in_1, deg1) = eval_sector_point(role_id, t1);

                    let rad0 = deg0.to_radians();
                    let rad1 = deg1.to_radians();

                    // 2. 地面邊緣頂點 (Middle Floor - 橢圓環繞，確保前後距離充裕)
                    let p_mid_0 = Vec3::new(rx_mid * rad0.sin(), y_mid, -rz_mid * rad0.cos());
                    let p_mid_1 = Vec3::new(rx_mid * rad1.sin(), y_mid, -rz_mid * rad1.cos());

                    // 3. 碗壁外緣頂點 (Outer Top Wall)
                    let p_top_0 = Vec3::new(rx_outer * rad0.sin(), y_top, -rz_outer * rad0.cos());
                    let p_top_1 = Vec3::new(rx_outer * rad1.sin(), y_top, -rz_outer * rad1.cos());

                    // 投影至 2D 螢幕
                    if let (Some(pt_in_0), Some(pt_in_1), Some(pt_mid_0), Some(pt_mid_1), Some(pt_top_0), Some(pt_top_1)) = (
                        project(p_in_0), project(p_in_1),
                        project(p_mid_0), project(p_mid_1),
                        project(p_top_0), project(p_top_1),
                    ) {
                        let u0 = t0;
                        let u1 = t1;
                        let tint = Color32::WHITE;

                        // ====== 第一段：地面過渡平鋪面片 (Inner Car Edge -> Middle Floor) ======
                        let base_idx_floor = mesh.vertices.len() as u32;
                        mesh.vertices.push(egui::epaint::Vertex { pos: pt_mid_0, uv: Pos2::new(u0, v_mid_split), color: tint });
                        mesh.vertices.push(egui::epaint::Vertex { pos: pt_mid_1, uv: Pos2::new(u1, v_mid_split), color: tint });
                        mesh.vertices.push(egui::epaint::Vertex { pos: pt_in_1,  uv: Pos2::new(u1, 1.0),          color: tint });
                        mesh.vertices.push(egui::epaint::Vertex { pos: pt_in_0,  uv: Pos2::new(u0, 1.0),          color: tint });

                        mesh.add_triangle(base_idx_floor, base_idx_floor + 1, base_idx_floor + 2);
                        mesh.add_triangle(base_idx_floor, base_idx_floor + 2, base_idx_floor + 3);

                        // ====== 第二段：立體碗壁/立面面片 (Middle Floor -> Top Wall) ======
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

            // 繪製交界無縫接縫線 (Seam Line)、後視鏡鏡頭光軸線與方位標記
            if self.show_seam_lines {
                let (seam_p_in, seam_deg) = eval_sector_point(role_id, 0.0);
                let rad_seam = seam_deg.to_radians();
                let seam_p_mid = Vec3::new(rx_mid * rad_seam.sin(), y_mid, -rz_mid * rad_seam.cos());
                let seam_p_top = Vec3::new(rx_outer * rad_seam.sin(), y_top, -rz_outer * rad_seam.cos());

                let seam_stroke = Stroke::new(1.6_f32, Color32::from_rgba_unmultiplied(0, 240, 255, 160));
                if let (Some(p1), Some(p2)) = (project(seam_p_in), project(seam_p_mid)) {
                    painter.line_segment([p1, p2], seam_stroke);
                }
                if let (Some(p2), Some(p3)) = (project(seam_p_mid), project(seam_p_top)) {
                    painter.line_segment([p2, p3], seam_stroke);
                }

                // 若為左/右後視鏡扇區，標示後視鏡鏡頭中心光軸 (t = 0.5)
                if role_id == 1 || role_id == 3 {
                    let (axis_in, axis_deg) = eval_sector_point(role_id, 0.5);
                    let axis_rad = axis_deg.to_radians();
                    let axis_mid = Vec3::new(rx_mid * axis_rad.sin(), y_mid, -rz_mid * axis_rad.cos());
                    if let (Some(a1), Some(a2)) = (project(axis_in), project(axis_mid)) {
                        painter.line_segment(
                            [a1, a2],
                            Stroke::new(1.3_f32, Color32::from_rgba_unmultiplied(255, 200, 40, 170)),
                        );
                    }
                }

                // 扇區方位文字標記
                let center_rad = label_deg.to_radians();
                let label_pos_3d = Vec3::new(
                    (rx_outer + 0.35) * center_rad.sin(),
                    y_top + 0.22,
                    -(rz_outer + 0.35) * center_rad.cos(),
                );
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
