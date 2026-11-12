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
    /// 结构化结果：普通文本行与还原出来的表格，按页面从上到下的顺序
    pub items: Vec<PageItem>,
    /// 页眉（/Artifact Header）里的内容，含图片，供 Word 页眉使用
    pub header: Option<HeaderBlock>,
    /// 页脚：所有页面底部都相同的那段内容（地址、电话、页码）
    pub footer: Option<FooterBlock>,
}

/// 页眉/页脚里的一段文字。带上横向起点，Word 里才能按原 PDF 的分栏位置重排
pub struct HeaderLine {
    pub text: String,
    pub x: f32,
    pub size: f32,
    pub color: Option<[u8; 3]>,
}

/// 页眉/页脚内容：按行组织，每行可能是并排的几段（两家公司抬头这种）
#[derive(Default)]
pub struct BandBlock {
    pub rows: Vec<Vec<HeaderLine>>,
    pub images: Vec<HeaderImage>,
}

pub type HeaderBlock = BandBlock;
pub type FooterBlock = BandBlock;

pub struct HeaderImage {
    pub png: Vec<u8>,
    /// 在页面上的显示尺寸（PDF 单位，pt）
    pub width: f32,
    pub height: f32,
    /// 在页面上的横向起点，用于页眉里多张 logo 的相对排布
    pub x: f32,
    /// 在页面上的纵向位置，正文图片按它排版
    pub y: f32,
}

/// 正文里的一张图片（海报、插图、扫描页），转 Word 时按原位置排进去
#[derive(Clone)]
pub struct BodyImage {
    pub png: Vec<u8>,
    /// 显示尺寸（PDF 单位，pt）
    pub width: f32,
    pub height: f32,
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
    // 很多 PDF 把空格映射成不换行空格/零宽字符，统一成普通空格，否则文本比较和搜索都会失效。
    // 图标字体（Wingdings 等）会解出 Unicode 私有区码位，落到 docx 里是一个个方块，直接去掉。
    let cleaned: String = text
        .chars()
        .filter(|c| !('\u{e000}'..='\u{f8ff}').contains(c))
        .collect();
    cleaned
        .replace('\u{a0}', " ")
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
            } else if (0x20..0x7F).contains(&code) {
                // 单字节西文字体常常缺少 ToUnicode 表（或只覆盖了一部分码位），
                // 直接丢字符会让数字、英文整段消失，按 ASCII 回退更接近阅读结果。
                out.push(code as u8 as char);
            }
        }
    }
    normalize(&out)
}

/// 一段带坐标的文字。PDF 里经常一个词一个 BT…ET 绝对定位块，
/// 单靠「换行操作符」切行会把一个词切成一行，所以这里先收集坐标再按位置聚类成行。
#[derive(Clone)]
struct Frag {
    x: f32,
    y: f32,
    size: f32,
    text: String,
    /// 文字颜色（RGB）。纯黑不记，省得每个字都带样式
    color: Option<[u8; 3]>,
}

/// 内容流里的矩形（在页面坐标下），用来识别表格框线和单元格底纹
#[derive(Clone)]
struct Rect {
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    /// 填充色；描边矩形拿不到填充色时记黑
    fill: [u8; 3],
}

fn m_mul(m: [f32; 6], n: [f32; 6]) -> [f32; 6] {
    [
        m[0] * n[0] + m[1] * n[2],
        m[0] * n[1] + m[1] * n[3],
        m[2] * n[0] + m[3] * n[2],
        m[2] * n[1] + m[3] * n[3],
        m[4] * n[0] + m[5] * n[2] + n[4],
        m[4] * n[1] + m[5] * n[3] + n[5],
    ]
}

fn m_apply(m: [f32; 6], x: f32, y: f32) -> (f32, f32) {
    (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5])
}

/// 估算一段文字的排版宽度（CJK 按 1em、西文按 0.55em），只用于列拼接时判断要不要补空格
fn est_width(text: &str, size: f32) -> f32 {
    text.chars()
        .map(|c| {
            let em = if c.is_whitespace() {
                0.4
            } else if (c as u32) >= 0x2E80 {
                1.0
            } else {
                0.55
            };
            em * size
        })
        .sum()
}

/// 把碎片按「同一视觉行」聚类：y 接近的算一行，行内按 x 排序拼接；返回值带上行的 y
fn frags_to_lines(mut frags: Vec<Frag>) -> Vec<(f32, LineData)> {
    frags.retain(|f| !f.text.trim().is_empty());
    if frags.is_empty() {
        return Vec::new();
    }
    // y 从大到小（PDF 坐标 y 向上），同一行再按 x
    frags.sort_by(|a, b| {
        b.y.partial_cmp(&a.y)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.x.partial_cmp(&b.x).unwrap_or(std::cmp::Ordering::Equal))
    });

    let mut lines: Vec<(f32, LineData)> = Vec::new();
    let mut row: Vec<Frag> = Vec::new();
    let mut row_y = f32::NAN;

    let flush = |row: &mut Vec<Frag>, y: f32, lines: &mut Vec<(f32, LineData)>| {
        if row.is_empty() {
            return;
        }
        row.sort_by(|a, b| a.x.partial_cmp(&b.x).unwrap_or(std::cmp::Ordering::Equal));
        let mut text = String::new();
        let mut end_x = f32::NAN;
        let x0 = row[0].x;
        let mut widest_gap = 0.0f32;
        let mut split_at: Option<usize> = None;
        for frag in row.iter() {
            let gap = frag.x - end_x;
            // 同一行内两段之间留了明显空白，补一个空格，避免「名称散客票」糊在一起
            if !text.is_empty() && gap > frag.size.max(4.0) * 0.6 {
                text.push(' ');
            }
            // 跨栏级的大空白（并列抬头那种）额外记下切分点
            if !text.is_empty() && gap > widest_gap && gap > 60.0 {
                widest_gap = gap;
                split_at = Some(text.chars().count());
            }
            text.push_str(frag.text.trim_end());
            end_x = frag.x + est_width(&frag.text, frag.size);
        }
        let trimmed = text.trim();
        let lead = text.chars().count() - text.trim_start().chars().count();
        if !trimmed.is_empty() {
            lines.push((
                y,
                LineData {
                    text: trimmed.to_string(),
                    color: frag_color(row),
                    size: frag_size(row),
                    y,
                    x0,
                    x1: end_x,
                    split_at: split_at.map(|s| s.saturating_sub(lead)),
                },
            ));
        }
        row.clear();
    };

    for frag in frags {
        let tol = (frag.size * 0.35).max(1.6);
        if row.is_empty() || (frag.y - row_y).abs() <= tol {
            if row.is_empty() {
                row_y = frag.y;
            }
            row.push(frag);
        } else {
            flush(&mut row, row_y, &mut lines);
            row_y = frag.y;
            row.push(frag);
        }
    }
    flush(&mut row, row_y, &mut lines);
    lines
}

fn collect_cell_text(frags: &[Frag]) -> String {
    let mut sorted: Vec<&Frag> = frags.iter().collect();
    sorted.sort_by(|a, b| {
        b.y.partial_cmp(&a.y)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.x.partial_cmp(&b.x).unwrap_or(std::cmp::Ordering::Equal))
    });
    let mut text = String::new();
    let mut last_y = f32::NAN;
    let mut end_x = f32::NAN;
    for frag in sorted {
        if !text.is_empty() {
            if !last_y.is_nan() && (last_y - frag.y).abs() > frag.size.max(4.0) * 0.8 {
                // 单元格里的多行文字要保留换行，别和成一句
                text.push('\n');
            } else if frag.x - end_x > frag.size.max(4.0) * 0.6 {
                text.push(' ');
            }
        }
        text.push_str(frag.text.trim());
        last_y = frag.y;
        end_x = frag.x + est_width(&frag.text, frag.size);
    }
    text.trim().to_string()
}

