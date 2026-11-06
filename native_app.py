"""
native_app.py
DashCam360 - 純原生 3D 汽車環景魚眼校正案頭應用程式
【完全不使用網頁 / Webview / 瀏覽器】
採用 Qt6 (PySide6) + 純原生 OpenGL 3D 硬體加速渲染與原生物件控制項。
可直接打包成獨立免安裝 Portable 應用程式。
"""

import os
import sys
import math
import json
import threading
from typing import Dict, Optional

# Windows UTF-8 輸出
if sys.platform == "win32":
    try:
        sys.stdout.reconfigure(encoding="utf-8")
        sys.stderr.reconfigure(encoding="utf-8")
    except Exception:
        pass

from PySide6.QtCore import Qt, QUrl, QTimer, Signal, Slot, QPoint, QSize
from PySide6.QtGui import (
    QMatrix4x4, QVector3D, QColor, QFont, QIcon, QPainter, QImage,
    QOpenGLFunctions
)
from PySide6.QtWidgets import (
    QApplication, QMainWindow, QWidget, QVBoxLayout, QHBoxLayout,
    QPushButton, QLabel, QSlider, QComboBox, QFileDialog, QProgressBar,
    QGroupBox, QGridLayout, QFrame, QSplitter
)
from PySide6.QtOpenGLWidgets import QOpenGLWidget
from PySide6.QtOpenGL import QOpenGLTexture
from PySide6.QtMultimedia import QMediaPlayer, QAudioOutput, QVideoSink, QVideoFrame

from OpenGL.GL import *

from core.ffmpeg_processor import FFmpegProcessor
from core.demo_generator import generate_demo_quad_video


