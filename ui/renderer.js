// ===== Tauri 桥接层（替代 Electron preload，接口保持 window.api.* 不变） =====
window.api = {
  startDownload: (url) => window.__TAURI__.core.invoke('download_start', { url }),
  onProgress: (cb) => window.__TAURI__.event.listen('download-progress', (e) => cb(e.payload)),
  getConfig: () => window.__TAURI__.core.invoke('config_get'),
  setConfig: (cfg) => window.__TAURI__.core.invoke('config_set', { newCfg: cfg }),
  pickTempDir: () => window.__TAURI__.core.invoke('config_pick_temp_dir'),
  openContextMenu: () => window.__TAURI__.core.invoke('open_context_menu'),
  // 打开上次右键用过的工具；没有记录时返回 false（前端据此回落到收起气泡）
  openLastTool: () => window.__TAURI__.core.invoke('open_last_tool'),
  onActivateInput: (cb) => window.__TAURI__.event.listen('float-activate-input', () => cb()),
  onToast: (cb) => window.__TAURI__.event.listen('float-toast', (e) => cb(e.payload)),
  notifyBlur: () => window.__TAURI__.core.invoke('notify_blur'),
  // 连同当前 innerWidth 一起上报：后端用它和窗口物理宽度反推
  // 「1 CSS 像素 = N 物理像素」，系统缩放/页面缩放时裁剪区域才不会算小
  resizeWindow: (w, h) => window.__TAURI__.core.invoke('window_resize', {
    w, h, cssW: window.innerWidth,
  }),
  startDrag: () => window.__TAURI__.core.invoke('drag_start'),
  stopDrag: () => window.__TAURI__.core.invoke('drag_end'),
  requestClose: () => window.__TAURI__.core.invoke('request_close'),
  onGlobalMouse: (cb) => window.__TAURI__.event.listen('mouse-global', (e) => cb(e.payload)),
  onOutsideClick: (cb) => window.__TAURI__.event.listen('mouse-outside-click', () => cb()),
  onBallHoverClear: (cb) => window.__TAURI__.event.listen('ball-hover-clear', () => cb())
};

const btn = document.getElementById('floatButton');
const miniClose = document.getElementById('miniClose');
let ballHovered = false;
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

// 悬浮球 hover 状态：由全局鼠标位置驱动，不依赖 WebView 的 :hover。
// Windows 原生右键菜单会打断 WebView 的 hover 状态，菜单关闭后 :hover 可能残留。
function setBallHovered(on) {
  on = !!on;
  if (ballHovered === on) return;
  ballHovered = on;
  btn.classList.toggle('hovered', on);
}

function updateBallHover(mx, my, wx, wy) {
  const dpr = window.devicePixelRatio || 1;
  const x = (mx - wx) / dpr;
  const y = (my - wy) / dpr;
  const rect = btn.getBoundingClientRect();
  const closeRect = miniClose.getBoundingClientRect();
  const inBall = x >= rect.left && x < rect.right && y >= rect.top && y < rect.bottom;
  // 关闭按钮在主体外侧，鼠标移到 × 上时仍要算作悬浮，否则按钮会在点击前消失。
  const inClose = x >= closeRect.left && x < closeRect.right
    && y >= closeRect.top && y < closeRect.bottom;
  setBallHovered(inBall || inClose);
}

// 系统文件选择框 / 资源管理器是独立窗口，里面任何一次点击都会被全局轮询判定为
// 「点到球外面」，从而收起文档气泡。这两个标记用来在这类操作期间屏蔽该判定。
let docsPickerOpen = false;
let docsSuppressUntil = 0;

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
  // 同一时刻只留一个气泡：文档面板开着时先让它退场，消息结束后再原样放回。
  // 三者共用 bottom:100px / right:22px，且窗口尺寸各按自己那套给（224 / 320 / 224 宽），
  // 直接叠着显示就会互相压住、外层还被裁窄。
  if (docs.open || face.open || wm.open || fs.open || shred.open) {
    if (docs.open) bubbleHide(docsBubble);
    if (face.open) bubbleHide(faceBubble);
    if (wm.open) bubbleHide(wmBubble);
    if (fs.open) bubbleHide(fsBubble);
    if (shred.open) bubbleHide(shredBubble);
    await new Promise((r) => setTimeout(r, BUBBLE_HIDE_MS));
  }
  const box = measureMsg(msg);
  // 区域始终右锚定：气泡右边距 22，宽度跟着文字走，球的位置不受影响。
  await resizeNow(Math.max(COLLAPSED_SIZE[0], box.width + 44), Math.round(box.height + 112));
  // 等窗口尺寸真正落屏后再播放入场，避免气泡在窗口仍处于收缩尺寸时开始淡入而被裁切
  requestAnimationFrame(() => bubbleShow(msgBubble));
  msgTimer = setTimeout(() => {
    bubbleHide(msgBubble);
    if (docs.open) {
      // 面板还开着（例如转换完成时弹的提示）：等消息退场后把窗口和面板交还给它
      setTimeout(() => { if (docs.open) docsLayout(); }, BUBBLE_HIDE_MS);
      return;
    }
    if (face.open) {
      setTimeout(() => { if (face.open) faceLayout(); }, BUBBLE_HIDE_MS);
      return;
    }
    if (wm.open) {
      setTimeout(() => { if (wm.open) wmLayout(); }, BUBBLE_HIDE_MS);
      return;
    }
    if (fs.open) {
      setTimeout(() => { if (fs.open) fsLayout(); }, BUBBLE_HIDE_MS);
      return;
    }
    if (shred.open) {
      setTimeout(() => { if (shred.open) shredLayout(); }, BUBBLE_HIDE_MS);
      return;
    }
    resizeLater(COLLAPSED_SIZE[0], COLLAPSED_SIZE[1]);
  }, duration);
}

// ===== 展开 / 收起（主体始终显示，气泡在主体上方弹出） =====
const COLLAPSED_SIZE = [112, 112];
const EXPANDED_SIZE = [264, 232];

// 主窗口的设计尺寸（CSS 像素，与 tauri.conf.json、lib.rs 的 MAIN_W / MAIN_H 一致）。
// 所有气泡都是按这个视口宽度排的固定 px 宽度，视口被压小就会溢出被裁，
// 所以启动时要用它把窗口物理尺寸校准回来（见文件末尾 fitWindowToDesign）。
const DESIGN_W = 420;
const DESIGN_H = 620;

// 提示气泡按文字长度自适应宽度：短消息（如“临时文件夹已清理”）不再撑成一条长条。
const MSG_PAD_X = 28;        // bubble 左右内边距 14 + 14
const MSG_MIN_W = 96;        // 再短也不窄于这个宽度
const MSG_MAX_W = 224;       // 与 .bubble 一致，超过就换行
const MSG_LINE_H = 34;       // 单行气泡高度（.msg-bubble min-height）
let msgMeasureCtx = null;

function measureMsg(msg) {
  if (!msgMeasureCtx) msgMeasureCtx = document.createElement('canvas').getContext('2d');
  // 用字号 + 字体族拼 canvas font（整条 font 简写里带行高时 canvas 可能解析失败）
  const style = getComputedStyle(msgBubble);
  msgMeasureCtx.font = `${style.fontSize} ${style.fontFamily}`;
  const textWidth = msgMeasureCtx.measureText(msg).width;
  // 换行按内容宽度算（border-box 的 224 里还含左右各 14 的内边距）
  const lines = Math.max(1, Math.ceil(textWidth / (MSG_MAX_W - MSG_PAD_X)));
  const width = Math.min(MSG_MAX_W, Math.max(MSG_MIN_W, Math.ceil(textWidth) + MSG_PAD_X + 2));
  return { width, lines, height: MSG_LINE_H + (lines - 1) * 16 };
}

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
  // 文档面板 / 人脸比对气泡与输入气泡互斥：先把面板收起再展开，否则两块气泡会叠在同一位置
  if (docs.open || face.open || wm.open || fs.open || shred.open) {
    if (docs.open) hideDocsPanel();
    if (face.open) hideFacePanel();
    if (wm.open) hideWmPanel();
    if (fs.open) hideFsPanel();
    if (shred.open) hideShredPanel();
    // 等面板退场（200ms）再缩窗口，否则 320 宽的面板会在淡出途中被 264 的窗口裁掉左边一截
    await new Promise((r) => setTimeout(r, BUBBLE_HIDE_MS));
  }
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

// 可交互元素（输入框/按钮/气泡）不参与拖动
function isInteractive(el) {
  return !!el.closest('input, button, .mini-close, #bubble, #docsBubble, #faceBubble, #wmBubble, #fsBubble, #shredBubble, #msgBubble');
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
      // 左键点球：打开上次右键菜单里用过的工具（文档复制 / 文档工具）。
      // 还没用过任何工具时保持原行为——把已展开的气泡收起。
      const closePanels = () => {
        if (docs.open) hideDocsPanel();
        if (face.open) hideFacePanel();
        if (wm.open) hideWmPanel();
        if (fs.open) hideFsPanel();
        if (shred.open) hideShredPanel();
        if (expanded) collapse();
      };
      window.api.openLastTool()
        .then((opened) => { if (!opened) closePanels(); })
        .catch(closePanels);
    } else if (!e.target.closest('#bubble') && !e.target.closest('#docsBubble')
      && !e.target.closest('#faceBubble') && !e.target.closest('#wmBubble')
      && !e.target.closest('#fsBubble') && !e.target.closest('#shredBubble')
      && !e.target.closest('#msgBubble')) {
      // 点击窗口内空白（气泡以外）→ 收起气泡与文档面板
      if (expanded) collapse();
      if (docs.open) hideDocsPanel();
      if (face.open) hideFacePanel();
      if (wm.open) hideWmPanel();
      if (fs.open) hideFsPanel();
      if (shred.open) hideShredPanel();
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

// 气泡面板内右键屏蔽：不弹浏览器默认菜单，也不打开工具菜单；悬浮球主体的右键菜单不受影响
document.addEventListener('contextmenu', (e) => {
  if (e.target.closest('.bubble')) {
    e.preventDefault();
  }
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

// Esc 收起输入气泡 / 文档面板 / 人脸比对气泡
document.addEventListener('keydown', (e) => {
  if (e.key !== 'Escape') return;
  if (expanded) collapse();
  if (docs.open) hideDocsPanel();
  if (face.open) hideFacePanel();
  if (wm.open) hideWmPanel();
  if (fs.open) hideFsPanel();
  if (shred.open) hideShredPanel();
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
  updateBallHover(mx, my, wx, wy);
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
    setStatus(T('input.needUrl'), 'err');
    return;
  }
  if (!/^https?:\/\//i.test(url)) {
    setStatus(T('input.badScheme'), 'err');
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
    progressText.textContent = T('input.downloading', { pct, mb, total: totalMb });
  } else {
    progressFill.style.width = '80%';
    const mb = (received / 1024 / 1024).toFixed(1);
    progressText.textContent = T('input.downloaded', { mb, name: filename });
  }
});

async function doDownload(url) {
  downloading = true;
  progressWrap.classList.remove('hidden');
  progressFill.style.width = '0%';
  progressText.textContent = T('input.connecting');
  setStatus('');

  const result = await window.api.startDownload(url);
  downloading = false;

  if (result.ok) {
    progressFill.style.width = '100%';
    progressText.textContent = T('input.done');
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
          showMsg(T('input.okCopied'));
        }
      }, 1000);
    } else {
      setStatus(T('input.okNoClip'), 'ok');
      showMood('10'); // 开心
      setTimeout(() => {
        if (!downloading) {
          bubbleHide(bubble);
          expanded = false;
          urlInput.value = '';
          progressWrap.classList.add('hidden');
          statusText.textContent = '';
          statusText.className = 'status-text';
          showMsg(T('input.okPathText'));
        }
      }, 1000);
    }
  } else {
    progressWrap.classList.add('hidden');
    setStatus('✗ ' + result.error, 'err');
    showMood('12'); // 失落（复制失败）
  }
}

// 原生右键菜单关闭后，主进程会通知一次：菜单可能让 WebView 丢失 hover 状态
window.api.onBallHoverClear(() => setBallHovered(false));

// ===== 主进程触发的事件 =====
window.api.onActivateInput(() => expand());
window.api.onToast((msg) => showMsg(msg));
// 点击主窗口以外的屏幕任意位置（气泡以外）→ 收起输入气泡与文档面板
window.api.onOutsideClick(() => {
  // 点到窗口外的桌面/其它程序：立即清掉悬浮态，不等下一次鼠标轮询
  setBallHovered(false);
  // 正在系统文件选择框里操作：那不是「点空白收起」，不能收面板
  if (docsPickerOpen || Date.now() < docsSuppressUntil) return;
  if (facePickerOpen || Date.now() < faceSuppressUntil) return;
  if (wmPickerOpen || Date.now() < wmSuppressUntil) return;
  if (fsPickerOpen || Date.now() < fsSuppressUntil) return;
  if (expanded) collapse();
  if (docs.open) hideDocsPanel();
  if (face.open) hideFacePanel();
  if (wm.open) hideWmPanel();
  if (fs.open) hideFsPanel();
  if (shred.open) hideShredPanel();
});

