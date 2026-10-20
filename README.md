# FilePaste — 悬浮文件粘贴工具

一个 **Tauri** 桌面悬浮球：粘贴在线文件链接，自动下载并复制到剪贴板，在微信/钉钉/企业微信等桌面应用里直接 Ctrl+V 就能把文件发出去。安装包仅 **2MB**（基于系统 WebView2，无内置浏览器内核）。

## 功能

- **米白圆形悬浮球**：桌面任意位置自由拖动，始终置顶，表情由 emotion-ball 引擎实时渲染（SVG + 原生 JS，零依赖）
- **左键点击**：弹出对话气泡输入框，粘贴文件 URL 后回车即下载；再次点击可收起
- **右键菜单**：粘贴文件链接 / 设置 / 打开临时文件夹 / 清理临时文件夹 / 开机自启动 / 退出
- **表情体系**（emotion-ball 32 种表情，纯表情表达，无嘴巴）：
  - 内置 **27 个表情随机池**：每 1.2~4.2 秒随机展示一个表情后回落"待机放空"，涵盖好奇/发呆/思考/忙碌/开心/疑惑/惊讶/专注等多种状态
  - 固定触发表情（执行时机不变）：
    - 复制成功 → 开心
    - 复制失败 → 失落（伤心）
    - 快速连点 3 次 → 生气（3 秒）
    - 空闲 1 分 50 秒 → 困（10 秒）
    - 空闲 2 分钟 → 睡觉（右上角飘 zzz），点击/拖动唤醒后先进"困"过渡
  - 眼睛**全屏跟随鼠标**（引擎 gaze，看向光标位置；生气时眼神固定）
  - 配色采用 emotion-ball 原生配色（米白球体 + 表情变色），已移除原项目的青色主题
- **消息气泡**：清理完成、复制成功等提示复用输入框同款对话气泡
- **下载进度**：实时显示百分比和已下载大小
- **设置项**：临时文件存放路径、窗口置顶、点击关闭时退出/最小化到托盘（设置窗口 540px，标题栏为中性米白）

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

1. 运行后屏幕右下角出现一个米白圆形悬浮球（表情待机）
2. **左键点它** → 弹出输入气泡
3. 粘贴文件直链（支持 http/https，自动跟随重定向）→ 回车
4. 等待下载完成，看到"已复制"提示（此时会开心）
5. 切到微信/钉钉聊天窗口，Ctrl+V，文件就进去了

## 文件结构

| 路径 | 作用 |
|---|---|
| `ui\index.html` | 悬浮球 UI（气泡、emotion-ball 渲染容器 #eb-render） |
| `ui\styles.css` | 样式（中性灰阶配色、气泡、悬浮球布局） |
| `ui\renderer.js` | 交互逻辑（点击/拖动/表情触发/下载流程/空闲计时） |
| `ui\settings.html` | 设置窗口（540px 宽） |
| `ui\vendor\emotion-ball\` | emotion-ball 表情引擎（rings.js 几何数据 / emotions.js 表情数据 / ball.js 渲染层 / engine.js 驱动层，原样引入） |
| `src-tauri\src\lib.rs` | Rust 主进程：窗口、全局鼠标轮询、拖动、下载、剪贴板、托盘、右键菜单、配置、DWM 标题栏配色 |
| `src-tauri\tauri.conf.json` | 窗口/打包配置 |
| `src-tauri\icons\` | 应用图标（托盘与窗口图标） |

## 剪贴板原理

- 通过 `clipboard-rs` 写入标准 **CF_HDROP** 格式（等同资源管理器 Ctrl+C 文件），微信/钉钉/资源管理器均可识别为文件
- 下载用 `reqwest`（rustls TLS，自动跟随重定向 ≤10 次，30s 超时）

## 配置文件位置

`%APPDATA%\com.filepaste.float\config.json`，记录临时目录路径、置顶、自启、关闭行为等设置。
