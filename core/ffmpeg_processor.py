"""
ffmpeg_processor.py
負責將 4 分割車載環景魚眼影片進行裁切與 FFmpeg v360 濾鏡畸變修正拉直。
支援即時進度監聽、自訂象限映射、自訂 FOV 與俯仰角參數。
"""

import os
import sys
import re
import json
import subprocess
import threading
from typing import Callable, Optional, Dict, Any

if sys.platform == "win36":
    pass

if sys.platform == "win32":
    try:
        sys.stdout.reconfigure(encoding="utf-8")
        sys.stderr.reconfigure(encoding="utf-8")
    except Exception:
        pass


def get_executable_path(name: str) -> str:
    """尋找 ffmpeg/ffprobe 執行檔，優先使用當前目錄或 sys._MEIPASS 中的隨附版本"""
    base_dirs = [
        getattr(sys, "_MEIPASS", ""),
        os.path.dirname(os.path.abspath(sys.argv[0])),
        os.path.abspath(os.path.join(os.path.dirname(__file__), "..")),
        os.path.dirname(os.path.abspath(__file__))
    ]
    for b in base_dirs:
        if b:
            candidate = os.path.join(b, f"{name}.exe" if sys.platform == "win32" else name)
            if os.path.exists(candidate):
                return candidate
    return name


def get_video_info(video_path: str) -> Dict[str, Any]:
    """使用 ffprobe 獲取影片詳細規格資訊"""
    if not os.path.exists(video_path):
        raise FileNotFoundError(f"找不到檔案: {video_path}")

    ffprobe_bin = get_executable_path("ffprobe")
    cmd = [
        ffprobe_bin,
        "-v", "quiet",
        "-print_format", "json",
        "-show_format",
        "-show_streams",
        video_path
    ]
    try:
        proc = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, check=True)
        data = json.loads(proc.stdout)
        video_stream = next((s for s in data.get("streams", []) if s.get("codec_type") == "video"), None)
        if not video_stream:
            raise ValueError("找不到視訊流 (Video Stream)")

        width = int(video_stream.get("width", 1920))
        height = int(video_stream.get("height", 1080))
        
        # 取得時長
        duration = float(data.get("format", {}).get("duration", video_stream.get("duration", 0.0)))
        
        # 取得幀率
        fps_str = video_stream.get("r_frame_rate", "30/1")
        if "/" in fps_str:
            num, den = fps_str.split("/")
            fps = float(num) / float(den) if float(den) != 0 else 30.0
        else:
            fps = float(fps_str)

        return {
            "path": video_path,
            "filename": os.path.basename(video_path),
            "width": width,
            "height": height,
            "duration": duration,
            "fps": round(fps, 2),
            "codec": video_stream.get("codec_name", "unknown")
        }
    except Exception as e:
        print(f"ffprobe 解析失敗: {e}")
        return {
            "path": video_path,
            "filename": os.path.basename(video_path),
            "width": 1920,
            "height": 1080,
            "duration": 5.0,
            "fps": 30.0,
            "codec": "h264"
        }