// 所有声明就绪后启动空闲睡觉计时（2 分钟无点击触发睡觉）
resetIdleTimer();

// ===== 文档工具：完全在悬浮球内完成（拖入 → 转换/压缩 → 进度 → 完成） =====
// 不再打开独立窗口：后端只把文件入队并发 docs-staged，气泡与按钮都在球边。

const invoke = window.__TAURI__.core.invoke;

// ===== 语言（默认中文，设置页切换后立即推送） =====
const T = (key, vars) => window.I18N.t(key, vars);

function applyLang(lang) {
  const next = lang === 'en' ? 'en' : 'zh';
  if (window.I18N.lang === next) return;
  window.I18N.setLang(next);
  if (docs.open) { renderDocs(); docsLayout(); }
  if (face.open) { renderFace(); faceLayout(); }
  if (wm.open) { renderWm(); wmLayout(); }
  if (fs.open) { renderFs(); fsLayout(); }
  if (shred.open) { renderShred(); shredLayout(); }
}

async function loadLangFromConfig() {
  let lang = 'zh';
  try {
    const cfg = await invoke('config_get');
    if (cfg && cfg.lang) lang = cfg.lang;
  } catch (err) {
    console.error('读取语言配置失败', err);
  }
  window.I18N.setLang(lang);
  if (docs.open) { renderDocs(); docsLayout(); }
  if (face.open) { renderFace(); faceLayout(); }
  if (wm.open) { renderWm(); wmLayout(); }
  if (fs.open) { renderFs(); fsLayout(); }
  if (shred.open) { renderShred(); shredLayout(); }
}

try {
  window.__TAURI__.event.listen('config-changed', (e) => {
    applyLang(e.payload && e.payload.lang);
  });
} catch (err) {
  console.error('语言切换监听注册失败', err);
}

const docsBubble = document.getElementById('docsBubble');
const docsTitle = document.getElementById('docsTitle');
const docsClose = document.getElementById('docsClose');
const docsFiles = document.getElementById('docsFiles');
const docsModes = document.getElementById('docsModes');
const docsModeConvert = document.getElementById('docsModeConvert');
const docsModeCompress = document.getElementById('docsModeCompress');
const docsTargets = document.getElementById('docsTargets');
const docsLevels = document.getElementById('docsLevels');
const docsHint = document.getElementById('docsHint');
const docsActions = document.getElementById('docsActions');
const docsPick = document.getElementById('docsPick');
const docsStart = document.getElementById('docsStart');
const docsRun = document.getElementById('docsRun');
const docsRunName = document.getElementById('docsRunName');
const docsRunFill = document.getElementById('docsRunFill');
const docsRunLabel = document.getElementById('docsRunLabel');
const docsRunPct = document.getElementById('docsRunPct');
const docsCancel = document.getElementById('docsCancel');
const docsDone = document.getElementById('docsDone');
const docsDoneMsg = document.getElementById('docsDoneMsg');
const docsResults = document.getElementById('docsResults');
const docsOpenDir = document.getElementById('docsOpenDir');
const docsAgain = document.getElementById('docsAgain');
const docsBack = document.getElementById('docsBack');
const ballProgress = document.getElementById('ballProgress');
const ringArc = document.getElementById('ringArc');
const ballPctPill = document.getElementById('ballPctPill');
const ballPct = document.getElementById('ballPct');

const DOCS_W = 360;
const DOCS_MAX_H = 508;      // 主窗口 620，扣掉气泡 100px 底部偏移与顶部留白
const RING_LEN = 289;        // 2πr，r=46

const KIND_LABEL = {
  image: 'kind.image',
  word: 'kind.word',
  sheet: 'kind.sheet',
  slides: 'kind.slides',
  pdf: 'kind.pdf',
  text: 'kind.text',
  markdown: 'kind.markdown',
  html: 'kind.html',
  csv: 'kind.csv',
  rtf: 'kind.rtf',
  odf: 'kind.odf',
  epub: 'kind.epub',
  other: 'kind.other',
};

function kindLabel(kind) {
  const key = KIND_LABEL[kind];
  if (!key) return kind || '';
  const label = T(key);
  return label === key ? kind : label;
}

const docs = {
  open: false,
  stage: 'choose',   // choose | options | running | done
  files: [],
  mode: null,        // convert | compress
  level: 'normal',
  target: null,
  outputs: [],
  running: false,
  batch: [],         // 正在处理的那一批，跑完后从中摘掉、清出列表
};

let docsEntering = false;    // 连续拖入时避免重复播放入场
let docsQuietTimer = 0;      // 进度静默计时器：超时就把进度条切成「不确定进度」

// ---------- 面板尺寸：按内容高度自适应 ----------
async function docsLayout() {
  clearTimeout(docsBubble._hideTimer);
  docsBubble.classList.remove('closing', 'hidden');
  const h = Math.min(DOCS_MAX_H, Math.max(160, docsBubble.offsetHeight));
  await resizeNow(DOCS_W, Math.round(h + 112));
  requestAnimationFrame(() => {
    if (docs.open) bubbleShow(docsBubble);
  });
}

/// 工具气泡互斥：打开新的工具前，先把其它还开着的气泡收起来，避免在同一位置叠住
async function hideOtherToolBubbles(except) {
  let any = false;
  // 粘贴链接输入气泡同样占这个位置，先收起
  if (expanded) { collapse(); any = true; }
  if (except !== 'docs' && docs.open) { hideDocsPanel(); any = true; }
  if (except !== 'face' && face.open) { hideFacePanel(); any = true; }
  if (except !== 'wm' && wm.open) { hideWmPanel(); any = true; }
  if (except !== 'fs' && fs.open) { hideFsPanel(); any = true; }
  if (except !== 'shred' && shred.open) { hideShredPanel(); any = true; }
  // 消息提示气泡也占同一个位置，一并退场
  if (!msgBubble.classList.contains('hidden')) { bubbleHide(msgBubble); any = true; }
  // 等淡出动画走完再让新面板入场，否则两块气泡会短暂压在一起
  if (any) await new Promise((r) => setTimeout(r, BUBBLE_HIDE_MS));
}

// 连续切换工具时只让最后一次打开生效，避免上一个 async 打开流程在等待后重新弹出
let toolOpenSeq = 0;

async function openDocsPanel() {
  if (expanded) collapse();
  const seq = ++toolOpenSeq;
  await hideOtherToolBubbles('docs');
  if (seq !== toolOpenSeq) return;
  if (!msgBubble.classList.contains('hidden')) bubbleHide(msgBubble);
  docs.open = true;
  // 完成页 / 运行中的进度页再打开时保持原页面：面板因失焦收起后，
  // 再点悬浮球应该回到刚才看到的那一页，而不是退回首页。
  if (docs.stage !== 'done' && docs.stage !== 'running') {
    docs.stage = 'choose';
    docs.mode = null;
    docs.target = null;
  }
  renderDocs();
  // 面板里有待处理文件时保持「检索资料」，收起面板或开跑后由其他表情接手
  if (docs.files.length && !docs.running) {
    showMood('40', true);
    clearTimeout(idleTimer);
    clearTimeout(drowsyTimer);
  }
  await docsLayout();
}

function hideDocsPanel() {
  if (!docs.open) return;
  docs.open = false;
  bubbleHide(docsBubble);
  // 任务仍在跑时保留球上的进度环，所以窗口不能缩到比进度环更小，
  // 进度环+百分比胶囊都在 112×112 的收起区域里，可直接收起。
  resizeLater(COLLAPSED_SIZE[0], COLLAPSED_SIZE[1]);
  if (!docs.running) {
    clearMood();
    resetIdleTimer();
  }
}

// ---------- 文件 ----------
function mergeDocs(files) {
  files.forEach((f) => {
    if (!docs.files.some((x) => x.path === f.path)) docs.files.push(f);
  });
}

function renderDocsFiles() {
  docsFiles.innerHTML = '';
  if (!docs.files.length) {
    const empty = document.createElement('div');
    empty.className = 'docs-empty';
    empty.textContent = T('docs.empty');
    docsFiles.appendChild(empty);
    return;
  }
  docs.files.forEach((f, idx) => {
    const row = document.createElement('div');
    row.className = 'docs-file-row';

    const name = document.createElement('span');
    name.className = 'name';
    name.textContent = f.name;
    name.title = `${f.path} · ${kindLabel(f.kind) || f.kind}`;

    const size = document.createElement('span');
    size.className = 'size';
    size.textContent = f.sizeLabel;

    const remove = document.createElement('button');
    remove.className = 'remove';
    remove.textContent = '×';
    remove.title = T('docs.remove');
    remove.addEventListener('click', (e) => {
      e.stopPropagation();
      if (docs.running) return;
      docs.files.splice(idx, 1);
      docs.target = null;
      renderDocs();
      docsLayout();
    });

    row.append(name, size, remove);
    docsFiles.appendChild(row);
  });
}

// 多文件时取目标格式交集；没有交集退回并集（逐文件提示）
function docsTargetsFor(files) {
  const per = files.filter((f) => f.targets && f.targets.length).map((f) => f.targets);
  if (!per.length) return [];
  const inter = per.reduce((acc, list) => acc.filter((t) => list.includes(t)));
  if (inter.length) return inter;
  return Array.from(new Set(per.flat()));
}

function docsCompressible() {
  return docs.files.filter((f) => f.canCompress);
}

function renderDocsTargets() {
  docsTargets.innerHTML = '';
  const targets = docsTargetsFor(docs.files);
  if (!targets.length) {
    docs.target = null;
    return;
  }
  targets.forEach((t) => {
    const chip = document.createElement('button');
    chip.className = 'chip' + (docs.target === t ? ' active' : '');
    chip.textContent = t.toUpperCase();
    chip.addEventListener('click', (e) => {
      e.stopPropagation();
      docs.target = t;
      Array.from(docsTargets.children).forEach((c) => c.classList.remove('active'));
      chip.classList.add('active');
      syncDocsControls();
    });
    docsTargets.appendChild(chip);
  });
}

function syncDocsControls() {
  const hasFiles = docs.files.length > 0;
  docsPick.textContent = hasFiles ? T('docs.pickMore') : T('docs.pick');

  // 提示行在 index.html 里是可选元素（当前被注释掉了）：没有它也要照常工作
  const setDocsHint = (text) => { if (docsHint) docsHint.textContent = text; };

  if (docs.stage === 'options' && docs.mode === 'convert') {
    const targets = docsTargetsFor(docs.files);
    docsStart.disabled = !(hasFiles && targets.length && docs.target);
    if (!hasFiles) {
      setDocsHint(T('docs.hintAddFirst'));
    } else if (!targets.length) {
      const first = docs.files.find((f) => !f.targets || !f.targets.length);
      setDocsHint(first
        ? T('docs.hintNoTargetOne', { name: first.name })
        : T('docs.hintNoTarget'));
    } else {
      setDocsHint(docs.target
        ? T('docs.hintWillConvert', { fmt: docs.target.toUpperCase() })
        : T('docs.hintPickTarget'));
    }
  } else if (docs.stage === 'options' && docs.mode === 'compress') {
    const ok = docsCompressible();
    docsStart.disabled = !(hasFiles && ok.length);
    if (!hasFiles) {
      setDocsHint(T('docs.hintAddFirst'));
    } else if (!ok.length) {
      const first = docs.files[0];
      setDocsHint(first ? first.compressHint : T('docs.hintNoCompress'));
    } else {
      const skip = docs.files.length - ok.length;
      const levelName = docs.level === 'small'
        ? T('docs.levelSmall')
        : docs.level === 'high' ? T('docs.levelHigh') : T('docs.levelNormal');
      setDocsHint(skip > 0
        ? T('docs.hintCompressSkip', { ok: ok.length, skip })
        : T('docs.hintCompressLevel', { level: levelName }));
    }
  } else {
    // 还没选「转换 / 压缩」时不能开跑：光有文件不足以确定要做什么
    docsStart.disabled = !hasFiles || !docs.mode;
    setDocsHint(hasFiles ? T('docs.hintPickMode') : T('docs.empty'));
  }
}

function renderDocs() {
  renderDocsFiles();

  // 面板是两级的：首页（选动作 / 选文件 / 进度）和完成页。完成页只留结果，
  // 首页元素全部收起，靠标题栏的返回按钮回到首页。
  const onDonePage = docs.stage === 'done';
  const showChoices = docs.stage === 'choose' || docs.stage === 'options';
  // 运行中只保留进度面板（文件名由 docsRunName 展示），避免文件列表与进度区重复显示文件名
  docsFiles.classList.toggle('hidden', docs.stage === 'running' || onDonePage);
  docsBack.classList.toggle('hidden', !onDonePage);
  docsModes.classList.toggle('hidden', !showChoices);
  docsTargets.classList.toggle('hidden', !(docs.stage === 'options' && docs.mode === 'convert'));
  docsLevels.classList.toggle('hidden', !(docs.stage === 'options' && docs.mode === 'compress'));
  if (docsHint) docsHint.classList.toggle('hidden', !showChoices);
  docsActions.classList.toggle('hidden', !showChoices);
  docsRun.classList.toggle('hidden', docs.stage !== 'running');
  docsDone.classList.toggle('hidden', docs.stage !== 'done');

  docsModeConvert.classList.toggle('active', docs.mode === 'convert');
  docsModeCompress.classList.toggle('active', docs.mode === 'compress');

  if (docs.stage === 'options' && docs.mode === 'convert') renderDocsTargets();

  if (showChoices) syncDocsControls();

  docsTitle.textContent = docs.files.length
    ? T(docs.files.length === 1 ? 'docs.titleCountOne' : 'docs.titleCount', { n: docs.files.length })
    : T('docs.title');
}

