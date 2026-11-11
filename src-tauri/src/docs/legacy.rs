// 开放格式与旧格式的文字抽取：RTF / ODT / ODS / ODP / EPUB。
//
// 目标只是「拿到可读文字」，不做版式还原；输出的结构块直接复用 text_ops::Block，
// 因此可以落到 txt / md / html / docx / pdf 任意方向。

use std::path::Path;

use super::office_write;
use super::text_ops::Block;
use super::to_pdf;
use super::zip_min;

// ============== RTF ==============

/// 需要整组跳过的目的地（字体表、样式表、图片、文档信息等）
fn rtf_skip_destination(word: &str) -> bool {
    matches!(
        word,
        "fonttbl"
            | "colortbl"
            | "stylesheet"
            | "info"
            | "pict"
            | "object"
            | "header"
            | "footer"
            | "headerl"
            | "headerr"
            | "footerl"
            | "footerr"
            | "listtable"
            | "listoverridetable"
            | "rsidtbl"
            | "generator"
            | "themedata"
            | "colorschememapping"
            | "latentstyles"
            | "datastore"
            | "xmlnstbl"
            | "fldinst"
    )
}

fn decode_escaped(bytes: &[u8], codepage: u32) -> String {
    if codepage == 936 || codepage == 54936 {
        let (text, _, _) = encoding_rs::GBK.decode(bytes);
        return text.to_string();
    }
    if codepage == 950 {
        let (text, _, _) = encoding_rs::BIG5.decode(bytes);
        return text.to_string();
    }
    if codepage == 932 {
        let (text, _, _) = encoding_rs::SHIFT_JIS.decode(bytes);
        return text.to_string();
    }
    let text = String::from_utf8_lossy(bytes).to_string();
    if text.contains('\u{FFFD}') {
        // 西欧等单字节代码页：按字节近似呈现，避免整段丢字
        bytes
            .iter()
            .map(|b| match *b {
                0x80..=0x8F => ' ',
                b if b >= 0x20 => b as char,
                _ => ' ',
            })
            .collect()
    } else {
        text
    }
}

