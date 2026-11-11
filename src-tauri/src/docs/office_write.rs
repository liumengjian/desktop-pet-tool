// 反向写出：纯文本 / Markdown → DOCX，CSV → XLSX。
// 用 zip_min 直接拼最小 OOXML 包，不依赖任何 Office 组件。

use std::path::Path;

use super::zip_min::{write_zip, OutEntry};

fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn column_name(mut index: usize) -> String {
    // 0 → A, 25 → Z, 26 → AA
    let mut out = String::new();
    loop {
        out.push((b'A' + (index % 26) as u8) as char);
        if index < 26 {
            break;
        }
        index = index / 26 - 1;
    }
    out.chars().rev().collect()
}

// ============== 纯文本 / Markdown → DOCX ==============

fn docx_paragraph(text: &str, level: u8) -> String {
    let trimmed = text.trim_end();
    let run_props = if level > 0 {
        format!(
            "<w:rPr><w:b/><w:sz w:val=\"{}\"/></w:rPr>",
            match level {
                1 => 40,
                2 => 34,
                _ => 28,
            }
        )
    } else {
        String::new()
    };
    format!(
        "<w:p><w:r>{}<w:t xml:space=\"preserve\">{}</w:t></w:r></w:p>",
        run_props,
        escape_xml(trimmed)
    )
}

/// markdown = true 时识别 # 标题、- 列表；否则按纯文本逐行
pub fn text_to_docx(
    src: &Path,
    dst: &Path,
    markdown: bool,
    cancelled: &dyn Fn() -> bool,
) -> Result<(), String> {
    let text = std::fs::read_to_string(src).map_err(|e| format!("读取失败: {}", e))?;
    text_content_to_docx(&text, markdown, dst, cancelled)
}

/// 已有文本内容（不是文件路径）→ DOCX
pub fn text_content_to_docx(
    text: &str,
    markdown: bool,
    dst: &Path,
    cancelled: &dyn Fn() -> bool,
) -> Result<(), String> {
    if cancelled() {
        return Err("已取消".to_string());
    }

    let mut body = String::new();
    let mut in_code = false;
    for line in text.lines() {
        if cancelled() {
            return Err("已取消".to_string());
        }
        if markdown && line.trim_start().starts_with("```") {
            in_code = !in_code;
            continue;
        }
        if line.trim().is_empty() {
            body.push_str("<w:p/>");
            continue;
        }
        if markdown && !in_code {
            let hashes = line.chars().take_while(|c| *c == '#').count();
            if hashes > 0 && hashes <= 6 && line.chars().nth(hashes) == Some(' ') {
                body.push_str(&docx_paragraph(line[hashes..].trim(), hashes as u8));
                continue;
            }
            if let Some(item) = line.trim_start().strip_prefix("- ") {
                body.push_str(&docx_paragraph(&format!("· {}", item), 0));
                continue;
            }
        }
        body.push_str(&docx_paragraph(line, 0));
    }

    let document = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
         <w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"><w:body>{}\
         <w:sectPr><w:pgSz w:w=\"11906\" w:h=\"16838\"/>\
         <w:pgMar w:top=\"1440\" w:right=\"1440\" w:bottom=\"1440\" w:left=\"1440\"/></w:sectPr>\
         </w:body></w:document>",
        body
    );

    let content_types = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
         <Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
         <Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
         <Default Extension=\"xml\" ContentType=\"application/xml\"/>\
         <Override PartName=\"/word/document.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml\"/>\
         </Types>";

    let rels = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
         <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
         <Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"word/document.xml\"/>\
         </Relationships>";

    write_zip(
        vec![
            OutEntry {
                name: "[Content_Types].xml".to_string(),
                data: content_types.as_bytes().to_vec(),
            },
            OutEntry {
                name: "_rels/.rels".to_string(),
                data: rels.as_bytes().to_vec(),
            },
            OutEntry {
                name: "word/document.xml".to_string(),
                data: document.as_bytes().to_vec(),
            },
        ],
        dst,
    )
}

// ============== CSV → XLSX ==============

/// 支持引号包裹、双引号转义、字段内逗号与换行
pub(crate) fn parse_csv(content: &str) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut in_quotes = false;
    let mut chars = content.chars().peekable();

    while let Some(ch) = chars.next() {
        match ch {
            '"' if in_quotes => {
                if chars.peek() == Some(&'"') {
                    field.push('"');
                    chars.next();
                } else {
                    in_quotes = false;
                }
            }
            '"' => in_quotes = true,
            ',' if !in_quotes => row.push(std::mem::take(&mut field)),
            '\r' if !in_quotes => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                row.push(std::mem::take(&mut field));
                if row.iter().any(|c| !c.trim().is_empty()) {
                    rows.push(std::mem::take(&mut row));
                } else {
                    row.clear();
                }
            }
            '\n' if !in_quotes => {
                row.push(std::mem::take(&mut field));
                if row.iter().any(|c| !c.trim().is_empty()) {
                    rows.push(std::mem::take(&mut row));
                } else {
                    row.clear();
                }
            }
            _ => field.push(ch),
        }
    }
    row.push(field);
    if row.iter().any(|c| !c.trim().is_empty()) {
        rows.push(row);
    }
    rows
}

