// 文本排版 → PDF（自写排版器，字体用内置 Noto Sans SC 子集嵌入）。
//
// 输入是一串 Item：段落块（带字号/缩进）与表格（自动列宽 + 单元格折行 + 网格线）。
// 输出 A4 多页 PDF，供 docx/xlsx/pptx/md/txt/html/csv 转 PDF 复用。

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use super::pdf_font::{default_face, FontFace};

pub const PAGE_W: f32 = 595.28;
pub const PAGE_H: f32 = 841.89;
const MARGIN_X: f32 = 56.0;
const MARGIN_TOP: f32 = 56.0;
const MARGIN_BOTTOM: f32 = 56.0;
const LINE_RATIO: f32 = 1.55;

const TABLE_SIZE: f32 = 9.0;
const CELL_PAD_X: f32 = 4.0;
const CELL_PAD_Y: f32 = 3.5;

#[derive(Clone)]
pub struct Block {
    pub text: String,
    pub size: f32,
    pub bold: bool,
    pub indent: f32,
    pub space_before: f32,
}

impl Block {
    pub fn body(text: impl Into<String>) -> Block {
        Block {
            text: text.into(),
            size: 10.5,
            bold: false,
            indent: 0.0,
            space_before: 2.0,
        }
    }

    pub fn heading(text: impl Into<String>, level: u8) -> Block {
        let size = match level {
            0 => 22.0,
            1 => 17.0,
            2 => 14.0,
            _ => 12.0,
        };
        Block {
            text: text.into(),
            size,
            bold: true,
            indent: 0.0,
            space_before: size * 0.9,
        }
    }
}

pub struct ImageItem {
    /// 原始编码字节（JPEG 直接透传，其它格式解码后按 RGB 重编码）
    pub data: Vec<u8>,
    /// 显示尺寸（pt）
    pub w: f32,
    pub h: f32,
}

pub enum Item {
    Text(Block),
    Table(Vec<Vec<String>>),
    Image(ImageItem),
}

enum Placed {
    Text {
        hex: String,
        x: f32,
        y: f32,
        size: f32,
        bold: bool,
    },
    Rule {
        x1: f32,
        x2: f32,
        y1: f32,
        y2: f32,
    },
    Image {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        slot: usize,
    },
}

impl Placed {
    fn y(&self) -> f32 {
        match self {
            Placed::Text { y, .. } => *y,
            Placed::Rule { y1, y2, .. } => y1.min(*y2),
            Placed::Image { y, .. } => *y,
        }
    }
}

struct Built {
    placed: Vec<Placed>,
    used_gids: HashSet<u16>,
    gid_chars: HashMap<u16, char>,
    images: Vec<(Vec<u8>, f32, f32)>,
    pages: usize,
}

struct Layout<'a> {
    face: &'a FontFace,
    placed: Vec<Placed>,
    used_gids: HashSet<u16>,
    gid_chars: HashMap<u16, char>,
    images: Vec<(Vec<u8>, f32, f32)>,
    y: f32,
}

impl<'a> Layout<'a> {
    fn width_of(&self, ch: char, size: f32) -> f32 {
        let gid = self.face.gid(ch);
        self.face.advance(gid) as f32 * size / self.face.units_per_em as f32
    }

    fn text_width(&self, text: &str, size: f32) -> f32 {
        text.chars().map(|c| self.width_of(c, size)).sum()
    }

    fn note(&mut self, gid: u16, ch: char) {
        self.used_gids.insert(gid);
        self.gid_chars.entry(gid).or_insert(ch);
    }

    fn hex_of(&mut self, glyphs: &[(u16, char)]) -> String {
        let mut hex = String::with_capacity(glyphs.len() * 4);
        for (gid, ch) in glyphs {
            hex.push_str(&format!("{:04X}", gid));
            self.note(*gid, *ch);
        }
        hex
    }

    fn new_page_if_needed(&mut self, need: f32) {
        if self.y - need < MARGIN_BOTTOM {
            self.y = PAGE_H - MARGIN_TOP;
        }
    }