/// RTF → 结构块
pub fn rtf_blocks(src: &Path) -> Result<Vec<Block>, String> {
    let raw = std::fs::read(src).map_err(|e| format!("读取失败: {}", e))?;
    let text = String::from_utf8_lossy(&raw).to_string();
    let mut blocks: Vec<Block> = Vec::new();
    let mut line = String::new();
    let mut codepage: u32 = 1252;
    let mut uc_skip: i32 = 1;
    let mut skip_depth: usize = 0;
    let mut depth: usize = 0;
    let mut pending_bytes: Vec<u8> = Vec::new();

    let flush_bytes = |pending: &mut Vec<u8>, line: &mut String, codepage: u32| {
        if !pending.is_empty() {
            let decoded = decode_escaped(pending, codepage);
            line.push_str(&decoded);
            pending.clear();
        }
    };

    let flush_line = |line: &mut String, blocks: &mut Vec<Block>| {
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            blocks.push(Block::Para(trimmed.to_string()));
        }
        line.clear();
    };

    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let ch = chars[i];
        match ch {
            '{' => {
                depth += 1;
                // 检查是否是 \* 或需要跳过的目的地组
                let mut j = i + 1;
                while j < chars.len() && chars[j].is_whitespace() {
                    j += 1;
                }
                let mut word = String::new();
                if j < chars.len() && chars[j] == '\\' {
                    let mut k = j + 1;
                    while k < chars.len() && chars[k].is_ascii_alphabetic() {
                        word.push(chars[k]);
                        k += 1;
                    }
                    if word == "*" || rtf_skip_destination(&word) {
                        if skip_depth == 0 {
                            skip_depth = depth;
                        }
                    }
                }
                i += 1;
            }
            '}' => {
                if skip_depth == depth {
                    skip_depth = 0;
                }
                depth = depth.saturating_sub(1);
                i += 1;
            }
            '\\' => {
                if i + 1 >= chars.len() {
                    break;
                }
                let next = chars[i + 1];
                if next.is_ascii_alphabetic() {
                    let mut word = String::new();
                    let mut k = i + 1;
                    while k < chars.len() && chars[k].is_ascii_alphabetic() {
                        word.push(chars[k]);
                        k += 1;
                    }
                    // 可选数字参数（含负号）
                    let mut num = String::new();
                    if k < chars.len() && (chars[k] == '-' || chars[k].is_ascii_digit()) {
                        while k < chars.len()
                            && (chars[k] == '-' || chars[k].is_ascii_digit())
                        {
                            num.push(chars[k]);
                            k += 1;
                        }
                    }
                    // 控制字后的单个空格属于分隔符，吞掉
                    if k < chars.len() && chars[k] == ' ' {
                        k += 1;
                    }
                    if skip_depth == 0 {
                        match word.as_str() {
                            "par" | "line" | "row" => flush_line(&mut line, &mut blocks),
                            "tab" | "cell" => line.push('\t'),
                            "u" => {
                                let value: i64 = num.parse().unwrap_or(0);
                                let code = if value < 0 { value + 65536 } else { value };
                                if let Some(c) = char::from_u32(code as u32) {
                                    flush_bytes(&mut pending_bytes, &mut line, codepage);
                                    line.push(c);
                                }
                                // \uN 后面跟 uc 个替代字符（默认 1），必须跳过，
                                // 否则中文会变成「?标?行」这类插着问号的文本
                                let mut skip = uc_skip as usize;
                                let mut m = k;
                                // 先看是否紧跟 \ucN 覆盖跳过量
                                if m + 1 < chars.len() && chars[m] == '\\' {
                                    let mut word2 = String::new();
                                    let mut p = m + 1;
                                    while p < chars.len() && chars[p].is_ascii_alphabetic() {
                                        word2.push(chars[p]);
                                        p += 1;
                                    }
                                    if word2 == "uc" {
                                        let mut digits = String::new();
                                        while p < chars.len()
                                            && (chars[p] == '-' || chars[p].is_ascii_digit())
                                        {
                                            digits.push(chars[p]);
                                            p += 1;
                                        }
                                        if p < chars.len() && chars[p] == ' ' {
                                            p += 1;
                                        }
                                        if let Ok(v) = digits.parse::<i64>() {
                                            skip = v.clamp(0, 4) as usize;
                                        }
                                        m = p;
                                    }
                                }
                                let mut skipped = 0;
                                while skipped < skip
                                    && m < chars.len()
                                    && !matches!(chars[m], '\\' | '{' | '}')
                                {
                                    m += 1;
                                    skipped += 1;
                                }
                                k = m;
                                flush_bytes(&mut pending_bytes, &mut line, codepage);
                            }
                            "ansicpg" => {
                                if let Ok(v) = num.parse::<u32>() {
                                    codepage = v;
                                }
                            }
                            "uc" => {
                                if let Ok(v) = num.parse::<i32>() {
                                    uc_skip = v.clamp(0, 4);
                                }
                            }
                            "emdash" => line.push('—'),
                            "endash" => line.push('–'),
                            "lquote" => line.push('‘'),
                            "rquote" => line.push('’'),
                            "ldblquote" => line.push('“'),
                            "rdblquote" => line.push('”'),
                            "bullet" => line.push('·'),
                            _ => {}
                        }
                    }
                    i = k;
                    continue;
                }
                if skip_depth == 0 {
                    match next {
                        '\'' => {
                            let hex: String = chars.iter().skip(i + 2).take(2).collect();
                            if let Ok(v) = u8::from_str_radix(&hex, 16) {
                                pending_bytes.push(v);
                            }
                            i += 4;
                            continue;
                        }
                        '\\' => line.push('\\'),
                        '{' => line.push('{'),
                        '}' => line.push('}'),
                        '~' => line.push(' '),
                        '_' => line.push('-'),
                        '\n' | '\r' => {}
                        _ => {}
                    }
                }
                i += 2;
                continue;
            }
            '\r' => {
                i += 1;
                continue;
            }
            '\n' => {
                if skip_depth == 0 {
                    flush_line(&mut line, &mut blocks);
                }
                i += 1;
                continue;
            }
            _ => {
                if skip_depth == 0 {
                    flush_bytes(&mut pending_bytes, &mut line, codepage);
                    line.push(ch);
                }
                i += 1;
                continue;
            }
        }
    }
    flush_bytes(&mut pending_bytes, &mut line, codepage);
    flush_line(&mut line, &mut blocks);

    if blocks.is_empty() {
        return Err("RTF 里没有可提取的文字".to_string());
    }
    Ok(blocks)
}