/// 一行普通文本（带颜色与字号，供 Word/Excel 还原样式）
#[derive(Clone)]
pub struct LineData {
    pub text: String,
    pub color: Option<[u8; 3]>,
    pub size: f32,
    /// 这一行的基线 y（PDF 坐标，向上增大），用来还原行间距
    pub y: f32,
    /// 这一行在 PDF 里的左边界（pt），用于还原居中、缩进
    pub x0: f32,
    /// 这一行的右边界（pt）
    pub x1: f32,
    /// 行内有一段明显跨栏的空白时，记下后半段起点在 text 里的字符位置，
    /// 写 Word 时在那里插制表符，把右半段推到版面右侧（TO / FM 这种并排抬头）
    pub split_at: Option<usize>,
}

/// 表格里的一个单元格
#[derive(Clone)]
pub struct Cell {
    pub text: String,
    /// 底纹色（表头绿底这类）
    pub fill: Option<[u8; 3]>,
    /// 文字颜色
    pub color: Option<[u8; 3]>,
    pub size: f32,
    /// 这一格横向跨了几列（PDF 里一句长文字跨过列线时，它其实是合并格）
    pub span: usize,
}

/// 还原出来的一张表
#[derive(Clone)]
pub struct TableData {
    pub rows: Vec<Vec<Cell>>,
    /// 每列的原始宽度（PDF 点），用来在 Word/Excel 里还原真实列宽
    pub widths: Vec<f32>,
}

/// 一页的结构化结果：普通行 / 表格 / 图片
pub enum PageItem {
    Line(LineData),
    Table(TableData),
    Image(BodyImage),
}

fn is_black(c: [u8; 3]) -> bool {
    c[0] < 40 && c[1] < 40 && c[2] < 40
}

fn clamp_byte(v: f32) -> u8 {
    (v * 255.0).round().clamp(0.0, 255.0) as u8
}

fn is_near_white(c: [u8; 3]) -> bool {
    c[0] > 230 && c[1] > 230 && c[2] > 230
}

/// 取覆盖整个单元格、面积最小的填充矩形颜色，作为该格的底纹
fn cell_fill(rects: &[Rect], x0: f32, x1: f32, y0: f32, y1: f32) -> Option<[u8; 3]> {
    let mut best: Option<(f32, [u8; 3])> = None;
    for r in rects {
        let rw = r.x1 - r.x0;
        let rh = r.y1 - r.y0;
        // 框线这类细长矩形不是底纹
        if rw < 8.0 || rh < 8.0 {
            continue;
        }
        if r.x0 > x0 + 2.5 || r.x1 < x1 - 2.5 || r.y0 > y0 + 2.5 || r.y1 < y1 - 2.5 {
            continue;
        }
        // 表头的整行底色会横跨多行高度，只认和本格尺寸接近的矩形，
        // 否则相邻数据行也会被染上表头底色
        let (cw, ch) = ((x1 - x0).max(1.0), (y1 - y0).max(1.0));
        if rw > cw * 1.6 || rh > ch * 1.6 {
            continue;
        }
        let area = rw * rh;
        if best.map(|(a, _)| area < a).unwrap_or(true) {
            best = Some((area, r.fill));
        }
    }
    best.map(|(_, c)| c)
        .filter(|c| !is_black(*c) && !is_near_white(*c))
}

fn frag_color(frags: &[Frag]) -> Option<[u8; 3]> {
    frags.iter().rev().find_map(|f| f.color)
}

fn frag_size(frags: &[Frag]) -> f32 {
    frags.iter().map(|f| f.size).fold(0.0f32, f32::max)
}

