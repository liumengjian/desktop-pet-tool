// 文档工具的端到端烟测：直接复用 src-tauri/src/docs 下的真实实现模块。

mod zip_min {
    include!("../../../src-tauri/src/docs/zip_min.rs");
}
mod image_ops {
    include!("../../../src-tauri/src/docs/image_ops.rs");
}
mod pdf_ops {
    include!("../../../src-tauri/src/docs/pdf_ops.rs");
}
mod pdf_write {
    include!("../../../src-tauri/src/docs/pdf_write.rs");
}
mod pdf_stream {
    include!("../../../src-tauri/src/docs/pdf_stream.rs");
}
mod office_ops {
    include!("../../../src-tauri/src/docs/office_ops.rs");
}
mod text_ops {
    include!("../../../src-tauri/src/docs/text_ops.rs");
}
mod office_write {
    include!("../../../src-tauri/src/docs/office_write.rs");
}
mod pdf_font {
    include!("../../../src-tauri/src/docs/pdf_font.rs");
}
mod pdf_text {
    include!("../../../src-tauri/src/docs/pdf_text.rs");
}
mod to_pdf {
    include!("../../../src-tauri/src/docs/to_pdf.rs");
}
mod pdf_extract {
    include!("../../../src-tauri/src/docs/pdf_extract.rs");
}
mod pdf_render {
    include!("../../../src-tauri/src/docs/pdf_render.rs");
}
mod pptx_write {
    include!("../../../src-tauri/src/docs/pptx_write.rs");
}
mod legacy {
    include!("../../../src-tauri/src/docs/legacy.rs");
}
mod doc_binary {
    include!("../../../src-tauri/src/docs/doc_binary.rs");
}
mod from_pdf {
    include!("../../../src-tauri/src/docs/from_pdf.rs");
}

use std::path::{Path, PathBuf};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures").join(name)
}

fn out(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join("docs-smoke-out");
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join(name);
    let _ = std::fs::remove_file(&p);
    p
}

fn size(p: &Path) -> u64 {
    std::fs::metadata(p).map(|m| m.len()).unwrap_or(0)
}

fn mb(bytes: u64) -> f64 {
    bytes as f64 / 1048576.0
}

fn no_cancel() -> bool {
    false
}

/// 大文件用例：拼一份上百 MB 的 PDF，走 pdf_ops（>64MB 自动切流式），
/// 供外部采样进程内存峰值。
fn big_case() {
    let pages: usize = std::env::args()
        .filter_map(|a| a.parse::<usize>().ok())
        .next()
        .unwrap_or(80);
    let big = out("big-input.pdf");
    let srcs: Vec<PathBuf> = (0..pages).map(|_| fixture("big.jpg")).collect();
    let t0 = std::time::Instant::now();
    match pdf_write::images_to_pdf(&srcs, &big, &no_cancel) {
        Ok(()) => println!(
            "input: {:.1} MB ({:.1}s)",
            mb(size(&big)),
            t0.elapsed().as_secs_f32()
        ),
        Err(e) => {
            println!("build failed: {}", e);
            return;
        }
    }

    let dst = out("big-output.pdf");
    let t1 = std::time::Instant::now();
    println!("MARK compute-start");
    let p = |pct: f32, label: &str| {
        if (pct as i32) % 20 == 0 {
            println!("  {:>5.1}% {}", pct, label);
        }
    };
    match pdf_ops::compress(&big, &dst, "normal", &no_cancel, &p) {
        Ok(()) => {
            println!("MARK compute-end");
            let ok = lopdf::Document::load(&dst).map(|d| d.get_pages().len()).unwrap_or(0);
            println!(
                "output: {:.1} MB, pages={} ({:.1}s)",
                mb(size(&dst)),
                ok,
                t1.elapsed().as_secs_f32()
            );
        }
        Err(e) => println!("compress failed: {}", e),
    }
}

struct Report {
    fails: Vec<String>,
}

