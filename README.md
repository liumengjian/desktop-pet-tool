# desktop-pet-tool

中文 | [English](README.en.md)

一个 **Tauri** 桌面悬浮球：一边把在线文件链接变成可直接粘贴的文件，一边把拖进来的文档就地转换 / 压缩。
安装包约 **10.4 MB**（基于系统 WebView2，无内置浏览器内核；体积主要来自内置中文字库）。

## 功能

### 1. 悬浮球本体

- **米白圆形悬浮球**：桌面任意位置自由拖动，始终置顶，表情由 emotion-ball 引擎实时渲染（SVG + 原生 JS，零依赖）
- **左键点击**：唤醒 / 收起当前打开的气泡。**左键不再弹出输入框**，输入框只从右键菜单「文档复制」打开
- **右键菜单**（两个分区）：
  - 文档相关：**文档复制** / **文档工具**
  - 照片相关：**人脸比对** / **图片水印去除**
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

产物统一落到**临时文件夹**（默认 `%TEMP%\desktop-pet-tool`），与「文档复制」下载的文件同一个目录，右键「清理临时文件夹」可以一次清干净。

### 4. 人脸比对

右键「人脸比对」→ 球边浮出同样的对话气泡，**分两组上传照片**，把两组里的人一一配对，最后产出一个 Word：

1. **两个上传区**：`第一组` / `第二组`，各选多张照片（可逐个移除、可继续添加）；支持 `png / jpg / jpeg / bmp / tif / webp`，非图片会被拒收并提示
2. 点「开始比对」→ 球外圈进度环 + 球下百分比，气泡内显示当前阶段（准备中 / 下载 det_10g.onnx / 第一组 图片5.png / 生成 Word）
3. 完成后弹出结果气泡：配上了几对、几张没配上、Word 路径 + 「打开临时文件夹」按钮

匹配规则：

- **跨组一一对应**：第一组每张最多配第二组一张，第二组每张也最多被配一次，不会出现一张照片被重复使用；配对按全局最优分配（不是逐张取最大），相似度太低的组合宁愿不配
- **只看人脸**：检测 → 五点对齐 → 提特征 → 余弦相似度，**不做 OCR**，证件照上的姓名编号不参与匹配
- **本地 CPU 推理**：InsightFace ONNX，SCRFD `det_10g` 负责检测、ResNet50 `w600k_r50` 负责特征；线程数按 `min(4, CPU-1)` 自适应，几十张照片通常十几秒

模型下发：

