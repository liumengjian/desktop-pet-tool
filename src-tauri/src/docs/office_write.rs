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

// ============== 结构化文档（段落 + 表格）→ DOCX / XLSX ==============

use super::pdf_extract::{
    BodyImage, Cell, FooterBlock, HeaderBlock, HeaderImage, HeaderLine, LineData, TableData,
};

pub enum DocItem {
    Para(LineData),
    Table(TableData),
    Image(BodyImage),
}

fn hex_color(c: [u8; 3]) -> String {
    format!("{:02X}{:02X}{:02X}", c[0], c[1], c[2])
}

/// 字号：PDF pt → Word 半磅
fn half_points(size: f32) -> u32 {
    ((size.max(5.0)) * 2.0).round() as u32
}

fn docx_run(text: &str, color: Option<[u8; 3]>, size: f32, bold: bool) -> String {
    let mut props = String::new();
    if bold {
        props.push_str("<w:b/>");
    }
    if let Some(c) = color {
        props.push_str(&format!("<w:color w:val=\"{}\"/>", hex_color(c)));
    }
    if size > 0.0 {
        props.push_str(&format!("<w:sz w:val=\"{}\"/>", half_points(size)));
    }
    // 单元格里的多行文字用软换行，保持原 PDF 的分段（w:br 必须与 w:t 并列在 w:r 内）
    let mut body = String::new();
    for (i, seg) in text.split('\n').enumerate() {
        if i > 0 {
            body.push_str("<w:br/>");
        }
        body.push_str(&format!(
            "<w:t xml:space=\"preserve\">{}</w:t>",
            escape_xml(seg)
        ));
    }
    if props.is_empty() {
        format!("<w:r>{}</w:r>", body)
    } else {
        format!("<w:r><w:rPr>{}</w:rPr>{}</w:r>", props, body)
    }
}

/// 正文段落：按 PDF 里这一行的实际横向位置决定对齐。
/// 标题行在 PDF 里是居中的、抬头是左右两段并排、正文首个行是缩进 2 字的，
/// 一律写成左对齐就会把版式抹平。
fn docx_para(line: &LineData, left: f32, right: f32, before_twips: u32) -> String {
    let width = (right - left).max(1.0);
    let (lx, rx) = (line.x0, line.x1.max(line.x0 + 1.0));
    let center_off = ((lx + rx) / 2.0 - (left + right) / 2.0).abs();
    let centered = center_off <= (line.size * 0.35).max(4.0) && (rx - lx) < width * 0.86;
    let indent_pt = lx - left;
    let spacing = if before_twips > 0 {
        format!("<w:spacing w:before=\"{}\" w:after=\"0\"/>", before_twips)
    } else {
        String::new()
    };
    let mut ppr = spacing.clone();
    // 行内跨栏空白：后半段用右制表位推到版面右侧
    if let Some(at) = line.split_at {
        let head: String = line.text.chars().take(at).collect();
        let tail: String = line.text.chars().skip(at).collect();
        let tail = tail.trim_start();
        if !tail.is_empty() {
            // pPr 的子元素有固定顺序：tabs 必须排在 spacing 前面
            ppr = format!(
                "<w:tabs><w:tab w:val=\"right\" w:pos=\"{}\"/></w:tabs>{}",
                ((width * 20.0).round() as u32).min(10400),
                spacing
            );
            let runs = format!(
                "{}<w:r><w:tab/></w:r>{}",
                docx_run(head.trim_end(), line.color, line.size, false),
                docx_run(tail, line.color, line.size, false)
            );
            return format!("<w:p><w:pPr>{}</w:pPr>{}</w:p>", ppr, runs);
        }
    }
    if centered {
        ppr.push_str("<w:jc w:val=\"center\"/>");
    } else if indent_pt > line.size * 0.5 {
        ppr.push_str(&format!(
            "<w:ind w:left=\"{}\"/><w:jc w:val=\"left\"/>",
            ((indent_pt * 20.0).round() as u32).min(8000)
        ));
    }
    if ppr.is_empty() {
        docx_run_fmt(&docx_run(&line.text, line.color, line.size, false))
    } else {
        format!(
            "<w:p><w:pPr>{}</w:pPr>{}</w:p>",
            ppr,
            docx_run(&line.text, line.color, line.size, false)
        )
    }
}

fn docx_run_fmt(runs: &str) -> String {
    format!("<w:p>{}</w:p>", runs)
}