/// 用表格框线（PDF 里是细长的填充矩形）还原网格，把文字按单元格归并。
/// 没有框线的页面退化为纯文本行。
fn build_items(
    frags: Vec<Frag>,
    rects: Vec<Rect>,
    pics: Vec<(f32, BodyImage)>,
) -> (Vec<PageItem>, Vec<String>) {
    let mut horizons: Vec<(f32, f32, f32)> = rects
        .iter()
        .filter(|r| (r.y1 - r.y0) <= 2.5 && (r.x1 - r.x0) >= 24.0)
        .map(|r| (r.y0, r.x0, r.x1))
        .collect();

    if horizons.is_empty() {
        // 没有格线：文字行与图片按纵向位置混排（海报页就是「图 + 少量文字」）
        let mut mixed: Vec<(f32, PageItem)> = frags_to_lines(frags)
            .into_iter()
            .map(|(y, d)| (y, PageItem::Line(d)))
            .collect();
        mixed.extend(pics.into_iter().map(|(y, b)| (y, PageItem::Image(b))));
        mixed.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        let all: Vec<PageItem> = mixed.into_iter().map(|(_, it)| it).collect();
        let texts: Vec<String> = all
            .iter()
            .filter_map(|it| match it {
                PageItem::Line(d) => Some(d.text.clone()),
                _ => None,
            })
            .collect();
        return (all, texts);
    }

    // 同一条线可能被画成多段，先按 y 合并成「行边界」
    horizons.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    let mut edges: Vec<(f32, f32, f32)> = Vec::new();
    for (y, x0, x1) in horizons {
        if let Some(last) = edges.last_mut() {
            // 表格外框和单元格裁剪框常常只差两三个点，同一条线要合并，否则整张表被切碎
            if (last.0 - y).abs() <= 3.0 {
                last.1 = last.1.min(x0);
                last.2 = last.2.max(x1);
                continue;
            }
        }
        edges.push((y, x0, x1));
    }

    // 相邻行边界若横向范围一致，归为同一张表
    struct Grid {
        ys: Vec<f32>,
        x0: f32,
        x1: f32,
    }
    let mut grids: Vec<Grid> = Vec::new();
    for (y, x0, x1) in edges {
        let mut joined = false;
        if let Some(g) = grids.last_mut() {
            let overlap = (g.x1.min(x1) - g.x0.max(x0)).max(0.0);
            let width = (g.x1 - g.x0).max(1.0);
            let gap = (g.ys.last().copied().unwrap_or(y) - y).abs();
            // 行距要和上一行接近才算同一张表，否则说明是另一张表（或页眉分隔线）
            let last_gap = if g.ys.len() >= 2 {
                (g.ys[g.ys.len() - 2] - g.ys[g.ys.len() - 1]).abs()
            } else {
                gap
            };
            // 单元格可能很高（多行备注），行距差异拉得很大，不能用上一行间距卡死
            if overlap / width > 0.5 && gap <= (last_gap * 8.0).max(70.0) && gap < 260.0 {
                g.ys.push(y);
                g.x0 = g.x0.min(x0);
                g.x1 = g.x1.max(x1);
                joined = true;
            }
        }
        if !joined {
            grids.push(Grid { ys: vec![y], x0, x1 });
        }
    }

    let mut items: Vec<(f32, PageItem)> = Vec::new();
    let mut used: Vec<bool> = vec![false; frags.len()];

    for grid in grids.iter() {
        // 至少两条横线才构成一行表格
        if grid.ys.len() < 2 {
            continue;
        }
        // 上边框如果比常规行距高出一截，得看多出来的那一行到底是什么：
        // 是表头就留着，是章节标题/空白就把它切出去，别把标题当表格第一行。
        let mut ys = grid.ys.clone();
        if ys.len() >= 3 {
            let mut gaps: Vec<f32> = ys.windows(2).map(|w| w[0] - w[1]).collect();
            gaps.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let median = gaps[gaps.len() / 2];
            if median > 1.0 && ys[0] - ys[1] > median * 1.6 {
                let top = ys[0];
                let second = ys[1];
                let mut sizes: Vec<f32> = Vec::new();
                let mut count = 0usize;
                for frag in frags.iter() {
                    if frag.y > second + 1.0
                        && frag.y < top + 1.0
                        && frag.x >= grid.x0 - 1.5
                        && frag.x <= grid.x1 + 1.5
                    {
                        count += 1;
                        sizes.push(frag.size);
                    }
                }
                let mut all: Vec<f32> = frags
                    .iter()
                    .filter(|f| f.y <= top + 1.0 && f.y >= ys.last().copied().unwrap_or(0.0) - 1.0)
                    .map(|f| f.size)
                    .collect();
                all.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                let body_size = all.get(all.len() / 2).copied().unwrap_or(0.0);
                let row_size = sizes.iter().copied().fold(0.0f32, f32::max);
                // 字明显比正文大 = 标题；几乎没有文字 = 空白带。两种情况都不算表格行
                if count < 2 || (body_size > 1.0 && row_size > body_size * 1.3) {
                    ys.remove(0);
                }
            }
        }
        if ys.len() < 2 {
            continue;
        }
        // 同一份表每行的竖线位置是一致的；段与段之间列线会变（4 列变 6 列之类），
        // 在那里必须切开，否则文字会被塞进错的列
        let mid_cols: Vec<Vec<f32>> = ys
            .windows(2)
            .map(|w| {
                let m = (w[0] + w[1]) / 2.0;
                let mut v: Vec<f32> = rects
                    .iter()
                    .filter(|r| (r.x1 - r.x0) <= 2.5 && r.y1 >= m && r.y0 <= m)
                    .map(|r| (r.x0 * 2.0).round() / 2.0)
                    .collect();
                v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                v.dedup_by(|a, b| (*a - *b).abs() < 1.0);
                v
            })
            .collect();
        let mut chunks: Vec<Vec<f32>> = Vec::new();
        let mut start = 0usize;
        for i in 0..mid_cols.len().saturating_sub(1) {
            let (a, b) = (&mid_cols[i], &mid_cols[i + 1]);
            // 竖线集合出现空档（段标题行没有格线），或列结构明显变了，都说明换了一段表
            let sim = if a.is_empty() && b.is_empty() {
                1.0
            } else if a.is_empty() || b.is_empty() {
                0.0
            } else {
                let hit = a
                    .iter()
                    .filter(|x| b.iter().any(|y| (*x - *y).abs() <= 3.0))
                    .count();
                hit as f32 / a.len().max(b.len()) as f32
            };
            if sim < 0.5 {
                chunks.push(ys[start..=i].to_vec());
                start = i + 1;
            }
        }
        chunks.push(ys[start..].to_vec());

        for ys in chunks.into_iter() {
            if ys.len() < 2 {
                continue;
            }
        if std::env::var("DOCS_DEBUG").is_ok() {
            eprintln!(
                "[grid] ys={:?} x={:.1}..{:.1}",
                ys.iter().map(|y| (*y * 10.0).round() / 10.0).collect::<Vec<f32>>(),
                grid.x0,
                grid.x1
            );
        }

        // 收集落在表格范围内的文字
        let inside: Vec<&Frag> = frags
            .iter()
            .enumerate()
            .filter(|(idx, frag)| {
                !used[*idx]
                    && frag.x >= grid.x0 - 1.5
                    && frag.x <= grid.x1 + 1.5
                    && frag.y <= ys[0] + 3.0
                    && ys.last().map(|b| frag.y >= *b - 2.0).unwrap_or(true)
            })
            .map(|(_, frag)| frag)
            .collect();
        if inside.is_empty() {
            continue;
        }

        let row_count = ys.len() - 1;

        // 列起点优先用表格框线里的竖线：只取与这张表纵向区间重叠、且贯穿多行的竖线，
        // 单元格内部的零散竖线会被频次过滤掉。
        let mut vxs: Vec<f32> = rects
            .iter()
            .filter(|r| (r.x1 - r.x0) <= 2.5 && (r.y1 - r.y0) >= 6.0)
            .filter(|r| r.x0 >= grid.x0 - 2.0 && r.x0 <= grid.x1 + 2.0)
            .filter(|r| {
                r.y1 >= ys.last().copied().unwrap_or(-1e9) - 2.0 && r.y0 <= ys[0] + 2.0
            })
            .map(|r| r.x0)
            .collect();
        vxs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let mut groups: Vec<(f32, usize)> = Vec::new();
        for x in vxs {
            if let Some(last) = groups.last_mut() {
                if (x - last.0).abs() <= 3.0 {
                    last.1 += 1;
                    continue;
                }
            }
            groups.push((x, 1));
        }
        let need = (row_count / 3).max(1);
        let mut cols: Vec<f32> = groups
            .iter()
            .filter(|(_, count)| *count >= need)
            .map(|(x, _)| *x)
            .collect();
        // 表格外框和格线常常只差十来个点，会多切出一列空列；靠得这么近的取内侧那条
        let mut merged: Vec<f32> = Vec::new();
        for x in cols {
            if let Some(last) = merged.last_mut() {
                if (x - *last).abs() < 14.0 {
                    *last = x;
                    continue;
                }
            }
            merged.push(x);
        }
        cols = merged;
        let from_lines = cols.len() >= 2;
        if from_lines {
            // 最后一条竖线是右边框，不是新列的起点
            cols.pop();
        } else {
            // 没有竖线（或框线是描边画出来的）时，退回用文字起点，要求同一个 x 至少出现在两行
            let mut starts: Vec<f32> = Vec::new();
            let mut row_ys: Vec<f32> = Vec::new();
            for frag in inside.iter() {
                if !row_ys.iter().any(|g| (*g - frag.y).abs() <= frag.size.max(6.0) * 0.9) {
                    row_ys.push(frag.y);
                }
            }
            for frag in inside.iter() {
                let support = row_ys
                    .iter()
                    .filter(|ry| {
                        inside
                            .iter()
                            .any(|o| (o.y - **ry).abs() <= 6.0 && (o.x - frag.x).abs() <= 5.0)
                    })
                    .count();
                if support >= 2 {
                    starts.push(frag.x);
                }
            }
            starts.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            for x in starts {
                if let Some(last) = cols.last() {
                    if (x - *last).abs() <= 8.0 {
                        continue;
                    }
                }
                cols.push(x);
            }
        }
        if cols.len() < 2 {
            continue;
        }

        let col_count = cols.len();
        let mut cells: Vec<Vec<Vec<Frag>>> = vec![vec![Vec::new(); col_count]; row_count];
        // 一句横排长文字会跨过列线，逐字归列会把它切碎；
        // 同一行里紧挨着的字跟着第一个字走，整句留在起点所在的那一格
        let mut trail: Option<(usize, usize, f32, f32, f32)> = None; // row, col, 右端 x, 基线, 字号

        for (idx, frag) in frags.iter().enumerate() {
            if used[idx] {
                continue;
            }
            let cx = frag.x;
            let cy = frag.y;
            if cx < grid.x0 - 1.5 || cx > grid.x1 + 1.5 {
                continue;
            }
            let mut row_idx: Option<usize> = None;
            for i in 0..row_count {
                let top = ys[i];
                let bottom = ys[i + 1];
                if cy <= top + 3.0 && cy >= bottom - 1.0 {
                    row_idx = Some(i);
                    break;
                }
            }
            let Some(row_idx) = row_idx else { continue };
            let mut col_idx = 0usize;
            for (j, start) in cols.iter().enumerate() {
                if cx + 2.0 >= *start {
                    col_idx = j;
                } else {
                    break;
                }
            }
            let follow = match trail {
                Some((trow, _tcol, tend, tbase, tsize)) => {
                    trow == row_idx
                        && (cy - tbase).abs() <= tsize.max(6.0) * 0.6
                        && cx >= tend - tsize.max(6.0) * 0.8
                        && (cx - tend) <= tsize.max(6.0) * 0.35
                }
                None => false,
            };
            if follow {
                col_idx = trail.unwrap().1;
            }
            // 字宽按中英文分别估：中文一个字约一个字号，ASCII 大概半个
            let width: f32 = frag
                .text
                .chars()
                .map(|c| if (c as u32) >= 0x2E80 { frag.size } else { frag.size * 0.55 })
                .sum();
            used[idx] = true;
            cells[row_idx][col_idx].push(frag.clone());
            trail = Some((row_idx, col_idx, cx + width, cy, frag.size));
        }

        let mut rows: Vec<Vec<Cell>> = Vec::new();
        let mut occupied = 0usize;
        for (i, row_cells) in cells.iter().enumerate() {
            let mut row: Vec<Cell> = Vec::new();
            let (y_top, y_bottom) = (ys[i], ys[i + 1]);
            let filled: Vec<bool> = row_cells.iter().map(|c| !c.is_empty()).collect();
            // 只有一两行的表通常是「标签 + 一整段正文」，正文本来就是一格跨到底；
            // 多行表里则要保守，别把相邻独立单元格并进来
            let lenient = row_count <= 2;
            for (j, cell) in row_cells.iter().enumerate() {
                if !cell.is_empty() {
                    occupied += 1;
                }
                let (cx0, cx1) = (cols[j], cols.get(j + 1).copied().unwrap_or(grid.x1));
                // 这一格的文字右端越过几条列线，就是跨了几列（Word 里要合并）；
                // 只有右边那些格本来就是空的才吞，否则会把旁边独立的单元格并进来
                let mut span = 1usize;
                if !cell.is_empty() {
                    let right = cell
                        .iter()
                        .map(|f| {
                            let w: f32 = f
                                .text
                                .chars()
                                .map(|c| if (c as u32) >= 0x2E80 { f.size } else { f.size * 0.55 })
                                .sum();
                            f.x + w
                        })
                        .fold(f32::NEG_INFINITY, f32::max);
                    for (k, start) in cols.iter().enumerate().skip(j + 1) {
                        if right > *start + 2.0 && (lenient || !filled[k]) {
                            span = k - j + 1;
                        } else {
                            break;
                        }
                    }
                    span = span.min(col_count - j);
                }
                row.push(Cell {
                    text: collect_cell_text(cell),
                    fill: if cell.is_empty() {
                        None
                    } else {
                        cell_fill(&rects, cx0, cx1, y_bottom, y_top)
                    },
                    color: frag_color(cell),
                    size: frag_size(cell),
                    span,
                });
            }
            rows.push(row);
        }
        // 一个几乎全空的「表格」多半是误判，按普通文本处理
        if occupied < 2 {
            continue;
        }
        // 页眉分隔线之类会在顶部留出一整行空白，去掉
        while rows
            .first()
            .map(|r| r.iter().all(|c| c.text.is_empty()))
            .unwrap_or(false)
        {
            rows.remove(0);
        }
        while rows
            .last()
            .map(|r| r.iter().all(|c| c.text.is_empty()))
            .unwrap_or(false)
        {
            rows.pop();
        }
        if rows.is_empty() {
            continue;
        }
        // 列宽按 PDF 里竖线的实际间距算，Word/Excel 里才能还原原来的列比例
        let mut widths: Vec<f32> = Vec::new();
        for (j, start) in cols.iter().enumerate() {
            let end = cols.get(j + 1).copied().unwrap_or(grid.x1);
            widths.push((end - *start).max(1.0));
        }
        items.push((ys[0], PageItem::Table(TableData { rows, widths })));
        }
    }

    let free: Vec<Frag> = frags
        .into_iter()
        .enumerate()
        .filter(|(idx, _)| !used[*idx])
        .map(|(_, f)| f)
        .collect();
    for (y, data) in frags_to_lines(free) {
        items.push((y, PageItem::Line(data)));
    }
    items.extend(pics.into_iter().map(|(y, b)| (y, PageItem::Image(b))));

    items.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    let all: Vec<PageItem> = items.into_iter().map(|(_, it)| it).collect();
    let mut lines: Vec<String> = Vec::new();
    for item in all.iter() {
        match item {
            PageItem::Line(d) => lines.push(d.text.clone()),
            PageItem::Table(t) => {
                for row in t.rows.iter() {
                    let joined = row
                        .iter()
                        .map(|c| c.text.clone())
                        .filter(|c| !c.is_empty())
                        .collect::<Vec<String>>()
                        .join(" ");
                    if !joined.trim().is_empty() {
                        lines.push(joined);
                    }
                }
            }
            PageItem::Image(_) => {}
        }
    }
    (all, lines)
}

