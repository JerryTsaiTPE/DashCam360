/**
 * player.js
 * 四路校正影片高精準同步播放控制器
 */

class QuadVideoPlayer {
  constructor() {
    this.videos = {
      front: document.getElementById('vid-front'),
      rear: document.getElementById('vid-rear'),
      left: document.getElementById('vid-left'),
      right: document.getElementById('vid-right')
    };

    this.isPlaying = false;
    this.playbackRate = 1.0;
    this.isSeeking = false;
    this.duration = 0;

    // UI 元件
    this.btnPlay = document.getElementById('btn-play');
    this.timeline = document.getElementById('timeline-slider');
    this.timeDisplay = document.getElementById('time-display');
    this.speedSelect = document.getElementById('speed-select');

    this.initEvents();
  }

  initEvents() {
    // 播放/暫停按鈕
    this.btnPlay.addEventListener('click', () => this.togglePlay());

    // 時間滑桿控制
    this.timeline.addEventListener('input', (e) => {
      this.isSeeking = true;
      const targetSec = (parseFloat(e.target.value) / 100) * this.duration;
      this.timeDisplay.innerText = `${this.formatTime(targetSec)} / ${this.formatTime(this.duration)}`;
    });

    this.timeline.addEventListener('change', (e) => {
      const targetSec = (parseFloat(e.target.value) / 100) * this.duration;
      this.seek(targetSec);
      this.isSeeking = false;
    });

    // 倍速調整
    this.speedSelect.addEventListener('change', (e) => {
      this.setPlaybackRate(parseFloat(e.target.value));
    });

    // 主視頻元數據載入
    this.videos.front.addEventListener('loadedmetadata', () => {
      this.duration = this.videos.front.duration || 5.0;
      this.updateTimeDisplay(0, this.duration);
    });

    // 主視頻時間更新
    this.videos.front.addEventListener('timeupdate', () => {
      if (!this.isSeeking && this.duration > 0) {
        const curr = this.videos.front.currentTime;
        this.timeline.value = (curr / this.duration) * 100;
        this.updateTimeDisplay(curr, this.duration);

        // 微幅同步校準 (防漂移)
        this.syncClocks(curr);
      }
    });

    // 鍵盤空白鍵快速播放暫停
    window.addEventListener('keydown', (e) => {
      if (e.code === 'Space' && e.target.tagName !== 'INPUT' && e.target.tagName !== 'SELECT') {
        e.preventDefault();
        this.togglePlay();
      }
    });
  }

  loadVideos(sources) {
    // sources: { front: url, rear: url, left: url, right: url }
    let loadedCount = 0;
    const roles = ['front', 'rear', 'left', 'right'];

    roles.forEach(role => {
      const vid = this.videos[role];
      if (sources[role]) {
        vid.src = sources[role];
        vid.load();
        vid.loop = true;
        vid.muted = true; // 靜音以支援無障礙自動播放
        vid.playsInline = true;

        vid.onloadeddata = () => {
          loadedCount++;
          if (loadedCount === roles.length) {
            console.log('四路影片全部載入完畢！');
            this.play();
          }
        };
      }
    });
  }

  play() {
    const promises = Object.values(this.videos).map(v => v.play().catch(() => {}));
    Promise.all(promises).then(() => {
      this.isPlaying = true;
      this.btnPlay.innerHTML = '<span>⏸</span> 暫停';
    });
  }

  pause() {
    Object.values(this.videos).forEach(v => v.pause());
    this.isPlaying = false;
    this.btnPlay.innerHTML = '<span>▶</span> 播放';
  }

  togglePlay() {
    if (this.isPlaying) {
      this.pause();
    } else {
      this.play();
    }
  }

  seek(seconds) {
    Object.values(this.videos).forEach(v => {
      v.currentTime = seconds;
    });
  }

  setPlaybackRate(rate) {
    this.playbackRate = rate;
    Object.values(this.videos).forEach(v => {
      v.playbackRate = rate;
    });
  }

  syncClocks(masterTime) {
    // 檢查其他三路是否偏差超過 0.15 秒，若是則平滑同步
    ['rear', 'left', 'right'].forEach(role => {
      const v = this.videos[role];
      if (v && Math.abs(v.currentTime - masterTime) > 0.15) {
        v.currentTime = masterTime;
      }
    });
  }

  formatTime(sec) {
    const m = Math.floor(sec / 60);
    const s = Math.floor(sec % 60);
    return `${m.toString().padStart(2, '0')}:${s.toString().padStart(2, '0')}`;
  }

  updateTimeDisplay(curr, total) {
    this.timeDisplay.innerText = `${this.formatTime(curr)} / ${this.formatTime(total)}`;
  }
}

window.QuadVideoPlayer = QuadVideoPlayer;
