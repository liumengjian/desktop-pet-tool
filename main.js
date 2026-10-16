const { app, BrowserWindow, Menu, ipcMain, clipboard, dialog, shell, screen, Tray, nativeImage } = require('electron');
const path = require('path');
const fs = require('fs');
const https = require('https');
const http = require('http');
const { URL } = require('url');
const { execFile } = require('child_process');

// ============== 配置持久化 ==============
const CONFIG_PATH = path.join(app.getPath('userData'), 'config.json');

function loadConfig() {
  try {
    if (fs.existsSync(CONFIG_PATH)) {
      return JSON.parse(fs.readFileSync(CONFIG_PATH, 'utf-8'));
    }
  } catch (e) {}
  return {
    tempDir: path.join(app.getPath('temp'), 'file-paste-float'),
    alwaysOnTop: true,
    autoStart: false,
    closeToTray: false
  };
}

function saveConfig(cfg) {
  fs.writeFileSync(CONFIG_PATH, JSON.stringify(cfg, null, 2), 'utf-8');
}

let config = loadConfig();

// 安装器在 userData 下写入的临时目录标记文件（installer.nsh 生成，一次性消费）
const TEMPDIR_FLAG = path.join(app.getPath('userData'), 'tempdir.txt');

// 应用安装时选择的临时文件存放位置
function applyInstallerTempDir() {
  try {
    if (fs.existsSync(TEMPDIR_FLAG)) {
      const dir = fs.readFileSync(TEMPDIR_FLAG, 'utf-8').trim();
      if (dir) {
        config.tempDir = dir;
        saveConfig(config);
      }
      fs.unlinkSync(TEMPDIR_FLAG);
    }
  } catch (e) {}
}

// 确保临时目录存在
function ensureTempDir() {
  try {
    if (!fs.existsSync(config.tempDir)) {
      fs.mkdirSync(config.tempDir, { recursive: true });
    }
  } catch (e) {
    console.error('创建临时目录失败:', e);
  }
}

// ============== 系统托盘 ==============
function createTray() {
  if (tray) return;
  let icon = nativeImage.createFromPath(path.join(__dirname, 'assets', 'tray.png'));
  if (icon.isEmpty()) {
    // 兜底：内嵌 1px 紫色点
    icon = nativeImage.createFromDataURL(
      'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGOSHzRgAAAABJRU5ErkJggg=='
    );
  }
  tray = new Tray(icon);
  tray.setToolTip('FilePaste 悬浮文件粘贴');
  tray.setContextMenu(Menu.buildFromTemplate([
    { label: '显示悬浮窗', click: () => { if (mainWin) { mainWin.show(); mainWin.focus(); } } },
    { type: 'separator' },
    { label: '退出', click: () => app.quit() }
  ]));
  tray.on('click', () => { if (mainWin) { mainWin.show(); mainWin.focus(); } });
  tray.on('double-click', () => { if (mainWin) { mainWin.show(); mainWin.focus(); } });
}

// ============== 窗口拖动（手动实现，替代 -webkit-app-region: drag） ==============
let dragTimer = null;
let dragOffset = null;
let dragStartCursor = null;
// 死区阈值：与渲染进程的点击判定阈值一致（<6px 视为点击）
// 鼠标在死区内移动时窗口不跟随，避免"点击时轻微抖动导致窗口漂移"
const DRAG_DEAD_ZONE = 6;

function startWindowDrag() {
  if (!mainWin || dragTimer) return;
  const cursor = screen.getCursorScreenPoint();
  const [wx, wy] = mainWin.getPosition();
  dragOffset = { dx: cursor.x - wx, dy: cursor.y - wy };
  dragStartCursor = { x: cursor.x, y: cursor.y };
  dragTimer = setInterval(() => {
    const c = screen.getCursorScreenPoint();
    const dist = Math.hypot(c.x - dragStartCursor.x, c.y - dragStartCursor.y);
    if (dist < DRAG_DEAD_ZONE) return; // 死区内：窗口保持不动
    mainWin.setPosition(Math.round(c.x - dragOffset.dx), Math.round(c.y - dragOffset.dy));
  }, 16);
}

function stopWindowDrag() {
  if (dragTimer) { clearInterval(dragTimer); dragTimer = null; }
  dragOffset = null;
  dragStartCursor = null;
}

// ============== 全局鼠标跟踪（眼睛全屏跟随） ==============
let mouseTimer = null;

function startGlobalMouseTrack() {
  if (mouseTimer) return;
  // 每 33ms 读取一次全局鼠标位置 + 窗口位置，发给渲染进程
  mouseTimer = setInterval(() => {
    if (!mainWin || mainWin.isDestroyed()) return;
    const c = screen.getCursorScreenPoint();
    const [wx, wy] = mainWin.getPosition();
    mainWin.webContents.send('mouse:global', { mx: c.x, my: c.y, wx, wy });
  }, 33);
}

// ============== 窗口 ==============
let mainWin = null;   // 悬浮主窗口
let settingsWin = null; // 设置窗口
let tray = null;