// ============== ODF（odt / ods / odp） ==============

fn decode_entities(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}

fn strip_tags(fragment: &str) -> String {
    let mut out = String::with_capacity(fragment.len());
    let mut in_tag = false;
    for ch in fragment.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    decode_entities(&out)
}

/// 从 content.xml 里抽段落、标题与表格文字
pub fn odf_blocks(src: &Path) -> Result<Vec<Block>, String> {
    let entries = zip_min::read_zip(src).map_err(|e| format!("打开 ODF 失败: {}", e))?;
    let content = entries
        .iter()
        .find(|e| e.name == "content.xml")
        .and_then(|e| e.bytes().ok())
        .ok_or("ODF 包里没有 content.xml")?;
    let xml = String::from_utf8_lossy(&content).to_string();

    let mut blocks: Vec<Block> = Vec::new();
    let mut rest = xml.as_str();
    while let Some(pos) = rest.find('<') {
        rest = &rest[pos..];
        // 段落 / 标题
        let mut consumed = false;
        for (open, close, heading) in [
            ("<text:h", "</text:h>", true),
            ("<text:p", "</text:p>", false),
            ("<text:list-item", "</text:list-item>", false),
        ] {
            if rest.starts_with(open) {
                let Some(end) = rest.find(close) else {
                    break;
                };
                let body = &rest[..end];
                let level = if heading {
                    body
                        .find("text:outline-level=\"")
                        .and_then(|p| body[p + 20..].split('"').next())
                        .and_then(|v| v.parse::<u8>().ok())
                        .unwrap_or(1)
                } else {
                    0
                };
                let text = strip_tags(body).trim().to_string();
                if !text.is_empty() {
                    if heading {
                        blocks.push(Block::Heading(level.min(3), text));
                    } else if rest.starts_with("<text:list-item") {
                        blocks.push(Block::Item(text));
                    } else {
                        blocks.push(Block::Para(text));
                    }
                }
                rest = &rest[end + close.len()..];
                consumed = true;
                break;
            }
        }
        if consumed {
            continue;
        }
        // 表格行
        if rest.starts_with("<table:table-row") || rest.starts_with("<table:table ") {
            let Some(end) = rest.find("</table:table-row>") else {
                rest = &rest[1..];
                continue;
            };
            let row_xml = &rest[..end];
            let mut cells: Vec<String> = Vec::new();
            let mut cell_rest = row_xml;
            while let Some(cpos) = cell_rest.find("<table:table-cell") {
                let after = &cell_rest[cpos..];
                let Some(cend) = after.find("</table:table-cell>") else {
                    break;
                };
                let cell_body = &after[..cend];
                // 跳过被 <text:p> 包裹的内容直接整体取文字
                cells.push(strip_tags(cell_body).trim().replace('\n', " "));
                cell_rest = &after[cend + "</table:table-cell>".len()..];
            }
            if cells.iter().any(|c| !c.is_empty()) {
                blocks.push(Block::Table(vec![cells]));
            }
            rest = &rest[end + "</table:table-row>".len()..];
            continue;
        }
        rest = &rest[1..];
    }

    // 表格行是逐行 push 的，合并成一张表
    let mut merged: Vec<Block> = Vec::new();
    let mut pending: Vec<Vec<String>> = Vec::new();
    for block in blocks {
        match block {
            Block::Table(rows) => pending.extend(rows),
            other => {
                if !pending.is_empty() {
                    merged.push(Block::Table(std::mem::take(&mut pending)));
                }
                merged.push(other);
            }
        }
    }
    if !pending.is_empty() {
        merged.push(Block::Table(pending));
    }

    if merged.is_empty() {
        return Err("ODF 里没有可提取的文字".to_string());
    }
    Ok(merged)
}