    /// 把一段文字按可用宽度切成多行（中文任意断行，英文优先在空白处断）
    fn wrap(&mut self, text: &str, size: f32, avail: f32) -> Vec<(Vec<(u16, char)>, f32)> {
        let mut lines: Vec<(Vec<(u16, char)>, f32)> = Vec::new();
        let mut line: Vec<(u16, char)> = Vec::new();
        let mut width = 0.0f32;
        let mut last_break: Option<usize> = None;
        let mut last_break_w = 0.0f32;

        for ch in text.chars() {
            let w = self.width_of(ch, size);
            let is_space = ch == ' ' || ch == '\t';
            if width + w > avail && !line.is_empty() {
                let cut = match last_break {
                    Some(i) if last_break_w > avail * 0.3 => i,
                    _ => line.len(),
                };
                let head: Vec<(u16, char)> = line.drain(..cut.min(line.len())).collect();
                let head_w: f32 = head.iter().map(|(_, c)| self.width_of(*c, size)).sum();
                let mut trimmed = head;
                while trimmed.last().map(|(_, c)| *c == ' ').unwrap_or(false) {
                    trimmed.pop();
                }
                lines.push((trimmed, head_w));
                let rest = std::mem::take(&mut line);
                width = rest.iter().map(|(_, c)| self.width_of(*c, size)).sum();
                line = rest;
                last_break = None;
                last_break_w = 0.0;
            }
            if is_space {
                last_break = Some(line.len() + 1);
                last_break_w = width + w;
            }
            width += w;
            line.push((self.face.gid(ch), ch));
        }
        if !line.is_empty() {
            let line_w: f32 = line.iter().map(|(_, c)| self.width_of(*c, size)).sum();
            lines.push((line, line_w));
        }
        lines
    }

    fn push_text_block(&mut self, block: &Block) {
        let line_h = block.size * LINE_RATIO;
        if block.text.trim().is_empty() {
            self.y -= line_h * 0.6;
            return;
        }
        if block.space_before > 0.0 {
            self.y -= block.space_before.min(line_h);
        }
        self.new_page_if_needed(line_h);

        let avail = PAGE_W - MARGIN_X * 2.0 - block.indent;
        for (glyphs, _) in self.wrap(&block.text, block.size, avail) {
            self.new_page_if_needed(line_h);
            let hex = self.hex_of(&glyphs);
            self.placed.push(Placed::Text {
                hex,
                x: MARGIN_X + block.indent,
                y: self.y - block.size,
                size: block.size,
                bold: block.bold,
            });
            self.y -= line_h;
        }
    }

