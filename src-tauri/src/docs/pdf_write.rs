// 图片 → PDF：自写的极简 PDF 生成器（A4 居中，JPEG 直接嵌入，无需任何外部引擎）。

use image::codecs::jpeg::JpegEncoder;
use image::DynamicImage;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

const PAGE_W: f32 = 595.28; // A4 宽（pt）
const PAGE_H: f32 = 841.89; // A4 高（pt）
const MARGIN: f32 = 24.0;

fn put(out: &mut BufWriter<File>, pos: &mut u64, bytes: &[u8]) -> Result<(), String> {
    out.write_all(bytes)
        .map_err(|e| format!("写入失败: {}", e))?;
    *pos += bytes.len() as u64;
    Ok(())
}

fn begin_obj(
    out: &mut BufWriter<File>,
    pos: &mut u64,
    offsets: &mut [u64],
    num: u32,
) -> Result<(), String> {
    if let Some(slot) = offsets.get_mut(num as usize) {
        *slot = *pos;
    }
    put(out, pos, format!("{} 0 obj\n", num).as_bytes())
}

pub fn images_to_pdf(
    srcs: &[PathBuf],
    dst: &Path,
    cancelled: &dyn Fn() -> bool,
) -> Result<(), String> {
    let mut pages: Vec<(u32, u32, Vec<u8>)> = Vec::new();
    for path in srcs {
        if cancelled() {
            return Err("已取消".to_string());
        }
        let img = image::open(path).map_err(|e| format!("无法读取图片: {}", e))?;
        let rgb = DynamicImage::ImageRgb8(img.to_rgb8());
        let mut jpeg: Vec<u8> = Vec::new();
        let encoder = JpegEncoder::new_with_quality(&mut jpeg, 82);
        rgb.write_with_encoder(encoder)
            .map_err(|e| format!("JPEG 编码失败: {}", e))?;
        pages.push((rgb.width(), rgb.height(), jpeg));
    }
    if pages.is_empty() {
        return Err("没有可用于生成 PDF 的图片".to_string());
    }

    let file = File::create(dst).map_err(|e| format!("写入失败: {}", e))?;
    let mut out = BufWriter::new(file);
    let mut pos: u64 = 0;
    let total_objects = 2 + pages.len() as u32 * 3;
    let mut offsets = vec![0u64; (total_objects + 1) as usize];

    put(&mut out, &mut pos, b"%PDF-1.4\n")?;

    // 1：目录
    begin_obj(&mut out, &mut pos, &mut offsets, 1)?;
    put(
        &mut out,
        &mut pos,
        b"<< /Type /Catalog /Pages 2 0 R >>\nendobj\n",
    )?;

    // 2：页面树
    let mut kids = String::new();
    for i in 0..pages.len() {
        kids.push_str(&format!("{} 0 R ", 3 + i as u32 * 3));
    }
    begin_obj(&mut out, &mut pos, &mut offsets, 2)?;
    put(
        &mut out,
        &mut pos,
        format!(
            "<< /Type /Pages /Kids [{}] /Count {} >>\nendobj\n",
            kids.trim_end(),
            pages.len()
        )
        .as_bytes(),
    )?;

    for (i, (w, h, jpeg)) in pages.iter().enumerate() {
        let page_no = 3 + i as u32 * 3;
        let content_no = 4 + i as u32 * 3;
        let image_no = 5 + i as u32 * 3;

        let usable_w = PAGE_W - MARGIN * 2.0;
        let usable_h = PAGE_H - MARGIN * 2.0;
        let scale = (usable_w / *w as f32).min(usable_h / *h as f32);
        let dw = (*w as f32 * scale).max(1.0);
        let dh = (*h as f32 * scale).max(1.0);
        let x = (PAGE_W - dw) / 2.0;
        let y = (PAGE_H - dh) / 2.0;

        begin_obj(&mut out, &mut pos, &mut offsets, page_no)?;
        put(
            &mut out,
            &mut pos,
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {:.2} {:.2}] /Resources << /XObject << /Im0 {} 0 R >> >> /Contents {} 0 R >>\nendobj\n",
                PAGE_W, PAGE_H, image_no, content_no
            )
            .as_bytes(),
        )?;

        let content = format!(
            "q\n{:.2} 0 0 {:.2} {:.2} {:.2} cm\n/Im0 Do\nQ\n",
            dw, dh, x, y
        );
        begin_obj(&mut out, &mut pos, &mut offsets, content_no)?;
        put(
            &mut out,
            &mut pos,
            format!("<< /Length {} >>\nstream\n", content.len()).as_bytes(),
        )?;
        put(&mut out, &mut pos, content.as_bytes())?;
        put(&mut out, &mut pos, b"endstream\nendobj\n")?;

        begin_obj(&mut out, &mut pos, &mut offsets, image_no)?;
        put(
            &mut out,
            &mut pos,
            format!(
                "<< /Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /DCTDecode /Length {} >>\nstream\n",
                w, h, jpeg.len()
            )
            .as_bytes(),
        )?;
        put(&mut out, &mut pos, jpeg)?;
        put(&mut out, &mut pos, b"\nendstream\nendobj\n")?;
    }

    // xref
    let xref_pos = pos;
    let mut xref = String::from("xref\n");
    xref.push_str(&format!("0 {}\n", total_objects + 1));
    xref.push_str("0000000000 65535 f \n");
    for num in 1..=total_objects {
        xref.push_str(&format!("{:010} 00000 n \n", offsets[num as usize]));
    }
    put(&mut out, &mut pos, xref.as_bytes())?;
    put(
        &mut out,
        &mut pos,
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{}\n%%EOF\n",
            total_objects + 1,
            xref_pos
        )
        .as_bytes(),
    )?;
    out.flush().map_err(|e| format!("写入失败: {}", e))?;
    Ok(())
}
