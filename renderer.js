const btn = document.getElementById('floatButton');
const miniClose = document.getElementById('miniClose');
const bubble = document.getElementById('bubble');
const bubbleClose = document.getElementById('bubbleClose');
const urlInput = document.getElementById('urlInput');
const progressWrap = document.getElementById('progressWrap');
const progressFill = document.getElementById('progressFill');
const progressText = document.getElementById('progressText');
const statusText = document.getElementById('statusText');
const msgBubble = document.getElementById('msgBubble');
const eyes = document.getElementById('eyes');
const pupils = document.querySelectorAll('.pupil');
const sleepMark = document.getElementById('sleepMark');
const angryMark = document.getElementById('angryMark');

let expanded = false;
let downloading = false;

// ===== 消息提示气泡（清理完成、复制成功等，对话冒泡样式） =====
let msgTimer = null;

async function showMsg(msg, duration = 2200) {
  // 若输入气泡仍展开，先隐藏（兜底；正常调用前已收起）
  if (expanded) {
    bubble.classList.add('hidden');
    expanded = false;
    urlInput.value = '';
    progressWrap.classList.add('hidden');
    statusText.textContent = '';
    statusText.className = 'status-text';
  }
  clearTimeout(msgTimer);
  msgBubble.textContent = msg;
  await resizeWindowSmooth(MSG_SIZE[0], MSG_SIZE[1]);
  msgBubble.classList.remove('hidden');
  msgTimer = setTimeout(() => {
    msgBubble.classList.add('hidden');
    resizeWindowSmooth(COLLAPSED_SIZE[0], COLLAPSED_SIZE[1]);
  }, duration);
}

// ===== 展开 / 收起（主体始终显示，气泡在主体上方弹出） =====
const COLLAPSED_SIZE = [112, 112];
const EXPANDED_SIZE = [264, 232];
const MSG_SIZE = [264, 142];

// resize 时暂停主体的呼吸动画，避免窗口尺寸变化时合成层重绘导致主体闪烁
async function resizeWindowSmooth(w, h) {
  const prev = btn.style.animationPlayState || '';
  btn.style.animationPlayState = 'paused';
  await window.api.resizeWindow(w, h);
  requestAnimationFrame(() => { btn.style.animationPlayState = prev; });
}

async function expand() {
  if (expanded) return;
  expanded = true;
  bubble.classList.remove('hidden');
  await resizeWindowSmooth(EXPANDED_SIZE[0], EXPANDED_SIZE[1]);
  setTimeout(() => urlInput.focus(), 50);
}

async function collapse() {
  if (!expanded) return;
  expanded = false;
  bubble.classList.add('hidden');
  urlInput.value = '';
  progressWrap.classList.add('hidden');
  statusText.textContent = '';
  statusText.className = 'status-text';
  await resizeWindowSmooth(COLLAPSED_SIZE[0], COLLAPSED_SIZE[1]);
}

// ===== 窗口拖动（手动实现）与点击区分 =====
// 不使用 -webkit-app-region: drag（它会拦截鼠标事件），
// 由主进程轮询鼠标位置移动窗口；按下后移动距离小于阈值视为点击。
const DRAG_CLICK_THRESHOLD = 6; // 像素
let dragArmed = false;
let downPos = null;

// 可交互元素（输入框/按钮）不参与拖动
function isInteractive(el) {
  return !!el.closest('input, button, .mini-close');
}

document.addEventListener('mousedown', (e) => {
  if (e.button !== 0) return;
  if (isInteractive(e.target)) return;
  // 按下主体即唤醒睡觉（点击、拖动都算接触）
  if (sleeping && e.target.closest('#floatButton')) {
    sleeping = false;
    clearMood();
  }
  downPos = { x: e.screenX, y: e.screenY };
  dragArmed = true;
  window.api.startDrag();
});

document.addEventListener('mouseup', (e) => {
  if (e.button !== 0 || !dragArmed) return;
  const moved = Math.hypot(e.screenX - downPos.x, e.screenY - downPos.y);
  dragArmed = false;
  window.api.stopDrag();
  // 未移动视为点击：点击主体切换展开/收起
  if (moved < DRAG_CLICK_THRESHOLD && e.target.closest('#floatButton')) {
    // 点击唤醒：退出持续睡觉状态
    if (sleeping) { sleeping = false; clearMood(); }
    // 连续点击计数：快速点击三下触发生气表情
    clickCount++;
    clearTimeout(clickTimer);
    clickTimer = setTimeout(() => { clickCount = 0; }, CLICK_WINDOW);
    resetIdleTimer();
    if (clickCount >= 3) {
      clickCount = 0;
      showMood('angry');
      return;
    }
    if (expanded) collapse(); else expand();
  }
});

