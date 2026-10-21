// ===== Tauri 桥接层（替代 Electron preload，接口保持 window.api.* 不变） =====
window.api = {
  startDownload: (url) => window.__TAURI__.core.invoke('download_start', { url }),
  onProgress: (cb) => window.__TAURI__.event.listen('download-progress', (e) => cb(e.payload)),
  getConfig: () => window.__TAURI__.core.invoke('config_get'),
  setConfig: (cfg) => window.__TAURI__.core.invoke('config_set', { newCfg: cfg }),
  pickTempDir: () => window.__TAURI__.core.invoke('config_pick_temp_dir'),
  openContextMenu: () => window.__TAURI__.core.invoke('open_context_menu'),
  onActivateInput: (cb) => window.__TAURI__.event.listen('float-activate-input', () => cb()),
  onToast: (cb) => window.__TAURI__.event.listen('float-toast', (e) => cb(e.payload)),
  notifyBlur: () => window.__TAURI__.core.invoke('notify_blur'),
  resizeWindow: (w, h) => window.__TAURI__.core.invoke('window_resize', { w, h }),
  startDrag: () => window.__TAURI__.core.invoke('drag_start'),
  stopDrag: () => window.__TAURI__.core.invoke('drag_end'),
  requestClose: () => window.__TAURI__.core.invoke('request_close'),
  onGlobalMouse: (cb) => window.__TAURI__.event.listen('mouse-global', (e) => cb(e.payload)),
  onOutsideClick: (cb) => window.__TAURI__.event.listen('mouse-outside-click', () => cb())
};

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

// ===== emotion-ball 表情引擎 =====
// 球体 / 眼睛 / 表情 / 粒子全部由引擎在 #eb-render 内渲染；
// idle:false 关闭引擎自带空闲逻辑，沿用下方现有计时器（1分50秒困 / 2分钟睡）
const ball = EmotionBall.create('#eb-render', {
  emotion: '02',    // 默认：待机放空
  shape: 'blob',    // 圆胖形，最接近现有圆球
  idle: false,      // 空闲计时由 renderer.js 控制
  eyeScale: 1.3,    // 小窗口（64px 主体）下放大眼睛保证可读（1.6 过大会连眼，取 1.3）
  eyeGap: 12,       // 左右眼各外推 12 单位，拉开间距防止眯眼/注视时连到一起
  zzzScale: 3.0     // 睡眠 zzz 放大 2 倍（64px 小窗下更清晰）
});

let expanded = false;
let downloading = false;

// ===== 消息提示气泡（清理完成、复制成功等，对话冒泡样式） =====
let msgTimer = null;

async function showMsg(msg, duration = 2200) {
  // 若输入气泡仍展开，先隐藏（兜底；正常调用前已收起）
  if (expanded) {
    bubbleHide(bubble);
    expanded = false;
    urlInput.value = '';
    progressWrap.classList.add('hidden');
    statusText.textContent = '';
    statusText.className = 'status-text';
  }
  clearTimeout(msgTimer);
  msgBubble.textContent = msg;
  await resizeNow(MSG_SIZE[0], MSG_SIZE[1]);
  // 等窗口尺寸真正落屏后再播放入场，避免气泡在窗口仍处于收缩尺寸时开始淡入而被裁切
  requestAnimationFrame(() => bubbleShow(msgBubble));
  msgTimer = setTimeout(() => {
    bubbleHide(msgBubble);
    resizeLater(COLLAPSED_SIZE[0], COLLAPSED_SIZE[1]);
  }, duration);
}

// ===== 展开 / 收起（主体始终显示，气泡在主体上方弹出） =====
const COLLAPSED_SIZE = [112, 112];
const EXPANDED_SIZE = [264, 232];
const MSG_SIZE = [264, 142];

// resize 窗口尺寸（表情动画由引擎 rAF 驱动，无需额外处理）
async function resizeWindowSmooth(w, h) {
  await window.api.resizeWindow(w, h);
}

// ===== 气泡显隐过渡动画（.entering 入场 / .closing 退场，时长与 styles.css 保持一致） =====
const BUBBLE_HIDE_MS = 200;   // 退场动画时长 ms（styles.css 中 .bubble.closing 的 transition-duration）
let pendingResize = null;     // 待执行的窗口尺寸调整（等气泡退场结束后再收缩）

// 先让浏览器确认透明的起始样式，再切到可见态，否则同一帧中
// 去掉 display:none 并设为 opacity:1 会跳过过渡，气泡瞬间出现。
function bubbleShow(el) {
  clearTimeout(el._hideTimer);
  el.classList.remove('closing');
  el.classList.remove('entering');
  el.classList.remove('hidden');
  void el.offsetWidth;
  el.classList.add('entering');
}

