# desktop-pet-tool

一个 **Tauri** 桌面悬浮球：一边把在线文件链接变成可直接粘贴的文件，一边把拖进来的文档就地转换 / 压缩。
安装包约 **10.4 MB**（基于系统 WebView2，无内置浏览器内核；体积主要来自内置中文字库）。

## 功能

### 1. 悬浮球本体

- **米白圆形悬浮球**：桌面任意位置自由拖动，始终置顶，表情由 emotion-ball 引擎实时渲染（SVG + 原生 JS，零依赖）
- **左键点击**：唤醒 / 收起当前打开的气泡。**左键不再弹出输入框**，输入框只从右键菜单「文档复制」打开
- **右键菜单**（两个分区）：
  - 文档相关：**文档复制** / **文档工具**
  - 临时文件与程序：**打开临时文件夹** / **清理临时文件夹** / **设置** / **开机自启动** / **退出**
- **表情体系**（emotion-ball 32 种表情，纯表情表达，无嘴巴）：
  - 内置 **27 个表情随机池**：每 1.2~4.2 秒随机展示一个表情后回落“待机放空”
  - 固定触发表情：复制成功 → 开心；复制失败 → 失落；快速连点 3 次 → 生气（3 秒）；文件拖到球上 → **检索资料**（emotion-ball 40，保持到文档面板收起）；文档处理中 → 常驻生气（防止随机表情打断）；空闲 1 分 50 秒 → 困（10 秒）；空闲 2 分钟 → 睡觉（右上角飘 zzz），点击/拖动唤醒后先进“困”过渡
  - 眼睛**全屏跟随鼠标**（引擎 gaze，看向光标位置；生气时眼神固定）

### 2. 文档复制

右键「文档复制」→ 气泡里粘贴文件直链（http/https，自动跟随重定向 ≤10 次）→ 回车 → 下载并写入剪贴板（标准 `CF_HDROP`），在微信 / 钉钉 / 企业微信里 Ctrl+V 直接发文件。

### 3. 文档工具（气泡内完成，不再开独立窗口）

把文件拖到悬浮球上，球边浮出对话气泡，整条流程都不离开球：

1. **拖入** → 气泡列出待处理文件（可逐个移除、可继续添加）
2. **两个动作按钮**：`转换` / `压缩`
3. 点「转换」再浮出**目标格式**按钮；点「压缩」再浮出**三档强度**（更小 / 标准 / 更清晰）
4. 点「开始处理」→ **球外圈进度环 + 球下百分比**，气泡内显示当前文件与百分比
5. 完成后弹**结果气泡**：每个文件的体积变化 + 「打开临时文件夹」按钮
6. 中途收起气泡，任务照常跑，进度仍显示在球上

产物统一落到**临时文件夹**（默认 `%TEMP%\file-paste-float`），与「文档复制」下载的文件同一个目录，右键「清理临时文件夹」可以一次清干净。

## 支持格式与转换矩阵

| 输入 | 可转换为 | 可压缩 |
|---|---|---|
| 图片 `png/jpg/jpeg/bmp/gif/tif/tiff/webp` | jpg, png, bmp, tif, pdf | ✅ |
| Word `docx` | txt, html, md, pdf | ✅ |
| Word 旧版 `doc`（OLE CFB） | txt, md, html, docx, pdf | ❌（先转 docx 再压缩） |
| `rtf` / `odt` / `epub` | txt, md, html, docx, pdf | ❌ |
| 表格 `xlsx` | csv, txt, html, pdf | ✅ |
| 表格 `ods` | csv, xlsx, txt, html, docx, pdf | ❌ |
| 演示 `pptx` | txt, html, md, pdf | ✅ |
| 演示 `odp` | txt, md, html, docx, pdf | ❌ |
| `pdf` | txt, md, html, docx, xlsx, pptx, jpg, png | ✅ |
| `txt` | docx, html, pdf | — 无需压缩 |
| `md` | docx, html, txt, pdf | — 无需压缩 |
| `html/htm` | txt, md, pdf | — 无需压缩 |
| `csv` | xlsx, txt, html, pdf | — 无需压缩 |
| 旧版 `xls` / `ppt` | — | — 暂不支持 |

- 单文件硬上限 **2 GB**；超过上限的文件不会进列表
- 多文件时目标格式取交集；没有交集则取并集并在运行时逐文件提示
- 压缩档位：图片按长边与质量、Office 按重打包与可选部件清理、PDF 按流式重写 + 图片重采样

## 使用方式