/// 正文图片段落：按 PDF 里的显示尺寸等比缩到版心宽度内，居中排
fn docx_image(rel: &str, img: &super::pdf_extract::BodyImage, id: usize) -> String {
    const MAX_W: f32 = 520.0;
    let mut w = img.width.max(1.0);
    let mut h = img.height.max(1.0);
    if w > MAX_W {
        h *= MAX_W / w;
        w = MAX_W;
    }
    let cx = ((w * 12700.0).round() as u64).max(1);
    let cy = ((h * 12700.0).round() as u64).max(1);
    format!(
        "<w:p><w:pPr><w:jc w:val=\"center\"/></w:pPr><w:r><w:drawing>\
         <wp:inline distT=\"0\" distB=\"0\" distL=\"0\" distR=\"0\">\
         <wp:extent cx=\"{cx}\" cy=\"{cy}\"/><wp:docPr id=\"{id}\" name=\"picture {id}\"/>\
         <a:graphic><a:graphicData uri=\"http://schemas.openxmlformats.org/drawingml/2006/picture\">\
         <pic:pic><pic:nvPicPr><pic:cNvPr id=\"{id}\" name=\"picture {id}\"/><pic:cNvPicPr/></pic:nvPicPr>\
         <pic:blipFill><a:blip r:embed=\"{rel}\"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill>\
         <pic:spPr><a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"{cx}\" cy=\"{cy}\"/></a:xfrm>\
         <a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></pic:spPr></pic:pic>\
         </a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>"
    )
}

fn docx_table(table: &TableData) -> String {
    let rows = &table.rows;
    let cols = rows.iter().map(|r| r.len()).max().unwrap_or(1).max(1);
    let total_width = 10466usize; // A4 减去左右 1.27cm 页边距后的正文宽度（twips）
    // 列宽优先用 PDF 框线的真实间距，退回时按各列最长内容的字数分配
    let weighted = |rows: &Vec<Vec<Cell>>| -> Vec<usize> {
        let mut weights: Vec<f32> = vec![1.0; cols];
        for row in rows {
            for (j, cell) in row.iter().enumerate() {
                let chars = cell.text.chars().count() as f32;
                if j < cols {
                    weights[j] = weights[j].max(chars.min(40.0));
                }
            }
        }
        let sum: f32 = weights.iter().sum::<f32>().max(1.0);
        weights
            .iter()
            .map(|w| ((*w / sum) * total_width as f32).max(600.0) as usize)
            .collect()
    };
    let widths: Vec<usize> = if table.widths.len() == cols && cols > 0 {
        let sum: f32 = table.widths.iter().sum::<f32>().max(1.0);
        table
            .widths
            .iter()
            .map(|w| ((*w / sum) * total_width as f32).max(480.0) as usize)
            .collect()
    } else {
        weighted(rows)
    };
    let width_sum: usize = widths.iter().sum();
    let col_width = |j: usize| -> usize {
        if width_sum > 0 {
            (widths[j] * total_width) / width_sum
        } else {
            total_width / cols
        }
    };
    // 表头判据：首行整行都有底色，或字号明显大于正文
    let header_row = rows.first().map(|r| {
        r.iter().filter(|c| c.fill.is_some()).count() >= r.len().saturating_sub(1) && r.len() > 1
    });
    let mut out = String::new();
    out.push_str("<w:tbl><w:tblPr><w:tblW w:w=\"0\" w:type=\"auto\"/><w:tblBorders>");
    for side in ["top", "left", "bottom", "right", "insideH", "insideV"] {
        out.push_str(&format!(
            "<w:{} w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"808080\"/>",
            side
        ));
    }
    out.push_str("</w:tblBorders><w:tblCellMar>\
        <w:top w:w=\"40\" w:type=\"dxa\"/><w:left w:w=\"80\" w:type=\"dxa\"/>\
        <w:bottom w:w=\"40\" w:type=\"dxa\"/><w:right w:w=\"80\" w:type=\"dxa\"/>\
        </w:tblCellMar></w:tblPr><w:tblGrid>");
    for j in 0..cols {
        out.push_str(&format!("<w:gridCol w:w=\"{}\"/>", col_width(j)));
    }
    out.push_str("</w:tblGrid>");
    for (row_index, row) in rows.iter().enumerate() {
        out.push_str("<w:tr>");
        let is_header = header_row.unwrap_or(false) && row_index == 0;
        let mut col = 0usize;
        while col < cols {
            let cell = row.get(col);
            let span = match cell {
                Some(c) => c.span.max(1).min(cols - col),
                None => 1,
            };
            let (text, fill, color, size) = match cell {
                Some(c) => (c.text.as_str(), c.fill, c.color, c.size),
                None => ("", None, None, 0.0),
            };
            let span_width: usize = (0..span).map(|k| col_width(col + k)).sum();
            let mut tc_props = format!("<w:tcW w:w=\"{}\" w:type=\"dxa\"/>", span_width);
            if span > 1 {
                // 一句长文字跨过列线：合并成跨列单元格，才不会挤在窄列里折成一列字
                tc_props.push_str(&format!("<w:gridSpan w:val=\"{}\"/>", span));
            }
            if let Some(f) = fill {
                // 单元格底纹就是 PDF 里表头那条绿色填充矩形
                tc_props.push_str(&format!(
                    "<w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"{}\"/>",
                    hex_color(f)
                ));
            }
            let rendered = docx_run(text, color, size, is_header);
            out.push_str(&format!(
                "<w:tc><w:tcPr>{}</w:tcPr><w:p>{}</w:p></w:tc>",
                tc_props, rendered
            ));
            col += span;
        }
        out.push_str("</w:tr>");
    }
    out.push_str("</w:tbl>");
    // Word 要求表格后面紧跟一个段落，否则文档结构不完整
    out.push_str("<w:p/>");
    out
}

