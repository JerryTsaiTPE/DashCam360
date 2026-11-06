/**
 * viewer3d.js
 * 核心 3D 虛擬空間、車身模型、4 路動態材質屏幕與視角控制器
 */

class DashCamViewer3D {
  constructor(canvasContainerId) {
    this.container = document.getElementById(canvasContainerId);
    this.scene = null;
    this.camera = null;
    this.renderer = null;
    this.controls = null;

    // 動態屏幕與材質
    this.screens = {};
    this.videoTextures = {};
    this.cylinderScreen = null;
    this.carGroup = null;

    // 視圖設定
    this.viewMode = 'surround'; // 'surround', 'cylinder'
    this.screenWidth = 4.2;
    this.screenHeight = 2.4;
    this.screenDist = 3.6;
    this.screenCurve = 0.4; // 屏幕弧度

    // 動畫補間
    this.targetCameraPos = null;
    this.targetLookAt = null;
    this.isTransitioning = false;

    this.init();
  }

  init() {
    const width = this.container.clientWidth;
    const height = this.container.clientHeight;

    // 1. 場景
    this.scene = new THREE.Scene();
    this.scene.background = new THREE.Color(0x0c1017);
    this.scene.fog = new THREE.FogExp2(0x0c1017, 0.035);

    // 2. 透視相機
    this.camera = new THREE.PerspectiveCamera(55, width / height, 0.1, 100);
    this.camera.position.set(0, 5.5, 7.5);

    // 3. WebGL 渲染器
    this.renderer = new THREE.WebGLRenderer({ antialias: true, alpha: true, powerPreference: 'high-performance' });
    this.renderer.setSize(width, height);
    this.renderer.setPixelRatio(Math.min(window.devicePixelRatio, 2));
    this.renderer.shadowMap.enabled = true;
    this.renderer.shadowMap.type = THREE.PCFSoftShadowMap;
    this.container.appendChild(this.renderer.domElement);

    // 4. OrbitControls 滑鼠控制 (360度旋轉、縮放、平移)
    this.controls = new THREE.OrbitControls(this.camera, this.renderer.domElement);
    this.controls.enableDamping = true;
    this.controls.dampingFactor = 0.06;
    this.controls.minDistance = 1.2;
    this.controls.maxDistance = 22.0;
    this.controls.maxPolarAngle = Math.PI / 2 + 0.05; // 稍微允許看到地面下緣
    this.controls.target.set(0, 0.6, 0);

    // 5. 光照系統
    this.setupLighting();

    // 6. 地面與雷達標線
    this.setupGround();

    // 7. 3D 車身幾何模型
    this.setupCarModel();

    // 8. 建立 4 路環景動態屏幕
    this.setupScreens();

    // 監聽視窗縮放
    window.addEventListener('resize', () => this.onWindowResize());

    // 開始渲染循環
    this.animate = this.animate.bind(this);
    requestAnimationFrame(this.animate);
  }

  setupLighting() {
    const ambientLight = new THREE.AmbientLight(0xffffff, 0.65);
    this.scene.add(ambientLight);

    const dirLight = new THREE.DirectionalLight(0xddeeff, 1.2);
    dirLight.position.set(5, 12, 8);
    dirLight.castShadow = true;
    dirLight.shadow.mapSize.width = 1024;
    dirLight.shadow.mapSize.height = 1024;
    this.scene.add(dirLight);

    // 藍色科技地表底光
    const bottomLight = new THREE.PointLight(0x00f0ff, 0.8, 12);
    bottomLight.position.set(0, 0.3, 0);
    this.scene.add(bottomLight);
  }

  setupGround() {
    // 科技網格
    const grid = new THREE.GridHelper(24, 24, 0x00a0ff, 0x1e2838);
    grid.position.y = 0.01;
    this.scene.add(grid);

    // 地面反光平面
    const planeGeo = new THREE.PlaneGeometry(36, 36);
    const planeMat = new THREE.MeshStandardMaterial({
      color: 0x0a0e14,
      roughness: 0.8,
      metalness: 0.2
    });
    const ground = new THREE.Mesh(planeGeo, planeMat);
    ground.rotation.x = -Math.PI / 2;
    ground.receiveShadow = true;
    this.scene.add(ground);

    // 雷達同心距離圓環 (1m, 2m, 3m, 4m)
    const ringRadii = [1.5, 2.5, 3.5, 4.5];
    const ringColors = [0xff3366, 0xffaa00, 0x00ffaa, 0x00d2ff];

    ringRadii.forEach((r, idx) => {
      const ringGeo = new THREE.RingGeometry(r - 0.02, r + 0.02, 64);
      const ringMat = new THREE.MeshBasicMaterial({
        color: ringColors[idx],
        side: THREE.DoubleSide,
        transparent: true,
        opacity: 0.45
      });
      const ring = new THREE.Mesh(ringGeo, ringMat);
      ring.rotation.x = -Math.PI / 2;
      ring.position.y = 0.02;
      this.scene.add(ring);
    });
  }

