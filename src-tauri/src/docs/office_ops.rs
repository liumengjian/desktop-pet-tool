// Office 容器压缩：docx/xlsx/pptx 本质是 ZIP。
// 两步收益：1) 全量以 deflate 最高级别重打包；2) 内嵌图片（media/）重编码。

use image::DynamicImage;
use std::collections::BTreeSet;
use std::path::Path;

use super::image_ops::{encode_jpeg_bytes, encode_png_bytes};
use super::zip_min::{read_zip, write_zip, OutEntry};

/// 压缩档位：内嵌图片长边上限 + JPEG 质量。
/// 「更小」按 1024px / q52 处理：Office 里的配图绝大多数远大于这个尺寸，
/// 只降一档（1280/62）在缩略图上几乎看不出差别，体积却差一截。
/// 可以整块丢掉的可选部件：
/// - `word/stylesWithEffects.xml`：Word 2010 遗留的样式快照，现代 Word/WPS 打开用不到，
///   但它常常有几百 KB 明文（压完仍占十几 KB），是纯文字文档最大的冗余来源；
/// - `docProps/thumbnail.*`：资源管理器预览图，删了只影响文件夹缩略图。
fn is_removable_part(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower == "word/styleswitheffects.xml" || lower.starts_with("docprops/thumbnail.")
}

fn attr_of(tag: &str, key: &str) -> Option<String> {
    let pat = format!("{}=\"", key);
    let start = tag.find(&pat)? + pat.len();
    let rest = &tag[start..];
    let end = rest.find('"')?;
    Some(rest[..end].replace("&amp;", "&"))
}

/// 把 rels 里的相对 Target 解析成包内绝对路径（`word/_rels/document.xml.rels` 的基准是 `word`）
fn resolve_part(base: &str, target: &str) -> String {
    if let Some(p) = target.strip_prefix('/') {
        return p.to_string();
    }
    let mut parts: Vec<String> = base
        .split('/')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect();
    for seg in target.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s.to_string()),
        }
    }
    parts.join("/")
}

/// 删除指向被丢弃部件的 `<Tag .../>` 元素（改写 rels / [Content_Types].xml 用）
fn drop_elements(xml: &str, tag: &str, attr: &str, base: &str, dropped: &BTreeSet<String>) -> String {
    let open = format!("<{}", tag);
    let mut out = String::with_capacity(xml.len());
    let mut idx = 0usize;
    while let Some(rel) = xml[idx..].find(&open) {
        let start = idx + rel;
        let after = xml[start + open.len()..].chars().next();
        // `<Relationships` 不能被当成 `<Relationship`
        if !matches!(after, Some(' ') | Some('\t') | Some('\r') | Some('\n') | Some('/')) {
            out.push_str(&xml[idx..start + open.len()]);
            idx = start + open.len();
            continue;
        }
        let Some(e) = xml[start..].find("/>") else {
            break;
        };
        let end = start + e + 2;
        let elem = &xml[start..end];
        out.push_str(&xml[idx..start]);
        let keep = match attr_of(elem, attr) {
            Some(target) => !dropped.contains(&resolve_part(base, &target)),
            None => true,
        };
        if keep {
            out.push_str(elem);
        }
        idx = end;
    }
    out.push_str(&xml[idx..]);
    out
}

/// 收集包内悬空引用（引用存在但目标部件缺失）。
/// 源文件本身可能就带历史遗留的悬空 Override（例如极简模板），
/// 所以调用方用「新增的悬空引用」来判断删减是否安全，而不是要求绝对为零。
fn dangling_refs(entries: &[(String, Vec<u8>)]) -> Vec<String> {
    let names: BTreeSet<String> = entries.iter().map(|(n, _)| n.clone()).collect();
    let mut bad: Vec<String> = Vec::new();
    for (name, data) in entries {
        let xml = String::from_utf8_lossy(data);
        if name.ends_with(".rels") {
            let base = name
                .split_once("_rels/")
                .map(|(d, _)| d.trim_end_matches('/'))
                .unwrap_or("");
            let mut idx = 0usize;
            while let Some(rel) = xml[idx..].find("<Relationship") {
                let start = idx + rel;
                let Some(e) = xml[start..].find("/>") else {
                    break;
                };
                let elem = &xml[start..start + e + 2];
                if !elem.contains("TargetMode=\"External\"") {
                    if let Some(t) = attr_of(elem, "Target") {
                        if !names.contains(&resolve_part(base, &t)) {
                            bad.push(format!("{} -> {}", name, t));
                        }
                    }
                }
                idx = start + e + 2;
            }
        } else if name == "[Content_Types].xml" {
            let mut idx = 0usize;
            while let Some(rel) = xml[idx..].find("<Override") {
                let start = idx + rel;
                let Some(e) = xml[start..].find("/>") else {
                    break;
                };
                let elem = &xml[start..start + e + 2];
                if let Some(p) = attr_of(elem, "PartName") {
                    let p = p.trim_start_matches('/').to_string();
                    if !names.contains(&p) {
                        bad.push(format!("{} -> {}", name, p));
                    }
                }
                idx = start + e + 2;
            }
        }
    }
    bad
}

