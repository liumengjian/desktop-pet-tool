# 文档工具烟测工程

不依赖 Tauri 运行时，直接用 `include!` 引用 `src-tauri/src/docs/*.rs` 的真实实现，
验证压缩与转换的端到端行为（体积、结构完整性、正文一致性、页数、图片尺寸）。

## 用法

```powershell
cd E:\project\desktop-pet-tool\tools\docs-smoke
python make_fixtures.py          # 生成 fixtures（需要 pillow）
cargo run --offline --quiet      # 输出逐项 PASS/FAIL，全部通过时打印 ALL PASS
```

产出写到 `%TEMP%\docs-smoke-out`。

## 覆盖

| 用例 | 断言 |
| --- | --- |
| docx / pptx 压缩 | 体积变小、条目数一致、`word/document.xml` 逐字节一致 |
| xlsx 压缩 | 小文件体积不显著增大、条目数一致 |
| docx→txt | 含中文与数字、无残留标签 |
| xlsx→csv | 含中文单元格与数值 |
| pptx→txt | 含两页文字 |
| 图片压缩 | 体积变小、长边不超过档位上限 |
| 图片转换 | PNG→BMP 产物非空 |
| 图片→PDF | lopdf 可解析、页数 = 2 |
| PDF 压缩 | 页面数不变、体积不增 |

夹具由 `make_fixtures.py` 现场生成，不入库（体积约 8MB）。