// 退场：先播插值到隐藏态，动画结束后（BUBBLE_HIDE_MS）再真正置为 display:none
function bubbleHide(el) {
  if (el.classList.contains('hidden')) return;
  clearTimeout(el._hideTimer);
  el.classList.remove('entering');
  el.classList.add('closing');
  el._hideTimer = setTimeout(() => {
    el.classList.add('hidden');
    el.classList.remove('closing');
  }, BUBBLE_HIDE_MS);
}

// 立即调整窗口尺寸（取消可能待执行的延迟调整，避免旧的收起延迟误触发）
async function resizeNow(w, h) {
  clearTimeout(pendingResize);
  pendingResize = null;
  await resizeWindowSmooth(w, h);
}

// 延迟调整窗口尺寸：等气泡退场动画结束后再收缩，避免淡出过程中被窗口边缘裁切
function resizeLater(w, h, delay = BUBBLE_HIDE_MS) {
  clearTimeout(pendingResize);
  pendingResize = setTimeout(() => {
    pendingResize = null;
    resizeWindowSmooth(w, h);
  }, delay);
}

async function expand() {
  if (expanded) return;
  expanded = true;
  // 窗口先展开，等尺寸落屏后再播放入场，避免气泡被窗口边缘裁切/闪动
  await resizeNow(EXPANDED_SIZE[0], EXPANDED_SIZE[1]);
  requestAnimationFrame(() => {
    // 收起已先于入场发生（快速切换）时跳过，避免误弹气泡
    if (!expanded) return;
    bubbleShow(bubble);
    setTimeout(() => urlInput.focus(), 50);
  });
}

async function collapse() {
  if (!expanded) return;
  expanded = false;
  bubbleHide(bubble);
  urlInput.value = '';
  progressWrap.classList.add('hidden');
  statusText.textContent = '';
  statusText.className = 'status-text';
  resizeLater(COLLAPSED_SIZE[0], COLLAPSED_SIZE[1]); // 退场动画结束后再收缩窗口
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
  // 仅记录按下位置并进入拖动预备，不唤醒睡眠（睡眠只能被"点击"取消，见 mouseup）
  downPos = { x: e.screenX, y: e.screenY };
  dragArmed = true;
  window.api.startDrag();
});