# ==========================================
# 1. 純原生 3D OpenGL 環景渲染視窗
# ==========================================
class GLSurroundWidget(QOpenGLWidget, QOpenGLFunctions):
    def __init__(self, parent=None):
        super().__init__(parent)
        # 相機控制參數
        self.camera_distance = 6.8
        self.camera_pitch = 30.0   # 俯仰角度
        self.camera_yaw = 45.0     # 360 度水平環視角度
        self.target_lookat = QVector3D(0.0, 0.6, 0.0)

        self.last_mouse_pos = QPoint()

        # 4 路動態紋理 ID (Front, Rear, Left, Right)
        self.textures = {
            "front": 0,
            "rear": 0,
            "left": 0,
            "right": 0
        }
        self.frame_images = {
            "front": None,
            "rear": None,
            "left": None,
            "right": None
        }

        # 屏幕環繞距離與尺寸
        self.screen_dist = 3.6
        self.screen_w = 4.0
        self.screen_h = 2.4

        self.setFocusPolicy(Qt.StrongFocus)

    def initializeGL(self):
        self.initializeOpenGLFunctions()
        glEnable(GL_DEPTH_TEST)
        glEnable(GL_CULL_FACE)
        glCullFace(GL_BACK)
        glEnable(GL_BLEND)
        glBlendFunc(GL_SRC_ALPHA, GL_ONE_MINUS_SRC_ALPHA)
        glClearColor(0.06, 0.08, 0.11, 1.0)

        # 初始化 4 個 OpenGL 紋理
        for role in self.textures:
            tex_id = glGenTextures(1)
            glBindTexture(GL_TEXTURE_2D, tex_id)
            glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_LINEAR)
            glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_LINEAR)
            glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE)
            glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE)
            
            # 建立預設底圖 (標示方位)
            img = self.create_placeholder_image(role.upper() + " CAM")
            glTexImage2D(GL_TEXTURE_2D, 0, GL_RGBA, img.width(), img.height(), 0, GL_RGBA, GL_UNSIGNED_BYTE, img.bits())
            self.textures[role] = tex_id

    def create_placeholder_image(self, text):
        img = QImage(512, 256, QImage.Format_RGBA8888)
        img.fill(QColor(25, 35, 48))
        painter = QPainter(img)
        painter.setPen(QColor(0, 210, 255))
        font = QFont("Arial", 28, QFont.Bold)
        painter.setFont(font)
        painter.drawText(img.rect(), Qt.AlignCenter, text)
        painter.end()
        return img

    def update_frame(self, role: str, image: QImage):
        """接收解碼出的影格並排入紋理更新"""
        self.frame_images[role] = image
        self.update()

    def resizeGL(self, w, h):
        glViewport(0, 0, w, h)

    def paintGL(self):
        glClear(GL_COLOR_BUFFER_BIT | GL_DEPTH_BUFFER_BIT)

        # 上傳有更新的視訊影格至 GPU 紋理
        for role, img in self.frame_images.items():
            if img is not None:
                # 轉為 OpenGL 相容的 RGBA8888 格式
                rgba = img.convertToFormat(QImage.Format_RGBA8888)
                glBindTexture(GL_TEXTURE_2D, self.textures[role])
                glTexImage2D(
                    GL_TEXTURE_2D, 0, GL_RGBA,
                    rgba.width(), rgba.height(), 0,
                    GL_RGBA, GL_UNSIGNED_BYTE, rgba.bits()
                )
                self.frame_images[role] = None

        # 計算相機矩陣 (Orbit View)
        view = QMatrix4x4()
        rad_pitch = math.radians(self.camera_pitch)
        rad_yaw = math.radians(self.camera_yaw)

        cam_x = self.target_lookat.x() + self.camera_distance * math.cos(rad_pitch) * math.sin(rad_yaw)
        cam_y = self.target_lookat.y() + self.camera_distance * math.sin(rad_pitch)
        cam_z = self.target_lookat.z() + self.camera_distance * math.cos(rad_pitch) * math.cos(rad_yaw)

        view.lookAt(QVector3D(cam_x, cam_y, cam_z), self.target_lookat, QVector3D(0, 1, 0))

        proj = QMatrix4x4()
        aspect = self.width() / max(self.height(), 1)
        proj.perspective(48.0, aspect, 0.2, 80.0)

        # 1. 繪製地面網格與雷達同心測距圓環
        self.draw_ground_and_radar(proj, view)

        # 2. 繪製 3D 車身實體模型
        self.draw_car_model(proj, view)

        # 3. 繪製 4 面動態材質屏幕 (前、後、左、右)
        self.draw_screens(proj, view)

    def draw_ground_and_radar(self, proj, view):
        glMatrixMode(GL_PROJECTION)
        glLoadMatrixf(proj.toGenericMatrix(4, 4).data())
        glMatrixMode(GL_MODELVIEW)
        glLoadMatrixf(view.toGenericMatrix(4, 4).data())

        glDisable(GL_TEXTURE_2D)

        # 地面灰色網格
        glLineWidth(1.0)
        glColor4f(0.12, 0.18, 0.25, 0.5)
        glBegin(GL_LINES)
        grid_size = 12
        step = 1.0
        for i in range(-grid_size, grid_size + 1):
            glVertex3f(i * step, 0.0, -grid_size * step)
            glVertex3f(i * step, 0.0, grid_size * step)
            glVertex3f(-grid_size * step, 0.0, i * step)
            glVertex3f(grid_size * step, 0.0, i * step)
        glEnd()

        # 雷達同心距離圓環 (1.5m, 2.5m, 3.5m, 4.5m)
        radii = [1.5, 2.5, 3.5, 4.5]
        colors = [(1.0, 0.2, 0.3), (1.0, 0.6, 0.0), (0.0, 1.0, 0.6), (0.0, 0.8, 1.0)]
        glLineWidth(2.0)

        for r, col in zip(radii, colors):
            glColor4f(col[0], col[1], col[2], 0.7)
            glBegin(GL_LINE_LOOP)
            for deg in range(0, 360, 5):
                rad = math.radians(deg)
                glVertex3f(r * math.cos(rad), 0.02, r * math.sin(rad))
            glEnd()

    def draw_car_model(self, proj, view):
        glMatrixMode(GL_PROJECTION)
        glLoadMatrixf(proj.toGenericMatrix(4, 4).data())
        glMatrixMode(GL_MODELVIEW)
        glLoadMatrixf(view.toGenericMatrix(4, 4).data())

        # 車輛本體尺寸 (長 4.0, 寬 1.8, 高 0.9)
        # 車身半透明藍色金屬
        glEnable(GL_BLEND)
        glColor4f(0.12, 0.28, 0.45, 0.85)
        self.draw_box(0, 0.65, 0, 1.8, 0.75, 4.0)

        # 車頂艙
        glColor4f(0.08, 0.18, 0.30, 0.75)
        self.draw_box(0, 1.3, -0.2, 1.5, 0.55, 2.2)

        # 車身線框輪廓
        glLineWidth(1.5)
        glColor4f(0.0, 0.85, 1.0, 0.9)
        self.draw_box_wireframe(0, 0.65, 0, 1.8, 0.75, 4.0)
        self.draw_box_wireframe(0, 1.3, -0.2, 1.5, 0.55, 2.2)

        # 車頭方向箭頭 (朝向 -Z)
        glColor4f(0.0, 1.0, 0.8, 1.0)
        glBegin(GL_TRIANGLES)
        glVertex3f(0.0, 1.6, -1.0)
        glVertex3f(-0.35, 1.6, -0.3)
        glVertex3f(0.35, 1.6, -0.3)
        glEnd()

        # 車輪 (4 個深色輪胎)
        glColor4f(0.1, 0.1, 0.12, 1.0)
        wheels = [(-0.95, 0.35, -1.3), (0.95, 0.35, -1.3), (-0.95, 0.35, 1.3), (0.95, 0.35, 1.3)]
        for wx, wy, wz in wheels:
            self.draw_box(wx, wy, wz, 0.25, 0.7, 0.7)

    def draw_box(self, x, y, z, sx, sy, sz):
        hx, hy, hz = sx / 2, sy / 2, sz / 2
        glBegin(GL_QUADS)
        # 前
        glVertex3f(x - hx, y - hy, z - hz); glVertex3f(x + hx, y - hy, z - hz)
        glVertex3f(x + hx, y + hy, z - hz); glVertex3f(x - hx, y + hy, z - hz)
        # 後
        glVertex3f(x - hx, y - hy, z + hz); glVertex3f(x - hx, y + hy, z + hz)
        glVertex3f(x + hx, y + hy, z + hz); glVertex3f(x + hx, y - hy, z + hz)
        # 上
        glVertex3f(x - hx, y + hy, z - hz); glVertex3f(x + hx, y + hy, z - hz)
        glVertex3f(x + hx, y + hy, z + hz); glVertex3f(x - hx, y + hy, z + hz)
        # 下
        glVertex3f(x - hx, y - hy, z - hz); glVertex3f(x - hx, y - hy, z + hz)
        glVertex3f(x + hx, y - hy, z + hz); glVertex3f(x + hx, y - hy, z - hz)
        # 左
        glVertex3f(x - hx, y - hy, z + hz); glVertex3f(x - hx, y - hy, z - hz)
        glVertex3f(x - hx, y + hy, z - hz); glVertex3f(x - hx, y + hy, z + hz)
        # 右
        glVertex3f(x + hx, y - hy, z - hz); glVertex3f(x + hx, y - hy, z + hz)
        glVertex3f(x + hx, y + hy, z + hz); glVertex3f(x + hx, y + hy, z - hz)
        glEnd()

    def draw_box_wireframe(self, x, y, z, sx, sy, sz):
        hx, hy, hz = sx / 2, sy / 2, sz / 2
        glBegin(GL_LINE_LOOP)
        glVertex3f(x - hx, y - hy, z - hz); glVertex3f(x + hx, y - hy, z - hz)
        glVertex3f(x + hx, y + hy, z - hz); glVertex3f(x - hx, y + hy, z - hz)
        glEnd()
        glBegin(GL_LINE_LOOP)
        glVertex3f(x - hx, y - hy, z + hz); glVertex3f(x + hx, y - hy, z + hz)
        glVertex3f(x + hx, y + hy, z + hz); glVertex3f(x - hx, y + hy, z + hz)
        glEnd()
        glBegin(GL_LINES)
        glVertex3f(x - hx, y - hy, z - hz); glVertex3f(x - hx, y - hy, z + hz)
        glVertex3f(x + hx, y - hy, z - hz); glVertex3f(x + hx, y - hy, z + hz)
        glVertex3f(x + hx, y + hy, z - hz); glVertex3f(x + hx, y + hy, z + hz)
        glVertex3f(x - hx, y + hy, z - hz); glVertex3f(x - hx, y + hy, z + hz)
        glEnd()

    def draw_screens(self, proj, view):
        """繪製四面立體屏幕並貼附對應動態視訊紋理"""
        d = self.screen_dist
        w = self.screen_w
        h = self.screen_h
        base_y = 1.6

        glEnable(GL_TEXTURE_2D)
        glDisable(GL_CULL_FACE) # 雙面可見
        glColor4f(1.0, 1.0, 1.0, 1.0)

        # 1. 前屏幕 (Front: Z = -d)
        glBindTexture(GL_TEXTURE_2D, self.textures["front"])
        glBegin(GL_QUADS)
        glTexCoord2f(0.0, 1.0); glVertex3f(-w / 2, base_y - h / 2, -d)
        glTexCoord2f(1.0, 1.0); glVertex3f(w / 2, base_y - h / 2, -d)
        glTexCoord2f(1.0, 0.0); glVertex3f(w / 2, base_y + h / 2, -d)
        glTexCoord2f(0.0, 0.0); glVertex3f(-w / 2, base_y + h / 2, -d)
        glEnd()

        # 2. 後屏幕 (Rear: Z = +d, 轉身面向車尾)
        glBindTexture(GL_TEXTURE_2D, self.textures["rear"])
        glBegin(GL_QUADS)
        glTexCoord2f(0.0, 1.0); glVertex3f(w / 2, base_y - h / 2, d)
        glTexCoord2f(1.0, 1.0); glVertex3f(-w / 2, base_y - h / 2, d)
        glTexCoord2f(1.0, 0.0); glVertex3f(-w / 2, base_y + h / 2, d)
        glTexCoord2f(0.0, 0.0); glVertex3f(w / 2, base_y + h / 2, d)
        glEnd()

        # 3. 左屏幕 (Left: X = -d, 面向車身左側)
        glBindTexture(GL_TEXTURE_2D, self.textures["left"])
        glBegin(GL_QUADS)
        glTexCoord2f(0.0, 1.0); glVertex3f(-d, base_y - h / 2, d / 1.1)
        glTexCoord2f(1.0, 1.0); glVertex3f(-d, base_y - h / 2, -d / 1.1)
        glTexCoord2f(1.0, 0.0); glVertex3f(-d, base_y + h / 2, -d / 1.1)
        glTexCoord2f(0.0, 0.0); glVertex3f(-d, base_y + h / 2, d / 1.1)
        glEnd()

        # 4. 右屏幕 (Right: X = +d, 面向車身右側)
        glBindTexture(GL_TEXTURE_2D, self.textures["right"])
        glBegin(GL_QUADS)
        glTexCoord2f(0.0, 1.0); glVertex3f(d, base_y - h / 2, -d / 1.1)
        glTexCoord2f(1.0, 1.0); glVertex3f(d, base_y - h / 2, d / 1.1)
        glTexCoord2f(1.0, 0.0); glVertex3f(d, base_y + h / 2, d / 1.1)
        glTexCoord2f(0.0, 0.0); glVertex3f(d, base_y + h / 2, -d / 1.1)
        glEnd()

        glDisable(GL_TEXTURE_2D)
        glEnable(GL_CULL_FACE)

        # 繪製屏幕發光藍色邊框
        glLineWidth(2.0)
        glColor4f(0.0, 0.85, 1.0, 0.65)
        # 前框
        glBegin(GL_LINE_LOOP)
        glVertex3f(-w / 2, base_y - h / 2, -d); glVertex3f(w / 2, base_y - h / 2, -d)
        glVertex3f(w / 2, base_y + h / 2, -d); glVertex3f(-w / 2, base_y + h / 2, -d)
        glEnd()
        # 後框
        glBegin(GL_LINE_LOOP)
        glVertex3f(-w / 2, base_y - h / 2, d); glVertex3f(w / 2, base_y - h / 2, d)
        glVertex3f(w / 2, base_y + h / 2, d); glVertex3f(-w / 2, base_y + h / 2, d)
        glEnd()
        # 左框
        glBegin(GL_LINE_LOOP)
        glVertex3f(-d, base_y - h / 2, d / 1.1); glVertex3f(-d, base_y - h / 2, -d / 1.1)
        glVertex3f(-d, base_y + h / 2, -d / 1.1); glVertex3f(-d, base_y + h / 2, d / 1.1)
        glEnd()
        # 右框
        glBegin(GL_LINE_LOOP)
        glVertex3f(d, base_y - h / 2, -d / 1.1); glVertex3f(d, base_y - h / 2, d / 1.1)
        glVertex3f(d, base_y + h / 2, d / 1.1); glVertex3f(d, base_y + h / 2, -d / 1.1)
        glEnd()

    # 原生滑鼠互動 (Orbit 旋轉、縮放、平移)
    def mousePressEvent(self, e):
        self.last_mouse_pos = e.pos()

    def mouseMoveEvent(self, e):
        dx = e.position().x() - self.last_mouse_pos.x()
        dy = e.position().y() - self.last_mouse_pos.y()

        if e.buttons() & Qt.LeftButton:
            # 左鍵 360 度旋轉環視
            self.camera_yaw += dx * 0.45
            self.camera_pitch = max(5.0, min(88.0, self.camera_pitch + dy * 0.45))
            self.update()

        elif e.buttons() & Qt.RightButton:
            # 右鍵平移目標
            rad_yaw = math.radians(self.camera_yaw)
            self.target_lookat.setX(self.target_lookat.x() - dx * 0.01 * math.cos(rad_yaw))
            self.target_lookat.setZ(self.target_lookat.z() + dx * 0.01 * math.sin(rad_yaw))
            self.target_lookat.setY(self.target_lookat.y() + dy * 0.01)
            self.update()

        self.last_mouse_pos = e.pos()

    def wheelEvent(self, e):
        # 滾輪平滑縮放
        delta = e.angleDelta().y() / 120.0
        self.camera_distance = max(1.8, min(24.0, self.camera_distance - delta * 0.5))
        self.update()

    def set_view_preset(self, preset: str):
        """快捷視角切換"""
        if preset == "birdEye":
            self.camera_pitch = 85.0
            self.camera_yaw = 0.0
            self.camera_distance = 11.0
            self.target_lookat = QVector3D(0, 0, 0)
        elif preset == "front":
            self.camera_pitch = 15.0
            self.camera_yaw = 180.0
            self.camera_distance = 4.5
            self.target_lookat = QVector3D(0, 1.2, -3.0)
        elif preset == "rear":
            self.camera_pitch = 15.0
            self.camera_yaw = 0.0
            self.camera_distance = 4.5
            self.target_lookat = QVector3D(0, 1.2, 3.0)
        elif preset == "left":
            self.camera_pitch = 15.0
            self.camera_yaw = 90.0
            self.camera_distance = 4.5
            self.target_lookat = QVector3D(-3.0, 1.2, 0)
        elif preset == "right":
            self.camera_pitch = 15.0
            self.camera_yaw = 270.0
            self.camera_distance = 4.5
            self.target_lookat = QVector3D(3.0, 1.2, 0)
        else: # orbit
            self.camera_pitch = 30.0
            self.camera_yaw = 45.0
            self.camera_distance = 6.8
            self.target_lookat = QVector3D(0, 0.6, 0)
        self.update()


