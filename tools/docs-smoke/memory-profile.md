# PDF 流式压缩实测（2026-11-10，release 构建）

夹具：`fixtures/big.jpg`（3200×2133）重复拼页，`pdf_write::images_to_pdf` 生成。
调用链：`docs-smoke.exe big <页数>` → `pdf_ops::compress`（>64MB 自动切 `pdf_stream`）。

| 用例 | 输入 | 输出 | 耗时 | 压缩阶段峰值 WS | 压缩阶段峰值私有 |
|---|---|---|---|---|---|
| 80 页 | 108.7 MB | 68.8 MB | 24.6 s | 89.8 MB | 84.8 MB |
| 480 页 | 652.1 MB | 412.9 MB | 157.1 s | 91.0 MB | 86.4 MB |

结论：输入体积 6.0×，峰值内存 1.01×。480 页产物用 lopdf 复核，`get_pages()==480`。
采样脚本按日志标记 `MARK compute-start` / `MARK compute-end` 只统计压缩阶段，
避免把随后 lopdf 校验整份产物的内存算进去。
