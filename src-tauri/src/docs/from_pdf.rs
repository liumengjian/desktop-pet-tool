// PDF 输出方向：提取文字写入 txt / md / html；DOCX 使用固定版面转换引擎。
//
// 纯文字输出要求可提取文字；DOCX 保留页面位置与可编辑文本。

use std::path::Path;

use tauri::AppHandle;

use super::office_write;
use super::pdf_extract::{self, PageText};
use super::pdf_render;
use super::pptx_write::{self, PageImage};
use super::sidecar_pdf;
use super::zip_min::{self, OutEntry};

fn gather(src: &Path, cancelled: &dyn Fn() -> bool) -> Result<Vec<PageText>, String> {
    let pages = pdf_extract::extract(src, cancelled)?;
    if !pdf_extract::has_text(&pages) {
        return Err("该 PDF 没有可提取的文字（可能是扫描件或字体缺少 ToUnicode 映射）".to_string());
    }
    Ok(pages)
}

fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

pub fn to_txt(src: &Path, dst: &Path, cancelled: &dyn Fn() -> bool) -> Result<(), String> {
    let pages = gather(src, cancelled)?;
    let mut out = String::new();
    for page in &pages {
        if !out.is_empty() {
            out.push('\n');
        }
        for line in &page.lines {
            out.push_str(line);
            out.push('\n');
        }
    }
    std::fs::write(dst, out).map_err(|e| format!("写入失败: {}", e))
}

pub fn to_md(src: &Path, dst: &Path, cancelled: &dyn Fn() -> bool) -> Result<(), String> {
    let pages = gather(src, cancelled)?;
    let mut out = String::new();
    for page in &pages {
        out.push_str(&format!("## 第 {} 页\n\n", page.page));
        for line in &page.lines {
            out.push_str(line);
            out.push_str("\n\n");
        }
    }
    std::fs::write(dst, out).map_err(|e| format!("写入失败: {}", e))
}

pub fn to_html(src: &Path, dst: &Path, cancelled: &dyn Fn() -> bool) -> Result<(), String> {
    let pages = gather(src, cancelled)?;
    let title = src
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "PDF".to_string());
    let mut body = String::new();
    for page in &pages {
        body.push_str(&format!("<section><h2>第 {} 页</h2>\n", page.page));
        for line in &page.lines {
            body.push_str(&format!("<p>{}</p>\n", escape_html(line)));
        }
        body.push_str("</section>\n");
    }
    let html = format!(
        "<!DOCTYPE html>\n<html lang=\"zh-CN\">\n<head>\n<meta charset=\"utf-8\">\n<title>{}</title>\n<style>\nbody{{font-family:-apple-system,'Segoe UI','Microsoft YaHei',sans-serif;line-height:1.75;max-width:900px;margin:32px auto;padding:0 20px;color:#22201c;}}\nh2{{font-size:15px;color:#7a6f5f;border-bottom:1px solid #e6e0d4;padding-bottom:4px;margin-top:22px;}}\np{{margin:6px 0;}}\n</style>\n</head>\n<body>\n<h1>{}</h1>\n{}\n</body>\n</html>\n",
        escape_html(&title),
        escape_html(&title),
        body
    );
    std::fs::write(dst, html).map_err(|e| format!("写入失败: {}", e))
}

/// PDF → DOCX：固定页面布局，保留可编辑文字。失败时返回原因，避免静默降级丢失排版。
pub fn to_docx(
    src: &Path,
    dst: &Path,
    cancelled: &dyn Fn() -> bool,
    app: &AppHandle,
    note: &mut Option<String>,
) -> Result<(), String> {
    match sidecar_pdf::convert(app, src, dst, cancelled, note) {
        Ok(()) => Ok(()),
        Err(err) => {
            let _ = std::fs::remove_file(dst);
            if cancelled() {
                return Err("已取消".to_string());
            }
            Err(format!("PDF 转 Word 失败：{}。为避免改变分页和布局，未使用精简转换。", err))
        }
    }
}

fn encode(page: &[u8], width: usize, height: usize, fmt: &str) -> Result<Vec<u8>, String> {
    let img = image::RgbaImage::from_raw(width as u32, height as u32, page.to_vec())
        .ok_or("页面缓冲区尺寸不符")?;
    let mut out = Vec::new();
    let dynamic = image::DynamicImage::ImageRgba8(img);
    match fmt {
        "jpg" => {
            let rgb = dynamic.to_rgb8();
            let mut writer = std::io::Cursor::new(&mut out);
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut writer, 85)
                .encode(&rgb, rgb.width(), rgb.height(), image::ExtendedColorType::Rgb8)
                .map_err(|e| format!("JPEG 编码失败: {}", e))?;
        }
        _ => {
            let mut writer = std::io::Cursor::new(&mut out);
            dynamic
                .write_to(&mut writer, image::ImageFormat::Png)
                .map_err(|e| format!("PNG 编码失败: {}", e))?;
        }
    }
    Ok(out)
}

