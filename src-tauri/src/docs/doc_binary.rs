// 旧版 Word 97-2003（.doc）：OLE 复合文档（CFB）容器取字。
//
// .doc 不是 XML，正文在 WordDocument 流的“文本流”里，靠 0Table/1Table 里的
// CLX 分片表（PlcPcd）定位：每个分片给出字符区间与文件偏移，并标记该段是
// 单字节（cp1252）还是 UTF-16LE。这里只取正文，不带样式；表格用制表符/换行恢复。

use std::io::Read;
use std::path::Path;

use super::text_ops::Block;

/// Word 的“压缩”分片按 cp1252 解释，0x80-0x9F 不是 Latin-1 原值
fn cp1252_char(b: u8) -> char {
    match b {
        0x80 => '€',
        0x82 => '‚',
        0x83 => 'ƒ',
        0x84 => '„',
        0x85 => '…',
        0x86 => '†',
        0x87 => '‡',
        0x88 => 'ˆ',
        0x89 => '‰',
        0x8A => 'Š',
        0x8B => '‹',
        0x8C => 'Œ',
        0x8E => 'Ž',
        0x91 => '‘',
        0x92 => '’',
        0x93 => '“',
        0x94 => '”',
        0x95 => '•',
        0x96 => '–',
        0x97 => '—',
        0x98 => '˜',
        0x99 => '™',
        0x9A => 'š',
        0x9B => '›',
        0x9C => 'œ',
        0x9E => 'ž',
        0x9F => 'Ÿ',
        _ => b as char,
    }
}

fn le_u16(data: &[u8], at: usize) -> Result<u16, String> {
    data.get(at..at + 2)
        .map(|s| u16::from_le_bytes([s[0], s[1]]))
        .ok_or_else(|| "文件被截断（读取 16 位字段越界）".to_string())
}

fn le_u32(data: &[u8], at: usize) -> Result<u32, String> {
    data.get(at..at + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
        .ok_or_else(|| "文件被截断（读取 32 位字段越界）".to_string())
}

/// 从 0Table/1Table 的 CLX 里拆出所有文本分片，拼成原始字符流
fn raw_text(word_doc: &[u8], table: &[u8]) -> Result<String, String> {
    let fc_clx = le_u32(word_doc, 0x01A2)? as usize;
    let lcb_clx = le_u32(word_doc, 0x01A6)? as usize;
    if lcb_clx == 0 || fc_clx + lcb_clx > table.len() {
        return Err("找不到正文索引（CLX 为空或越界），请用 Word/WPS 另存为 .docx".to_string());
    }
    let clx = &table[fc_clx..fc_clx + lcb_clx];

    // CLX = 若干 Prc(0x01) + 一个 Pcdt(0x02)
    let mut pos = 0usize;
    let mut plc: Option<&[u8]> = None;
    while pos < clx.len() {
        match clx[pos] {
            0x01 => {
                let cb = le_u16(clx, pos + 1)? as usize;
                pos += 3 + cb;
            }
            0x02 => {
                let lcb = le_u32(clx, pos + 1)? as usize;
                let start = pos + 5;
                if start + lcb > clx.len() {
                    return Err("正文索引长度异常".to_string());
                }
                plc = Some(&clx[start..start + lcb]);
                break;
            }
            other => return Err(format!("正文索引类型异常（0x{:02X}）", other)),
        }
    }
    let plc = plc.ok_or_else(|| "找不到正文分片表（Pcdt）".to_string())?;

    // PlcPcd = (n+1) 个 CP + n 个 8 字节 PCD
    if plc.len() < 4 || (plc.len() - 4) % 12 != 0 {
        return Err("正文分片表长度异常".to_string());
    }
    let n = (plc.len() - 4) / 12;
    let mut out = String::new();
    for i in 0..n {
        let cp_start = le_u32(plc, 4 * i)? as usize;
        let cp_end = le_u32(plc, 4 * (i + 1))? as usize;
        if cp_end <= cp_start {
            continue;
        }
        let count = cp_end - cp_start;
        let pcd_at = 4 * (n + 1) + 8 * i;
        let fc_raw = le_u32(plc, pcd_at + 2)?;
        if fc_raw & 0x4000_0000 != 0 {
            // 单字节分片：偏移是“字符偏移 / 2”
            let off = (fc_raw & 0x3FFF_FFFF) as usize / 2;
            let end = off.saturating_add(count);
            if end > word_doc.len() {
                return Err("正文字符流越界".to_string());
            }
            out.extend(word_doc[off..end].iter().map(|b| cp1252_char(*b)));
        } else {
            let off = (fc_raw & 0x3FFF_FFFF) as usize;
            let end = off.saturating_add(count * 2);
            if end > word_doc.len() {
                return Err("正文字符流越界".to_string());
            }
            let units: Vec<u16> = word_doc[off..end]
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect();
            out.push_str(&String::from_utf16_lossy(&units));
        }
    }
    Ok(out)
}

/// 控制字符 → 纯文本：段落/换行变 \n，单元格 \t，行尾 \t\t 变 \n，
/// 域代码（PAGE / NUMPAGES 等）只保留域结果。
fn sanitize(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut fields: Vec<bool> = Vec::new(); // false=域指令, true=域结果
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\u{13}' => {
                fields.push(false);
                continue;
            }
            '\u{14}' => {
                if let Some(last) = fields.last_mut() {
                    *last = true;
                }
                continue;
            }
            '\u{15}' => {
                fields.pop();
                continue;
            }
            _ => {}
        }
        if fields.iter().any(|in_result| !*in_result) {
            continue; // 域指令内容不输出
        }
        match c {
            '\r' | '\u{0B}' | '\u{0C}' | '\u{0E}' => out.push('\n'),
            '\u{07}' => {
                // 单元格结束是单个 0x07，行结束是连续两个
                if chars.peek() == Some(&'\u{07}') {
                    chars.next();
                    out.push('\n');
                } else {
                    out.push('\t');
                }
            }
            '\u{A0}' => out.push(' '),
            '\u{1E}' => out.push('-'),
            '\t' | '\n' => out.push(c),
            c if (c as u32) < 0x20 => {}
            c => out.push(c),
        }
    }
    out
}

