# dpt

中文 | [English](README.en.md)

一个 **Tauri** 桌面悬浮球。
安装包约 **22 MB**（基于系统 WebView2，无内置浏览器内核；体积主要来自内置中文字库）。

## 功能

### 1. 悬浮球本体

- **米白圆形悬浮球**：桌面任意位置自由拖动，始终置顶，表情由 emotion-ball 引擎实时渲染（SVG + 原生 JS，零依赖）
- **应用图标**：深石墨圆角底 + 奶油色悬浮球 + 淡紫灰清扫轨道，无文字；同一套图形提供 16/24/32/48/64/128/256 多尺寸 ICO、512 px PNG 与 32 px 托盘图，小尺寸单独加粗五官，16 px 下仍可辨认
- **左键点击**：唤醒 / 收起当前打开的气泡。**左键不再弹出输入框**，输入框只从右键菜单「文档复制」打开
- **右键菜单**（两个分区）：
  - 工具区：**文档复制** / **文档工具** / **文件查找** / **人脸比对** / **水印去除** / **系统工具**（二级菜单：垃圾清理 / C 盘清理 / 大文件清理）
  - 程序区：**打开临时文件夹** / **清理临时文件夹** / **开机自启动** / **设置** / **退出**
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

产物统一落到**临时文件夹**（默认 `%TEMP%\desktop-pet-tool`）：点「打开临时文件夹」会打开该目录并**直接选中本次生成的文件**；它与「文档复制」下载的文件是同一个目录，右键「清理临时文件夹」可以一次清干净。

### 4. 文件查找

右键「文件查找」→ 球边浮出气泡，按文件名模糊匹配，几秒钟内把本机或共享盘上的相关文件列出来：

1. **来源文件夹（可选）**：留空 = 只搜本机磁盘（固定磁盘 + U 盘）；点「选择」挑一个文件夹，或直接填写路径，就只搜这个目录（本机文件夹、已映射网络盘、`\\服务器\共享` 都行；只填服务器名如 `\\DS423` 时会自动枚举该服务器上的所有磁盘共享，逐个搜索）
2. **关键词**：输入文件名的一部分即可，不区分大小写；用空格分隔多个关键词时要求全部出现
3. **结果排序**：前缀命中 → 包含 → 多关键词全命中 → 顺序模糊（`年度报告` 能找到 `2024年度总结报告`）
4. **结果行**：文件名 + 所在目录 + 大小 + 修改日期；点一行或「定位」在资源管理器里选中它，点「打开」用系统默认程序打开
5. **可控**：最多列 300 条（到上限立刻停止扫描，不会越扫越久）、跳过系统目录与 junction、扫描中随时可停；扫完显示扫描数量与耗时

查找只读文件系统，不移动、不改名、不删除任何文件。

### 5. 人脸比对

右键「人脸比对」→ 球边浮出同样的对话气泡，**分两组上传照片**，把两组里的人一一配对，最后产出一个 Word：

1. **两个上传区**：`第一组` / `第二组`，各选多张照片（可逐个移除、可继续添加）；支持 `png / jpg / jpeg / bmp / tif / webp`，非图片会被拒收并提示
2. 点「开始比对」→ 球外圈进度环 + 球下百分比，气泡内显示当前阶段（准备中 / 下载 det_10g.onnx / 第一组 图片5.png / 生成 Word）
3. 完成后弹出结果气泡：配上了几对、几张没配上、Word 路径 + 「打开临时文件夹」按钮

匹配规则：

- **跨组一一对应**：第一组每张最多配第二组一张，第二组每张也最多被配一次，不会出现一张照片被重复使用；配对按全局最优分配（不是逐张取最大），相似度太低的组合宁愿不配
- **只看人脸**：检测 → 五点对齐 → 提特征 → 余弦相似度，**不做 OCR**，证件照上的姓名编号不参与匹配
- **本地 CPU 推理**：InsightFace ONNX，SCRFD `det_10g` 负责检测、ResNet50 `w600k_r50` 负责特征；线程数按 `min(4, CPU-1)` 自适应，几十张照片通常十几秒