/// 页眉里一张 logo 的内联图，尺寸按 PDF 里的显示尺寸
fn band_image_run(rel: &str, img: &HeaderImage, id: usize) -> String {
    let cx = ((img.width * 12700.0).round() as u64).max(1);
    let cy = ((img.height * 12700.0).round() as u64).max(1);
    format!(
        "<w:r><w:drawing>\
         <wp:inline distT=\"0\" distB=\"0\" distL=\"0\" distR=\"0\">\
         <wp:extent cx=\"{cx}\" cy=\"{cy}\"/><wp:docPr id=\"{id}\" name=\"header logo {id}\"/>\
         <a:graphic><a:graphicData uri=\"http://schemas.openxmlformats.org/drawingml/2006/picture\">\
         <pic:pic><pic:nvPicPr><pic:cNvPr id=\"{id}\" name=\"header logo {id}\"/><pic:cNvPicPr/></pic:nvPicPr>\
         <pic:blipFill><a:blip r:embed=\"{rel}\"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill>\
         <pic:spPr><a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"{cx}\" cy=\"{cy}\"/></a:xfrm>\
         <a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></pic:spPr></pic:pic>\
         </a:graphicData></a:graphic></wp:inline></w:drawing></w:r>",
        id = id,
        rel = rel,
        cx = cx,
        cy = cy
    )
}

/// 估算一段文字的横向宽度（中文按 1em，ASCII 按 0.55em）
fn band_text_width(line: &HeaderLine) -> f32 {
    line.text
        .chars()
        .map(|c| if (c as u32) >= 0x2E80 { line.size } else { line.size * 0.55 })
        .sum()
}

#[derive(Clone)]
enum BandItem<'a> {
    Image(&'a HeaderImage, String),
    Text(&'a HeaderLine),
}

/// 页眉/页脚里「并排的一栏」：PDF 里横向不重叠的几段各占一栏
struct BandCell<'a> {
    x0: f32,
    x1: f32,
    /// (行序, 内容)，行序用于栏内从上到下的排序
    items: Vec<(usize, BandItem<'a>)>,
}

fn insert_band_cell<'a>(
    cells: &mut Vec<BandCell<'a>>,
    x0: f32,
    x1: f32,
    row: usize,
    item: BandItem<'a>,
) {
    if let Some(c) = cells
        .iter_mut()
        .find(|c| x0 <= c.x1 + 2.0 && c.x0 <= x1 + 2.0)
    {
        c.x0 = c.x0.min(x0);
        c.x1 = c.x1.max(x1);
        c.items.push((row, item));
        return;
    }
    cells.push(BandCell {
        x0,
        x1,
        items: vec![(row, item)],
    });
}

/// 页眉 XML：按 PDF 里的横向位置把 logo 与各栏文字放回各自的位置。
/// Word 页眉里没有自由的横向定位，用一张无边框表，列宽取「下一栏起点 - 本栏起点」，
/// 这样 logo、两家公司抬头、下面的英文行都能落在原 PDF 的位置上。
fn header_xml(header: &HeaderBlock, image_rels: &[String]) -> String {
    let mut cells: Vec<BandCell> = Vec::new();
    for (ri, row) in header.rows.iter().enumerate() {
        for line in row.iter() {
            let w = band_text_width(line);
            insert_band_cell(&mut cells, line.x, line.x + w, ri, BandItem::Text(line));
        }
    }
    for (i, img) in header.images.iter().enumerate() {
        if let Some(rel) = image_rels.get(i) {
            insert_band_cell(
                &mut cells,
                img.x,
                img.x + img.width,
                0,
                BandItem::Image(img, rel.clone()),
            );
        }
    }
    cells.sort_by(|a, b| a.x0.partial_cmp(&b.x0).unwrap_or(std::cmp::Ordering::Equal));

    if cells.is_empty() {
        return band_part("hdr", String::new());
    }

    // PDF 版心左边距按 54pt 算；表格缩进到第一栏的起点，列宽等于到下一栏的距离
    let left_margin = 54.0f32;
    let right = cells.iter().map(|c| c.x1).fold(0.0, f32::max) + 6.0;
    let indent = ((cells[0].x0 - left_margin).max(0.0) * 20.0).round() as u64;
    let mut widths: Vec<f32> = Vec::new();
    for i in 0..cells.len() {
        let next = cells
            .get(i + 1)
            .map(|c| c.x0)
            .unwrap_or(right);
        widths.push((next - cells[i].x0).max(30.0));
    }
    let total: f32 = widths.iter().sum();

    let mut grid = String::new();
    for w in widths.iter() {
        grid.push_str(&format!(
            "<w:gridCol w:w=\"{}\"/>",
            (w * 20.0).round() as u64
        ));
    }

    let mut row_xml = String::new();
    for (i, cell) in cells.iter().enumerate() {
        let mut inner = String::new();
        let mut items = cell.items.clone();
        items.sort_by(|a, b| a.0.cmp(&b.0));
        let mut img_id = 1usize;
        for (_, item) in items.iter() {
            match item {
                BandItem::Text(line) => {
                    inner.push_str(&format!(
                        "<w:p><w:pPr><w:spacing w:before=\"0\" w:after=\"0\" w:line=\"240\" w:lineRule=\"auto\"/><w:jc w:val=\"left\"/></w:pPr>{}</w:p>",
                        docx_run(&line.text, line.color, line.size, false)
                    ));
                }
                BandItem::Image(img, rel) => {
                    inner.push_str(&format!(
                        "<w:p><w:pPr><w:spacing w:before=\"0\" w:after=\"0\" w:line=\"240\" w:lineRule=\"auto\"/><w:jc w:val=\"left\"/></w:pPr>{}</w:p>",
                        band_image_run(rel, img, img_id)
                    ));
                    img_id += 1;
                }
            }
        }
        if inner.is_empty() {
            inner.push_str("<w:p/>");
        }
        row_xml.push_str(&format!(
            "<w:tc><w:tcPr><w:tcW w:w=\"{}\" w:type=\"dxa\"/></w:tcPr>{}</w:tc>",
            (widths[i] * 20.0).round() as u64,
            inner
        ));
    }

    let table = format!(
        "<w:tbl><w:tblPr><w:tblW w:w=\"{total}\" w:type=\"dxa\"/>\
         <w:tblInd w:w=\"{indent}\" w:type=\"dxa\"/><w:tblLayout w:type=\"fixed\"/>\
         <w:tblBorders><w:top w:val=\"none\"/><w:left w:val=\"none\"/><w:bottom w:val=\"none\"/><w:right w:val=\"none\"/><w:insideH w:val=\"none\"/><w:insideV w:val=\"none\"/></w:tblBorders>\
         <w:tblCellMar><w:top w:w=\"0\" w:type=\"dxa\"/><w:left w:w=\"0\" w:type=\"dxa\"/><w:bottom w:w=\"0\" w:type=\"dxa\"/><w:right w:w=\"0\" w:type=\"dxa\"/></w:tblCellMar>\
         </w:tblPr><w:tblGrid>{grid}</w:tblGrid><w:tr>{row_xml}</w:tr></w:tbl>",
        total = (total * 20.0).round() as u64,
        indent = indent,
        grid = grid,
        row_xml = row_xml
    );
    band_part("hdr", table)
}

