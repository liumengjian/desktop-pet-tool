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

// 可交互元素（输入框/按钮/气泡）不参与拖动
function isInteractive(el) {
  return !!el.closest('input, button, .mini-close, #bubble, #docsBubble, #msgBubble');
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
        if (expanded) collapse();
      };
      window.api.openLastTool()
        .then((opened) => { if (!opened) closePanels(); })
        .catch(closePanels);
    } else if (!e.target.closest('#bubble') && !e.target.closest('#docsBubble') && !e.target.closest('#msgBubble')) {
      // 点击窗口内空白（气泡以外）→ 收起气泡与文档面板
      if (expanded) collapse();
      if (docs.open) hideDocsPanel();
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

// Esc 收起输入气泡 / 文档面板
document.addEventListener('keydown', (e) => {
  if (e.key !== 'Escape') return;
  if (expanded) collapse();
  if (docs.open) hideDocsPanel();
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

// ===== 主进程触发的事件 =====
window.api.onActivateInput(() => expand());
window.api.onToast((msg) => showMsg(msg));
// 点击主窗口以外的屏幕任意位置（气泡以外）→ 收起输入气泡与文档面板
window.api.onOutsideClick(() => {
  // 正在系统文件选择框里操作：那不是「点空白收起」，不能收面板
  if (docsPickerOpen || Date.now() < docsSuppressUntil) return;
  if (expanded) collapse();
  if (docs.open) hideDocsPanel();
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

function openDocsPanel() {
  if (expanded) collapse();
  if (!msgBubble.classList.contains('hidden')) bubbleHide(msgBubble);
  docs.open = true;
  docs.stage = 'choose';
  docs.mode = null;
  docs.target = null;
  renderDocs();
  // 面板里有待处理文件时保持「检索资料」，收起面板或开跑后由其他表情接手
  if (docs.files.length && !docs.running) {
    showMood('40', true);
    clearTimeout(idleTimer);
    clearTimeout(drowsyTimer);
  }
  docsLayout();
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

  if (docs.stage === 'options' && docs.mode === 'convert') {
    const targets = docsTargetsFor(docs.files);
    docsStart.disabled = !(hasFiles && targets.length && docs.target);
    if (!hasFiles) {
      docsHint.textContent = T('docs.hintAddFirst');
    } else if (!targets.length) {
      const first = docs.files.find((f) => !f.targets || !f.targets.length);
      docsHint.textContent = first
        ? T('docs.hintNoTargetOne', { name: first.name })
        : T('docs.hintNoTarget');
    } else {
      docsHint.textContent = docs.target
        ? T('docs.hintWillConvert', { fmt: docs.target.toUpperCase() })
        : T('docs.hintPickTarget');
    }
  } else if (docs.stage === 'options' && docs.mode === 'compress') {
    const ok = docsCompressible();
    docsStart.disabled = !(hasFiles && ok.length);
    if (!hasFiles) {
      docsHint.textContent = T('docs.hintAddFirst');
    } else if (!ok.length) {
      const first = docs.files[0];
      docsHint.textContent = first ? first.compressHint : T('docs.hintNoCompress');
    } else {
      const skip = docs.files.length - ok.length;
      const levelName = docs.level === 'small'
        ? T('docs.levelSmall')
        : docs.level === 'high' ? T('docs.levelHigh') : T('docs.levelNormal');
      docsHint.textContent = skip > 0
        ? T('docs.hintCompressSkip', { ok: ok.length, skip })
        : T('docs.hintCompressLevel', { level: levelName });
    }
  } else {
    docsStart.disabled = !hasFiles;
    docsHint.textContent = hasFiles ? T('docs.hintPickMode') : T('docs.empty');
  }
}

function renderDocs() {
  renderDocsFiles();

  const showChoices = docs.stage === 'choose' || docs.stage === 'options';
  docsModes.classList.toggle('hidden', !showChoices);
  docsTargets.classList.toggle('hidden', !(docs.stage === 'options' && docs.mode === 'convert'));
  docsLevels.classList.toggle('hidden', !(docs.stage === 'options' && docs.mode === 'compress'));
  docsHint.classList.toggle('hidden', !showChoices);
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
    if (!Array.isArray(paths) || !paths.length) return;
    const info = await invoke('docs_inspect', { paths });
    mergeDocs(info || []);
  } catch (err) {
    console.error('读取待处理文件失败', err);
  }
}

async function pickDocs() {
  // 选择框是独立窗口：期间所有点击都不该被当作「点空白收起」
  docsPickerOpen = true;
  try {
    const paths = await invoke('docs_pick_files');
    if (!Array.isArray(paths) || !paths.length) {
      // 取消了也要保证面板还在（极端情况下可能已被别的路径收起）
      docs.open = true;
      renderDocs();
      await docsLayout();
      return;
    }
    await invoke('docs_stage', { paths });
    await loadPendingDocs();
    docs.open = true;
    docs.stage = docs.mode ? 'options' : 'choose';
    renderDocs();
    await docsLayout();
  } catch (err) {
    docsHint.textContent = T('docs.pickFailed', { err });
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

async function startDocsJob() {
  if (!docs.files.length || docs.running) return;
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
  docsRunFill.style.width = `${pct}%`;
  docsRunPct.textContent = `${Math.round(pct)}%`;
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
  docs.outputs = (payload && payload.outputs) || [];
  hideRing();
  // 这一批已经结束：把它们从已上传列表里摘掉，再次打开面板从「选择文件」开始，
  // 不会把上一批文件带回来。结果仍留在 docs.outputs 里给完成页展示。
  docs.files = docs.files.filter((f) => !(docs.batch || []).includes(f.path));
  docs.batch = [];

  // 用户中途收了气泡：只在球边提示一句，不强行弹回面板
  if (!docs.open) {
    if (payload && payload.ok) {
      showMsg(T('docs.doneToast', { n: docs.outputs.length }), 2600);
    } else {
      showMsg(T('docs.failedToast'), 2600);
    }
    return;
  }

  docs.stage = 'done';
  docsDoneMsg.textContent = payload && payload.message ? payload.message : '';
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
    delta.textContent = item.note
      ? `${item.before} → ${item.after}${window.I18N.isEn() ? ` (${item.note})` : `（${item.note}）`}`
      : item.after;
    if (item.note) delta.style.color = '#3a9d5d';

    row.append(name, delta);
    docsResults.appendChild(row);
  });
  docsOpenDir.disabled = !(payload && payload.ok);
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
  invoke('docs_open_outdir').catch(() => {});
});
docsAgain.addEventListener('click', (e) => {
  e.stopPropagation();
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
    await loadPendingDocs();
    if (docs.running) return;
    // 已经选过动作就留在选项页，只把新文件补进列表
    docs.stage = docs.mode ? 'options' : 'choose';
    if (docs.mode !== 'convert') docs.target = null;
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
    showMood('21', true); // 处理中：常驻生气表情，避免随机表情打断
    resetIdleTimer();
  } else if (mood === 'err') {
    showMood('12', false, 2500); // 失落
  } else {
    showMood('10', false, 1500); // 完成：开心一下
    resetIdleTimer();
  }
});

// 所有函数声明完成后按配置落定界面语言（默认中文）
loadLangFromConfig();


