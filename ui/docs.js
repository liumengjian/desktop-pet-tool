// 文档工具窗口：拖入/选择文件 → 压缩 or 转换 → 进度 → 输出到固定文件夹

const invoke = window.__TAURI__.core.invoke;
const listen = window.__TAURI__.event.listen;

const el = {
  stageMain: document.getElementById('stage'),
  panelPick: document.getElementById('panelPick'),
  panelMode: document.getElementById('panelMode'),
  footerPick: document.getElementById('footerPick'),
  stageRun: document.getElementById('stageRun'),
  stageDone: document.getElementById('stageDone'),
  fileList: document.getElementById('fileList'),
  btnAdd: document.getElementById('btnAdd'),
  modeCompress: document.getElementById('modeCompress'),
  modeConvert: document.getElementById('modeConvert'),
  levelBox: document.getElementById('levelBox'),
  targetBox: document.getElementById('targetBox'),
  targetChips: document.getElementById('targetChips'),
  targetHint: document.getElementById('targetHint'),
  notice: document.getElementById('notice'),
  footNote: document.getElementById('footNote'),
  btnStart: document.getElementById('btnStart'),
  runTitle: document.getElementById('runTitle'),
  runFile: document.getElementById('runFile'),
  runLabel: document.getElementById('runLabel'),
  runPct: document.getElementById('runPct'),
  barFill: document.getElementById('barFill'),
  btnCancel: document.getElementById('btnCancel'),
  doneTitle: document.getElementById('doneTitle'),
  doneMsg: document.getElementById('doneMsg'),
  resultList: document.getElementById('resultList'),
  doneDir: document.getElementById('doneDir'),
  btnOpenDir: document.getElementById('btnOpenDir'),
  btnAgain: document.getElementById('btnAgain'),
  btnClose: document.getElementById('btnClose'),
};

const KIND_LABEL = {
  image: '图片',
  word: 'Word',
  sheet: '表格',
  slides: '幻灯片',
  pdf: 'PDF',
  other: '不支持',
};

const state = {
  files: [],
  mode: null,
  level: 'normal',
  target: null,
  outputs: [],
};

function setNotice(text) {
  if (!text) {
    el.notice.hidden = true;
    el.notice.textContent = '';
    return;
  }
  el.notice.hidden = false;
  el.notice.textContent = text;
}

function renderFiles() {
  el.fileList.innerHTML = '';
  if (!state.files.length) {
    const empty = document.createElement('li');
    empty.className = 'empty';
    empty.textContent = '把文件拖到悬浮球上，或点右上角「选择文件」';
    el.fileList.appendChild(empty);
    return;
  }
  state.files.forEach((f, idx) => {
    const row = document.createElement('div');
    row.className = 'file-row';

    const name = document.createElement('div');
    name.className = 'name';
    name.textContent = f.name;
    name.title = f.path;

    const kind = document.createElement('span');
    kind.className = 'kind';
    kind.textContent = KIND_LABEL[f.kind] || f.kind;

    const size = document.createElement('span');
    size.className = 'size';
    size.textContent = f.sizeLabel;

    const remove = document.createElement('button');
    remove.className = 'remove';
    remove.textContent = '×';
    remove.title = '移出列表';
    remove.addEventListener('click', () => {
      state.files.splice(idx, 1);
      renderFiles();
      syncControls();
    });

    row.append(name, kind, size, remove);
    el.fileList.appendChild(row);
  });
}

function supportedTargets() {
  const perFile = state.files
    .filter((f) => f.targets && f.targets.length)
    .map((f) => f.targets);
  if (!perFile.length) return [];
  const inter = perFile.reduce((acc, list) => acc.filter((t) => list.includes(t)));
  if (inter.length) return inter;
  // 没有交集时退回并集，运行时逐文件提示
  return Array.from(new Set(perFile.flat()));
}

