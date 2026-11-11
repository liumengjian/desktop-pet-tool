// PDF 压缩：分层策略（参考已验证的 PyMuPDF 压缩路径，改为纯 Rust 实现）
//
// 顺序很重要——体积往往不在图片上：
//   1. 剥离设计软件私有数据（PieceInfo / Thumb / Metadata），Illustrator 导出的
//      画册里这类 /AIPDFPrivateData 流可达文件体积的一半；
//   2. 剔除因此无人引用的对象（garbage collection 等价物）；
//   3. 再对图片按「页面尺寸推导出的像素上界」重采样，转 JPEG；
//   4. 最后压缩未压缩的页面内容流。
//
// 只压图片往往只能压掉一半，必须先剥离再重采样。

use image::{DynamicImage, ImageBuffer, Luma, Rgb};
use lopdf::{Document, Object, ObjectId};
use std::path::Path;

use super::image_ops::encode_jpeg_bytes;

fn level_cfg(level: &str) -> (f32, u8) {
    // (目标 DPI, JPEG 质量)
    match level {
        "small" => (84.0, 60),
        "high" => (220.0, 88),
        _ => (150.0, 80),
    }
}

/// 超过这个体积改走流式重写，避免整份文件进内存
const STREAM_THRESHOLD: u64 = 64 * 1024 * 1024;
/// 压缩对象流的 PDF 无法逐对象重写，这个体积以内回退到整份载入仍安全
const OBJSTM_FALLBACK_LIMIT: u64 = 400 * 1024 * 1024;

fn page_size(doc: &Document, page_id: ObjectId) -> Option<(f32, f32)> {
    let mut current = Some(page_id);
    for _ in 0..12 {
        let id = current?;
        let dict = doc.objects.get(&id)?.as_dict().ok()?;
        if let Ok(arr) = dict.get(b"MediaBox").and_then(Object::as_array) {
            if arr.len() == 4 {
                let x1 = arr[0].as_f32().ok()?;
                let y1 = arr[1].as_f32().ok()?;
                let x2 = arr[2].as_f32().ok()?;
                let y2 = arr[3].as_f32().ok()?;
                return Some(((x2 - x1).abs(), (y2 - y1).abs()));
            }
        }
        current = dict.get(b"Parent").ok().and_then(|o| o.as_reference().ok());
    }
    None
}

fn filters_of(s: &lopdf::Stream) -> Option<Vec<Vec<u8>>> {
    s.filters()
        .ok()
        .map(|list| list.into_iter().map(|f| f.to_vec()).collect())
}

fn has_predictor(s: &lopdf::Stream) -> bool {
    if let Ok(parms) = s.dict.get(b"DecodeParms") {
        if let Ok(d) = parms.as_dict() {
            if let Ok(pred) = d.get(b"Predictor").and_then(Object::as_i64) {
                return pred != 1;
            }
        }
    }
    false
}

fn image_dims(s: &lopdf::Stream) -> Option<(u32, u32)> {
    let w = s.dict.get(b"Width").ok()?.as_i64().ok()?;
    let h = s.dict.get(b"Height").ok()?.as_i64().ok()?;
    if w <= 0 || h <= 0 {
        return None;
    }
    Some((w as u32, h as u32))
}

fn color_space(s: &lopdf::Stream) -> Option<Vec<u8>> {
    s.dict
        .get(b"ColorSpace")
        .ok()
        .and_then(|o| o.as_name().ok())
        .map(|n| n.to_vec())
}