// 完成页 → 首页：丢掉这一批的结果与动作选择，回到「选转换 / 选压缩」
function docsBackToHome() {
  if (docs.running) return;
  docs.stage = 'choose';
  docs.outputs = [];
  docs.batch = [];
  docs.mode = null;
  docs.target = null;
  docs.level = 'normal';
  Array.from(docsLevels.children).forEach((c) => {
    c.classList.toggle('active', c.dataset.level === 'normal');
  });
  renderDocs();
  docsLayout();
}

// ---------- 交互 ----------
function selectDocsMode(mode) {
  if (docs.running) return;
  docs.mode = mode;
  if (mode === 'convert') {
    const targets = docsTargetsFor(docs.files);
    docs.target = targets.length ? (docs.target && targets.includes(docs.target) ? docs.target : null) : null;
  }
  docs.stage = 'options';
  renderDocs();
  docsLayout();
}

docsModeConvert.addEventListener('click', (e) => { e.stopPropagation(); selectDocsMode('convert'); });
docsModeCompress.addEventListener('click', (e) => { e.stopPropagation(); selectDocsMode('compress'); });

docsLevels.addEventListener('click', (e) => {
  const chip = e.target.closest('.chip');
  if (!chip) return;
  docs.level = chip.dataset.level;
  Array.from(docsLevels.children).forEach((c) => c.classList.remove('active'));
  chip.classList.add('active');
  syncDocsControls();
});

async function loadPendingDocs() {
  try {
    const paths = await invoke('docs_take_pending');
    if (!Array.isArray(paths) || !paths.length) return 0;
    const info = await invoke('docs_inspect', { paths });
    const added = (info || []).filter((f) => !docs.files.some((x) => x.path === f.path)).length;
    mergeDocs(info || []);
    return added;
  } catch (err) {
    console.error('读取待处理文件失败', err);
    return 0;
  }
}

async function pickDocs() {
  // 选择框是独立窗口：期间所有点击都不该被当作「点空白收起」
  docsPickerOpen = true;
  try {
    const paths = await invoke('docs_pick_files');
    if (!Array.isArray(paths) || !paths.length) {
      // 取消了也要保证面板还在（极端情况下可能已被别的路径收起）
      await hideOtherToolBubbles('docs');
      docs.open = true;
      renderDocs();
      await docsLayout();
      return;
    }
    await invoke('docs_stage', { paths });
    await loadPendingDocs();
    await hideOtherToolBubbles('docs');
    docs.open = true;
    docs.stage = docs.mode ? 'options' : 'choose';
    renderDocs();
    await docsLayout();
  } catch (err) {
    if (docsHint) docsHint.textContent = T('docs.pickFailed', { err });
    await hideOtherToolBubbles('docs');
    docs.open = true;
    renderDocs();
    await docsLayout();
  } finally {
    docsPickerOpen = false;
  }
}

function showRing(percent) {
  ballProgress.classList.remove('hidden');
  ballPctPill.classList.remove('hidden');
  updateRing(percent);
}

function updateRing(percent) {
  const pct = Math.max(0, Math.min(100, Number(percent) || 0));
  const offset = RING_LEN * (1 - pct / 100);
  ringArc.style.strokeDashoffset = String(offset);
  ballPct.textContent = `${Math.round(pct)}%`;
}

function hideRing() {
  ballProgress.classList.add('hidden');
  ballPctPill.classList.add('hidden');
  updateRing(0);
}

// 某些链路没有中间进度（PDF→DOCX 在 sidecar 里一次跑完，只在结束时出结果），
// 静默超过 1.2s 就把进度条改成不确定动画，不要一直停在 0% 让人以为卡死。
function armDocsQuiet() {
  clearTimeout(docsQuietTimer);
  docsQuietTimer = setTimeout(() => {
    if (!docs.running) return;
    docsRunFill.classList.add('is-indeterminate');
    docsRunPct.textContent = T('docs.working');
  }, 1200);
}

function disarmDocsQuiet() {
  clearTimeout(docsQuietTimer);
  docsQuietTimer = 0;
  docsRunFill.classList.remove('is-indeterminate');
}

async function startDocsJob() {
  if (!docs.files.length || docs.running || !docs.mode) return;
  if (docs.mode === 'convert' && !docs.target) return;
  docs.running = true;
  docs.stage = 'running';
  docs.outputs = [];
  // 记下这一批的文件，跑完后只把它们从列表里摘掉——
  // 跑的过程中新拖进来的文件要留在面板里，等下一批处理。
  docs.batch = docs.files.map((f) => f.path);
  docsRunName.textContent = docs.files[0] ? docs.files[0].name : '';
  docsRunLabel.textContent = T('docs.fileCount', { n: docs.files.length });
  docsRunPct.textContent = '0%';
  docsRunFill.style.width = '0%';
  disarmDocsQuiet();
  armDocsQuiet();
  renderDocs();
  await docsLayout();
  showRing(0);

  try {
    await invoke('docs_start', {
      job: {
        paths: docs.files.map((f) => f.path),
        mode: docs.mode,
        target: docs.target,
        level: docs.level,
      },
    });
  } catch (err) {
    docs.running = false;
    hideRing();
    onDocsDone({ ok: false, message: String(err), outdir: '', outputs: [] });
  }
}

function onDocsProgress(payload) {
  if (!payload || !docs.running) return;
  const pct = Math.max(0, Math.min(100, Number(payload.percent) || 0));
  disarmDocsQuiet();
  docsRunFill.style.width = `${pct}%`;
  docsRunPct.textContent = `${Math.round(pct)}%`;
  armDocsQuiet();
  if (payload.name) docsRunName.textContent = payload.name;
  if (payload.label) {
    docsRunLabel.textContent = payload.total
      ? `${payload.index + 1}/${payload.total} · ${payload.label}`
      : payload.label;
  }
  updateRing(pct);
}

function onDocsDone(payload) {
  docs.running = false;
  disarmDocsQuiet();
  docs.outputs = (payload && payload.outputs) || [];
  hideRing();
  // 这一批已经结束：把它们从已上传列表里摘掉，再次打开面板从「选择文件」开始，
  // 不会把上一批文件带回来。结果仍留在 docs.outputs 里给完成页展示。
  docs.files = docs.files.filter((f) => !(docs.batch || []).includes(f.path));
  docs.batch = [];

  // 先把完成页内容准备好：即使此刻面板是收起的，下次打开也应该直接是完成页，
  // 而不是空壳或还停在进度页。
  docs.stage = 'done';
  docsDoneMsg.textContent = T(payload && payload.ok ? 'docs.successStatus' : 'docs.failedToast');
  docsDoneMsg.className = 'docs-done-msg' + (payload && payload.ok ? '' : ' err');
  docsResults.innerHTML = '';
  docs.outputs.forEach((item) => {
    const row = document.createElement('div');
    row.className = 'docs-result-row';

    const name = document.createElement('span');
    name.className = 'name';
    name.textContent = item.name;
    name.title = item.path;

    const delta = document.createElement('span');
    delta.className = 'delta';
    delta.textContent = T('docs.successStatus');

    row.append(name, delta);
    docsResults.appendChild(row);
  });
  docsOpenDir.disabled = !(payload && payload.ok);

  // 用户中途收了气泡：只在球边提示一句，不强行弹回面板
  if (!docs.open) {
    if (payload && payload.ok) {
      showMsg(T('docs.doneToast', { n: docs.outputs.length }), 2600);
    } else {
      showMsg(T('docs.failedToast'), 2600);
    }
    return;
  }

  renderDocs();
  docsLayout();
}

docsPick.addEventListener('click', (e) => { e.stopPropagation(); pickDocs(); });
docsStart.addEventListener('click', (e) => { e.stopPropagation(); startDocsJob(); });
docsCancel.addEventListener('click', (e) => {
  e.stopPropagation();
  invoke('docs_cancel').catch(() => {});
  docsRunLabel.textContent = T('docs.canceling');
});
docsOpenDir.addEventListener('click', (e) => {
  e.stopPropagation();
  // 资源管理器是独立窗口，点进去的瞬间别把面板收掉
  docsSuppressUntil = Date.now() + 1500;
  invoke('docs_open_outdir', { paths: (docs.outputs || []).map((o) => o.path) }).catch(() => {});
});
docsBack.addEventListener('click', (e) => {
  e.stopPropagation();
  docsBackToHome();
});
docsAgain.addEventListener('click', (e) => {
  e.stopPropagation();
  // 这一批已经清空，再打开应该是首页重新选文件
  docs.stage = 'choose';
  docs.files = [];
  docs.outputs = [];
  docs.mode = null;
  docs.target = null;
  docs.level = 'normal';
  Array.from(docsLevels.children).forEach((c) => {
    c.classList.toggle('active', c.dataset.level === 'normal');
  });
  hideDocsPanel();
});
docsClose.addEventListener('click', (e) => {
  e.stopPropagation();
  if (docs.running) {
    invoke('docs_cancel').catch(() => {});
  }
  hideDocsPanel();
});

// 后端事件：拖入 / 进度 / 完成
try {
  if (!window.__TAURI__ || !window.__TAURI__.event) throw new Error('__TAURI__.event 不存在');
  const ev = window.__TAURI__.event;
  ev.listen('tauri://drag-enter', () => { enterAccept(); });
  ev.listen('tauri://drag-over', () => { enterAccept(); });
  ev.listen('tauri://drag-leave', () => { leaveAccept(); });
  ev.listen('tauri://drag-drop', (e) => {
    // 入队与事件通知由后端（drop_target）完成，这里只结束拖入态。
    // 真有文件落下时保持「检索资料」，交给紧接着弹出的文档气泡继续显示。
    const paths = (e.payload && e.payload.paths) || [];
    leaveAccept(paths.length > 0);
  });
  ev.listen('docs-staged', async () => {
    // 这个事件既用于「拖入文件」也用于「重新打开面板」，只有真的带进来新文件时才重置页面：
    // 否则从菜单/左键再打开时会从完成页被踢回首页。
    const staged = await loadPendingDocs();
    if (!docs.running && staged > 0) {
      if (docs.stage === 'done') {
        // 停在完成页时又拖进来文件：按「回首页」处理，重新选动作
        docs.stage = 'choose';
        docs.mode = null;
        docs.target = null;
        docs.outputs = [];
      } else {
        // 已经选过动作就留在选项页，只把新文件补进列表
        docs.stage = docs.mode ? 'options' : 'choose';
        if (docs.mode !== 'convert') docs.target = null;
      }
    }
    renderDocs();
    if (!docs.open) {
      openDocsPanel();
    } else {
      docsLayout();
    }
  });
  ev.listen('docs-progress', (e) => onDocsProgress(e.payload));
  ev.listen('docs-done', (e) => onDocsDone(e.payload));
} catch (err) {
  console.error('拖拽/文档事件监听注册失败', err);
}

// ===== 人脸比对：两组照片（证件照 / 自拍）跨组一一对应 → Word（每页一对） =====
// 与文档工具同一套气泡范式：选图 → 运行进度 → 完成，全程在球边完成。
// 推理放在随包 Python sidecar（模型首次使用才下载），这里只管交互与进度。

const faceBubble = document.getElementById('faceBubble');
const faceTitle = document.getElementById('faceTitle');
const faceBack = document.getElementById('faceBack');
const faceClose = document.getElementById('faceClose');
const faceGroups = document.getElementById('faceGroups');
const faceListA = document.getElementById('faceListA');
const faceListB = document.getElementById('faceListB');
const faceCountA = document.getElementById('faceCountA');
const faceCountB = document.getElementById('faceCountB');
const facePickA = document.getElementById('facePickA');
const facePickB = document.getElementById('facePickB');
const faceHint = document.getElementById('faceHint');
const faceActions = document.getElementById('faceActions');
const faceStart = document.getElementById('faceStart');
const faceRun = document.getElementById('faceRun');
const faceRunName = document.getElementById('faceRunName');
const faceRunFill = document.getElementById('faceRunFill');
const faceRunLabel = document.getElementById('faceRunLabel');
const faceRunPct = document.getElementById('faceRunPct');
const faceCancel = document.getElementById('faceCancel');
const faceDone = document.getElementById('faceDone');
const faceDoneMsg = document.getElementById('faceDoneMsg');
const faceResults = document.getElementById('faceResults');
const faceOpenDir = document.getElementById('faceOpenDir');
const faceAgain = document.getElementById('faceAgain');

const FACE_W = 380;              // 气泡 340 + 左右各 20，与文档面板同一套边距
const FACE_MAX_H = 508;          // 主窗口 620，扣掉气泡 100px 底部偏移与顶部留白

