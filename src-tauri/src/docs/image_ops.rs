// 图片压缩与格式转换（纯 Rust，image crate）。

use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::{CompressionType, FilterType as PngFilter, PngEncoder};
use image::{DynamicImage, ImageFormat};
use std::fs::File;
use std::io::Write;
use std::path::Path;

/// 压缩档位：长边上限 + JPEG 质量
fn level_cfg(level: &str) -> (u32, u8) {
    match level {
        // 「更小」取 1080 长边 / q55：仍高于 1080p 屏的显示需求，体感不糊但体积明显下降
        "small" => (1080, 55),
        "high" => (2560, 88),
        _ => (1600, 75),
    }
}

pub fn encode_jpeg_bytes(img: &DynamicImage, quality: u8) -> Result<Vec<u8>, String> {
    let rgb = DynamicImage::ImageRgb8(img.to_rgb8());
    let mut buf: Vec<u8> = Vec::new();
    let encoder = JpegEncoder::new_with_quality(&mut buf, quality);
    rgb.write_with_encoder(encoder)
        .map_err(|e| format!("JPEG 编码失败: {}", e))?;
    Ok(buf)
}

pub fn encode_png_bytes(img: &DynamicImage) -> Result<Vec<u8>, String> {
    let mut buf: Vec<u8> = Vec::new();
    let encoder = PngEncoder::new_with_quality(&mut buf, CompressionType::Best, PngFilter::Adaptive);
    img.write_with_encoder(encoder)
        .map_err(|e| format!("PNG 编码失败: {}", e))?;
    Ok(buf)
}

fn fit_within(img: DynamicImage, max_edge: u32) -> DynamicImage {
    let (w, h) = (img.width(), img.height());
    if w.max(h) <= max_edge {
        return img;
    }
    // Lanczos3 降采样：与参考实现一致，避免缩小后发糊
    img.resize(max_edge, max_edge, image::imageops::FilterType::Lanczos3)
}

pub fn compress(
    src: &Path,
    dst: &Path,
    level: &str,
    cancelled: &dyn Fn() -> bool,
) -> Result<(), String> {
    if cancelled() {
        return Err("已取消".to_string());
    }
    let (max_edge, quality) = level_cfg(level);
    let img = image::open(src).map_err(|e| format!("无法解码图片: {}", e))?;
    let resized = fit_within(img, max_edge);
    if cancelled() {
        return Err("已取消".to_string());
    }

    let ext = src
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "jpg" | "jpeg" => {
            let bytes = encode_jpeg_bytes(&resized, quality)?;
            let mut out = File::create(dst).map_err(|e| format!("写入失败: {}", e))?;
            out.write_all(&bytes).map_err(|e| format!("写入失败: {}", e))?;
        }
        "png" => {
            let file = File::create(dst).map_err(|e| format!("写入失败: {}", e))?;
            let mut writer = std::io::BufWriter::new(file);
            let encoder =
                PngEncoder::new_with_quality(&mut writer, CompressionType::Best, PngFilter::Adaptive);
            resized
                .write_with_encoder(encoder)
                .map_err(|e| format!("PNG 编码失败: {}", e))?;
            writer.flush().map_err(|e| format!("写入失败: {}", e))?;
        }
        _ => return Err("该图片格式请用「转换」处理".to_string()),
    }
    Ok(())
}

pub fn convert(
    src: &Path,
    dst: &Path,
        target: &str,
    cancelled: &dyn Fn() -> bool,
) -> Result<(), String> {
    if cancelled() {
        return Err("已取消".to_string());
    }
    let img = image::open(src).map_err(|e| format!("无法解码图片: {}", e))?;
    if cancelled() {
        return Err("已取消".to_string());
    }
    match target {
        "jpg" | "jpeg" => {
            let bytes = encode_jpeg_bytes(&img, 86)?;
            let mut out = File::create(dst).map_err(|e| format!("写入失败: {}", e))?;
            out.write_all(&bytes).map_err(|e| format!("写入失败: {}", e))?;
        }
        "png" => {
            img.save_with_format(dst, ImageFormat::Png)
                .map_err(|e| format!("PNG 写入失败: {}", e))?;
        }
        "bmp" => {
            img.save_with_format(dst, ImageFormat::Bmp)
                .map_err(|e| format!("BMP 写入失败: {}", e))?;
        }
        "tif" | "tiff" => {
            img.save_with_format(dst, ImageFormat::Tiff)
                .map_err(|e| format!("TIFF 写入失败: {}", e))?;
        }
        _ => return Err("不支持的目标格式".to_string()),
    }
    Ok(())
}