/// 把图像流解码成 DynamicImage；无法安全处理的（CMYK / 预测器 / 调色板）返回 None
fn decode_image(s: &lopdf::Stream, filter: &[u8]) -> Option<DynamicImage> {
    if has_predictor(s) {
        return None;
    }
    let bits = s
        .dict
        .get(b"BitsPerComponent")
        .ok()
        .and_then(|o| o.as_i64().ok())
        .unwrap_or(8);
    if bits != 8 {
        return None;
    }
    let (w, h) = image_dims(s)?;
    let cs = color_space(s)?;
    if cs == b"DeviceCMYK" || cs == b"Indexed" {
        // CMYK 直接转 RGB 会有可见偏色，Indexed 需要调色板，均不在此处理
        return None;
    }

    if filter == b"DCTDecode" {
        // JPEG 直接交给解码器
        return image::load_from_memory(&s.content).ok();
    }
    if filter != b"FlateDecode" {
        return None;
    }

    // Flate 解码后是裸采样数据，不是图片文件
    let raw = s.decompressed_content().ok()?;
    match cs.as_slice() {
        b"DeviceRGB" => {
            let buf: ImageBuffer<Rgb<u8>, Vec<u8>> = ImageBuffer::from_raw(w, h, raw)?;
            Some(DynamicImage::ImageRgb8(buf))
        }
        b"DeviceGray" => {
            let buf: ImageBuffer<Luma<u8>, Vec<u8>> = ImageBuffer::from_raw(w, h, raw)?;
            Some(DynamicImage::ImageLuma8(buf))
        }
        _ => None,
    }
}

pub fn compress(
    src: &Path,
    dst: &Path,
    level: &str,
    cancelled: &dyn Fn() -> bool,
    progress: &dyn Fn(f32, &str),
) -> Result<(), String> {
    let (dpi, quality) = level_cfg(level);
    let size = std::fs::metadata(src).map(|m| m.len()).unwrap_or(0);

    // 大文件走流式重写：内存只与「最大单张图」相关，与文件大小无关
    if size > STREAM_THRESHOLD {
        return match super::pdf_stream::compress(src, dst, dpi, quality, cancelled, progress) {
            Ok(()) => Ok(()),
            Err(e) if e == super::pdf_stream::OBJSTM_MSG && size <= OBJSTM_FALLBACK_LIMIT => {
                // 压缩对象流的结构我们无法逐对象重写，但这个体积下整份载入还撑得住
                eprintln!("[docs] pdf: ObjStm 结构，回退到整份载入路径 ({})", size);
                compress_in_memory(src, dst, dpi, quality, cancelled, progress)
            }
            Err(e) => Err(e),
        };
    }

    compress_in_memory(src, dst, dpi, quality, cancelled, progress)
}