- 模型与 onnxruntime 依赖**首次使用才下载**，落到 `%LOCALAPPDATA%\com.desktop-pet-tool.float\face\v1\`（`models\` + `deps\` + `marker.json`，约 233 MB），之后离线可用
- 下载地址在 `src-tauri\src\face\models.rs` 的 `BASE_URL`（当前 `.../tools/desktop-pet-tool/face-models/pack`），清单与 sha256 见 `tools\face-models\README.md`；要不要下载只看本地文件是否齐、sha256 是否对得上，`marker.json` 只记录缓存版本，方便排查
- 下载失败会带着原因退出，不会静默跑到一半才报错

Word 版式：

- 一个 Word（`人脸比对.docx`，落在临时文件夹），**每页只放一对照片**：左第一组、右第二组，居中并排，单张宽 ≤ 7.5 cm、高 ≤ 10.3 cm
- 页面里**没有标题、没有表格、没有文件名、没有相似度**，只有两张图
- 有没配上的照片时，**最后一页**用文字列出：`共配对 N 组。第一组 未匹配 x 张，第二组 未匹配 y 张，无法自动处理 z 张`，逐条给出文件名 + 最高分 / 跳过原因；全部配上就没有这一页

阈值调整：

- **能不能配上**由 `--low`（默认 **0.35**）决定，低于它的组合一律不分配
- 状态文案与「待人工复核」计数用 `--high`（默认 **0.45**）
- 两个参数都写在 `src-tauri\binaries\pdfconv\scripts\face_match.py` 的 argparse 默认值里，Rust 端不传，**改默认值即全局生效**；开发态该脚本是从仓库目录读的，改完直接重跑就生效，不用重新编译
- 实测参考：同一批样本里陌生人最高 0.11、真实配对 0.55–0.78，默认值留了很宽的间隔，一般不需要动

小脸处理：

- 检测视口固定 640（SCRFD 就是按 640 训练的，调高反而掉检测率）；原图人脸短边 < 112 px 时，按检测框外扩 2.5 倍裁脸再放大到 640 重检关键点，用放大图做对齐与提特征
- 短边 < 24 px 才判定为「人脸过小」直接跳过；112 px 以下的配对照常成立，只在结果 JSON 里带 `lowq` 标记（Word 不显示）

### 5. 图片水印去除

右键「图片水印去除」→ 球边浮出对话气泡，选一张图片后在画布上圈出水印，本地模型把选中区域的背景补全，产出一张新的 PNG：

1. **选图**：支持 `png / jpg / jpeg / bmp / tif / webp`，非图片会被拒收并提示；画布初始按容器完整显示，**光标停在画布内滚动滚轮可放大 / 缩小（0.4～4 倍）**，图片层与涂抹层同步缩放
2. **两种圈选方式**：**画笔**涂抹（可调笔刷粗细）或**矩形框选**；支持撤销、清空，笔画坐标按当前显示比例换算回原图
3. 点「开始去除」→ 球外圈进度环 + 球下百分比，气泡内显示当前阶段（准备模型 / 推理中）
4. 完成后弹出结果气泡：产物路径 +「打开临时文件夹」按钮；产物统一落到临时文件夹

修复要点：

- **本地 CPU 推理**：LaMa ONNX 图像修复模型（`inpainting_lama.onnx`，约 88 MB）；onnxruntime 复用人脸比对那一份运行时，不重复下载
- **模型首次使用才下载**，落到 `%LOCALAPPDATA%\com.desktop-pet-tool.float\watermark\v1\`（`models\` + `deps\` + `marker.json`），之后离线可用；deps 优先复用 `face\v1\deps`，人脸那套不在本地时才回退下载同一份 ort 包
- 处理流程：笔画重绘成 mask → 膨胀 2 次 + 高斯羽化 → 512×512 推理 → 仅在 mask 区域按羽化 alpha 回贴修复结果，区域外保持原图，避免边界生硬
- 模型清单与 sha256 见 `deploy\watermark-models\README.md`；也支持手工把 onnx 放进 `models\` 目录离线使用

## 界面语言

支持**中文（默认）**与**英文**两套界面：

- 在**设置 → 语言 / Language** 切换，立即生效，不需要点保存
- 主进程侧同步：右键菜单、托盘菜单、系统对话框标题、文档任务提示
- 前端侧同步：粘贴气泡、文档气泡、人脸比对气泡、去水印气泡、结果行、设置页
- 选择记录在 `config.json` 的 `lang`（`zh` | `en`）；旧配置文件没有这个字段时按中文处理

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
4. **要比对照片**：右键 →「人脸比对」→ 两组各上传照片 → 开始比对 → 完成后点「打开临时文件夹」取 Word
5. **要去除图片水印**：右键 →「图片水印去除」→ 选图 → 画笔涂抹或框选水印 → 开始去除 → 完成后点「打开临时文件夹」取 PNG

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
| `ui\i18n.js` | 中英文案字典、`t()` 占位替换、`data-i18n` 静态文案应用 |
| `ui\settings.html` | 设置窗口（540px 宽） |
| `ui\docs.html` `ui\docs.js` `ui\docs.css` | 旧版独立「文档工具窗口」页面，**当前已不再打开**（逻辑已并入 `renderer.js`），保留待清理 |
| `ui\vendor\emotion-ball\` | emotion-ball 表情引擎（rings / emotions / ball / engine，原样引入） |
| `src-tauri\src\lib.rs` | Rust 主进程：窗口与可见区域裁剪、全局鼠标轮询、拖动、下载、剪贴板、托盘与右键菜单、配置、文档命令注册 |
| `src-tauri\src\i18n.rs` | 主进程语言状态（`t(zh, en)`），服务菜单、托盘、对话框与文档提示 |
| `src-tauri\src\drop_target.rs` | OLE 拖放接收器（拖入文件 → 入队 → 通知悬浮球） |
| `src-tauri\src\docs\mod.rs` | 文档任务调度：格式分类矩阵、任务线程、进度/完成事件、输出目录 |
| `src-tauri\src\face\models.rs` | 人脸模型下发：COS 清单校验、下载、解包到 `face\v1`、`marker.json` |
| `src-tauri\src\face\mod.rs` | 人脸比对任务：两组校验、sidecar 线程（`catch_unwind`）、`face-progress` 事件、Word 产物 |
| `src-tauri\src\watermark\models.rs` | 去水印模型下发：COS 清单校验、下载、解包到 `watermark\v1`；ort 依赖优先复用 face 那一份 |
| `src-tauri\src\watermark\mod.rs` | 去水印任务：选图校验、sidecar 线程、`wm-progress` 事件、PNG 产物 |
| `src-tauri\binaries\pdfconv\scripts\face_match.py` | 比对 sidecar：检测/对齐/提特征/分配/写 Word，stdout 逐行输出 JSON 进度 |
| `src-tauri\binaries\pdfconv\scripts\watermark_erase.py` | 修复 sidecar：mask 重绘/膨胀羽化、LaMa 推理、羽化回贴，stdout 逐行输出 JSON 进度 |
| `tools\face-models\` | 模型分包脚本、manifest 与上传示例（供 COS 下发） |
| `deploy\watermark-models\` | 去水印模型 manifest 与上传说明（供 COS 下发） |
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
- **语言切换**：设置页写入配置后由 `config_set` 重建托盘菜单并向主窗口 / 设置窗口发 `config-changed`，前端就地重绘；主进程文案走 `i18n::t(中文, English)`
- **输出目录统一**：`resolve_output_dir()` 读配置的 `temp_dir`（默认 `%TEMP%\desktop-pet-tool`），「打开临时文件夹」与「清理临时文件夹」对全部产物生效

## 已知限制

- 旧版二进制 **`.xls`（BIFF8）/ `.ppt`** 尚未接入，选这类文件会提示「没有可用的目标格式」
- `.doc` 不支持直接压缩；`.doc → docx` 的表格是制表符段落（但 `.doc → pdf` 是完整表格网格）
- `xlsx → pdf` 只输出第一张工作表；`pptx → pdf` 是「每页文字 + 图片 + 页码标题」的文本式版面；`pdf → xlsx` 只搬文字、不还原表格结构
- `docx` 表格跨页不会重复表头
- PDF 栅格化不解析 CFF/Type1 轮廓，`ICCBased` 一律按 RGB 处理
- 扫描件 / 无 ToUnicode 的 PDF 取不到文字时直接报错，需要先 OCR
- 内置字库只有 Noto Sans SC（无衬线），未内置衬线字库
- 人脸比对只认正脸 / 近正脸：侧脸、遮挡、逆光、戴墨镜容易漏检；检测视口固定 640，整图放大并不能提升小脸召回，靠的是裁脸重检
- 人脸比对不做 OCR，证件照上的姓名编号不参与匹配；两组数量不等时，多出来的人进末页清单，不会被强行配对
- 图片水印去除靠选中区域周边的背景推断：大面积遮挡、纹理剧烈变化，或水印压在复杂图案上时，修补边缘可能留淡痕；CPU 首次加载模型约需数十秒

## 配置文件位置

`%APPDATA%\com.desktop-pet-tool.float\config.json`，记录临时目录路径、置顶、自启、关闭行为、界面语言（`lang`）等设置。