// 兜底：鼠标快速甩出窗口导致 mouseup 丢失时，停止拖动
document.addEventListener('mouseleave', () => {
  if (dragArmed) {
    dragArmed = false;
    window.api.stopDrag();
  }
});

// ===== 事件绑定 =====
btn.addEventListener('contextmenu', (e) => {
  e.preventDefault();
  window.api.openContextMenu();
});

// 右上角关闭按钮：退出或最小化到托盘（由设置决定）
miniClose.addEventListener('click', (e) => {
  e.stopPropagation();
  window.api.requestClose();
});

// 对话弹窗右上角关闭按钮：仅收起气泡
bubbleClose.addEventListener('click', (e) => {
  e.stopPropagation();
  collapse();
});

// Esc 收起输入气泡
document.addEventListener('keydown', (e) => {
  if (e.key === 'Escape' && expanded) collapse();
});

// ===== 眼睛：随机循环动画（眨眼 / wink / 变大变小 / 大小眼 / 无语） =====
// 注意：睡觉不在此池中，由空闲 2 分钟触发
const EYE_ANIMS = ['blink', 'wink', 'bigsmall', 'leftbig', 'rightbig', 'speechless'];
let eyeTimer = null;

// 连续点击三下触发：生气
let clickCount = 0;
let clickTimer = null;
const CLICK_WINDOW = 800; // 连续点击判定窗口 ms

function randomOf(arr) { return arr[Math.floor(Math.random() * arr.length)]; }

function scheduleEyeAnim() {
  // 随机间隔 1.2s ~ 4.2s 触发一次动画
  const delay = 1200 + Math.random() * 3000;
  eyeTimer = setTimeout(() => playEyeAnim(randomOf(EYE_ANIMS)), delay);
}

function playEyeAnim(type) {
  // 表情展示期间跳过随机动画，避免互相干扰
  if (moodActive) { scheduleEyeAnim(); return; }
  if (type === 'blink') {
    eyes.classList.add('blinking');
    setTimeout(() => eyes.classList.remove('blinking'), 340);
  } else if (type === 'wink') {
    const side = Math.random() < 0.5 ? 'left-eye' : 'right-eye';
    const eye = eyes.querySelector('.' + side);
    eye.classList.add('winking');
    setTimeout(() => eye.classList.remove('winking'), 380);
  } else if (type === 'bigsmall') {
    eyes.classList.add('bigsmall');
    setTimeout(() => eyes.classList.remove('bigsmall'), 760);
  } else {
    // leftbig / rightbig / speechless / sleep 走表情状态
    showMood(type);
  }
  scheduleEyeAnim();
}

// 页面加载后启动眼睛动画循环
// （空闲睡觉计时 resetIdleTimer 在文件末尾启动，避免 let 变量 TDZ 问题）
scheduleEyeAnim();

// ===== 表情状态：开心/伤心/大小眼/无语/睡觉/生气 =====
let moodTimer = null;
let moodActive = false;
// 睡觉持续状态：2 分钟无点击触发，点击主体唤醒
let sleeping = false;
let idleTimer = null;
const IDLE_SLEEP_MS = 120000; // 2 分钟无点击触发睡觉
// 各表情展示时长
const MOOD_DURATION = {
  happy: 2000, sad: 2000, leftbig: 1600, rightbig: 1600,
  speechless: 1700, sleep: 2800, angry: 3000
};
const MOOD_CLASSES = ['happy', 'sad', 'leftbig', 'rightbig', 'speechless', 'sleep', 'angry'];

function clearMood() {
  eyes.classList.remove(...MOOD_CLASSES);
  sleepMark.classList.remove('visible');
  angryMark.classList.remove('visible');
  moodActive = false;
  clearTimeout(moodTimer);
}

function showMood(type, persistent = false) {
  clearMood();
  if (MOOD_CLASSES.includes(type)) {
    eyes.classList.add(type);
    if (type === 'sleep') sleepMark.classList.add('visible');
    if (type === 'angry') angryMark.classList.add('visible');
  }
  moodActive = true;
  if (persistent) return; // 持续表情（睡觉）不自动退出，等待点击唤醒
  moodTimer = setTimeout(clearMood, MOOD_DURATION[type] || 2000);
}