fn compress_in_memory(
    src: &Path,
    dst: &Path,
    dpi: f32,
    quality: u8,
    cancelled: &dyn Fn() -> bool,
    progress: &dyn Fn(f32, &str),
) -> Result<(), String> {
    progress(2.0, "读取 PDF");
    let mut doc = Document::load(src).map_err(|e| format!("PDF 解析失败: {}", e))?;
    if doc.trailer.get(b"Encrypt").is_ok() {
        return Err("加密 PDF 暂不支持压缩".to_string());
    }
    if cancelled() {
        return Err("已取消".to_string());
    }

    let pages = doc.get_pages();
    let mut max_pt: f32 = 0.0;
    for (_, id) in pages.iter() {
        if let Some((w, h)) = page_size(&doc, *id) {
            max_pt = max_pt.max(w.max(h));
        }
    }
    // 图片再大也不可能在页面上显示到超过整页尺寸，按整页尺寸 + 目标 DPI 推导像素上界
    let cap_px = if max_pt > 0.0 {
        ((max_pt / 72.0) * dpi).round().clamp(800.0, 4096.0) as u32
    } else {
        2000
    };

    // 1) 剥离设计软件私有数据与缩略图
    progress(8.0, "剥离私有数据");
    let page_ids: Vec<ObjectId> = pages.values().copied().collect();
    for id in &page_ids {
        if let Some(Object::Dictionary(d)) = doc.objects.get_mut(id) {
            d.remove(b"PieceInfo");
            d.remove(b"Thumb");
        }
    }
    if let Ok(root) = doc.trailer.get(b"Root").and_then(Object::as_reference) {
        if let Some(Object::Dictionary(d)) = doc.objects.get_mut(&root) {
            d.remove(b"PieceInfo");
            d.remove(b"Metadata");
        }
    }
    if cancelled() {
        return Err("已取消".to_string());
    }

    // 2) 剔除无人引用的对象（孤儿流，含刚被剥离的私有数据）
    progress(16.0, "清理冗余对象");
    let removed = doc.prune_objects().len();
    if cancelled() {
        return Err("已取消".to_string());
    }

    // 3) 图片按页面尺寸上界重采样
    let total_images = doc
        .objects
        .values()
        .filter(|o| match o {
            Object::Stream(s) => {
                s.dict.get(b"Subtype").ok().and_then(|x| x.as_name().ok()) == Some(b"Image")
            }
            _ => false,
        })
        .count();
    let mut seen = 0usize;
    let mut replaced = 0usize;
    let mut saved_bytes: i64 = 0;

    for obj in doc.objects.values_mut() {
        if let Object::Stream(s) = obj {
            let is_image =
                s.dict.get(b"Subtype").ok().and_then(|x| x.as_name().ok()) == Some(b"Image");
            if !is_image {
                continue;
            }
            seen += 1;
            if seen % 16 == 0 {
                if cancelled() {
                    return Err("已取消".to_string());
                }
                progress(
                    20.0 + 65.0 * (seen as f32 / total_images.max(1) as f32),
                    &format!("重采样图片 {}/{}", seen, total_images),
                );
            }
            if s.dict.get(b"SMask").is_ok() || s.dict.get(b"Mask").is_ok() {
                continue; // 带蒙版的不动，避免出现黑底或透明丢失
            }
            let filters = match filters_of(s) {
                Some(f) => f,
                None => continue,
            };
            if filters.len() != 1 {
                continue;
            }
            let (w, h) = match image_dims(s) {
                Some(v) => v,
                None => continue,
            };
            if w.max(h) <= cap_px {
                continue; // 已经不超过页面上限，重编码只会掉质量
            }
            let img = match decode_image(s, &filters[0]) {
                Some(i) => i,
                None => continue,
            };
            let long = w.max(h);
            let scale = cap_px as f32 / long as f32;
            let nw = ((w as f32 * scale).round() as u32).max(1);
            let nh = ((h as f32 * scale).round() as u32).max(1);
            let resized = img.resize_exact(nw, nh, image::imageops::FilterType::Lanczos3);
            let jpeg = match encode_jpeg_bytes(&resized, quality) {
                Ok(b) => b,
                Err(_) => continue,
            };
            // 只有确实变小才替换
            if jpeg.len() >= s.content.len() {
                continue;
            }
            saved_bytes += s.content.len() as i64 - jpeg.len() as i64;
            s.set_plain_content(jpeg);
            s.dict.set("Filter", Object::Name(b"DCTDecode".to_vec()));
            s.dict.set("Width", Object::Integer(nw as i64));
            s.dict.set("Height", Object::Integer(nh as i64));
            s.dict.set("ColorSpace", Object::Name(b"DeviceRGB".to_vec()));
            s.dict.set("BitsPerComponent", Object::Integer(8));
            s.allows_compression = false;
            replaced += 1;
        }
    }

    // 4) 页面内容流仍为明文时压缩一次
    progress(88.0, "压缩内容流");
    for id in pages.values() {
        for cid in doc.get_page_contents(*id) {
            if let Some(Object::Stream(s)) = doc.objects.get_mut(&cid) {
                if s.dict.get(b"Filter").is_err() {
                    let _ = s.compress();
                }
            }
        }
    }

    if cancelled() {
        return Err("已取消".to_string());
    }
    progress(94.0, "写入输出文件");
    doc.save(dst).map_err(|e| format!("PDF 写入失败: {}", e))?;

    eprintln!(
        "[docs] pdf compress: removed_objects={} images_replaced={} image_bytes_saved={}",
        removed, replaced, saved_bytes
    );
    progress(100.0, "完成");
    Ok(())
}