    fn push_table(&mut self, rows: &[Vec<String>]) {
        if rows.is_empty() {
            return;
        }
        let avail = PAGE_W - MARGIN_X * 2.0;
        let cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
        if cols == 0 {
            return;
        }

        // 自然列宽：单格里超长文字按“约 60 字”封顶，避免一列吃掉整页宽度
        let probe = self.text_width(&"中".repeat(60), TABLE_SIZE) + CELL_PAD_X * 2.0;
        let mut widths: Vec<f32> = vec![0.0; cols];
        for row in rows {
            for (index, cell) in row.iter().enumerate() {
                let w = (self.text_width(cell, TABLE_SIZE) + CELL_PAD_X * 2.0).min(probe);
                if w > widths[index] {
                    widths[index] = w;
                }
            }
        }

        // 权重封顶到平均列宽的 2.6 倍，再等比收敛到可用宽度
        let avg = avail / cols as f32;
        let weights: Vec<f32> = widths.iter().map(|w| w.max(24.0).min(avg * 2.6)).collect();
        let sum_w: f32 = weights.iter().sum();
        let mut widths: Vec<f32> = weights.iter().map(|w| w / sum_w * avail).collect();

        // 过窄的列补到 40pt，从最宽的列扣回来
        let mut deficit: f32 = 0.0;
        for w in widths.iter_mut() {
            if *w < 40.0 {
                deficit += 40.0 - *w;
                *w = 40.0;
            }
        }
        let mut guard = 0;
        while deficit > 0.5 && guard < 40 {
            guard += 1;
            let widest = widths
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
                .map(|(i, _)| i)
                .unwrap_or(0);
            if widths[widest] <= 40.5 {
                break;
            }
            let cut = (deficit * 0.5).min(widths[widest] - 40.0);
            widths[widest] -= cut;
            deficit -= cut;
        }

        let line_h = TABLE_SIZE * LINE_RATIO;
        let content_h = PAGE_H - MARGIN_TOP - MARGIN_BOTTOM;
        let per_page = (((content_h - CELL_PAD_Y * 2.0) / line_h).floor() as usize).max(1);

        for row in rows {
            // 每个单元格先折行，行的实际高度取最高的那格
            let mut cells: Vec<Vec<(Vec<(u16, char)>, f32)>> = Vec::new();
            let mut max_lines = 1usize;
            for index in 0..cols {
                let text = row.get(index).cloned().unwrap_or_default();
                let inner = (widths[index] - CELL_PAD_X * 2.0).max(8.0);
                let wrapped = if text.trim().is_empty() {
                    vec![(Vec::new(), 0.0)]
                } else {
                    self.wrap(&text, TABLE_SIZE, inner)
                };
                max_lines = max_lines.max(wrapped.len());
                cells.push(wrapped);
            }

            // 一行高于整页时按页切段继续画，避免文字跑到页外被裁掉
            let mut offset = 0usize;
            loop {
                let mut seg_lines = 1usize;
                for wrapped in cells.iter() {
                    seg_lines = seg_lines.max(wrapped.len().saturating_sub(offset));
                }
                let take = seg_lines.min(per_page);
                let row_h = take as f32 * line_h + CELL_PAD_Y * 2.0;
                if offset > 0 {
                    // 续段另起一页
                    self.y = PAGE_H - MARGIN_TOP;
                }
                self.new_page_if_needed(row_h);
                let top = self.y;

                // 按“行优先”落字：同一视觉行先画完所有列，再下一行。
                // 否则下一列会从行首重新开始 y，被页合并逻辑误判为换页。
                let xs: Vec<f32> = {
                    let mut xs = Vec::with_capacity(cols);
                    let mut x = MARGIN_X;
                    for w in widths.iter() {
                        xs.push(x + CELL_PAD_X);
                        x += *w;
                    }
                    xs
                };
                for visible in 0..take {
                    let line_index = offset + visible;
                    let y = top - CELL_PAD_Y - TABLE_SIZE - visible as f32 * line_h;
                    for (index, wrapped) in cells.iter().enumerate() {
                        if let Some((glyphs, _)) = wrapped.get(line_index) {
                            if glyphs.is_empty() {
                                continue;
                            }
                            let hex = self.hex_of(glyphs);
                            self.placed.push(Placed::Text {
                                hex,
                                x: xs[index],
                                y,
                                size: TABLE_SIZE,
                                bold: false,
                            });
                        }
                    }
                }
                self.y = top - row_h;

                // 网格：行底线 + 各列竖线
                self.placed.push(Placed::Rule {
                    x1: MARGIN_X,
                    x2: MARGIN_X + widths.iter().sum::<f32>(),
                    y1: self.y,
                    y2: self.y,
                });
                let mut vx = MARGIN_X;
                for w in &widths {
                    vx += *w;
                    self.placed.push(Placed::Rule {
                        x1: vx,
                        x2: vx,
                        y1: self.y,
                        y2: top,
                    });
                }

                offset += take;
                let done = cells.iter().all(|w| w.len() <= offset);
                if done {
                    break;
                }
            }
        }
        self.y -= TABLE_SIZE * 0.4;
    }

    /// 插图：按可用宽度等比缩放，超过一页高度时压到一页内容高
    fn push_image(&mut self, image: &ImageItem) {
        let avail_w = PAGE_W - MARGIN_X * 2.0;
        let avail_h = PAGE_H - MARGIN_TOP - MARGIN_BOTTOM;
        let w0 = if image.w > 1.0 { image.w } else { 300.0 };
        let h0 = if image.h > 1.0 { image.h } else { 200.0 };
        let mut scale = 1.0f32;
        if w0 > avail_w {
            scale = avail_w / w0;
        }
        if h0 * scale > avail_h {
            scale = avail_h / h0;
        }
        let w = w0 * scale;
        let h = h0 * scale;
        self.new_page_if_needed(h);
        let slot = self.images.len();
        self.images.push((image.data.clone(), w, h));
        self.placed.push(Placed::Image {
            x: MARGIN_X,
            y: self.y - h,
            w,
            h,
            slot,
        });
        self.y -= h + 8.0;
    }
}

