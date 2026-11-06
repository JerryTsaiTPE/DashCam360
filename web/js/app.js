/**
 * app.js
 * 應用程式總控：整合 3D 視圖、播放器、介面控制與 Python 後端橋樑
 */

document.addEventListener('DOMContentLoaded', () => {
  // 1. 初始化 3D 視圖
  const viewer = new DashCamViewer3D('viewport-container');

  // 2. 初始化四路播放器
  const player = new QuadVideoPlayer();

  // 將 4 個 video 元素綁定至 3D 屏幕作為動態材質
  ['front', 'rear', 'left', 'right'].forEach(role => {
    viewer.setVideoTexture(role, player.videos[role]);
  });

  // 3. UI 元素
  const btnImport = document.getElementById('btn-import');
  const btnGenerateDemo = document.getElementById('btn-generate-demo');
  const btnProcess = document.getElementById('btn-process');
  const dropzone = document.getElementById('dropzone');
  const fileInput = document.getElementById('file-input');
  const fileBadge = document.getElementById('file-badge');

  // 進度面板
  const progressCard = document.getElementById('progress-card');
  const progressBar = document.getElementById('progress-fill');
  const progressStatus = document.getElementById('progress-status');

  // 參數元件
  const sliderInFov = document.getElementById('param-in-fov');
  const valInFov = document.getElementById('val-in-fov');
  const sliderOutFov = document.getElementById('param-out-fov');
  const valOutFov = document.getElementById('val-out-fov');
  const sliderScreenDist = document.getElementById('param-screen-dist');
  const valScreenDist = document.getElementById('val-screen-dist');

  // 象限映射選擇器
  const selTL = document.getElementById('sel-quad-tl');
  const selTR = document.getElementById('sel-quad-tr');
  const selBL = document.getElementById('sel-quad-bl');
  const selBR = document.getElementById('sel-quad-br');

  let currentVideoPath = null;
  let pyBridge = null;

  // 4. 連接 QtWebChannel (如果處於 PySide6 視窗環境)
  if (typeof QWebChannel !== 'undefined') {
    new QWebChannel(qt.webChannelTransport, (channel) => {
      pyBridge = channel.objects.pyBridge;
      console.log('QtWebChannel 已連線');

      // 監聽 Python 後端訊號
      if (pyBridge) {
        pyBridge.progressUpdated.connect((pct, msg) => {
          updateProgress(pct, msg);
        });

        pyBridge.processingComplete.connect((resultJson) => {
          try {
            const res = JSON.parse(resultJson);
            onProcessingDone(res);
          } catch (e) {
            console.error(e);
          }
        });

        pyBridge.fileSelected.connect((filePath) => {
          if (filePath) {
            setLoadedFile(filePath);
          }
        });
      }
    });
  }

  // 檔案選取事件
  btnImport.addEventListener('click', () => {
    if (pyBridge) {
      pyBridge.openFileDialog();
    } else {
      fileInput.click();
    }
  });

  fileInput.addEventListener('change', (e) => {
    if (e.target.files.length > 0) {
      const file = e.target.files[0];
      setLoadedFile(file.name, file);
    }
  });

  // 拖曳上傳
  dropzone.addEventListener('dragover', (e) => {
    e.preventDefault();
    dropzone.style.borderColor = '#00f0ff';
  });

  dropzone.addEventListener('dragleave', () => {
    dropzone.style.borderColor = 'rgba(0, 240, 255, 0.35)';
  });

  dropzone.addEventListener('drop', (e) => {
    e.preventDefault();
    dropzone.style.borderColor = 'rgba(0, 240, 255, 0.35)';
    if (e.dataTransfer.files.length > 0) {
      const file = e.dataTransfer.files[0];
      setLoadedFile(file.name, file);
    }
  });

  function setLoadedFile(name, fileObj = null) {
    currentVideoPath = name;
    fileBadge.style.display = 'block';
    fileBadge.innerText = `已載入: ${name}`;
    btnProcess.disabled = false;
  }

  // 產生示範測試影片按鈕
  btnGenerateDemo.addEventListener('click', () => {
    updateProgress(5, '正在建立 4 分割魚眼示範影片...');
    progressCard.style.display = 'flex';

    if (pyBridge) {
      pyBridge.generateDemoVideo();
    } else {
      // 網頁獨立預覽模式：直接請求本地已生成好的示範檔
      fetch('/sample_videos/demo_quad_fisheye.mp4')
        .then(res => {
          if (res.ok) {
            setLoadedFile('sample_videos/demo_quad_fisheye.mp4');
            updateProgress(100, '示範影片準備就緒！請點擊下方「開始裁切與 v360 校正」');
          }
        });
    }
  });

  // 一鍵執行裁切與 v360 魚眼校正
  btnProcess.addEventListener('click', () => {
    if (!currentVideoPath) {
      alert('請先匯入 4 分割魚眼影片或生成測試樣品！');
      return;
    }

    const mapping = {
      top_left: selTL.value,
      top_right: selTR.value,
      bottom_left: selBL.value,
      bottom_right: selBR.value
    };

    const inFov = parseInt(sliderInFov.value, 10);
    const outFov = parseInt(sliderOutFov.value, 10);

    progressCard.style.display = 'flex';
    updateProgress(2, '準備啟動 FFmpeg v360 濾鏡運算...');

    if (pyBridge) {
      pyBridge.startProcessing(currentVideoPath, JSON.stringify(mapping), inFov, outFov);
    } else {
      // 若在純瀏覽器測試，直接載入已輸出的 output 影片
      setTimeout(() => updateProgress(40, '模擬 FFmpeg 象限裁切...'), 500);
      setTimeout(() => updateProgress(75, '模擬 v360 魚眼拉直運算...'), 1200);
      setTimeout(() => {
        updateProgress(100, '處理完成！正在掛載動態材質...');
        onProcessingDone({
          front: '/output/front.mp4',
          rear: '/output/rear.mp4',
          left: '/output/left.mp4',
          right: '/output/right.mp4'
        });
      }, 1800);
    }
  });

  function updateProgress(pct, msg) {
    progressBar.style.width = `${pct}%`;
    progressStatus.innerText = msg;
    if (pct >= 100) {
      setTimeout(() => {
        progressCard.style.display = 'none';
      }, 2500);
    }
  }

  function onProcessingDone(res) {
    console.log('校正完成，掛載 4 路影片至 3D 空間:', res);
    player.loadVideos({
      front: res.front,
      rear: res.rear,
      left: res.left,
      right: res.right
    });
  }

  // 快捷視角切換列
  document.querySelectorAll('.preset-btn').forEach(btn => {
    btn.addEventListener('click', () => {
      document.querySelectorAll('.preset-btn').forEach(b => b.classList.remove('active'));
      btn.classList.add('active');
      const preset = btn.dataset.preset;
      viewer.setViewPreset(preset);
    });
  });

  // 參數滑動事件
  sliderInFov.addEventListener('input', (e) => {
    valInFov.innerText = `${e.target.value}°`;
  });

  sliderOutFov.addEventListener('input', (e) => {
    valOutFov.innerText = `${e.target.value}°`;
  });

  sliderScreenDist.addEventListener('input', (e) => {
    const val = parseFloat(e.target.value);
    valScreenDist.innerText = `${val.toFixed(1)}m`;
    viewer.setScreenDistance(val);
  });

  // 預設載入示範影片（若 output 目錄已具備校正檔案，直接自動載入 3D 體驗）
  fetch('/output/front.mp4').then(res => {
    if (res.ok) {
      player.loadVideos({
        front: '/output/front.mp4',
        rear: '/output/rear.mp4',
        left: '/output/left.mp4',
        right: '/output/right.mp4'
      });
      setLoadedFile('sample_videos/demo_quad_fisheye.mp4');
    }
  }).catch(() => {});
});