/// 页码「第1 页共2 页」换成 Word 域，翻页后会自动更新
fn page_number_runs(text: &str) -> Option<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        if chars[i] == '第' {
            let mut j = i + 1;
            while j < chars.len() && (chars[j].is_ascii_digit() || chars[j] == ' ') {
                j += 1;
            }
            if j + 1 < chars.len() && chars[j] == '页' && chars[j + 1] == '共' {
                let mut k = j + 2;
                while k < chars.len() && (chars[k].is_ascii_digit() || chars[k] == ' ') {
                    k += 1;
                }
                if k < chars.len() && chars[k] == '页' {
                    let prefix: String = chars[..i].iter().collect();
                    let suffix: String = chars[k + 1..].iter().collect();
                    let mut out = String::new();
                    if !prefix.trim().is_empty() {
                        out.push_str(&docx_run(prefix.trim_end(), None, 10.4, false));
                    }
                    out.push_str(
                        "<w:r><w:t xml:space=\"preserve\">第</w:t></w:r>\
                         <w:fldSimple w:instr=\" PAGE \"><w:r><w:t>1</w:t></w:r></w:fldSimple>\
                         <w:r><w:t xml:space=\"preserve\"> 页共</w:t></w:r>\
                         <w:fldSimple w:instr=\" NUMPAGES \"><w:r><w:t>1</w:t></w:r></w:fldSimple>\
                         <w:r><w:t xml:space=\"preserve\"> 页</w:t></w:r>",
                    );
                    if !suffix.trim().is_empty() {
                        out.push_str(&docx_run(suffix.trim_start(), None, 10.4, false));
                    }
                    return Some(out);
                }
            }
        }
        i += 1;
    }
    None
}

