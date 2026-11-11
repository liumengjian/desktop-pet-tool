// 文本类转换：docx / xlsx / pptx / html / md / txt / csv 之间的结构化互转。
// 只搬运结构（段落、标题、列表、表格、幻灯片顺序），不做版式重排；
// 需要排版引擎的 Office → PDF 在 layout_pdf.rs（后续批次），不在本文件。

use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::path::Path;

use super::zip_min::{read_zip, Entry};

fn decode_entities(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// 去掉所有 XML 标签，只留文本
fn strip_tags(xml: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for ch in xml.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    decode_entities(&out)
}

fn squeeze_blank_lines(s: &str) -> String {
    let mut out = String::new();
    let mut blanks = 0;
    for line in s.lines() {
        let trimmed = line.trim_end();
        if trimmed.trim().is_empty() {
            blanks += 1;
            if blanks > 1 {
                continue;
            }
            out.push('\n');
        } else {
            blanks = 0;
            out.push_str(trimmed);
            out.push('\n');
        }
    }
    out.trim().to_string()
}

fn write_text(dst: &Path, text: &str, with_bom: bool) -> Result<(), String> {
    let mut out = File::create(dst).map_err(|e| format!("写入失败: {}", e))?;
    if with_bom {
        out.write_all(b"\xEF\xBB\xBF")
            .map_err(|e| format!("写入失败: {}", e))?;
    }
    out.write_all(text.as_bytes())
        .map_err(|e| format!("写入失败: {}", e))
}

fn entry_text(entries: &[Entry], name: &str) -> Result<String, String> {
    let entry = entries
        .iter()
        .find(|e| e.name == name)
        .ok_or_else(|| format!("文件缺少 {}", name))?;
    Ok(String::from_utf8_lossy(&entry.bytes()?).to_string())
}

fn attr_value(attrs: &str, key: &str) -> Option<String> {
    let pos = attrs.find(&format!("{}=\"", key))?;
    let start = pos + key.len() + 2;
    let rest = &attrs[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// 收集 s 中所有 <open ...>...</close> 的内部内容
fn blocks<'a>(s: &'a str, open: &str, close: &str) -> Vec<&'a str> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    while let Some(i) = s[pos..].find(open) {
        let after_open = pos + i + open.len();
        let body_start = match s[after_open..].find('>') {
            Some(j) => after_open + j + 1,
            None => break,
        };
        match s[body_start..].find(close) {
            Some(j) => {
                out.push(&s[body_start..body_start + j]);
                pos = body_start + j + close.len();
            }
            None => break,
        }
    }
    out
}

/// 按 WordprocessingML 的 run 规则取文本：<w:t> 内文 + <w:tab/> + <w:br/>
fn runs_text(xml: &str) -> String {
    let mut out = String::new();
    let mut pos = 0usize;
    while let Some(i) = xml[pos..].find('<') {
        let tag_start = pos + i;
        let tag_end = match xml[tag_start..].find('>') {
            Some(j) => tag_start + j + 1,
            None => break,
        };
        let tag = &xml[tag_start + 1..tag_end - 1];
        let closing = tag.starts_with('/');
        let name = tag
            .trim_start_matches('/')
            .split(|c: char| c.is_whitespace() || c == '/' || c == '>')
            .next()
            .unwrap_or("");
        if name == "w:t" && !closing && !tag.ends_with('/') {
            match xml[tag_end..].find("</w:t>") {
                Some(k) => {
                    out.push_str(&decode_entities(&xml[tag_end..tag_end + k]));
                    pos = tag_end + k + 6;
                }
                None => break,
            }
        } else {
            if name == "w:tab" {
                out.push('\t');
            } else if name == "w:br" || name == "w:cr" {
                out.push('\n');
            }
            pos = tag_end;
        }
    }
    out
}

// ============== docx → html / md ==============

#[derive(Clone)]
pub enum Block {
    Heading(u8, String),
    Para(String),
    Item(String),
    Table(Vec<Vec<String>>),
}

fn marker_index(text: &str) -> Option<usize> {
    let rest = text.strip_prefix("@@TABLE:")?;
    rest.strip_suffix("@@")?.trim().parse::<usize>().ok()
}

fn heading_level(body: &str) -> u8 {
    let p = match body.find("<w:pStyle") {
        Some(p) => p,
        None => return 0,
    };
    let seg = &body[p..(p + 160).min(body.len())];
    if let Some(v) = attr_value(seg, "w:val") {
        if let Some(rest) = v.strip_prefix("Heading") {
            return rest.trim().parse::<u8>().unwrap_or(1).clamp(1, 6);
        }
        if v == "Title" {
            return 1;
        }
    }
    0
}

/// 把表格整段摘出来，原位留一个标记段落，保证段落与表格的相对顺序不丢
fn split_tables(xml: &str) -> (String, Vec<&str>) {
    let mut out = String::with_capacity(xml.len());
    let mut tables: Vec<&str> = Vec::new();
    let mut pos = 0usize;
    while let Some(i) = xml[pos..].find("<w:tbl") {
        let start = pos + i;
        let close = match xml[start..].find("</w:tbl>") {
            Some(j) => start + j + "</w:tbl>".len(),
            None => break,
        };
        out.push_str(&xml[pos..start]);
        out.push_str(&format!(
            "<w:p><w:r><w:t>@@TABLE:{}@@</w:t></w:r></w:p>",
            tables.len()
        ));
        tables.push(&xml[start..close]);
        pos = close;
    }
    out.push_str(&xml[pos..]);
    (out, tables)
}