function renderTargets() {
  const targets = supportedTargets();
  el.targetChips.innerHTML = '';
  if (!targets.length) {
    el.targetHint.textContent = state.files.length
      ? '所选文件在当前版本没有可用的目标格式'
      : '先添加文件，之后这里会列出可选的目标格式';
    state.target = null;
    return;
  }
  const hasCommon = state.files
    .filter((f) => f.targets && f.targets.length)
    .reduce((acc, f) => acc && f.targets.includes(targets[0]), true);
  targets.forEach((t) => {
    const chip = document.createElement('button');
    chip.className = 'chip' + (state.target === t ? ' active' : '');
    chip.textContent = t.toUpperCase();
    chip.addEventListener('click', () => {
      state.target = t;
      Array.from(el.targetChips.children).forEach((c) => c.classList.remove('active'));
      chip.classList.add('active');
      syncControls();
    });
    el.targetChips.appendChild(chip);
  });
  if (state.target && !targets.includes(state.target)) state.target = null;
  el.targetHint.textContent = hasCommon
    ? '输出为同名文件，保存在「临时文件夹」（右键菜单可打开或清理）'
    : '不同文件支持的目标格式不同，不支持的会单独提示';
}

function syncControls() {
  const hasFiles = state.files.length > 0;
  // 压缩 / 转换 允许在没有文件时先选好：拖文件进来前就能挑好动作与强度，
  // 不再把按钮禁用掉（禁用后点了没反应，用户以为卡住）。
  el.modeCompress.disabled = false;
  el.modeConvert.disabled = false;

  if (state.mode === 'convert' && state.target === 'docx' && state.files.some((f) => f.kind === 'pdf')) {
    el.targetHint.textContent = 'PDF 转 Word：文字和规则表格可编辑，照片、线条可单独移动；尽量保留原分页与字体，缺失字体或扫描件会提示。';
  } else if (supportedTargets().length) {
    el.targetHint.textContent = '输出为同名文件，保存在「临时文件夹」（右键菜单可打开或清理）；不支持的格式会单独提示';
  }

  let ready = false;
  if (hasFiles && state.mode === 'compress') {
    ready = state.files.some((f) => f.canCompress);
  } else if (hasFiles && state.mode === 'convert') {
    ready = Boolean(state.target) && state.files.some((f) => f.targets.includes(state.target));
  }
  el.btnStart.disabled = !ready || state.running;

  if (!hasFiles) {
    el.footNote.textContent = state.mode
      ? '先添加文件，再开始处理'
      : '先添加文件，或先选好压缩 / 转换';
  } else if (state.mode === 'compress') {
    const blocked = state.files.filter((f) => !f.canCompress);
    if (blocked.length === state.files.length) {
      const hints = blocked.map((f) => f.compressHint).filter(Boolean);
      el.footNote.textContent = hints.length
        ? hints[0]
        : '所选文件都不支持压缩，可改用「转换」';
    } else {
      el.footNote.textContent = blocked.length
        ? `${blocked.length} 个文件无法压缩，将被跳过`
        : `${state.files.length} 个文件待压缩`;
    }
  } else if (state.mode === 'convert') {
    el.footNote.textContent = state.target ? `目标格式 ${state.target.toUpperCase()}` : '请选择目标格式';
  } else {
    el.footNote.textContent = '请选择压缩或转换';
  }
}

function selectMode(mode) {
  state.mode = mode;
  el.modeCompress.classList.toggle('active', mode === 'compress');
  el.modeConvert.classList.toggle('active', mode === 'convert');
  el.levelBox.hidden = mode !== 'compress';
  el.targetBox.hidden = mode !== 'convert';
  if (mode === 'convert') renderTargets();
  syncControls();
}

async function mergePaths(paths) {
  if (!paths || !paths.length) return;
  let infos = [];
  try {
    infos = await invoke('docs_inspect', { paths });
  } catch (err) {
    setNotice(`读取文件信息失败：${err}`);
    return;
  }
  infos.forEach((info) => {
    if (!state.files.some((f) => f.path === info.path)) state.files.push(info);
  });
  const ignored = paths.length - infos.length;
  setNotice(ignored > 0 ? `已忽略 ${ignored} 个文件：超过 2GB 上限或无法读取` : '');
  renderFiles();
  if (state.mode === 'convert') renderTargets();
  syncControls();
}

async function loadPending() {
  try {
    const paths = await invoke('docs_take_pending');
    await mergePaths(paths);
  } catch (err) {
    setNotice(`读取待处理文件失败：${err}`);
  }
}