/// 页脚 XML：地址、英文地址、电话传真各占一行，行内各段按原 PDF 的间距补空格
fn footer_xml(footer: &FooterBlock, image_rels: &[String]) -> String {
    let mut body = String::new();
    for (i, img) in footer.images.iter().enumerate() {
        if let Some(rel) = image_rels.get(i) {
            body.push_str(&format!(
                "<w:p><w:pPr><w:spacing w:before=\"0\" w:after=\"0\"/><w:jc w:val=\"left\"/></w:pPr>{}</w:p>",
                band_image_run(rel, img, i + 1)
            ));
        }
    }
    // 页脚用一张无边框两列表：左列放地址/电话，右列单独放页码。
    // 页码靠右排，地址再长也不会挤到它。
    const TOTAL: u32 = 10466;
    const RIGHT_COL: u32 = 1900;
    let left_col = TOTAL - RIGHT_COL;
    let mut rows_xml = String::new();
    let mut row_count = 0usize;
    for row in footer.rows.iter() {
        let mut left_runs = String::new();
        let mut right_runs = String::new();
        let mut prev_end = f32::NAN;
        let mut has_page_field = false;
        for line in row.iter() {
            match page_number_runs(&line.text) {
                Some(field) => {
                    right_runs.push_str(&field);
                    has_page_field = true;
                }
                None => {
                    if !prev_end.is_nan() {
                        let gap = (line.x - prev_end).max(0.0);
                        if gap > 3.0 {
                            let spaces =
                                ((gap / (line.size.max(6.0) * 0.5)).round() as usize).clamp(1, 12);
                            left_runs.push_str(&format!(
                                "<w:r><w:t xml:space=\"preserve\">{}</w:t></w:r>",
                                " ".repeat(spaces)
                            ));
                        }
                    }
                    left_runs.push_str(&docx_run(&line.text, line.color, line.size, false));
                    prev_end = line.x + band_text_width(line);
                }
            }
        }
        if left_runs.is_empty() && right_runs.is_empty() {
            continue;
        }
        let para = |runs: &str, jc: &str| -> String {
            format!(
                "<w:p><w:pPr><w:spacing w:before=\"0\" w:after=\"0\" w:line=\"240\" w:lineRule=\"auto\"/><w:jc w:val=\"{jc}\"/></w:pPr>{runs}</w:p>",
                jc = jc,
                runs = runs
            )
        };
        if has_page_field && !left_runs.is_empty() {
            rows_xml.push_str(&format!(
                "<w:tr><w:tc><w:tcPr><w:tcW w:w=\"{left}\" w:type=\"dxa\"/></w:tcPr>{}</w:tc>\
                 <w:tc><w:tcPr><w:tcW w:w=\"{right}\" w:type=\"dxa\"/></w:tcPr>{}</w:tc></w:tr>",
                para(&left_runs, "left"),
                para(&right_runs, "right"),
                left = left_col,
                right = RIGHT_COL
            ));
        } else {
            // 没有页码的行整行占满，保持原 PDF 的左对齐
            let runs = if left_runs.is_empty() {
                right_runs.clone()
            } else {
                left_runs.clone()
            };
            rows_xml.push_str(&format!(
                "<w:tr><w:tc><w:tcPr><w:tcW w:w=\"{total}\" w:type=\"dxa\"/><w:gridSpan w:val=\"2\"/></w:tcPr>{}</w:tc></w:tr>",
                para(&runs, "left"),
                total = TOTAL
            ));
        }
        row_count += 1;
    }
    if rows_xml.is_empty() {
        return band_part("ftr", body);
    }
    body.push_str(&format!(
        "<w:tbl><w:tblPr><w:tblW w:w=\"{total}\" w:type=\"dxa\"/><w:tblLayout w:type=\"fixed\"/>\
         <w:tblBorders><w:top w:val=\"none\"/><w:left w:val=\"none\"/><w:bottom w:val=\"none\"/><w:right w:val=\"none\"/><w:insideH w:val=\"none\"/><w:insideV w:val=\"none\"/></w:tblBorders>\
         <w:tblCellMar><w:top w:w=\"0\" w:type=\"dxa\"/><w:left w:w=\"0\" w:type=\"dxa\"/><w:bottom w:w=\"0\" w:type=\"dxa\"/><w:right w:w=\"0\" w:type=\"dxa\"/></w:tblCellMar>\
         </w:tblPr><w:tblGrid><w:gridCol w:w=\"{left}\"/><w:gridCol w:w=\"{right}\"/></w:tblGrid>{rows}</w:tbl>",
        total = TOTAL,
        left = left_col,
        right = RIGHT_COL,
        rows = rows_xml
    ));
    let _ = row_count;
    band_part("ftr", body)
}

fn band_part(tag: &str, body: String) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
         <w:{tag} xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\" \
         xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" \
         xmlns:wp=\"http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing\" \
         xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" \
         xmlns:pic=\"http://schemas.openxmlformats.org/drawingml/2006/picture\">{body}</w:{tag}>",
        tag = tag,
        body = body
    )
}