1. 运行后屏幕右下角出现米白圆形悬浮球
2. **要发文件链接**：右键 →「文档复制」→ 粘贴直链回车 → 完成后切到聊天窗口 Ctrl+V
3. **要转换 / 压缩文档**：直接把文件拖到球上 → 选「转换」或「压缩」→ 选格式或强度 → 开始 → 完成后点「打开临时文件夹」取产物

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
npm run build                # 等价：tauri build
npm run build:win            # 等价：tauri build --bundles nsis
```

产物位置：`src-tauri\target\release\bundle\nsis\desktop-pet-tool_1.0.0_x64-setup.exe`（约 10.4 MB，NSIS 向导式安装，可选安装目录）

离线环境打包：

```powershell
cd src-tauri; $env:CARGO_NET_OFFLINE='true'; cargo build --offline --release
cd ..; $env:CARGO_NET_OFFLINE='true'; npx tauri build --bundles nsis
```

> 只改前端时，先 touch `src-tauri\src\lib.rs` 再打包，否则增量构建不会重新嵌入 `ui/` 资源。
> 若终端提示找不到 `cargo`，重新打开终端，或先执行：
> ```powershell
> $env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path;C:\w64devkit\w64devkit\bin"
> ```

## 测试

`tools\docs-smoke` 是复用真实源码（`include!`）的端到端烟测，不依赖 Tauri 运行时：

```powershell
cd tools\docs-smoke
$env:CARGO_NET_OFFLINE='true'
cargo run --offline --quiet                 # 全量断言
cargo run --offline --quiet -- compress <文件> small   # 单文件压缩直测
```

覆盖图片 / Office / PDF / 结构化转换 / 旧版 `.doc` 取字 / 表格转 PDF 排版等分支，全部通过时 stdout 输出 `ALL PASS`。

## 文件结构

| 路径 | 作用 |
|---|---|
| `ui\index.html` | 悬浮球 UI（输入气泡、文档气泡、emotion-ball 渲染容器、进度环） |
| `ui\styles.css` | 样式（气泡、文档面板、chips、球体进度环） |
| `ui\renderer.js` | 交互逻辑（点击/拖动/表情/下载/文档气泡状态机） |
| `ui\settings.html` | 设置窗口（540px 宽） |
| `ui\docs.html` `ui\docs.js` `ui\docs.css` | 旧版独立「文档工具窗口」页面，**当前已不再打开**（逻辑已并入 `renderer.js`），保留待清理 |
| `ui\vendor\emotion-ball\` | emotion-ball 表情引擎（rings / emotions / ball / engine，原样引入） |
| `src-tauri\src\lib.rs` | Rust 主进程：窗口与可见区域裁剪、全局鼠标轮询、拖动、下载、剪贴板、托盘与右键菜单、配置、文档命令注册 |
| `src-tauri\src\drop_target.rs` | OLE 拖放接收器（拖入文件 → 入队 → 通知悬浮球） |
| `src-tauri\src\docs\mod.rs` | 文档任务调度：格式分类矩阵、任务线程、进度/完成事件、输出目录 |
| `src-tauri\src\docs\` 其余模块 | `office_ops` / `image_ops` / `pdf_ops` / `pdf_render` / `pdf_text` / `pdf_font` / `pdf_stream` / `pdf_extract` / `pdf_write` / `legacy` / `from_pdf` / `office_write` / `to_pdf` / `text_ops` / `pptx_write` / `zip_min` / `doc_binary` |
| `src-tauri\tauri.conf.json` | 窗口/打包配置 |
| `tools\docs-smoke\` | 文档能力的端到端烟测与夹具 |
| `src-tauri\icons\` | 应用图标（托盘与窗口图标） |

## 实现要点

- **纯 Rust，不依赖本机 Office / WPS / LibreOffice / PDFium**：OOXML 与 CFB 自己解包取字，PDF 自己排版与栅格化，内置 Noto Sans SC 子集字体
- **旧版 `.doc`**：读 CFB 的 `WordDocument` / `0Table` / `1Table`，按 `PlcPcd` 分片取字，处理域代码、单元格与段落控制符后转结构块
- **文档交互全部在球上**：主窗口本身 420×620，但用 `SetWindowRgn` 只保留右下角可视区域，球的位置与原来一致；展开气泡时只放大可点击区域
- **事件投递**：文档进度 / 完成事件发往主窗口（`docs-progress` / `docs-done`），不再有独立的 `docs` 窗口
- **拖放安全**：文件拖入只在 `IDropTarget::Drop` 里入队，等 OLE 拖放循环退出后才通知前端，避免在拖放回调里同步驱动 WebView2 造成卡死
- **输出目录统一**：`resolve_output_dir()` 读配置的 `temp_dir`（默认 `%TEMP%\file-paste-float`），「打开临时文件夹」与「清理临时文件夹」对全部产物生效

## 已知限制

- 旧版二进制 **`.xls`（BIFF8）/ `.ppt`** 尚未接入，选这类文件会提示「没有可用的目标格式」
- `.doc` 不支持直接压缩；`.doc → docx` 的表格是制表符段落（但 `.doc → pdf` 是完整表格网格）
- `xlsx → pdf` 只输出第一张工作表；`pptx → pdf` 是「每页文字 + 图片 + 页码标题」的文本式版面；`pdf → xlsx` 只搬文字、不还原表格结构
- `docx` 表格跨页不会重复表头
- PDF 栅格化不解析 CFF/Type1 轮廓，`ICCBased` 一律按 RGB 处理
- 扫描件 / 无 ToUnicode 的 PDF 取不到文字时直接报错，需要先 OCR
- 内置字库只有 Noto Sans SC（无衬线），未内置衬线字库

## 配置文件位置

`%APPDATA%\com.desktop-pet-tool.float\config.json`，记录临时目录路径、置顶、自启、关闭行为等设置。