fn level_cfg(level: &str) -> (u32, u8) {
    match level {
        "small" => (1024, 52),
        "high" => (2560, 88),
        _ => (1600, 72),
    }
}

fn is_media(name: &str) -> bool {
    let lower = name.to_lowercase();
    (lower.contains("/media/") || lower.starts_with("media/"))
        && (lower.ends_with(".png")
            || lower.ends_with(".jpg")
            || lower.ends_with(".jpeg")
            || lower.ends_with(".bmp")
            || lower.ends_with(".tif")
            || lower.ends_with(".tiff"))
}

/// 重编码内嵌图片；只有确实变小时才替换，失败一律保持原样
fn recompress_media(data: &[u8], level: &str) -> Option<Vec<u8>> {
    let (max_edge, quality) = level_cfg(level);
    let img: DynamicImage = image::load_from_memory(data).ok()?;
    let resized = if img.width().max(img.height()) > max_edge {
        img.resize(max_edge, max_edge, image::imageops::FilterType::Lanczos3)
    } else {
        img
    };
    // 同格式重编码：png 仍为 png，jpg 仍为 jpg，避免改扩展名导致 Office 打不开
    let is_png = data.starts_with(&[0x89, b'P', b'N', b'G']);
    let bytes = if is_png {
        encode_png_bytes(&resized).ok()?
    } else {
        encode_jpeg_bytes(&resized, quality).ok()?
    };
    if bytes.len() < data.len() {
        Some(bytes)
    } else {
        None
    }
}

pub fn compress(
    src: &Path,
    dst: &Path,
    level: &str,
    cancelled: &dyn Fn() -> bool,
    progress: &dyn Fn(f32, &str),
) -> Result<(), String> {
    let entries = read_zip(src)?;
    let total = entries.len();
    let mut raw: Vec<(String, Vec<u8>)> = Vec::with_capacity(total);

    for (i, entry) in entries.iter().enumerate() {
        if cancelled() {
            return Err("已取消".to_string());
        }
        if i % 8 == 0 {
            progress(
                (100.0 * i as f32 / total.max(1) as f32).min(99.0),
                &format!("重打包 {}/{}", i, total),
            );
        }
        if entry.is_dir() {
            raw.push((entry.name.clone(), Vec::new()));
            continue;
        }
        let mut data = entry.bytes()?;
        if is_media(&entry.name) {
            if let Some(new) = recompress_media(&data, level) {
                data = new;
            }
        }
        raw.push((entry.name.clone(), data));
    }

    // 「更小 / 标准」再清掉可选部件（更高清晰度档保留原样，避免任何观感损失）
    if level != "high" {
        let dropped: BTreeSet<String> = raw
            .iter()
            .map(|(n, _)| n.clone())
            .filter(|n| is_removable_part(n))
            .collect();
        if !dropped.is_empty() {
            progress(50.0, "清理可选部件");
            let mut slim: Vec<(String, Vec<u8>)> = Vec::with_capacity(raw.len());
            for (name, data) in &raw {
                if dropped.contains(name) {
                    continue;
                }
                if name == "[Content_Types].xml" {
                    let xml = String::from_utf8_lossy(data);
                    let out = drop_elements(&xml, "Override", "PartName", "", &dropped);
                    slim.push((name.clone(), out.into_bytes()));
                } else if name.ends_with(".rels") {
                    let base = name
                        .split_once("_rels/")
                        .map(|(d, _)| d.trim_end_matches('/'))
                        .unwrap_or("");
                    let xml = String::from_utf8_lossy(data);
                    let out = drop_elements(&xml, "Relationship", "Target", base, &dropped);
                    slim.push((name.clone(), out.into_bytes()));
                } else {
                    slim.push((name.clone(), data.clone()));
                }
            }
            // 只要没引入新的悬空引用就采用删减版，否则退回原结构
            let before: Vec<String> = dangling_refs(&raw);
            let after: Vec<String> = dangling_refs(&slim);
            if after.iter().all(|d| before.contains(d)) {
                raw = slim;
            }
        }
    }

    let baseline: Vec<String> = dangling_refs(&raw);

    let packed: Vec<OutEntry> = raw
        .into_iter()
        .map(|(name, data)| OutEntry { name, data })
        .collect();
    let expected = packed.len();
    write_zip(packed, dst)?;

    // 结构校验：重开产物，[Content_Types].xml 仍在、部件数与预期一致、没有新增悬空引用
    let check = read_zip(dst)?;
    if check.len() != expected {
        return Err(format!("容器条目数不一致（{} / {}）", check.len(), expected));
    }
    if !check.iter().any(|e| e.name == "[Content_Types].xml") {
        return Err("缺少 [Content_Types].xml".to_string());
    }
    let reread: Vec<(String, Vec<u8>)> = check
        .iter()
        .map(|e| (e.name.clone(), e.bytes().unwrap_or_default()))
        .collect();
    let now: Vec<String> = dangling_refs(&reread);
    if let Some(bad) = now.iter().find(|d| !baseline.contains(d)) {
        return Err(format!("产物引用悬空：{}", bad));
    }

    progress(100.0, "完成");
    Ok(())
}
