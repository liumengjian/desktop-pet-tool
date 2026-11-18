# 图片水印去除模型 · COS 分发说明

模型按需下载，**不进安装包**；首次使用时由 `watermark/models.rs` 校验 sha256 后缓存到
`<安装目录>\data\watermark\v1\`，例如 `D:\tools\dpt\data\watermark\v1\`。

`manifest.json` 只有 1KB，已经通过 `include_str!` 编进 exe，**不再上传 COS**。COS 上只放大模型。

## 上传目标（腾讯云 COS 公开读直链）

目录前缀：
`https://prina-1413661498.cos.ap-shanghai.myqcloud.com/tools/desktop-pet-tool/watermark-models/pack/`

只需要上传一个对象：

| 对象 key | 本地来源 | 校验 |
| --- | --- | --- |
| `pack/inpainting_lama.onnx` | LaMa ONNX 模型本体（约 88.3MB） | 见本目录 `manifest.json` 的 `sha256` / `bytes` |

`manifest.json` 不需要上传；它随 exe 编译，客户端不再请求
`.../watermark-models/pack/manifest.json`。

水印功能在干净机器上首次运行，还依赖下面两项**已经上传**的共享资源：

| 用途 | COS key | 说明 |
| --- | --- | --- |
| onnxruntime | `face-models/pack/face-deps-v1.zip` | 与人脸比对共用，已存在，无需重复上传 |
| Python 图像依赖（numpy / cv2 等） | `v0.5.13-1/site-packages.zip` | 与文档转换共用，已存在，无需重复上传 |

内置 Python 解释器和 `scripts/watermark_erase.py` 在安装包内，不需要上传 COS。

- 清单内容（本目录 `manifest.json`，编译进 exe；`deps` 段本模块不读取，onnxruntime 优先复用人脸模块那套）：

```json
{
  "models": [
    {
      "name": "inpainting_lama.onnx",
      "sha256": "7df918ac3921d3daf0aae1d219776cf0dc4e4935f035af81841b40adcf74fdf2",
      "bytes": 92591623
    }
  ]
}
```

- 模型来源：OpenCV Model Zoo（`opencvinpaint/lama`），经 hf-mirror 镜像下载。
- 运行时依赖 onnxruntime 复用 `face-models/pack/face-deps-v1.zip`，不在本目录重复下发；
  该对象必须已经存在，否则首次去水印仍会因依赖下载失败而报错。
- 换模型时：改本目录 `manifest.json` 的 `sha256` / `bytes`（必要时换 `MODEL_NAME`），重新编译发版，再上传新的 onnx；不要直接覆盖同名模型，避免旧客户端哈希校验失败。

## 许可证（待补齐）

LaMa 权重为 **Apache-2.0**。模型虽按需下载、不进安装包，仍需在 COS 同目录随附
`LICENSE` / `NOTICE`（或在应用关于说明中注明模型出处与协议），上传前补齐许可证文本。

## 本地手工放置（离线可用）

把 `inpainting_lama.onnx` 放到：
`<安装目录>\data\watermark\v1\models\inpainting_lama.onnx`
例如 `D:\tools\dpt\data\watermark\v1\models\inpainting_lama.onnx`
即可跳过网络下载；onnxruntime 缺省时仍会回退下载 `face-deps-v1.zip`。
