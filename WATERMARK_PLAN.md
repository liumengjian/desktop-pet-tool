# 图片去水印功能 · 落地方案（待确认后动工）

> 参考项目：https://github.com/Sanster/IOPaint （图像修复 / erase 工具）
> 状态：方案设计阶段，**尚未改动任何现有源码**。确认路线与入口后再实现。

## 1. 背景与目标

给悬浮球工具增加「图片去水印」能力：选一张图 → 在图上涂掉 / 框选水印区域 → AI 修复补全背景 → 产物落到统一临时文件夹。

IOPaint 的原理是 **mask（待抹除区域）+ erase 模型填充**，本身不提供"一键自动去水印"；它的完整安装要 `torch`、diffusers、transformers 一整套（CPU wheel 约 200MB+），且主仓库已归档。因此**不直接套完整 IOPaint**，只复用它的「mask + erase」思路，按本项目既有 `face`（人脸比对）的隔离 sidecar 模式落地。

## 2. 选型结论（默认方案，待拍板）

| 项 | 结论 | 理由 |
|---|---|---|
| 推理路线 | **B：onnxruntime + 轻量 erase ONNX 模型** | 复用 `face/models.rs` 已跑通的「首用下载 + 哈希校验 + LocalAppData 缓存」链路；不装 torch，不跑常驻本地服务，进程跑完即退 |
| 首版模型 | **MI-GAN ONNX（约 30MB，MIT）** | 体积最小、资源最省；Picsart AI Research，ICCV 2023，作者本人导出过 ONNX；MIT 可商用，保留署名即可 |
| 备选模型 | **LaMa ONNX（Apache-2.0，体积更大）** | IOPaint 推荐档，复杂背景效果更好；权重需随包带 `LICENSE-LAMA.txt` / NOTICE，二期作为可选档位 |
| mask 方式 | **手动涂抹 + 矩形框选**（一期） | IOPaint 同款，可控通用；自动检测角落 / 半透明水印准确率有限，列二期 |
| 处理粒度 | **一期单图** | 每张图都要手动涂区域，批量需逐张交互，二期再议 |

### 不做的事（一期边界）

- 不安装 torch / diffusers / 完整 iopaint 包，不起常驻 8080 服务。
- 不做自动水印定位、不做扩散式"替换物体"、不做超分 / 人脸修复。
- 不改已有转换 / 压缩 / 人脸比对链路。

## 3. 总体架构（对照 `face` 模块照搬）

```
前端 watermark 气泡（canvas 涂抹 → mask PNG）
   │  invoke wm_start { image, mask, model? }
后端 watermark 模块（命令 / 状态 / 进度 / 取消）
   │  models::ensure()  下载或复用 ONNX 模型 + onnxruntime 依赖
   │  Command 拉起内嵌 python.exe 跑 watermark_erase.py
sidecar Python（复用 binaries/pdfconv/python 运行时）
   │  读 image + mask → onnxruntime 推理 → 写产物 PNG
产物 → 统一临时目录（docs::resolve_output_dir）
```

进程级隔离：推理在独立 Python 子进程，跑完退出，不占 Tauri 主进程内存（与 `face` 注释里的考量一致）。

## 4. 后端改动清单

### 4.1 新增文件

| 路径 | 作用 | 对照 |
|---|---|---|
| `src-tauri/src/watermark/mod.rs` | 命令、状态、任务调度、sidecar 拉起、进度 / 取消 / 产物 | `face/mod.rs` |
| `src-tauri/src/watermark/models.rs` | erase ONNX 模型 + onnxruntime 依赖的清单下载、sha256 校验、缓存、状态查询 | `face/models.rs` |
| `binaries/pdfconv/scripts/watermark_erase.py` | 推理脚本：读参、预处理、ONNX 推理、后处理、stdout 行协议 | `scripts/face_match.py` |

### 4.2 修改文件（均为挂载点，不改既有逻辑）

| 文件 | 改动 |
|---|---|
| `src-tauri/src/lib.rs` | `mod watermark;`；`MenuHandles` 加 `watermark` 句柄；`build_menu` 加菜单项；`TOOL_MENU_IDS` 加 `"watermark"`；`run_tool_menu_item` 分支 `open_watermark`（emit `wm-open`）；`manage(watermark::WatermarkState::default())`；`generate_handler!` 注册命令 |
| `src-tauri/src/i18n.rs` | 主进程文案走现有 `i18n::t(中文, English)`，无需新机制 |

### 4.3 命令（命名沿用 `face_` 前缀风格，用 `wm_` 前缀）