/// 粗略词法：把内容流切成操作数与算符，记录每段文字的坐标与颜色，最后按位置聚类成行
fn run_content(
    content: &[u8],
    fonts: &HashMap<Vec<u8>, FontMap>,
) -> (
    Vec<Frag>,
    Vec<Rect>,
    Vec<Rect>,
    Vec<(Vec<u8>, [f32; 6])>,
    bool,
) {
    let mut frags: Vec<Frag> = Vec::new();
    let mut rects: Vec<Rect> = Vec::new();
    let mut pending: Vec<Rect> = Vec::new();
    // 单元格裁剪框：不少 PDF（Office 另存、排版软件导出）用「每格一个裁剪框」代替画线，
    // 这些框的边就是单元格边界，比描边更好用
    let mut clips: Vec<Rect> = Vec::new();
    // 描边路径（m/l + S），Word 导出的 PDF 表格线通常是这种
    let mut path: Vec<(f32, f32)> = Vec::new();
    let mut header_images: Vec<(Vec<u8>, [f32; 6])> = Vec::new();
    let mut operands: Vec<Operand> = Vec::new();
    let mut active: Option<Vec<u8>> = None; // 当前字体名
    let mut ctm: [f32; 6] = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
    let mut fill: [u8; 3] = [0, 0, 0];
    let mut stroke: [u8; 3] = [0, 0, 0];
    let mut gstack: Vec<([f32; 6], [u8; 3], [u8; 3])> = Vec::new();
    let mut tm: [f32; 6] = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
    let mut tlm: [f32; 6] = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
    let mut size = 12.0f32;
    // 有些 PDF 整页套了 `1 0 0 -1 0 H cm`，此时 y 是「距顶」坐标，需要翻回来
    let mut flip_neg = 0usize;
    let mut flip_pos = 0usize;
    let mut page_flip = false;
    let mut i = 0usize;
    let len = content.len();

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
                let nums: Vec<f32> = operands
                    .iter()
                    .filter_map(|o| match o {
                        Operand::Num(v) => Some(*v),
                        _ => None,
                    })
                    .collect();
                match op.as_str() {
                    "q" => gstack.push((ctm, fill, stroke)),
                    "Q" => {
                        if let Some((m, f, s)) = gstack.pop() {
                            ctm = m;
                            fill = f;
                            stroke = s;
                        }
                    }
                    "RG" => {
                        if nums.len() >= 3 {
                            let n = nums.len();
                            stroke = [
                                clamp_byte(nums[n - 3]),
                                clamp_byte(nums[n - 2]),
                                clamp_byte(nums[n - 1]),
                            ];
                        }
                    }
                    "G" => {
                        if let Some(v) = nums.last() {
                            let gray = clamp_byte(*v);
                            stroke = [gray, gray, gray];
                        }
                    }
                    "rg" => {
                        if nums.len() >= 3 {
                            let n = nums.len();
                            fill = [
                                clamp_byte(nums[n - 3]),
                                clamp_byte(nums[n - 2]),
                                clamp_byte(nums[n - 1]),
                            ];
                        }
                    }
                    "g" => {
                        if let Some(v) = nums.last() {
                            let gray = clamp_byte(*v);
                            fill = [gray, gray, gray];
                        }
                    }
                    "k" => {
                        if nums.len() >= 4 {
                            let n = nums.len();
                            let (c, m, y, k) = (
                                nums[n - 4].clamp(0.0, 1.0),
                                nums[n - 3].clamp(0.0, 1.0),
                                nums[n - 2].clamp(0.0, 1.0),
                                nums[n - 1].clamp(0.0, 1.0),
                            );
                            fill = [
                                clamp_byte((1.0 - c) * (1.0 - k)),
                                clamp_byte((1.0 - m) * (1.0 - k)),
                                clamp_byte((1.0 - y) * (1.0 - k)),
                            ];
                        }
                    }
                    "Do" => {
                        // 页眉里的 logo 是图片 XObject，记下绘制矩阵好从资源里取出原图
                        for operand in operands.iter() {
                            if let Operand::Name(name) = operand {
                                header_images.push((name.clone(), ctm));
                            }
                        }
                    }
                    "BDC" => {}
                    "BMC" => {}
                    "EMC" => {}
                    "cm" => {
                        if nums.len() >= 6 {
                            let m = [
                                nums[nums.len() - 6],
                                nums[nums.len() - 5],
                                nums[nums.len() - 4],
                                nums[nums.len() - 3],
                                nums[nums.len() - 2],
                                nums[nums.len() - 1],
                            ];
                            // 页面首条 `1 0 0 -1 0 H cm` 说明整页 y 轴朝下，坐标是「距顶」的
                            if !page_flip
                                && ctm[3] > 0.0
                                && m[0] > 0.0
                                && m[3] < 0.0
                                && m[5] > 100.0
                            {
                                page_flip = true;
                            }
                            ctm = m_mul(m, ctm);
                        }
                    }
                    "re" => {
                        // 只有真正被填充的矩形才是底纹/框线；`re W* n` 只是裁剪框，
                        // 每个文字片段都带一套，绝不能当成表格底纹
                        if nums.len() >= 4 {
                            let n = nums.len();
                            let (rx, ry, rw, rh) =
                                (nums[n - 4], nums[n - 3], nums[n - 2], nums[n - 1]);
                            let (ax, ay) = m_apply(ctm, rx, ry);
                            let (bx, by) = m_apply(ctm, rx + rw, ry + rh);
                            let (x0, x1) = if ax <= bx { (ax, bx) } else { (bx, ax) };
                            let (y0, y1) = if ay <= by { (ay, by) } else { (by, ay) };
                            pending.push(Rect { x0, y0, x1, y1, fill });
                        }
                    }
                    "f" | "f*" | "F" => {
                        for r in pending.iter() {
                            rects.push(Rect {
                                x0: r.x0,
                                y0: r.y0,
                                x1: r.x1,
                                y1: r.y1,
                                fill: r.fill,
                            });
                        }
                        pending.clear();
                    }
                    "m" => {
                        path.clear();
                        if nums.len() >= 2 {
                            let (x, y) = (nums[nums.len() - 2], nums[nums.len() - 1]);
                            path.push(m_apply(ctm, x, y));
                        }
                    }
                    "l" => {
                        if nums.len() >= 2 && !path.is_empty() {
                            let (x, y) = (nums[nums.len() - 2], nums[nums.len() - 1]);
                            path.push(m_apply(ctm, x, y));
                        }
                    }
                    "W" | "W*" => {
                        // 裁剪框的边就是单元格边界，交给表格识别用
                        for r in pending.drain(..) {
                            clips.push(r);
                        }
                    }
                    "n" => pending.clear(),
                    "S" | "s" | "S*" => {
                        // 轴对齐的描边线段当成一条细线收进来
                        if path.len() >= 2 {
                            let (ax, ay) = path[0];
                            let (bx, by) = path[path.len() - 1];
                            let (x0, x1) = if ax <= bx { (ax, bx) } else { (bx, ax) };
                            let (y0, y1) = if ay <= by { (ay, by) } else { (by, ay) };
                            let t = 0.8f32;
                            if y1 - y0 <= 1.2 && x1 - x0 >= 6.0 {
                                rects.push(Rect {
                                    x0,
                                    y0: y0 - t / 2.0,
                                    x1,
                                    y1: y0 + t / 2.0,
                                    fill: stroke,
                                });
                            } else if x1 - x0 <= 1.2 && y1 - y0 >= 6.0 {
                                rects.push(Rect {
                                    x0: x0 - t / 2.0,
                                    y0,
                                    x1: x0 + t / 2.0,
                                    y1,
                                    fill: stroke,
                                });
                            }
                        }
                        pending.clear();
                        path.clear();
                    }
                    "BT" => {
                        tm = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
                        tlm = tm;
                    }
                    "Tf" => {
                        if let Some(last) = nums.last() {
                            size = *last;
                        }
                        let mut name: Option<Vec<u8>> = None;
                        for operand in operands.iter() {
                            if let Operand::Name(n) = operand {
                                name = Some(n.clone());
                            }
                        }
                        active = name;
                    }
                    "Tm" => {
                        if nums.len() >= 6 {
                            tm = [
                                nums[nums.len() - 6],
                                nums[nums.len() - 5],
                                nums[nums.len() - 4],
                                nums[nums.len() - 3],
                                nums[nums.len() - 2],
                                nums[nums.len() - 1],
                            ];
                            tlm = tm;
                        }
                    }
                    "Td" | "TD" => {
                        if nums.len() >= 2 {
                            let (tx, ty) = (nums[nums.len() - 2], nums[nums.len() - 1]);
                            tlm = m_mul([1.0, 0.0, 0.0, 1.0, tx, ty], tlm);
                            tm = tlm;
                        }
                    }
                    "T*" => {
                        tlm = m_mul([1.0, 0.0, 0.0, 1.0, 0.0, -size * 1.2], tlm);
                        tm = tlm;
                    }
                    "Tj" | "TJ" | "'" | "\"" => {
                        if op == "'" || op == "\"" {
                            tlm = m_mul([1.0, 0.0, 0.0, 1.0, 0.0, -size * 1.2], tlm);
                            tm = tlm;
                        }
                        if let Some(font) = active.as_ref().and_then(|n| fonts.get(n)) {
                            let mut text = String::new();
                            for operand in operands.iter() {
                                match operand {
                                    Operand::Str(bytes) => {
                                        text.push_str(&decode_str(bytes, font))
                                    }
                                    // TJ 数组里夹的字距数字就是词间空白，不补空格的话
                                    // 「Chongqing Skyward」会被拼成一个词
                                    Operand::Num(n) if (*n).abs() >= 120.0 => {
                                        if !text.is_empty() && !text.ends_with(' ') {
                                            text.push(' ');
                                        }
                                    }
                                    _ => {}
                                }
                            }
                            if !text.trim().is_empty() {
                                let trm = m_mul(tm, ctm);
                                let (x, y) = m_apply(trm, 0.0, 0.0);
                                // 页面若套了翻转矩阵，y 是「距顶」坐标，记下来统一翻回去
                                if trm[0] * trm[3] - trm[1] * trm[2] < 0.0 {
                                    flip_neg += 1;
                                } else {
                                    flip_pos += 1;
                                }
                                let adv = est_width(&text, size);
                                frags.push(Frag {
                                    x,
                                    y,
                                    // 字号由 Tf 给定，实际大小还要乘文本矩阵的缩放；
                                    // 有些 PDF 用 `0.05 0 0 -0.05` 做缩放，不能拿 .max(0.2) 兜底放大
                                    size: if trm[3].abs() > 0.01 {
                                        size * trm[3].abs()
                                    } else {
                                        size
                                    },
                                    text,
                                    color: if is_black(fill) { None } else { Some(fill) },
                                });
                                tm = m_mul([1.0, 0.0, 0.0, 1.0, adv, 0.0], tm);
                            }
                        }
                    }
                    _ => {}
                }
                operands.clear();
            }
        }
    }
    let flipped = page_flip || flip_neg > flip_pos;
    (frags, rects, clips, header_images, flipped)
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

