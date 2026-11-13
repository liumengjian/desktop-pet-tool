# PDF→Word 依赖外置 OSS 清单（方案 B）

> 原则：**整个 `site-packages` 打成一个压缩包外置**，不按单个库拆分（库之间有二进制依赖，拆散容易漏文件）。
> Python 运行时本体与脚本随安装包；依赖在安装时由 bootstrap 下载，失败/离线则首启补下载，再不行回退自研链路。
> 统计基准：`src-tauri/binaries/pdfconv/python/Lib/site-packages/`，实测 189.5MB。

## 一、必须随安装包（不要传 OSS）

| 内容 | 路径 | 体积 | 说明 |
| --- | --- | --- | --- |
| Python 运行时本体 | `python/` 下除 `Lib/site-packages/` 外全部 | 20.3MB | 拉起 sidecar 的前提，不能远程拿 |
| 转换脚本 | `scripts/convert.py`、`scripts/postprocess.py` | <1MB | 工程源码，随包 |
| 引导器（已落地） | 主程序 `--fetch-pdf-deps` 子命令 | 极小 | 复用主程序 Rust（reqwest），无需独立 bootstrap.exe；安装钩子与首启补下载都走它 |

## 二、传到 OSS 的内容：整个 `site-packages/`

打包前先按第三节裁剪，再把剩余内容打成一个 zip（包内平铺下列目录/文件，解压后直接落到 `site-packages/`）。

| 目录/文件 | 体积 | 是否进 OSS 包 | 备注 |
| --- | --- | --- | --- |
| `cv2/` | 81.18MB | 进（裁剪 data 里的 xml，见下） | OpenCV 轮廓/线条检测 |
| `pymupdf/` | 44.63MB | 进（剔除 `mupdf-devel/`） | PDF 解析 |
| `numpy.libs/` | 20.18MB | 进 | openblas 运行库，被 numpy 依赖，必须同包 |
| `numpy/` | 19.63MB | 进（剔除各 `tests/`） | |
| `fontTools/` | 11.35MB | 进（`.c` 源码可选剔除） | 字体处理 |
| `lxml/` | 8.51MB | 进 | python-docx 依赖 |
| `docx/` | 1.47MB | 进 | python-docx |
| `pdf2docx/` | 0.35MB | 进 | 转换引擎本体 |
| `fire/` | 0.35MB | 进 | pdf2docx CLI 依赖 |
| `termcolor/` | 0.01MB | 进 | fire 依赖 |
| `typing_extensions.py` | 0.16MB | 进 | 顶层单文件 |
| `fitz/` | <0.01MB | **进（保留）** | pymupdf 兼容别名 shim，`import fitz` 靠它 |
| 各 `*.dist-info/` | ~1.2MB | 进 | 版本元数据，保留便于排查 |
| `bin/` | 0.81MB | **剔除** | 命令行 exe（f2py/fonttools/pymupdf 等），库运行时不调用 |
| `share/` | 0.01MB | **剔除** | man 手册 |

## 三、打包前裁剪规则（不进 OSS 包，也不进安装包）

### 3.1 安全裁剪（运行时确定不需要，约 22.8MB）

| 项 | 路径 | 体积 |
| --- | --- | --- |
| OpenCV 级联分类器 | `cv2/data/haarcascade_*.xml`（**保留 `cv2/data/__init__.py`**） | 9.31MB |
| pymupdf 开发库 | `pymupdf/mupdf-devel/`（`.lib`/`.h`） | 5.91MB |
| numpy 测试目录 | `numpy/` 下 13 个 `tests/` 目录 | 6.73MB |
| 命令行入口 | `bin/*.exe` | 0.81MB |
| man 手册 | `share/` | 0.01MB |
| 字节码缓存 | 所有 `__pycache__/`、`*.pyc` | 0（打包前清一次） |

### 3.2 可选裁剪（需转换回归确认，约 5.9MB）

- `fontTools/**/*.c` 源码文件（fontTools 为纯 Python，运行时不加载 `.c`；删掉后需跑一遍样本确认字体解析正常）。

### 3.3 裁剪后体积估算

- 安全裁剪后 site-packages 约 **166.7MB**；连同可选裁剪约 **160.8MB**。
- 打成 zip（原生 dll/pyd 压缩比有限），预计包体 **60–75MB**，以实压为准。

## 四、OSS 目录结构与命名建议

```
https://<你的OSS域名>/pdfconv/
  v0.5.13-1/
    site-packages.zip        # 裁剪后的依赖包（包内平铺各库目录）
    site-packages.sha256     # 整包 SHA-256 哈希
    manifest.json            # 版本、包大小、文件清单、更新时间
```