const face = {
  open: false,
  stage: 'choose',   // choose | running | done
  a: [],             // 第一组（签证照）：[{ path, name, sizeLabel }]
  b: [],             // 第二组（自拍照）
  running: false,
  outputs: [],
  needModel: false,  // 本地还没缓存模型：面板上提前说明会下载
  pasteTarget: 'a',  // Ctrl+V 粘贴到哪一组：a / b
};

let facePickerOpen = false;      // 系统选图框开着：期间任何点击都不是「点空白收起」
let faceSuppressUntil = 0;       // 资源管理器等独立窗口刚打开，短暂屏蔽外部点击
let faceQuietTimer = 0;

async function faceLayout() {
  clearTimeout(faceBubble._hideTimer);
  faceBubble.classList.remove('closing', 'hidden');
  const h = Math.min(FACE_MAX_H, Math.max(160, faceBubble.offsetHeight));
  await resizeNow(FACE_W, Math.round(h + 112));
  requestAnimationFrame(() => {
    if (face.open) bubbleShow(faceBubble);
  });
}

async function refreshFaceModelState() {
  try {
    const info = await invoke('face_status');
    // [ready, cacheRoot]
    face.needModel = !(Array.isArray(info) ? info[0] : info);
  } catch (err) {
    face.needModel = false;
  }
}

async function openFacePanel() {
  if (expanded) collapse();
  const seq = ++toolOpenSeq;
  // 与文档工具 / 文件查找 / 水印气泡互斥：同一位置只能有一块气泡
  await hideOtherToolBubbles('face');
  if (seq !== toolOpenSeq) return;
  if (!msgBubble.classList.contains('hidden')) bubbleHide(msgBubble);
  face.open = true;
  if (face.stage !== 'done' && face.stage !== 'running') face.stage = 'choose';
  await refreshFaceModelState();
  renderFace();
  await faceLayout();
}

function hideFacePanel() {
  if (!face.open) return;
  face.open = false;
  bubbleHide(faceBubble);
  // 任务仍在跑时球上还挂着进度环，收起到 112×112 刚好装得下
  resizeLater(COLLAPSED_SIZE[0], COLLAPSED_SIZE[1]);
}

function renderFaceList(el, items, which) {
  el.innerHTML = '';
  if (!items.length) {
    const empty = document.createElement('div');
    empty.className = 'face-list-empty';
    empty.textContent = T('face.groupEmpty');
    el.appendChild(empty);
    return;
  }
  items.forEach((f, idx) => {
    const row = document.createElement('div');
    row.className = 'docs-file-row';

    const name = document.createElement('span');
    name.className = 'name';
    name.textContent = f.name;
    name.title = f.path;

    const size = document.createElement('span');
    size.className = 'size';
    size.textContent = f.sizeLabel;

    const remove = document.createElement('button');
    remove.className = 'remove';
    remove.textContent = '×';
    remove.title = T('docs.remove');
    remove.addEventListener('click', (e) => {
      e.stopPropagation();
      if (face.running) return;
      items.splice(idx, 1);
      renderFace();
      faceLayout();
    });

    row.append(name, size, remove);
    el.appendChild(row);
  });
}

function renderFace() {
  const onDonePage = face.stage === 'done';
  const showChoices = face.stage === 'choose';

  renderFaceList(faceListA, face.a, 'a');
  renderFaceList(faceListB, face.b, 'b');
  faceCountA.textContent = String(face.a.length);
  faceCountB.textContent = String(face.b.length);

  faceGroups.classList.toggle('hidden', !showChoices);
  faceHint.classList.toggle('hidden', !showChoices);
  faceActions.classList.toggle('hidden', !showChoices);
  faceRun.classList.toggle('hidden', face.stage !== 'running');
  faceDone.classList.toggle('hidden', !onDonePage);
  faceBack.classList.toggle('hidden', !onDonePage);

  faceStart.disabled = face.running || !face.a.length || !face.b.length;
  faceTitle.textContent = T('face.title');
  Array.from(faceGroups.querySelectorAll('.face-group')).forEach((group) => {
    group.classList.toggle('active', group.dataset.group === face.pasteTarget);
  });

  if (showChoices) {
    if (face.needModel) {
      faceHint.textContent = T('face.hintNeedModel');
    } else if (!face.a.length && !face.b.length) {
      faceHint.textContent = T('face.hintEmpty');
    } else if (!face.a.length || !face.b.length) {
      faceHint.textContent = T('face.hintOneGroup');
    } else {
      faceHint.textContent = T('face.hintReady', { a: face.a.length, b: face.b.length });
    }
  }
}

async function pickFaceGroup(which) {
  face.pasteTarget = which;
  facePickerOpen = true;
  try {
    const picked = await invoke('face_pick_group');
    if (!Array.isArray(picked) || !picked.length) {
      // 取消选择也要保证面板还在
      await hideOtherToolBubbles('face');
      face.open = true;
      renderFace();
      await faceLayout();
      return;
    }
    const list = which === 'a' ? face.a : face.b;
    let added = 0;
    picked.forEach((f) => {
      if (!f || !f.ok) return;                 // 选了非图片：静默忽略
      if (list.some((x) => x.path === f.path)) return;
      list.push({ path: f.path, name: f.name, sizeLabel: f.sizeLabel });
      added += 1;
    });
    await hideOtherToolBubbles('face');
    face.open = true;
    face.stage = 'choose';
    renderFace();
    if (!added && picked.length) {
      faceHint.textContent = T('face.notImage');
    }
    await faceLayout();
  } catch (err) {
    await hideOtherToolBubbles('face');
    face.open = true;
    faceHint.textContent = T('face.pickFailed', { err });
    faceHint.classList.remove('hidden');
    renderFace();
    await faceLayout();
  } finally {
    facePickerOpen = false;
  }
}

// 点击第一组 / 第二组盒子，决定下一次 Ctrl+V 粘贴到哪一组。
Array.from(faceGroups.querySelectorAll('.face-group')).forEach((group) => {
  group.addEventListener('click', () => {
    const which = group.dataset.group === 'b' ? 'b' : 'a';
    if (face.pasteTarget === which) return;
    face.pasteTarget = which;
    renderFace();
  });
});

function armFaceQuiet() {
  clearTimeout(faceQuietTimer);
  faceQuietTimer = setTimeout(() => {
    if (!face.running) return;
    faceRunFill.classList.add('is-indeterminate');
    faceRunPct.textContent = T('docs.working');
  }, 1200);
}

function disarmFaceQuiet() {
  clearTimeout(faceQuietTimer);
  faceQuietTimer = 0;
  faceRunFill.classList.remove('is-indeterminate');
}

async function startFaceJob() {
  if (face.running || !face.a.length || !face.b.length) return;
  face.running = true;
  face.stage = 'running';
  face.outputs = [];
  faceRunName.textContent = '';
  faceRunLabel.textContent = T('face.runStart');
  faceRunPct.textContent = '0%';
  faceRunFill.style.width = '0%';
  disarmFaceQuiet();
  armFaceQuiet();
  renderFace();
  await faceLayout();
  showRing(0);

  try {
    await invoke('face_start', {
      job: {
        groupA: face.a.map((f) => f.path),
        groupB: face.b.map((f) => f.path),
      },
    });
  } catch (err) {
    face.running = false;
    hideRing();
    onFaceDone({ ok: false, message: String(err), outdir: '', outputs: [] });
  }
}

function onFaceProgress(payload) {
  if (!payload || !face.running) return;
  const pct = Math.max(0, Math.min(100, Number(payload.percent) || 0));
  disarmFaceQuiet();
  faceRunFill.style.width = `${pct}%`;
  faceRunPct.textContent = `${Math.round(pct)}%`;
  armFaceQuiet();
  if (payload.name) faceRunName.textContent = payload.name;
  if (payload.label) faceRunLabel.textContent = payload.label;
  updateRing(pct);
}

function onFaceDone(payload) {
  const ok = !!(payload && payload.ok);
  face.running = false;
  disarmFaceQuiet();
  hideRing();
  face.outputs = (payload && payload.outputs) || [];
  face.stage = 'done';

  faceDoneMsg.textContent = (payload && payload.message) || T(ok ? 'docs.successStatus' : 'face.failed');
  faceDoneMsg.className = 'docs-done-msg' + (ok ? '' : ' err');

  faceResults.innerHTML = '';
  face.outputs.forEach((item) => {
    const row = document.createElement('div');
    row.className = 'docs-result-row';

    const name = document.createElement('span');
    name.className = 'name';
    name.textContent = item.name;
    name.title = item.path;

    const note = document.createElement('span');
    note.className = 'delta';
    note.textContent = item.note || '';

    row.append(name, note);
    faceResults.appendChild(row);
  });
  faceOpenDir.disabled = !ok;

  // 成功：这批照片已经用掉，下一位客户从空列表开始；失败保留列表，方便原地重试
  if (ok) {
    face.a = [];
    face.b = [];
    refreshFaceModelState();
  }

  // 用户中途把气泡收了：只在球边提示一句，不强行弹回面板
  if (!face.open) {
    const pairs = (payload && payload.summary && payload.summary.pairs) || 0;
    showMsg(ok ? T('face.doneToast', { n: pairs }) : T('face.failedToast'), 2600);
    return;
  }
  renderFace();
  faceLayout();
}

facePickA.addEventListener('click', (e) => { e.stopPropagation(); pickFaceGroup('a'); });
facePickB.addEventListener('click', (e) => { e.stopPropagation(); pickFaceGroup('b'); });
faceStart.addEventListener('click', (e) => { e.stopPropagation(); startFaceJob(); });
faceCancel.addEventListener('click', (e) => {
  e.stopPropagation();
  invoke('face_cancel').catch(() => {});
  faceRunLabel.textContent = T('face.canceling');
});
faceOpenDir.addEventListener('click', (e) => {
  e.stopPropagation();
  // 资源管理器是独立窗口，点进去的瞬间别把面板收掉
  faceSuppressUntil = Date.now() + 1500;
  invoke('face_open_outdir', { paths: (face.outputs || []).map((o) => o.path) }).catch(() => {});
});
faceBack.addEventListener('click', (e) => {
  e.stopPropagation();
  if (face.running) return;
  face.stage = 'choose';
  face.outputs = [];
  refreshFaceModelState();
  renderFace();
  faceLayout();
});
faceAgain.addEventListener('click', (e) => {
  e.stopPropagation();
  face.stage = 'choose';
  face.a = [];
  face.b = [];
  face.outputs = [];
  hideFacePanel();
});
faceClose.addEventListener('click', (e) => {
  e.stopPropagation();
  if (face.running) invoke('face_cancel').catch(() => {});
  hideFacePanel();
});

try {
  window.__TAURI__.event.listen('face-open', () => { openFacePanel(); });
  window.__TAURI__.event.listen('face-progress', (e) => onFaceProgress(e.payload));
  window.__TAURI__.event.listen('face-done', (e) => onFaceDone(e.payload));
} catch (err) {
  console.error('人脸比对事件监听注册失败', err);
}

// ===== 图片水印去除：选图 → 画笔 / 框选 mask → 模型修复 =====
const wmBubble = document.getElementById('wmBubble');
const wmTitle = document.getElementById('wmTitle');
const wmBack = document.getElementById('wmBack');
const wmClose = document.getElementById('wmClose');
const wmPickRow = document.getElementById('wmPickRow');
const wmFile = document.getElementById('wmFile');
const wmEditor = document.getElementById('wmEditor');
const wmBrush = document.getElementById('wmBrush');
const wmRect = document.getElementById('wmRect');
const wmRadius = document.getElementById('wmRadius');
const wmUndo = document.getElementById('wmUndo');
const wmClear = document.getElementById('wmClear');
const wmCanvasWrap = document.getElementById('wmCanvasWrap');
const wmImgCanvas = document.getElementById('wmImgCanvas');
const wmMarkCanvas = document.getElementById('wmMarkCanvas');
const wmHint = document.getElementById('wmHint');
const wmActions = document.getElementById('wmActions');
const wmStart = document.getElementById('wmStart');
const wmRun = document.getElementById('wmRun');
const wmRunName = document.getElementById('wmRunName');
const wmRunFill = document.getElementById('wmRunFill');
const wmRunLabel = document.getElementById('wmRunLabel');
const wmRunPct = document.getElementById('wmRunPct');
const wmCancel = document.getElementById('wmCancel');
const wmDone = document.getElementById('wmDone');
const wmDoneMsg = document.getElementById('wmDoneMsg');
const wmResults = document.getElementById('wmResults');
const wmOpenDir = document.getElementById('wmOpenDir');
const wmAgain = document.getElementById('wmAgain');

const WM_W = 420;  // 窗口宽；气泡 380 + 左右边距 40，与文档/人脸同一套边距
const WM_MAX_H = 508;