  setupCarModel() {
    this.carGroup = new THREE.Group();

    // 科技感半透明車身主體
    const bodyGeo = new THREE.BoxGeometry(1.8, 0.85, 4.2);
    const bodyMat = new THREE.MeshStandardMaterial({
      color: 0x224466,
      metalness: 0.85,
      roughness: 0.25,
      transparent: true,
      opacity: 0.85
    });
    const body = new THREE.Mesh(bodyGeo, bodyMat);
    body.position.y = 0.7;
    body.castShadow = true;
    this.carGroup.add(body);

    // 車頂與擋風玻璃
    const cabinGeo = new THREE.BoxGeometry(1.5, 0.65, 2.2);
    const cabinMat = new THREE.MeshStandardMaterial({
      color: 0x112233,
      metalness: 0.9,
      roughness: 0.1,
      transparent: true,
      opacity: 0.7
    });
    const cabin = new THREE.Mesh(cabinGeo, cabinMat);
    cabin.position.set(0, 1.35, -0.2);
    this.carGroup.add(cabin);

    // 車頭指示箭頭 (標示前方 FRONT，朝向 -Z)
    const arrowShape = new THREE.Shape();
    arrowShape.moveTo(0, 0.7);
    arrowShape.lineTo(0.5, -0.4);
    arrowShape.lineTo(0.2, -0.4);
    arrowShape.lineTo(0.2, -0.8);
    arrowShape.lineTo(-0.2, -0.8);
    arrowShape.lineTo(-0.2, -0.4);
    arrowShape.lineTo(-0.5, -0.4);
    arrowShape.closePath();

    const arrowGeo = new THREE.ShapeGeometry(arrowShape);
    const arrowMat = new THREE.MeshBasicMaterial({ color: 0x00f0ff, side: THREE.DoubleSide });
    const arrow = new THREE.Mesh(arrowGeo, arrowMat);
    arrow.rotation.x = -Math.PI / 2;
    arrow.position.set(0, 1.72, -0.2);
    this.carGroup.add(arrow);

    // 4 個輪胎
    const wheelGeo = new THREE.CylinderGeometry(0.38, 0.38, 0.26, 24);
    const wheelMat = new THREE.MeshStandardMaterial({ color: 0x1a1a1a, roughness: 0.9 });
    const wheelPositions = [
      [-0.95, 0.38, -1.3],
      [0.95, 0.38, -1.3],
      [-0.95, 0.38, 1.3],
      [0.95, 0.38, 1.3]
    ];

    wheelPositions.forEach(pos => {
      const wheel = new THREE.Mesh(wheelGeo, wheelMat);
      wheel.rotation.z = Math.PI / 2;
      wheel.position.set(...pos);
      wheel.castShadow = true;
      this.carGroup.add(wheel);
    });

    // 車頭 LED 大燈 (發光前錐)
    const headLightGeo = new THREE.ConeGeometry(0.8, 3.5, 16, 1, true);
    const headLightMat = new THREE.MeshBasicMaterial({
      color: 0x88e0ff,
      transparent: true,
      opacity: 0.15,
      side: THREE.DoubleSide
    });
    const leftBeam = new THREE.Mesh(headLightGeo, headLightMat);
    leftBeam.rotation.x = Math.PI / 2;
    leftBeam.position.set(-0.6, 0.6, -3.2);
    this.carGroup.add(leftBeam);

    const rightBeam = new THREE.Mesh(headLightGeo, headLightMat);
    rightBeam.rotation.x = Math.PI / 2;
    rightBeam.position.set(0.6, 0.6, -3.2);
    this.carGroup.add(rightBeam);

    this.scene.add(this.carGroup);
  }

  createCurvedScreenGeo(width, height, radius = 5.0) {
    // 建立帶有優雅弧度的曲面螢幕 (比平直四方更具沉浸立體感)
    const segments = 24;
    const angle = width / radius;
    return new THREE.CylinderGeometry(radius, radius, height, segments, 1, true, -angle / 2, angle);
  }

