# DashCam360 - 4 分割車載環景魚眼校正與 3D 無縫立體碗面系統 (Rust Native v0.1)

專為車載 360 環景行車記錄器（Around View Monitoring, AVM）設計的高性能純原生桌面端案頭應用程式。  
**100% 純原生 Rust 打造，徹底拋棄網頁與瀏覽器核心，記憶體超低占用、秒開運行、隨插即用。**

---

## 🌟 核心功能亮點

1. **一鍵 4 象限自動裁切與 FFmpeg `v360` 魚眼透視展開拉直**：
   - 裁切 4 分割影片（前、後、左、右象限）。
   - 調用 FFmpeg `v360=input=fisheye:output=rectilinear` 濾鏡消除魚眼鏡頭的強烈圓弧畸變，將道路、建築與地面標線拉直為標準針孔透視視角。
   - 支援自由自訂象限映射（適配不同廠牌車機的前後左右排列順序）。
   - 支援調整輸入魚眼 FOV（140° ~ 220°）與拉直輸出水平 FOV（70° ~ 140°）。
   - **完全無窗靜默運作**：使用 Windows `CREATE_NO_WINDOW` 旗標，後台非同步即時處理，絕不彈出黑底 CMD 控制台視窗。

2. **真實車載「3D 無縫環景立體碗面 (Surround Bowl)」**：
   - **360° 四方位角對角無縫閉合**：前視、後視、左視、右視在交界處 100% 座標共用接合，徹底消滅角落黑邊斷裂。
   - **「地面平鋪 + 向上碗壁」兩段式立體幾何**：
     - **地面區**：從車身底盤向外平鋪延伸（對應下半部路面、標線與地磚）。
     - **立面碗壁區**：從地面邊緣向上弧形隆起（對應遠方街景與建築），呈現如原廠車機（Porsche / Mercedes / Tesla）般的沉浸立體環景。
   - 支援切換 **【立體碗面 (Surround Bowl)】** 與 **【無縫圓柱 (Cylinder)】** 模式，並支援微調地面半徑與碗壁高度。

3. **滑鼠 360 度互動控制 (Orbit Controls)**：
   - **滑鼠左鍵拖曳**：360° 自由環視旋轉。
   - **滑鼠滾輪**：平滑縮放相機距離。
   - **滑鼠右鍵拖曳**：平移畫面焦點。
   - **快捷視角按鈕**：360° 自由環視、鳥瞰地面全景、車前視角、車後視角、車左視角、車右視角。

4. **原生中文字型自動注入**：
   - 自動掛載 Windows 繁體中文微軟正黑體（Microsoft JhengHei），全介面字體清晰，杜絕方塊亂碼。

5. **極致輕量與可攜性 (Portable)**：
   - 主程式僅不到 **4 MB**，啟動時間小於 **0.05 秒**，記憶體占用僅約 40~70 MB。
   - 支援隨身碟隨插即用，無須在目標電腦安裝 Python、Node 或額外執行環境。

---

## 🚀 快速開始

### 直接執行免安裝版
在 Release 頁面下載 `DashCam360_v0.1_Portable.zip` 解壓縮後，直接雙擊：
```
DashCam360.exe
```

### 從源碼編譯 (Rust)
需要安裝 Rust 工具鏈 (1.75+)：
```powershell
cd dashcam360_rust
cargo build --release
```
編譯產物位於 `dashcam360_rust/target/release/dashcam360_rust.exe`。

---

## 📁 專案架構

```
DashCam360/
├── dashcam360_rust/            # 🦀 Rust 核心工程
│   ├── Cargo.toml              # 依賴清單 (eframe, egui, glam, rfd, crossbeam)
│   └── src/
│       ├── main.rs             # 程式入口點、Windows 子系統與微軟正黑體注入
│       ├── app.rs              # 核心狀態機、暗黑風格 UI 與事件調度
│       ├── renderer3d.rs       # 3D 透視矩陣 (MVP) 與 360° 無縫立體碗面幾何渲染
│       ├── player.rs           # 4 路視訊靜默即時解碼與動態紋理更新
│       └── ffmpeg_worker.rs    # FFmpeg v360 裁切校正管線與示範樣品生成
├── sample_videos/              # 測試示範影片
└── run_rust.bat                # 快捷啟動腳本
```

---

## 📄 License
MIT License