const wm = {
  open: false,
  stage: 'choose', // choose | running | done
  image: null,     // { path, name, sizeLabel, dataUrl, origW, origH, w, h }
  strokes: [],     // 缩略图坐标：[{ kind, points, radius, rect }]
  running: false,
  outputs: [],
  needModel: false,
  tool: null,       // null = 拖图；'brush' / 'rect' = 标记
  brushRadius: 12,
  drawing: false,
  current: null,    // 正在画的一笔 / 框选预览
  panning: false,
  panStart: null,
  panX: 0,          // 图片相对容器的平移（CSS px）
  panY: 0,
  zoom: 1,          // 相对基准尺寸的缩放倍数
  baseW: 0,         // contain 适配容器后的基准显示宽（CSS px）
  baseH: 0,         // 基准显示高（CSS px）
};

let wmPickerOpen = false;
let wmSuppressUntil = 0;
let wmQuietTimer = 0;

async function wmLayout() {
  clearTimeout(wmBubble._hideTimer);
  wmBubble.classList.remove('closing', 'hidden');
  const h = Math.min(WM_MAX_H, Math.max(160, wmBubble.offsetHeight));
  await resizeNow(WM_W, Math.round(h + 112));
  requestAnimationFrame(() => {
    if (wm.open) bubbleShow(wmBubble);
  });
}

async function refreshWmModelState() {
  try {
    const info = await invoke('wm_status');
    wm.needModel = !(Array.isArray(info) ? info[0] : info);
  } catch (err) {
    wm.needModel = false;
  }
}

async function openWmPanel(paths) {
  if (expanded) collapse();
  const seq = ++toolOpenSeq;
  // 与文档工具 / 文件查找 / 人脸比对气泡互斥
  await hideOtherToolBubbles('wm');
  if (seq !== toolOpenSeq) return;
  if (!msgBubble.classList.contains('hidden')) bubbleHide(msgBubble);
  wm.open = true;
  if (wm.stage !== 'done' && wm.stage !== 'running') wm.stage = 'choose';
  await refreshWmModelState();
  renderWm();
  await wmLayout();
  // 拖放图片打开：路径随事件带过来，气泡布置完成后再载入，避免首帧尺寸算错。
  if (Array.isArray(paths) && paths.length) {
    await loadWmPaths(paths);
    renderWm();
    await wmLayout();
  }
}

function hideWmPanel() {
  if (!wm.open) return;
  wm.open = false;
  bubbleHide(wmBubble);
  resizeLater(COLLAPSED_SIZE[0], COLLAPSED_SIZE[1]);
}

function wmCanvasPoint(e) {
  const rect = wmMarkCanvas.getBoundingClientRect();
  const x = ((e.clientX - rect.left) / rect.width) * wmMarkCanvas.width;
  const y = ((e.clientY - rect.top) / rect.height) * wmMarkCanvas.height;
  return [Math.round(x), Math.round(y)];
}

function clampWmPan() {
  const cw = Math.max(1, wmCanvasWrap.clientWidth);
  const ch = Math.max(1, wmCanvasWrap.clientHeight);
  const dw = wm.baseW * wm.zoom;
  const dh = wm.baseH * wm.zoom;
  const maxX = Math.max(0, (dw - cw) / 2);
  const maxY = Math.max(0, (dh - ch) / 2);
  wm.panX = Math.max(-maxX, Math.min(maxX, wm.panX));
  wm.panY = Math.max(-maxY, Math.min(maxY, wm.panY));
}

function applyWmZoom() {
  if (!wm.baseW || !wm.baseH) return;
  clampWmPan();
  const w = wm.baseW * wm.zoom;
  const h = wm.baseH * wm.zoom;
  const transform = `translate(-50%, -50%) translate(${wm.panX}px, ${wm.panY}px)`;
  wmImgCanvas.style.width = w + 'px';
  wmImgCanvas.style.height = h + 'px';
  wmImgCanvas.style.transform = transform;
  wmMarkCanvas.style.width = w + 'px';
  wmMarkCanvas.style.height = h + 'px';
  wmMarkCanvas.style.transform = transform;
  updateWmCursor();
}

function updateWmCursor() {
  if (wm.panning) {
    wmMarkCanvas.style.cursor = 'grabbing';
  } else if (!wm.tool) {
    wmMarkCanvas.style.cursor = wm.zoom > 1 ? 'grab' : 'default';
  } else {
    wmMarkCanvas.style.cursor = 'crosshair';
  }
}

// 光标停留在画布区域内：滚轮放大 / 缩小图片，范围 0.4 ~ 4 倍
wmCanvasWrap.addEventListener('wheel', (e) => {
  if (!wm.image) return;
  e.preventDefault();
  const next = wm.zoom * (e.deltaY < 0 ? 1.12 : 1 / 1.12);
  wm.zoom = Math.min(4, Math.max(0.4, next));
  applyWmZoom();
}, { passive: false });

function redrawMark() {
  const ctx = wmMarkCanvas.getContext('2d');
  ctx.clearRect(0, 0, wmMarkCanvas.width, wmMarkCanvas.height);
  ctx.lineCap = 'round';
  ctx.lineJoin = 'round';
  const drawStroke = (s) => {
    if (s.kind === 'brush') {
      ctx.strokeStyle = 'rgba(217, 83, 79, 0.55)';
      ctx.fillStyle = 'rgba(217, 83, 79, 0.55)';
      ctx.lineWidth = s.radius;
      const pts = s.points;
      if (pts.length === 1) {
        ctx.beginPath();
        ctx.arc(pts[0][0], pts[0][1], s.radius / 2, 0, Math.PI * 2);
        ctx.fill();
      } else {
        ctx.beginPath();
        ctx.moveTo(pts[0][0], pts[0][1]);
        for (let i = 1; i < pts.length; i++) ctx.lineTo(pts[i][0], pts[i][1]);
        ctx.stroke();
      }
    } else if (s.kind === 'rect' && s.rect.length === 4) {
      const [x1, y1, x2, y2] = s.rect;
      ctx.fillStyle = 'rgba(217, 83, 79, 0.30)';
      ctx.strokeStyle = 'rgba(217, 83, 79, 0.85)';
      ctx.lineWidth = 1.5;
      ctx.fillRect(x1, y1, x2 - x1, y2 - y1);
      ctx.strokeRect(x1, y1, x2 - x1, y2 - y1);
    }
  };
  wm.strokes.forEach(drawStroke);
  if (wm.current) drawStroke(wm.current);
}

function loadWmImage(info) {
  wm.image = info;
  wm.strokes = [];
  wm.current = null;
  wm.tool = null;
  wm.drawing = false;
  wm.panning = false;
  wm.panStart = null;
  wm.panX = 0;
  wm.panY = 0;
  if (!info || !info.dataUrl || !info.w || !info.h) {
    console.error('wm thumbnail invalid', info);
    wmHint.textContent = T('wm.notImage');
    return;
  }
  const img = new Image();
  img.onload = () => {
    wmImgCanvas.width = info.w;
    wmImgCanvas.height = info.h;
    wmMarkCanvas.width = info.w;
    wmMarkCanvas.height = info.h;
    const ictx = wmImgCanvas.getContext('2d');
    ictx.drawImage(img, 0, 0, info.w, info.h);
    // 按容器可用空间 contain 出基准显示尺寸，再由缩放倍数叠加
    const availW = Math.max(80, wmCanvasWrap.clientWidth - 4);
    const availH = Math.max(80, wmCanvasWrap.clientHeight - 4);
    const factor = Math.min(availW / info.w, availH / info.h, 1);
    wm.baseW = info.w * factor;
    wm.baseH = info.h * factor;
    wm.zoom = 1;
    applyWmZoom();
    redrawMark();
  };
  img.onerror = () => {
    console.error('wm thumbnail decode failed');
    wmHint.textContent = T('wm.notImage');
  };
  img.src = info.dataUrl;
}

function renderWm() {
  const onDone = wm.stage === 'done';
  const showChoices = wm.stage === 'choose';

  wmPickRow.classList.toggle('hidden', !showChoices);
  wmEditor.classList.toggle('hidden', !showChoices || !wm.image);
  wmHint.classList.toggle('hidden', !showChoices);
  wmActions.classList.toggle('hidden', !showChoices);
  wmRun.classList.toggle('hidden', wm.stage !== 'running');
  wmDone.classList.toggle('hidden', !onDone);
  wmBack.classList.toggle('hidden', !onDone);

  wmBrush.classList.toggle('active', wm.tool === 'brush');
  wmRect.classList.toggle('active', wm.tool === 'rect');
  wmRadius.disabled = wm.tool !== 'brush';
  updateWmCursor();

  wmFile.textContent = wm.image ? wm.image.name : '';
  wmStart.disabled = wm.running || !wm.image || wm.strokes.length === 0;
  wmTitle.textContent = T('wm.title');

  if (showChoices) {
    if (wm.needModel) {
      wmHint.textContent = T('wm.hintNeedModel');
    } else if (!wm.image) {
      wmHint.textContent = T('wm.hintEmpty');
    } else if (!wm.tool) {
      wmHint.textContent = T('wm.hintDrag');
    } else {
      wmHint.textContent = T('wm.hintReady');
    }
  }
}

async function pickWmImage() {
  wmPickerOpen = true;
  try {
    const picked = await invoke('wm_pick');
    if (!Array.isArray(picked) || !picked.length) {
      await hideOtherToolBubbles('wm');
      wm.open = true;
      renderWm();
      await wmLayout();
      return;
    }
    const item = picked.find((f) => f && f.ok);
    if (!item) {
      wmHint.textContent = T('wm.notImage');
      return;
    }
    await useWmItem(item);
    await hideOtherToolBubbles('wm');
    wm.open = true;
    renderWm();
    await wmLayout();
  } catch (err) {
    await hideOtherToolBubbles('wm');
    wm.open = true;
    wmHint.textContent = T('wm.pickFailed', { err });
    renderWm();
    await wmLayout();
  } finally {
    wmPickerOpen = false;
  }
}

// 把一张已校验的图片装进编辑器：选图、Ctrl+V、拖放共用。
async function useWmItem(item) {
  const info = await invoke('wm_thumbnail', { path: item.path });
  wm.outputs = [];
  wm.image = {
    path: item.path,
    name: item.name,
    sizeLabel: item.sizeLabel,
    dataUrl: info.dataUrl,
    origW: info.origW,
    origH: info.origH,
    w: info.w,
    h: info.h,
  };
  loadWmImage(wm.image);
  wm.stage = 'choose';
}

// 从路径列表里挑第一张有效图片载入；走 wm_inspect 统一过滤不存在 / 非图片文件。
async function loadWmPaths(paths) {
  if (wm.running) return false;
  try {
    const info = await invoke('wm_inspect', { paths });
    const item = (info || []).find((f) => f && f.ok);
    if (!item) {
      wmHint.textContent = T('paste.noImage');
      return false;
    }
    await useWmItem(item);
    return true;
  } catch (err) {
    wmHint.textContent = T('wm.pickFailed', { err });
    return false;
  }
}

function armWmQuiet() {
  clearTimeout(wmQuietTimer);
  wmQuietTimer = setTimeout(() => {
    if (!wm.running) return;
    wmRunFill.classList.add('is-indeterminate');
    wmRunPct.textContent = T('docs.working');
  }, 1200);
}

function disarmWmQuiet() {
  clearTimeout(wmQuietTimer);
  wmQuietTimer = 0;
  wmRunFill.classList.remove('is-indeterminate');
}

function submitStrokes() {
  if (!wm.image) return [];
  const sx = wm.image.origW / wm.image.w;
  const sy = wm.image.origH / wm.image.h;
  return wm.strokes.map((s) => {
    if (s.kind === 'brush') {
      return {
        kind: 'brush',
        radius: Math.max(1, Math.round(s.radius * sx)),
        points: s.points.map((p) => [Math.round(p[0] * sx), Math.round(p[1] * sy)]),
      };
    }
    const [x1, y1, x2, y2] = s.rect;
    return {
      kind: 'rect',
      radius: 0,
      rect: [Math.round(x1 * sx), Math.round(y1 * sy), Math.round(x2 * sx), Math.round(y2 * sy)],
      points: [],
    };
  });
}

async function startWmJob() {
  if (wm.running || !wm.image || wm.strokes.length === 0) return;
  wm.running = true;
  wm.stage = 'running';
  wm.outputs = [];
  wmRunName.textContent = '';
  wmRunLabel.textContent = T('wm.runStart');
  wmRunPct.textContent = '0%';
  wmRunFill.style.width = '0%';
  disarmWmQuiet();
  armWmQuiet();
  renderWm();
  await wmLayout();
  showRing(0);

  try {
    await invoke('wm_start', {
      job: { image: wm.image.path, strokes: submitStrokes() },
    });
  } catch (err) {
    wm.running = false;
    hideRing();
    onWmDone({ ok: false, message: String(err), outdir: '', outputs: [] });
  }
}

function onWmProgress(payload) {
  if (!payload || !wm.running) return;
  const pct = Math.max(0, Math.min(100, Number(payload.percent) || 0));
  disarmWmQuiet();
  wmRunFill.style.width = `${pct}%`;
  wmRunPct.textContent = `${Math.round(pct)}%`;
  armWmQuiet();
  if (payload.name) wmRunName.textContent = payload.name;
  if (payload.label) wmRunLabel.textContent = payload.label;
  updateRing(pct);
}