function createMainWindow() {
  mainWin = new BrowserWindow({
    width: 112,
    height: 112,
    frame: false,
    transparent: true,
    alwaysOnTop: config.alwaysOnTop,
    resizable: false,
    skipTaskbar: true,
    hasShadow: false,
    webPreferences: {
      preload: path.join(__dirname, 'preload.js'),
      contextIsolation: true,
      nodeIntegration: false
    }
  });

  mainWin.setAlwaysOnTop(true, 'floating');
  mainWin.setVisibleOnAllWorkspaces(true);
  mainWin.loadFile('index.html');

  // 初始位置：屏幕右下角偏上
  const display = screen.getPrimaryDisplay();
  const { width: sw, height: sh } = display.workAreaSize;
  mainWin.setPosition(sw - 160, sh - 220);
}

function createSettingsWindow() {
  if (settingsWin) {
    settingsWin.focus();
    return;
  }
  // 在鼠标当前所在屏幕（当前屏幕）的正中央打开
  const cursor = screen.getCursorScreenPoint();
  const display = screen.getDisplayNearestPoint(cursor);
  const wa = display.workArea;
  const W = 460, H = 380;
  settingsWin = new BrowserWindow({
    width: W,
    height: H,
    x: Math.round(wa.x + (wa.width - W) / 2),
    y: Math.round(wa.y + (wa.height - H) / 2),
    title: '设置',
    resizable: false,
    parent: mainWin || undefined,
    modal: false,
    webPreferences: {
      preload: path.join(__dirname, 'preload.js'),
      contextIsolation: true,
      nodeIntegration: false
    }
  });
  settingsWin.setMenuBarVisibility(false);
  settingsWin.loadFile('settings.html');
  settingsWin.on('closed', () => { settingsWin = null; });
}

// ============== 右键菜单 ==============
function showContextMenu(e) {
  const menu = Menu.buildFromTemplate([
    { label: '粘贴文件链接…', click: () => mainWin.webContents.send('float:activate-input') },
    { type: 'separator' },
    { label: '设置', click: () => createSettingsWindow() },
    { label: '打开临时文件夹', click: () => {
      ensureTempDir();
      shell.openPath(config.tempDir);
    }},
    { label: '清理临时文件夹', click: async () => {
      try {
        ensureTempDir();
        const files = fs.readdirSync(config.tempDir);
        for (const f of files) {
          fs.unlinkSync(path.join(config.tempDir, f));
        }
        mainWin.webContents.send('float:toast', '临时文件夹已清理');
      } catch (err) {
        mainWin.webContents.send('float:toast', '清理失败: ' + err.message);
      }
    }},
    { type: 'separator' },
    { label: '开机自启动', type: 'checkbox', checked: config.autoStart, click: (item) => {
      config.autoStart = item.checked;
      saveConfig(config);
      app.setLoginItemSettings({ openAtLogin: item.checked });
    }},
    { label: '退出', click: () => app.quit() }
  ]);
  menu.popup({ window: mainWin });
}