fn obj_dict<'a>(doc: &'a Document, object: &'a Object) -> Option<&'a lopdf::Dictionary> {
    match object {
        Object::Dictionary(d) => Some(d),
        Object::Reference(id) => doc.get_object(*id).ok().and_then(|o| o.as_dict().ok()),
        _ => None,
    }
}

/// 把页眉里绘制过的图片 XObject 取出来，转成 PNG 供 Word 页眉使用
fn header_pngs(
    doc: &Document,
    resources: Option<&lopdf::Dictionary>,
    images: &[(Vec<u8>, [f32; 6])],
) -> Vec<HeaderImage> {
    let mut out = Vec::new();
    let Some(xobjects) = resources
        .and_then(|r| r.get(b"XObject").ok())
        .and_then(|o| obj_dict(doc, o))
    else {
        return out;
    };
    for (name, ctm) in images.iter() {
        let Ok(object) = xobjects.get(name.as_slice()) else {
            continue;
        };
        let Some(stream) = (match object {
            Object::Stream(s) => Some(s.clone()),
            Object::Reference(id) => doc.get_object(*id).ok().and_then(|o| match o {
                Object::Stream(s) => Some(s.clone()),
                _ => None,
            }),
            _ => None,
        }) else {
            continue;
        };
        let Some((w, h, rgb)) = super::pdf_render::decode_image(&stream) else {
            continue;
        };
        let (iw, ih) = (w as u32, h as u32);
        // 页眉 logo 多半带 /SMask 透明通道，丢掉 alpha 的话透明区会渲染成黑块
        let alpha = stream
            .dict
            .get(b"SMask")
            .ok()
            .and_then(|o| match o {
                Object::Reference(id) => doc.get_object(*id).ok().and_then(|x| x.as_stream().ok().cloned()),
                Object::Stream(s) => Some(s.clone()),
                _ => None,
            })
            .and_then(|sm| super::pdf_render::decode_image(&sm))
            .filter(|(aw, ah, _)| *aw == w && *ah == h)
            .map(|(_, _, mask)| mask.chunks_exact(3).map(|p| p[0]).collect::<Vec<u8>>());
        let mut png: Vec<u8> = Vec::new();
        {
            let mut cursor = std::io::Cursor::new(&mut png);
            let wrote = match alpha {
                Some(a) => {
                    let mut rgba = Vec::with_capacity(rgb.len() / 3 * 4);
                    for (i, px) in rgb.chunks_exact(3).enumerate() {
                        rgba.extend_from_slice(px);
                        rgba.push(a.get(i).copied().unwrap_or(255));
                    }
                    match image::RgbaImage::from_raw(iw, ih, rgba) {
                        Some(buf) => buf.write_to(&mut cursor, image::ImageFormat::Png).is_ok(),
                        None => false,
                    }
                }
                None => match image::RgbImage::from_raw(iw, ih, rgb) {
                    Some(buf) => buf.write_to(&mut cursor, image::ImageFormat::Png).is_ok(),
                    None => false,
                },
            };
            if !wrote {
                continue;
            }
        }
        out.push(HeaderImage {
            png,
            width: ctm[0].abs(),
            height: ctm[3].abs(),
            x: ctm[4],
            y: ctm[5],
        });
    }
    out
}