function onWmDone(payload) {
  const ok = !!(payload && payload.ok);
  wm.running = false;
  disarmWmQuiet();
  hideRing();
  wm.outputs = (payload && payload.outputs) || [];
  wm.stage = 'done';

  wmDoneMsg.textContent = (payload && payload.message) || T(ok ? 'docs.successStatus' : 'wm.failed');
  wmDoneMsg.className = 'docs-done-msg' + (ok ? '' : ' err');

  wmResults.innerHTML = '';
  wm.outputs.forEach((item) => {
    const row = document.createElement('div');
    row.className = 'docs-result-row';
    const name = document.createElement('span');
    name.className = 'name';
    name.textContent = item.name;
    name.title = item.path;
    const note = document.createElement('span');
    note.className = 'delta';
    note.textContent = item.note || '';
    row.append(name, note);
    wmResults.appendChild(row);
  });
  wmOpenDir.disabled = !ok;

  if (ok) {
    wm.image = null;
    wm.strokes = [];
    refreshWmModelState();
  }

  if (!wm.open) {
    showMsg(ok ? T('wm.doneToast') : T('wm.failedToast'), 2600);
    return;
  }
  renderWm();
  wmLayout();
}

// 画布指针交互：未选工具时拖图；选中画笔/框选后才开始标记
wmMarkCanvas.addEventListener('pointerdown', (e) => {
  if (!wm.image || wm.running) return;
  e.preventDefault();
  wmMarkCanvas.setPointerCapture(e.pointerId);
  if (!wm.tool) {
    wm.panning = true;
    wm.panStart = { x: e.clientX, y: e.clientY, panX: wm.panX, panY: wm.panY };
    updateWmCursor();
    return;
  }
  const [x, y] = wmCanvasPoint(e);
  if (wm.tool === 'brush') {
    wm.drawing = true;
    wm.current = { kind: 'brush', points: [[x, y]], radius: wm.brushRadius };
  } else {
    wm.drawing = true;
    wm.current = { kind: 'rect', points: [], radius: 0, rect: [x, y, x, y] };
  }
  redrawMark();
});

wmMarkCanvas.addEventListener('pointermove', (e) => {
  if (wm.panning && wm.panStart) {
    wm.panX = wm.panStart.panX + (e.clientX - wm.panStart.x);
    wm.panY = wm.panStart.panY + (e.clientY - wm.panStart.y);
    applyWmZoom();
    return;
  }
  if (!wm.drawing || !wm.current) return;
  const [x, y] = wmCanvasPoint(e);
  if (wm.current.kind === 'brush') {
    wm.current.points.push([x, y]);
  } else {
    wm.current.rect[2] = x;
    wm.current.rect[3] = y;
  }
  redrawMark();
});

function endWmStroke() {
  if (wm.panning) {
    wm.panning = false;
    wm.panStart = null;
    updateWmCursor();
    return;
  }
  if (!wm.drawing || !wm.current) return;
  if (wm.current.kind === 'brush') {
    if (wm.current.points.length > 0) wm.strokes.push(wm.current);
  } else {
    const [x1, y1, x2, y2] = wm.current.rect;
    if (Math.abs(x2 - x1) > 2 || Math.abs(y2 - y1) > 2) {
      wm.current.rect = [Math.min(x1, x2), Math.min(y1, y2), Math.max(x1, x2), Math.max(y1, y2)];
      wm.strokes.push(wm.current);
    }
  }
  wm.drawing = false;
  wm.current = null;
  redrawMark();
  renderWm();
}

wmMarkCanvas.addEventListener('pointerup', endWmStroke);
wmMarkCanvas.addEventListener('pointercancel', endWmStroke);

wmBrush.addEventListener('click', (e) => {
  e.stopPropagation();
  wm.tool = wm.tool === 'brush' ? null : 'brush';
  wm.drawing = false;
  wm.current = null;
  wm.panning = false;
  wm.panStart = null;
  redrawMark();
  renderWm();
});
wmRect.addEventListener('click', (e) => {
  e.stopPropagation();
  wm.tool = wm.tool === 'rect' ? null : 'rect';
  wm.drawing = false;
  wm.current = null;
  wm.panning = false;
  wm.panStart = null;
  redrawMark();
  renderWm();
});
wmRadius.addEventListener('input', () => {
  wm.brushRadius = Number(wmRadius.value) || 12;
});
wmUndo.addEventListener('click', (e) => {
  e.stopPropagation();
  if (wm.running) return;
  wm.strokes.pop();
  redrawMark();
  renderWm();
});
wmClear.addEventListener('click', (e) => {
  e.stopPropagation();
  if (wm.running) return;
  wm.strokes = [];
  redrawMark();
  renderWm();
});
wmPickRow.querySelector('#wmPick').addEventListener('click', (e) => {
  e.stopPropagation();
  pickWmImage();
});
wmStart.addEventListener('click', (e) => {
  e.stopPropagation();
  startWmJob();
});
wmCancel.addEventListener('click', (e) => {
  e.stopPropagation();
  invoke('wm_cancel').catch(() => {});
  wmRunLabel.textContent = T('wm.canceling');
});
wmOpenDir.addEventListener('click', (e) => {
  e.stopPropagation();
  wmSuppressUntil = Date.now() + 1500;
  invoke('wm_open_outdir', { paths: (wm.outputs || []).map((o) => o.path) }).catch(() => {});
});
wmBack.addEventListener('click', (e) => {
  e.stopPropagation();
  if (wm.running) return;
  wm.stage = 'choose';
  wm.outputs = [];
  refreshWmModelState();
  renderWm();
  wmLayout();
});
wmAgain.addEventListener('click', (e) => {
  e.stopPropagation();
  wm.stage = 'choose';
  wm.image = null;
  wm.strokes = [];
  wm.outputs = [];
  hideWmPanel();
});
wmClose.addEventListener('click', (e) => {
  e.stopPropagation();
  if (wm.running) invoke('wm_cancel').catch(() => {});
  hideWmPanel();
});

try {
  window.__TAURI__.event.listen('wm-open', (e) => {
    openWmPanel(e && e.payload ? e.payload.paths : null);
  });
  window.__TAURI__.event.listen('wm-progress', (e) => onWmProgress(e.payload));
  window.__TAURI__.event.listen('wm-done', (e) => onWmDone(e.payload));
} catch (err) {
  console.error('水印去除事件监听注册失败', err);
}

// ===== 气泡内 Ctrl+V：文档 / 人脸 / 水印共用后端剪贴板读取 =====
let clipboardPasting = false;

function setPasteError(target, message) {
  if (target === 'docs') {
    if (docsHint) {
      docsHint.textContent = message;
      docsHint.classList.remove('hidden');
    }
  } else if (target === 'face') {
    faceHint.textContent = message;
    faceHint.classList.remove('hidden');
  } else if (target === 'wm') {
    wmHint.textContent = message;
    wmHint.classList.remove('hidden');
  }
}

async function pasteDocsPaths(paths) {
  const info = await invoke('docs_inspect', { paths });
  const valid = (info || []).filter((f) => f && f.path);
  const before = docs.files.length;
  mergeDocs(valid);
  const added = docs.files.length - before;
  if (!added) {
    setPasteError('docs', T('paste.noValid'));
    return;
  }
  if (!docs.running) {
    docs.outputs = [];
    docs.stage = docs.mode ? 'options' : 'choose';
  }
  renderDocs();
  await docsLayout();
}

async function pasteFacePaths(paths) {
  if (face.running) {
    setPasteError('face', T('paste.busy'));
    return;
  }
  const info = await invoke('face_inspect', { paths });
  const valid = (info || []).filter((f) => f && f.ok);
  const which = face.pasteTarget === 'b' ? 'b' : 'a';
  const list = which === 'b' ? face.b : face.a;
  let added = 0;
  valid.forEach((f) => {
    if (list.some((x) => x.path === f.path)) return;
    list.push({ path: f.path, name: f.name, sizeLabel: f.sizeLabel });
    added += 1;
  });
  face.outputs = [];
  face.stage = 'choose';
  renderFace();
  if (!added) {
    setPasteError('face', T('paste.noImage'));
  }
  await faceLayout();
}

async function pasteWmPaths(paths) {
  if (wm.running) {
    setPasteError('wm', T('paste.busy'));
    return;
  }
  const loaded = await loadWmPaths(paths);
  renderWm();
  if (!loaded) {
    setPasteError('wm', T('paste.noImage'));
  }
  await wmLayout();
}

async function pasteClipboard(target) {
  if (clipboardPasting) return;
  clipboardPasting = true;
  try {
    const payload = await invoke('clipboard_read');
    const paths = payload && Array.isArray(payload.files) ? payload.files : [];
    if (!paths.length) {
      setPasteError(target, T('paste.empty'));
      return;
    }
    if (target === 'docs') {
      await pasteDocsPaths(paths);
    } else if (target === 'face') {
      await pasteFacePaths(paths);
    } else if (target === 'wm') {
      await pasteWmPaths(paths);
    }
  } catch (err) {
    setPasteError(target, T('paste.failed', { err }));
  } finally {
    clipboardPasting = false;
  }
}

// 只在三个工具气泡之一打开时接管 Ctrl+V；不影响其它输入场景。
document.addEventListener('keydown', (e) => {
  if (!(e.ctrlKey || e.metaKey) || e.repeat) return;
  if (String(e.key || '').toLowerCase() !== 'v') return;
  const target = docs.open ? 'docs' : face.open ? 'face' : wm.open ? 'wm' : null;
  if (!target) return;
  e.preventDefault();
  e.stopPropagation();
  pasteClipboard(target);
});

// ===== 文件查找：按文件名模糊匹配；来源文件夹留空 = 只查本机 =====
// 扫描在 Rust 侧后台线程里跑，按批把结果推回来；这里负责输入、来源、结果与打开/定位。

const fsBubble = document.getElementById('fsBubble');
const fsClose = document.getElementById('fsClose');
const fsQuery = document.getElementById('fsQuery');
const fsNetPath = document.getElementById('fsNetPath');
const fsNetPick = document.getElementById('fsNetPick');
const fsHint = document.getElementById('fsHint');
const fsStat = document.getElementById('fsStat');
const fsStart = document.getElementById('fsStart');
const fsResults = document.getElementById('fsResults');

const FS_W = 400;         // 窗口宽：气泡 360 + 右侧箭头/边距
const FS_MAX_H = 508;     // 与其它面板一致：主窗口 620，扣掉 112 的底部偏移
const FS_SHOW_LIMIT = 300;

const fs = {
  open: false,
  running: false,
  done: false,
  canceled: false,
  truncated: false,
  results: [],      // [{ path, name, dir, sizeLabel, modified, score }]
  scanned: 0,
  hits: 0,
  elapsedMs: 0,
  errors: [],
  fail: '',          // 查找没能启动的原因（如共享盘不可达），直接显示原文
  lastH: 0,
};

let fsPickerOpen = false;      // 系统文件夹选择框开着：不要把它当成「点空白收起」
let fsSuppressUntil = 0;       // 刚打开的资源管理器窗口，短暂屏蔽外部点击
let fsRenderTimer = 0;

async function fsLayout() {
  clearTimeout(fsBubble._hideTimer);
  fsBubble.classList.remove('closing', 'hidden');
  const h = Math.min(FS_MAX_H, Math.max(160, fsBubble.offsetHeight));
  fs.lastH = h;
  await resizeNow(FS_W, Math.round(h + 112));
  requestAnimationFrame(() => {
    if (fs.open) bubbleShow(fsBubble);
  });
}

async function openFsPanel() {
  if (expanded) collapse();
  const seq = ++toolOpenSeq;
  // 与文档工具 / 人脸比对 / 水印气泡互斥
  await hideOtherToolBubbles('fs');
  if (seq !== toolOpenSeq) return;
  if (!msgBubble.classList.contains('hidden')) bubbleHide(msgBubble);
  fs.open = true;
  renderFs();
  await fsLayout();
  setTimeout(() => { if (fs.open) fsQuery.focus(); }, 80);
}

function hideFsPanel() {
  if (!fs.open) return;
  fs.open = false;
  bubbleHide(fsBubble);
  resizeLater(COLLAPSED_SIZE[0], COLLAPSED_SIZE[1]);
}

function fsElapsedSec() {
  return Math.max(0, Math.round(fs.elapsedMs / 1000));
}

function fsFormatTime(unixSec) {
  if (!unixSec) return '';
  try {
    const d = new Date(unixSec * 1000);
    const pad = (n) => String(n).padStart(2, '0');
    return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
  } catch (err) {
    return '';
  }
}

function fsNetworkPath() {
  return fsNetPath.value.trim();
}

/// 来源或关键词变了以后，旧结果不再对应，清掉避免误解
function fsResetResults() {
  fs.results = [];
  fs.done = false;
  fs.canceled = false;
  fs.truncated = false;
  fs.scanned = 0;
  fs.hits = 0;
  fs.elapsedMs = 0;
  fs.errors = [];
  fs.fail = '';
}