document.addEventListener('mouseup', (e) => {
  if (e.button !== 0 || !dragArmed) return;
  const moved = Math.hypot(e.screenX - downPos.x, e.screenY - downPos.y);
  dragArmed = false;
  window.api.stopDrag();
  // 未移动视为点击：点击主体切换展开/收起；点击窗口内其他非对话框区域则收起气泡
  if (moved < DRAG_CLICK_THRESHOLD) {
    if (e.target.closest('#floatButton')) {
      // 只有点击才唤醒睡眠（拖动不再唤醒），先进入“困”过渡
      wakeUp();
      // 点击自旋一圈：引擎达速后甩出一圈彩带（进行中的自旋不可打断，重复点击自动忽略）
      ball.spin(1);
      // 连续点击计数：快速点击三下触发生气表情
      clickCount++;
      clearTimeout(clickTimer);
      clickTimer = setTimeout(() => { clickCount = 0; }, CLICK_WINDOW);
      resetIdleTimer();
      if (clickCount >= 3) {
        clickCount = 0;
        showMood('21'); // 生气
        return;
      }
      if (expanded) collapse(); else expand();
    } else if (expanded && !e.target.closest('#bubble')) {
      // 点击窗口内空白（输入对话框以外）→ 收起气泡
      collapse();
    }
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

// ===== 随机表情池 =====
// 从 32 个表情中剔除固定触发的 5 个（00 睡眠 / 10 开心 / 12 失落 / 15 疲惫 / 21 生气），
// 其余 27 个全部进入随机池，随机间隔展示后回落“待机放空 02”
const RANDOM_EMOTIONS = [
  '01', '02', '03', '04', '05', '06', '07', // 生命周期：唤醒/待机/好奇/发呆/加载苏醒/休眠/抖动唤醒
  '11', '13', '14', '16', '17', '18', '19', '20', // 情绪：疑惑/惊讶/害羞/专注/慌张/无奈/满意/困惑
  '30', '31', '32', '33', '34', '35', '36', '37', '38', '39', '40', '41' // 代理状态：思考中/接收任务/忙碌/完成/出错/等待输入/联网加载/回忆/拒绝/回复/检索/停止
];
let eyeTimer = null;

// 连续点击三下触发：生气
let clickCount = 0;
let clickTimer = null;
const CLICK_WINDOW = 800; // 连续点击判定窗口 ms

function randomOf(arr) { return arr[Math.floor(Math.random() * arr.length)]; }

function scheduleEyeAnim() {
  // 随机间隔 1.2s ~ 4.2s 触发一次动画
  const delay = 1200 + Math.random() * 3000;
  eyeTimer = setTimeout(() => playEyeAnim(), delay);
}

function playEyeAnim() {
  // 表情展示期间（含睡觉）跳过随机动画，避免互相干扰
  if (moodActive) { scheduleEyeAnim(); return; }
  showMood(randomOf(RANDOM_EMOTIONS), false, 1600 + Math.random() * 400);
  scheduleEyeAnim();
}

// 页面加载后启动随机表情循环
// （空闲睡觉计时 resetIdleTimer 在文件末尾启动，避免 let 变量 TDZ 问题）
scheduleEyeAnim();

// ===== 表情状态：由 emotion-ball 引擎渲染，执行时机不变 =====
let moodTimer = null;
let moodActive = false;
// 睡觉持续状态：2 分钟无点击触发，点击主体唤醒
let sleeping = false;
// 唤醒过渡：刚醒来先进入“困”状态，结束后再回到随机表情
let waking = false;
let idleTimer = null;
let drowsyTimer = null;
const IDLE_DROWSY_MS = 110000; // 1 分 50 秒：先犯困（持续 10 秒）
const IDLE_SLEEP_MS = 120000; // 2 分钟：进入睡眠
// 各表情展示时长（犯困默认 5 秒，空闲犯困由调用方显式传 10 秒覆盖）
const MOOD_DURATION = {
  '10': 2000, // 开心
  '12': 2000, // 失落（复制失败）
  '15': 5000, // 疲惫（困，唤醒后 5 秒）
  '21': 3000  // 生气
};

// 回落目标：待机放空
const IDLE_EMOTION = '02';

function clearMood() {
  ball.setEmotion(IDLE_EMOTION);
  moodActive = false;
  clearTimeout(moodTimer);
}

function showMood(id, persistent = false, duration) {
  clearMood();
  ball.setEmotion(id);
  moodActive = true;
  if (persistent) return; // 持续表情（睡觉）不自动退出，等待点击唤醒
  moodTimer = setTimeout(clearMood, duration || MOOD_DURATION[id] || 2000);
}

// 空闲犯困：1 分 50 秒先播放“困”10 秒，120 秒由 enterSleep 覆盖进入睡眠
function enterDrowsy() {
  if (sleeping) return;
  showMood('15', false, 10000); // 疲惫（困）
  // 收尾加守卫：若期间已进入睡眠，则不清场，避免把睡眠表情清掉
  clearTimeout(moodTimer);
  moodTimer = setTimeout(() => { if (!sleeping) clearMood(); }, 10000);
}

// 空闲睡觉：超过 2 分钟没有点击主体 → 进入持续睡觉状态
function enterSleep() {
  sleeping = true;
  waking = false;
  showMood('00', true); // 睡眠
}

// 睡眠被唤醒：先进入“困”5 秒，结束后随机表情自然恢复
function wakeUp() {
  if (!sleeping && !waking) return;
  sleeping = false;
  waking = true;
  clearMood();
  showMood('15', false, MOOD_DURATION['15']); // 疲惫（困）
  setTimeout(() => { waking = false; }, MOOD_DURATION['15']);
}

function resetIdleTimer() {
  clearTimeout(idleTimer);
  clearTimeout(drowsyTimer);
  drowsyTimer = setTimeout(enterDrowsy, IDLE_DROWSY_MS);
  idleTimer = setTimeout(enterSleep, IDLE_SLEEP_MS);
}

// ===== 眼睛：跟随鼠标（整个屏幕范围，主进程每 33ms 轮询全局鼠标位置） =====
window.api.onGlobalMouse(({ mx, my, wx, wy }) => {
  // 生气表情时取消瞳孔跟随（愤怒的眼神固定）
  if (ball.emotionId === '21') { ball.clearGaze(); return; }
  const rect = btn.getBoundingClientRect();
  // 主体中心在屏幕上的坐标 = 窗口位置 + 主体在窗口内的位置
  const cx = wx + rect.left + rect.width / 2;
  const cy = wy + rect.top + rect.height / 2;
  // 归一化到 [-1, 1]，交给引擎做眼神跟随
  let dx = (mx - cx) / (rect.width / 2);
  let dy = (my - cy) / (rect.height / 2);
  dx = Math.max(-1, Math.min(1, dx));
  dy = Math.max(-1, Math.min(1, dy));
  ball.setGaze(dx, dy);
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
      showMood('10'); // 开心
      // 稍作停留后收起输入气泡，改用消息气泡提示
      setTimeout(() => {
        if (!downloading) {
          bubbleHide(bubble);
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
      showMood('10'); // 开心
      setTimeout(() => {
        if (!downloading) {
          bubbleHide(bubble);
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
    showMood('12'); // 失落（复制失败）
  }
}

// ===== 主进程触发的事件 =====
window.api.onActivateInput(() => expand());
window.api.onToast((msg) => showMsg(msg));
// 点击主窗口以外的屏幕任意位置（输入对话框以外）→ 收起输入气泡
window.api.onOutsideClick(() => { if (expanded) collapse(); });

// 所有声明就绪后启动空闲睡觉计时（2 分钟无点击触发睡觉）
resetIdleTimer();