/// 按页面结构写出 DOCX：普通行是段落，识别到的表格写成真正的 Word 表格，
/// 并保留 PDF 里的文字颜色、单元格底纹与页眉（含 logo 图片）
pub fn doc_items_to_docx(
    items: &[DocItem],
    header: Option<&HeaderBlock>,
    footer: Option<&FooterBlock>,
    dst: &Path,
    cancelled: &dyn Fn() -> bool,
) -> Result<(), String> {
    let mut body = String::new();
    let mut body_images: Vec<Vec<u8>> = Vec::new();
    // 先用正文行自己的左右边界推出这一页的版心，作为居中/缩进的判断基准
    let (mut left_edge, mut right_edge) = (f32::MAX, 0.0f32);
    for item in items {
        if let DocItem::Para(line) = item {
            if line.x0 > 20.0 && line.x0 < 200.0 {
                left_edge = left_edge.min(line.x0);
            }
            right_edge = right_edge.max(line.x1);
        }
    }
    if !left_edge.is_finite() {
        left_edge = 54.0;
    }
    if right_edge < left_edge + 200.0 {
        right_edge = left_edge + 487.0;
    }
    let mut prev_y = f32::NAN;
    for item in items {
        if cancelled() {
            return Err("已取消".to_string());
        }
        match item {
            DocItem::Para(line) => {
                if line.text.trim().is_empty() {
                    continue;
                }
                // 相邻两行的基线差减去行高，就是 PDF 里留出来的段间距；
                // 换页或跨度过大（表格跳页）时不对齐，交给 Word 自己排
                let dy = prev_y - line.y;
                let before = if dy.is_finite() && dy > 0.0 && dy < 100.0 {
                    (((dy - line.size * 1.18).max(0.0) * 20.0).round() as u32).min(600)
                } else {
                    0
                };
                body.push_str(&docx_para(line, left_edge, right_edge, before));
                prev_y = line.y;
            }
            DocItem::Table(table) => {
                if table.rows.is_empty() {
                    continue;
                }
                body.push_str(&docx_table(table));
                prev_y = f32::NAN;
            }
            DocItem::Image(img) => {
                if img.png.is_empty() {
                    continue;
                }
                body_images.push(img.png.clone());
                let id = body_images.len();
                body.push_str(&docx_image(&format!("rId{}", id + 10), img, id));
                prev_y = f32::NAN;
            }
        }
    }

    let has_header = header
        .map(|h| !h.rows.is_empty() || !h.images.is_empty())
        .unwrap_or(false);
    let has_footer = footer
        .map(|f| !f.rows.is_empty() || !f.images.is_empty())
        .unwrap_or(false);
    let mut extras: Vec<OutEntry> = Vec::new();
    let mut section = String::new();
    if has_header {
        section.push_str(&body_header_reference());
    }
    if has_footer {
        section.push_str("<w:footerReference w:type=\"default\" r:id=\"rId3\"/>");
    }
    let document = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
         <w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\" \
         xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" \
         xmlns:wp=\"http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing\" \
         xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" \
         xmlns:pic=\"http://schemas.openxmlformats.org/drawingml/2006/picture\"><w:body>{}\
         <w:sectPr>{}<w:pgSz w:w=\"11906\" w:h=\"16838\"/>\
         <w:pgMar w:top=\"900\" w:right=\"720\" w:bottom=\"1080\" w:left=\"720\" \
         w:header=\"283\" w:footer=\"425\"/></w:sectPr>\
         </w:body></w:document>",
        body, section
    );

    let mut content_types = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
         <Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
         <Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
         <Default Extension=\"xml\" ContentType=\"application/xml\"/>\
         <Default Extension=\"png\" ContentType=\"image/png\"/>\
         <Override PartName=\"/word/document.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml\"/>",
    );
    let mut doc_rels = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
         <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">",
    );

    let mut image_rels: Vec<String> = Vec::new();
    let mut header_has_image = false;
    let mut header_rels = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
         <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">",
    );
    if let Some(h) = header {
        if has_header {
            content_types.push_str(
                "<Override PartName=\"/word/header1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.header+xml\"/>",
            );
            doc_rels.push_str(
                "<Relationship Id=\"rId2\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/header\" Target=\"header1.xml\"/>",
            );
            for (i, img) in h.images.iter().enumerate() {
                let rel = format!("rId{}", i + 1);
                let file = if i == 0 {
                    "media/header-logo.png".to_string()
                } else {
                    format!("media/header-logo{}.png", i + 1)
                };
                extras.push(OutEntry {
                    name: format!("word/{}", file),
                    data: img.png.clone(),
                });
                // 页眉部件有自己的关系表，图片必须在 header1.xml.rels 里声明，
                // 只写在 document.xml.rels 里 Word 解析不到，会把图片画成空白
                header_rels.push_str(&format!(
                    "<Relationship Id=\"{}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/image\" Target=\"{}\"/>",
                    rel, file
                ));
                image_rels.push(rel);
                header_has_image = true;
            }
            extras.push(OutEntry {
                name: "word/header1.xml".to_string(),
                data: header_xml(h, &image_rels).into_bytes(),
            });
            if header_has_image {
                header_rels.push_str("</Relationships>");
                extras.push(OutEntry {
                    name: "word/_rels/header1.xml.rels".to_string(),
                    data: header_rels.into_bytes(),
                });
            }
        }
    }

    // 页脚：和页眉一样是独立部件，图片必须声明在 footer1.xml.rels 里
    let mut footer_image_rels: Vec<String> = Vec::new();
    if let Some(f) = footer {
        if has_footer {
            content_types.push_str(
                "<Override PartName=\"/word/footer1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.footer+xml\"/>",
            );
            doc_rels.push_str(
                "<Relationship Id=\"rId3\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/footer\" Target=\"footer1.xml\"/>",
            );
            let mut footer_rels = String::from(
                "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
                 <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">",
            );
            let mut footer_has_image = false;
            for (i, img) in f.images.iter().enumerate() {
                let rel = format!("rId{}", i + 1);
                let file = if i == 0 {
                    "media/footer-img.png".to_string()
                } else {
                    format!("media/footer-img{}.png", i + 1)
                };
                extras.push(OutEntry {
                    name: format!("word/{}", file),
                    data: img.png.clone(),
                });
                footer_rels.push_str(&format!(
                    "<Relationship Id=\"{}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/image\" Target=\"{}\"/>",
                    rel, file
                ));
                footer_image_rels.push(rel);
                footer_has_image = true;
            }
            extras.push(OutEntry {
                name: "word/footer1.xml".to_string(),
                data: footer_xml(f, &footer_image_rels).into_bytes(),
            });
            if footer_has_image {
                footer_rels.push_str("</Relationships>");
                extras.push(OutEntry {
                    name: "word/_rels/footer1.xml.rels".to_string(),
                    data: footer_rels.into_bytes(),
                });
            }
        }
    }

    content_types.push_str("</Types>");
    // 正文图片：media 部件 + document.xml.rels 里的关系声明，缺一个 Word 就画不出来
    for (i, png) in body_images.into_iter().enumerate() {
        let n = i + 1;
        let file = format!("media/body-img{}.png", n);
        extras.push(OutEntry {
            name: format!("word/{}", file),
            data: png,
        });
        doc_rels.push_str(&format!(
            "<Relationship Id=\"rId{}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/image\" Target=\"{}\"/>",
            n + 10,
            file
        ));
    }
    doc_rels.push_str("</Relationships>");

    let rels = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
         <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
         <Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"word/document.xml\"/>\
         </Relationships>";

    let mut entries = vec![
        OutEntry {
            name: "[Content_Types].xml".to_string(),
            data: content_types.into_bytes(),
        },
        OutEntry {
            name: "_rels/.rels".to_string(),
            data: rels.as_bytes().to_vec(),
        },
        OutEntry {
            name: "word/document.xml".to_string(),
            data: document.into_bytes(),
        },
        OutEntry {
            name: "word/_rels/document.xml.rels".to_string(),
            data: doc_rels.into_bytes(),
        },
    ];
    entries.append(&mut extras);
    write_zip(entries, dst)
}