function fsHintText() {
  if (fs.fail) return fs.fail;
  const errNote = fs.errors.length ? ` · ${T('fs.errors', { n: fs.errors.length })}` : '';
  if (fs.running) return T('fs.searching');
  if (!fs.done) {
    if (fsNetworkPath()) return T('fs.hintNetwork');
    return T('fs.hintLocal');
  }
  const n = fs.results.length;
  if (fs.canceled) return (n ? T('fs.canceledFound', { n }) : T('fs.canceled')) + errNote;
  if (!n) return T('fs.empty') + errNote;
  return (fs.truncated ? T('fs.foundTrunc', { n }) : T('fs.found', { n })) + errNote;
}

function renderFs() {
  fsNetPath.disabled = fs.running;
  fsNetPick.disabled = fs.running;

  // 提示与进度
  fsHint.textContent = fsHintText();
  fsHint.title = fs.fail || fs.errors.join('\n');
  fsHint.classList.remove('hidden');
  if (fs.running) {
    fsStat.textContent = T('fs.progress', { n: fs.scanned, m: fs.hits });
  } else if (fs.done && !fs.fail) {
    fsStat.textContent = T('fs.summary', { n: fs.scanned, s: fsElapsedSec() });
  } else {
    fsStat.textContent = '';
  }

  // 操作按钮在「开始查找 / 停止」之间切换
  fsStart.textContent = fs.running ? T('fs.stop') : T('fs.start');
  fsStart.disabled = !fs.running && !fsQuery.value.trim();

  // 结果列表：得分高的在前，同分按名称排
  const scrollTop = fsResults.scrollTop;
  fsResults.innerHTML = '';
  if (!fs.results.length) {
    if (fs.done && !fs.running && !fs.fail) {
      const empty = document.createElement('div');
      empty.className = 'fs-empty';
      empty.textContent = T('fs.empty');
      fsResults.appendChild(empty);
    }
  } else {
    const list = fs.results
      .slice()
      .sort((a, b) => (b.score - a.score) || a.name.localeCompare(b.name));
    list.slice(0, FS_SHOW_LIMIT).forEach((item) => {
      const row = document.createElement('div');
      row.className = 'fs-result';
      row.title = item.path;
      row.addEventListener('click', (e) => {
        e.stopPropagation();
        fsReveal(item.path);
      });

      const main = document.createElement('div');
      main.className = 'fs-result-main';

      const name = document.createElement('span');
      name.className = 'fs-result-name';
      name.textContent = item.name;

      const meta = document.createElement('span');
      meta.className = 'fs-result-meta';
      const time = fsFormatTime(item.modified);
      meta.textContent = [item.dir, item.sizeLabel, time].filter(Boolean).join(' · ');

      main.append(name, meta);

      const actions = document.createElement('div');
      actions.className = 'fs-result-actions';

      const open = document.createElement('button');
      open.className = 'link-btn';
      open.textContent = T('fs.openAction');
      open.addEventListener('click', (e) => {
        e.stopPropagation();
        fsOpenFile(item.path);
      });

      const reveal = document.createElement('button');
      reveal.className = 'link-btn';
      reveal.textContent = T('fs.revealAction');
      reveal.title = T('fs.revealTitle');
      reveal.addEventListener('click', (e) => {
        e.stopPropagation();
        fsReveal(item.path);
      });

      actions.append(open, reveal);
      row.append(main, actions);
      fsResults.appendChild(row);
    });
  }
  fsResults.scrollTop = scrollTop;
}

function scheduleFsRender(forceLayout = false) {
  if (fsRenderTimer) return;
  fsRenderTimer = setTimeout(() => {
    fsRenderTimer = 0;
    if (!fs.open) return;
    renderFs();
    const h = Math.min(FS_MAX_H, Math.max(160, fsBubble.offsetHeight));
    if (forceLayout || Math.abs(h - fs.lastH) > 2) fsLayout();
  }, 150);
}

async function startFsSearch() {
  if (fs.running) return;
  const query = fsQuery.value.trim();
  if (!query) {
    fsHint.textContent = T('fs.hintNeedQuery');
    fsHint.classList.remove('hidden');
    fsQuery.focus();
    return;
  }
  const root = fsNetworkPath();
  fs.running = true;
  fs.done = false;
  fs.canceled = false;
  fs.truncated = false;
  fs.results = [];
  fs.scanned = 0;
  fs.hits = 0;
  fs.elapsedMs = 0;
  fs.errors = [];
  fs.fail = '';
  renderFs();
  await fsLayout();

  try {
    await invoke('fs_search', {
      req: {
        query,
        // 留空 = 只查本机磁盘；填了就只查这个文件夹（本机目录或网络共享）
        root: root || null,
        limit: FS_SHOW_LIMIT,
      },
    });
  } catch (err) {
    // 例如共享路径不可达：后端直接返回错误，面板回到可再次查找的状态
    fs.running = false;
    fs.done = true;
    fs.fail = T('fs.failed', { err: String(err) });
    renderFs();
    await fsLayout();
  }
}

async function stopFsSearch() {
  try {
    await invoke('fs_cancel');
  } catch (err) {
    console.error('停止查找失败', err);
  }
  fsHint.textContent = T('fs.canceling');
}

async function fsReveal(path) {
  // 资源管理器是独立窗口：点进去的瞬间别把面板收掉
  fsSuppressUntil = Date.now() + 1500;
  try {
    await invoke('fs_reveal', { path });
  } catch (err) {
    showMsg(T('fs.revealFailed', { err: String(err) }), 2600);
  }
}

async function fsOpenFile(path) {
  fsSuppressUntil = Date.now() + 1500;
  try {
    await invoke('fs_open', { path });
  } catch (err) {
    showMsg(T('fs.openFailed', { err: String(err) }), 2600);
  }
}

async function pickFsFolder() {
  fsPickerOpen = true;
  try {
    const picked = await invoke('fs_pick_folder');
    if (picked) {
      fsNetPath.value = picked;
      fsResetResults();
    } else {
      fs.fail = '';
    }
    // 文件选择框不是模态的，期间可能点开了别的工具气泡
    await hideOtherToolBubbles('fs');
    fs.open = true;
    renderFs();
    await fsLayout();
  } catch (err) {
    await hideOtherToolBubbles('fs');
    fs.open = true;
    fs.fail = T('fs.pickFailed', { err: String(err) });
    renderFs();
    await fsLayout();
  } finally {
    fsPickerOpen = false;
  }
}

function onFsBatch(payload) {
  if (!payload || !Array.isArray(payload.items)) return;
  let added = 0;
  payload.items.forEach((item) => {
    if (!item || !item.path) return;
    if (fs.results.some((x) => x.path === item.path)) return;
    fs.results.push(item);
    added += 1;
  });
  if (typeof payload.total === 'number') fs.hits = Math.max(fs.hits, payload.total);
  if (added && fs.open) scheduleFsRender(true);
}

function onFsProgress(payload) {
  if (!payload || !fs.running) return;
  fs.scanned = Number(payload.scanned) || 0;
  fs.hits = Number(payload.hits) || 0;
  fs.elapsedMs = Number(payload.elapsedMs) || 0;
  if (fs.open) fsStat.textContent = T('fs.progress', { n: fs.scanned, m: fs.hits });
}

function onFsDone(payload) {
  fs.running = false;
  fs.done = true;
  fs.canceled = !!(payload && payload.canceled);
  fs.truncated = !!(payload && payload.truncated);
  fs.errors = (payload && Array.isArray(payload.errors)) ? payload.errors : [];
  fs.scanned = Number(payload && payload.scanned) || fs.scanned;
  fs.elapsedMs = Number(payload && payload.elapsedMs) || fs.elapsedMs;

  // 用户中途收了气泡：只在球边提示结果数量，不强行弹回面板
  if (!fs.open) {
    if (fs.results.length) {
      showMsg(T('fs.doneToast', { n: fs.results.length }), 2600);
    } else {
      showMsg(T('fs.noResultToast'), 2600);
    }
    return;
  }

  renderFs();
  fsLayout();
}

fsStart.addEventListener('click', (e) => {
  e.stopPropagation();
  if (fs.running) stopFsSearch();
  else startFsSearch();
});

fsQuery.addEventListener('input', () => {
  if (fs.fail) {
    fs.fail = '';
    renderFs();
  }
  fsStart.disabled = !fs.running && !fsQuery.value.trim();
});

fsNetPath.addEventListener('input', () => {
  if (fs.running) return;
  // 换了来源文件夹，旧结果不再对应当前来源
  fsResetResults();
  scheduleFsRender(false);
});

fsQuery.addEventListener('keydown', (e) => {
  if (e.key !== 'Enter') return;
  e.preventDefault();
  if (!fs.running) startFsSearch();
});

fsNetPath.addEventListener('keydown', (e) => {
  if (e.key !== 'Enter') return;
  e.preventDefault();
  if (!fs.running) startFsSearch();
});

fsNetPick.addEventListener('click', (e) => {
  e.stopPropagation();
  pickFsFolder();
});

fsClose.addEventListener('click', (e) => {
  e.stopPropagation();
  hideFsPanel();
});

try {
  window.__TAURI__.event.listen('fs-open', () => { openFsPanel(); });
  window.__TAURI__.event.listen('fs-batch', (e) => onFsBatch(e.payload));
  window.__TAURI__.event.listen('fs-progress', (e) => onFsProgress(e.payload));
  window.__TAURI__.event.listen('fs-done', (e) => onFsDone(e.payload));
} catch (err) {
  console.error('文件查找事件监听注册失败', err);
}

// ===== 文件粉碎：资源管理器右键 → 悬浮球气泡确认 → 随机覆盖后删除 =====
// 右键菜单（HKCU 静态动词）只把路径带进来、永远不直接删；用户在气泡里点
// 「开始粉碎」才动手：逐个文件用随机数据覆盖再删除，被占用时结束占用进程重试。

const shredBubble = document.getElementById('shredBubble');
const shredTitle = document.getElementById('shredTitle');
const shredClose = document.getElementById('shredClose');
const shredList = document.getElementById('shredList');
const shredWarn = document.getElementById('shredWarn');
const shredOptRow = document.getElementById('shredOptRow');
const shredKillLocks = document.getElementById('shredKillLocks');
const shredActions = document.getElementById('shredActions');
const shredStat = document.getElementById('shredStat');
const shredStart = document.getElementById('shredStart');
const shredRun = document.getElementById('shredRun');
const shredRunName = document.getElementById('shredRunName');
const shredRunFill = document.getElementById('shredRunFill');
const shredRunLabel = document.getElementById('shredRunLabel');
const shredRunPct = document.getElementById('shredRunPct');
const shredCancel = document.getElementById('shredCancel');
const shredDone = document.getElementById('shredDone');
const shredDoneMsg = document.getElementById('shredDoneMsg');
const shredDoneNote = document.getElementById('shredDoneNote');
const shredDoneList = document.getElementById('shredDoneList');
const shredAgain = document.getElementById('shredAgain');

const SHRED_W = 380;          // 与其它面板同一套窗口宽度
const SHRED_MAX_H = 508;      // 主窗口 620，扣掉 112 的底部偏移
const SHRED_LIST_MAX = 20;    // 列表最多显示 20 项，再多靠标题里的数量

const shred = {
  open: false,
  stage: 'confirm',   // confirm | running | done
  paths: [],          // 待粉碎的路径
  items: [],          // shred_inspect 返回的详情
  running: false,
  percent: 0,
  current: '',
  doneFiles: 0,
  filesTotal: 0,
  lastDone: 0,      // 已覆盖字节（进度文案用）
  lastTotal: 0,     // 总字节
  summary: null,
  lastH: 0,
};

async function shredLayout() {
  clearTimeout(shredBubble._hideTimer);
  shredBubble.classList.remove('closing', 'hidden');
  const h = Math.min(SHRED_MAX_H, Math.max(160, shredBubble.offsetHeight));
  shred.lastH = h;
  await resizeNow(SHRED_W, Math.round(h + 112));
  requestAnimationFrame(() => {
    if (shred.open) bubbleShow(shredBubble);
  });
}

function hideShredPanel() {
  if (!shred.open) return;
  shred.open = false;
  bubbleHide(shredBubble);
  resizeLater(COLLAPSED_SIZE[0], COLLAPSED_SIZE[1]);
}

function shredSize(bytes) {
  const n = Number(bytes) || 0;
  if (n >= 1024 * 1024 * 1024) return (n / 1024 / 1024 / 1024).toFixed(2) + ' GB';
  if (n >= 1024 * 1024) return (n / 1024 / 1024).toFixed(1) + ' MB';
  if (n >= 1024) return (n / 1024).toFixed(1) + ' KB';
  return n + ' B';
}

function shredItemMeta(item) {
  if (item.error) return T('shred.badPath', { err: item.error });
  const files = Number(item.files) || 0;
  if (item.isDir && !files) return T('shred.itemEmpty');
  if (item.isDir) return T('shred.itemDir', { files, size: shredSize(item.size) });
  return shredSize(item.size);
}