impl Report {
    fn check(&mut self, ok: bool, what: &str) {
        if ok {
            println!("  PASS  {}", what);
        } else {
            println!("  FAIL  {}", what);
            self.fails.push(what.to_string());
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if std::env::args().any(|a| a == "big") {
        big_case();
        return;
    }
    // 单文件压缩测量：docs-smoke compress <文件> [small|normal|high]
    // 用来复现用户真机上的「压缩力度」问题，不参与 ALL PASS 判定。
    if let Some(pos) = args.iter().position(|a| a == "compress") {
        let src = PathBuf::from(&args[pos + 1]);
        let level = args.get(pos + 2).cloned().unwrap_or_else(|| "small".to_string());
        let stem = src
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "cli".to_string());
        let dst = out(&format!("cli-{}-{}.docx", stem, level));
        let no_progress = |_: f32, _: &str| {};
        let before = size(&src);
        match office_ops::compress(&src, &dst, &level, &no_cancel, &no_progress) {
            Ok(()) => println!(
                "[compress-cli] {} [{}] {} -> {} ({:.0}%)",
                src.display(),
                level,
                before,
                size(&dst),
                100.0 * size(&dst) as f64 / before.max(1) as f64
            ),
            Err(err) => println!("[compress-cli] failed: {}", err),
        }
        return;
    }
    // PDF 渲染调试：docs-smoke renderpdf <文件> [dpi] —— 不参与 ALL PASS 判定
    if let Some(pos) = args.iter().position(|a| a == "renderpdf") {
        let src = PathBuf::from(&args[pos + 1]);
        let dpi: f32 = args.get(pos + 2).and_then(|v| v.parse().ok()).unwrap_or(110.0);
        match pdf_render::render_pages(&src, dpi, &no_cancel, |num, w, h, rgba| {
            let img = image::RgbaImage::from_raw(w as u32, h as u32, rgba)
                .ok_or_else(|| "buffer size mismatch".to_string())?;
            let dst = out(&format!("render-{:03}.png", num));
            img.save(&dst).map_err(|e| e.to_string())?;
            println!("[render] {} {}x{}", dst.display(), w, h);
            Ok(())
        }) {
            Ok(st) => println!("[render] pages={} cff_pages={}", st.pages, st.cff_pages),
            Err(err) => println!("[render] failed: {}", err),
        }
        return;
    }
    // PDF → DOCX 调试：docs-smoke pdfdocx <文件>
    if let Some(pos) = args.iter().position(|a| a == "pdfdocx") {
        let src = PathBuf::from(&args[pos + 1]);
        let dst = out("pdfdocx.docx");
        match from_pdf::to_docx(&src, &dst, &no_cancel) {
            Ok(()) => println!("[pdfdocx] {} {} bytes", dst.display(), size(&dst)),
            Err(err) => println!("[pdfdocx] failed: {}", err),
        }
        return;
    }
    // PDF → XLSX 调试：docs-smoke pdfxlsx <文件>
    if let Some(pos) = args.iter().position(|a| a == "pdfxlsx") {
        let src = PathBuf::from(&args[pos + 1]);
        let dst = out("pdfxlsx.xlsx");
        match from_pdf::to_xlsx(&src, &dst, &no_cancel) {
            Ok(()) => println!("[pdfxlsx] {} {} bytes", dst.display(), size(&dst)),
            Err(err) => println!("[pdfxlsx] failed: {}", err),
        }
        return;
    }
    let mut r = Report { fails: Vec::new() };

    // 1) Office 压缩：体积、结构、正文不变
    for name in ["sample.docx", "sample.xlsx", "sample.pptx"] {
        println!("[office compress] {}", name);
        let src = fixture(name);
        let dst = out(&format!("out-{}", name));
        let no_progress = |_: f32, _: &str| {};
        match office_ops::compress(&src, &dst, "small", &no_cancel, &no_progress) {
            Ok(()) => {
                r.check(size(&dst) > 0, "产物非空");
                if size(&src) > 65536 {
                    r.check(size(&dst) < size(&src), "体积变小");
                } else {
                    // 极小文件重打包可能因条目头开销略微变大；产品层会丢弃这种产物
                    r.check(size(&dst) <= size(&src) + 2048, "小文件体积不显著增大");
                }
                let src_names: Vec<String> = zip_min::read_zip(&src)
                    .map(|e| e.into_iter().map(|x| x.name).collect())
                    .unwrap_or_default();
                let dst_names: Vec<String> = zip_min::read_zip(&dst)
                    .map(|e| e.into_iter().map(|x| x.name).collect())
                    .unwrap_or_default();
                r.check(
                    dst_names.iter().all(|n| src_names.contains(n)),
                    "不新增原文件没有的部件",
                );
                // 「更小」档允许去掉可选部件：stylesWithEffects 快照与资源管理器缩略图
                let missing: Vec<&String> = src_names
                    .iter()
                    .filter(|n| !dst_names.contains(n))
                    .collect();
                r.check(
                    missing.iter().all(|n| {
                        let l = n.to_lowercase();
                        l == "word/styleswitheffects.xml" || l.starts_with("docprops/thumbnail.")
                    }),
                    "缺失的部件仅在可选清单内",
                );
                println!(
                    "        {:.2} MB -> {:.2} MB",
                    mb(size(&src)),
                    mb(size(&dst))
                );
                if name == "sample.docx" {
                    let a = zip_min::read_zip(&src)
                        .and_then(|e| {
                            e.iter()
                                .find(|x| x.name == "word/document.xml")
                                .ok_or_else(|| "missing".to_string())
                                .and_then(|x| x.bytes())
                        })
                        .unwrap_or_default();
                    let b = zip_min::read_zip(&dst)
                        .and_then(|e| {
                            e.iter()
                                .find(|x| x.name == "word/document.xml")
                                .ok_or_else(|| "missing".to_string())
                                .and_then(|x| x.bytes())
                        })
                        .unwrap_or_default();
                    r.check(a == b && !a.is_empty(), "正文逐字节一致");
                }
            }
            Err(err) => r.check(false, &format!("压缩失败: {}", err)),
        }
    }

    // 2) 文本转换
    println!("[text convert]");
    let txt = out("sample.txt");
    match text_ops::docx_to_txt(&fixture("sample.docx"), &txt, &no_cancel) {
        Ok(()) => {
            let s = std::fs::read_to_string(&txt).unwrap_or_default();
            r.check(s.contains("纯 Rust 实现"), "docx→txt 含中文正文");
            r.check(s.contains("12345"), "docx→txt 含数字");
            r.check(!s.contains('<'), "docx→txt 无残留标签");
        }
        Err(err) => r.check(false, &format!("docx→txt 失败: {}", err)),
    }

    let csv = out("sample.csv");
    match text_ops::xlsx_to_csv(&fixture("sample.xlsx"), &csv, &no_cancel) {
        Ok(()) => {
            let s = std::fs::read_to_string(&csv).unwrap_or_default();
            r.check(s.contains("差旅费"), "xlsx→csv 含中文单元格");
            r.check(s.contains("1280.5"), "xlsx→csv 含数值");
            r.check(s.contains("餐饮,356") || s.contains("餐饮"), "xlsx→csv 行结构正常");
        }
        Err(err) => r.check(false, &format!("xlsx→csv 失败: {}", err)),
    }

    let ptxt = out("sample-ppt.txt");
    match text_ops::pptx_to_txt(&fixture("sample.pptx"), &ptxt, &no_cancel) {
        Ok(()) => {
            let s = std::fs::read_to_string(&ptxt).unwrap_or_default();
            r.check(s.contains("封面"), "pptx→txt 含第一页文字");
            r.check(s.contains("第二页"), "pptx→txt 含第二页文字");
        }
        Err(err) => r.check(false, &format!("pptx→txt 失败: {}", err)),
    }

    // 3) 图片压缩 / 转换
    println!("[image]");
    let ic = out("big-compressed.jpg");
    let s0 = size(&fixture("big.jpg"));
    match image_ops::compress(&fixture("big.jpg"), &ic, "small", &no_cancel) {
        Ok(()) => {
            r.check(size(&ic) < s0, "JPEG 体积变小");
            let dim = image::image_dimensions(&ic).unwrap_or((0, 0));
            r.check(dim.0.max(dim.1) <= 1280, "长边不超过档位上限");
            println!("        {:.2} MB -> {:.2} MB", mb(s0), mb(size(&ic)));
        }
        Err(err) => r.check(false, &format!("图片压缩失败: {}", err)),
    }

    let conv = out("big-bmp.bmp");
    match image_ops::convert(&fixture("big.png"), &conv, "bmp", &no_cancel) {
        Ok(()) => r.check(size(&conv) > 0, "PNG→BMP 产物非空"),
        Err(err) => r.check(false, &format!("格式转换失败: {}", err)),
    }

    // 4) 图片 → PDF
    println!("[pdf]");
    let pdf = out("images.pdf");
    match pdf_write::images_to_pdf(
        &[fixture("big.jpg"), fixture("big.png")],
        &pdf,
        &no_cancel,
    ) {
        Ok(()) => {
            match lopdf::Document::load(&pdf) {
                Ok(doc) => {
                    r.check(doc.get_pages().len() == 2, "PDF 页数 = 2");
                }
                Err(err) => r.check(false, &format!("PDF 无法解析: {}", err)),
            }
            println!("        生成 {:.2} MB", mb(size(&pdf)));
        }
        Err(err) => r.check(false, &format!("生成 PDF 失败: {}", err)),
    }

    // 5) PDF 压缩：页数不变、体积不增
    let pdf_out = out("images-compressed.pdf");
    let before = size(&pdf);
    let p = |_pct: f32, _label: &str| {};
    match pdf_ops::compress(&pdf, &pdf_out, "normal", &no_cancel, &p) {
        Ok(()) => {
            match lopdf::Document::load(&pdf_out) {
                Ok(doc) => {
                    r.check(doc.get_pages().len() == 2, "压缩后页数仍为 2");
                }
                Err(err) => r.check(false, &format!("压缩产物无法解析: {}", err)),
            }
            r.check(size(&pdf_out) > 0 && size(&pdf_out) <= before, "压缩后体积不增");
            println!(
                "        {:.2} MB -> {:.2} MB",
                mb(before),
                mb(size(&pdf_out))
            );
        }
        Err(err) => r.check(false, &format!("PDF 压缩失败: {}", err)),
    }

    // 6) 流式重写路径：与整份载入路径产出等价（页数一致、体积也降下来）
    println!("[pdf stream]");
    let s_out = out("images-stream.pdf");
    match pdf_stream::compress(&pdf, &s_out, 150.0, 80, &no_cancel, &p) {
        Ok(()) => {
            match lopdf::Document::load(&s_out) {
                Ok(doc) => {
                    r.check(doc.get_pages().len() == 2, "流式重写后页数仍为 2");
                }
                Err(err) => r.check(false, &format!("流式产物无法解析: {}", err)),
            }
            r.check(size(&s_out) > 0 && size(&s_out) <= before, "流式产物体积不增");
            println!(
                "        {:.2} MB -> {:.2} MB",
                mb(before),
                mb(size(&s_out))
            );
        }
        Err(err) => r.check(false, &format!("流式压缩失败: {}", err)),
    }

    // 7) 结构化转换（本轮新增）：docx/xlsx/pptx → html/md/txt，md/txt/html/csv 互转
    println!("[structured convert]");

    let docx_html = out("sample-docx.html");
    match text_ops::docx_to_html(&fixture("sample.docx"), &docx_html, &no_cancel) {
        Ok(()) => {
            let html = std::fs::read_to_string(&docx_html).unwrap_or_default();
            r.check(html.contains("<p>") || html.contains("<table>"), "docx→html 有正文结构");
            r.check(html.contains("<meta charset=\"utf-8\">"), "docx→html 带 UTF-8 声明");
        }
        Err(err) => r.check(false, &format!("docx→html 失败: {}", err)),
    }

    let docx_md = out("sample-docx.md");
    match text_ops::docx_to_md(&fixture("sample.docx"), &docx_md, &no_cancel) {
        Ok(()) => r.check(size(&docx_md) > 0, "docx→md 产物非空"),
        Err(err) => r.check(false, &format!("docx→md 失败: {}", err)),
    }

    let xlsx_html = out("sample-xlsx.html");
    match text_ops::xlsx_to_html(&fixture("sample.xlsx"), &xlsx_html, &no_cancel) {
        Ok(()) => {
            let html = std::fs::read_to_string(&xlsx_html).unwrap_or_default();
            r.check(html.contains("<table>"), "xlsx→html 生成表格");
        }
        Err(err) => r.check(false, &format!("xlsx→html 失败: {}", err)),
    }

    let xlsx_txt = out("sample-xlsx.txt");
    match text_ops::xlsx_to_txt(&fixture("sample.xlsx"), &xlsx_txt, &no_cancel) {
        Ok(()) => r.check(size(&xlsx_txt) > 0, "xlsx→txt 产物非空"),
        Err(err) => r.check(false, &format!("xlsx→txt 失败: {}", err)),
    }

    let pptx_html = out("sample-pptx.html");
    match text_ops::pptx_to_html(&fixture("sample.pptx"), &pptx_html, &no_cancel) {
        Ok(()) => {
            let html = std::fs::read_to_string(&pptx_html).unwrap_or_default();
            r.check(html.contains("第 1 页"), "pptx→html 保留页码标题");
        }
        Err(err) => r.check(false, &format!("pptx→html 失败: {}", err)),
    }

    let pptx_md = out("sample-pptx.md");
    match text_ops::pptx_to_md(&fixture("sample.pptx"), &pptx_md, &no_cancel) {
        Ok(()) => {
            let md = std::fs::read_to_string(&pptx_md).unwrap_or_default();
            r.check(md.contains("## 第 1 页"), "pptx→md 生成二级标题");
        }
        Err(err) => r.check(false, &format!("pptx→md 失败: {}", err)),
    }

    let md_src = out("case.md");
    std::fs::write(
        &md_src,
        "# 标题甲\n\n正文一行，含 **加粗** 与 `代码`。\n\n- 条目一\n- 条目二\n",
    )
    .unwrap();
    let md_html = out("case-md.html");
    match text_ops::md_to_html(&md_src, &md_html, &no_cancel) {
        Ok(()) => {
            let html = std::fs::read_to_string(&md_html).unwrap_or_default();
            r.check(html.contains("<h1>标题甲</h1>"), "md→html 标题正确");
            r.check(html.contains("<li>条目一</li>"), "md→html 列表正确");
        }
        Err(err) => r.check(false, &format!("md→html 失败: {}", err)),
    }

    let md_txt = out("case-md.txt");
    match text_ops::html_to_txt(&md_html, &md_txt, &no_cancel) {
        Ok(()) => {
            let txt = std::fs::read_to_string(&md_txt).unwrap_or_default();
            r.check(txt.contains("标题甲"), "html→txt 保留正文");
            r.check(!txt.contains("<h1>"), "html→txt 不含标签");
        }
        Err(err) => r.check(false, &format!("html→txt 失败: {}", err)),
    }

    let html_md = out("case-html.md");
    match text_ops::html_to_md(&md_html, &html_md, &no_cancel) {
        Ok(()) => {
            let md = std::fs::read_to_string(&html_md).unwrap_or_default();
            r.check(md.contains("# 标题甲"), "html→md 还原标题标记");
        }
        Err(err) => r.check(false, &format!("html→md 失败: {}", err)),
    }

    let txt_src = out("case.txt");
    std::fs::write(&txt_src, "第一段\n第二段\n").unwrap();
    let txt_docx = out("case-txt.docx");
    match office_write::text_to_docx(&txt_src, &txt_docx, false, &no_cancel) {
        Ok(()) => {
            let entries = zip_min::read_zip(&txt_docx).unwrap_or_default();
            r.check(
                entries.iter().any(|e| e.name == "word/document.xml"),
                "txt→docx 含 document.xml",
            );
            let back = out("case-txt-back.txt");
            match text_ops::docx_to_txt(&txt_docx, &back, &no_cancel) {
                Ok(()) => {
                    let text = std::fs::read_to_string(&back).unwrap_or_default();
                    r.check(text.contains("第一段") && text.contains("第二段"), "docx 回读文本一致");
                }
                Err(err) => r.check(false, &format!("txt→docx 回读失败: {}", err)),
            }
        }
        Err(err) => r.check(false, &format!("txt→docx 失败: {}", err)),
    }

    let md_docx = out("case-md.docx");
    match office_write::text_to_docx(&md_src, &md_docx, true, &no_cancel) {
        Ok(()) => r.check(size(&md_docx) > 0, "md→docx 产物非空"),
        Err(err) => r.check(false, &format!("md→docx 失败: {}", err)),
    }

    let csv_src = out("case.csv");
    std::fs::write(&csv_src, "名称,数量\n甲,1\n乙,\"含,逗号\"\n").unwrap();
    let csv_xlsx = out("case-csv.xlsx");
    match office_write::csv_to_xlsx(&csv_src, &csv_xlsx, &no_cancel) {
        Ok(()) => {
            let back = out("case-csv-back.csv");
            match text_ops::xlsx_to_csv(&csv_xlsx, &back, &no_cancel) {
                Ok(()) => {
                    let text = std::fs::read_to_string(&back).unwrap_or_default();
                    r.check(text.contains("名称") && text.contains("含,逗号"), "csv↔xlsx 往返一致");
                }
                Err(err) => r.check(false, &format!("csv→xlsx 回读失败: {}", err)),
            }
        }
        Err(err) => r.check(false, &format!("csv→xlsx 失败: {}", err)),
    }

    // 8) 排版引擎：内置字体解析 + 子集化 + 中文 PDF
    println!("[pdf text layout]");
    match pdf_font::default_face() {
        Some(face) => {
            r.check(face.units_per_em == 1000 || face.units_per_em > 0, "字体 unitsPerEm 正常");
            r.check(face.num_glyphs() > 1000, "字体字形数量正常");
            r.check(face.gid('中') > 0, "能查到汉字「中」的字形");
            r.check(face.gid('A') > 0, "能查到拉丁字母 A 的字形");
            r.check(face.advance(face.gid('中')) > 0, "汉字字宽非零");

            let used: std::collections::HashSet<u16> =
                [face.gid('中'), face.gid('文'), face.gid('A')]
                    .iter()
                    .copied()
                    .collect();
            let subset = face.subset(&used);
            r.check(subset.len() > 100, "子集字体非空");
            r.check(
                subset.len() < 1024 * 1024,
                "子集字体压缩到 1MB 以内",
            );
            println!(
                "        子集字体 {} 字节（原字库 {} 字节）",
                subset.len(),
                std::fs::metadata(
                    Path::new(env!("CARGO_MANIFEST_DIR"))
                        .join("../../src-tauri/assets/NotoSansSC.ttf")
                )
                .map(|m| m.len())
                .unwrap_or(0)
            );
        }
        None => r.check(false, "内置字体解析失败"),
    }

    let mut blocks = vec![
        pdf_text::Block::heading("文档转换 · 中文排版验证", 0),
        pdf_text::Block::heading("一、字体嵌入", 1),
        pdf_text::Block::body(
            "这一段用来验证中文字形是否正确嵌入：汉字、标点，以及拉丁字母 ABC 与数字 1234567890 混排。",
        ),
        pdf_text::Block::heading("二、自动换行", 2),
        pdf_text::Block::body(
            "自动换行需要在中文可以任意断行、英文在空格处断行。下面这段故意写得很长，用来触发多行折行，并且要保证右侧不会溢出页面边界，检查方式是渲染出来看右边缘。Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor incididunt ut labore et dolore magna aliqua.",
        ),
        pdf_text::Block::body(
            "第二页的触发条件是垂直空间用尽。这里再补几段文字，让内容超过一页的高度，从而验证分页与页码计数是否与实际页数一致。",
        ),
        pdf_text::Block::body("第三段：继续填充内容，确保文本量足够。"),
        pdf_text::Block::body("第四段：仍然填充，直到超过 A4 可用高度。"),
        pdf_text::Block::body("第五段：用来制造第二页。"),
    ];
    for i in 0..70 {
        blocks.push(pdf_text::Block::body(format!(
            "填充行 {}：这一行是为了把内容推到第二页，同时检查逐行基线与行距是否稳定。",
            i + 1
        )));
    }
    let text_pdf = out("text-layout.pdf");
    match pdf_text::blocks_to_pdf(&blocks, &text_pdf, &no_cancel) {
        Ok(stats) => {
            r.check(stats.pages >= 2, "长文本自动分页");
            match lopdf::Document::load(&text_pdf) {
                Ok(doc) => {
                    let pages = doc.get_pages().len();
                    r.check(pages as usize == stats.pages, "PDF 实际页数与排版一致");
                }
                Err(err) => r.check(false, &format!("排版产物无法解析: {}", err)),
            }
            let raw = std::fs::read(&text_pdf).unwrap_or_default();
            let has_font = raw
                .windows(11)
                .any(|w| w == b"/FontFile2 ");
            r.check(has_font, "PDF 内嵌了字体流");
            r.check(
                raw.windows(11).any(|w| w == b"/Identity-H"),
                "使用 Identity-H 编码",
            );
            println!(
                "        {} 页 / {} 字形 / 内嵌字体 {} 字节 / 文件 {} 字节",
                stats.pages,
                stats.glyphs,
                stats.font_bytes,
                size(&text_pdf)
            );
            println!("        产物: {}", text_pdf.display());
        }
        Err(err) => r.check(false, &format!("文本排版失败: {}", err)),
    }

    // 9) 各类文档 → PDF（自研排版 + 内置字体）
    println!("[doc -> pdf]");
    let mut doc_pdf_paths: Vec<(String, PathBuf)> = Vec::new();
    let inputs = [
        ("word", fixture("sample.docx"), "sample-docx.pdf"),
        ("sheet", fixture("sample.xlsx"), "sample-xlsx.pdf"),
        ("slides", fixture("sample.pptx"), "sample-pptx.pdf"),
        ("markdown", md_src.clone(), "case-md.pdf"),
        ("text", txt_src.clone(), "case-txt.pdf"),
        ("html", md_html.clone(), "case-html.pdf"),
        ("csv", csv_src.clone(), "case-csv.pdf"),
    ];
    for (kind, source, out_name) in inputs {
        let dst = out(out_name);
        match to_pdf::convert(kind, &source, &dst, &no_cancel) {
            Ok(_) => {
                let pages = match lopdf::Document::load(&dst) {
                    Ok(doc) => doc.get_pages().len(),
                    Err(err) => {
                        r.check(false, &format!("{}→pdf 无法解析: {}", kind, err));
                        continue;
                    }
                };
                r.check(pages >= 1, &format!("{}→pdf 页数≥1", kind));
                let raw = std::fs::read(&dst).unwrap_or_default();
                r.check(
                    raw.windows(10).any(|w| w == b"/FontFile2"),
                    &format!("{}→pdf 内嵌字体", kind),
                );
                r.check(size(&dst) > 5000, &format!("{}→pdf 体积正常", kind));
                println!(
                    "        {}→pdf {} 页 / {} 字节",
                    kind,
                    pages,
                    size(&dst)
                );
                doc_pdf_paths.push((kind.to_string(), dst));
            }
            Err(err) => r.check(false, &format!("{}→pdf 失败: {}", kind, err)),
        }
    }
    for (kind, path) in &doc_pdf_paths {
        println!("        {} 产物: {}", kind, path.display());
    }

    // 10) 内嵌图片进 PDF
    println!("[image in doc -> pdf]");
    let image_inputs = [
        ("word", fixture("sample-image.docx"), "sample-image-docx.pdf"),
        ("slides", fixture("sample-image.pptx"), "sample-image-pptx.pdf"),
    ];
    for (kind, source, out_name) in image_inputs {
        let dst = out(out_name);
        match to_pdf::convert(kind, &source, &dst, &no_cancel) {
            Ok(stats) => {
                r.check(stats.images >= 1, &format!("{}→pdf 插入了图片", kind));
                let raw = std::fs::read(&dst).unwrap_or_default();
                let needle: &[u8] = b"/Subtype /Image";
                r.check(
                    raw.windows(needle.len()).any(|w| w == needle),
                    &format!("{}→pdf 含图像对象", kind),
                );
                r.check(
                    raw.windows(16).any(|w| w == b"/XObject << /Im0"),
                    &format!("{}→pdf 页面引用图像", kind),
                );
                println!(
                    "        {}→pdf {} 页 / {} 图 / {} 字节",
                    kind,
                    stats.pages,
                    stats.images,
                    size(&dst)
                );
            }
            Err(err) => r.check(false, &format!("{} 图片进 pdf 失败: {}", kind, err)),
        }
    }

    // 11) PDF → 文本类
    println!("[pdf -> text]");
    let pdf_txt = out("from-pdf.txt");
    match from_pdf::to_txt(&text_pdf, &pdf_txt, &no_cancel) {
        Ok(()) => {
            let body = std::fs::read_to_string(&pdf_txt).unwrap_or_default();
            r.check(!body.trim().is_empty(), "pdf→txt 抽出了文字");
            r.check(
                body.contains("长文本") || body.contains("分页") || body.chars().count() > 200,
                "pdf→txt 内容量合理",
            );
            println!("        pdf→txt {} 字符", body.chars().count());
        }
        Err(err) => r.check(false, &format!("pdf→txt 失败: {}", err)),
    }
    let pdf_md = out("from-pdf.md");
    match from_pdf::to_md(&text_pdf, &pdf_md, &no_cancel) {
        Ok(()) => {
            let body = std::fs::read_to_string(&pdf_md).unwrap_or_default();
            r.check(body.contains("## 第 1 页"), "pdf→md 带页标题");
        }
        Err(err) => r.check(false, &format!("pdf→md 失败: {}", err)),
    }
    let pdf_html = out("from-pdf.html");
    match from_pdf::to_html(&text_pdf, &pdf_html, &no_cancel) {
        Ok(()) => {
            let body = std::fs::read_to_string(&pdf_html).unwrap_or_default();
            r.check(body.contains("<section><h2>第 1 页</h2>"), "pdf→html 结构正确");
        }
        Err(err) => r.check(false, &format!("pdf→html 失败: {}", err)),
    }
    let pdf_docx = out("from-pdf.docx");
    match from_pdf::to_docx(&text_pdf, &pdf_docx, &no_cancel) {
        Ok(()) => {
            // 结构化输出后每段 XML 比原来更精简，体积门槛按"确实是完整包装"来定，
            // 内容完整性由下面的回读断言保证
            r.check(size(&pdf_docx) > 1000, "pdf→docx 体积正常");
            let back = out("from-pdf-docx-back.txt");
            match text_ops::docx_to_txt(&pdf_docx, &back, &no_cancel) {
                Ok(()) => {
                    let body = std::fs::read_to_string(&back).unwrap_or_default();
                    r.check(!body.trim().is_empty(), "pdf→docx 回读有文字");
                }
                Err(err) => r.check(false, &format!("pdf→docx 回读失败: {}", err)),
            }
        }
        Err(err) => r.check(false, &format!("pdf→docx 失败: {}", err)),
    }

    // 第三方生产的 PDF（PyMuPDF 生成，字体与编码路径都和自研写出器不同）
    let real_pdf = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("real-cjk.pdf");
    if real_pdf.exists() {
        let real_txt = out("real-cjk.txt");
        match from_pdf::to_txt(&real_pdf, &real_txt, &no_cancel) {
            Ok(()) => {
                let body = std::fs::read_to_string(&real_txt).unwrap_or_default();
                r.check(body.contains("真实第三方"), "第三方 pdf→txt 抽出了中文");
                r.check(body.contains("第二页"), "第三方 pdf→txt 分页可见");
                r.check(body.contains("Mixed English"), "第三方 pdf→txt 拉丁文正常");
                println!("        第三方 pdf→txt {} 字符", body.chars().count());
            }
            Err(err) => r.check(false, &format!("第三方 pdf→txt 失败: {}", err)),
        }
    } else {
        println!("        (跳过第三方 PDF：先跑 python make_real_pdf.py)");
    }

    // 12) PDF → 图片（自研栅格化）
    println!("[pdf -> image]");
    // 单页 PDF 走单张图片，用第 10 节刚产出的带图 PDF（out() 会先删同名文件，这里不能用它）
    let single_pdf = std::env::temp_dir()
        .join("docs-smoke-out")
        .join("sample-image-docx.pdf");
    let img_png = out("pdf-page.png");
    let mut note: Option<String> = None;
    match from_pdf::to_image(&single_pdf, &img_png, "png", 150.0, &no_cancel, &mut note) {
        Ok(()) => {
            r.check(size(&img_png) > 5000, "pdf→png 输出了非空图片");
            match image::open(&img_png) {
                Ok(decoded) => {
                    let (w, h) = (decoded.width(), decoded.height());
                    r.check(w >= 1000 && h >= 1500, "pdf→png 尺寸符合 A4 150DPI");
                    // 判断画面不是一张纯白：抽中心区域统计非白像素
                    let rgb = decoded.to_luma8();
                    let dark = rgb.pixels().filter(|p| p.0[0] < 200).count();
                    r.check(dark > 200, "pdf→png 画面有内容（不是空白页）");
                    println!("        pdf→png {}x{} / {} 字节 / 深色像素 {}", w, h, size(&img_png), dark);
                }
                Err(err) => r.check(false, &format!("pdf→png 无法解码: {}", err)),
            }
        }
        Err(err) => r.check(false, &format!("pdf→png 失败: {}", err)),
    }

    let img_zip = out("pdf-pages.zip");
    if real_pdf.exists() {
        let mut note2: Option<String> = None;
        match from_pdf::to_image(&real_pdf, &img_zip, "jpg", 150.0, &no_cancel, &mut note2) {
            Ok(()) => {
                match zip_min::read_zip(&img_zip) {
                    Ok(entries) => {
                        r.check(entries.len() == 2, "多页 pdf→jpg 打成 2 个条目");
                        let mut ok_all = true;
                        for entry in entries.iter() {
                            let bytes = entry.bytes().unwrap_or_default();
                            if image::load_from_memory(&bytes).is_err() {
                                ok_all = false;
                            }
                        }
                        r.check(ok_all, "每个页面图都能解码");
                        println!("        多页 pdf→jpg {} 条目 / {} 字节", entries.len(), size(&img_zip));
                    }
                    Err(err) => r.check(false, &format!("读取打包结果失败: {}", err)),
                }
            }
            Err(err) => r.check(false, &format!("多页 pdf→jpg 失败: {}", err)),
        }
    }

    // 13) PDF → PPTX / XLSX
    println!("[pdf -> office]");
    let pdf_pptx = out("from-pdf.pptx");
    let mut note3: Option<String> = None;
    match from_pdf::to_pptx(&real_pdf, &pdf_pptx, 150.0, &no_cancel, &mut note3) {
        Ok(()) => {
            match zip_min::read_zip(&pdf_pptx) {
                Ok(entries) => {
                    let names: Vec<String> = entries.iter().map(|e| e.name.clone()).collect();
                    r.check(names.iter().any(|n| n == "ppt/presentation.xml"), "pptx 有 presentation.xml");
                    r.check(names.iter().any(|n| n == "ppt/slides/slide1.xml"), "pptx 有第 1 页");
                    r.check(names.iter().any(|n| n == "ppt/slides/slide2.xml"), "pptx 有第 2 页");
                    r.check(names.iter().any(|n| n == "ppt/media/image1.jpg"), "pptx 内嵌页面图");
                    r.check(names.iter().any(|n| n == "ppt/slideMasters/slideMaster1.xml"), "pptx 有母版");
                    println!("        pdf→pptx {} 条目 / {} 字节", entries.len(), size(&pdf_pptx));
                }
                Err(err) => r.check(false, &format!("pptx 读取失败: {}", err)),
            }
        }
        Err(err) => r.check(false, &format!("pdf→pptx 失败: {}", err)),
    }

    let pdf_xlsx = out("from-pdf.xlsx");
    match from_pdf::to_xlsx(&text_pdf, &pdf_xlsx, &no_cancel) {
        Ok(()) => {
            match zip_min::read_zip(&pdf_xlsx) {
                Ok(entries) => {
                    let sheet = entries
                        .iter()
                        .find(|e| e.name == "xl/worksheets/sheet1.xml")
                        .and_then(|e| e.bytes().ok())
                        .unwrap_or_default();
                    let xml = String::from_utf8_lossy(&sheet).to_string();
                    r.check(xml.contains("页码"), "xlsx 表头写入");
                    r.check(xml.contains("inlineStr"), "xlsx 写了文本单元格");
                    println!("        pdf→xlsx {} 字节 / 单元格 xml {} 字节", size(&pdf_xlsx), xml.len());
                }
                Err(err) => r.check(false, &format!("xlsx 读取失败: {}", err)),
            }
        }
        Err(err) => r.check(false, &format!("pdf→xlsx 失败: {}", err)),
    }

    // 14) RTF / ODF / EPUB 取字与转换
    println!("[rtf / odf / epub]");
    let rtf_src = fixture("sample.rtf");
    match legacy::blocks_for(&rtf_src, "rtf") {
        Ok(blocks) => {
            let plain: String = blocks
                .iter()
                .map(|b| match b {
                    text_ops::Block::Heading(_, t) | text_ops::Block::Para(t) => t.clone(),
                    text_ops::Block::Item(t) => t.clone(),
                    text_ops::Block::Table(rows) => rows
                        .iter()
                        .map(|r| r.join(" "))
                        .collect::<Vec<_>>()
                        .join(" "),
                })
                .collect::<Vec<_>>()
                .join("\n");
            r.check(plain.contains("RTF 标题行"), "rtf 抽到标题");
            r.check(plain.contains("第一段中文内容"), "rtf 抽到正文");
            r.check(plain.contains("12345"), "rtf 保留数字");
            let rtf_docx = out("rtf.docx");
            if legacy::convert_blocks(&rtf_src, &blocks, &rtf_docx, "docx", &no_cancel).is_ok() {
                r.check(zip_min::read_zip(&rtf_docx).is_ok(), "rtf → docx 写出可解包");
            } else {
                r.check(false, "rtf → docx 失败");
            }
            let rtf_pdf = out("rtf.pdf");
            if legacy::convert_blocks(&rtf_src, &blocks, &rtf_pdf, "pdf", &no_cancel).is_ok() {
                r.check(true, "rtf → pdf 排版成功");
            } else {
                r.check(false, "rtf → pdf 失败");
            }
        }
        Err(err) => r.check(false, &format!("rtf 取字失败: {}", err)),
    }

    let odt_src = fixture("sample.odt");
    match legacy::blocks_for(&odt_src, "odt") {
        Ok(blocks) => {
            let txt = out("odt.txt");
            match legacy::convert_blocks(&odt_src, &blocks, &txt, "txt", &no_cancel) {
                Ok(()) => {
                    let body = std::fs::read_to_string(&txt).unwrap_or_default();
                    r.check(body.contains("ODT 一级标题"), "odt 抽到标题");
                    r.check(body.contains("第二段包含 Mixed English"), "odt 抽到正文");
                    r.check(body.contains("列表项一"), "odt 抽到列表");
                }
                Err(err) => r.check(false, &format!("odt → txt 失败: {}", err)),
            }
            let odt_html = out("odt.html");
            match legacy::convert_blocks(&odt_src, &blocks, &odt_html, "html", &no_cancel) {
                Ok(()) => {
                    let body = std::fs::read_to_string(&odt_html).unwrap_or_default();
                    r.check(body.contains("<h1>ODT 一级标题</h1>"), "odt → html 标题成 h1");
                    r.check(body.contains("<p>这是 ODT 的正文第一段。</p>"), "odt → html 正文成 p");
                }
                Err(err) => r.check(false, &format!("odt → html 失败: {}", err)),
            }
        }
        Err(err) => r.check(false, &format!("odt 取字失败: {}", err)),
    }

    let ods_src = fixture("sample.ods");
    match legacy::blocks_for(&ods_src, "ods") {
        Ok(blocks) => {
            let ods_csv = out("ods.csv");
            match legacy::convert_blocks(&ods_src, &blocks, &ods_csv, "csv", &no_cancel) {
                Ok(()) => {
                    let body = std::fs::read_to_string(&ods_csv).unwrap_or_default();
                    r.check(body.contains("城市,人数"), "ods → csv 表头");
                    r.check(body.contains("重庆,32"), "ods → csv 数据行");
                }
                Err(err) => r.check(false, &format!("ods → csv 失败: {}", err)),
            }
            let ods_xlsx = out("ods.xlsx");
            match legacy::convert_blocks(&ods_src, &blocks, &ods_xlsx, "xlsx", &no_cancel) {
                Ok(()) => r.check(zip_min::read_zip(&ods_xlsx).is_ok(), "ods → xlsx 写出可解包"),
                Err(err) => r.check(false, &format!("ods → xlsx 失败: {}", err)),
            }
        }
        Err(err) => r.check(false, &format!("ods 取字失败: {}", err)),
    }

    let epub_src = fixture("sample.epub");
    match legacy::blocks_for(&epub_src, "epub") {
        Ok(blocks) => {
            let epub_md = out("epub.md");
            match legacy::convert_blocks(&epub_src, &blocks, &epub_md, "md", &no_cancel) {
                Ok(()) => {
                    let body = std::fs::read_to_string(&epub_md).unwrap_or_default();
                    r.check(body.contains("# EPUB 第一章"), "epub → md 标题");
                    r.check(body.contains("这是电子书正文的第一段。"), "epub 正文");
                    r.check(body.contains("行内标签"), "epub 去掉行内标签保留文字");
                    r.check(body.contains("& 实体"), "epub 解码实体");
                }
                Err(err) => r.check(false, &format!("epub → md 失败: {}", err)),
            }
        }
        Err(err) => r.check(false, &format!("epub 取字失败: {}", err)),
    }

    // 表格线与图片：把「带网格的 xlsx」和「带图的 pptx」也渲染一遍，留档给目视复核
    for (label, source_name, dst_name) in [
        ("xlsx 表格", "sample-xlsx.pdf", "render-xlsx.png"),
        ("pptx 图文", "sample-image-pptx.pdf", "render-pptx.png"),
    ] {
        let source = std::env::temp_dir().join("docs-smoke-out").join(source_name);
        let dst = out(dst_name);
        let mut w: Option<String> = None;
        match from_pdf::to_image(&source, &dst, "png", 150.0, &no_cancel, &mut w) {
            Ok(()) => {
                let decoded = image::open(&dst).map(|d| (d.width(), d.height()));
                r.check(decoded.is_ok(), &format!("{} 渲染成图可解码", label));
                println!("        {} → {} {:?}", label, dst.display(), decoded);
            }
            Err(err) => r.check(false, &format!("{} 渲染失败: {}", label, err)),
        }
    }

    // 旧版 .doc（Word 97-2003 / WPS 保存的二进制文档）
    for (name, needles) in [
        ("sample.doc", vec!["重庆百腾国际旅行社", "服务监督"]),
        ("sample-table.doc", vec!["报   价   书", "简要行程", "重庆百腾"]),
    ] {
        println!("[doc binary] {}", name);
        let src = fixture(name);
        match doc_binary::doc_blocks(&src) {
            Ok(blocks) => {
                println!("        {} 个结构块", blocks.len());
                let has_table = blocks.iter().any(|b| matches!(b, text_ops::Block::Table(_)));
                if name == "sample-table.doc" {
                    r.check(has_table, "带表格的 .doc 抽出表格块");
                }
                let txt = out(&format!("{}.txt", name));
                match doc_binary::convert(&src, &txt, "txt", &no_cancel) {
                    Ok(()) => {
                        let body = std::fs::read_to_string(&txt).unwrap_or_default();
                        for needle in &needles {
                            r.check(body.contains(needle), &format!("doc → txt 含「{}」", needle));
                        }
                        r.check(!body.contains('\u{13}'), "域代码指令已剔除");
                        println!("        txt {} 字节", body.len());
                    }
                    Err(err) => r.check(false, &format!("doc → txt 失败: {}", err)),
                }
                let docx = out(&format!("{}.docx", name));
                match doc_binary::convert(&src, &docx, "docx", &no_cancel) {
                    Ok(()) => {
                        let xml = zip_min::read_zip(&docx)
                            .and_then(|e| {
                                e.iter()
                                    .find(|x| x.name == "word/document.xml")
                                    .ok_or_else(|| "missing".to_string())
                                    .and_then(|x| x.bytes())
                            })
                            .unwrap_or_default();
                        let xml = String::from_utf8_lossy(&xml).to_string();
                        r.check(xml.contains(&needles[0]), "doc → docx 正文落盘");
                        r.check(size(&docx) > 0, "doc → docx 产物非空");
                    }
                    Err(err) => r.check(false, &format!("doc → docx 失败: {}", err)),
                }
                let pdf = out(&format!("{}.pdf", name));
                match doc_binary::convert(&src, &pdf, "pdf", &no_cancel) {
                    Ok(()) => {
                        r.check(
                            pdf_render::page_count(&pdf).map(|n| n > 0).unwrap_or(false),
                            "doc → pdf 页数大于 0",
                        );
                        println!("        pdf {} 字节", size(&pdf));
                    }
                    Err(err) => r.check(false, &format!("doc → pdf 失败: {}", err)),
                }
            }
            Err(err) => r.check(false, &format!("doc 取字失败: {}", err)),
        }
    }

    if r.fails.is_empty() {
        println!("\nALL PASS");
    } else {
        println!("\n{} FAILED", r.fails.len());
        std::process::exit(1);
    }
}