class FFmpegProcessor:
    def __init__(self):
        self.is_running = False
        self._cancel_requested = False
        self.process: Optional[subprocess.Popen] = None
        self.last_stderr = []

    def cancel(self):
        self._cancel_requested = True
        if self.process:
            try:
                self.process.terminate()
            except Exception:
                pass

    def process_quad_video(
        self,
        input_path: str,
        output_dir: str = "output",
        mapping: Optional[Dict[str, str]] = None,
        in_fov: int = 180,
        out_hfov: int = 110,
        out_vfov: int = 80,
        pitch_adjust: Optional[Dict[str, int]] = None,
        progress_cb: Optional[Callable[[float, str], None]] = None
    ) -> Dict[str, str]:
        """
        核心一鍵處理：
        1. 裁切 4 象限
        2. 套用 v360 魚眼拉直濾鏡 (input=fisheye:output=rectilinear)
        3. 輸出 4 路獨立影片 (front.mp4, rear.mp4, left.mp4, right.mp4) 以及 quad_rectified.mp4
        """
        self.is_running = True
        self._cancel_requested = False
        self.last_stderr = []
        os.makedirs(output_dir, exist_ok=True)

        if mapping is None:
            mapping = {
                "top_left": "front",
                "top_right": "rear",
                "bottom_left": "left",
                "bottom_right": "right"
            }

        if pitch_adjust is None:
            pitch_adjust = {
                "front": 0,
                "rear": 0,
                "left": 0,
                "right": 0
            }

        # 獲取影片時長
        info = get_video_info(input_path)
        total_duration = max(info.get("duration", 1.0), 1.0)

        # 象限裁切定義
        quad_crops = {
            "top_left": "crop=iw/2:ih/2:0:0",
            "top_right": "crop=iw/2:ih/2:iw/2:0",
            "bottom_left": "crop=iw/2:ih/2:0:ih/2",
            "bottom_right": "crop=iw/2:ih/2:iw/2:ih/2"
        }

        filter_parts = []
        outputs_map = {}

        for quad_pos, role in mapping.items():
            crop_cmd = quad_crops.get(quad_pos, "crop=iw/2:ih/2:0:0")
            pitch = pitch_adjust.get(role, 0)
            pitch_str = f":pitch={pitch}" if pitch != 0 else ""
            
            # 建立濾鏡鏈並 split 複製一份給四合一預覽
            filter_parts.append(
                f"[0:v]{crop_cmd},"
                f"v360=input=fisheye:output=rectilinear:ih_fov={in_fov}:iv_fov={in_fov}:h_fov={out_hfov}:v_fov={out_vfov}{pitch_str},"
                f"format=yuv420p,split=2[{role}][{role}_q]"
            )
            outputs_map[role] = os.path.join(output_dir, f"{role}.mp4")

        # 建立 2x2 合成預覽 (front, rear / left, right)
        filter_parts.append("[front_q][rear_q]hstack[top_row]")
        filter_parts.append("[left_q][right_q]hstack[bot_row]")
        filter_parts.append("[top_row][bot_row]vstack,format=yuv420p[quad_out]")
        quad_preview_path = os.path.join(output_dir, "quad_rectified.mp4")

        filter_complex_str = ";".join(filter_parts)

        # 組裝 FFmpeg 命令
        ffmpeg_bin = get_executable_path("ffmpeg")
        cmd = [
            ffmpeg_bin, "-y",
            "-i", input_path,
            "-filter_complex", filter_complex_str,
            "-map", "[front]", "-c:v", "libx264", "-preset", "veryfast", "-crf", "20", "-pix_fmt", "yuv420p", "-movflags", "+faststart", outputs_map["front"],
            "-map", "[rear]",  "-c:v", "libx264", "-preset", "veryfast", "-crf", "20", "-pix_fmt", "yuv420p", "-movflags", "+faststart", outputs_map["rear"],
            "-map", "[left]",  "-c:v", "libx264", "-preset", "veryfast", "-crf", "20", "-pix_fmt", "yuv420p", "-movflags", "+faststart", outputs_map["left"],
            "-map", "[right]", "-c:v", "libx264", "-preset", "veryfast", "-crf", "20", "-pix_fmt", "yuv420p", "-movflags", "+faststart", outputs_map["right"],
            "-map", "[quad_out]", "-c:v", "libx264", "-preset", "veryfast", "-crf", "22", "-pix_fmt", "yuv420p", "-movflags", "+faststart", quad_preview_path,
            "-progress", "pipe:1"
        ]

        if progress_cb:
            progress_cb(2.0, "FFmpeg 開始並行裁切與 v360 魚眼拉直運算...")

        try:
            self.process = subprocess.Popen(
                cmd,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
                bufsize=1,
                universal_newlines=True
            )

            # 在獨立線程中讀取 stderr 避免 buffer 溢出阻塞
            def read_stderr():
                try:
                    for l in iter(self.process.stderr.readline, ""):
                        if l:
                            self.last_stderr.append(l)
                            if len(self.last_stderr) > 50:
                                self.last_stderr.pop(0)
                except Exception:
                    pass

            stderr_thread = threading.Thread(target=read_stderr, daemon=True)
            stderr_thread.start()

            # 主線程監聽 stdout 的 progress 資訊
            time_regex = re.compile(r"out_time_us=(\d+)")
            fps_regex = re.compile(r"fps=([\d\.]+)")
            curr_fps = "30"

            while True:
                line = self.process.stdout.readline()
                if not line and self.process.poll() is not None:
                    break
                if self._cancel_requested:
                    self.process.terminate()
                    if progress_cb:
                        progress_cb(0, "處理已取消")
                    return {}
                if not line:
                    continue

                fps_match = fps_regex.search(line)
                if fps_match:
                    curr_fps = fps_match.group(1)

                time_match = time_regex.search(line)
                if time_match and progress_cb:
                    time_us = int(time_match.group(1))
                    curr_sec = time_us / 1_000_000.0
                    pct = min(98.0, max(2.0, (curr_sec / total_duration) * 100.0))
                    progress_cb(pct, f"處理中: {pct:.1f}% ({curr_sec:.1f}s / {total_duration:.1f}s, @{curr_fps}fps)")

            self.process.wait()
            stderr_thread.join(timeout=1.0)

            if self.process.returncode != 0:
                err_text = "".join(self.last_stderr)
                raise RuntimeError(f"FFmpeg 處理失敗 (code {self.process.returncode}):\n{err_text}")

            if progress_cb:
                progress_cb(100.0, "v360 魚眼校正與 4 路分離處理完成！")

            result_paths = {
                "front": os.path.abspath(outputs_map["front"]),
                "rear": os.path.abspath(outputs_map["rear"]),
                "left": os.path.abspath(outputs_map["left"]),
                "right": os.path.abspath(outputs_map["right"]),
                "quad": os.path.abspath(quad_preview_path)
            }
            return result_paths

        finally:
            self.is_running = False
            self.process = None


if __name__ == "__main__":
    sample = "sample_videos/demo_quad_fisheye.mp4"
    if os.path.exists(sample):
        proc = FFmpegProcessor()
        def on_prog(p, msg):
            print(f"[{p:5.1f}%] {msg}")
        res = proc.process_quad_video(sample, progress_cb=on_prog)
        print("輸出結果:", res)
