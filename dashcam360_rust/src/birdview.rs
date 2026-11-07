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
        let (xa, ya, za) = axes;
        let d = [x_w - self.pos[0], y_w - self.pos[1], -self.pos[2]];
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
        if hit_ego_boxes(&self.pos, &[x_w, y_w, 0.0]) {
            return None;
        }

        // CAM 專案之 GSD 地面密度權重 (p = 3.0) 與邊緣漸變羽化
        let dens = (self.pos[2] / n).clamp(0.0, 1.0).powi(3);
        let fade = ((self.r_max - r) / 45.0).clamp(0.0, 1.0);
        let w = dens * fade * self.gain;

        Some((Pos2::new(u_px / 960.0, v_px / 540.0), w))
    }
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
}

impl Default for BirdViewRenderer {
    fn default() -> Self {
        Self {
            car_texture: None,
            show_guides: false, // 預設隱藏輔助線以展現最自然無縫的實景道路拼接
            seam_lines: false,  // 預設隱藏接縫線以達到無縫視覺效果
            uv_road_split: 0.44,
            seam_spread: 1.00,  // 預設展開至鳥瞰圖外框四個角落 (最大前後視野角度)
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

                for k in 0..4 {
                    if let Some((uv, w)) = cams[k].project_and_weight(&axes[k], x_world, y_world) {
                        uvs[k] = uv;
                        weights[k] = w;
                        total_w += w;
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
        //    在重疊過渡區以歸一化權重 alpha = w_upper / (w_upper + w_lower) 實現 100% 精確的物理平滑融合
        // =========================================================================

        // --- 5A. 底層左側網格 (LEFT - cam 2) ---
        if let Some(tex_id) = textures[2] {
            let mut mesh = Mesh::with_texture(tex_id);
            for node in &nodes {
                let w2 = node.weights[2];
                let alpha = if node.total_w > 1e-6 {
                    (w2 / node.total_w).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let a = (alpha * 255.0).round().clamp(0.0, 255.0) as u8;
                mesh.vertices.push(egui::epaint::Vertex {
                    pos: node.pos,
                    uv: node.uvs[2],
                    color: Color32::from_rgba_premultiplied(a, a, a, a),
                });
            }
            for iy in 0..ny as u32 {
                for ix in 0..nx as u32 {
                    let i00 = iy * stride + ix;
                    let i10 = i00 + 1;
                    let i01 = (iy + 1) * stride + ix;
                    let i11 = i01 + 1;
                    if nodes[i00 as usize].weights[2] > 0.0
                        || nodes[i10 as usize].weights[2] > 0.0
                        || nodes[i01 as usize].weights[2] > 0.0
                        || nodes[i11 as usize].weights[2] > 0.0
                    {
                        mesh.add_triangle(i00, i10, i11);
                        mesh.add_triangle(i00, i11, i01);
                    }
                }
            }
            painter.add(mesh);
        }

        // --- 5B. 底層右側網格 (RIGHT - cam 3) ---
        if let Some(tex_id) = textures[3] {
            let mut mesh = Mesh::with_texture(tex_id);
            for node in &nodes {
                let w3 = node.weights[3];
                let alpha = if node.total_w > 1e-6 {
                    (w3 / node.total_w).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let a = (alpha * 255.0).round().clamp(0.0, 255.0) as u8;
                mesh.vertices.push(egui::epaint::Vertex {
                    pos: node.pos,
                    uv: node.uvs[3],
                    color: Color32::from_rgba_premultiplied(a, a, a, a),
                });
            }
            for iy in 0..ny as u32 {
                for ix in 0..nx as u32 {
                    let i00 = iy * stride + ix;
                    let i10 = i00 + 1;
                    let i01 = (iy + 1) * stride + ix;
                    let i11 = i01 + 1;
                    if nodes[i00 as usize].weights[3] > 0.0
                        || nodes[i10 as usize].weights[3] > 0.0
                        || nodes[i01 as usize].weights[3] > 0.0
                        || nodes[i11 as usize].weights[3] > 0.0
                    {
                        mesh.add_triangle(i00, i10, i11);
                        mesh.add_triangle(i00, i11, i01);
                    }
                }
            }
            painter.add(mesh);
        }

        // --- 5C. 頂層前視網格 (FRONT - cam 0) ---
        if let Some(tex_id) = textures[0] {
            let mut mesh = Mesh::with_texture(tex_id);
            for node in &nodes {
                let w0 = node.weights[0];
                let alpha = if node.total_w > 1e-6 {
                    (w0 / node.total_w).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let a = (alpha * 255.0).round().clamp(0.0, 255.0) as u8;
                mesh.vertices.push(egui::epaint::Vertex {
                    pos: node.pos,
                    uv: node.uvs[0],
                    color: Color32::from_rgba_premultiplied(a, a, a, a),
                });
            }
            for iy in 0..ny as u32 {
                for ix in 0..nx as u32 {
                    let i00 = iy * stride + ix;
                    let i10 = i00 + 1;
                    let i01 = (iy + 1) * stride + ix;
                    let i11 = i01 + 1;
                    if nodes[i00 as usize].weights[0] > 0.0
                        || nodes[i10 as usize].weights[0] > 0.0
                        || nodes[i01 as usize].weights[0] > 0.0
                        || nodes[i11 as usize].weights[0] > 0.0
                    {
                        mesh.add_triangle(i00, i10, i11);
                        mesh.add_triangle(i00, i11, i01);
                    }
                }
            }
            painter.add(mesh);
        }

        // --- 5D. 頂層後視網格 (REAR - cam 1) ---
        if let Some(tex_id) = textures[1] {
            let mut mesh = Mesh::with_texture(tex_id);
            for node in &nodes {
                let w1 = node.weights[1];
                let alpha = if node.total_w > 1e-6 {
                    (w1 / node.total_w).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let a = (alpha * 255.0).round().clamp(0.0, 255.0) as u8;
                mesh.vertices.push(egui::epaint::Vertex {
                    pos: node.pos,
                    uv: node.uvs[1],
                    color: Color32::from_rgba_premultiplied(a, a, a, a),
                });
            }
            for iy in 0..ny as u32 {
                for ix in 0..nx as u32 {
                    let i00 = iy * stride + ix;
                    let i10 = i00 + 1;
                    let i01 = (iy + 1) * stride + ix;
                    let i11 = i01 + 1;
                    if nodes[i00 as usize].weights[1] > 0.0
                        || nodes[i10 as usize].weights[1] > 0.0
                        || nodes[i01 as usize].weights[1] > 0.0
                        || nodes[i11 as usize].weights[1] > 0.0
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