fn table_rows(tbl: &str) -> Vec<Vec<String>> {
    blocks(tbl, "<w:tr", "</w:tr>")
        .into_iter()
        .map(|row| {
            blocks(row, "<w:tc", "</w:tc>")
                .into_iter()
                .map(|cell| runs_text(cell).replace(['\n', '\r'], " ").trim().to_string())
                .collect::<Vec<_>>()
        })
        .filter(|row: &Vec<String>| row.iter().any(|c| !c.is_empty()))
        .collect()
}

fn docx_blocks(xml: &str) -> Vec<Block> {
    let (clean, tables) = split_tables(xml);
    let mut out = Vec::new();
    for body in blocks(&clean, "<w:p", "</w:p>") {
        let text = runs_text(body);
        let trimmed = text.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(index) = marker_index(trimmed) {
            if let Some(tbl) = tables.get(index) {
                let rows = table_rows(tbl);
                if !rows.is_empty() {
                    out.push(Block::Table(rows));
                }
            }
            continue;
        }
        let level = heading_level(body);
        if level > 0 {
            out.push(Block::Heading(level, trimmed.to_string()));
        } else if body.contains("<w:numPr") {
            out.push(Block::Item(trimmed.to_string()));
        } else {
            out.push(Block::Para(trimmed.to_string()));
        }
    }
    out
}

fn html_shell(title: &str, body: &str) -> String {
    format!(
        "<!DOCTYPE html>\n<html lang=\"zh-CN\">\n<head>\n<meta charset=\"utf-8\">\n<title>{}</title>\n<style>\nbody{{font-family:-apple-system,'Segoe UI','Microsoft YaHei',sans-serif;line-height:1.7;max-width:900px;margin:32px auto;padding:0 20px;color:#22201c;}}\ntable{{border-collapse:collapse;margin:14px 0;width:100%;}}\ntd,th{{border:1px solid #d8d2c6;padding:6px 10px;}}\n</style>\n</head>\n<body>\n{}\n</body>\n</html>\n",
        escape_html(title),
        body
    )
}