fn obj_f32(doc: &Document, o: &Object) -> Option<f32> {
    match o {
        Object::Integer(i) => Some(*i as f32),
        Object::Real(r) => Some(*r as f32),
        Object::Reference(id) => doc.get_object(*id).ok().and_then(|x| obj_f32(doc, x)),
        _ => None,
    }
}

/// 页面高度，用来判断哪一段是页眉区（PDF 坐标原点在左下角，y 越大越靠上）
fn page_height(doc: &Document, page_id: lopdf::ObjectId) -> f32 {
    let dict = doc.get_object(page_id).ok().and_then(|o| o.as_dict().ok());
    let media = dict.and_then(|d| d.get(b"MediaBox").ok()).and_then(|o| match o {
        Object::Array(a) => Some(a.clone()),
        Object::Reference(id) => doc.get_object(*id).ok().and_then(|x| x.as_array().ok().cloned()),
        _ => None,
    });
    match media {
        Some(a) if a.len() >= 4 => {
            let y1 = obj_f32(doc, &a[3]).unwrap_or(842.0);
            let y0 = obj_f32(doc, &a[1]).unwrap_or(0.0);
            (y1 - y0).abs().max(1.0)
        }
        _ => 842.0,
    }
}

/// 页面宽度
fn page_width(doc: &Document, page_id: lopdf::ObjectId) -> f32 {
    let dict = doc.get_object(page_id).ok().and_then(|o| o.as_dict().ok());
    let media = dict.and_then(|d| d.get(b"MediaBox").ok()).and_then(|o| match o {
        Object::Array(a) => Some(a.clone()),
        Object::Reference(id) => doc.get_object(*id).ok().and_then(|x| x.as_array().ok().cloned()),
        _ => None,
    });
    match media {
        Some(a) if a.len() >= 4 => {
            let x1 = obj_f32(doc, &a[2]).unwrap_or(595.0);
            let x0 = obj_f32(doc, &a[0]).unwrap_or(0.0);
            (x1 - x0).abs().max(1.0)
        }
        _ => 595.0,
    }
}

/// 页眉探测：只有「每一页顶部都重复出现同一段文字」才算页眉。
/// 只看页面顶部 150pt 内的行，抽样首/中/末页比对，返回页眉底边（PDF y）。
fn detect_header(
    doc: &Document,
    pages: &[(u32, lopdf::ObjectId)],
) -> Option<f32> {    if pages.len() < 2 {
        return None;
    }
    let mut probe: Vec<usize> = vec![0, pages.len() - 1];
    if pages.len() > 2 {
        probe.push(pages.len() / 2);
    }
    probe.sort_unstable();
    probe.dedup();

    let norm = |s: &str| -> String {
        s.chars()
            .filter(|c| !c.is_whitespace() && *c != '\u{3000}')
            .collect()
    };

    let mut samples: Vec<Vec<(f32, f32, String)>> = Vec::new(); // (y, size, 行文本)
    for idx in probe.iter() {
        let (_, page_id) = pages[*idx];
        let fonts = doc.get_page_fonts(page_id).unwrap_or_default();
        let fonts_for_page: HashMap<Vec<u8>, FontMap> = fonts
            .iter()
            .map(|(name, dict)| (name.clone(), font_map(doc, dict)))
            .collect();
        let content = doc.get_page_content(page_id).unwrap_or_default();
        let (frags, ..) = run_content(&content, &fonts_for_page);
        let page_h = page_height(doc, page_id);
        let mut lines = frags_to_lines(
            frags
                .into_iter()
                .filter(|f| f.y >= page_h - 150.0)
                .collect(),
        );
        lines.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        samples.push(
            lines
                .into_iter()
                .map(|(y, d)| (y, d.size, norm(&d.text)))
                .filter(|(_, _, t)| !t.is_empty())
                .collect(),
        );
    }
    if samples.iter().any(|s| s.is_empty()) {
        return None;
    }

    // 从最顶一行往下逐行比对，取所有抽样页都一致的最长前缀
    let mut matched = 0usize;
    let limit = samples.iter().map(|s| s.len()).min().unwrap_or(0).min(4);
    for i in 0..limit {
        let head = samples[0][i].2.clone();
        if head.is_empty() {
            break;
        }
        if !samples.iter().all(|s| s[i].2 == head) {
            break;
        }
        matched = i + 1;
    }
    if matched == 0 {
        return None;
    }
    // 以第一页的页眉块底边作为分界（行 y 是基线，减去字号得到下缘）
    let (y, size, _) = samples[0][matched - 1].clone();
    let bottom = y - size * 1.15;
    let page_h = page_height(doc, pages[0].1);
    if bottom < page_h - 170.0 || bottom > page_h - 2.0 {
        return None;
    }
    Some(bottom)
}

