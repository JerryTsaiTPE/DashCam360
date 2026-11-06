"""
demo_generator.py
使用 Pillow 與 FFmpeg 生成高度逼真的 4 分割汽車環景魚眼測試影片。
四個象限分別模擬：
  - 左上: 前視 (道路前方、紅色道路邊緣線、藍天街景、車頭弧線)
  - 右上: 後視 (倒車透視、倒車輔助標線、建築街景)
  - 左下: 左側 (車身左緣、俯視地面水溝蓋、人行道)
  - 右下: 右側 (車身右緣、路邊停靠機車輪廓、地磚)
"""

import os
import sys
import math
import subprocess
from PIL import Image, ImageDraw, ImageFont

if sys.platform == "win32":
    try:
        sys.stdout.reconfigure(encoding="utf-8")
        sys.stderr.reconfigure(encoding="utf-8")
    except Exception:
        pass


def get_ffmpeg_path():
    base_dirs = [
        getattr(sys, "_MEIPASS", ""),
        os.path.dirname(os.path.abspath(sys.argv[0])),
        os.path.abspath(os.path.join(os.path.dirname(__file__), "..")),
        os.path.dirname(os.path.abspath(__file__))
    ]
    for b in base_dirs:
        if b:
            candidate = os.path.join(b, "ffmpeg.exe" if sys.platform == "win32" else "ffmpeg")
            if os.path.exists(candidate):
                return candidate
    return "ffmpeg"