fn layout(face: &FontFace, items: &[Item]) -> Built {
    let mut l = Layout {
        face,
        placed: Vec::new(),
        used_gids: HashSet::new(),
        gid_chars: HashMap::new(),
        images: Vec::new(),
        y: PAGE_H - MARGIN_TOP,
    };
    for item in items {
        match item {
            Item::Text(block) => l.push_text_block(block),
            Item::Table(rows) => l.push_table(rows),
            Item::Image(image) => l.push_image(image),
        }
    }

    let mut pages = 1usize;
    let mut prev = f32::INFINITY;
    for p in &l.placed {
        if p.y() > prev + 1.0 {
            pages += 1;
        }
        prev = p.y();
    }
    Built {
        placed: l.placed,
        used_gids: l.used_gids,
        gid_chars: l.gid_chars,
        images: l.images,
        pages,
    }
}

fn write_all(out: &mut BufWriter<File>, pos: &mut u64, bytes: &[u8]) -> Result<(), String> {
    out.write_all(bytes)
        .map_err(|e| format!("写入失败: {}", e))?;
    *pos += bytes.len() as u64;
    Ok(())
}

// 版式统计：当前由烟测工程校验（主工程只关心生成结果，不读这些计数）
#[allow(dead_code)]
pub struct Stats {
    pub pages: usize,
    pub glyphs: usize,
    pub font_bytes: usize,
    pub images: usize,
}

/// 识别 JPEG：返回 (宽, 高, 分量数)。非 JPEG 返回 None。
fn jpeg_info(data: &[u8]) -> Option<(u32, u32, u8)> {
    if data.len() < 4 || data[0] != 0xFF || data[1] != 0xD8 {
        return None;
    }
    let mut i = 2usize;
    while i + 3 < data.len() {
        if data[i] != 0xFF {
            i += 1;
            continue;
        }
        let marker = data[i + 1];
        if marker == 0xFF {
            i += 1;
            continue;
        }
        if marker == 0xD8 || marker == 0x01 || (0xD0..=0xD7).contains(&marker) {
            i += 2;
            continue;
        }
        let len = u16::from_be_bytes([data[i + 2], data[i + 3]]) as usize;
        if matches!(
            marker,
            0xC0..=0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF
        ) {
            if i + 9 < data.len() {
                let height = u16::from_be_bytes([data[i + 5], data[i + 6]]) as u32;
                let width = u16::from_be_bytes([data[i + 7], data[i + 8]]) as u32;
                return Some((width, height, data[i + 9]));
            }
            return None;
        }
        i += 2 + len;
    }
    None
}

struct EncodedImage {
    width: u32,
    height: u32,
    colorspace: &'static str,
    filter: &'static str,
    data: Vec<u8>,
}

fn deflate(data: &[u8]) -> Result<Vec<u8>, String> {
    use flate2::write::ZlibEncoder;
    use flate2::Compression;
    let mut out: Vec<u8> = Vec::new();
    let mut enc = ZlibEncoder::new(&mut out, Compression::new(6));
    enc.write_all(data).map_err(|e| e.to_string())?;
    enc.finish().map_err(|e| e.to_string())?;
    Ok(out)
}

/// 准备图像对象：JPEG 直接内嵌（DCTDecode），其它格式解码成 RGB 走 FlateDecode。
/// 显示尺寸小但原始像素很大的图先降到 150 DPI，避免一个封面图把 PDF 撑到几十 MB。
fn encode_image(bytes: &[u8], w_pt: f32, h_pt: f32) -> Result<EncodedImage, String> {
    if let Some((width, height, components)) = jpeg_info(bytes) {
        if width > 0 && height > 0 && (components == 3 || components == 1) {
            let fits = fits_display(width, height, w_pt, h_pt);
            if fits {
                return Ok(EncodedImage {
                    width,
                    height,
                    colorspace: if components == 1 {
                        "/DeviceGray"
                    } else {
                        "/DeviceRGB"
                    },
                    filter: "DCTDecode",
                    data: bytes.to_vec(),
                });
            }
        }
    }

    let dynimg = image::load_from_memory(bytes).map_err(|e| format!("图片解码失败: {}", e))?;
    let mut rgba = dynimg.to_rgba8();
    let (mut width, mut height) = (rgba.width(), rgba.height());
    if width == 0 || height == 0 {
        return Err("图片尺寸为 0".to_string());
    }

    // 按显示尺寸的 150 DPI 上界降采样
    if !fits_display(width, height, w_pt, h_pt) {
        let max_w = (w_pt.max(1.0) / 72.0 * 150.0).ceil().max(64.0);
        let max_h = (h_pt.max(1.0) / 72.0 * 150.0).ceil().max(64.0);
        let scale = (max_w / width as f32).min(max_h / height as f32);
        let new_w = ((width as f32 * scale).round() as u32).max(1);
        let new_h = ((height as f32 * scale).round() as u32).max(1);
        rgba = image::imageops::resize(&rgba, new_w, new_h, image::imageops::FilterType::Triangle);
        width = new_w;
        height = new_h;
    }

    // 透明区域铺白底，避免 PDF 里显示成黑块
    let mut rgb = Vec::with_capacity((width * height * 3) as usize);
    for pixel in rgba.pixels() {
        let a = pixel[3] as f32 / 255.0;
        for c in 0..3 {
            let v = pixel[c] as f32 * a + 255.0 * (1.0 - a);
            rgb.push(v.round().clamp(0.0, 255.0) as u8);
        }
    }
    Ok(EncodedImage {
        width,
        height,
        colorspace: "/DeviceRGB",
        filter: "FlateDecode",
        data: deflate(&rgb)?,
    })
}

