"""
web_server.py
輕量本機 HTTP 伺服器，支援 HTTP 206 Partial Content (Range requests)，
確保 Three.js 和 HTML5 <video> 在 seek 時間軸時 100% 流暢無延遲。
"""

import os
import re
import sys
import mimetypes
from http.server import HTTPServer, BaseHTTPRequestHandler
import socketserver
import threading
from urllib.parse import unquote, urlparse

if sys.platform == "win32":
    try:
        sys.stdout.reconfigure(encoding="utf-8")
        sys.stderr.reconfigure(encoding="utf-8")
    except Exception:
        pass


class ThreadedHTTPServer(socketserver.ThreadingMixIn, HTTPServer):
    daemon_threads = True


class StreamingHandler(BaseHTTPRequestHandler):
    base_dir = os.path.abspath(os.path.join(os.path.dirname(__file__), ".."))

    def log_message(self, format, *args):
        # 靜音常規日誌，避免終端機洗版
        pass

    def do_GET(self):
        parsed = urlparse(self.path)
        rel_path = unquote(parsed.path.lstrip("/"))
        if not rel_path:
            rel_path = "web/index.html"

        # 如果路徑不是以 web/ 或 output/ 或 sample_videos/ 開頭，預設映射至 web/
        file_path = os.path.join(self.base_dir, rel_path)
        if not os.path.exists(file_path):
            alt_path = os.path.join(self.base_dir, "web", rel_path)
            if os.path.exists(alt_path):
                file_path = alt_path
            else:
                self.send_error(404, "File Not Found")
                return

        if os.path.isdir(file_path):
            file_path = os.path.join(file_path, "index.html")
            if not os.path.exists(file_path):
                self.send_error(404, "Index Not Found")
                return

        file_size = os.path.getsize(file_path)
        mime_type, _ = mimetypes.guess_type(file_path)
        if not mime_type:
            if file_path.endswith(".js"):
                mime_type = "application/javascript"
            elif file_path.endswith(".css"):
                mime_type = "text/css"
            elif file_path.endswith(".mp4"):
                mime_type = "video/mp4"
            else:
                mime_type = "application/octet-stream"

        range_header = self.headers.get("Range", None)
        if range_header and file_path.endswith(".mp4"):
            # 處理 Range 請求 (用於影片 seek)
            range_match = re.match(r"bytes=(\d+)-(\d*)", range_header)
            if range_match:
                start = int(range_match.group(1))
                end = int(range_match.group(2)) if range_match.group(2) else file_size - 1
                end = min(end, file_size - 1)
                length = end - start + 1

                self.send_response(206)
                self.send_header("Content-Type", mime_type)
                self.send_header("Content-Range", f"bytes {start}-{end}/{file_size}")
                self.send_header("Content-Length", str(length))
                self.send_header("Accept-Ranges", "bytes")
                self.send_header("Access-Control-Allow-Origin", "*")
                self.end_headers()

                with open(file_path, "rb") as f:
                    f.seek(start)
                    chunk_size = 64 * 1024
                    bytes_to_send = length
                    while bytes_to_send > 0:
                        chunk = f.read(min(chunk_size, bytes_to_send))
                        if not chunk:
                            break
                        try:
                            self.wfile.write(chunk)
                            bytes_to_send -= len(chunk)
                        except (BrokenPipeError, ConnectionResetError):
                            break
                return

        # 一般檔案傳輸 (200 OK)
        self.send_response(200)
        self.send_header("Content-Type", mime_type)
        self.send_header("Content-Length", str(file_size))
        self.send_header("Accept-Ranges", "bytes")
        self.send_header("Access-Control-Allow-Origin", "*")
        self.end_headers()

        with open(file_path, "rb") as f:
            shutil_copy = getattr(os, "sendfile", None)
            try:
                while True:
                    buf = f.read(64 * 1024)
                    if not buf:
                        break
                    self.wfile.write(buf)
            except (BrokenPipeError, ConnectionResetError):
                pass


class LocalWebServer:
    def __init__(self, port=8877):
        self.port = port
        self.server = None
        self.thread = None

    def start(self):
        # 尋找可用連接埠
        for p in range(self.port, self.port + 20):
            try:
                self.server = ThreadedHTTPServer(("127.0.0.1", p), StreamingHandler)
                self.port = p
                break
            except OSError:
                continue

        if not self.server:
            raise RuntimeError("無法啟動本地 HTTP 伺服器")

        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        print(f"本地伺服器已在 http://127.0.0.1:{self.port} 啟動")
        return self.port

    def stop(self):
        if self.server:
            self.server.shutdown()
            self.server.server_close()