/// 页脚判定：所有页面底部都出现的那段相同内容才算页脚（页码里的数字归一后再比）。
/// 返回页脚区上边界，frag.y 小于它即属于页脚。
fn detect_footer(
    doc: &Document,
    pages: &[(u32, lopdf::ObjectId)],
) -> Option<f32> {
    if pages.len() < 2 {
        return None;
    }
    let mut probe: Vec<usize> = vec![0, pages.len() - 1];
    if pages.len() > 2 {
        probe.push(pages.len() / 2);
    }
    probe.sort_unstable();
    probe.dedup();

    // 页码每页不同，比较前把数字统一成 #，否则「第1页共2页」和「第2页共2页」永远对不上
    let norm = |s: &str| -> String {
        s.chars()
            .filter(|c| !c.is_whitespace() && *c != '\u{3000}')
            .map(|c| if c.is_ascii_digit() { '#' } else { c })
            .collect()
    };

    let mut samples: Vec<Vec<(f32, f32, String)>> = Vec::new();
    for idx in probe.iter() {
        let (_, page_id) = pages[*idx];
        let fonts = doc.get_page_fonts(page_id).unwrap_or_default();
        let fonts_for_page: HashMap<Vec<u8>, FontMap> = fonts
            .iter()
            .map(|(name, dict)| (name.clone(), font_map(doc, dict)))
            .collect();
        let content = doc.get_page_content(page_id).unwrap_or_default();
        let (frags, ..) = run_content(&content, &fonts_for_page);
        let mut lines = frags_to_lines(
            frags
                .into_iter()
                .filter(|f| f.y <= 150.0)
                .collect(),
        );
        // y 是 PDF 原生坐标（越大越靠上），从最底一行开始往上比
        lines.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        samples.push(
            lines
                .into_iter()
                .map(|(y, d)| (y, d.size, norm(&d.text)))
                .filter(|(_, _, t)| !t.is_empty())
                .collect(),
        );
    }
    if samples.iter().any(|s| s.is_empty()) {
        return None;
    }

    let mut matched = 0usize;
    let limit = samples.iter().map(|s| s.len()).min().unwrap_or(0).min(4);
    for i in 0..limit {
        let tail = samples[0][i].2.clone();
        if tail.is_empty() {
            break;
        }
        if !samples.iter().all(|s| s[i].2 == tail) {
            break;
        }
        matched = i + 1;
    }
    if matched == 0 {
        return None;
    }
    let (y, size, _) = samples[0][matched - 1].clone();
    let top = y + size * 1.15;
    if top < 2.0 || top > 170.0 {
        return None;
    }
    Some(top)
}

/// 单个字形的近似宽度（em）。这份 PDF 逐字 `Tj` + `TD` 定位，
/// 词间空白没有空格字形，只能靠「步进比字宽多出多少」判断。
fn band_glyph_em(ch: char) -> f32 {
    match ch {
        ' ' | '!' | '|' | ',' | '.' | ':' | ';' | 'f' | 't' => 0.278,
        '"' => 0.355,
        '#' | '$' | '0'..='9' | 'E' | 'F' | 'L' | 'T' | 'Z' | 'a' | 'b' | 'd' | 'e' | 'g'
        | 'h' | 'n' | 'o' | 'p' | 'q' | 'u' => 0.556,
        '%' => 0.889,
        '&' | 'C' | 'K' | 'P' | 'S' | 'V' | 'X' | 'Y' | 'A' | 'B' => 0.667,
        '\'' | '`' => 0.191,
        '(' | ')' | '-' | 'r' => 0.333,
        '*' => 0.389,
        '+' | '<' | '=' | '>' => 0.584,
        '?' => 0.556,
        '@' => 1.015,
        'D' | 'H' | 'N' | 'R' | 'U' | 'w' => 0.722,
        'W' => 0.944,
        'G' | 'O' | 'Q' | 'M' | 'm' => 0.833,
        'I' | 'i' | 'j' | 'l' => 0.222,
        'J' | 'c' | 'k' | 's' | 'x' | 'y' | 'z' | 'v' => 0.500,
        c if (c as u32) >= 0x2E80 => 1.0,
        _ => 0.55,
    }
}