模型下发：

- 模型与 onnxruntime 依赖**首次使用才下载**，落到 `<安装目录>\data\face\v1\`（例如 `D:\tools\dpt\data\face\v1\`，`models\` + `deps\` + `marker.json`，约 233 MB），之后离线可用
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

### 6. 图片水印去除

右键「水印去除」→ 球边浮出对话气泡，选一张图片后在画布上圈出水印，本地模型把选中区域的背景补全，产出一张新的 PNG：

1. **选图**：支持 `png / jpg / jpeg / bmp / tif / webp`，非图片会被拒收并提示；画布初始按容器完整显示，**光标停在画布内滚动滚轮可放大 / 缩小（0.4～4 倍）**，图片层与涂抹层同步缩放
2. **两种圈选方式**：**画笔**涂抹（可调笔刷粗细）或**矩形框选**；支持撤销、清空，笔画坐标按当前显示比例换算回原图
3. 点「开始去除」→ 球外圈进度环 + 球下百分比，气泡内显示当前阶段（准备模型 / 推理中）
4. 完成后弹出结果气泡：产物路径 +「打开临时文件夹」按钮；产物统一落到临时文件夹

修复要点：

- **本地 CPU 推理**：LaMa ONNX 图像修复模型（`inpainting_lama.onnx`，约 88 MB）；onnxruntime 复用人脸比对那一份运行时，不重复下载
- **模型首次使用才下载**，落到 `<安装目录>\data\watermark\v1\`（例如 `D:\tools\dpt\data\watermark\v1\`，`models\` + `deps\` + `marker.json`），之后离线可用；deps 优先复用 `data\face\v1\deps`，人脸那套不在本地时才回退下载同一份 ort 包
- 处理流程：笔画重绘成 mask → 膨胀 2 次 + 高斯羽化 → 512×512 推理 → 仅在 mask 区域按羽化 alpha 回贴修复结果，区域外保持原图，避免边界生硬
- 模型清单与 sha256 见 `deploy\watermark-models\README.md`；也支持手工把 onnx 放进 `models\` 目录离线使用

### 7. 磁盘清理

右键「系统工具 → 垃圾清理 / C 盘清理 / 大文件清理」→ 打开 **1000×760 独立窗口**（可缩放），三个标签页共用同一套后端，扫描与清理互不阻塞主球。

**垃圾清理**

- **15 类清理项**：用户临时文件、Windows 临时目录、Windows 更新缓存、传递优化缓存、缩略图与图标缓存、显卡着色器缓存、崩溃转储、Windows 错误报告、组件日志、系统网络缓存、浏览器缓存（Chrome / Edge，默认不勾）、回收站（默认不勾）、旧驱动、卸载残留、注册表冗余
- 先**只读扫描**给出体积与条数，勾选后才动手；每条可展开看明细（路径 + 体积）
- 旧驱动：`pnputil /enum-drivers` 枚举，排除被 `HKLM\SYSTEM\...\Class` 引用的在用驱动，删除前用 `pnputil /export-driver` 备份到 `%APPDATA%\com.desktop-pet-tool.float\cleanup-backup\drivers-<时间戳>\`
- 卸载残留：对比三处 Uninstall 注册表根与 Program Files / ProgramData 一级目录，列出没有卸载器、仍占空间的疑似残留，清理时**移入回收站**（可恢复）
- 注册表冗余：只处理图标与安装目录都指向不存在路径、且不含 `msiexec` 的卸载项；删前 `reg export` 到 `cleanup-backup\reg-<时间戳>-N.reg`，并用 `RegDeleteTreeW` 限定在 `...\CurrentVersion\Uninstall\` 子树
- 回收站、卸载残留、注册表冗余、旧驱动**四项强制二次确认**，弹窗列出体积、条数或前几个路径
- 回收站清空走 PowerShell `Clear-RecycleBin -DriveLetter …`，逐盘执行

**大文件清理**

- 勾选磁盘 + 阈值（100 MB / 300 MB / 500 MB / 1 GB），**有界并发**扫描（默认 2 线程，上限 8，按 CPU 核数自适应），不跟随目录联接，跳过 `C:\Windows`、`$Recycle.Bin`、`System Volume Information`、`Recovery`、`Boot`
- 保护 `pagefile.sys` / `hiberfil.sys` / `swapfile.sys` / `memory.dmp` 等系统文件，不会出现在删除列表
- 结果按体积降序，显示路径、大小、修改时间，支持逐项勾选、全选、「打开位置」；删除只接受**本次扫描结果里出现过的路径**并再次校验可写性
- 后端最多保留 4000 条，界面渲染前 500 条，避免超大结果集拖慢界面

**C 盘专清**

- **空间去哪了**：按一级目录聚合并显示条形占比（`C:\Users\<用户>` 自动再深一层），点目录继续下钻、可返回上一级，直接回答“C 盘被什么占用”；C 盘根目录直属的系统文件（`pagefile.sys`、`hiberfil.sys`、`swapfile.sys` 等）单独归为只读的「根目录系统文件」，不当作可下钻目录
- **系统级清理**：休眠文件、虚拟内存、Windows.old、回收站、系统还原点、组件存储六项；统计体积后按需执行
  - 休眠文件：`powercfg -h off`（会同时关闭休眠功能）
  - Windows.old：`takeown` + `icacls` + `rd` 整体删除
  - 还原点：`vssadmin delete shadows /for=C: /all /quiet`
  - 组件存储：进入 C 盘页后自动调用 `DISM /AnalyzeComponentStore` 读取实际占用（需管理员，通常几十秒），卡片显示实际大小；清理走 `dism /online /cleanup-image /startcomponentcleanup`
  - 虚拟内存只展示占用，不提供删除
- **软件缓存数据清理**：扫描后按应用列出所有缓存，识别微信、QQ、企业微信、飞书、钉钉、WPS、剪映 / CapCut、Premiere、Photoshop、npm、conda、pip、JetBrains、Visual Studio、Codex、Claude、豆包工作、WorkBuddy 等 20+ 类
  - 每个缓存项分「7 天前 / 30 天前 / 90 天前 / 半年前 / 更早」五档，可分别勾选清理；扫描时固定时间基准，清理不会误删扫描后新生成的文件
  - 只清规则里写死的缓存、日志、临时目录，**不碰聊天数据库、会话和工程目录**
  - 微信 / QQ / 企业微信的聊天图片、视频、接收文件，WPS 自动备份，Premiere / Photoshop 自动保存属于历史内容，标记为**高风险、默认不勾选**，删除后历史消息或恢复能力会受影响
  - 删除前二次确认，只删所选时间档内、非目录联接的普通文件，并保护 `.db` / `.sqlite` / `.db-wal` 等数据库文件；应用正在占用时删不掉会计入失败
  - 微信 4.x 会从 `%APPDATA%\Tencent\xwechat\config\*.ini` 读取自定义数据盘（如 `D:\data`），再扫描其中的 `xwechat_files`；其它软件仍按常见默认路径配置，软件未安装或改过数据目录时，对应缓存不会出现在结果中
- **应用迁移**：列出安装在 C 盘的应用（名称 / 版本 / 安装路径 / 目录体积），选择目标磁盘后可一键迁移
  - `robocopy /E /MOVE` 把目录搬到 `<目标盘>:\AppMoved\<目录名>` → 删空源目录 → `cmd /c mklink /J` 在原路径建**目录联接**，应用仍从原路径启动
  - 任一步失败会把内容搬回原路径并保留原目录，不会留在半迁移状态
  - 迁移前校验：目标盘必须是固定磁盘、不是 C 盘、剩余空间 ≥ 目录体积 + 512 MB
- 系统级清理与应用迁移需要管理员权限；窗口右上角检测到普通权限时会提示，可点「以管理员重启」直接以管理员身份重新拉起

进度通过 `cleanup-progress` 事件推送到窗口（`{task, files, bytes}` 或 `{task, stage}`，节流 180 ms），扫描可随时取消。

### 8. 文件粉碎

安装后自动在当前用户的资源管理器右键菜单里加入「文件粉碎」（HKCU，不需要管理员权限）：

- 在资源管理器里**选中任意文件或文件夹 → 右键 → 文件粉碎**，悬浮球会弹出确认气泡，列出名称、体积与文件数（Windows 11 的折叠式右键菜单需先点「显示更多选项」）
- 气泡里默认勾选「被占用时结束占用进程」，点「开始粉碎」才真正执行；右键本身**绝不会直接删东西**
- 粉碎流程：逐个文件用随机数据整文件覆盖（1 MiB 块，xorshift64 填充）→ 删除文件 → 目录按层级从深到浅删除；写之前清掉只读 / 隐藏 / 系统属性
- 删除失败时用 Restart Manager 找出占用进程，经确认后结束它们（跳过 `explorer`、`lsass`、`svchost` 等系统关键进程与本程序自身），等待句柄释放后重试；仍失败且当前是管理员时用 `MoveFileExW(MOVEFILE_DELAY_UNTIL_REBOOT)` 登记「重启后删除」
- 拒绝粉碎磁盘根目录与 `C:\Windows` 本身；符号链接 / 目录联接只删链接本身，不会顺着删到别处；单次最多展开 20 万条
- 多选文件时资源管理器会为每一项各拉起一次进程，靠单实例互斥量 + 命名管道把这些请求合并到已在运行的悬浮球里，不会堆出多个球
- 进度按 120 ms 节流推送到气泡，可随时取消；完成后显示释放体积、被结束的进程与未能删除的条目
- 菜单文字跟随界面语言（`文件粉碎` / `Shred file` / `Shred folder`），切换语言后立即改注册表并刷新资源管理器；卸载时由 NSIS 钩子删除同名注册表键

## 界面语言

支持**中文（默认）**与**英文**两套界面：

- 在**设置 → 语言 / Language** 切换，立即生效，不需要点保存
- 主进程侧同步：右键菜单、托盘菜单、系统对话框标题、文档任务提示、资源管理器「文件粉碎」菜单文字
- 前端侧同步：粘贴气泡、文档气泡、文件查找气泡、人脸比对气泡、去水印气泡、文件粉碎气泡、结果行、设置页
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
4. **要查找文件**：右键 →「文件查找」→ 输入文件名关键词 → 来源文件夹留空只搜本机，或点「选择」指定目录/共享盘 → 开始查找 → 点结果行的「定位」在资源管理器里选中，或「打开」直接打开
5. **要比对照片**：右键 →「人脸比对」→ 两组各上传照片 → 开始比对 → 完成后点「打开临时文件夹」取 Word
6. **要去除图片水印**：右键 →「水印去除」→ 选图 → 画笔涂抹或框选水印 → 开始去除 → 完成后点「打开临时文件夹」取 PNG
7. **要清磁盘**：右键 →「系统工具」→「垃圾清理」/「C 盘清理」/「大文件清理」→ 在清理窗口里扫描、勾选、确认后清理；C 盘清理还能看空间占用、做系统级清理、按时间档清软件缓存，以及把应用迁到其他盘

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

产物位置：`src-tauri\target\release\bundle\nsis\dpt_<version>_x64-setup.exe`（约 10.4 MB，NSIS 向导式安装，可选安装目录）

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
| `ui\cleanup.html` `ui\cleanup.css` `ui\cleanup.js` | 磁盘清理窗口：垃圾清理 / 大文件 / C 盘专清三个标签页 |
| `src-tauri\src\cleanup\mod.rs` | 清理模块入口：任务状态、`cleanup-progress` 事件、磁盘/权限/路径工具、PowerShell `-EncodedCommand` 执行器、全部清理命令 |
| `src-tauri\src\cleanup\walk.rs` | 有界并发遍历：大小统计、大文件收集、目录聚合，支持取消、修改时间/后缀过滤、不跟随联接 |
| `src-tauri\src\cleanup\junk.rs` | 15 条垃圾规则、扫描与清理（回收站、卸载残留、旧驱动、注册表入口） |
| `src-tauri\src\cleanup\reg.rs` | 注册表读取、卸载残留、冗余项、旧驱动枚举/备份/删除、C 盘应用清单 |
| `src-tauri\src\cleanup\large.rs` | 大文件扫描与受控删除 |
| `src-tauri\src\cleanup\cdrive.rs` | C 盘空间分析、系统级清理动作、应用体积统计与迁移 |
| `src-tauri\src\cleanup\cache.rs` | 软件缓存规则、按应用与时间档扫描、高风险聊天媒体标记与受控清理 |
| `ui\docs.html` `ui\docs.js` `ui\docs.css` | 旧版独立「文档工具窗口」页面，**当前已不再打开**（逻辑已并入 `renderer.js`），保留待清理 |
| `ui\vendor\emotion-ball\` | emotion-ball 表情引擎（rings / emotions / ball / engine，原样引入） |
| `src-tauri\src\lib.rs` | Rust 主进程：窗口与可见区域裁剪、全局鼠标轮询、拖动、下载、剪贴板、托盘与右键菜单、配置、文档与磁盘清理命令注册 |
| `src-tauri\src\i18n.rs` | 主进程语言状态（`t(zh, en)`），服务菜单、托盘、对话框与文档提示 |
| `src-tauri\src\drop_target.rs` | OLE 拖放接收器（拖入文件 → 入队 → 通知悬浮球） |
| `src-tauri\src\docs\mod.rs` | 文档任务调度：格式分类矩阵、任务线程、进度/完成事件、输出目录 |
| `src-tauri\src\face\models.rs` | 人脸模型下发：COS 清单校验、下载、解包到 `face\v1`、`marker.json` |
| `src-tauri\src\face\mod.rs` | 人脸比对任务：两组校验、sidecar 线程（`catch_unwind`）、`face-progress` 事件、Word 产物 |
| `src-tauri\src\watermark\models.rs` | 去水印模型下发：COS 清单校验、下载、解包到 `watermark\v1`；ort 依赖优先复用 face 那一份 |
| `src-tauri\src\watermark\mod.rs` | 去水印任务：选图校验、sidecar 线程、`wm-progress` 事件、PNG 产物 |
| `src-tauri\src\filesearch\mod.rs` | 文件查找任务：磁盘/共享盘枚举、后台扫描线程、`fs-batch` / `fs-progress` / `fs-done` 事件、定位与打开 |
| `src-tauri\src\filesearch\engine.rs` | 文件查找纯逻辑：关键词评分、共享路径规范化、跳过目录与深度优先遍历（不依赖 tauri，可单独跑单测） |
| `src-tauri\src\single.rs` | 单实例互斥量 + 命名管道 IPC：把右键多选拉起的多个进程合并到一个悬浮球，`--fetch-pdf-deps` / `--elevated` 例外 |
| `src-tauri\src\shred\mod.rs` | 文件粉碎入口：路径入队/取队、确认页测量、`shred-progress` / `shred-done` 事件、右键菜单自检 |
| `src-tauri\src\shred\engine.rs` | 粉碎引擎：遍历统计、随机覆盖写、删除与失败重试、重启后删除、进度节流（含单测） |
| `src-tauri\src\shred\shell.rs` | 注册表注册 / 删除资源管理器右键动词（`HKCU\Software\Classes\*\shell` 与 `Directory\shell`），`SHChangeNotify` 刷新 |
| `src-tauri\src\shred\lock.rs` | Restart Manager 占用检测与强制结束进程（跳过系统关键进程） |
| `src-tauri\binaries\pdfconv\scripts\face_match.py` | 比对 sidecar：检测/对齐/提特征/分配/写 Word，stdout 逐行输出 JSON 进度 |
| `src-tauri\binaries\pdfconv\scripts\watermark_erase.py` | 修复 sidecar：mask 重绘/膨胀羽化、LaMa 推理、羽化回贴，stdout 逐行输出 JSON 进度 |
| `tools\face-models\` | 模型分包脚本、manifest 与上传示例（供 COS 下发） |
| `deploy\watermark-models\` | 去水印模型内置清单与上传说明（COS 只传 onnx） |
| `src-tauri\src\docs\` 其余模块 | `office_ops` / `image_ops` / `pdf_ops` / `pdf_render` / `pdf_text` / `pdf_font` / `pdf_stream` / `pdf_extract` / `pdf_write` / `legacy` / `from_pdf` / `office_write` / `to_pdf` / `text_ops` / `pptx_write` / `zip_min` / `doc_binary` |
| `src-tauri\tauri.conf.json` | 窗口/打包配置 |
| `tools\docs-smoke\` | 文档能力的端到端烟测与夹具 |
| `src-tauri\icons\` | 应用图标：`icon.ico` 多尺寸（16/24/32/48/64/128/256）、`icon.png`（512 px）、托盘 `tray.png`（32 px） |

## 实现要点

- **纯 Rust，不依赖本机 Office / WPS / LibreOffice / PDFium**：OOXML 与 CFB 自己解包取字，PDF 自己排版与栅格化，内置 Noto Sans SC 子集字体
- **旧版 `.doc`**：读 CFB 的 `WordDocument` / `0Table` / `1Table`，按 `PlcPcd` 分片取字，处理域代码、单元格与段落控制符后转结构块
- **文档交互全部在球上**：主窗口本身 420×620，但用 `SetWindowRgn` 只保留右下角可视区域，球的位置与原来一致；展开气泡时只放大可点击区域
- **事件投递**：文档进度 / 完成事件发往主窗口（`docs-progress` / `docs-done`），不再有独立的 `docs` 窗口
- **拖放安全**：文件拖入只在 `IDropTarget::Drop` 里入队，等 OLE 拖放循环退出后才通知前端，避免在拖放回调里同步驱动 WebView2 造成卡死
- **语言切换**：设置页写入配置后由 `config_set` 重建托盘菜单并向主窗口 / 设置窗口发 `config-changed`，前端就地重绘；主进程文案走 `i18n::t(中文, English)`
- **输出目录统一**：`resolve_output_dir()` 读配置的 `temp_dir`（默认 `%TEMP%\desktop-pet-tool`）；「打开临时文件夹」会打开目录并选中本次产物，「清理临时文件夹」对全部产物生效

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
- 旧驱动枚举依赖系统 `pnputil`：非管理员只能看到「需要管理员权限」提示，无法列出候选
- 文件粉碎的「覆盖写」是软件层擦除：SSD 的 TRIM / 磨损均衡、云同步的历史版本、文件系统的残留副本都无法由应用彻底抹掉；只是删不掉的系统文件（多为需要管理员权限的）会登记为「重启后删除」，重启前仍在原处
- 应用迁移对带服务、驱动或自更新的应用可能失败；部分应用检测到路径变化会拒绝启动。迁移前请先退出该应用
- C 盘空间分析要逐文件遍历，文件多或机械盘时需要几分钟，可随时取消；只统计一级目录（`Users\<用户>` 到二级），更深的占用可继续点目录下钻
- DISM 组件清理可能需要 5–30 分钟，期间窗口要保持在打开状态

## 配置文件位置

`%APPDATA%\com.desktop-pet-tool.float\config.json`，记录临时目录路径、置顶、自启、关闭行为、界面语言（`lang`）等设置。目录名沿用 identifier `com.desktop-pet-tool.float`，不随显示名 `dpt` 改变。