| 命令 | 入参 | 返回 | 说明 |
|---|---|---|---|
| `wm_pick` | — | `Vec<PickItem>` | 弹出图片选择框（png/jpg/jpeg/bmp/tif/tiff/webp） |
| `wm_inspect` | `paths: Vec<String>` | `Vec<PickItem>` | 校验存在性 / 格式 / 大小，过滤非法项 |
| `wm_status` | — | `(bool, String)` | 不触发下载：模型与依赖是否就绪、缓存目录（前端提示"首次需下载"） |
| `wm_start` | `job: { image: String, mask: String, model?: String }` | `Result<(), String>` | 起后台线程跑任务；并发占用校验、取消标志 |
| `wm_cancel` | — | — | 置取消标志，子进程终止 |
| `wm_open_outdir` | — | `Result<(), String>` | 打开统一临时文件夹 |

### 4.4 事件（发到 `main` 窗口）

- `wm-progress`：`{ percent, label, name }`，进度环复用现有球体进度通道。
- `wm-done`：`{ ok, message, outdir, outputs: [{ path, name, before, after, note }] }`。
- `wm-open`：后端 → 前端弹出去水印气泡。

### 4.5 任务流程（`run_job`）

1. `models::ensure` 确保模型 + onnxruntime 就绪（下载进度映射到前 20%，推理 20%–99%）。
2. 定位内嵌 Python（复用 `sidecar_pdf::resolve_root`）与外置 site-packages（复用 `sidecar_deps::ensure`，已有 cv2 / numpy）。
3. 把 `image`、`mask`、模型路径、产物路径作为参数传给 `watermark_erase.py`；环境变量传 `PDFCONV_SITE_PACKAGES`、`WM_DEPS`（onnxruntime 叠加目录），`env_remove("PYTHONPATH")`。
4. stdout 逐行解析 JSON 进度行 / 终行；stderr 留尾部用于报错；超时常量参照 `face`（单图推理给足余量）。
5. 产物非空校验后入 `outputs`；失败清理半成品临时文件。

## 5. 前端改动清单

### 5.1 新增 / 修改

| 文件 | 改动 |
|---|---|
| `ui/index.html` | 新增 `watermarkBubble` 面板：图片预览 / canvas 画布、画笔半径、撤销、清空、选择图片、开始按钮、运行中进度行、结果行、打开目录按钮（结构对照 `faceBubble`） |
| `ui/styles.css` | 画布、工具条、结果行样式（复用现有气泡 / chips / 进度环样式变量） |
| `ui/renderer.js` | `watermark` 独立状态机；监听 `wm-open`；canvas 涂抹 / 框选 → 导出 mask PNG 到临时目录；调 `wm_start`；接 `wm-progress` / `wm-done`；取消、打开目录、收起逻辑与 `face` 一致 |
| `ui/i18n.js` | `wm.*` 中英文案（标题、选择图片、涂抹提示、开始、取消、首次下载提示、失败 / 完成等），`zh` / `en` 两套同步补齐 |

### 5.2 画布交互要点

- canvas 按图片原始比例等比缩放显示，坐标映射回原图尺寸。
- 画笔：圆形软边涂抹；辅助矩形框选；可撤销上一笔、可清空重涂。
- mask 导出：白色 = 待修复区域，黑色 = 保留；与原图同尺寸 PNG，写到临时目录后把路径随 `wm_start` 提交。
- 未涂任何区域时「开始」按钮置灰并提示先涂抹。

### 5.3 入口（二选一，待确认）

- **方案 ①**：右键菜单新增独立项「去水印」，与「文档工具 / 人脸比对」同级（`face` 同款入口，推荐）。
- **方案 ②**：放进文档气泡作为第三个动作卡（与「转换 / 压缩」并列），拖入图片后选择。

> 按项目约定：新增页面必须配套对应 HTML/CSS/JS/i18n 资源；未确认前不预先新增按钮资源。

## 6. 模型与依赖下发（复用 `face/models.rs` 机制）

缓存布局（LocalAppData，随 bundleid 目录卸载清理）：

```
watermark/<TAG>/models/migan.onnx          # 首版 erase 模型
watermark/<TAG>/deps/onnxruntime/...       # onnxruntime 独立叠加目录
watermark/<TAG>/marker.json
```

