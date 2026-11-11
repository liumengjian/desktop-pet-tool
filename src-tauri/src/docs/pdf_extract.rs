// PDF 内容流文字抽取（纯 Rust，不依赖 PDFium）。
//
// 只做「取字」这一件事：解析内容流里的 Tf/Td/TD/Tm/T*/Tj/TJ/'/"，
// 借助各字体自己的 /ToUnicode CMap 把字形码翻译回 Unicode。
// 扫描件（页面只有图片、没有文字算子）抽不出字，由调用方按降级处理。

use std::collections::HashMap;
use std::path::Path;

use lopdf::{Document, Object};

pub struct PageText {
    pub page: u32,
    pub lines: Vec<String>,
}

/// 字体：码长（1 或 2 字节） + 码 → Unicode 映射
struct FontMap {
    two_byte: bool,
    map: HashMap<u32, String>,
}

/// CMap 词法：hex 串（<> 内）与数组括号
enum CmapTok {
    Hex(String),
    ArrStart,
    ArrEnd,
}

fn cmap_tokens(body: &str) -> Vec<CmapTok> {
    let mut out = Vec::new();
    let mut chars = body.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '<' => {
                let mut hex = String::new();
                while let Some(&c) = chars.peek() {
                    chars.next();
                    if c == '>' {
                        break;
                    }
                    if !c.is_whitespace() {
                        hex.push(c);
                    }
                }
                if !hex.is_empty() {
                    out.push(CmapTok::Hex(hex));
                }
            }
            '[' => out.push(CmapTok::ArrStart),
            ']' => out.push(CmapTok::ArrEnd),
            _ => {}
        }
    }
    out
}

fn hex_code(text: &str) -> Option<u32> {
    if text.is_empty() || text.len() > 8 {
        return None;
    }
    u32::from_str_radix(text, 16).ok()
}

fn parse_cmap(text: &str) -> HashMap<u32, String> {
    let mut map = HashMap::new();

    let mut rest = text;
    while let Some(pos) = rest.find("beginbfchar") {
        let body_start = pos + "beginbfchar".len();
        let end = match rest[body_start..].find("endbfchar") {
            Some(e) => body_start + e,
            None => break,
        };
        let tokens = cmap_tokens(&rest[body_start..end]);
        let mut i = 0;
        while i + 1 < tokens.len() {
            if let (CmapTok::Hex(code), CmapTok::Hex(target)) = (&tokens[i], &tokens[i + 1]) {
                if let (Some(code), Some(text)) = (hex_code(code), hex_to_utf16(target)) {
                    map.insert(code, text);
                }
            }
            i += 2;
        }
        rest = &rest[end + "endbfchar".len()..];
    }

    let mut rest = text;
    while let Some(pos) = rest.find("beginbfrange") {
        let body_start = pos + "beginbfrange".len();
        let end = match rest[body_start..].find("endbfrange") {
            Some(e) => body_start + e,
            None => break,
        };
        let tokens = cmap_tokens(&rest[body_start..end]);
        let mut i = 0;
        while i + 2 < tokens.len() {
            let lo = match &tokens[i] {
                CmapTok::Hex(h) => hex_code(h),
                _ => None,
            };
            let hi = match &tokens[i + 1] {
                CmapTok::Hex(h) => hex_code(h),
                _ => None,
            };
            let (Some(lo), Some(hi)) = (lo, hi) else {
                i += 1;
                continue;
            };
            let count = hi.saturating_sub(lo).min(4096);
            match &tokens[i + 2] {
                CmapTok::Hex(base) => {
                    if let Some(text) = hex_to_utf16(base) {
                        for step in 0..=count {
                            map.insert(lo + step, shift_last_char(&text, step));
                        }
                    }
                    i += 3;
                }
                CmapTok::ArrStart => {
                    let mut step = 0u32;
                    let mut k = i + 3;
                    while k < tokens.len() {
                        match &tokens[k] {
                            CmapTok::Hex(h) => {
                                if let Some(text) = hex_to_utf16(h) {
                                    if step <= count {
                                        map.insert(lo + step, text);
                                    }
                                }
                                step += 1;
                                k += 1;
                            }
                            CmapTok::ArrEnd => {
                                k += 1;
                                break;
                            }
                            CmapTok::ArrStart => k += 1,
                        }
                    }
                    i = k;
                }
                CmapTok::ArrEnd => i += 3,
            }
        }
        rest = &rest[end + "endbfrange".len()..];
    }
    map
}