- 版本标签 `v0.5.13-1`：pdf2docx 版本号 + 修订序号；依赖升级就换新标签目录，旧缓存不污染。
- 哈希文件用于下载后完整性校验，防止传输损坏/被篡改。
- 打包 zip 时在 `site-packages/` 目录内部压缩，使解压结果直接是各库目录，不要再套一层 `site-packages/` 外壳。

## 五、需要回传给我的信息

文件上传好后，给我：

1. **依赖包下载 URL**（`site-packages.zip` 完整地址，HTTPS）；
2. **SHA-256 哈希**（在 zip 所在目录用 PowerShell 生成）：
   ```powershell
   Get-FileHash .\site-packages.zip -Algorithm SHA256 | Select-Object -ExpandProperty Hash
   ```
3. **版本标签**（默认 `v0.5.13-1`，如另有命名请告知）；
4. 若 OSS 有访问鉴权（签名 URL / Token），说明有效期和刷新方式。

拿到地址后，我再做：bootstrap 下载/校验/解压逻辑、NSIS `POSTINSTALL` 钩子调用、Rust 侧缓存定位与首启补下载、卸载清理钩子。

## 六、落地实现记录（2026-11-12）

### 6.1 已固化的远端信息

| 项 | 值 |
| --- | --- |
| 下载 URL | `https://prina-1413661498.cos.ap-shanghai.myqcloud.com/tools/desktop-pet-tool/v0.5.13-1/site-packages.zip` |
| SHA-256（小写） | `76f345772198a6a86ec7cc171d33adcf5e95c2105c0a54ff8c8133878ee4b012` |
| 版本标签 | `v0.5.13-1` |
| 包体 | 58.8MB（zip）；解压后 site-packages 166.7MB |
| 鉴权 | 公开读、无签名 URL，无 Token 有效期问题 |

### 6.2 缓存布局（用户可写目录，不进安装目录）

```
%LOCALAPPDATA%\com.desktop-pet-tool.float\pdfconv\v0.5.13-1\
  site-packages\          解压就位的依赖目录
  marker.json             { version, sha256, at }，用于判定缓存是否可用
```

- 放 LocalAppData：体积大、无需随账号漫游；卸载时安装器会自动清理整个 bundleid 目录。
- 按版本标签隔离：升级依赖换新标签，旧缓存不混用。
- 下载 → 哈希校验 → 解压到 `stage` 暂存目录 → 关键文件齐全校验 → 原子改名就位 → 写 marker；任一失败只清半成品，不污染正式目录。

### 6.3 触发时机（方案 B）

1. **安装时优先下载**：NSIS `POSTINSTALL` 钩子 `ExecWait` 拉起主程序 `--fetch-pdf-deps`；退出码非 0 仅 `DetailPrint` 警告，不阻断安装。
2. **首启 / 转换时补下载**：sidecar 转换开头 `ensure()`，缓存缺失或不完整时补下载。
3. **再不行回退自研**：`ensure` 返回 Err → sidecar 失败 → 清半成品 → 走 Rust 自研精简链路，界面 note 标注兜底。

### 6.4 打包资源裁剪（`tauri.conf.json > bundle.resources`）

- `binaries/pdfconv/python/*`：仅运行时顶层文件（exe/dll/pyd/zip/_pth），`Lib/site-packages` 子目录被非递归 glob 排除，不进安装包。
- `binaries/pdfconv/scripts/*`：两个转换脚本。
- 钩子文件：`src-tauri/windows/hooks.nsh`，由 `bundle.windows.nsis.installerHooks` 挂载。

### 6.5 已验证

- 开发态 `--fetch-pdf-deps`：下载、SHA-256 校验、解压、10 项关键文件齐全、marker 写入、临时包/暂存目录清理均通过；二次运行命中缓存（约 1.8s）不重复下载。
- 端到端：随包 python.exe + 缓存依赖转样本 PDF，退出码 0，2 页 4 表，产物 73807 字节。
- release 打包：安装包 **19.16 MiB**；生成 nsi 确认 resources 无 `Lib/site-packages` 条目，hooks.nsh 已 `!include`，POSTINSTALL 宏挂载就位。

### 6.6 尚未闭环（需在真实安装机上验收）

- 真实安装器走完安装流程，观察安装期钩子拉起主程序下载、失败不阻断；首次从悬浮球选 PDF→Word 整链路点击验收。
- 多份典型 PDF 转换回归；`fontTools/**/*.c` 可选裁剪（3.2）尚未做回归确认。