# ==========================================
# 2. 四路原生視頻同步控制器 (QMediaPlayer)
# ==========================================
class QuadPlayerManager(QWidget):
    progress_changed = Signal(int, int) # current_ms, total_ms

    def __init__(self, gl_widget: GLSurroundWidget, parent=None):
        super().__init__(parent)
        self.gl_widget = gl_widget
        self.roles = ["front", "rear", "left", "right"]
        self.players = {}
        self.sinks = {}
        self.is_playing = False
        self.duration = 0

        for role in self.roles:
            player = QMediaPlayer(self)
            audio = QAudioOutput(self)
            audio.setMuted(True) # 靜音
            player.setAudioOutput(audio)

            sink = QVideoSink(self)
            player.setVideoSink(sink)

            # 連接解碼後影格回調，直接轉送給 OpenGL 紋理
            def make_handler(r):
                return lambda frame: self.on_frame_received(r, frame)

            sink.videoFrameChanged.connect(make_handler(role))

            self.players[role] = player
            self.sinks[role] = sink

        # 主視訊時鐘更新
        self.players["front"].positionChanged.connect(self.on_position_changed)
        self.players["front"].durationChanged.connect(self.on_duration_changed)

    def on_frame_received(self, role: str, frame: QVideoFrame):
        if frame.isValid():
            img = frame.toImage()
            self.gl_widget.update_frame(role, img)

    def on_position_changed(self, pos_ms):
        self.progress_changed.emit(pos_ms, self.duration)
        # 輔助同步防漂移
        for r in ["rear", "left", "right"]:
            p = self.players[r]
            if abs(p.position() - pos_ms) > 200:
                p.setPosition(pos_ms)

    def on_duration_changed(self, dur_ms):
        self.duration = dur_ms

    def load_videos(self, video_dict: Dict[str, str]):
        for role, path in video_dict.items():
            if role in self.players and os.path.exists(path):
                url = QUrl.fromLocalFile(os.path.abspath(path))
                self.players[role].setSource(url)
                self.players[role].setLoops(QMediaPlayer.Infinite)

    def play(self):
        for p in self.players.values():
            p.play()
        self.is_playing = True

    def pause(self):
        for p in self.players.values():
            p.pause()
        self.is_playing = False

    def toggle(self):
        if self.is_playing:
            self.pause()
        else:
            self.play()

    def seek(self, pos_ms):
        for p in self.players.values():
            p.setPosition(pos_ms)