// ============== 下载逻辑 ==============
function downloadFile(url, redirectCount = 0) {
  return new Promise((resolve, reject) => {
    if (redirectCount > 10) return reject(new Error('重定向次数过多'));

    let parsed;
    try { parsed = new URL(url); } catch (e) { return reject(new Error('链接格式不正确')); }

    const client = parsed.protocol === 'https:' ? https : http;

    ensureTempDir();

    const req = client.get(parsed, {
      headers: { 'User-Agent': 'Mozilla/5.0 FilePaste/1.0' },
      timeout: 30000
    }, (res) => {
      // 处理重定向
      if ([301, 302, 303, 307, 308].includes(res.statusCode)) {
        const loc = res.headers.location;
        res.resume();
        if (!loc) return reject(new Error('重定向无目标'));
        const nextUrl = loc.startsWith('http') ? loc : new URL(loc, url).href;
        return resolve(downloadFile(nextUrl, redirectCount + 1));
      }

      if (res.statusCode < 200 || res.statusCode >= 300) {
        res.resume();
        return reject(new Error('下载失败，HTTP ' + res.statusCode));
      }

      // 推断文件名
      let filename = '';
      const cd = res.headers['content-disposition'];
      if (cd) {
        const m = cd.match(/filename\*?=(?:UTF-8'')?["']?([^;"']+)["']?/i);
        if (m) filename = decodeURIComponent(m[1]);
      }
      if (!filename) {
        const pathname = decodeURIComponent(parsed.pathname);
        filename = path.basename(pathname) || 'download';
      }
      // 去掉非法字符
      filename = filename.replace(/[\\/:*?"<>|]/g, '_').trim();
      if (!filename) filename = 'download';

      const totalBytes = parseInt(res.headers['content-length'] || '0', 10);
      const destPath = path.join(config.tempDir, filename);
      const out = fs.createWriteStream(destPath);

      let receivedBytes = 0;
      let lastReport = 0;

      res.on('data', (chunk) => {
        receivedBytes += chunk.length;
        const now = Date.now();
        // 每 100ms 上报一次进度
        if (now - lastReport > 100) {
          lastReport = now;
          mainWin.webContents.send('download:progress', {
            received: receivedBytes,
            total: totalBytes,
            filename
          });
        }
      });

      res.pipe(out);

      out.on('finish', () => {
        out.close(() => resolve({ filePath: destPath, filename, size: receivedBytes }));
      });
      out.on('error', (err) => {
        try { fs.unlinkSync(destPath); } catch (e) {}
        reject(err);
      });
    });

    req.on('error', reject);
    req.on('timeout', () => { req.destroy(new Error('下载超时')); });
  });
}

// ============== 复制文件到剪贴板 ==============
function copyFileToClipboard(filePath) {
  const absPath = path.resolve(filePath);
  const platform = process.platform;

  if (platform === 'win32') {
    // 用 PowerShell 调 .NET Clipboard.SetFileDropList，
    // 生成标准 CF_HDROP 格式，微信/钉钉/资源管理器都能识别为文件
    return new Promise((resolve) => {
      const psScript =
        `Add-Type -AssemblyName System.Windows.Forms;` +
        `$files = New-Object System.Collections.Specialized.StringCollection;` +
        `$files.Add('${absPath.replace(/'/g, "''")}') | Out-Null;` +
        `[System.Windows.Forms.Clipboard]::SetFileDropList($files);`;
      execFile('powershell.exe', ['-NoProfile', '-STA', '-Command', psScript],
        { windowsHide: true, timeout: 5000 },
        (err) => {
          if (err) {
            console.error('PowerShell 剪贴板写入失败:', err);
            try { clipboard.writeText(absPath); } catch (e) {}
            resolve(false);
          } else {
            resolve(true);
          }
        });
    });
  } else if (platform === 'darwin') {
    // macOS: 用 pbcopy 写文件 URL（基础支持）
    return new Promise((resolve) => {
      const script = `set the clipboard to POSIX file "${absPath}" as alias`;
      execFile('osascript', ['-e', script], { timeout: 5000 }, (err) => {
        if (err) {
          try { clipboard.writeText(absPath); } catch (e) {}
          resolve(false);
        } else {
          resolve(true);
        }
      });
    });
  } else {
    // Linux: 写文本路径作为降级
    try { clipboard.writeText(absPath); } catch (e) {}
    return Promise.resolve(false);
  }
}

// ============== IPC ==============
ipcMain.handle('download:start', async (event, url) => {
  try {
    const result = await downloadFile(url);
    const copied = await copyFileToClipboard(result.filePath);
    return {
      ok: true,
      filePath: result.filePath,
      filename: result.filename,
      size: result.size,
      copiedToClipboard: copied
    };
  } catch (err) {
    return { ok: false, error: err.message };
  }
});

ipcMain.handle('config:get', () => config);

ipcMain.handle('config:set', (event, newCfg) => {
  config = { ...config, ...newCfg };
  saveConfig(config);
  if (mainWin) mainWin.setAlwaysOnTop(config.alwaysOnTop, 'floating');
  // 关闭托盘模式时同步销毁托盘图标
  if (!config.closeToTray && tray) { tray.destroy(); tray = null; }
  return config;
});

ipcMain.handle('config:pickTempDir', async () => {
  const res = await dialog.showOpenDialog(mainWin, {
    title: '选择临时文件存放目录',
    defaultPath: config.tempDir,
    properties: ['openDirectory', 'createDirectory']
  });
  if (!res.canceled && res.filePaths[0]) {
    config.tempDir = res.filePaths[0];
    saveConfig(config);
    return config.tempDir;
  }
  return null;
});

ipcMain.on('float:context-menu', () => showContextMenu());

// 关闭悬浮球右上角按钮：根据设置决定退出或最小化到托盘
ipcMain.on('float:request-close', () => {
  if (config.closeToTray) {
    createTray();
    mainWin.hide();
  } else {
    app.quit();
  }
});

// 手动拖动窗口
ipcMain.on('float:drag-start', () => startWindowDrag());
ipcMain.on('float:drag-end', () => stopWindowDrag());

// 动态调整窗口大小：保持右下角锚点位置不变
// 用 setBounds 原子设置位置+尺寸，避免 setSize/setPosition 两步导致的中间态漂移
ipcMain.handle('window:resize', (event, w, h) => {
  if (!mainWin) return;
  const [curW, curH] = mainWin.getSize();
  const [curX, curY] = mainWin.getPosition();
  // 计算原右下角
  const right = curX + curW;
  const bottom = curY + curH;
  // 新窗口位置让右下角对齐原右下角
  mainWin.setBounds({ x: right - w, y: bottom - h, width: w, height: h });
});

ipcMain.on('float:focus-out', () => {
  // 输入框失焦后可让窗口收起（由渲染进程自己控制）
});

// ============== App 生命周期 ==============
app.whenReady().then(() => {
  // 应用安装器选择的临时文件目录（首次启动生效）
  applyInstallerTempDir();
  // 恢复开机自启设置
  if (config.autoStart) {
    app.setLoginItemSettings({ openAtLogin: true });
  }
  createMainWindow();
  startGlobalMouseTrack();
});

app.on('window-all-closed', () => {
  // 悬浮窗关闭即退出（无托盘图标，保持轻量）
  app.quit();
});