fn blocks_to_html(blocks: &[Block]) -> String {
    let mut out = String::new();
    for block in blocks {
        match block {
            Block::Heading(level, text) => {
                out.push_str(&format!("<h{}>{}</h{}>\n", level, escape_html(text), level));
            }
            Block::Para(text) => {
                out.push_str(&format!("<p>{}</p>\n", escape_html(text)));
            }
            Block::Item(text) => {
                out.push_str(&format!("<li>{}</li>\n", escape_html(text)));
            }
            Block::Table(rows) => {
                out.push_str("<table>\n");
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
    out
}

fn blocks_to_md(blocks: &[Block]) -> String {
    let mut out = String::new();
    for block in blocks {
        match block {
            Block::Heading(level, text) => {
                out.push_str(&format!("{} {}\n\n", "#".repeat(*level as usize), text));
            }
            Block::Para(text) => {
                out.push_str(text);
                out.push_str("\n\n");
            }
            Block::Item(text) => {
                out.push_str(&format!("- {}\n", text));
            }
            Block::Table(rows) => {
                if let Some(first) = rows.first() {
                    out.push_str("| ");
                    out.push_str(&first.join(" | "));
                    out.push_str(" |\n|");
                    for _ in first {
                        out.push_str(" --- |");
                    }
                    out.push('\n');
                    for row in rows.iter().skip(1) {
                        out.push_str("| ");
                        out.push_str(&row.join(" | "));
                        out.push_str(" |\n");
                    }
                    out.push('\n');
                }
            }
        }
    }
    out
}

pub fn docx_to_txt(src: &Path, dst: &Path, cancelled: &dyn Fn() -> bool) -> Result<(), String> {
    let entries = read_zip(src)?;
    let xml = entry_text(&entries, "word/document.xml")?;
    if cancelled() {
        return Err("已取消".to_string());
    }

    let shaped = xml
        .replace("<w:br/>", "\n")
        .replace("<w:br />", "\n")
        .replace("<w:tab/>", "\t")
        .replace("<w:tab />", "\t")
        .replace("</w:p>", "\n");
    let text = squeeze_blank_lines(&strip_tags(&shaped));
    write_text(dst, &text, false)
}

pub fn docx_to_html(src: &Path, dst: &Path, cancelled: &dyn Fn() -> bool) -> Result<(), String> {
    let entries = read_zip(src)?;
    let xml = entry_text(&entries, "word/document.xml")?;
    if cancelled() {
        return Err("已取消".to_string());
    }
    let blocks = docx_blocks(&xml);
    let title = src
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    write_text(dst, &html_shell(&title, &blocks_to_html(&blocks)), false)
}

pub fn docx_to_md(src: &Path, dst: &Path, cancelled: &dyn Fn() -> bool) -> Result<(), String> {
    let entries = read_zip(src)?;
    let xml = entry_text(&entries, "word/document.xml")?;
    if cancelled() {
        return Err("已取消".to_string());
    }
    let blocks = docx_blocks(&xml);
    write_text(dst, blocks_to_md(&blocks).trim_end(), false)
}

// ============== xlsx → csv / html / txt ==============

fn col_index(cell_ref: &str) -> Option<usize> {
    let letters: String = cell_ref
        .chars()
        .take_while(|c| c.is_ascii_alphabetic())
        .collect();
    if letters.is_empty() {
        return None;
    }
    let mut idx = 0usize;
    for c in letters.chars() {
        idx = idx * 26 + (c.to_ascii_uppercase() as usize - 'A' as usize + 1);
    }
    Some(idx - 1)
}

// 旧版单元格引用解析：xlsx 读取已改用逐行遍历，保留供后续按坐标取值时使用
#[allow(dead_code)]
fn row_index(cell_ref: &str) -> Option<usize> {
    let digits: String = cell_ref.chars().skip_while(|c| c.is_ascii_alphabetic()).collect();
    digits.parse::<usize>().ok()
}

fn csv_escape(value: &str) -> String {
    if value.contains(',') || value.contains('"') || value.contains('\n') || value.contains('\r') {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

fn shared_strings(entries: &[Entry]) -> Result<Vec<String>, String> {
    match entries.iter().find(|e| e.name == "xl/sharedStrings.xml") {
        Some(entry) => {
            let xml = String::from_utf8_lossy(&entry.bytes()?).to_string();
            Ok(blocks(&xml, "<si", "</si>")
                .into_iter()
                .map(|b| strip_tags(b).trim().to_string())
                .collect())
        }
        None => Ok(Vec::new()),
    }
}

/// 一张工作表 → 行 × (列号, 文本)
fn sheet_rows(sheet_xml: &str, shared: &[String]) -> Vec<Vec<(usize, String)>> {
    let mut rows: Vec<Vec<(usize, String)>> = Vec::new();
    for row in blocks(sheet_xml, "<row", "</row>") {
        let mut cells: Vec<(usize, String)> = Vec::new();
        for chunk in row.split("<c").skip(1) {
            let (attrs, body) = match chunk.find('>') {
                Some(i) => (&chunk[..i], &chunk[i + 1..]),
                None => continue,
            };
            let reference = match attr_value(attrs, "r") {
                Some(r) => r,
                None => continue,
            };
            let col = match col_index(&reference) {
                Some(c) => c,
                None => continue,
            };
            let kind = attr_value(attrs, "t").unwrap_or_default();
            let value = if kind == "s" {
                blocks(body, "<v", "</v>")
                    .first()
                    .and_then(|v| v.trim().parse::<usize>().ok())
                    .and_then(|i| shared.get(i).cloned())
                    .unwrap_or_default()
            } else if kind == "inlineStr" {
                blocks(body, "<t", "</t>")
                    .first()
                    .map(|t| decode_entities(t).trim().to_string())
                    .unwrap_or_default()
            } else {
                blocks(body, "<v", "</v>")
                    .first()
                    .map(|v| decode_entities(v).trim().to_string())
                    .unwrap_or_default()
            };
            if !value.is_empty() {
                cells.push((col, value));
            }
        }
        if !cells.is_empty() {
            rows.push(cells);
        }
    }
    rows
}

fn padded_rows(rows: &[Vec<(usize, String)>]) -> Vec<Vec<String>> {
    let width = rows
        .iter()
        .map(|r| r.iter().map(|(c, _)| c + 1).max().unwrap_or(0))
        .max()
        .unwrap_or(0);
    rows.iter()
        .map(|row| {
            let mut line = vec![String::new(); width];
            for (col, value) in row {
                if *col < width {
                    line[*col] = value.clone();
                }
            }
            line
        })
        .collect()
}

fn first_sheet(entries: &[Entry]) -> Result<String, String> {
    let sheet = entries
        .iter()
        .find(|e| e.name.starts_with("xl/worksheets/") && e.name.ends_with(".xml"))
        .ok_or_else(|| "不是有效的 xlsx（找不到工作表）".to_string())?;
    Ok(String::from_utf8_lossy(&sheet.bytes()?).to_string())
}

pub fn xlsx_to_csv(src: &Path, dst: &Path, cancelled: &dyn Fn() -> bool) -> Result<(), String> {
    let entries = read_zip(src)?;
    let shared = shared_strings(&entries)?;
    if cancelled() {
        return Err("已取消".to_string());
    }
    let sheet_xml = first_sheet(&entries)?;
    let grid = padded_rows(&sheet_rows(&sheet_xml, &shared));
    if cancelled() {
        return Err("已取消".to_string());
    }

    let mut content = String::new();
    for line in &grid {
        content.push_str(&line.iter().map(|v| csv_escape(v)).collect::<Vec<_>>().join(","));
        content.push_str("\r\n");
    }
    // UTF-8 BOM：让 Excel 双击打开时正确识别中文
    write_text(dst, &content, true)
}

pub fn xlsx_to_txt(src: &Path, dst: &Path, cancelled: &dyn Fn() -> bool) -> Result<(), String> {
    let entries = read_zip(src)?;
    let shared = shared_strings(&entries)?;
    let sheet_xml = first_sheet(&entries)?;
    let grid = padded_rows(&sheet_rows(&sheet_xml, &shared));
    if cancelled() {
        return Err("已取消".to_string());
    }
    let mut content = String::new();
    for line in &grid {
        content.push_str(&line.join("\t"));
        content.push('\n');
    }
    write_text(dst, &content, false)
}

pub fn xlsx_to_html(src: &Path, dst: &Path, cancelled: &dyn Fn() -> bool) -> Result<(), String> {
    let entries = read_zip(src)?;
    let shared = shared_strings(&entries)?;
    let sheet_xml = first_sheet(&entries)?;
    let grid = padded_rows(&sheet_rows(&sheet_xml, &shared));
    if cancelled() {
        return Err("已取消".to_string());
    }

    let mut body = String::from("<table>\n");
    for (index, line) in grid.iter().enumerate() {
        let tag = if index == 0 { "th" } else { "td" };
        body.push_str("<tr>");
        for cell in line {
            body.push_str(&format!("<{0}>{1}</{0}>", tag, escape_html(cell)));
        }
        body.push_str("</tr>\n");
    }
    body.push_str("</table>\n");

    let title = src
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    write_text(dst, &html_shell(&title, &body), false)
}

// ============== pptx → txt / html / md ==============

fn slide_paragraphs(xml: &str) -> Vec<String> {
    let mut out = Vec::new();
    for paragraph in xml.split("</a:p>") {
        let text: String = blocks(paragraph, "<a:t", "</a:t>")
            .into_iter()
            .map(decode_entities)
            .collect();
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            out.push(trimmed.to_string());
        }
    }
    out
}

fn sorted_slides(entries: &[Entry]) -> Vec<(u32, String)> {
    let mut slides: Vec<(u32, &Entry)> = entries
        .iter()
        .filter(|e| e.name.starts_with("ppt/slides/slide") && e.name.ends_with(".xml"))
        .map(|e| {
            let num: u32 = e
                .name
                .trim_start_matches("ppt/slides/slide")
                .trim_end_matches(".xml")
                .parse()
                .unwrap_or(0);
            (num, e)
        })
        .collect();
    slides.sort_by_key(|(n, _)| *n);
    slides
        .into_iter()
        .filter_map(|(num, entry)| entry.bytes().ok().map(|b| (num, String::from_utf8_lossy(&b).to_string())))
        .collect()
}

pub fn pptx_to_txt(src: &Path, dst: &Path, cancelled: &dyn Fn() -> bool) -> Result<(), String> {
    let entries = read_zip(src)?;
    let slides = sorted_slides(&entries);
    if slides.is_empty() {
        return Err("不是有效的 pptx（找不到幻灯片）".to_string());
    }

    let mut content = String::new();
    for (number, xml) in &slides {
        if cancelled() {
            return Err("已取消".to_string());
        }
        content.push_str(&format!("第 {} 页\n", number));
        for paragraph in slide_paragraphs(xml) {
            content.push_str(&paragraph);
            content.push('\n');
        }
        content.push('\n');
    }
    write_text(dst, &content, false)
}

pub fn pptx_to_md(src: &Path, dst: &Path, cancelled: &dyn Fn() -> bool) -> Result<(), String> {
    let entries = read_zip(src)?;
    let slides = sorted_slides(&entries);
    if slides.is_empty() {
        return Err("不是有效的 pptx（找不到幻灯片）".to_string());
    }
    let mut content = String::new();
    for (number, xml) in &slides {
        if cancelled() {
            return Err("已取消".to_string());
        }
        content.push_str(&format!("## 第 {} 页\n\n", number));
        for paragraph in slide_paragraphs(xml) {
            content.push_str(&paragraph);
            content.push_str("\n\n");
        }
    }
    write_text(dst, content.trim_end(), false)
}

pub fn pptx_to_html(src: &Path, dst: &Path, cancelled: &dyn Fn() -> bool) -> Result<(), String> {
    let entries = read_zip(src)?;
    let slides = sorted_slides(&entries);
    if slides.is_empty() {
        return Err("不是有效的 pptx（找不到幻灯片）".to_string());
    }
    let mut body = String::new();
    for (number, xml) in &slides {
        if cancelled() {
            return Err("已取消".to_string());
        }
        body.push_str(&format!("<h2>第 {} 页</h2>\n", number));
        for paragraph in slide_paragraphs(xml) {
            body.push_str(&format!("<p>{}</p>\n", escape_html(&paragraph)));
        }
    }
    let title = src
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    write_text(dst, &html_shell(&title, &body), false)
}

// ============== html → txt / md ==============

/// 轻量 HTML 扫描器：识别标题、列表、段落、换行、强调、链接，其余标签丢弃。
fn html_to_markup(html: &str, markdown: bool) -> String {
    let mut out = String::new();
    let mut pos = 0usize;
    let mut skip_depth = 0i32;
    let lower = html.to_ascii_lowercase();

    while let Some(i) = html[pos..].find('<') {
        let text = &html[pos..pos + i];
        if skip_depth == 0 && !text.trim().is_empty() {
            out.push_str(&decode_entities(text));
        }
        let tag_start = pos + i;
        let tag_end = match html[tag_start..].find('>') {
            Some(j) => tag_start + j + 1,
            None => break,
        };
        let raw = &lower[tag_start + 1..tag_end - 1];
        let closing = raw.starts_with('/');
        let name: &str = raw
            .trim_start_matches('/')
            .split(|c: char| c.is_whitespace() || c == '/' || c == '>')
            .next()
            .unwrap_or("");

        if name == "script" || name == "style" {
            if closing {
                skip_depth = (skip_depth - 1).max(0);
            } else {
                skip_depth += 1;
            }
        } else if skip_depth == 0 {
            match name {
                "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                    let level = name.as_bytes()[1] - b'0';
                    if closing {
                        out.push_str("\n\n");
                    } else {
                        out.push_str("\n\n");
                        if markdown {
                            out.push_str(&"#".repeat(level as usize));
                            out.push(' ');
                        }
                    }
                }
                "li" => {
                    out.push('\n');
                    if markdown && !closing {
                        out.push_str("- ");
                    }
                }
                "p" | "div" | "section" | "article" | "tr" | "table" | "ul" | "ol" | "blockquote" => {
                    if closing {
                        out.push_str("\n\n");
                    }
                }
                "br" => out.push('\n'),
                "td" | "th" => {
                    out.push('\t');
                }
                "strong" | "b" if markdown => out.push_str("**"),
                "em" | "i" if markdown => out.push('*'),
                "code" if markdown => out.push('`'),
                _ => {}
            }
        }
        pos = tag_end;
    }
    if pos < html.len() {
        let tail = &html[pos..];
        if skip_depth == 0 {
            out.push_str(&decode_entities(tail));
        }
    }

    let squeezed = squeeze_blank_lines(&out.replace("\n\n\n", "\n\n"));
    if markdown {
        squeezed
    } else {
        squeezed.replace("\n\n", "\n")
    }
}

pub fn html_to_txt(src: &Path, dst: &Path, cancelled: &dyn Fn() -> bool) -> Result<(), String> {
    let html = std::fs::read_to_string(src).map_err(|e| format!("读取失败: {}", e))?;
    if cancelled() {
        return Err("已取消".to_string());
    }
    write_text(dst, &html_to_markup(&html, false), false)
}

pub fn html_to_md(src: &Path, dst: &Path, cancelled: &dyn Fn() -> bool) -> Result<(), String> {
    let html = std::fs::read_to_string(src).map_err(|e| format!("读取失败: {}", e))?;
    if cancelled() {
        return Err("已取消".to_string());
    }
    write_text(dst, &html_to_markup(&html, true), false)
}

// ============== md / txt → html ==============

fn inline_md(text: &str) -> String {
    let escaped = escape_html(text);
    // **粗体** → <strong>，`代码` → <code>，[文字](链接) → <a>
    let mut out = String::new();
    let mut rest = escaped.as_str();
    while !rest.is_empty() {
        if let Some(inner) = rest.strip_prefix("**") {
            if let Some(end) = inner.find("**") {
                out.push_str(&format!("<strong>{}</strong>", &inner[..end]));
                rest = &inner[end + 2..];
                continue;
            }
        }
        if let Some(inner) = rest.strip_prefix('`') {
            if let Some(end) = inner.find('`') {
                out.push_str(&format!("<code>{}</code>", &inner[..end]));
                rest = &inner[end + 1..];
                continue;
            }
        }
        if rest.starts_with('[') {
            if let Some(close) = rest.find("](") {
                if let Some(end) = rest[close + 2..].find(')') {
                    let label = &rest[1..close];
                    let href = &rest[close + 2..close + 2 + end];
                    out.push_str(&format!("<a href=\"{}\">{}</a>", href, label));
                    rest = &rest[close + 2 + end + 1..];
                    continue;
                }
            }
        }
        let ch = rest.chars().next().unwrap();
        out.push(ch);
        rest = &rest[ch.len_utf8()..];
    }
    out
}

pub fn md_to_html(src: &Path, dst: &Path, cancelled: &dyn Fn() -> bool) -> Result<(), String> {
    let md = std::fs::read_to_string(src).map_err(|e| format!("读取失败: {}", e))?;
    if cancelled() {
        return Err("已取消".to_string());
    }

    let mut body = String::new();
    let mut in_code = false;
    let mut in_ul = false;
    for line in md.lines() {
        let trimmed = line.trim_end();
        if trimmed.trim_start().starts_with("```") {
            if in_code {
                body.push_str("</pre>\n");
            } else {
                body.push_str("<pre>\n");
            }
            in_code = !in_code;
            continue;
        }
        if in_code {
            body.push_str(&escape_html(trimmed));
            body.push('\n');
            continue;
        }
        if trimmed.trim().is_empty() {
            if in_ul {
                body.push_str("</ul>\n");
                in_ul = false;
            }
            continue;
        }
        let hashes = trimmed.chars().take_while(|c| *c == '#').count();
        if hashes > 0 && hashes <= 6 && trimmed.chars().nth(hashes) == Some(' ') {
            if in_ul {
                body.push_str("</ul>\n");
                in_ul = false;
            }
            let text = trimmed[hashes..].trim();
            body.push_str(&format!(
                "<h{n}>{text}</h{n}>\n",
                text = inline_md(text),
                n = hashes
            ));
            continue;
        }
        if let Some(item) = trimmed.trim_start().strip_prefix("- ").or_else(|| {
            trimmed
                .trim_start()
                .strip_prefix("* ")
        }) {
            if !in_ul {
                body.push_str("<ul>\n");
                in_ul = true;
            }
            body.push_str(&format!("<li>{}</li>\n", inline_md(item)));
            continue;
        }
        if in_ul {
            body.push_str("</ul>\n");
            in_ul = false;
        }
        body.push_str(&format!("<p>{}</p>\n", inline_md(trimmed)));
    }
    if in_ul {
        body.push_str("</ul>\n");
    }
    if in_code {
        body.push_str("</pre>\n");
    }

    let title = src
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    write_text(dst, &html_shell(&title, &body), false)
}

pub fn txt_to_html(src: &Path, dst: &Path, cancelled: &dyn Fn() -> bool) -> Result<(), String> {
    let text = std::fs::read_to_string(src).map_err(|e| format!("读取失败: {}", e))?;
    if cancelled() {
        return Err("已取消".to_string());
    }
    let mut body = String::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        body.push_str(&format!("<p>{}</p>\n", escape_html(line)));
    }
    let title = src
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    write_text(dst, &html_shell(&title, &body), false)
}

/// 供 md → txt / md → docx 复用的纯文本化
pub fn md_to_plain(md: &str) -> String {
    let mut out = String::new();
    let mut in_code = false;
    for line in md.lines() {
        let trimmed = line.trim_end();
        if trimmed.trim_start().starts_with("```") {
            in_code = !in_code;
            continue;
        }
        if in_code {
            out.push_str(trimmed);
            out.push('\n');
            continue;
        }
        let hashes = trimmed.chars().take_while(|c| *c == '#').count();
        if hashes > 0 && hashes <= 6 && trimmed.chars().nth(hashes) == Some(' ') {
            out.push_str(trimmed[hashes..].trim());
            out.push('\n');
            continue;
        }
        if let Some(item) = trimmed.trim_start().strip_prefix("- ") {
            out.push_str(&format!("· {}\n", item));
            continue;
        }
        out.push_str(trimmed);
        out.push('\n');
    }
    squeeze_blank_lines(&out)
}

// ============== 结构化抽取（供 PDF 排版复用） ==============

/// 去掉行内 Markdown 标记，只留可显示的纯文本
pub fn inline_plain(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '*' | '_' | '`' => {}
            '[' => {
                // [文字](链接) → 文字
                let mut label = String::new();
                let mut closed = false;
                for c in chars.by_ref() {
                    if c == ']' {
                        closed = true;
                        break;
                    }
                    label.push(c);
                }
                if closed && chars.peek() == Some(&'(') {
                    let mut depth = 0;
                    for c in chars.by_ref() {
                        if c == '(' {
                            depth += 1;
                        } else if c == ')' {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                    }
                    out.push_str(&label);
                } else {
                    out.push('[');
                    out.push_str(&label);
                    if closed {
                        out.push(']');
                    }
                }
            }
            '\\' => {
                if let Some(next) = chars.next() {
                    out.push(next);
                }
            }
            _ => out.push(ch),
        }
    }
    out
}

fn table_separator(line: &str) -> bool {
    let cells: Vec<&str> = line
        .trim()
        .trim_matches('|')
        .split('|')
        .map(|c| c.trim())
        .collect();
    !cells.is_empty()
        && cells.iter().all(|c| {
            !c.is_empty() && c.chars().all(|ch| ch == '-' || ch == ':')
        })
}

fn table_row(line: &str) -> Vec<String> {
    line.trim()
        .trim_matches('|')
        .split('|')
        .map(|c| inline_plain(c.trim()))
        .collect()
}

/// Markdown 文本 → 结构化块（标题 / 列表 / 段落 / 管道表格 / 代码块）
pub fn md_text_blocks(md: &str) -> Vec<Block> {
    let lines: Vec<&str> = md.lines().collect();
    let mut out: Vec<Block> = Vec::new();
    let mut i = 0usize;
    let mut para: Vec<String> = Vec::new();

    let flush_para = |para: &mut Vec<String>, out: &mut Vec<Block>| {
        if !para.is_empty() {
            out.push(Block::Para(para.join(" ")));
            para.clear();
        }
    };

    while i < lines.len() {
        let raw = lines[i];
        let line = raw.trim_end();
        let trimmed = line.trim();

        if trimmed.is_empty() {
            flush_para(&mut para, &mut out);
            i += 1;
            continue;
        }
        if trimmed.starts_with("```") {
            flush_para(&mut para, &mut out);
            i += 1;
            while i < lines.len() && !lines[i].trim_start().starts_with("```") {
                if !lines[i].trim().is_empty() {
                    out.push(Block::Para(lines[i].trim_end().to_string()));
                }
                i += 1;
            }
            i += 1;
            continue;
        }
        if trimmed.chars().all(|c| c == '-' || c == '*' || c == '_') && trimmed.len() >= 3 {
            flush_para(&mut para, &mut out);
            i += 1;
            continue;
        }
        let hashes = trimmed.chars().take_while(|c| *c == '#').count();
        if hashes > 0 && hashes <= 6 && trimmed.chars().nth(hashes) == Some(' ') {
            flush_para(&mut para, &mut out);
            out.push(Block::Heading(
                hashes as u8,
                inline_plain(trimmed[hashes..].trim()),
            ));
            i += 1;
            continue;
        }
        let bullet = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
            .or_else(|| trimmed.strip_prefix("+ "));
        if let Some(item) = bullet {
            flush_para(&mut para, &mut out);
            out.push(Block::Item(inline_plain(item.trim())));
            i += 1;
            continue;
        }
        // 有序列表 1. / 1)
        let ordered = trimmed
            .split_once('.')
            .filter(|(n, rest)| {
                !n.is_empty()
                    && n.chars().all(|c| c.is_ascii_digit())
                    && rest.starts_with(' ')
            })
            .map(|(_, rest)| rest.trim());
        if let Some(item) = ordered {
            flush_para(&mut para, &mut out);
            out.push(Block::Item(inline_plain(item)));
            i += 1;
            continue;
        }
        if trimmed.starts_with('|') && i + 1 < lines.len() && table_separator(lines[i + 1]) {
            flush_para(&mut para, &mut out);
            let mut rows: Vec<Vec<String>> = vec![table_row(trimmed)];
            i += 2;
            while i < lines.len() && lines[i].trim().starts_with('|') {
                rows.push(table_row(lines[i]));
                i += 1;
            }
            out.push(Block::Table(rows));
            continue;
        }
        let quote = trimmed.strip_prefix("> ").unwrap_or(trimmed);
        para.push(inline_plain(quote));
        i += 1;
    }
    flush_para(&mut para, &mut out);
    out
}

// 段落级抽取：排版走 docx_flow（带内嵌图），这里保留纯文本版本
#[allow(dead_code)]
pub fn docx_items(src: &Path) -> Result<Vec<Block>, String> {
    let entries = read_zip(src)?;
    let xml = entry_text(&entries, "word/document.xml")?;
    Ok(docx_blocks(&xml))
}

pub fn xlsx_grid(src: &Path) -> Result<Vec<Vec<String>>, String> {
    let entries = read_zip(src)?;
    let shared = shared_strings(&entries)?;
    let sheet = first_sheet(&entries)?;
    Ok(padded_rows(&sheet_rows(&sheet, &shared)))
}

// 幻灯片段落抽取：排版走 pptx_flow，这里保留不分块版本
#[allow(dead_code)]
pub fn pptx_sections(src: &Path) -> Result<Vec<(usize, Vec<String>)>, String> {
    let entries = read_zip(src)?;
    let slides = sorted_slides(&entries);
    if slides.is_empty() {
        return Err("不是有效的 pptx（找不到幻灯片）".to_string());
    }
    Ok(slides
        .iter()
        .map(|(number, xml)| (*number as usize, slide_paragraphs(xml)))
        .collect())
}

pub fn csv_grid(src: &Path) -> Result<Vec<Vec<String>>, String> {
    let raw = std::fs::read(src).map_err(|e| format!("读取失败: {}", e))?;
    let content = if raw.starts_with(&[0xEF, 0xBB, 0xBF]) {
        String::from_utf8_lossy(&raw[3..]).to_string()
    } else {
        String::from_utf8_lossy(&raw).to_string()
    };
    Ok(super::office_write::parse_csv(&content))
}

/// 纯文本 → 段落块：连续非空行合成一段，空行分段
pub fn txt_items(src: &Path) -> Result<Vec<Block>, String> {
    let raw = std::fs::read(src).map_err(|e| format!("读取失败: {}", e))?;
    let content = if raw.starts_with(&[0xEF, 0xBB, 0xBF]) {
        String::from_utf8_lossy(&raw[3..]).to_string()
    } else {
        String::from_utf8_lossy(&raw).to_string()
    };
    let mut out = Vec::new();
    let mut para: Vec<String> = Vec::new();
    for line in content.lines() {
        if line.trim().is_empty() {
            if !para.is_empty() {
                out.push(Block::Para(para.join(" ")));
                para.clear();
            }
        } else {
            para.push(line.trim_end().to_string());
        }
    }
    if !para.is_empty() {
        out.push(Block::Para(para.join(" ")));
    }
    Ok(out)
}

pub fn md_items(src: &Path) -> Result<Vec<Block>, String> {
    let md = std::fs::read_to_string(src).map_err(|e| format!("读取失败: {}", e))?;
    Ok(md_text_blocks(&md))
}

pub fn html_items(src: &Path) -> Result<Vec<Block>, String> {
    let html = std::fs::read_to_string(src).map_err(|e| format!("读取失败: {}", e))?;
    Ok(md_text_blocks(&html_to_markup(&html, true)))
}

// ============== 图片感知抽取（docx / pptx → PDF） ==============
// 段落文本之外，把内嵌图片按文档顺序一起带出来，供排版器插到正文对应位置。
// 图片尺寸取 OOXML 里的 EMU 显示尺寸（12700 EMU = 1pt），没有则给默认宽高。

pub enum FlowBlock {
    Text(Block),
    Image {
        bytes: Vec<u8>,
        w_pt: f32,
        h_pt: f32,
    },
}

const DEFAULT_IMG_W: f32 = 320.0;
const DEFAULT_IMG_H: f32 = 220.0;
const EMU_PER_PT: f32 = 12700.0;

fn entry_bytes(entries: &[Entry], name: &str) -> Option<Vec<u8>> {
    entries
        .iter()
        .find(|e| e.name == name)
        .and_then(|e| e.bytes().ok())
}

/// 关系表：rId → Target（相对该 part 所在目录）
fn rel_map(entries: &[Entry], rels_name: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let xml = match entry_text(entries, rels_name) {
        Ok(x) => x,
        Err(_) => return map,
    };
    for seg in xml.split("<Relationship").skip(1) {
        let head = match seg.find("/>").or_else(|| seg.find('>')) {
            Some(i) => &seg[..i],
            None => continue,
        };
        if let (Some(id), Some(target)) = (attr_value(head, "Id"), attr_value(head, "Target")) {
            map.insert(id, target);
        }
    }
    map
}

/// 把 part 内的相对 Target 归一化成 zip 内的完整路径（处理 ../ 与开头的 /）
fn normalize_path(base: &str, target: &str) -> String {
    let mut parts: Vec<&str> = base.split('/').filter(|s| !s.is_empty()).collect();
    for seg in target.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    parts.join("/")
}

/// 收集标签属性片段：返回所有出现 <tag 之后的属性串
fn tag_segments<'a>(xml: &'a str, tag: &str) -> Vec<&'a str> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    while let Some(i) = xml[pos..].find(tag) {
        let start = pos + i;
        let head = match xml[start..].find('>') {
            Some(j) => &xml[start..start + j],
            None => break,
        };
        out.push(head);
        pos = start + head.len();
    }
    out
}