fn sheet_xml(rows: &[Vec<String>]) -> String {
    let mut sheet = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
         <worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"><sheetData>",
    );
    for (row_index, row) in rows.iter().enumerate() {
        sheet.push_str(&format!("<row r=\"{}\">", row_index + 1));
        for (col_index, value) in row.iter().enumerate() {
            if value.is_empty() {
                continue;
            }
            let reference = format!("{}{}", column_name(col_index), row_index + 1);
            if let Ok(number) = value.trim().parse::<f64>() {
                sheet.push_str(&format!("<c r=\"{}\"><v>{}</v></c>", reference, number));
                continue;
            }
            sheet.push_str(&format!(
                "<c r=\"{}\" t=\"inlineStr\"><is><t xml:space=\"preserve\">{}</t></is></c>",
                reference,
                escape_xml(value)
            ));
        }
        sheet.push_str("</row>");
    }
    sheet.push_str("</sheetData></worksheet>");
    sheet
}

/// 二维字符串表 → 单表 XLSX
pub fn rows_to_xlsx(rows: &[Vec<String>], sheet_name: &str, dst: &Path) -> Result<(), String> {
    if rows.is_empty() {
        return Err("没有可写入的内容".to_string());
    }
    let content_types = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
         <Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
         <Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
         <Default Extension=\"xml\" ContentType=\"application/xml\"/>\
         <Override PartName=\"/xl/workbook.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml\"/>\
         <Override PartName=\"/xl/worksheets/sheet1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml\"/>\
         </Types>";

    let rels = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
         <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
         <Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"xl/workbook.xml\"/>\
         </Relationships>";

    let workbook = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
         <workbook xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" \
         xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\">\
         <sheets><sheet name=\"{}\" sheetId=\"1\" r:id=\"rId1\"/></sheets></workbook>",
        escape_xml(sheet_name)
    );

    let workbook_rels = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
         <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
         <Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet\" Target=\"worksheets/sheet1.xml\"/>\
         </Relationships>";

    write_zip(
        vec![
            OutEntry {
                name: "[Content_Types].xml".to_string(),
                data: content_types.as_bytes().to_vec(),
            },
            OutEntry {
                name: "_rels/.rels".to_string(),
                data: rels.as_bytes().to_vec(),
            },
            OutEntry {
                name: "xl/workbook.xml".to_string(),
                data: workbook.as_bytes().to_vec(),
            },
            OutEntry {
                name: "xl/_rels/workbook.xml.rels".to_string(),
                data: workbook_rels.as_bytes().to_vec(),
            },
            OutEntry {
                name: "xl/worksheets/sheet1.xml".to_string(),
                data: sheet_xml(rows).into_bytes(),
            },
        ],
        dst,
    )
}

pub fn csv_to_xlsx(src: &Path, dst: &Path, cancelled: &dyn Fn() -> bool) -> Result<(), String> {
    let raw = std::fs::read(src).map_err(|e| format!("读取失败: {}", e))?;
    // 兼容带 BOM 的 CSV（Excel 导出常见）
    let content = if raw.starts_with(&[0xEF, 0xBB, 0xBF]) {
        String::from_utf8_lossy(&raw[3..]).to_string()
    } else {
        String::from_utf8_lossy(&raw).to_string()
    };
    if cancelled() {
        return Err("已取消".to_string());
    }
    let rows = parse_csv(&content);
    if rows.is_empty() {
        return Err("CSV 没有可写入的内容".to_string());
    }

    let content_types = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
         <Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
         <Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
         <Default Extension=\"xml\" ContentType=\"application/xml\"/>\
         <Override PartName=\"/xl/workbook.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml\"/>\
         <Override PartName=\"/xl/worksheets/sheet1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml\"/>\
         </Types>";

    let rels = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
         <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
         <Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"xl/workbook.xml\"/>\
         </Relationships>";

    let workbook = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
         <workbook xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" \
         xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\">\
         <sheets><sheet name=\"Sheet1\" sheetId=\"1\" r:id=\"rId1\"/></sheets></workbook>";

    let workbook_rels = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
         <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
         <Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet\" Target=\"worksheets/sheet1.xml\"/>\
         </Relationships>";

    write_zip(
        vec![
            OutEntry {
                name: "[Content_Types].xml".to_string(),
                data: content_types.as_bytes().to_vec(),
            },
            OutEntry {
                name: "_rels/.rels".to_string(),
                data: rels.as_bytes().to_vec(),
            },
            OutEntry {
                name: "xl/workbook.xml".to_string(),
                data: workbook.as_bytes().to_vec(),
            },
            OutEntry {
                name: "xl/_rels/workbook.xml.rels".to_string(),
                data: workbook_rels.as_bytes().to_vec(),
            },
            OutEntry {
                name: "xl/worksheets/sheet1.xml".to_string(),
                data: sheet_xml(&rows).into_bytes(),
            },
        ],
        dst,
    )
}