/// PDF → PPTX：每页渲染成一张图铺满整页幻灯片
pub fn to_pptx(
    src: &Path,
    dst: &Path,
    dpi: f32,
    cancelled: &dyn Fn() -> bool,
    note: &mut Option<String>,
) -> Result<(), String> {
    let (pw, ph) = pdf_render::page_size(src)?;
    let mut pages: Vec<PageImage> = Vec::new();
    let stats = pdf_render::render_pages(src, dpi, cancelled, |_number, w, h, pixels| {
        let jpg = encode(&pixels, w, h, "jpg")?;
        pages.push(PageImage { bytes: jpg, ext: "jpg" });
        Ok(())
    })?;
    if pages.is_empty() {
        return Err("该 PDF 没有可渲染的页面".to_string());
    }
    pptx_write::images_to_pptx(&pages, pw, ph, dst)?;
    *note = Some(if stats.cff_pages > 0 {
        format!("{} 页文字用 CFF/Type1 字体，未渲染出文字（其余画面正常）", stats.cff_pages)
    } else {
        format!("{} 页幻灯片", stats.pages)
    });
    Ok(())
}

/// PDF → XLSX：识别到的表格按行列落成单元格（保留底色与边框），
/// 没有表格的页面退回「页码 | 行内容」两列。
pub fn to_xlsx(src: &Path, dst: &Path, cancelled: &dyn Fn() -> bool) -> Result<(), String> {
    let pages = gather(src, cancelled)?;
    let name = src
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "PDF".to_string());

    let mut items: Vec<office_write::DocItem> = Vec::new();
    for page in pages.iter() {
        for item in page.items.iter() {
            match item {
                pdf_extract::PageItem::Line(line) => {
                    if !line.text.trim().is_empty() {
                        items.push(office_write::DocItem::Para(line.clone()));
                    }
                }
                pdf_extract::PageItem::Table(table) => {
                    if !table.rows.is_empty() {
                        items.push(office_write::DocItem::Table(table.clone()));
                    }
                }
                pdf_extract::PageItem::Image(_) => {}
            }
        }
    }

    let has_table = pages.iter().any(|p| {
        p.items
            .iter()
            .any(|it| matches!(it, pdf_extract::PageItem::Table(_)))
    });
    if !has_table {
        let mut rows: Vec<Vec<String>> = vec![vec!["页码".to_string(), "行内容".to_string()]];
        for page in pages.iter() {
            for line in page.lines.iter() {
                if cancelled() {
                    return Err("已取消".to_string());
                }
                let text = line.trim();
                if text.is_empty() {
                    continue;
                }
                rows.push(vec![page.page.to_string(), text.to_string()]);
            }
        }
        if rows.len() <= 1 {
            return Err(
                "该 PDF 没有可提取的文字（可能是扫描件或字体缺少 ToUnicode 映射）".to_string(),
            );
        }
        return office_write::rows_to_xlsx(&rows, &name, dst);
    }
    if items.is_empty() {
        return Err("该 PDF 没有可提取的文字（可能是扫描件或字体缺少 ToUnicode 映射）".to_string());
    }
    office_write::tables_to_xlsx(&items, &name, dst, cancelled)
}

/// PDF → 图片：单页直接落到 dst，多页按页打包成 zip（页名 page-001.jpg）
pub fn to_image(
    src: &Path,
    dst: &Path,
    fmt: &str,
    dpi: f32,
    cancelled: &dyn Fn() -> bool,
    note: &mut Option<String>,
) -> Result<(), String> {
    let total = pdf_render::page_count(src)?;
    let mut pages: Vec<(u32, Vec<u8>)> = Vec::new();
    let stats = pdf_render::render_pages(src, dpi, cancelled, |number, w, h, pixels| {
        let encoded = encode(&pixels, w, h, fmt)?;
        pages.push((number, encoded));
        Ok(())
    })?;

    if pages.is_empty() {
        return Err("该 PDF 没有可渲染的页面".to_string());
    }

    if total <= 1 {
        let (_, bytes) = pages.remove(0);
        std::fs::write(dst, bytes).map_err(|e| format!("写入失败: {}", e))?;
    } else {
        let ext = if fmt == "jpg" { "jpg" } else { "png" };
        let entries: Vec<OutEntry> = pages
            .iter()
            .enumerate()
            .map(|(i, (_, bytes))| OutEntry {
                name: format!("page-{:03}.{}", i + 1, ext),
                data: bytes.clone(),
            })
            .collect();
        zip_min::write_zip(entries, dst)?;
    }

    *note = Some(if total > 1 {
        format!("{} 页，已打包为 zip", total)
    } else {
        format!("{} DPI", dpi as i32)
    });
    if stats.cff_pages > 0 {
        *note = Some(format!(
            "{} 页文字用 CFF/Type1 字体，未渲染（图像与线条正常）",
            stats.cff_pages
        ));
    }
    Ok(())
}