def create_quad_frame(frame_idx, total_frames, width=1920, height=1080):
    """生成單一幀 4 分割環景畫面"""
    img = Image.new("RGB", (width, height), (15, 18, 24))
    draw = ImageDraw.Draw(img)

    qw, qh = width // 2, height // 2
    t = frame_idx / 30.0

    # 嘗試載入系統字型
    font_large = None
    font_mid = None
    font_small = None
    for fp in ["C:/Windows/Fonts/arialbd.ttf", "C:/Windows/Fonts/arial.ttf"]:
        if os.path.exists(fp):
            try:
                font_large = ImageFont.truetype(fp, 38)
                font_mid = ImageFont.truetype(fp, 26)
                font_small = ImageFont.truetype(fp, 20)
                break
            except Exception:
                pass
    if font_large is None:
        font_large = ImageFont.load_default()
        font_mid = font_large
        font_small = font_large

    # ---------------- 象限 1: 左上 (FRONT - 車前) ----------------
    # 天空與道路漸層
    draw.rectangle([0, 0, qw, qh // 2], fill=(45, 60, 85))
    draw.rectangle([0, qh // 2, qw, qh], fill=(35, 38, 42))
    
    # 遠方建築輪廓
    for i in range(8):
        bx = i * 120 + 20
        bw = 80
        bh = 70 + (i % 3) * 40
        draw.rectangle([bx, qh // 2 - bh, bx + bw, qh // 2], fill=(25, 35, 50))
    
    # 前方道路透視線 (動態向前移動)
    road_center = qw // 2
    vanish_y = qh // 2
    offset = (frame_idx * 8) % 100
    for dy in range(0, qh // 2, 40):
        y = vanish_y + dy + (offset % 40)
        if y < qh:
            scale = (y - vanish_y) / (qh // 2)
            lw = int(12 * scale)
            draw.rectangle([road_center - lw // 2, y, road_center + lw // 2, y + 15], fill=(240, 200, 30))

    # 紅色路緣邊線 (類似使用者圖片)
    draw.line([road_center - 80, vanish_y, 160, qh], fill=(220, 40, 40), width=6)
    draw.line([road_center + 80, vanish_y, qw - 160, qh], fill=(220, 220, 220), width=5)

    # 前車頭引擎蓋弧形
    draw.chord([-100, qh - 100, qw + 100, qh + 260], 0, 180, fill=(180, 190, 200), outline=(230, 240, 255), width=3)
    
    # 前方標籤
    draw.rectangle([20, 20, 360, 75], fill=(0, 0, 0))
    draw.text((30, 25), "CAM 1: FRONT (前)", fill=(255, 230, 80), font=font_large)
    draw.text((30, 85), "SPEED: 42 km/h  |  FOV 180", fill=(180, 255, 180), font=font_small)

    # ---------------- 象限 2: 右上 (REAR - 車後) ----------------
    rx, ry = qw, 0
    draw.rectangle([rx, ry, width, qh // 2], fill=(50, 55, 65))
    draw.rectangle([rx, qh // 2, width, qh], fill=(40, 42, 45))

    # 後方街景與人行道
    draw.rectangle([rx, qh // 2, rx + qw // 3, qh], fill=(70, 75, 80))
    # 倒車動態標線 (綠-黃-紅)
    draw.line([rx + 180, qh, rx + qw // 2 - 40, qh // 2 + 60], fill=(240, 50, 50), width=5)
    draw.line([width - 180, qh, rx + qw // 2 + 40, qh // 2 + 60], fill=(240, 50, 50), width=5)
    draw.line([rx + 240, qh - 50, width - 240, qh - 50], fill=(240, 50, 50), width=4)
    draw.line([rx + 300, qh - 120, width - 300, qh - 120], fill=(240, 200, 40), width=4)
    draw.line([rx + 340, qh - 180, width - 340, qh - 180], fill=(50, 220, 50), width=4)

    # 後保險桿下緣
    draw.chord([rx - 80, qh - 90, width + 80, qh + 240], 0, 180, fill=(160, 170, 180), outline=(220, 230, 240), width=3)

    # 後方標籤
    draw.rectangle([rx + 20, 20, rx + 360, 75], fill=(0, 0, 0))
    draw.text((rx + 30, 25), "CAM 2: REAR (後)", fill=(255, 160, 60), font=font_large)
    draw.text((rx + 30, 85), "DISTANCE: 1.8m  |  REVERSE", fill=(255, 200, 150), font=font_small)

    # ---------------- 象限 3: 左下 (LEFT - 車左) ----------------
    lx, ly = 0, qh
    draw.rectangle([lx, ly, qw, height], fill=(30, 32, 36))
    
    # 人行道地磚格網
    for gx in range(0, qw - 240, 60):
        draw.line([gx, ly, gx, height], fill=(55, 60, 65), width=2)
    for gy in range(ly, height, 40):
        draw.line([0, gy, qw - 240, gy], fill=(55, 60, 65), width=2)

    # 水溝蓋格柵 (類似使用者圖片)
    draw.rectangle([qw // 4, ly + 80, qw // 4 + 140, ly + 260], fill=(20, 22, 25), outline=(100, 110, 120), width=3)
    for dy in range(ly + 95, ly + 250, 20):
        draw.line([qw // 4 + 10, dy, qw // 4 + 130, dy], fill=(120, 130, 140), width=3)

    # 左側車身弧形 (從右側切入)
    draw.chord([qw - 260, ly - 50, qw + 300, height + 50], 90, 270, fill=(170, 180, 195), outline=(230, 240, 255), width=4)

    # 左側標籤
    draw.rectangle([20, ly + 20, 360, ly + 75], fill=(0, 0, 0))
    draw.text((30, ly + 25), "CAM 3: LEFT (左)", fill=(80, 220, 255), font=font_large)
    draw.text((30, ly + 85), "SIDE MIRROR DOWN-VIEW", fill=(180, 230, 255), font=font_small)

    # ---------------- 象限 4: 右下 (RIGHT - 車右) ----------------
    bx, by = qw, qh
    draw.rectangle([bx, by, width, height], fill=(32, 34, 38))

    # 右側路面與地磚
    for gx in range(bx + 240, width, 60):
        draw.line([gx, by, gx, height], fill=(55, 60, 65), width=2)
    for gy in range(by, height, 40):
        draw.line([bx + 240, gy, width, gy], fill=(55, 60, 65), width=2)

    # 模擬路旁機車輪廓 / 障礙物
    draw.rectangle([width - 240, by + 120, width - 80, by + 320], fill=(50, 30, 30), outline=(220, 60, 60), width=3)
    draw.text((width - 230, by + 180), "OBSTACLE", fill=(255, 100, 100), font=font_mid)
    draw.text((width - 215, by + 220), "SCOOTER", fill=(255, 200, 200), font=font_small)

    # 右側車身弧形 (從左側切入)
    draw.chord([bx - 300, by - 50, bx + 260, height + 50], 270, 90, fill=(170, 180, 195), outline=(230, 240, 255), width=4)

    # 右側標籤
    draw.rectangle([bx + 20, by + 20, bx + 360, by + 75], fill=(0, 0, 0))
    draw.text((bx + 30, by + 25), "CAM 4: RIGHT (右)", fill=(255, 100, 240), font=font_large)
    draw.text((bx + 30, by + 85), "SIDE MIRROR DOWN-VIEW", fill=(255, 200, 250), font=font_small)

    # ---------------- 中央十字分割線與時間戳記 ----------------
    draw.line([0, qh, width, qh], fill=(0, 0, 0), width=6)
    draw.line([qw, 0, qw, height], fill=(0, 0, 0), width=6)

    # 右上方整體時間戳 (如使用者圖片上的 2026-06-12 08:08:53)
    time_str = f"2026-11-04 23:15:{int(t):02d}.{int((t % 1)*10):01d}"
    draw.rectangle([width - 320, 15, width - 20, 50], fill=(0, 0, 0))
    draw.text((width - 310, 20), time_str, fill=(240, 240, 240), font=font_mid)

    return img


def generate_demo_quad_video(output_path="sample_videos/demo_quad_fisheye.mp4", duration=5, fps=25):
    """
    透過生成單幀圖片序列並經由 FFmpeg 編碼成高品質 4 分割魚眼影片。
    四象限各自施加魚眼扭曲效果，完美貼近真實車用魚眼相機！
    """
    os.makedirs(os.path.dirname(os.path.abspath(output_path)), exist_ok=True)
    temp_dir = os.path.join(os.path.dirname(os.path.abspath(output_path)), "_temp_frames")
    os.makedirs(temp_dir, exist_ok=True)

    total_frames = duration * fps
    print(f"正在繪製 {total_frames} 幀示範畫面...")

    try:
        for idx in range(total_frames):
            frame = create_quad_frame(idx, total_frames, 1920, 1080)
            frame.save(os.path.join(temp_dir, f"frame_{idx:04d}.png"))

        print("正在透過 FFmpeg 合成動態魚眼模擬影片...")
        # 分割 4 個象限並各自套用魚眼濾鏡 (v360 rectilinear->fisheye)，然後拼回 2x2
        filter_complex = (
            "[0:v]crop=iw/2:ih/2:0:0,v360=input=rectilinear:output=fisheye:ih_fov=110:iv_fov=80:h_fov=180:v_fov=180[tl];"
            "[0:v]crop=iw/2:ih/2:iw/2:0,v360=input=rectilinear:output=fisheye:ih_fov=110:iv_fov=80:h_fov=180:v_fov=180[tr];"
            "[0:v]crop=iw/2:ih/2:0:ih/2,v360=input=rectilinear:output=fisheye:ih_fov=110:iv_fov=80:h_fov=180:v_fov=180[bl];"
            "[0:v]crop=iw/2:ih/2:iw/2:ih/2,v360=input=rectilinear:output=fisheye:ih_fov=110:iv_fov=80:h_fov=180:v_fov=180[br];"
            "[tl][tr]hstack[top];"
            "[bl][br]hstack[bot];"
            "[top][bot]vstack,format=yuv420p[out]"
        )

        input_pattern = os.path.join(temp_dir, "frame_%04d.png")
        cmd = [
            get_ffmpeg_path(), "-y",
            "-r", str(fps),
            "-i", input_pattern,
            "-filter_complex", filter_complex,
            "-map", "[out]",
            "-c:v", "libx264",
            "-preset", "veryfast",
            "-crf", "18",
            "-pix_fmt", "yuv420p",
            "-movflags", "+faststart",
            output_path
        ]

        proc = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        if proc.returncode != 0:
            print(f"FFmpeg 合成失敗: {proc.stderr}")
            return False

        print(f"示範魚眼影片生成成功: {output_path}")
        return True
    finally:
        # 清理暫存幀圖
        for f in os.listdir(temp_dir):
            try:
                os.remove(os.path.join(temp_dir, f))
            except Exception:
                pass
        try:
            os.rmdir(temp_dir)
        except Exception:
            pass


if __name__ == "__main__":
    generate_demo_quad_video()