fn body_header_reference() -> String {
    "<w:headerReference w:type=\"default\" r:id=\"rId2\"/>".to_string()
}

// ============== 结构化文档 → XLSX ==============

/// 单元格样式索引：0 = 默认、1 = 表头（有底色 + 边框）、2 = 普通格（带边框）
fn xlsx_styles(header_fill: [u8; 3]) -> String {
    let fills = format!(
        "<fills count=\"3\"><fill><patternFill patternType=\"none\"/></fill>\
         <fill><patternFill patternType=\"gray125\"/></fill>\
         <fill><patternFill patternType=\"solid\"><fgColor rgb=\"FF{}\"/><bgColor indexed=\"64\"/></patternFill></fill></fills>",
        hex_color(header_fill)
    );
    let fonts = "<fonts count=\"2\"><font><sz val=\"10\"/><name val=\"宋体\"/></font>\
         <font><b/><sz val=\"10\"/><name val=\"宋体\"/></font></fonts>";
    let xfs = "<cellXfs count=\"3\">\
         <xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"0\" xfId=\"0\"/>\
         <xf numFmtId=\"0\" fontId=\"1\" fillId=\"2\" borderId=\"1\" xfId=\"0\" applyFont=\"1\" applyFill=\"1\" applyBorder=\"1\"/>\
         <xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"1\" xfId=\"0\" applyBorder=\"1\"/></cellXfs>";
    let cell_style_xfs =
        "<cellStyleXfs count=\"1\"><xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"0\"/></cellStyleXfs>";
    let cell_styles = "<cellStyles count=\"1\"><cellStyle name=\"Normal\" xfId=\"0\" builtinId=\"0\"/></cellStyles>";
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
         <styleSheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\">\
         <borders count=\"2\"><border><left/><right/><top/><bottom/><diagonal/></border>\
         <border><left style=\"thin\"><color rgb=\"FF808080\"/></left>\
         <right style=\"thin\"><color rgb=\"FF808080\"/></right>\
         <top style=\"thin\"><color rgb=\"FF808080\"/></top>\
         <bottom style=\"thin\"><color rgb=\"FF808080\"/></bottom><diagonal/></border></borders>\
         {}{}{}{}{}</styleSheet>",
        fills, fonts, xfs, cell_style_xfs, cell_styles
    )
}