fn emu_pt(value: Option<String>) -> f32 {
    value
        .and_then(|v| v.trim().parse::<f32>().ok())
        .map(|v| v / EMU_PER_PT)
        .unwrap_or(0.0)
}

fn extents(xml: &str, tag: &str) -> Vec<(f32, f32)> {
    tag_segments(xml, tag)
        .into_iter()
        .map(|seg| (emu_pt(attr_value(seg, "cx")), emu_pt(attr_value(seg, "cy"))))
        .collect()
}

fn blips(xml: &str) -> Vec<String> {
    tag_segments(xml, "<a:blip")
        .into_iter()
        .filter_map(|seg| attr_value(seg, "r:embed"))
        .collect()
}

pub fn docx_flow(src: &Path) -> Result<Vec<FlowBlock>, String> {
    let entries = read_zip(src)?;
    let xml = entry_text(&entries, "word/document.xml")?;
    let rels = rel_map(&entries, "word/_rels/document.xml.rels");
    let (clean, tables) = split_tables(&xml);
    let mut out: Vec<FlowBlock> = Vec::new();

    for body in blocks(&clean, "<w:p", "</w:p>") {
        let text = runs_text(body);
        let trimmed = text.trim();
        if let Some(index) = marker_index(trimmed) {
            if let Some(tbl) = tables.get(index) {
                let rows = table_rows(tbl);
                if !rows.is_empty() {
                    out.push(FlowBlock::Text(Block::Table(rows)));
                }
            }
            continue;
        }
        if !trimmed.is_empty() {
            let level = heading_level(body);
            let block = if level > 0 {
                Block::Heading(level, trimmed.to_string())
            } else if body.contains("<w:numPr") {
                Block::Item(trimmed.to_string())
            } else {
                Block::Para(trimmed.to_string())
            };
            out.push(FlowBlock::Text(block));
        }

        // 同一段里可能有多张图，按出现顺序取尺寸与关系
        let sizes = extents(body, "<wp:extent");
        for (index, rid) in blips(body).iter().enumerate() {
            let target = match rels.get(rid) {
                Some(t) => t,
                None => continue,
            };
            let name = normalize_path("word", target.trim_start_matches('/'));
            let bytes = match entry_bytes(&entries, &name) {
                Some(b) => b,
                None => continue,
            };
            let (w, h) = sizes.get(index).copied().unwrap_or((0.0, 0.0));
            out.push(FlowBlock::Image {
                bytes,
                w_pt: if w > 1.0 { w } else { DEFAULT_IMG_W },
                h_pt: if h > 1.0 { h } else { DEFAULT_IMG_H },
            });
        }
    }
    Ok(out)
}