/// 页眉/页脚的文字按行聚起来，行内再按横向间隔分块：
/// 两家公司抬头并排时是两个独立的块，Word 里才能各占一栏。
fn frags_to_bands(frags: Vec<Frag>) -> Vec<Vec<HeaderLine>> {
    let mut frags = frags;
    frags.retain(|f| !f.text.trim().is_empty());
    if frags.is_empty() {
        return Vec::new();
    }
    frags.sort_by(|a, b| {
        b.y.partial_cmp(&a.y)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.x.partial_cmp(&b.x).unwrap_or(std::cmp::Ordering::Equal))
    });

    let mut rows: Vec<Vec<Frag>> = Vec::new();
    let mut row_y = f32::NAN;
    for frag in frags {
        let tol = (frag.size * 0.35).max(1.6);
        if rows.is_empty() || (frag.y - row_y).abs() > tol {
            rows.push(Vec::new());
            row_y = frag.y;
        }
        rows.last_mut().unwrap().push(frag);
    }

    let mut out: Vec<Vec<HeaderLine>> = Vec::new();
    for mut row in rows {
        row.sort_by(|a, b| a.x.partial_cmp(&b.x).unwrap_or(std::cmp::Ordering::Equal));
        let mut blocks: Vec<Vec<Frag>> = Vec::new();
        let mut last_right = f32::NAN;
        for frag in row {
            let brk = blocks.is_empty()
                || (frag.x - last_right) > (frag.size * 2.0).max(8.0)
                || (frag.x - last_right) < -(frag.size * 2.0);
            if brk {
                blocks.push(Vec::new());
            }
            last_right = frag.x + est_width(&frag.text, frag.size);
            blocks.last_mut().unwrap().push(frag);
        }
        let mut lines: Vec<HeaderLine> = Vec::new();
        for block in blocks {
            let x = block.first().map(|f| f.x).unwrap_or(0.0);
            let size = block.iter().map(|f| f.size).fold(0.0, f32::max);
            let color = frag_color(&block);
            let mut text = String::new();
            let mut end_x = f32::NAN;
            // 上一个单字 frag 的 (字形, 起点 x, 字号)，用来判断词间有没有留白
            let mut prev: Option<(char, f32, f32)> = None;
            for frag in block.iter() {
                if let Some((ch, px, psize)) = prev {
                    // 只有西文之间才按字距判断：中文每个字本来就占满一格，
                    // 拿来比较会把「重庆市」拆成「重 庆 市」
                    let next_ascii = frag
                        .text
                        .chars()
                        .next()
                        .map(|c| c.is_ascii())
                        .unwrap_or(false);
                    if ch.is_ascii() && next_ascii {
                        let step = frag.x - px;
                        let expected = band_glyph_em(ch) * psize;
                        if step - expected > psize * 0.22 && !text.ends_with(' ') {
                            text.push(' ');
                        }
                    }
                }
                let gap = frag.x - end_x;
                if !text.is_empty() && gap > frag.size.max(4.0) * 0.6 && !text.ends_with(' ') {
                    text.push(' ');
                }
                text.push_str(&frag.text);
                end_x = frag.x + est_width(&frag.text, frag.size);
                let mut chars = frag.text.chars();
                prev = match (chars.next(), chars.next()) {
                    (Some(c), None) => Some((c, frag.x, frag.size)),
                    _ => None,
                };
            }
            // 连续空白压成一个空格，首尾去掉
            let trimmed = text
                .split(|c: char| c == ' ' || c == '\u{3000}')
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            if !trimmed.is_empty() {
                lines.push(HeaderLine {
                    text: trimmed.to_string(),
                    x,
                    size,
                    color,
                });
            }
        }
        if !lines.is_empty() {
            out.push(lines);
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
    let page_list: Vec<(u32, lopdf::ObjectId)> = pages.into_iter().collect();
    // 先探测「每页顶部重复出现的文字」和「每页底部重复出现的内容」，
    // 只有它们才算页眉 / 页脚，单页大标题、封面、顶格段落都不算
    let header_zone_global = detect_header(&doc, &page_list);
    let footer_zone_global = detect_footer(&doc, &page_list);
    let mut out: Vec<PageText> = Vec::new();
    let mut header_done = false;
    let mut footer_done = false;
    for (number, page_id) in page_list.iter().copied() {
        if cancelled() {
            return Err("已取消".to_string());
        }
        let fonts = doc.get_page_fonts(page_id).unwrap_or_default();
        let fonts_for_page: HashMap<Vec<u8>, FontMap> = fonts
            .iter()
            .map(|(name, dict)| (name.clone(), font_map(&doc, dict)))
            .collect();
        // 页眉的图片要顺着页面资源里的 XObject 取原始像素
        let resources = doc
            .get_page_resources(page_id)
            .ok()
            .and_then(|(dict, _)| dict.cloned());
        let content = doc.get_page_content(page_id).unwrap_or_default();
        let (frags, mut rects, clips, images, flipped) =
            run_content(&content, &fonts_for_page);

        let page_h = page_height(&doc, page_id);
        let page_w = page_width(&doc, page_id);
        // 整页的 `1 0 0 -1 0 H cm` 已经在 run_content 里经 ctm 换算成正常坐标，这里不需要再翻

        // 单元格裁剪框的边就是格线，转成细线交给表格识别。
        // 单独一个、又不够宽的裁剪框通常只是给某段文字/图片留的裁切区，不是单元格，要丢掉。
        let mut keep = vec![false; clips.len()];
        for (i, ci) in clips.iter().enumerate() {
            let same_row = |cj: &Rect| {
                (ci.y0 - cj.y0).abs() <= 3.0 && (ci.y1 - cj.y1).abs() <= 3.0
            };
            let neighbour = clips.iter().enumerate().any(|(j, cj)| {
                j != i && same_row(cj) && ((cj.x0 - ci.x1).abs() <= 6.0 || (ci.x0 - cj.x1).abs() <= 6.0)
            });
            // 只有一个格子有内容时（比如跨页延续的高行），它没有并排邻居，
            // 但格子本身够宽，仍然是单元格边界
            if neighbour || (ci.x1 - ci.x0) >= page_w * 0.4 {
                keep[i] = true;
            }
        }
        for (i, c) in clips.iter().enumerate() {
            if !keep[i] {
                continue;
            }
            let w = c.x1 - c.x0;
            let h = c.y1 - c.y0;
            if w < 8.0 || h < 6.0 {
                continue;
            }
            if w > page_w * 0.96 && h > page_h * 0.96 {
                continue; // 整页裁剪框
            }
            let t = 0.6f32;
            rects.push(Rect { x0: c.x0, y0: c.y0 - t, x1: c.x1, y1: c.y0, fill: [0, 0, 0] });
            rects.push(Rect { x0: c.x0, y0: c.y1, x1: c.x1, y1: c.y1 + t, fill: [0, 0, 0] });
            rects.push(Rect { x0: c.x0 - t, y0: c.y0, x1: c.x0, y1: c.y1, fill: [0, 0, 0] });
            rects.push(Rect { x0: c.x1, y0: c.y0, x1: c.x1 + t, y1: c.y1, fill: [0, 0, 0] });
        }

        // 页眉不是「页面顶部那一带」，而是「每一页顶部都相同的那段文字」。
        // 单页大标题、封面、正文顶格段落都不算，否则会被整段吃掉。
        let header_bottom = header_zone_global;
        let make_header = header_bottom.is_some() && !header_done;
        if let Some(z) = header_bottom {
            // 页眉区里的横线（页眉下的分隔线之类）不能再参与表格网格，
            // 否则表格会一路顶到页眉，把标题和收件人信息吞成表格头几行
            rects.retain(|r| !((r.y1 - r.y0) <= 2.5 && r.y0 >= z));
        }
        let (mut top_frags, body_frags): (Vec<Frag>, Vec<Frag>) = match header_bottom {
            Some(z) => frags.into_iter().partition(|f| f.y >= z),
            None => (Vec::new(), frags),
        };
        // 整页大图（海报、扫描页）不是页眉 logo，要留在正文里
        let is_header_image = |m: &[f32; 6]| -> bool {
            match header_bottom {
                Some(z) => m[5] >= z && m[3].abs() < page_h * 0.25,
                None => false,
            }
        };
        let header = if make_header {
            header_done = true;
            let mut picked: Vec<(Vec<u8>, [f32; 6])> = images
                .iter()
                .filter(|(_, m)| is_header_image(m))
                .cloned()
                .collect();
            picked.sort_by(|a, b| a.1[4].partial_cmp(&b.1[4]).unwrap_or(std::cmp::Ordering::Equal));
            Some(HeaderBlock {
                rows: frags_to_bands(top_frags),
                images: header_pngs(&doc, resources.as_ref(), &picked),
            })
        } else {
            top_frags.clear();
            None
        };

        // 页脚：底部那段每页都一样的内容，正文里不能再重复一遍
        let footer_top = footer_zone_global;
        let make_footer = footer_top.is_some() && !footer_done;
        let (footer_frags, body_frags): (Vec<Frag>, Vec<Frag>) = match footer_top {
            Some(t) => {
                let mut foot = Vec::new();
                let mut rest = Vec::new();
                for f in body_frags {
                    if f.y <= t {
                        foot.push(f);
                    } else {
                        rest.push(f);
                    }
                }
                (foot, rest)
            }
            None => (Vec::new(), body_frags),
        };
        let is_footer_image = |m: &[f32; 6]| -> bool {
            match footer_top {
                Some(t) => m[5] <= t,
                None => false,
            }
        };
        let footer = if make_footer {
            footer_done = true;
            let mut picked: Vec<(Vec<u8>, [f32; 6])> = images
                .iter()
                .filter(|(_, m)| is_footer_image(m))
                .cloned()
                .collect();
            picked.sort_by(|a, b| a.1[4].partial_cmp(&b.1[4]).unwrap_or(std::cmp::Ordering::Equal));
            Some(FooterBlock {
                rows: frags_to_bands(footer_frags),
                images: header_pngs(&doc, resources.as_ref(), &picked),
            })
        } else {
            None
        };

        // 剩下的图片就是正文里的图，按原位置排进 Word
        let body_picked: Vec<(Vec<u8>, [f32; 6])> = images
            .iter()
            .filter(|(_, m)| !is_header_image(m) && !is_footer_image(m))
            .cloned()
            .collect();
        let pics: Vec<(f32, BodyImage)> = header_pngs(&doc, resources.as_ref(), &body_picked)
            .into_iter()
            .map(|h| {
                (
                    h.y,
                    BodyImage {
                        png: h.png,
                        width: h.width,
                        height: h.height,
                    },
                )
            })
            .collect();

        if std::env::var("DOCS_DEBUG").is_ok() {
            let mut hs: Vec<f32> = rects
                .iter()
                .filter(|r| (r.y1 - r.y0) <= 2.5 && (r.x1 - r.x0) >= 24.0)
                .map(|r| (r.y0 * 10.0).round() / 10.0)
                .collect();
            hs.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
            hs.dedup_by(|a, b| (*a - *b).abs() < 1.0);
            eprintln!(
                "[page {}] flipped={} clips={} kept={} header_zone={:?} horizons={:?}",
                number,
                flipped,
                clips.len(),
                keep.iter().filter(|k| **k).count(),
                header_zone_global,
                hs
            );
        }

        let (items, lines) = build_items(body_frags, rects, pics);
        out.push(PageText {
            page: number,
            lines,
            items,
            header,
            footer,
        });
    }
    Ok(out)
}

/// 是否有任何页面抽出了文字（判断扫描件用）
pub fn has_text(pages: &[PageText]) -> bool {
    pages.iter().any(|p| !p.lines.is_empty())
}