/// 结构化内容 → XLSX：识别到的表格按行列落成单元格（保留底色与列宽），
/// 非表格的普通行按行写在最左侧一列。
pub fn tables_to_xlsx(
    items: &[DocItem],
    sheet_name: &str,
    dst: &Path,
    cancelled: &dyn Fn() -> bool,
) -> Result<(), String> {
    // 先找表头用的底色（PDF 里表头是一条绿色填充矩形）
    let mut header_fill: Option<[u8; 3]> = None;
    for item in items {
        if let DocItem::Table(t) = item {
            for c in t.rows.first().map(|r| r.as_slice()).unwrap_or(&[]) {
                if let Some(f) = c.fill {
                    header_fill = Some(f);
                    break;
                }
            }
        }
        if header_fill.is_some() {
            break;
        }
    }
    let styles = xlsx_styles(header_fill.unwrap_or([0xA9, 0xC4, 0x7F]));

    let mut sheet = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
         <worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"><cols>",
    );
    // 列宽按各列最长内容估算（中文按两个字符宽算），别一列宽窄一致把备注挤爆
    let max_cols = items
        .iter()
        .filter_map(|it| match it {
            DocItem::Table(t) => Some(t.rows.iter().map(|r| r.len()).max().unwrap_or(0)),
            _ => None,
        })
        .max()
        .unwrap_or(1)
        .max(1);
    let mut col_chars = vec![8usize; max_cols];
    for item in items {
        match item {
            DocItem::Para(line) => {
                let wide = line.text.chars().map(|c| if (c as u32) >= 0x2E80 { 2 } else { 1 }).sum::<usize>();
                col_chars[0] = col_chars[0].max(wide.min(40));
            }
            DocItem::Table(t) => {
                for row in t.rows.iter() {
                    for (j, cell) in row.iter().enumerate().take(max_cols) {
                        let wide: usize = cell
                            .text
                            .chars()
                            .map(|c| if (c as u32) >= 0x2E80 { 2 } else { 1 })
                            .sum();
                        col_chars[j] = col_chars[j].max(wide.min(50));
                    }
                }
            }
            DocItem::Image(_) => {}
        }
    }
    for i in 0..max_cols {
        // 有 PDF 框线的真实列宽就用它换算（1 个字符约 5.4pt），否则用内容估算值
        let real = items
            .iter()
            .filter_map(|it| match it {
                DocItem::Table(t) => Some(t),
                _ => None,
            })
            .filter_map(|t| t.widths.get(i).copied())
            .fold(0f32, f32::max);
        let width = if real > 0.0 {
            (real as f64 / 5.4).clamp(8.0, 60.0)
        } else {
            (col_chars[i] as f64 * 1.1).clamp(10.0, 55.0)
        };
        sheet.push_str(&format!(
            "<col min=\"{}\" max=\"{}\" width=\"{}\" customWidth=\"1\"/>",
            i + 1,
            i + 1,
            width
        ));
    }
    sheet.push_str("</cols><sheetData>");

    let mut row_no = 0usize;
    for item in items {
        if cancelled() {
            return Err("已取消".to_string());
        }
        match item {
            DocItem::Para(line) => {
                let text = line.text.trim();
                if text.is_empty() {
                    continue;
                }
                // 标题这类单独一行，占据 A 列并按普通格式落字
                sheet.push_str(&format!(
                    "<row r=\"{}\"><c r=\"A{}\" t=\"inlineStr\"><is><t xml:space=\"preserve\">{}</t></is></c></row>",
                    row_no + 1,
                    row_no + 1,
                    escape_xml(text)
                ));
                row_no += 1;
            }
            DocItem::Table(t) => {
                for (i, row) in t.rows.iter().enumerate() {
                    let is_header = i == 0 && row.iter().any(|c| c.fill.is_some());
                    sheet.push_str(&format!("<row r=\"{}\">", row_no + 1));
                    for (j, cell) in row.iter().enumerate() {
                        if cell.text.trim().is_empty() {
                            continue;
                        }
                        let reference = format!("{}{}", column_name(j), row_no + 1);
                        // 样式 1 = 表头（有底色），样式 2 = 有边框的普通格
                        let style = if is_header { 1 } else { 2 };
                        if let Ok(number) = cell.text.trim().parse::<f64>() {
                            sheet.push_str(&format!(
                                "<c r=\"{}\" s=\"{}\"><v>{}</v></c>",
                                reference, style, number
                            ));
                        } else {
                            // Excel 单元格里的换行需要额外开自动换行，这里压成空格更稳妥
                            sheet.push_str(&format!(
                                "<c r=\"{}\" s=\"{}\" t=\"inlineStr\"><is><t xml:space=\"preserve\">{}</t></is></c>",
                                reference,
                                style,
                                escape_xml(&cell.text.trim().replace('\n', " "))
                            ));
                        }
                    }
                    sheet.push_str("</row>");
                    row_no += 1;
                }
                // 表与表之间空一行，避免在 Excel 里糊成一坨
                row_no += 1;
            }
            DocItem::Image(_) => {}
        }
    }
    sheet.push_str("</sheetData>");
    sheet.push_str("</worksheet>");

    let content_types = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
         <Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
         <Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
         <Default Extension=\"xml\" ContentType=\"application/xml\"/>\
         <Override PartName=\"/xl/workbook.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml\"/>\
         <Override PartName=\"/xl/worksheets/sheet1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml\"/>\
         <Override PartName=\"/xl/styles.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml\"/>\
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
         <Relationship Id=\"rId2\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles\" Target=\"styles.xml\"/>\
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
                data: workbook.into_bytes(),
            },
            OutEntry {
                name: "xl/_rels/workbook.xml.rels".to_string(),
                data: workbook_rels.as_bytes().to_vec(),
            },
            OutEntry {
                name: "xl/styles.xml".to_string(),
                data: styles.into_bytes(),
            },
            OutEntry {
                name: "xl/worksheets/sheet1.xml".to_string(),
                data: sheet.into_bytes(),
            },
        ],
        dst,
    )
}