// ============== 统一出口 ==============

/// 按扩展名抽取结构块
pub fn blocks_for(src: &Path, ext: &str) -> Result<Vec<Block>, String> {
    match ext {
        "rtf" => rtf_blocks(src),
        "odt" | "ods" | "odp" | "fodt" | "fods" | "fodp" => odf_blocks(src),
        "epub" => epub_blocks(src),
        other => Err(format!(".{} 暂不支持", other)),
    }
}

fn blocks_to_plain(blocks: &[Block], markdown: bool) -> String {
    let mut out = String::new();
    for block in blocks {
        match block {
            Block::Heading(level, text) => {
                if markdown {
                    out.push_str(&format!("{} {}\n\n", "#".repeat(*level as usize), text));
                } else {
                    out.push_str(text);
                    out.push('\n');
                }
            }
            Block::Para(text) => {
                out.push_str(text);
                out.push('\n');
                if markdown {
                    out.push('\n');
                }
            }
            Block::Item(text) => {
                out.push_str(&format!("- {}\n", text));
            }
            Block::Table(rows) => {
                for row in rows {
                    out.push_str(&row.join(markdown.then_some(" | ").unwrap_or("\t")));
                    out.push('\n');
                }
                if markdown {
                    out.push('\n');
                }
            }
        }
    }
    out.trim_end().to_string()
}

fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn blocks_to_html(title: &str, blocks: &[Block]) -> String {
    let mut out = format!(
        "<!DOCTYPE html><html><head><meta charset=\"utf-8\"><title>{}</title></head><body>\n",
        escape_html(title)
    );
    for block in blocks {
        match block {
            Block::Heading(level, text) => out.push_str(&format!(
                "<h{l}>{}</h{l}>\n",
                escape_html(text),
                l = (*level as usize).clamp(1, 6)
            )),
            Block::Para(text) => out.push_str(&format!("<p>{}</p>\n", escape_html(text))),
            Block::Item(text) => out.push_str(&format!("<p>· {}</p>\n", escape_html(text))),
            Block::Table(rows) => {
                out.push_str("<table border=\"1\">\n");
                for row in rows {
                    out.push_str("<tr>");
                    for cell in row {
                        out.push_str(&format!("<td>{}</td>", escape_html(cell)));
                    }
                    out.push_str("</tr>\n");
                }
                out.push_str("</table>\n");
            }
        }
    }
    out.push_str("</body></html>\n");
    out
}

fn tables_of(blocks: &[Block]) -> Vec<Vec<String>> {
    let mut rows: Vec<Vec<String>> = Vec::new();
    for block in blocks {
        if let Block::Table(items) = block {
            rows.extend(items.iter().cloned());
        }
    }
    rows
}

