# FilePaste — 悬浮文件粘贴工具

一个 Electron 桌面悬浮窗：粘贴在线文件链接，自动下载并复制到剪贴板，在微信/钉钉/企业微信等桌面应用里直接 Ctrl+V 就能把文件发出去。

## 功能

- **圆形悬浮窗**：桌面任意位置自由拖动，始终置顶
- **左键点击**：弹出输入框，粘贴文件 URL 后回车即下载
- **右键菜单**：设置 / 打开临时文件夹 / 清空临时目录 / 开机自启 / 退出
- **下载进度**：实时显示百分比和已下载大小
- **设置项**：自定义临时文件存放路径、窗口置顶开关

## 快速开始

```bash
# 1. 安装依赖（需要 Node.js 18+）
npm install

# 2. 开发模式运行
npm start
```

## 打包成 exe

```bash
# 打包 Windows 安装包（在 Windows 上执行）
npm run build:win

# 打包 macOS（在 Mac 上执行）
npm run build:mac

# 打包 Linux
npm run build:linux
```

产物在 `dist/` 目录。

## 使用方式

1. 运行后屏幕右下角出现一个淡紫色圆形按钮
2. **左键点它** → 弹出输入框
3. 粘贴文件直链（支持 http/https，自动跟随重定向）→ 回车
4. 等待下载完成，看到"已复制，直接粘贴"提示
5. 切到微信/钉钉聊天窗口，Ctrl+V，文件就进去了

## 文件说明

| 文件 | 作用 |
|---|---|
| `main.js` | Electron 主进程：窗口、下载、剪贴板、右键菜单 |
| `preload.js` | 安全桥接（contextIsolation） |
| `index.html` | 悬浮窗 UI（圆形按钮 + 输入面板） |
| `renderer.js` | 渲染进程交互逻辑 |
| `styles.css` | 样式（主题色 #715EBD） |
| `settings.html` | 设置窗口 |

## 剪贴板原理

- **Windows**：通过 PowerShell 调用 .NET `Clipboard.SetFileDropList()`，写入标准 CF_HDROP 格式，等同于在资源管理器里 Ctrl+C 一个文件
- **macOS**：通过 AppleScript 把文件路径写入 NSPasteboard
- 其他平台降级为复制文件路径文本

## 配置文件位置

`%APPDATA%/file-paste-float/config.json`（Windows），记录临时目录路径等设置。
