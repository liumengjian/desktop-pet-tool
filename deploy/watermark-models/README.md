# 图片水印去除模型 · COS 分发说明

模型按需下载，**不进安装包**；首次使用时由 `watermark/models.rs` 拉取清单、校验 sha256 后缓存到
`LocalAppData/<bundleid>/watermark/v1/`。

## 上传目标（腾讯云 COS 公开读直链）

目录前缀：
`https://prina-1413661498.cos.ap-shanghai.myqcloud.com/tools/desktop-pet-tool/watermark-models/pack/`

需要上传两个对象：

| 对象 key | 本地来源 | 校验 |
| --- | --- | --- |
| `pack/manifest.json` | 本目录 `manifest.json` | 清单白名单 |
| `pack/inpainting_lama.onnx` | LaMa ONNX 模型本体（约 88.3MB） | 见清单 `sha256` / `bytes` |

- 清单内容（`models` 段，`deps` 段本模块不读取；onnxruntime 优先复用人脸模块那套）：

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
- 运行时依赖 onnxruntime 复用 `face-models/pack/face-deps-v1.zip`，不在本目录重复下发。

## 许可证（待补齐）

LaMa 权重为 **Apache-2.0**。模型虽按需下载、不进安装包，仍需在 COS 同目录随附
`LICENSE` / `NOTICE`（或在应用关于说明中注明模型出处与协议），上传前补齐许可证文本。

## 本地手工放置（离线可用）

把 `inpainting_lama.onnx` 放到：
`%LocalAppData%\<bundleid>\watermark\v1\models\inpainting_lama.onnx`
即可跳过网络下载；onnxruntime 缺省时仍会回退下载 `face-deps-v1.zip`。
