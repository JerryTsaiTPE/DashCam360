# DashCam360 - 4 分割車載環景魚眼校正與 360 度 AVM 影像系統 (Rust Native)

專為車載 360 環景行車記錄器（Around View Monitoring, AVM）設計的高性能純原生桌面端案頭應用程式。  
**100% 純原生 Rust 打造，徹底拋棄網頁與瀏覽器核心，記憶體超低占用、秒開運行、隨插即用。**

---

## 🌟 核心功能亮點

1. **360 度 AVM 同步播放與像素級精準固化輸出 (FFmpeg `remap` 核心)**：
   - **所見即所得 (WYSIWYG)**：輸出影片不再受制於傳統固定拉直限制，直接依據當前調整的 Zoom、FOV、三軸角度微調（Pitch/Yaw/Roll）與 X/Y 軸平移，動態生成 16-bit 像素重映射表（`remap`），使輸出影片與在軟體畫面上看到的畫面 **100% 像素級絕對一致**。
   - **四象限排版指定**：
     - **左上**：前視 (Front)
     - **右上**：後視 (Rear)
     - **左下**：左視 (Left)
     - **右下**：右視 (Right)
   - **頂欄獨立時間戳記 (0% 遮擋畫面)**：頂部規劃 60px 獨立純黑資訊列，完整擷取原始後視右上角時間戳記（440x56，頭尾字元安全保留不裁切）置於最右上角，整體畫面保持標準 1920x1080，時間戳記與 4 路畫面完全零重疊。
   - **4 路單路分離同步輸出**：同步產生校正後的 `front.mp4`、`rear.mp4`、`left.mp4`、`right.mp4`（960x540 標準 16:9）。

2. **鳥瞰圖 (BirdView) 專用無縫拼接校正**：
   - 移植實車地面幾何標定與鳥瞰圖融合拼接算法，精準校正魚眼鏡頭地面投影。
   - 鳥瞰圖專用校正與 AVM 同步播放之微調參數完全獨立解耦，互不干擾。
   - 提供光心對準、展開視場角、縮放、Pitch/Yaw/Roll 角度微調與 X/Y 平移控制。

3. **未裁切原始影片 2x2 播放分頁**：
   - 提供專屬分頁，原汁原味以 2x2 佈局同步播放未裁切、未拉直的 4 分割原始魚眼影片，方便隨時核對原始畫面細節。

4. **主視角切換佈局 (前後左右十字直覺控制)**：
   - 中央主視角切換按鈕依據前後左右物理相對位置佈局，操作直覺順手。

5. **全車校正參數檔匯入/匯出 (JSON)**：
   - 支援將包含各路相機微調、鳥瞰圖拼接、象限對應等全車參數一鍵匯出為 JSON 檔案，方便備份或套用至不同車輛。

6. **原生中文字型自動注入**：
   - 自動掛載 Windows 繁體中文微軟正黑體（Microsoft JhengHei），全介面字體清晰，杜絕方塊亂碼。

7. **極致輕量與可攜性 (Portable)**：
   - 主程式僅約 **5 MB**，啟動時間小於 **0.05 秒**，記憶體占用低。
   - 支援隨身碟隨插即用，無須在目標電腦安裝額外執行環境。

---

## 🚀 快速開始

### 直接執行免安裝版
解壓縮免安裝可攜版資料夾後，直接雙擊：
```
DashCam360.exe
```
或在專案根目錄執行：
```powershell
.\run_rust.bat
```

### 從源碼編譯 (Rust MSVC)
```powershell
$env:PATH = "$env:USERPROFILE\.cargo\msvc_bin;$env:USERPROFILE\.cargo\bin;$env:PATH"
cd dashcam360_rust
cargo xwin build --release --target x86_64-pc-windows-msvc
```
編譯產物位於 `dashcam360_rust/target/x86_64-pc-windows-msvc/release/dashcam360_rust.exe`。

---

## 📁 專案架構

```
DashCam360/
├── dashcam360_rust/            # 🦀 Rust 核心工程
│   ├── Cargo.toml              # 依賴清單 (eframe, egui, glam, rfd, crossbeam)
│   └── src/
│       ├── main.rs             # 程式入口點、Windows 子系統與微軟正黑體注入
│       ├── app.rs              # 核心狀態機、多分頁 UI (AVM/鳥瞰/原始影片) 與事件調度
│       ├── birdview.rs         # 實車地面透視與鳥瞰圖 (BirdView) 幾何渲染
│       ├── player.rs           # 4 路視訊靜默即時解碼與動態紋理更新
│       └── ffmpeg_worker.rs    # FFmpeg 像素級 remap 裁切校正管線與單路分離
├── sample_videos/              # 測試示範影片
└── run_rust.bat                # 快捷啟動腳本
```

---

## 📄 License
MIT License
