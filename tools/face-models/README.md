# 人脸比对 · 模型分包脚本

把 `buffalo_l` 里真正需要的两个 onnx 和 onnxruntime 依赖目录打成可直传 COS 的分包，
并生成 Rust 端（`src-tauri/src/face/models.rs`）读取的 `manifest.json`。

## 用法

```powershell
# 默认取本地已下载的素材（.preview 下的调试目录）
pwsh -File tools/face-models/package-models.ps1

# 指定来源与输出
pwsh -File tools/face-models/package-models.ps1 `
  -ModelsDir .preview/face-models/unzipped `
  -DepsDir .preview/face-deps `
  -Out .preview/face-models/pack
```

输出目录里会生成 3 个待上传文件 + 清单：

| 文件 | 用途 | 体积参考 |
| --- | --- | --- |
| `det_10g.onnx` | SCRFD 人脸检测 | 16.1 MB |
| `w600k_r50.onnx` | ResNet50 人脸识别（512 维 embedding） | 166.3 MB |
| `face-deps-v1.zip` | onnxruntime 依赖，解压后平铺到 `deps/` | 约 40 MB |
| `manifest.json` | 文件名 / sha256 / 字节数，应用按它校验 | < 1 KB |

## 打包约束（改脚本前先看）

1. **zip 内不能套外壳目录**：解压结果必须是 `onnxruntime/`、`google/` 直接位于 `deps/` 下，
   因为应用用 `deps/onnxruntime/__init__.py` 是否存在判定依赖就绪。
2. **不带 numpy**：运行时复用 PDF 转换 sidecar 已装的 numpy（`pdfconv/v0.5.13-1/site-packages`），
   重复下发会让 48MB 的依赖包白白多出 20MB，且两个 numpy 版本混用会互相覆盖。
3. **只带推理真正需要的**：`onnxruntime`（含 `google`(protobuf)/`flatbuffers`/`packaging`）即可，
   不要整包 `site-packages`。
4. **sha256 必须来自实际文件**：应用逐字节校验，哈希对不上会删包重试。

## 上传后需要回填的信息

| 项 | 说明 |
| --- | --- |
| COS 基址 | 应用里写死为 `https://prina-1413661498.cos.ap-shanghai.myqcloud.com/tools/desktop-pet-tool/face-models/pack`，改目录要同步改 `models.rs` 的 `BASE_URL` |
| 4 个文件的 sha256 | 以本脚本生成的 `manifest.json` 为准，不要手填 |
| 读权限 | 必须公开读（不带签名），应用用匿名 GET |
| `Content-Length` | 应用按清单里的 `bytes` 显示下载进度；COS 需返回真实长度（默认满足） |

上传顺序建议：先传 3 个文件，最后传 `manifest.json`（清单最后到位，避免用户拿到清单却下不到文件）。

## 校验

```powershell
# 上传后直接拉一遍，确认可匿名读、长度与哈希与清单一致
$base = 'https://prina-1413661498.cos.ap-shanghai.myqcloud.com/tools/desktop-pet-tool/face-models/pack'
Invoke-WebRequest "$base/manifest.json" -UseBasicParsing | Select-Object -ExpandProperty Content
```

应用侧的缓存布局（命中后完全不走网络）：

```
<安装目录>\data\face\v1\
  models\{det_10g.onnx, w600k_r50.onnx}
  deps\{onnxruntime, google, flatbuffers, packaging, ...}
  marker.json
```

例如安装到 `D:\tools\dpt` 时，实际路径为 `D:\tools\dpt\data\face\v1\`。