/// 把 UTF-16 编码的十六进制串转成字符串
fn hex_to_utf16(hex: &str) -> Option<String> {
    let clean: String = hex.chars().filter(|c| !c.is_whitespace()).collect();
    if clean.is_empty() || clean.len() % 4 != 0 {
        return None;
    }
    let mut units = Vec::with_capacity(clean.len() / 4);
    let mut i = 0;
    while i < clean.len() {
        let unit = u16::from_str_radix(&clean[i..i + 4], 16).ok()?;
        units.push(unit);
        i += 4;
    }
    let text: String = char::decode_utf16(units).filter_map(|r| r.ok()).collect();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

/// bfrange 连续递增：把最后一位字符加 step
fn shift_last_char(base: &str, step: u32) -> String {
    let mut chars: Vec<char> = base.chars().collect();
    if let Some(last) = chars.last_mut() {
        if let Some(code) = char::from_u32(*last as u32 + step) {
            *last = code;
        }
    }
    chars.into_iter().collect()
}

fn stream_bytes(object: &Object) -> Option<Vec<u8>> {
    match object {
        Object::Stream(stream) => {
            // 有 /Filter 才需要解压；lopdf 对无 filter 的流 decompressed_content 会返回空
            if stream.dict.get(b"Filter").is_ok() {
                stream.decompressed_content().ok()
            } else {
                Some(stream.content.clone())
            }
        }
        _ => None,
    }
}

fn font_map(doc: &Document, font: &lopdf::Dictionary) -> FontMap {
    let subtype = font
        .get(b"Subtype")
        .ok()
        .and_then(|o| o.as_name().ok())
        .map(|n| n.to_vec())
        .unwrap_or_default();
    let encoding = font
        .get(b"Encoding")
        .ok()
        .and_then(|o| o.as_name().ok())
        .map(|n| n.to_vec())
        .unwrap_or_default();
    let two_byte =
        subtype == b"Type0" || encoding == b"Identity-H" || encoding == b"Identity-V";

    let mut map = HashMap::new();
    if let Ok(obj) = font.get(b"ToUnicode") {
        let target = match obj {
            Object::Reference(id) => doc.get_object(*id).ok(),
            other => Some(other),
        };
        if let Some(data) = target.and_then(stream_bytes) {
            if !data.is_empty() {
                let text = String::from_utf8_lossy(&data).to_string();
                map = parse_cmap(&text);
            }
        }
    }
    FontMap { two_byte, map }
}

enum Operand {
    Num(f32),
    Str(Vec<u8>),
    Name(Vec<u8>),
}

fn normalize(text: &str) -> String {
    // 很多 PDF 把空格映射成不换行空格/零宽字符，统一成普通空格，否则文本比较和搜索都会失效
    text.replace('\u{a0}', " ")
        .replace(['\u{200b}', '\u{200c}', '\u{200d}', '\u{feff}'], "")
}

fn decode_str(bytes: &[u8], font: &FontMap) -> String {
    let mut out = String::new();
    if font.two_byte {
        let mut i = 0;
        while i + 1 < bytes.len() {
            let code = ((bytes[i] as u32) << 8) | bytes[i + 1] as u32;
            if let Some(text) = font.map.get(&code) {
                out.push_str(text);
            }
            i += 2;
        }
    } else {
        for byte in bytes {
            let code = *byte as u32;
            if let Some(text) = font.map.get(&code) {
                out.push_str(text);
            }
        }
    }
    normalize(&out)
}

/// 粗略词法：把内容流切成操作数与算符，按文本算符累积行
fn run_content(content: &[u8], fonts: &HashMap<Vec<u8>, FontMap>) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut operands: Vec<Operand> = Vec::new();
    let mut active: Option<Vec<u8>> = None; // 当前字体名
    let mut i = 0usize;
    let len = content.len();

    let flush_line = |current: &mut String, lines: &mut Vec<String>| {
        let trimmed = current.trim();
        if !trimmed.is_empty() {
            lines.push(trimmed.to_string());
        }
        current.clear();
    };

    while i < len {
        let b = content[i];
        match b {
            b' ' | b'\n' | b'\r' | b'\t' | 0 | 0x0C => {
                i += 1;
            }
            b'%' => {
                while i < len && content[i] != b'\n' {
                    i += 1;
                }
            }
            b'(' => {
                i += 1;
                let mut depth = 1;
                let mut bytes = Vec::new();
                while i < len {
                    let c = content[i];
                    if c == b'\\' {
                        i += 1;
                        if i >= len {
                            break;
                        }
                        let esc = content[i];
                        match esc {
                            b'n' => bytes.push(b'\n'),
                            b'r' => bytes.push(b'\r'),
                            b't' => bytes.push(b'\t'),
                            b'b' => bytes.push(8),
                            b'f' => bytes.push(12),
                            b'(' => bytes.push(b'('),
                            b')' => bytes.push(b')'),
                            b'\\' => bytes.push(b'\\'),
                            b'0'..=b'7' => {
                                let mut value = 0u32;
                                let mut taken = 0;
                                while taken < 3 && i < len && (b'0'..=b'7').contains(&content[i]) {
                                    value = value * 8 + (content[i] - b'0') as u32;
                                    i += 1;
                                    taken += 1;
                                }
                                bytes.push(value as u8);
                                continue;
                            }
                            b'\n' => {}
                            other => bytes.push(other),
                        }
                        i += 1;
                        continue;
                    }
                    if c == b'(' {
                        depth += 1;
                        bytes.push(c);
                    } else if c == b')' {
                        depth -= 1;
                        if depth == 0 {
                            i += 1;
                            break;
                        }
                        bytes.push(c);
                    } else {
                        bytes.push(c);
                    }
                    i += 1;
                }
                operands.push(Operand::Str(bytes));
            }
            b'<' => {
                if i + 1 < len && content[i + 1] == b'<' {
                    i += 2;
                    continue;
                }
                i += 1;
                let mut hex = String::new();
                while i < len && content[i] != b'>' {
                    let c = content[i];
                    if !c.is_ascii_whitespace() {
                        hex.push(c as char);
                    }
                    i += 1;
                }
                i += 1;
                let bytes = hex_string(&hex);
                operands.push(Operand::Str(bytes));
            }
            b'>' => {
                i += 1;
            }
            b'[' | b']' => {
                i += 1;
            }
            b'/' => {
                i += 1;
                let mut name = Vec::new();
                while i < len && !is_delimiter(content[i]) {
                    name.push(content[i]);
                    i += 1;
                }
                operands.push(Operand::Name(name));
            }
            b')' => {
                i += 1;
            }
            b'B' if i + 1 < len && content[i + 1] == b'I' => {
                // 内联图片：跳到 EI
                i += 2;
                while i + 1 < len {
                    if content[i] == b'E' && content[i + 1] == b'I' {
                        i += 2;
                        break;
                    }
                    i += 1;
                }
                operands.clear();
            }
            _ => {
                let mut token = Vec::new();
                while i < len && !is_delimiter(content[i]) {
                    token.push(content[i]);
                    i += 1;
                }
                let op = String::from_utf8_lossy(&token).to_string();
                if op.is_empty() {
                    i += 1;
                    continue;
                }
                if token
                    .first()
                    .map(|c| c.is_ascii_digit() || *c == b'-' || *c == b'+' || *c == b'.')
                    .unwrap_or(false)
                {
                    let value: f32 = op.parse().unwrap_or(0.0);
                    operands.push(Operand::Num(value));
                    continue;
                }
                match op.as_str() {
                    "Tf" => {
                        let mut name: Option<Vec<u8>> = None;
                        for operand in operands.iter() {
                            if let Operand::Name(n) = operand {
                                name = Some(n.clone());
                            }
                        }
                        active = name;
                    }
                    "Td" | "TD" => {
                        if let Some(Operand::Num(ty)) = operands.last() {
                            if ty.abs() > 0.5 {
                                flush_line(&mut current, &mut lines);
                            }
                        }
                    }
                    "Tm" => {
                        if operands.len() >= 6 {
                            flush_line(&mut current, &mut lines);
                        }
                    }
                    "T*" => flush_line(&mut current, &mut lines),
                    "Tj" | "TJ" | "'" | "\"" => {
                        if op == "'" || op == "\"" {
                            flush_line(&mut current, &mut lines);
                        }
                        if let Some(font) = active.as_ref().and_then(|n| fonts.get(n)) {
                            for operand in operands.iter() {
                                if let Operand::Str(bytes) = operand {
                                    current.push_str(&decode_str(bytes, font));
                                }
                            }
                        }
                    }
                    _ => {}
                }
                operands.clear();
            }
        }
    }
    flush_line(&mut current, &mut lines);
    lines
}