/// 纯文本 → 结构块；连续带制表符的行合成表格
fn text_to_blocks(text: &str) -> Vec<Block> {
    let mut blocks: Vec<Block> = Vec::new();
    let mut table: Vec<Vec<String>> = Vec::new();

    let flush = |table: &mut Vec<Vec<String>>, blocks: &mut Vec<Block>| {
        if !table.is_empty() {
            blocks.push(Block::Table(std::mem::take(table)));
        }
    };

    for line in text.split('\n') {
        let line = line.trim_end();
        if line.contains('\t') {
            let cells: Vec<String> = line.split('\t').map(|c| c.trim().to_string()).collect();
            if cells.iter().any(|c| !c.is_empty()) {
                table.push(cells);
                continue;
            }
        }
        flush(&mut table, &mut blocks);
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        blocks.push(Block::Para(trimmed.to_string()));
    }
    flush(&mut table, &mut blocks);
    blocks
}

fn open_stream(name: &str, src: &Path) -> Result<Vec<u8>, String> {
    let mut ole = cfb::open(src).map_err(|e| format!("不是有效的 .doc 文件: {}", e))?;
    if !ole.exists(name) {
        return Err(format!("缺少 {} 流", name));
    }
    let mut stream = ole
        .open_stream(name)
        .map_err(|e| format!("读取 {} 流失败: {}", name, e))?;
    let mut buf = Vec::new();
    stream
        .read_to_end(&mut buf)
        .map_err(|e| format!("读取 {} 流失败: {}", name, e))?;
    Ok(buf)
}

fn blocks_from_doc(src: &Path) -> Result<Vec<Block>, String> {
    let word_doc = open_stream("WordDocument", src)?;
    if word_doc.len() < 0x01AA {
        return Err("WordDocument 流过短，不是有效的 .doc".to_string());
    }
    let ident = le_u16(&word_doc, 0)?;
    if ident != 0xA5EC {
        return Err("不是 Word 97-2003 文档（FIB 标识不匹配）".to_string());
    }
    let n_fib = le_u16(&word_doc, 2)?;
    if n_fib < 0x00C1 {
        return Err("这是 Word 6.0/95 旧格式，请用 Word/WPS 另存为 .docx".to_string());
    }
    let flags = le_u16(&word_doc, 0x0A)?;
    let preferred = if flags & 0x0200 != 0 { "1Table" } else { "0Table" };
    let fallback = if preferred == "1Table" { "0Table" } else { "1Table" };
    let table = open_stream(preferred, src).or_else(|_| open_stream(fallback, src))?;

    let raw = raw_text(&word_doc, &table)?;
    let text = sanitize(&raw);
    let blocks = text_to_blocks(&text);
    if blocks.is_empty() {
        return Err("没有抽取到正文（可能是纯图片扫描件）".to_string());
    }
    Ok(blocks)
}

/// 按内容判别：有的 .doc 其实是 RTF 或改了扩展名的 docx
pub fn doc_blocks(src: &Path) -> Result<Vec<Block>, String> {
    let mut head = [0u8; 8];
    {
        use std::io::Read as _;
        let mut f = std::fs::File::open(src).map_err(|e| format!("打开失败: {}", e))?;
        let n = f.read(&mut head).map_err(|e| format!("读取失败: {}", e))?;
        if n == 0 {
            return Err("文件为空".to_string());
        }
    }
    if head.starts_with(b"{\\rtf") {
        return super::legacy::blocks_for(src, "rtf");
    }
    if head.starts_with(b"PK\x03\x04") {
        // 实际是 docx，只是扩展名写成 .doc
        return super::text_ops::docx_items(src);
    }
    blocks_from_doc(src)
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
    let blocks = doc_blocks(src)?;
    super::legacy::convert_blocks(src, &blocks, dst, target, cancelled)
}
