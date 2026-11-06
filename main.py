"""
main.py
DashCam360 案頭應用程式主程式
使用 PySide6 (Qt6 QWebEngineView + QWebChannel) 與 Three.js 打造高流暢度 3D 汽車環景動態投影檢視系統。
"""

import os
import sys
import json
import threading

# Windows 編碼相容
if sys.platform == "win32":
    try:
        sys.stdout.reconfigure(encoding="utf-8")
        sys.stderr.reconfigure(encoding="utf-8")
    except Exception:
        pass

from core.web_server import LocalWebServer
from core.ffmpeg_processor import FFmpegProcessor
from core.demo_generator import generate_demo_quad_video

# 嘗試載入 PySide6
try:
    from PySide6.QtCore import QObject, Slot, Signal, QUrl
    from PySide6.QtWidgets import QApplication, QMainWindow, QFileDialog, QMessageBox
    from PySide6.QtWebEngineWidgets import QWebEngineView
    from PySide6.QtWebEngineCore import QWebEngineSettings
    from PySide6.QtWebChannel import QWebChannel
    PYSIDE_AVAILABLE = True
except ImportError:
    PYSIDE_AVAILABLE = False


class Bridge(QObject):
    """Qt 與 JavaScript 雙向通訊橋樑"""
    progressUpdated = Signal(float, str)
    processingComplete = Signal(str)
    fileSelected = Signal(str)

    def __init__(self, main_window, processor: FFmpegProcessor, server_port: int):
        super().__init__()
        self.main_window = main_window
        self.processor = processor
        self.server_port = server_port

    @Slot()
    def openFileDialog(self):
        """開啟原生檔案選取對話框"""
        filePath, _ = QFileDialog.getOpenFileName(
            self.main_window,
            "選取 4 分割汽車環景魚眼影片",
            "",
            "視訊檔案 (*.mp4 *.mkv *.avi *.mov *.ts);;所有檔案 (*.*)"
        )
        if filePath:
            self.fileSelected.emit(filePath)

    @Slot()
    def generateDemoVideo(self):
        """在背景線程生成示範魚眼測試影片"""
        def run():
            out_sample = os.path.abspath("sample_videos/demo_quad_fisheye.mp4")
            self.progressUpdated.emit(10.0, "正在生成合成測試畫面...")
            success = generate_demo_quad_video(output_path=out_sample, duration=5, fps=25)
            if success:
                self.progressUpdated.emit(100.0, "示範影片產生成功！")
                self.fileSelected.emit(out_sample)
            else:
                self.progressUpdated.emit(0.0, "示範影片生成失敗")

        threading.Thread(target=run, daemon=True).start()

    @Slot(str, str, int, int)
    def startProcessing(self, inputPath: str, mappingJson: str, inFov: int, outFov: int):
        """啟動 FFmpeg 裁切與 v360 魚眼校正拉直"""
        def run():
            try:
                mapping = json.loads(mappingJson)
            except Exception:
                mapping = None

            def on_progress(pct: float, msg: str):
                self.progressUpdated.emit(pct, msg)

            try:
                # 執行處理
                results = self.processor.process_quad_video(
                    input_path=inputPath,
                    output_dir=os.path.abspath("output"),
                    mapping=mapping,
                    in_fov=inFov,
                    out_hfov=outFov,
                    out_vfov=80,
                    progress_cb=on_progress
                )

                # 轉換為本地伺服器 HTTP URL
                web_results = {
                    "front": f"http://127.0.0.1:{self.server_port}/output/front.mp4?t={os.urandom(4).hex()}",
                    "rear": f"http://127.0.0.1:{self.server_port}/output/rear.mp4?t={os.urandom(4).hex()}",
                    "left": f"http://127.0.0.1:{self.server_port}/output/left.mp4?t={os.urandom(4).hex()}",
                    "right": f"http://127.0.0.1:{self.server_port}/output/right.mp4?t={os.urandom(4).hex()}"
                }
                self.processingComplete.emit(json.dumps(web_results))

            except Exception as e:
                self.progressUpdated.emit(0.0, f"錯誤: {str(e)}")

        threading.Thread(target=run, daemon=True).start()


class MainWindow(QMainWindow):
    def __init__(self, server_port: int):
        super().__init__()
        self.server_port = server_port
        self.setWindowTitle("DashCam360 - 汽車環景魚眼校正與 3D 虛擬動態投影系統")
        self.resize(1380, 860)

        self.processor = FFmpegProcessor()

        # 建立 QWebEngineView
        self.browser = QWebEngineView(self)
        settings = self.browser.settings()
        settings.setAttribute(QWebEngineSettings.WebAttribute.WebGLEnabled, True)
        settings.setAttribute(QWebEngineSettings.WebAttribute.Accelerated2dCanvasEnabled, True)
        settings.setAttribute(QWebEngineSettings.WebAttribute.LocalContentCanAccessRemoteUrls, True)
        settings.setAttribute(QWebEngineSettings.WebAttribute.LocalContentCanAccessFileUrls, True)

        # 設定 WebChannel 橋樑
        self.channel = QWebChannel(self)
        self.bridge = Bridge(self, self.processor, server_port)
        self.channel.registerObject("pyBridge", self.bridge)
        self.browser.page().setWebChannel(self.channel)

        self.setCentralWidget(self.browser)
        self.browser.load(QUrl(f"http://127.0.0.1:{server_port}/index.html"))


def main():
    # 1. 啟動本地串流伺服器
    server = LocalWebServer(port=8877)
    port = server.start()

    # 檢查是否指定僅以網頁模式開啟 (例如沒有 GUI 顯示卡或遠端桌面)
    if "--web-only" in sys.argv or not PYSIDE_AVAILABLE:
        import webbrowser
        url = f"http://127.0.0.1:{port}/index.html"
        print(f"\n==========================================")
        print(f"以網頁伺服器模式運行")
        print(f"請在瀏覽器開啟: {url}")
        print(f"==========================================\n")
        webbrowser.open(url)
        try:
            import time
            while True:
                time.sleep(1)
        except KeyboardInterrupt:
            server.stop()
            print("伺服器已關閉。")
        return

    # 2. 啟動 PySide6 桌面案頭視窗
    app = QApplication(sys.argv)
    window = MainWindow(port)
    window.show()

    exit_code = app.exec()
    server.stop()
    sys.exit(exit_code)


if __name__ == "__main__":
    main()