/// 像素数是否已经低于「显示尺寸 × 150 DPI」的 1.2 倍
fn fits_display(width: u32, height: u32, w_pt: f32, h_pt: f32) -> bool {
    let max_w = (w_pt.max(1.0) / 72.0 * 150.0 * 1.2).ceil();
    let max_h = (h_pt.max(1.0) / 72.0 * 150.0 * 1.2).ceil();
    width as f32 <= max_w && height as f32 <= max_h
}

// 纯文字入口：烟测工程用它验证版式，主工程统一走 to_pdf::convert
#[allow(dead_code)]
pub fn blocks_to_pdf(
    blocks: &[Block],
    dst: &Path,
    cancelled: &dyn Fn() -> bool,
) -> Result<Stats, String> {
    let items: Vec<Item> = blocks.iter().cloned().map(Item::Text).collect();
    items_to_pdf(&items, dst, cancelled)
}

pub fn items_to_pdf(
    items: &[Item],
    dst: &Path,
    cancelled: &dyn Fn() -> bool,
) -> Result<Stats, String> {
    let face = default_face().ok_or("内置字体不可用，无法排版")?;
    let has_text = items.iter().any(|item| match item {
        Item::Text(b) => !b.text.trim().is_empty(),
        Item::Table(rows) => rows.iter().any(|r| r.iter().any(|c| !c.trim().is_empty())),
        Item::Image(_) => false,
    });
    let has_images = items.iter().any(|item| matches!(item, Item::Image(_)));
    if !has_text && !has_images {
        return Err("没有可排版的内容".to_string());
    }
    if cancelled() {
        return Err("已取消".to_string());
    }

    let built = layout(face, items);

    // 每张图编码一次；相同字节只留一份对象
    let mut encoded: Vec<EncodedImage> = Vec::new();
    let mut slot_to_obj: Vec<usize> = Vec::with_capacity(built.images.len());
    let mut by_hash: HashMap<u64, usize> = HashMap::new();
    for (bytes, w_pt, h_pt) in &built.images {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        std::hash::Hash::hash(bytes, &mut hasher);
        let key = std::hash::Hasher::finish(&hasher);
        if let Some(index) = by_hash.get(&key) {
            slot_to_obj.push(*index);
            continue;
        }
        let image = encode_image(bytes, *w_pt, *h_pt)?;
        let index = encoded.len();
        encoded.push(image);
        by_hash.insert(key, index);
        slot_to_obj.push(index);
    }

    if cancelled() {
        return Err("已取消".to_string());
    }

    let subset = face.subset(&built.used_gids);
    let font_bytes = subset.len();
    let mut compressed: Vec<u8> = Vec::new();
    {
        use flate2::write::ZlibEncoder;
        use flate2::Compression;
        let mut enc = ZlibEncoder::new(&mut compressed, Compression::new(9));
        enc.write_all(&subset).map_err(|e| e.to_string())?;
        enc.finish().map_err(|e| e.to_string())?;
    }

    let mut pairs: Vec<(u16, char)> = built
        .gid_chars
        .iter()
        .filter(|(g, _)| **g != 0)
        .map(|(g, c)| (*g, *c))
        .collect();
    pairs.sort();
    let tounicode = face.to_unicode_cmap(&pairs);

    let mut widths = String::new();
    let mut gids: Vec<u16> = built.used_gids.iter().copied().collect();
    gids.sort();
    for gid in gids {
        widths.push_str(&format!("{} [{}] ", gid, face.width_1000(gid)));
    }

    let (bbox, ascent, descent, cap_height) = face.descriptor_numbers();
    let page_count = built.pages;
    let img_base = 8 + page_count as u32 * 2;
    let img_count = encoded.len() as u32;
    let total_objects = 7 + page_count as u32 * 2 + img_count;
    let mut offsets = vec![0u64; (total_objects + 1) as usize];

    let file = File::create(dst).map_err(|e| format!("写入失败: {}", e))?;
    let mut out = BufWriter::new(file);
    let mut pos: u64 = 0;
    write_all(&mut out, &mut pos, b"%PDF-1.5\n%\xE2\xE3\xCF\xD3\n")?;

    let begin = |num: u32,
                 out: &mut BufWriter<File>,
                 pos: &mut u64,
                 offsets: &mut Vec<u64>|
     -> Result<(), String> {
        offsets[num as usize] = *pos;
        write_all(out, pos, format!("{} 0 obj\n", num).as_bytes())
    };

    begin(1, &mut out, &mut pos, &mut offsets)?;
    write_all(
        &mut out,
        &mut pos,
        b"<< /Type /Catalog /Pages 2 0 R >>\nendobj\n",
    )?;

    let mut kids = String::new();
    for i in 0..page_count {
        kids.push_str(&format!("{} 0 R ", 8 + i as u32 * 2));
    }
    begin(2, &mut out, &mut pos, &mut offsets)?;
    write_all(
        &mut out,
        &mut pos,
        format!(
            "<< /Type /Pages /Kids [{}] /Count {} >>\nendobj\n",
            kids.trim_end(),
            page_count
        )
        .as_bytes(),
    )?;

    begin(3, &mut out, &mut pos, &mut offsets)?;
    write_all(
        &mut out,
        &mut pos,
        b"<< /Type /Font /Subtype /Type0 /BaseFont /NotoSansSC /Encoding /Identity-H /DescendantFonts [4 0 R] /ToUnicode 7 0 R >>\nendobj\n",
    )?;

    begin(4, &mut out, &mut pos, &mut offsets)?;
    write_all(
        &mut out,
        &mut pos,
        format!(
            "<< /Type /Font /Subtype /CIDFontType2 /BaseFont /NotoSansSC /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> /FontDescriptor 5 0 R /DW 1000 /W [{}] /CIDToGIDMap /Identity >>\nendobj\n",
            widths.trim_end()
        )
        .as_bytes(),
    )?;

    begin(5, &mut out, &mut pos, &mut offsets)?;
    write_all(
        &mut out,
        &mut pos,
        format!(
            "<< /Type /FontDescriptor /FontName /NotoSansSC /Flags 4 /FontBBox [{} {} {} {}] /ItalicAngle 0 /Ascent {} /Descent {} /CapHeight {} /StemV 80 /FontFile2 6 0 R >>\nendobj\n",
            bbox[0], bbox[1], bbox[2], bbox[3], ascent, descent, cap_height
        )
        .as_bytes(),
    )?;

    begin(6, &mut out, &mut pos, &mut offsets)?;
    write_all(
        &mut out,
        &mut pos,
        format!(
            "<< /Length {} /Length1 {} /Filter /FlateDecode >>\nstream\n",
            compressed.len(),
            subset.len()
        )
        .as_bytes(),
    )?;
    write_all(&mut out, &mut pos, &compressed)?;
    write_all(&mut out, &mut pos, b"\nendstream\nendobj\n")?;

    begin(7, &mut out, &mut pos, &mut offsets)?;
    write_all(
        &mut out,
        &mut pos,
        format!("<< /Length {} >>\nstream\n", tounicode.len()).as_bytes(),
    )?;
    write_all(&mut out, &mut pos, &tounicode)?;
    write_all(&mut out, &mut pos, b"endstream\nendobj\n")?;

    let mut page_index = 0u32;
    let mut prev_y = f32::INFINITY;
    let mut current = String::new();
    let mut page_images: HashSet<usize> = HashSet::new();

    fn flush_page(
        content: &mut String,
        index: &mut u32,
        page_images: &mut HashSet<usize>,
        slot_to_obj: &[usize],
        img_base: u32,
        out: &mut BufWriter<File>,
        pos: &mut u64,
        offsets: &mut [u64],
    ) -> Result<(), String> {
        let page_no = 8 + *index * 2;
        let content_no = page_no + 1;
        let mut xobjects = String::new();
        for slot in page_images.iter() {
            let obj = img_base + slot_to_obj[*slot] as u32;
            xobjects.push_str(&format!("/Im{} {} 0 R ", slot, obj));
        }
        let resources = if xobjects.is_empty() {
            "<< /Font << /F1 3 0 R >> >>".to_string()
        } else {
            format!(
                "<< /Font << /F1 3 0 R >> /XObject << {} >> >>",
                xobjects.trim_end()
            )
        };
        offsets[page_no as usize] = *pos;
        write_all(
            out,
            pos,
            format!(
                "{} 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {:.2} {:.2}] /Resources {} /Contents {} 0 R >>\nendobj\n",
                page_no, PAGE_W, PAGE_H, resources, content_no
            )
            .as_bytes(),
        )?;
        offsets[content_no as usize] = *pos;
        write_all(
            out,
            pos,
            format!("{} 0 obj\n<< /Length {} >>\nstream\n", content_no, content.len()).as_bytes(),
        )?;
        write_all(out, pos, content.as_bytes())?;
        write_all(out, pos, b"\nendstream\nendobj\n")?;
        content.clear();
        page_images.clear();
        *index += 1;
        Ok(())
    }

    for item in &built.placed {
        if item.y() > prev_y + 1.0 {
            flush_page(
                &mut current,
                &mut page_index,
                &mut page_images,
                &slot_to_obj,
                img_base,
                &mut out,
                &mut pos,
                &mut offsets,
            )?;
        }
        prev_y = item.y();
        match item {
            Placed::Text {
                hex,
                x,
                y,
                size,
                bold,
            } => {
                current.push_str(&format!(
                    "BT\n/F1 {:.2} Tf\n{}1 0 0 1 {:.2} {:.2} Tm\n<{}> Tj\nET\n",
                    size,
                    if *bold { "2 Tr\n0.45 w\n0 g 0 G\n" } else { "" },
                    x,
                    y,
                    hex
                ));
            }
            Placed::Rule { x1, x2, y1, y2 } => {
                current.push_str(&format!(
                    "q\n0.6 w\n0.62 G\n{:.2} {:.2} m\n{:.2} {:.2} l\nS\nQ\n",
                    x1, y1, x2, y2
                ));
            }
            Placed::Image { x, y, w, h, slot } => {
                page_images.insert(*slot);
                current.push_str(&format!(
                    "q\n{:.2} 0 0 {:.2} {:.2} {:.2} cm\n/Im{} Do\nQ\n",
                    w, h, x, y, slot
                ));
            }
        }
    }
    while (page_index as usize) < page_count {
        flush_page(
            &mut current,
            &mut page_index,
            &mut page_images,
            &slot_to_obj,
            img_base,
            &mut out,
            &mut pos,
            &mut offsets,
        )?;
    }

    for (index, image) in encoded.iter().enumerate() {
        let number = img_base + index as u32;
        begin(number, &mut out, &mut pos, &mut offsets)?;
        write_all(
            &mut out,
            &mut pos,
            format!(
                "<< /Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace {} /BitsPerComponent 8 /Filter /{} /Length {} >>\nstream\n",
                image.width, image.height, image.colorspace, image.filter, image.data.len()
            )
            .as_bytes(),
        )?;
        write_all(&mut out, &mut pos, &image.data)?;
        write_all(&mut out, &mut pos, b"\nendstream\nendobj\n")?;
    }

    let xref_pos = pos;
    let mut xref = String::from("xref\n");
    xref.push_str(&format!("0 {}\n", total_objects + 1));
    xref.push_str("0000000000 65535 f \n");
    for num in 1..=total_objects {
        xref.push_str(&format!("{:010} 00000 n \n", offsets[num as usize]));
    }
    write_all(&mut out, &mut pos, xref.as_bytes())?;
    write_all(
        &mut out,
        &mut pos,
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{}\n%%EOF\n",
            total_objects + 1,
            xref_pos
        )
        .as_bytes(),
    )?;
    out.flush().map_err(|e| format!("写入失败: {}", e))?;

    Ok(Stats {
        pages: page_count,
        glyphs: pairs.len(),
        font_bytes,
        images: encoded.len(),
    })
}