  setupScreens() {
    const roles = ['front', 'rear', 'left', 'right'];
    const screenGeo = this.createCurvedScreenGeo(this.screenWidth, this.screenHeight, 6.0);

    roles.forEach(role => {
      // 預設純色加網格紋理，當影片載入時替換為 VideoTexture
      const canvas = document.createElement('canvas');
      canvas.width = 512;
      canvas.height = 256;
      const ctx = canvas.getContext('2d');
      ctx.fillStyle = '#141c28';
      ctx.fillRect(0, 0, 512, 256);
      ctx.strokeStyle = '#00a0ff';
      ctx.lineWidth = 4;
      ctx.strokeRect(10, 10, 492, 236);
      ctx.fillStyle = '#ffffff';
      ctx.font = 'bold 36px sans-serif';
      ctx.textAlign = 'center';
      ctx.fillText(role.toUpperCase() + ' CAM', 256, 130);
      ctx.font = '20px sans-serif';
      ctx.fillStyle = '#8a99ad';
      ctx.fillText('Waiting for video...', 256, 170);

      const placeholderTexture = new THREE.CanvasTexture(canvas);
      const mat = new THREE.MeshBasicMaterial({
        map: placeholderTexture,
        side: THREE.DoubleSide
      });

      const mesh = new THREE.Mesh(screenGeo, mat);
      this.screens[role] = mesh;
      this.scene.add(mesh);
    });

    this.updateScreenPositions();
  }

  updateScreenPositions() {
    const d = this.screenDist;
    const h = 1.6;

    // 前屏幕 (朝向車頭 Z-)
    if (this.screens.front) {
      this.screens.front.position.set(0, h, -d);
      this.screens.front.rotation.set(0, 0, 0);
    }

    // 後屏幕 (朝向車後 Z+)
    if (this.screens.rear) {
      this.screens.rear.position.set(0, h, d);
      this.screens.rear.rotation.set(0, Math.PI, 0);
    }

    // 左屏幕 (朝向車左 X-)
    if (this.screens.left) {
      this.screens.left.position.set(-d, h, 0);
      this.screens.left.rotation.set(0, Math.PI / 2, 0);
    }

    // 右屏幕 (朝向車右 X+)
    if (this.screens.right) {
      this.screens.right.position.set(d, h, 0);
      this.screens.right.rotation.set(0, -Math.PI / 2, 0);
    }
  }

  setVideoTexture(role, videoElement) {
    if (!this.screens[role]) return;

    const texture = new THREE.VideoTexture(videoElement);
    texture.minFilter = THREE.LinearFilter;
    texture.magFilter = THREE.LinearFilter;
    texture.format = THREE.RGBAFormat;

    this.videoTextures[role] = texture;

    // 設定新材質
    this.screens[role].material = new THREE.MeshBasicMaterial({
      map: texture,
      side: THREE.DoubleSide
    });
  }

  setScreenDistance(dist) {
    this.screenDist = dist;
    this.updateScreenPositions();
  }

  setScreenHeight(h) {
    Object.values(this.screens).forEach(screen => {
      screen.position.y = h;
    });
  }

  setViewPreset(preset) {
    // 快捷視角動畫設定
    const presets = {
      birdEye: { pos: [0, 11.0, 0.1], target: [0, 0, 0] },
      front: { pos: [0, 1.8, -1.2], target: [0, 1.6, -6.0] },
      rear: { pos: [0, 1.8, 1.2], target: [0, 1.6, 6.0] },
      left: { pos: [-1.2, 1.8, 0], target: [-6.0, 1.6, 0] },
      right: { pos: [1.2, 1.8, 0], target: [6.0, 1.6, 0] },
      cockpit: { pos: [0, 1.3, -0.2], target: [0, 1.4, -4.0] },
      orbit: { pos: [4.5, 4.5, 6.0], target: [0, 0.8, 0] }
    };

    const config = presets[preset] || presets.orbit;
    this.transitionCamera(new THREE.Vector3(...config.pos), new THREE.Vector3(...config.target));
  }

  transitionCamera(targetPos, targetLookAt) {
    this.targetCameraPos = targetPos;
    this.targetLookAt = targetLookAt;
    this.isTransitioning = true;
  }

  onWindowResize() {
    const width = this.container.clientWidth;
    const height = this.container.clientHeight;
    this.camera.aspect = width / height;
    this.camera.updateProjectionMatrix();
    this.renderer.setSize(width, height);
  }

  animate() {
    requestAnimationFrame(this.animate);

    // 平滑視角過渡插值
    if (this.isTransitioning && this.targetCameraPos) {
      this.camera.position.lerp(this.targetCameraPos, 0.08);
      this.controls.target.lerp(this.targetLookAt, 0.08);

      if (this.camera.position.distanceTo(this.targetCameraPos) < 0.05) {
        this.camera.position.copy(this.targetCameraPos);
        this.controls.target.copy(this.targetLookAt);
        this.isTransitioning = false;
      }
    }

    this.controls.update();
    this.renderer.render(this.scene, this.camera);
  }
}

window.DashCamViewer3D = DashCamViewer3D;