/// 结构块 → 目标格式（txt / md / html / docx / csv / xlsx / pdf）
pub fn convert_blocks(
    src: &Path,
    blocks: &[Block],
    dst: &Path,
    target: &str,
    cancelled: &dyn Fn() -> bool,
) -> Result<(), String> {
    if cancelled() {
        return Err("已取消".to_string());
    }
    let title = src
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "文档".to_string());

    match target {
        "txt" => std::fs::write(dst, blocks_to_plain(blocks, false) + "\n")
            .map_err(|e| format!("写入失败: {}", e)),
        "md" => std::fs::write(dst, blocks_to_plain(blocks, true) + "\n")
            .map_err(|e| format!("写入失败: {}", e)),
        "html" => std::fs::write(dst, blocks_to_html(&title, blocks))
            .map_err(|e| format!("写入失败: {}", e)),
        "docx" => office_write::text_content_to_docx(
            &blocks_to_plain(blocks, true),
            true,
            dst,
            cancelled,
        ),
        "csv" => {
            let rows = tables_of(blocks);
            if rows.is_empty() {
                return Err("该文档里没有表格，无法转成 CSV".to_string());
            }
            let mut out = String::new();
            for row in rows {
                let cells: Vec<String> = row
                    .iter()
                    .map(|c| {
                        if c.contains(',') || c.contains('"') || c.contains('\n') {
                            format!("\"{}\"", c.replace('"', "\"\""))
                        } else {
                            c.clone()
                        }
                    })
                    .collect();
                out.push_str(&cells.join(","));
                out.push('\n');
            }
            std::fs::write(dst, out).map_err(|e| format!("写入失败: {}", e))
        }
        "xlsx" => {
            let rows = tables_of(blocks);
            if rows.is_empty() {
                return Err("该文档里没有表格，无法转成 XLSX".to_string());
            }
            office_write::rows_to_xlsx(&rows, &title, dst)
        }
        "pdf" => to_pdf::from_blocks(&title, blocks.to_vec(), dst, cancelled).map(|_| ()),
        other => Err(format!("→ {}：暂不支持", other)),
    }
}


pub fn epub_blocks(src: &Path) -> Result<Vec<Block>, String> {
    let entries = zip_min::read_zip(src).map_err(|e| format!("打开 EPUB 失败: {}", e))?;
    let mut html_entries: Vec<&zip_min::Entry> = entries
        .iter()
        .filter(|e| {
            let name = e.name.to_ascii_lowercase();
            name.ends_with(".xhtml") || name.ends_with(".html") || name.ends_with(".htm")
        })
        .collect();
    html_entries.sort_by(|a, b| a.name.cmp(&b.name));
    if html_entries.is_empty() {
        return Err("EPUB 里没有可读的正文文件".to_string());
    }

    let mut blocks: Vec<Block> = Vec::new();
    for entry in html_entries {
        let Ok(bytes) = entry.bytes() else { continue };
        let xml = String::from_utf8_lossy(&bytes).to_string();
        // 先去掉 head / style / script 段
        let mut body = xml.as_str();
        for tag in ["<head", "<style", "<script"] {
            if let Some(start) = body.find(tag) {
                if let Some(rel) = body[start..].find('>') {
                    let after = &body[start + rel + 1..];
                    let close = format!("</{}", &tag[1..]);
                    if let Some(end) = after.to_ascii_lowercase().find(&close) {
                        body = &after[end..];
                    }
                }
            }
        }
        let mut rest = body;
        loop {
            let mut best: Option<(usize, usize, u8, bool)> = None; // (pos, len, level, heading)
            for (open, close, heading) in [
                ("<h1", "</h1>", true),
                ("<h2", "</h2>", true),
                ("<h3", "</h3>", true),
                ("<p", "</p>", false),
                ("<li", "</li>", false),
            ]
            {
                if let Some(pos) = rest.to_ascii_lowercase().find(open) {
                    if best.map(|b| pos < b.0).unwrap_or(true) {
                        let level = match open {
                            "<h1" => 1,
                            "<h2" => 2,
                            _ => 3,
                        };
                        if let Some(end) = rest[pos..].to_ascii_lowercase().find(close) {
                            best = Some((pos, pos + end + close.len(), level, heading));
                        }
                    }
                }
            }
            let Some((start, end, level, heading)) = best else {
                break;
            };
            let text = strip_tags(&rest[start..end]).trim().to_string();
            if !text.is_empty() {
                if heading {
                    blocks.push(Block::Heading(level, text));
                } else if rest[start..].starts_with("<li") {
                    blocks.push(Block::Item(text));
                } else {
                    blocks.push(Block::Para(text));
                }
            }
            rest = &rest[end..];
        }
    }

    if blocks.is_empty() {
        return Err("EPUB 里没有可提取的文字".to_string());
    }
    Ok(blocks)
}