function renderShred() {
  const confirm = shred.stage === 'confirm';
  shredTitle.textContent = shred.paths.length > 1
    ? T('shred.titleCount', { n: shred.paths.length })
    : T('shred.title');

  shredList.classList.toggle('hidden', !confirm);
  shredWarn.classList.toggle('hidden', !confirm);
  shredOptRow.classList.toggle('hidden', !confirm);
  shredActions.classList.toggle('hidden', !confirm);
  shredRun.classList.toggle('hidden', shred.stage !== 'running');
  shredDone.classList.toggle('hidden', shred.stage !== 'done');

  if (confirm) {
    shredList.innerHTML = '';
    shred.items.slice(0, SHRED_LIST_MAX).forEach((item) => {
      const row = document.createElement('div');
      row.className = 'shred-item';
      row.title = item.path;

      const main = document.createElement('div');
      main.className = 'shred-item-main';

      const name = document.createElement('span');
      name.className = 'shred-item-name';
      name.textContent = item.name;

      const meta = document.createElement('span');
      meta.className = 'shred-item-meta' + (item.error ? ' bad' : '');
      meta.textContent = shredItemMeta(item);

      main.append(name, meta);

      const remove = document.createElement('button');
      remove.className = 'shred-remove';
      remove.textContent = '×';
      remove.title = T('docs.remove');
      remove.addEventListener('click', (e) => {
        e.stopPropagation();
        shredRemove(item.path);
      });

      row.append(main, remove);
      shredList.appendChild(row);
    });

    const good = shred.items.filter((it) => it.exists && !it.error);
    const totalBytes = good.reduce((a, it) => a + (Number(it.size) || 0), 0);
    const totalFiles = good.reduce((a, it) => a + (Number(it.files) || 0), 0);
    shredStat.textContent = good.length
      ? T('shred.item', { files: totalFiles, size: shredSize(totalBytes) })
      : '';

    const dirs = good.filter((it) => it.isDir).length;
    shredWarn.textContent = T('shred.warnOne') + (dirs ? ' ' + T('shred.warnDir', { n: dirs }) : '');
    shredWarn.title = T('shred.killTip');

    shredStart.disabled = !good.length;
  } else if (shred.stage === 'running') {
    const pct = Math.max(0, Math.min(100, Number(shred.percent) || 0));
    shredRunName.textContent = shred.current || T('shred.running');
    shredRunFill.style.width = pct + '%';
    shredRunLabel.textContent = T('shred.progress', {
      filesDone: shred.doneFiles,
      filesTotal: shred.filesTotal,
      done: shredSize(shred.lastDone),
      total: shredSize(shred.lastTotal),
    });
    shredRunPct.textContent = pct + '%';
  } else if (shred.stage === 'done' && shred.summary) {
    const s = shred.summary;
    const files = Number(s.okFiles) || 0;
    const dirs = Number(s.okDirs) || 0;
    const failed = Number(s.failed) || 0;

    if (s.canceled) {
      shredDoneMsg.textContent = T('shred.canceled');
    } else if (failed) {
      shredDoneMsg.textContent = T('shred.doneFail', { files, failed });
    } else {
      shredDoneMsg.textContent = T('shred.done', { files, dirs, size: shredSize(s.freed) });
    }

    const notes = [];
    if (s.killed && s.killed.length) notes.push(T('shred.killed', { names: s.killed.join(', ') }));
    if (s.reboot && s.reboot.length) notes.push(T('shred.reboot', { n: s.reboot.length }));
    shredDoneNote.textContent = notes.join('\n');
    shredDoneNote.classList.toggle('hidden', !notes.length);
    shredDoneNote.classList.toggle('multiline', !!notes.length);

    shredDoneList.innerHTML = '';
    (s.failedItems || []).forEach((text) => {
      const row = document.createElement('div');
      row.className = 'fs-result';
      const line = document.createElement('span');
      line.className = 'fs-result-name';
      line.textContent = text;
      row.appendChild(line);
      shredDoneList.appendChild(row);
    });
  }
}

async function shredInspect() {
  if (!shred.paths.length) {
    shred.items = [];
    renderShred();
    await shredLayout();
    return;
  }
  try {
    const items = await invoke('shred_inspect', { paths: shred.paths });
    shred.items = Array.isArray(items) ? items : [];
  } catch (err) {
    shred.items = shred.paths.map((p) => ({
      path: p,
      name: p.split(/[\\/]/).pop() || p,
      isDir: false,
      size: 0,
      files: 0,
      exists: true,
      error: String(err),
    }));
  }
  if (!shred.open) return;
  renderShred();
  await shredLayout();
}

async function openShredPanel(paths) {
  if (shred.running) {
    showMsg(T('shred.running'), 2200);
    return;
  }
  const incoming = (paths || []).slice();

  // 多选时资源管理器会一项拉起一次进程：确认页已经开着就把新路径并进列表，
  // 不要用最后一项覆盖前面已经收到的。
  if (shred.open && shred.stage === 'confirm') {
    const merged = shred.paths.slice();
    incoming.forEach((p) => {
      if (!merged.some((x) => x.toLowerCase() === p.toLowerCase())) merged.push(p);
    });
    shred.paths = merged;
    await shredInspect();
    return;
  }

  if (expanded) collapse();
  const seq = ++toolOpenSeq;
  await hideOtherToolBubbles('shred');
  if (seq !== toolOpenSeq) return;
  if (!msgBubble.classList.contains('hidden')) bubbleHide(msgBubble);

  shred.open = true;
  shred.stage = 'confirm';
  shred.summary = null;
  shred.running = false;
  shred.items = [];
  const uniq = [];
  incoming.forEach((p) => {
    if (!uniq.some((x) => x.toLowerCase() === p.toLowerCase())) uniq.push(p);
  });
  shred.paths = uniq;
  shredStart.disabled = true;
  shredStat.textContent = '';
  renderShred();
  await shredLayout();
  await shredInspect();
}

/// 后端在右键点击时已把路径入队（见 shred::stage_and_open）。
/// 多选会连续来好几个请求：用一条 promise 串起来，避免并发弹出 / 互相覆盖列表。
let shredChain = Promise.resolve();

function pullShredPending() {
  shredChain = shredChain
    .then(async () => {
      const pending = (await invoke('shred_take_pending')) || [];
      if (!pending.length) return;
      await openShredPanel(pending);
    })
    .catch((err) => {
      console.error('读取待粉碎路径失败', err);
    });
  return shredChain;
}

function shredRemove(path) {
  if (shred.running) return;
  shred.paths = shred.paths.filter((p) => p !== path);
  shred.items = shred.items.filter((it) => it.path !== path);
  renderShred();
  shredLayout();
}

async function startShred() {
  if (shred.running || !shred.paths.length) return;
  shred.running = true;
  shred.stage = 'running';
  shred.percent = 0;
  shred.current = '';
  shred.doneFiles = 0;
  shred.filesTotal = 0;
  shred.lastDone = 0;
  shred.lastTotal = 0;
  renderShred();
  await shredLayout();
  try {
    await invoke('shred_start', {
      req: { paths: shred.paths, killLocks: !!shredKillLocks.checked },
    });
  } catch (err) {
    shred.running = false;
    shred.stage = 'confirm';
    renderShred();
    await shredLayout();
    showMsg(T('shred.failedToast', { err: String(err) }), 2600);
  }
}

async function stopShred() {
  shredRunLabel.textContent = T('shred.canceling');
  try {
    await invoke('shred_cancel');
  } catch (err) {
    console.error('取消粉碎失败', err);
  }
}

function onShredProgress(payload) {
  if (!payload) return;
  shred.percent = Number(payload.percent) || 0;
  shred.current = String(payload.current || '');
  shred.doneFiles = Number(payload.filesDone) || 0;
  shred.filesTotal = Number(payload.filesTotal) || 0;
  shred.lastDone = Number(payload.done) || 0;
  shred.lastTotal = Number(payload.total) || 0;
  if (!shred.open || shred.stage !== 'running') return;
  const pct = Math.max(0, Math.min(100, shred.percent));
  shredRunName.textContent = shred.current || T('shred.running');
  shredRunFill.style.width = pct + '%';
  shredRunLabel.textContent = T('shred.progress', {
    filesDone: shred.doneFiles,
    filesTotal: shred.filesTotal,
    done: shredSize(shred.lastDone),
    total: shredSize(shred.lastTotal),
  });
  shredRunPct.textContent = pct + '%';
}

function onShredDone(payload) {
  shred.running = false;
  shred.summary = payload || null;
  shred.stage = 'done';
  shred.percent = 100;
  const s = payload || {};
  if (!shred.open) {
    if (s.canceled) showMsg(T('shred.canceled'), 2600);
    else showMsg(T('shred.doneToast', { files: Number(s.okFiles) || 0, size: shredSize(s.freed) }), 2600);
    return;
  }
  renderShred();
  shredLayout();
}

shredStart.addEventListener('click', (e) => {
  e.stopPropagation();
  startShred();
});

shredCancel.addEventListener('click', (e) => {
  e.stopPropagation();
  stopShred();
});

shredClose.addEventListener('click', (e) => {
  e.stopPropagation();
  hideShredPanel();
});

shredAgain.addEventListener('click', (e) => {
  e.stopPropagation();
  hideShredPanel();
  showMsg(T('shred.againToast'), 2600);
});

try {
  window.__TAURI__.event.listen('shred-open', () => { pullShredPending(); });
  window.__TAURI__.event.listen('shred-progress', (e) => onShredProgress(e.payload));
  window.__TAURI__.event.listen('shred-done', (e) => onShredDone(e.payload));
} catch (err) {
  console.error('文件粉碎事件监听注册失败', err);
}

let dragAccepting = false;

function enterAccept() {
  if (dragAccepting) return;
  dragAccepting = true;
  clearTimeout(idleTimer);
  clearTimeout(drowsyTimer);
  if (sleeping || waking) { sleeping = false; waking = false; }
  // 文件悬停在球上时固定为「检索资料」(emotion-ball 40)，直到 drag-leave / drop
  showMood('40', true);
}

function leaveAccept(keepMood = false) {
  dragAccepting = false;
  if (keepMood) {
    // 文件已收下、文档气泡要接棒：保持「检索资料」，等面板收起再恢复
    showMood('40', true);
    clearTimeout(idleTimer);
    clearTimeout(drowsyTimer);
    return;
  }
  clearMood();
  resetIdleTimer();
}

// 后端处理任务的状态（忙碌 / 空闲 / 出错）
window.__TAURI__.event.listen('docs-mood', (e) => {
  const mood = e.payload;
  if (mood === 'busy') {
    showMood('32', true); // 处理中：常驻「忙碌」表情，避免随机表情打断
    resetIdleTimer();
  } else if (mood === 'err') {
    showMood('12', false, 2500); // 失落
  } else {
    showMood('10', false, 1500); // 完成：开心一下
    resetIdleTimer();
  }
});

// ===== 窗口尺寸自校准 =====
// 正常情况下系统缩放多少，CSS 视口都是 420×620。但有的机器（DPI 虚拟化、
// 兼容性设置里的「替代高 DPI 缩放行为」、页面缩放叠加）窗口物理尺寸没跟着放大，
// WebView2 却按真实渲染比例绘制，CSS 视口被压到 300 出头 —— 固定宽度、右侧贴边的
// 气泡就会从左侧溢出，被 body 的 overflow:hidden 裁掉。
//
// 校正不猜比例：把「当前物理尺寸 × 设计CSS ÷ 当前CSS」交给后端（一次就能收敛）。
// 不用 devicePixelRatio，它可能只反映系统缩放，而实际渲染还叠加了页面缩放。
async function fitWindowToDesign() {
  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
  for (let attempt = 0; attempt < 4; attempt++) {
    const cw = window.innerWidth;
    const ch = window.innerHeight;
    if (Math.abs(cw - DESIGN_W) <= 1 && Math.abs(ch - DESIGN_H) <= 1) break;
    try {
      await window.__TAURI__.core.invoke('window_fit', {
        w: DESIGN_W,
        h: DESIGN_H,
        cssW: cw,
        cssH: ch,
      });
    } catch (err) {
      console.error('窗口尺寸校准失败', err);
      break;
    }
    await sleep(80); // 等尺寸真正生效，否则读到的还是旧视口
    // 窗口改了但视口一点没动：这台机器不接受这种改法，别再空转，交给 CSS 兜底
    if (window.innerWidth === cw && window.innerHeight === ch) break;
  }
  // fit 内部用的是推算比例；这里按实测视口再算一次裁剪区域，确保「可点击区域」
  // 与真实内容严格对齐（比例算大了会让窗口露出透明边，透出后面的程序）
  try {
    await window.api.resizeWindow(COLLAPSED_SIZE[0], COLLAPSED_SIZE[1]);
  } catch (err) {
    console.error('裁剪区域重算失败', err);
  }
  const cw = window.innerWidth;
  const ch = window.innerHeight;
  if (Math.abs(cw - DESIGN_W) > 1 || Math.abs(ch - DESIGN_H) > 1) {
    console.warn(`窗口视口未能对齐设计尺寸：${cw}×${ch}（期望 ${DESIGN_W}×${DESIGN_H}），已交给 CSS 兜底`);
  }
}

fitWindowToDesign().catch((err) => console.error('窗口尺寸校准异常', err));

// 所有函数声明完成后按配置落定界面语言（默认中文），
// 再检查是否有右键菜单带进来的粉碎路径（本次可能是从资源管理器拉起的）。
loadLangFromConfig().then(() => {
  setTimeout(() => { if (!shred.open) pullShredPending(); }, 150);
});