// 空闲睡觉：超过 2 分钟没有点击主体 → 进入持续睡觉状态
function enterSleep() {
  sleeping = true;
  showMood('sleep', true);
}

function resetIdleTimer() {
  clearTimeout(idleTimer);
  idleTimer = setTimeout(enterSleep, IDLE_SLEEP_MS);
}

// ===== 眼睛：跟随鼠标（整个屏幕范围，主进程每 33ms 轮询全局鼠标位置） =====
const PUPIL_MAX_SHIFT = 3; // 瞳孔最大偏移量 px
window.api.onGlobalMouse(({ mx, my, wx, wy }) => {
  // 生气表情时取消瞳孔跟随（愤怒的眼神固定）
  if (eyes.classList.contains('angry')) return;
  const rect = btn.getBoundingClientRect();
  // 主体中心在屏幕上的坐标 = 窗口位置 + 主体在窗口内的位置
  const cx = wx + rect.left + rect.width / 2;
  const cy = wy + rect.top + rect.height / 2;
  // 归一化到 [-1, 1]，再限制瞳孔偏移范围
  let dx = (mx - cx) / (rect.width / 2);
  let dy = (my - cy) / (rect.height / 2);
  dx = Math.max(-1, Math.min(1, dx));
  dy = Math.max(-1, Math.min(1, dy));
  const tx = (dx * PUPIL_MAX_SHIFT).toFixed(2);
  const ty = (dy * PUPIL_MAX_SHIFT).toFixed(2);
  pupils.forEach(p => {
    p.style.transform = `translate(calc(-50% + ${tx}px), calc(-50% + ${ty}px))`;
  });
});

// 输入框回车开始下载
urlInput.addEventListener('keydown', async (e) => {
  if (e.key !== 'Enter' || downloading) return;
  const url = urlInput.value.trim();
  if (!url) {
    setStatus('请输入文件链接', 'err');
    return;
  }
  if (!/^https?:\/\//i.test(url)) {
    setStatus('链接需以 http:// 或 https:// 开头', 'err');
    return;
  }
  await doDownload(url);
});

// ===== 下载 =====
function setStatus(text, type = '') {
  statusText.textContent = text;
  statusText.className = 'status-text' + (type ? ' ' + type : '');
}

// 进度监听器只注册一次
window.api.onProgress(({ received, total, filename }) => {
  if (total > 0) {
    const pct = Math.min(100, Math.round((received / total) * 100));
    progressFill.style.width = pct + '%';
    const mb = (received / 1024 / 1024).toFixed(1);
    const totalMb = (total / 1024 / 1024).toFixed(1);
    progressText.textContent = `下载中 ${pct}%  (${mb} / ${totalMb} MB)`;
  } else {
    progressFill.style.width = '80%';
    const mb = (received / 1024 / 1024).toFixed(1);
    progressText.textContent = `已下载 ${mb} MB — ${filename}`;
  }
});

async function doDownload(url) {
  downloading = true;
  progressWrap.classList.remove('hidden');
  progressFill.style.width = '0%';
  progressText.textContent = '连接中…';
  setStatus('');

  const result = await window.api.startDownload(url);
  downloading = false;

  if (result.ok) {
    progressFill.style.width = '100%';
    progressText.textContent = '完成';
    if (result.copiedToClipboard) {
      setStatus('', 'ok');
      showMood('happy');
      // 稍作停留后收起输入气泡，改用消息气泡提示
      setTimeout(() => {
        if (!downloading) {
          bubble.classList.add('hidden');
          expanded = false;
          urlInput.value = '';
          progressWrap.classList.add('hidden');
          statusText.textContent = '';
          statusText.className = 'status-text';
          showMsg('已复制，直接粘贴');
        }
      }, 1000);
    } else {
      setStatus('✓ 已下载（剪贴板复制受限）', 'ok');
      showMood('happy');
      setTimeout(() => {
        if (!downloading) {
          bubble.classList.add('hidden');
          expanded = false;
          urlInput.value = '';
          progressWrap.classList.add('hidden');
          statusText.textContent = '';
          statusText.className = 'status-text';
          showMsg('已下载（路径已复制为文本）');
        }
      }, 1000);
    }
  } else {
    progressWrap.classList.add('hidden');
    setStatus('✗ ' + result.error, 'err');
    showMood('sad');
  }
}

// ===== 主进程触发的事件 =====
window.api.onActivateInput(() => expand());
window.api.onToast((msg) => showMsg(msg));

// 所有声明就绪后启动空闲睡觉计时（2 分钟无点击触发睡觉）
resetIdleTimer();