async function startJob() {
  if (state.running || el.btnStart.disabled) return;
  state.running = true;
  syncControls();

  el.panelPick.hidden = true;
  el.panelMode.hidden = true;
  el.footerPick.hidden = true;
  // 整个「选文件 + 选动作」主区一起收起来，否则它的 flex:1 会在上方留一大片空白
  el.stageMain.hidden = true;
  el.stageRun.hidden = false;
  el.stageDone.hidden = true;

  const total = state.files.length;
  el.runTitle.textContent = state.mode === 'compress' ? '正在压缩' : '正在转换';
  el.runFile.textContent = state.files[0] ? state.files[0].name : '';
  el.runLabel.textContent = `${total} 个文件`;
  el.runPct.textContent = '0%';
  el.barFill.style.width = '0%';

  try {
    await invoke('docs_start', {
      job: {
        paths: state.files.map((f) => f.path),
        mode: state.mode,
        target: state.target,
        level: state.level,
      },
    });
  } catch (err) {
    state.running = false;
    showDone({
      ok: false,
      message: String(err),
      outdir: '',
      outputs: [],
    });
  }
}

function onProgress(payload) {
  if (!payload) return;
  const pct = Math.max(0, Math.min(100, Number(payload.percent) || 0));
  el.barFill.style.width = `${pct}%`;
  el.runPct.textContent = `${Math.round(pct)}%`;
  if (payload.name) el.runFile.textContent = payload.name;
  if (payload.label) {
    el.runLabel.textContent = payload.total
      ? `${payload.index + 1}/${payload.total} · ${payload.label}`
      : payload.label;
  }
}

function showDone(payload) {
  state.running = false;
  state.outputs = payload.outputs || [];

  el.panelPick.hidden = true;
  el.panelMode.hidden = true;
  el.footerPick.hidden = true;
  el.stageMain.hidden = true;
  el.stageRun.hidden = true;
  el.stageDone.hidden = false;

  el.doneTitle.textContent = payload.ok ? '处理成功' : '处理失败';
  el.doneMsg.textContent = '';
  el.doneDir.textContent = payload.outdir ? `输出目录：${payload.outdir}` : '';

  el.resultList.innerHTML = '';
  state.outputs.forEach((item) => {
    const row = document.createElement('div');
    row.className = 'result-row';

    const name = document.createElement('div');
    name.className = 'name';
    name.textContent = item.name;
    name.title = item.path;

    const delta = document.createElement('span');
    delta.className = 'delta';
    delta.textContent = '处理成功';
    delta.classList.add('flat');
    row.append(name, delta);
    el.resultList.appendChild(row);
  });

  el.btnOpenDir.disabled = !payload.ok;
}

function resetToPick() {
  state.outputs = [];
  state.files = [];
  state.mode = null;
  state.target = null;
  el.modeCompress.classList.remove('active');
  el.modeConvert.classList.remove('active');
  el.levelBox.hidden = true;
  el.targetBox.hidden = true;
  renderFiles();
  el.stageDone.hidden = true;
  el.stageRun.hidden = true;
  el.stageMain.hidden = false;
  el.panelPick.hidden = false;
  el.panelMode.hidden = false;
  el.footerPick.hidden = false;
  syncControls();
  loadPending();
}

// ---------- 事件绑定 ----------

el.btnAdd.addEventListener('click', async () => {
  try {
    const paths = await invoke('docs_pick_files');
    await mergePaths(paths);
  } catch (err) {
    setNotice(`选择文件失败：${err}`);
  }
});

el.modeCompress.addEventListener('click', () => selectMode('compress'));
el.modeConvert.addEventListener('click', () => selectMode('convert'));

document.getElementById('levelChips').addEventListener('click', (e) => {
  const chip = e.target.closest('.chip');
  if (!chip) return;
  state.level = chip.dataset.level;
  Array.from(chip.parentElement.children).forEach((c) => c.classList.remove('active'));
  chip.classList.add('active');
});

el.btnStart.addEventListener('click', startJob);
el.btnCancel.addEventListener('click', () => {
  invoke('docs_cancel');
  el.runLabel.textContent = '正在取消…';
});
el.btnOpenDir.addEventListener('click', () => {
  invoke('docs_open_outdir').catch(() => {});
});
el.btnAgain.addEventListener('click', resetToPick);
el.btnClose.addEventListener('click', () => {
  invoke('close_docs').catch(() => {});
});

listen('docs-progress', (e) => onProgress(e.payload));
listen('docs-done', (e) => showDone(e.payload || {}));
listen('docs-staged', () => loadPending());
listen('tauri://drag-drop', (e) => {
  const paths = (e.payload && e.payload.paths) || [];
  if (paths.length) mergePaths(paths);
});

// 初次加载
loadPending();