# ==========================================
# 3. 原生主視窗 (Native Desktop Window)
# ==========================================
class NativeAppWindow(QMainWindow):
    def __init__(self):
        super().__init__()
        self.setWindowTitle("DashCam360 Native - 汽車環景魚眼校正與 3D 動態投影 (可攜原生版)")
        self.resize(1380, 860)

        self.processor = FFmpegProcessor()
        self.current_video_file = None

        self.init_ui()
        self.apply_dark_style()

    def init_ui(self):
        main_widget = QWidget()
        main_layout = QHBoxLayout(main_widget)
        main_layout.setContentsMargins(10, 10, 10, 10)
        main_layout.setSpacing(10)

        # 左側：3D OpenGL 畫布與底部播放列
        left_layout = QVBoxLayout()

        # 頂部快捷視角按鈕列
        preset_bar = QHBoxLayout()
        preset_buttons = [
            ("🔄 自由環視", "orbit"),
            ("🦅 鳥瞰俯視", "birdEye"),
            ("⬆️ 車前視角", "front"),
            ("⬇️ 車後視角", "rear"),
            ("⬅️ 車左視角", "left"),
            ("➡️ 車右視角", "right")
        ]
        for label, p_id in preset_buttons:
            btn = QPushButton(label)
            btn.clicked.connect(lambda _, pid=p_id: self.gl_widget.set_view_preset(pid))
            preset_bar.addWidget(btn)

        left_layout.addLayout(preset_bar)

        # 3D OpenGL Widget
        self.gl_widget = GLSurroundWidget(self)
        left_layout.addWidget(self.gl_widget, stretch=1)

        # 播放控制器管理器
        self.player_mgr = QuadPlayerManager(self.gl_widget, self)
        self.player_mgr.progress_changed.connect(self.update_player_ui)

        # 底部播放控制列
        player_bar = QHBoxLayout()
        self.btn_play = QPushButton("▶ 播放")
        self.btn_play.clicked.connect(self.on_btn_play_clicked)
        player_bar.addWidget(self.btn_play)

        self.lbl_time = QLabel("00:00 / 00:00")
        player_bar.addWidget(self.lbl_time)

        self.slider_time = QSlider(Qt.Horizontal)
        self.slider_time.setRange(0, 1000)
        self.slider_time.sliderMoved.connect(self.on_slider_seek)
        player_bar.addWidget(self.slider_time, stretch=1)

        left_layout.addLayout(player_bar)

        # 右側控制面板 (檔案匯入、象限設定、v360 參數、進度條)
        right_panel = QFrame()
        right_panel.setFixedWidth(360)
        panel_layout = QVBoxLayout(right_panel)
        panel_layout.setSpacing(14)

        # 卡片 1: 檔案匯入
        grp_import = QGroupBox("📁 匯入 4 分割魚眼影片")
        lay_import = QVBoxLayout(grp_import)

        self.lbl_file = QLabel("未載入影片 (支援 MP4/MKV/AVI)")
        self.lbl_file.setWordWrap(True)
        lay_import.addWidget(self.lbl_file)

        btn_row = QHBoxLayout()
        self.btn_open = QPushButton("選取本地檔案")
        self.btn_open.clicked.connect(self.choose_file)
        btn_row.addWidget(self.btn_open)

        self.btn_demo = QPushButton("🎲 產生測試樣品")
        self.btn_demo.clicked.connect(self.generate_demo)
        btn_row.addWidget(self.btn_demo)

        lay_import.addLayout(btn_row)
        panel_layout.addWidget(grp_import)

        # 卡片 2: 象限指派
        grp_quad = QGroupBox("🧭 象限方位映射指派")
        lay_quad = QGridLayout(grp_quad)

        self.sel_tl = QComboBox(); self.sel_tl.addItems(["前視 (Front)", "後視 (Rear)", "左側 (Left)", "右側 (Right)"])
        self.sel_tr = QComboBox(); self.sel_tr.addItems(["前視 (Front)", "後視 (Rear)", "左側 (Left)", "右側 (Right)"]); self.sel_tr.setCurrentIndex(1)
        self.sel_bl = QComboBox(); self.sel_bl.addItems(["前視 (Front)", "後視 (Rear)", "左側 (Left)", "右側 (Right)"]); self.sel_bl.setCurrentIndex(2)
        self.sel_br = QComboBox(); self.sel_br.addItems(["前視 (Front)", "後視 (Rear)", "左側 (Left)", "右側 (Right)"]); self.sel_br.setCurrentIndex(3)

        lay_quad.addWidget(QLabel("↖ 左上:"), 0, 0); lay_quad.addWidget(self.sel_tl, 0, 1)
        lay_quad.addWidget(QLabel("↗ 右上:"), 0, 2); lay_quad.addWidget(self.sel_tr, 0, 3)
        lay_quad.addWidget(QLabel("↙ 左下:"), 1, 0); lay_quad.addWidget(self.sel_bl, 1, 1)
        lay_quad.addWidget(QLabel("↘ 右下:"), 1, 2); lay_quad.addWidget(self.sel_br, 1, 3)

        panel_layout.addWidget(grp_quad)

        # 卡片 3: FFmpeg v360 校正參數
        grp_param = QGroupBox("📐 FFmpeg v360 魚眼拉直參數")
        lay_param = QVBoxLayout(grp_param)

        self.slider_infov = QSlider(Qt.Horizontal); self.slider_infov.setRange(140, 220); self.slider_infov.setValue(180)
        self.lbl_infov = QLabel("輸入魚眼 FOV: 180°")
        self.slider_infov.valueChanged.connect(lambda v: self.lbl_infov.setText(f"輸入魚眼 FOV: {v}°"))
        lay_param.addWidget(self.lbl_infov); lay_param.addWidget(self.slider_infov)

        self.slider_outfov = QSlider(Qt.Horizontal); self.slider_outfov.setRange(70, 140); self.slider_outfov.setValue(110)
        self.lbl_outfov = QLabel("拉直輸出水平 FOV: 110°")
        self.slider_outfov.valueChanged.connect(lambda v: self.lbl_outfov.setText(f"拉直輸出水平 FOV: {v}°"))
        lay_param.addWidget(self.lbl_outfov); lay_param.addWidget(self.slider_outfov)

        panel_layout.addWidget(grp_param)

        # 卡片 4: 執行與進度
        self.progress_bar = QProgressBar()
        self.progress_bar.setValue(0)
        self.lbl_progress = QLabel("就緒")
        panel_layout.addWidget(self.progress_bar)
        panel_layout.addWidget(self.lbl_progress)

        self.btn_start = QPushButton("🚀 一鍵自動裁切與 v360 魚眼校正")
        self.btn_start.setFixedHeight(44)
        self.btn_start.clicked.connect(self.start_processing)
        panel_layout.addWidget(self.btn_start)

        panel_layout.addStretch()

        # 組合主佈局
        main_layout.addLayout(left_layout, stretch=1)
        main_layout.addWidget(right_panel)

        self.setCentralWidget(main_widget)

    def apply_dark_style(self):
        """專業深色科技主題 QSS 樣式表"""
        self.setStyleSheet("""
            QWidget {
                background-color: #0f141d;
                color: #e4edf8;
                font-family: 'Segoe UI', 'Microsoft JhengHei', sans-serif;
                font-size: 13px;
            }
            QGroupBox {
                border: 1px solid rgba(0, 210, 255, 0.25);
                border-radius: 8px;
                margin-top: 10px;
                padding-top: 10px;
                font-weight: bold;
                color: #00d2ff;
            }
            QPushButton {
                background-color: #1a2536;
                border: 1px solid rgba(0, 210, 255, 0.3);
                border-radius: 6px;
                padding: 6px 12px;
                color: #ffffff;
            }
            QPushButton:hover {
                background-color: #0077cc;
                border-color: #00f0ff;
            }
            QPushButton:pressed {
                background-color: #005599;
            }
            QComboBox {
                background-color: #16202e;
                border: 1px solid rgba(0, 210, 255, 0.25);
                border-radius: 4px;
                padding: 4px;
            }
            QSlider::groove:horizontal {
                height: 6px;
                background: #1e2b3e;
                border-radius: 3px;
            }
            QSlider::sub-page:horizontal {
                background: #00d2ff;
                border-radius: 3px;
            }
            QSlider::handle:horizontal {
                width: 14px;
                background: #ffffff;
                margin-top: -4px;
                margin-bottom: -4px;
                border-radius: 7px;
            }
            QProgressBar {
                border: 1px solid #1e2b3e;
                border-radius: 4px;
                text-align: center;
                background: #16202e;
            }
            QProgressBar::chunk {
                background: qlineargradient(x1:0, y1:0, x2:1, y2:0, stop:0 #0077ff, stop:1 #00f0ff);
            }
        """)

    def choose_file(self):
        path, _ = QFileDialog.getOpenFileName(
            self, "選取 4 分割汽車魚眼影片", "", "視訊檔案 (*.mp4 *.mkv *.avi *.mov *.ts)"
        )
        if path:
            self.current_video_file = path
            self.lbl_file.setText(f"已選取: {os.path.basename(path)}")

    def generate_demo(self):
        self.lbl_progress.setText("正在生成 4 分割魚眼測試示範影片...")
        self.progress_bar.setValue(10)

        def run():
            sample_out = os.path.abspath("sample_videos/demo_quad_fisheye.mp4")
            ok = generate_demo_quad_video(sample_out, duration=5, fps=25)
            if ok:
                self.current_video_file = sample_out
                self.lbl_file.setText(f"已選取示範檔: {os.path.basename(sample_out)}")
                self.lbl_progress.setText("示範樣品建立完成！請點擊下方開始校正。")
                self.progress_bar.setValue(100)
            else:
                self.lbl_progress.setText("生成失敗")

        threading.Thread(target=run, daemon=True).start()

    def start_processing(self):
        if not self.current_video_file or not os.path.exists(self.current_video_file):
            self.lbl_progress.setText("請先匯入影片檔案或產生測試樣品！")
            return

        # 映射關係
        role_map = {
            "前視 (Front)": "front",
            "後視 (Rear)": "rear",
            "左側 (Left)": "left",
            "右側 (Right)": "right"
        }
        mapping = {
            "top_left": role_map[self.sel_tl.currentText()],
            "top_right": role_map[self.sel_tr.currentText()],
            "bottom_left": role_map[self.sel_bl.currentText()],
            "bottom_right": role_map[self.sel_br.currentText()]
        }

        in_fov = self.slider_infov.value()
        out_fov = self.slider_outfov.value()

        self.btn_start.setEnabled(False)
        self.progress_bar.setValue(2)
        self.lbl_progress.setText("啟動 FFmpeg v360 魚眼拉直並行運算...")

        def on_prog(pct, msg):
            QTimer.singleShot(0, lambda: self.progress_bar.setValue(int(pct)))
            QTimer.singleShot(0, lambda: self.lbl_progress.setText(msg))

        def run():
            try:
                results = self.processor.process_quad_video(
                    input_path=self.current_video_file,
                    output_dir=os.path.abspath("output"),
                    mapping=mapping,
                    in_fov=in_fov,
                    out_hfov=out_fov,
                    out_vfov=80,
                    progress_cb=on_prog
                )
                # 載入影片至原生播放器
                QTimer.singleShot(0, lambda: self.on_process_complete(results))
            except Exception as e:
                QTimer.singleShot(0, lambda: self.lbl_progress.setText(f"處理失敗: {e}"))
                QTimer.singleShot(0, lambda: self.btn_start.setEnabled(True))

        threading.Thread(target=run, daemon=True).start()

    def on_process_complete(self, results):
        self.btn_start.setEnabled(True)
        self.lbl_progress.setText("校正完成！4 路動態材質已投影至 3D 空間四周。")
        self.player_mgr.load_videos(results)
        self.player_mgr.play()
        self.btn_play.setText("⏸ 暫停")

    def on_btn_play_clicked(self):
        self.player_mgr.toggle()
        self.btn_play.setText("⏸ 暫停" if self.player_mgr.is_playing else "▶ 播放")

    def on_slider_seek(self, val):
        dur = self.player_mgr.duration
        if dur > 0:
            target_ms = int((val / 1000.0) * dur)
            self.player_mgr.seek(target_ms)

    def update_player_ui(self, curr_ms, total_ms):
        if total_ms > 0:
            pct = int((curr_ms / total_ms) * 1000)
            self.slider_time.setValue(pct)
            c_s = curr_ms // 1000
            t_s = total_ms // 1000
            self.lbl_time.setText(f"{c_s//60:02d}:{c_s%60:02d} / {t_s//60:02d}:{t_s%60:02d}")


def main():
    app = QApplication(sys.argv)
    window = NativeAppWindow()
    window.show()
    sys.exit(app.exec())


if __name__ == "__main__":
    main()
