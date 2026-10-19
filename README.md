# FilePaste — 悬浮文件粘贴工具

一个 **Tauri** 桌面悬浮球：粘贴在线文件链接，自动下载并复制到剪贴板，在微信/钉钉/企业微信等桌面应用里直接 Ctrl+V 就能把文件发出去。安装包仅 **2MB**（基于系统 WebView2，无内置浏览器内核）。

## 功能

- **圆形悬浮球**：桌面任意位置自由拖动，始终置顶，自动呼吸动画
- **左键点击**：弹出对话气泡输入框，粘贴文件 URL 后回车即下载；再次点击可收起
- **右键菜单**：粘贴文件链接 / 设置 / 打开临时文件夹 / 清理临时文件夹 / 开机自启动 / 退出
- **表情体系**（纯眼睛表达，无嘴巴）：
  - 随机动画：眨眼、wink、左大右小、左小右大、变大变小、无语（横线）
  - 情绪：下载成功 → 微笑开心；流程报错 → 伤心 + 气泡报错
  - 空闲 2 分钟 → 睡觉（右上角飘 zzz），点击/拖动唤醒
  - 快速连点 3 次 → 生气（3 秒，右上角四向箭头动态标记）
  - 眼睛**全屏跟随鼠标**（瞳孔看向光标位置）
- **消息气泡**：清理完成、复制成功等提示复用输入框同款对话气泡
- **下载进度**：实时显示百分比和已下载大小
- **设置项**：临时文件存放路径、窗口置顶、点击关闭时退出/最小化到托盘

## 环境要求

- Windows 10/11（系统自带 WebView2）
- [Rust](https://rustup.rs/) 1.90+（GNU 工具链）+ MinGW（w64devkit，含 gcc/dlltool）
- Node.js 18+（用于 tauri CLI）

## 开发与打包

```bash
# 安装 tauri CLI
npm install

# 开发模式（热更新调试）
npm run dev

# 一键打包 Windows 安装包
npm run build        # 等价：tauri build
yarn build:win       # 等价：tauri build --bundles nsis
```

产物位置：`src-tauri\target\release\bundle\nsis\FilePaste_1.0.0_x64-setup.exe`（约 2MB，NSIS 向导式安装，可选安装目录）

> 若终端提示找不到 `cargo`，重新打开终端，或先执行：
> ```powershell
> $env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path;C:\w64devkit\w64devkit\bin"
> ```

## 使用方式

1. 运行后屏幕右下角出现一个青色圆形悬浮球
2. **左键点它** → 弹出输入气泡
3. 粘贴文件直链（支持 http/https，自动跟随重定向）→ 回车
4. 等待下载完成，看到"已复制"提示（此时会微笑）
5. 切到微信/钉钉聊天窗口，Ctrl+V，文件就进去了

## 文件结构

| 路径 | 作用 |
|---|---|
| `ui\index.html` | 悬浮球 UI（气泡、表情 DOM） |
| `ui\styles.css` | 样式（主题色 #6ee4de、呼吸动画、表情动画） |
| `ui\renderer.js` | 交互逻辑（点击/拖动/表情/下载流程） |
| `ui\settings.html` | 设置窗口 |
| `src-tauri\src\lib.rs` | Rust 主进程：窗口、全局鼠标轮询、拖动、下载、剪贴板、托盘、右键菜单、配置 |
| `src-tauri\tauri.conf.json` | 窗口/打包配置 |
| `src-tauri\icons\` | 应用图标（青色笑脸） |

## 剪贴板原理

- 通过 `clipboard-rs` 写入标准 **CF_HDROP** 格式（等同资源管理器 Ctrl+C 文件），微信/钉钉/资源管理器均可识别为文件
- 下载用 `reqwest`（rustls TLS，自动跟随重定向 ≤10 次，30s 超时）

## 配置文件位置

`%APPDATA%\com.filepaste.float\config.json`，记录临时目录路径、置顶、自启、关闭行为等设置。