fn is_delimiter(c: u8) -> bool {
    matches!(
        c,
        b' ' | b'\n' | b'\r' | b'\t' | 0 | 0x0C | b'/' | b'[' | b']' | b'<' | b'>' | b'(' | b')' | b'%'
    )
}

fn hex_string(hex: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let mut buffer = String::new();
    for ch in hex.chars() {
        buffer.push(ch);
        if buffer.len() == 2 {
            if let Ok(v) = u8::from_str_radix(&buffer, 16) {
                out.push(v);
            }
            buffer.clear();
        }
    }
    if buffer.len() == 1 {
        if let Ok(v) = u8::from_str_radix(&format!("{}0", buffer), 16) {
            out.push(v);
        }
    }
    out
}

pub fn extract(src: &Path, cancelled: &dyn Fn() -> bool) -> Result<Vec<PageText>, String> {
    let doc = Document::load(src).map_err(|e| format!("无法解析该 PDF：{}", e))?;
    if doc.is_encrypted() {
        return Err("该 PDF 已加密，暂不支持取字".to_string());
    }
    let pages = doc.get_pages();
    let mut out = Vec::new();
    for (number, page_id) in pages {
        if cancelled() {
            return Err("已取消".to_string());
        }
        let fonts = doc.get_page_fonts(page_id).unwrap_or_default();
        let fonts_for_page: HashMap<Vec<u8>, FontMap> = fonts
            .iter()
            .map(|(name, dict)| (name.clone(), font_map(&doc, dict)))
            .collect();
        let content = doc.get_page_content(page_id).unwrap_or_default();
        let lines = run_content(&content, &fonts_for_page);
        out.push(PageText { page: number, lines });
    }
    Ok(out)
}

/// 是否有任何页面抽出了文字（判断扫描件用）
pub fn has_text(pages: &[PageText]) -> bool {
    pages.iter().any(|p| !p.lines.is_empty())
}