pub fn pptx_flow(src: &Path) -> Result<Vec<FlowBlock>, String> {
    let entries = read_zip(src)?;
    let mut slides: Vec<(u32, String)> = entries
        .iter()
        .filter(|e| e.name.starts_with("ppt/slides/slide") && e.name.ends_with(".xml"))
        .map(|e| {
            let num: u32 = e
                .name
                .trim_start_matches("ppt/slides/slide")
                .trim_end_matches(".xml")
                .parse()
                .unwrap_or(0);
            (num, e.name.clone())
        })
        .collect();
    slides.sort_by_key(|(n, _)| *n);
    if slides.is_empty() {
        return Err("不是有效的 pptx（找不到幻灯片）".to_string());
    }

    let mut out: Vec<FlowBlock> = Vec::new();
    for (number, name) in slides {
        let xml = entry_text(&entries, &name)?;
        out.push(FlowBlock::Text(Block::Heading(2, format!("第 {} 页", number))));
        for paragraph in slide_paragraphs(&xml) {
            out.push(FlowBlock::Text(Block::Para(paragraph)));
        }
        let rels_name = name.replace("ppt/slides/", "ppt/slides/_rels/") + ".rels";
        let rels = rel_map(&entries, &rels_name);
        let sizes = extents(&xml, "<a:ext");
        for (index, rid) in blips(&xml).iter().enumerate() {
            let target = match rels.get(rid) {
                Some(t) => t,
                None => continue,
            };
            let path = normalize_path("ppt/slides", target);
            let bytes = match entry_bytes(&entries, &path) {
                Some(b) => b,
                None => continue,
            };
            let (w, h) = sizes.get(index).copied().unwrap_or((0.0, 0.0));
            out.push(FlowBlock::Image {
                bytes,
                w_pt: if w > 1.0 { w } else { DEFAULT_IMG_W },
                h_pt: if h > 1.0 { h } else { DEFAULT_IMG_H },
            });
        }
    }
    Ok(out)
}