- `manifest.json` 托管在自有 COS（沿用 `face-models` 同桶路径模式），条目含 `name / sha256 / bytes`。
- 下载走临时 `.part` → 完成后原子改名 → sha256 校验 → 关键文件齐全校验 → 写 marker；损坏只清半成品，不污染已提交缓存。
- **onnxruntime 依赖去重**：若 `face` 已下载过 onnxruntime 叠加目录，直接复用，不再重复下载（实现时先探测 `face/<TAG>/deps`）。
- 支持手工放置：把 onnx 放进 `models` 目录即可离线使用，不走网络。
- 模型标签 `TAG`：换模型 / 换运行时就换新标签，旧缓存不混用。

## 7. sidecar 脚本契约（`watermark_erase.py`）

入参（命令行参数，对照 `face_match.py`）：

```
--src <原图路径>
--mask <mask PNG 路径>
--out <产物路径>
--model <onnx 模型路径>
[--model-name migan|lama]
```

环境变量：`PDFCONV_SITE_PACKAGES`（cv2/numpy）、`WM_DEPS`（onnxruntime 叠加目录，插入 `sys.path`）。

stdout 行协议（每行一个 JSON）：

- 进度行：`{"t":"p","pct":0.0-1.0,"label":"...","name":"..."}`
- 终行：`{"t":"done","status":"ok","out":"..."}` 或 `{"t":"done","status":"error","message":"..."}`

预处理 / 后处理约定：

- mask 转单通道二值；与原图对齐尺寸；必要时按模型要求长边限制（避免 CPU 大图过慢，建议 ≤1600 长边预处理，产物回贴原尺寸）。
- 输出 RGB PNG，产物非空由后端二次校验。
- 脚本内只做推理，不弹窗口、不联网；模型路径由后端传入。

## 8. 验收用例（可挂到 `tools/docs-smoke` 同类烟测思路）

1. 选一张带角落文字水印的图 → 涂抹水印 → 产物对应区域被背景补全，无明显残留色块。
2. 未涂抹时开始按钮置灰；取消任务能中断子进程并回收。
3. 首次使用：缺模型时进度条走下载 → 校验 → 推理；二次进入直接复用缓存。
4. 非图片 / 损坏图 / 超 2GB 被拦截并给出提示。
5. 中英文案切换后，菜单、气泡、进度、结果提示同步生效。
6. 产物出现在统一临时文件夹，「打开临时文件夹」可见，「清理临时文件夹」可清。

## 9. 风险与限制

| 风险 | 说明 / 对策 |
|---|---|
| ONNX 张量接口差异 | MI-GAN / LaMa 的输入输出通道、归一化、pad 倍数不同；落地脚本时先在 Python 侧核对张量定义，预处理按模型分别适配 |
| CPU 推理速度 | 大图偏慢；预处理限长边，进度条给足超时；后续可加 GPU/DirectML provider 探测（二期） |
| 模型许可证 | MI-GAN MIT（保留署名）；LaMa Apache-2.0（需带 LICENSE/NOTICE）；打包前复核各自 LICENSE 文本 |
| 涂抹边界 | mask 边缘建议做轻微模糊（IOPaint 的 Mask Blur 思路），避免修复边界生硬 |
| 复杂纹理 | 大面积水印 / 强纹理背景仍可能修补不自然，属模型能力上限，结果如实提示 |
| 离线环境 | 模型首用必须联网下载；已下载后离线可用；支持手工放置模型目录 |

## 10. 待确认事项（动工前需要你拍板）

1. **路线**：是否确认 B（onnxruntime + 轻量 ONNX）？若希望零新依赖离线即用，可退回 A（cv2.inpaint 传统修复，效果一般）。
2. **首版模型**：MI-GAN（小、MIT）还是直接上 LaMa（效果好、更大）？
3. **入口**：右键菜单独立项（推荐）还是文档气泡第三个动作卡？
4. **mask 交互**：一期只要画笔涂抹，还是画笔 + 矩形框选都要？
5. **onnxruntime 复用**：是否同意复用 `face` 已下载的 onnxruntime 依赖目录（不重复下）？

## 11. 参考实现位置（本项目内对照）

- 后端隔离任务：`src-tauri/src/face/mod.rs`
- 模型下载 / 校验 / 缓存：`src-tauri/src/face/models.rs`
- Python sidecar 拉起：`src-tauri/src/docs/sidecar_pdf.rs`、`src-tauri/src/docs/sidecar_deps.rs`
- 内嵌运行时与脚本目录：`src-tauri/binaries/pdfconv/python/`、`src-tauri/binaries/pdfconv/scripts/`
- 前端气泡状态机：`ui/renderer.js`（face 段）、`ui/index.html`（faceBubble）、`ui/i18n.js`（`face.*`）
