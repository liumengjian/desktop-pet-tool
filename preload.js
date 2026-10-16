const { contextBridge, ipcRenderer } = require('electron');

contextBridge.exposeInMainWorld('api', {
  // 下载
  startDownload: (url) => ipcRenderer.invoke('download:start', url),
  onProgress: (cb) => ipcRenderer.on('download:progress', (_e, data) => cb(data)),
  // 配置
  getConfig: () => ipcRenderer.invoke('config:get'),
  setConfig: (cfg) => ipcRenderer.invoke('config:set', cfg),
  pickTempDir: () => ipcRenderer.invoke('config:pickTempDir'),
  // 事件
  openContextMenu: () => ipcRenderer.send('float:context-menu'),
  onActivateInput: (cb) => ipcRenderer.on('float:activate-input', () => cb()),
  onToast: (cb) => ipcRenderer.on('float:toast', (_e, msg) => cb(msg)),
  // 主动通知主进程失焦
  notifyBlur: () => ipcRenderer.send('float:focus-out'),
  // 调整窗口大小（展开/收起面板）
  resizeWindow: (w, h) => ipcRenderer.invoke('window:resize', w, h),
  // 手动拖动窗口
  startDrag: () => ipcRenderer.send('float:drag-start'),
  stopDrag: () => ipcRenderer.send('float:drag-end'),
  // 请求关闭（退出或最小化到托盘，由设置决定）
  requestClose: () => ipcRenderer.send('float:request-close'),
  // 全局鼠标位置（主进程轮询，用于眼睛全屏跟随）
  onGlobalMouse: (cb) => ipcRenderer.on('mouse:global', (_e, data) => cb(data))
});
