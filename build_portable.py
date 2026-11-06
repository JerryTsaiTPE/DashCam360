"""
build_portable.py
使用 Python 調用 PyInstaller 進行可攜式打包，
自動將 native_app.py、core 模組以及系統中的 ffmpeg.exe、ffprobe.exe 一併打包至發行目錄，
生成真正免安裝、隨身碟隨插即用的綠色版 DashCam360。
"""

import os
import sys
import shutil
import subprocess

if sys.platform == "win32":
    try:
        sys.stdout.reconfigure(encoding="utf-8")
        sys.stderr.reconfigure(encoding="utf-8")
    except Exception:
        pass


def find_binary(name):
    """在系統中尋找指定執行檔路徑"""
    cmd = ["where", name] if sys.platform == "win32" else ["which", name]
    try:
        out = subprocess.check_output(cmd, text=True).strip().splitlines()
        if out:
            return out[0]
    except Exception:
        pass

    # Windows WinGet 常見預設路徑檢查
    winget_base = os.path.expanduser(r"~\AppData\Local\Microsoft\WinGet\Packages")
    if os.path.exists(winget_base):
        for root, dirs, files in os.walk(winget_base):
            if f"{name}.exe" in files:
                return os.path.join(root, f"{name}.exe")
    return None


def build():
    print("=" * 60)
    print("開始建置 DashCam360 Native 免安裝可攜式 App (Portable)...")
    print("=" * 60)

    project_root = os.path.dirname(os.path.abspath(__file__))
    os.chdir(project_root)

    dist_dir = os.path.join(project_root, "dist")
    build_dir = os.path.join(project_root, "build")
    output_app_dir = os.path.join(dist_dir, "DashCam360_Portable")

    pyinstaller_cmd = [
        sys.executable,
        "-m", "PyInstaller",
        "--noconfirm",
        "--onedir",
        "--windowed",
        "--name", "DashCam360_Portable",
        "--add-data", f"core{os.pathsep}core",
        os.path.join(project_root, "native_app.py")
    ]

    print("\n[步驟 1/3] 執行 PyInstaller 打包 Python 核心與 Qt6/OpenGL...")
    ret = subprocess.run(pyinstaller_cmd)
    if ret.returncode != 0:
        print("[錯誤] PyInstaller 打包失敗！請檢查相依庫。")
        return False

    print("\n[步驟 2/3] 正在隨附複製 FFmpeg 與 FFprobe 執行檔...")
    ffmpeg_path = find_binary("ffmpeg")
    ffprobe_path = find_binary("ffprobe")

    if ffmpeg_path and os.path.exists(ffmpeg_path):
        target_ffmpeg = os.path.join(output_app_dir, "ffmpeg.exe")
        shutil.copy2(ffmpeg_path, target_ffmpeg)
        print(f"  已複製: {ffmpeg_path} -> {target_ffmpeg}")
    else:
        print("  [警告] 找不到 ffmpeg.exe，可攜版將依賴目標電腦的 PATH 環境變數。")

    if ffprobe_path and os.path.exists(ffprobe_path):
        target_ffprobe = os.path.join(output_app_dir, "ffprobe.exe")
        shutil.copy2(ffprobe_path, target_ffprobe)
        print(f"  已複製: {ffprobe_path} -> {target_ffprobe}")

    # 複製 sample_videos 目錄 (若有)
    sample_src = os.path.join(project_root, "sample_videos")
    if os.path.exists(sample_src):
        sample_dst = os.path.join(output_app_dir, "sample_videos")
        shutil.copytree(sample_src, sample_dst, dirs_exist_ok=True)

    print("\n[步驟 3/3] 建立可攜式啟動捷徑與說明文件...")
    readme_txt = os.path.join(output_app_dir, "README_PORTABLE.txt")
    with open(readme_txt, "w", encoding="utf-8") as f:
        f.write(
            "DashCam360 Native 免安裝可攜版 (Portable)\n"
            "=========================================\n"
            "本資料夾為完全獨立的綠色免安裝應用程式，包含：\n"
            "  - DashCam360_Portable.exe (純原生 3D 環景主程式)\n"
            "  - 內建 FFmpeg 與 FFprobe 影片魚眼修正拉直引擎\n\n"
            "使用方式：\n"
            "直接雙擊『DashCam360_Portable.exe』即可執行，無需安裝任何環境！\n"
            "可將整個資料夾複製到隨身碟，在任意 Windows 10/11 電腦隨插即用。\n"
        )

    print("\n" + "=" * 60)
    print("【打包成功】可攜式免安裝版已生成於：")
    print(f"  -> {output_app_dir}")
    print("您可直接將該資料夾複製到隨身碟，在任何 Windows 電腦點擊執行！")
    print("=" * 60 + "\n")
    return True


if __name__ == "__main__":
    build()
