// PDF 页面栅格化（纯 Rust，不依赖 PDFium / Ghostscript）。
//
// 覆盖常用内容流算子：q/Q/cm、路径(m/l/c/v/y/h/re)与填充/描边、颜色(rg/RG/g/G/k/K/sc/scn)、
// Do（图像 XObject 与 Form XObject）、BT…ET 文本（Tf/Td/TD/Tm/T*/Tj/TJ/'/"/Tc/Tw/Tz/TL/Tr）。
// 文字轮廓取自内嵌 TrueType（FontFile2）；CFF/Type1 轮廓暂不解析，会在统计里计数供上层提示。

use std::collections::HashMap;

use lopdf::{Dictionary, Document, Object, Stream};

use super::pdf_font::FontFace;

const SS: usize = 3; // 超采样倍数

#[derive(Clone, Copy, Debug)]
struct Mat {
    a: f32,
    b: f32,
    c: f32,
    d: f32,
    e: f32,
    f: f32,
}

impl Mat {
    fn identity() -> Mat {
        Mat { a: 1.0, b: 0.0, c: 0.0, d: 1.0, e: 0.0, f: 0.0 }
    }
    fn translate(x: f32, y: f32) -> Mat {
        Mat { a: 1.0, b: 0.0, c: 0.0, d: 1.0, e: x, f: y }
    }
    fn mul(&self, other: &Mat) -> Mat {
        Mat {
            a: self.a * other.a + self.b * other.c,
            b: self.a * other.b + self.b * other.d,
            c: self.c * other.a + self.d * other.c,
            d: self.c * other.b + self.d * other.d,
            e: self.e * other.a + self.f * other.c + other.e,
            f: self.e * other.b + self.f * other.d + other.f,
        }
    }
    fn apply(&self, x: f32, y: f32) -> (f32, f32) {
        (
            self.a * x + self.c * y + self.e,
            self.b * x + self.d * y + self.f,
        )
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Color {
    r: f32,
    g: f32,
    b: f32,
}

#[derive(Clone)]
struct State {
    ctm: Mat,
    fill: Color,
    stroke: Color,
    lw: f32,
    font: Option<usize>,
    size: f32,
    tc: f32,
    tw: f32,
    th: f32,
    tl: f32,
    rise: f32,
    render: i32,
}

impl State {
    fn new(ctm: Mat) -> State {
        State {
            ctm,
            fill: Color { r: 0.0, g: 0.0, b: 0.0 },
            stroke: Color { r: 0.0, g: 0.0, b: 0.0 },
            lw: 1.0,
            font: None,
            size: 1.0,
            tc: 0.0,
            tw: 0.0,
            th: 1.0,
            tl: 0.0,
            rise: 0.0,
            render: 0,
        }
    }
}

struct PdfFont {
    face: Option<FontFace>,
    two_byte: bool,
    default_width: f32,
    widths: HashMap<u32, f32>,
    cff: bool,
}

impl PdfFont {
    fn width(&self, code: u32) -> f32 {
        self.widths.get(&code).copied().unwrap_or(self.default_width)
    }
}

enum Tok {
    Num(f32),
    Str(Vec<u8>),
    Name(Vec<u8>),
    ArrStart,
    ArrEnd,
    DictStart,
    DictEnd,
    Op(String),
}

pub struct Canvas {
    pub w: usize,
    pub h: usize,
    pub pixels: Vec<u8>, // RGBA
}

impl Canvas {
    fn new(w: usize, h: usize) -> Canvas {
        let mut pixels = vec![255u8; w * h * 4];
        for p in pixels.chunks_mut(4) {
            p[3] = 255;
        }
        Canvas { w, h, pixels }
    }

    fn blend(&mut self, x0: i32, y0: i32, mask: &[f32], mw: usize, mh: usize, color: Color) {
        let r = (color.r * 255.0).clamp(0.0, 255.0) as f32;
        let g = (color.g * 255.0).clamp(0.0, 255.0) as f32;
        let b = (color.b * 255.0).clamp(0.0, 255.0) as f32;
        for my in 0..mh {
            let py = y0 + my as i32;
            if py < 0 || py >= self.h as i32 {
                continue;
            }
            for mx in 0..mw {
                let px = x0 + mx as i32;
                if px < 0 || px >= self.w as i32 {
                    continue;
                }
                let a = mask[my * mw + mx];
                if a <= 0.0 {
                    continue;
                }
                let idx = ((py as usize) * self.w + px as usize) * 4;
                self.pixels[idx] = ((self.pixels[idx] as f32) * (1.0 - a) + r * a) as u8;
                self.pixels[idx + 1] =
                    ((self.pixels[idx + 1] as f32) * (1.0 - a) + g * a) as u8;
                self.pixels[idx + 2] =
                    ((self.pixels[idx + 2] as f32) * (1.0 - a) + b * a) as u8;
            }
        }
    }
}

/// 扫描线填充（非零环绕 / 奇偶规则），带 SS×SS 超采样抗锯齿
pub fn fill_polygons(canvas: &mut Canvas, polys: &[Vec<(f32, f32)>], color: Color, even_odd: bool) {
    if polys.is_empty() {
        return;
    }
    let mut min_x = f32::MAX;
    let mut min_y = f32::MAX;
    let mut max_x = f32::MIN;
    let mut max_y = f32::MIN;
    for poly in polys {
        for (x, y) in poly {
            if !x.is_finite() || !y.is_finite() {
                return;
            }
            min_x = min_x.min(*x);
            max_x = max_x.max(*x);
            min_y = min_y.min(*y);
            max_y = max_y.max(*y);
        }
    }
    if !(min_x <= max_x && min_y <= max_y) {
        return;
    }
    let x0 = min_x.floor() as i32;
    let y0 = min_y.floor() as i32;
    let mw = ((max_x.ceil() as i32 - x0) as usize).clamp(1, 8192);
    let mh = ((max_y.ceil() as i32 - y0) as usize).clamp(1, 8192);
    // 整块落在画布外就不做
    if x0 >= canvas.w as i32 || y0 >= canvas.h as i32 || x0 + mw as i32 <= 0 || y0 + mh as i32 <= 0 {
        return;
    }

    let sw = mw * SS;
    let sh = mh * SS;
    let mut acc = vec![0u8; sw * sh];
    let mut crossings: Vec<(f32, i32)> = Vec::with_capacity(64);

    for sy in 0..sh {
        let yc = sy as f32 + 0.5;
        crossings.clear();
        for poly in polys {
            if poly.len() < 2 {
                continue;
            }
            for i in 0..poly.len() {
                let (ax, ay) = poly[i];
                let (bx, by) = poly[(i + 1) % poly.len()];
                let ay = (ay - y0 as f32) * SS as f32;
                let by = (by - y0 as f32) * SS as f32;
                if ay == by {
                    continue;
                }
                let lo = ay.min(by);
                let hi = ay.max(by);
                if yc < lo || yc >= hi {
                    continue;
                }
                let t = (yc - ay) / (by - ay);
                let ax = (ax - x0 as f32) * SS as f32;
                let bx = (bx - x0 as f32) * SS as f32;
                crossings.push((ax + t * (bx - ax), if by > ay { 1 } else { -1 }));
            }
        }
        if crossings.is_empty() {
            continue;
        }
        crossings.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

        let mut winding = 0i32;
        let mut start: Option<f32> = None;
        let row = sy * sw;
        for (idx, (x, dir)) in crossings.iter().enumerate() {
            let was_inside = if even_odd { idx % 2 == 1 } else { winding != 0 };
            winding += *dir;
            let is_inside = if even_odd { (idx + 1) % 2 == 1 } else { winding != 0 };
            if !was_inside && is_inside {
                start = Some(*x);
            } else if was_inside && !is_inside {
                if let Some(from) = start.take() {
                    paint_span(&mut acc, row, sw, from, *x);
                }
            }
        }
        if let Some(from) = start {
            paint_span(&mut acc, row, sw, from, sw as f32);
        }
    }

    let mut mask = vec![0f32; mw * mh];
    let denom = (SS * SS) as f32;
    for my in 0..mh {
        for mx in 0..mw {
            let mut sum = 0u32;
            for dy in 0..SS {
                let row = (my * SS + dy) * sw;
                for dx in 0..SS {
                    sum += acc[row + mx * SS + dx] as u32;
                }
            }
            mask[my * mw + mx] = sum as f32 / denom;
        }
    }
    canvas.blend(x0, y0, &mask, mw, mh, color);
}

fn paint_span(acc: &mut [u8], row: usize, sw: usize, from: f32, to: f32) {
    let start = from.max(0.0).min(sw as f32) as usize;
    let end = to.max(0.0).min(sw as f32) as usize;
    if start >= end {
        return;
    }
    for x in start..end {
        acc[row + x] = 1;
    }
}

fn flatten_cubic(
    p0: (f32, f32),
    p1: (f32, f32),
    p2: (f32, f32),
    p3: (f32, f32),
    out: &mut Vec<(f32, f32)>,
    depth: u8,
) {
    if depth == 0 {
        out.push(p3);
        return;
    }
    let flat = (p0.0 - 2.0 * p1.0 + p2.0).abs()
        + (p0.1 - 2.0 * p1.1 + p2.1).abs()
        + (p1.0 - 2.0 * p2.0 + p3.0).abs()
        + (p1.1 - 2.0 * p2.1 + p3.1).abs();
    if flat < 1.2 {
        out.push(p3);
        return;
    }
    let m01 = ((p0.0 + p1.0) / 2.0, (p0.1 + p1.1) / 2.0);
    let m12 = ((p1.0 + p2.0) / 2.0, (p1.1 + p2.1) / 2.0);
    let m23 = ((p2.0 + p3.0) / 2.0, (p2.1 + p3.1) / 2.0);
    let m012 = ((m01.0 + m12.0) / 2.0, (m01.1 + m12.1) / 2.0);
    let m123 = ((m12.0 + m23.0) / 2.0, (m12.1 + m23.1) / 2.0);
    let mid = ((m012.0 + m123.0) / 2.0, (m012.1 + m123.1) / 2.0);
    flatten_cubic(p0, m01, m012, mid, out, depth - 1);
    flatten_cubic(mid, m123, m23, p3, out, depth - 1);
}

fn resolve_dict<'a>(doc: &'a Document, object: &'a Object) -> Option<&'a Dictionary> {
    match object {
        Object::Dictionary(d) => Some(d),
        Object::Reference(id) => doc.get_object(*id).ok().and_then(|o| o.as_dict().ok()),
        _ => None,
    }
}

fn resolve_stream(doc: &Document, object: &Object) -> Option<Stream> {
    match object {
        Object::Stream(s) => Some(s.clone()),
        Object::Reference(id) => doc.get_object(*id).ok().and_then(|o| match o {
            Object::Stream(s) => Some(s.clone()),
            _ => None,
        }),
        _ => None,
    }
}

fn stream_data(stream: &Stream) -> Option<Vec<u8>> {
    if stream.dict.get(b"Filter").is_ok() {
        stream.decompressed_content().ok()
    } else {
        Some(stream.content.clone())
    }
}

/// PDF 数字对象可能是 Integer 也可能是 Real，统一取值
fn num_of(object: &Object) -> Option<f32> {
    match object {
        Object::Integer(i) => Some(*i as f32),
        Object::Real(r) => Some(*r),
        _ => None,
    }
}

fn load_font(doc: &Document, dict: &Dictionary) -> PdfFont {
    let subtype = dict
        .get(b"Subtype")
        .ok()
        .and_then(|o| o.as_name().ok())
        .map(|n| n.to_vec())
        .unwrap_or_default();
    let encoding = dict
        .get(b"Encoding")
        .ok()
        .and_then(|o| o.as_name().ok())
        .map(|n| n.to_vec())
        .unwrap_or_default();
    let two_byte = subtype == b"Type0" || encoding == b"Identity-H" || encoding == b"Identity-V";

    let cid_dict = if two_byte {
        dict.get(b"DescendantFonts")
            .ok()
            .and_then(|o| o.as_array().ok())
            .and_then(|a| a.first())
            .and_then(|o| resolve_dict(doc, o))
    } else {
        None
    };

    let descriptor = match cid_dict {
        Some(cid) => cid
            .get(b"FontDescriptor")
            .ok()
            .and_then(|o| resolve_dict(doc, o)),
        None => dict
            .get(b"FontDescriptor")
            .ok()
            .and_then(|o| resolve_dict(doc, o)),
    };

    let mut face = None;
    let mut cff = false;
    if let Some(desc) = descriptor {
        if let Ok(obj) = desc.get(b"FontFile2") {
            if let Some(data) = resolve_stream(doc, obj).and_then(|s| stream_data(&s)) {
                if let Ok(parsed) = FontFace::parse(&data) {
                    face = Some(parsed);
                }
            }
        } else if desc.get(b"FontFile3").is_ok() {
            cff = true;
        }
    }

    let default_width = if let Some(cid) = cid_dict {
        cid.get(b"DW")
            .ok()
            .and_then(|o| num_of(o))
            .unwrap_or(1000.0)
    } else {
        500.0
    };

    let mut widths = HashMap::new();
    if let Some(cid) = cid_dict {
        // /W 既可能直接是数组，也可能是间接引用（PyMuPDF 就写成 9 0 R）
        let w = cid.get(b"W").ok().and_then(|o| match o {
            Object::Array(a) => Some(a.clone()),
            Object::Reference(id) => doc
                .get_object(*id)
                .ok()
                .and_then(|obj| obj.as_array().ok())
                .cloned(),
            _ => None,
        });
        if let Some(w) = w {
            let mut i = 0;
            while i + 1 < w.len() {
                let start = num_of(&w[i]).map(|v| v as u32);
                let Some(start) = start else {
                    i += 1;
                    continue;
                };
                match &w[i + 1] {
                    Object::Array(items) => {
                        for (k, item) in items.iter().enumerate() {
                            if let Some(v) = num_of(item) {
                                widths.insert(start + k as u32, v);
                            }
                        }
                        i += 2;
                    }
                    other => {
                        let end = num_of(other).map(|v| v as u32);
                        let wv = w.get(i + 2).and_then(|o| num_of(o));
                        if let (Some(end), Some(wv)) = (end, wv) {
                            for g in start..=end {
                                widths.insert(g, wv);
                            }
                        }
                        i += 3;
                    }
                }
            }
        }
    } else if let (Ok(first), Ok(list)) = (
        dict.get(b"FirstChar").and_then(|o| o.as_i64()),
        dict.get(b"Widths").and_then(|o| o.as_array().map(|v| v.clone())),
    ) {
        for (k, item) in list.iter().enumerate() {
            if let Some(v) = num_of(item) {
                widths.insert((first as u32) + k as u32, v);
            }
        }
    }

    PdfFont { face, two_byte, default_width, widths, cff }
}

/// 解码图像 XObject → (宽, 高, RGB)
fn decode_image(stream: &Stream) -> Option<(usize, usize, Vec<u8>)> {
    let dict = &stream.dict;
    let width = dict.get(b"Width").ok().and_then(|o| num_of(o))? as usize;
    let height = dict.get(b"Height").ok().and_then(|o| num_of(o))? as usize;
    if width == 0 || height == 0 || width.saturating_mul(height) > 40_000_000 {
        return None;
    }
    let filter = filter_name(dict);

    if filter.contains("DCTDecode") {
        if let Ok(img) = image::load_from_memory(&stream.content) {
            let rgb = img.to_rgb8();
            return Some((rgb.width() as usize, rgb.height() as usize, rgb.into_raw()));
        }
        return None;
    }

    let data = stream_data(stream)?;
    let bpc = dict
        .get(b"BitsPerComponent")
        .ok()
        .and_then(|o| num_of(o))
        .unwrap_or(8.0) as i64;
    let cs = color_space(dict)?;
    let components = match cs {
        ColorSpace::Gray => 1,
        ColorSpace::Rgb => 3,
        ColorSpace::Cmyk => 4,
        ColorSpace::Indexed { .. } => 1,
    };
    if bpc != 8 && bpc != 1 {
        return None;
    }
    let expected = width * height * components;
    if data.len() < expected {
        return None;
    }

    let mut rgb = Vec::with_capacity(width * height * 3);
    match cs {
        ColorSpace::Indexed { base, hival, lookup } => {
            for i in 0..(width * height) {
                let idx = match bpc {
                    8 => data[i] as usize,
                    _ => return None,
                }
                .min(hival as usize);
                let comps = base_components(&base);
                let off = idx * comps;
                if off + comps <= lookup.len() && comps >= 3 {
                    rgb.push(lookup[off]);
                    rgb.push(lookup[off + 1]);
                    rgb.push(lookup[off + 2]);
                } else if off < lookup.len() {
                    let v = lookup[off];
                    rgb.push(v);
                    rgb.push(v);
                    rgb.push(v);
                } else {
                    rgb.push(0);
                    rgb.push(0);
                    rgb.push(0);
                }
            }
        }
        ColorSpace::Gray => {
            if bpc == 1 {
                for i in 0..(width * height) {
                    let byte = data[i / 8];
                    let bit = (byte >> (7 - (i % 8))) & 1;
                    let v = if bit == 1 { 0 } else { 255 };
                    rgb.push(v);
                    rgb.push(v);
                    rgb.push(v);
                }
            } else {
                for i in 0..(width * height) {
                    let v = data[i];
                    rgb.push(v);
                    rgb.push(v);
                    rgb.push(v);
                }
            }
        }
        ColorSpace::Rgb => {
            for i in 0..(width * height) {
                let off = i * 3;
                rgb.push(data[off]);
                rgb.push(data[off + 1]);
                rgb.push(data[off + 2]);
            }
        }
        ColorSpace::Cmyk => {
            for i in 0..(width * height) {
                let off = i * 4;
                let c = data[off] as f32 / 255.0;
                let m = data[off + 1] as f32 / 255.0;
                let y = data[off + 2] as f32 / 255.0;
                let k = data[off + 3] as f32 / 255.0;
                rgb.push(((1.0 - c) * (1.0 - k) * 255.0) as u8);
                rgb.push(((1.0 - m) * (1.0 - k) * 255.0) as u8);
                rgb.push(((1.0 - y) * (1.0 - k) * 255.0) as u8);
            }
        }
    }
    Some((width, height, rgb))
}

fn base_components(cs: &ColorSpace) -> usize {
    match cs {
        ColorSpace::Gray => 1,
        ColorSpace::Rgb => 3,
        ColorSpace::Cmyk => 4,
        ColorSpace::Indexed { base, .. } => base_components(base),
    }
}

enum ColorSpace {
    Gray,
    Rgb,
    Cmyk,
    Indexed { base: Box<ColorSpace>, hival: i64, lookup: Vec<u8> },
}

fn color_space(dict: &Dictionary) -> Option<ColorSpace> {
    let object = dict.get(b"ColorSpace").ok()?;
    color_space_from(object, dict)
}

fn color_space_from(object: &Object, _dict: &Dictionary) -> Option<ColorSpace> {
    match object {
        Object::Name(name) => match name.as_slice() {
            b"DeviceGray" | b"G" | b"CalGray" => Some(ColorSpace::Gray),
            b"DeviceRGB" | b"RGB" | b"CalRGB" => Some(ColorSpace::Rgb),
            b"DeviceCMYK" | b"CMYK" => Some(ColorSpace::Cmyk),
            _ => Some(ColorSpace::Rgb),
        },
        Object::Array(items) => {
            let first = items.first()?.as_name().ok()?.to_vec();
            match first.as_slice() {
                b"Indexed" | b"I" => {
                    let base = items.get(1).and_then(|o| color_space_from(o, _dict))?;
                    let hival = num_of(items.get(2)?)? as i64;
                    let lookup = items.get(3)?;
                    let bytes = match lookup {
                        Object::String(s, _) => s.clone(),
                        Object::Stream(_) => Vec::new(),
                        _ => Vec::new(),
                    };
                    Some(ColorSpace::Indexed { base: Box::new(base), hival, lookup: bytes })
                }
                b"ICCBased" => {
                    // ICCBased: /N 分量数决定灰/彩
                    Some(ColorSpace::Rgb)
                }
                _ => Some(ColorSpace::Rgb),
            }
        }
        _ => Some(ColorSpace::Rgb),
    }
}

fn filter_name(dict: &Dictionary) -> String {
    dict.get(b"Filter")
        .ok()
        .map(|o| match o {
            Object::Name(n) => String::from_utf8_lossy(n).to_string(),
            Object::Array(arr) => arr
                .iter()
                .map(|o| {
                    o.as_name()
                        .map(|n| String::from_utf8_lossy(n).to_string())
                        .unwrap_or_default()
                })
                .collect::<Vec<_>>()
                .join(","),
            _ => String::new(),
        })
        .unwrap_or_default()
}

struct Rasterizer {
    canvas: Canvas,
    scale: f32,
    page_h: f32,
    fonts: Vec<PdfFont>,
}

impl Rasterizer {
    fn dev(&self, x: f32, y: f32) -> (f32, f32) {
        (x * self.scale, (self.page_h - y) * self.scale)
    }

    /// 用页面空间坐标多边形填充
    fn fill_page_space(&mut self, polys: &[Vec<(f32, f32)>], color: Color, even_odd: bool) {
        let mapped: Vec<Vec<(f32, f32)>> = polys
            .iter()
            .map(|poly| poly.iter().map(|(x, y)| self.dev(*x, *y)).collect())
            .collect();
        fill_polygons(&mut self.canvas, &mapped, color, even_odd);
    }

    fn fill_device(&mut self, polys: &[Vec<(f32, f32)>], color: Color, even_odd: bool) {
        fill_polygons(&mut self.canvas, polys, color, even_odd);
    }

    fn draw_image(&mut self, stream: &Stream, ctm: &Mat) {
        let Some((iw, ih, rgb)) = decode_image(stream) else {
            return;
        };
        if iw == 0 || ih == 0 {
            return;
        }
        // 单位方块 → 设备坐标
        let corners = [
            ctm.apply(0.0, 0.0),
            ctm.apply(1.0, 0.0),
            ctm.apply(1.0, 1.0),
            ctm.apply(0.0, 1.0),
        ];
        let mapped: Vec<(f32, f32)> = corners
            .iter()
            .map(|(x, y)| self.dev(*x, *y))
            .collect();
        let min_x = mapped.iter().map(|p| p.0).fold(f32::MAX, f32::min).floor() as i32;
        let max_x = mapped.iter().map(|p| p.0).fold(f32::MIN, f32::max).ceil() as i32;
        let min_y = mapped.iter().map(|p| p.1).fold(f32::MAX, f32::min).floor() as i32;
        let max_y = mapped.iter().map(|p| p.1).fold(f32::MIN, f32::max).ceil() as i32;
        if max_x <= min_x || max_y <= min_y {
            return;
        }
        // 反向映射：设备像素 → 单位方块 uv
        let p0 = mapped[0];
        let px = (mapped[1].0 - p0.0, mapped[1].1 - p0.1);
        let py = (mapped[3].0 - p0.0, mapped[3].1 - p0.1);
        let det = px.0 * py.1 - px.1 * py.0;
        if det.abs() < 1e-6 {
            return;
        }

        for dy in min_y..max_y {
            if dy < 0 || dy >= self.canvas.h as i32 {
                continue;
            }
            for dx in min_x..max_x {
                if dx < 0 || dx >= self.canvas.w as i32 {
                    continue;
                }
                let r = (dx as f32 + 0.5 - p0.0, dy as f32 + 0.5 - p0.1);
                let u = (r.0 * py.1 - r.1 * py.0) / det;
                let v = (px.0 * r.1 - px.1 * r.0) / det;
                if !(0.0..=1.0).contains(&u) || !(0.0..=1.0).contains(&v) {
                    continue;
                }
                let sx = ((u * iw as f32) as usize).min(iw - 1);
                let sy = ((v * ih as f32) as usize).min(ih - 1);
                let off = (sy * iw + sx) * 3;
                let idx = ((dy as usize) * self.canvas.w + dx as usize) * 4;
                self.canvas.pixels[idx] = rgb[off];
                self.canvas.pixels[idx + 1] = rgb[off + 1];
                self.canvas.pixels[idx + 2] = rgb[off + 2];
            }
        }
    }
}

pub struct RenderStats {
    pub pages: usize,
    pub cff_pages: usize,
}

/// 逐页渲染；回调拿到 RGBA 画布
pub fn render_pages<F>(
    src: &std::path::Path,
    dpi: f32,
    cancelled: &dyn Fn() -> bool,
    mut sink: F,
) -> Result<RenderStats, String>
where
    F: FnMut(u32, usize, usize, Vec<u8>) -> Result<(), String>,
{
    let doc = Document::load(src).map_err(|e| format!("无法解析该 PDF：{}", e))?;
    if doc.is_encrypted() {
        return Err("该 PDF 已加密，暂不支持渲染".to_string());
    }
    let scale = (dpi / 72.0).max(0.5);
    let pages = doc.get_pages();
    let mut stats = RenderStats { pages: 0, cff_pages: 0 };

    for (number, page_id) in pages {
        if cancelled() {
            return Err("已取消".to_string());
        }
        let page_dict = doc
            .get_object(page_id)
            .ok()
            .and_then(|o| o.as_dict().ok())
            .ok_or("页面对象读取失败")?
            .clone();
        let media = page_dict
            .get(b"MediaBox")
            .ok()
            .and_then(|o| o.as_array().ok())
            .map(|a| {
                (
                    a.first().and_then(|o| num_of(o)).unwrap_or(595.0),
                    a.get(1).and_then(|o| num_of(o)).unwrap_or(0.0),
                    a.get(2).and_then(|o| num_of(o)).unwrap_or(595.0),
                    a.get(3).and_then(|o| num_of(o)).unwrap_or(842.0),
                )
            })
            .unwrap_or((0.0, 0.0, 595.0, 842.0));
        let (x0, y0, x1, y1) = media;
        let bw = (x1 - x0).abs().max(1.0);
        let bh = (y1 - y0).abs().max(1.0);
        let cw = ((bw * scale).ceil() as usize).clamp(8, 20_000);
        let ch = ((bh * scale).ceil() as usize).clamp(8, 20_000);

        let mut canvas = Canvas::new(cw, ch);
        let mut fonts: Vec<PdfFont> = Vec::new();
        let mut cff_pages = false;

        // 页面坐标系先平移到 MediaBox 原点，再把 y 翻到图像坐标
        let base = Mat {
            a: 1.0,
            b: 0.0,
            c: 0.0,
            d: 1.0,
            e: -x0,
            f: -y0,
        };
        {
            let mut rast = Rasterizer {
                canvas: Canvas::new(1, 1),
                scale,
                page_h: y1 - y0,
                fonts: Vec::new(),
            };
            std::mem::swap(&mut rast.canvas, &mut canvas);
            rast.fonts = std::mem::take(&mut fonts);
            run_page(&doc, &page_dict, &mut rast, base, &mut cff_pages)?;
            std::mem::swap(&mut rast.canvas, &mut canvas);
        }

        stats.pages += 1;
        if cff_pages {
            stats.cff_pages += 1;
        }
        sink(number, cw, ch, canvas.pixels)?;
    }
    Ok(stats)
}

fn run_page(
    doc: &Document,
    page_dict: &Dictionary,
    rast: &mut Rasterizer,
    base: Mat,
    cff_pages: &mut bool,
) -> Result<(), String> {
    let resources = page_dict
        .get(b"Resources")
        .ok()
        .and_then(|o| resolve_dict(doc, o))
        .cloned()
        .unwrap_or_default();
    let content = page_dict
        .get(b"Contents")
        .ok()
        .and_then(|o| match o {
            Object::Reference(id) => doc.get_object(*id).ok(),
            other => Some(other),
        })
        .map(|o| match o {
            Object::Stream(s) => stream_data(s).unwrap_or_default(),
            Object::Array(items) => {
                let mut buf = Vec::new();
                for item in items {
                    if let Some(stream) = resolve_stream(doc, item) {
                        if let Some(mut data) = stream_data(&stream) {
                            buf.append(&mut data);
                            buf.push(b'\n');
                        }
                    }
                }
                buf
            }
            _ => Vec::new(),
        })
        .unwrap_or_default();

    let mut ctx = Context::new(doc, rast, resources, base, 0);
    ctx.run(&content, cff_pages);
    Ok(())
}

struct Context<'a, 'b> {
    doc: &'a Document,
    rast: &'b mut Rasterizer,
    resources: Dictionary,
    stack: Vec<State>,
    state: State,
    // 路径
    polys: Vec<Vec<(f32, f32)>>,
    current: Vec<(f32, f32)>,
    cur: (f32, f32),
    start: (f32, f32),
    depth: u8,
}

impl<'a, 'b> Context<'a, 'b> {
    fn new(
        doc: &'a Document,
        rast: &'b mut Rasterizer,
        resources: Dictionary,
        base: Mat,
        depth: u8,
    ) -> Context<'a, 'b> {
        Context {
            doc,
            rast,
            resources,
            stack: Vec::new(),
            state: State::new(base),
            polys: Vec::new(),
            current: Vec::new(),
            cur: (0.0, 0.0),
            start: (0.0, 0.0),
            depth,
        }
    }

    fn set_color(&mut self, components: &[f32], stroke: bool) {
        let color = match components.len() {
            0 => Color { r: 0.0, g: 0.0, b: 0.0 },
            1 => Color { r: components[0], g: components[0], b: components[0] },
            3 => Color { r: components[0], g: components[1], b: components[2] },
            _ => {
                let c = components[0];
                let m = components[1];
                let y = components[2];
                let k = components[3];
                Color {
                    r: (1.0 - c) * (1.0 - k),
                    g: (1.0 - m) * (1.0 - k),
                    b: (1.0 - y) * (1.0 - k),
                }
            }
        };
        if stroke {
            self.state.stroke = color;
        } else {
            self.state.fill = color;
        }
    }

    fn path_move(&mut self, x: f32, y: f32) {
        self.finish_subpath();
        let p = self.state.ctm.apply(x, y);
        self.cur = p;
        self.start = p;
        self.current = vec![p];
    }

    fn path_line(&mut self, x: f32, y: f32) {
        if self.current.is_empty() {
            self.path_move(x, y);
            return;
        }
        let p = self.state.ctm.apply(x, y);
        self.cur = p;
        self.current.push(p);
    }

    fn path_curve(&mut self, c1: (f32, f32), c2: (f32, f32), p: (f32, f32)) {
        if self.current.is_empty() {
            self.path_move(c1.0, c1.1);
        }
        let p0 = self.cur;
        let p1 = self.state.ctm.apply(c1.0, c1.1);
        let p2 = self.state.ctm.apply(c2.0, c2.1);
        let p3 = self.state.ctm.apply(p.0, p.1);
        flatten_cubic(p0, p1, p2, p3, &mut self.current, 4);
        self.cur = p3;
    }

    fn path_close(&mut self) {
        if !self.current.is_empty() {
            self.current.push(self.start);
        }
        self.finish_subpath();
        self.cur = self.start;
    }

    fn path_rect(&mut self, x: f32, y: f32, w: f32, h: f32) {
        self.finish_subpath();
        let p0 = self.state.ctm.apply(x, y);
        let p1 = self.state.ctm.apply(x + w, y);
        let p2 = self.state.ctm.apply(x + w, y + h);
        let p3 = self.state.ctm.apply(x, y + h);
        self.polys.push(vec![p0, p1, p2, p3, p0]);
        self.cur = p0;
        self.start = p0;
        self.current.clear();
    }

    fn finish_subpath(&mut self) {
        if self.current.len() >= 2 {
            let closed = self.current.first() == self.current.last();
            let mut path = std::mem::take(&mut self.current);
            if !closed {
                path.push(self.start);
            }
            self.polys.push(path);
        } else {
            self.current.clear();
        }
    }

    fn paint_path(&mut self, fill: bool, stroke: bool, even_odd: bool) {
        self.finish_subpath();
        let polys = std::mem::take(&mut self.polys);
        if fill && !polys.is_empty() {
            // 路径点是页面空间坐标，fill_page_space 会做 scale + y 翻转
            let color = self.state.fill;
            self.rast.fill_page_space(&polys, color, even_odd);
        }
        if stroke && self.state.lw > 0.0 {
            let color = self.state.stroke;
            let half = (self.state.lw * self.rast.scale / 2.0).max(0.35);
            let mut strokes: Vec<Vec<(f32, f32)>> = Vec::new();
            for poly in polys.iter() {
                for i in 0..poly.len().saturating_sub(1) {
                    let (ax, ay) = poly[i];
                    let (bx, by) = poly[i + 1];
                    let dx = bx - ax;
                    let dy = by - ay;
                    let len = (dx * dx + dy * dy).sqrt();
                    if len < 1e-6 {
                        continue;
                    }
                    let nx = -dy / len * half;
                    let ny = dx / len * half;
                    strokes.push(vec![
                        (ax + nx, ay + ny),
                        (bx + nx, by + ny),
                        (bx - nx, by - ny),
                        (ax - nx, ay - ny),
                        (ax + nx, ay + ny),
                    ]);
                }
            }
            self.rast.fill_page_space(&strokes, color, false);
        }
    }

    fn show_text(&mut self, bytes: &[u8], tm: &mut Mat, cff_pages: &mut bool) -> f32 {
        let Some(font_index) = self.state.font else {
            return 0.0;
        };
        if self.rast.fonts[font_index].cff {
            *cff_pages = true;
            return 0.0;
        }
        let two_byte = self.rast.fonts[font_index].two_byte;

        let mut codes: Vec<u32> = Vec::new();
        if two_byte {
            let mut i = 0;
            while i + 1 < bytes.len() {
                codes.push(((bytes[i] as u32) << 8) | bytes[i + 1] as u32);
                i += 2;
            }
        } else {
            for b in bytes {
                codes.push(*b as u32);
            }
        }

        let size = self.state.size;
        let th = self.state.th;
        let tc = self.state.tc;
        let tw = self.state.tw;
        let rise = self.state.rise;
        let ctm = self.state.ctm;
        let fill = self.state.fill;
        let visible = self.state.render != 3 && self.state.render != 7;
        let dev_scale = self.rast.scale;
        let page_h = self.rast.page_h;
        let mut advance = 0.0f32;

        for code in codes {
            // 先在不可变借用内把字形轮廓与字宽取出来
            let (width, contours) = {
                let font = &self.rast.fonts[font_index];
                let width = font.width(code) / 1000.0;
                let query = if visible {
                    font.face.as_ref().map(|face| {
                        let gid = if font.two_byte {
                            code as u16
                        } else {
                            face.gid(char::from_u32(code).unwrap_or(' '))
                        };
                        // 字形坐标是字体单位，除以 units_per_em 折成 1em，字号再由 trm 缩放
                        let unit = 1.0 / face.units_per_em as f32;
                        face.outline(gid)
                            .map(|contours| (contours, unit))
                    })
                } else {
                    None
                };
                (width, query.flatten())
            };

            if let Some((contours, unit)) = contours {
                let trm = Mat {
                    a: size * th,
                    b: 0.0,
                    c: 0.0,
                    d: size,
                    e: 0.0,
                    f: rise,
                }
                .mul(tm)
                .mul(&ctm);
                let mut dev_polys: Vec<Vec<(f32, f32)>> = Vec::with_capacity(contours.len());
                for contour in contours {
                    let mut poly: Vec<(f32, f32)> = Vec::with_capacity(contour.len() + 1);
                    for (gx, gy) in contour.iter() {
                        let tx = gx * unit;
                        let ty = gy * unit;
                        let (px, py) = trm.apply(tx, ty);
                        poly.push((px * dev_scale, (page_h - py) * dev_scale));
                    }
                    if let Some(first) = poly.first().copied() {
                        poly.push(first);
                    }
                    dev_polys.push(poly);
                }
                self.rast.fill_device(&dev_polys, fill, false);
            }

            let space_extra = if code == 32 { tw } else { 0.0 };
            let tx = (width * size + tc + space_extra) * th;
            *tm = Mat::translate(tx, 0.0).mul(tm);
            advance += tx;
        }
        advance
    }

    fn run(&mut self, content: &[u8], cff_pages: &mut bool) {
        let tokens = tokenize(content);
        let mut operands: Vec<Tok> = Vec::new();
        let mut tm = Mat::identity();
        let mut tlm = Mat::identity();

        for tok in tokens {
            match tok {
                Tok::Op(op) => {
                    match op.as_str() {
                        "q" => self.stack.push(self.state.clone()),
                        "Q" => {
                            if let Some(prev) = self.stack.pop() {
                                self.state = prev;
                            }
                        }
                        "cm" => {
                            if operands.len() >= 6 {
                                if let Some(m) = matrix_from(&operands) {
                                    self.state.ctm = m.mul(&self.state.ctm);
                                }
                            }
                        }
                        "w" => {
                            if let Some(v) = first_num(&operands) {
                                self.state.lw = v;
                            }
                        }
                        "g" => {
                            let c = vec![first_num(&operands).unwrap_or(0.0)];
                            self.set_color(&c, false);
                        }
                        "G" => {
                            let c = vec![first_num(&operands).unwrap_or(0.0)];
                            self.set_color(&c, true);
                        }
                        "rg" => {
                            let c = nums(&operands);
                            self.set_color(&c, false);
                        }
                        "RG" => {
                            let c = nums(&operands);
                            self.set_color(&c, true);
                        }
                        "k" => {
                            let c = nums(&operands);
                            self.set_color(&c, false);
                        }
                        "K" => {
                            let c = nums(&operands);
                            self.set_color(&c, true);
                        }
                        "sc" | "scn" => {
                            let c = nums(&operands);
                            self.set_color(&c, false);
                        }
                        "SC" | "SCN" => {
                            let c = nums(&operands);
                            self.set_color(&c, true);
                        }
                        "m" => {
                            let n = nums(&operands);
                            if n.len() >= 2 {
                                self.path_move(n[0], n[1]);
                            }
                        }
                        "l" => {
                            let n = nums(&operands);
                            if n.len() >= 2 {
                                self.path_line(n[0], n[1]);
                            }
                        }
                        "c" => {
                            let n = nums(&operands);
                            if n.len() >= 6 {
                                self.path_curve((n[0], n[1]), (n[2], n[3]), (n[4], n[5]));
                            }
                        }
                        "v" => {
                            let n = nums(&operands);
                            if n.len() >= 4 {
                                let p0 = self.cur;
                                self.path_curve(p0, (n[0], n[1]), (n[2], n[3]));
                            }
                        }
                        "y" => {
                            let n = nums(&operands);
                            if n.len() >= 4 {
                                self.path_curve((n[0], n[1]), (n[2], n[3]), (n[2], n[3]));
                            }
                        }
                        "h" => self.path_close(),
                        "re" => {
                            let n = nums(&operands);
                            if n.len() >= 4 {
                                self.path_rect(n[0], n[1], n[2], n[3]);
                            }
                        }
                        "f" | "F" => self.paint_path(true, false, false),
                        "f*" => self.paint_path(true, false, true),
                        "S" => self.paint_path(false, true, false),
                        "s" => {
                            self.path_close();
                            self.paint_path(false, true, false);
                        }
                        "B" => self.paint_path(true, true, false),
                        "B*" => self.paint_path(true, true, true),
                        "b" | "b*" => {
                            self.path_close();
                            self.paint_path(true, true, false);
                        }
                        "n" => {
                            self.finish_subpath();
                            self.polys.clear();
                        }
                        "BT" => {
                            tm = Mat::identity();
                            tlm = Mat::identity();
                        }
                        "ET" => {}
                        "Tf" => {
                            let name = operands.iter().find_map(|t| match t {
                                Tok::Name(n) => Some(n.clone()),
                                _ => None,
                            });
                            let size = operands.iter().find_map(|t| match t {
                                Tok::Num(v) => Some(*v),
                                _ => None,
                            });
                            if let Some(name) = name {
                                self.state.font = self.resolve_font(&name, cff_pages);
                            }
                            if let Some(size) = size {
                                self.state.size = size;
                            }
                        }
                        "Tc" => self.state.tc = first_num(&operands).unwrap_or(0.0),
                        "Tw" => self.state.tw = first_num(&operands).unwrap_or(0.0),
                        "Tz" => self.state.th = first_num(&operands).unwrap_or(100.0) / 100.0,
                        "TL" => self.state.tl = first_num(&operands).unwrap_or(0.0),
                        "Ts" => self.state.rise = first_num(&operands).unwrap_or(0.0),
                        "Tr" => self.state.render = first_num(&operands).unwrap_or(0.0) as i32,
                        "Td" => {
                            let n = nums(&operands);
                            if n.len() >= 2 {
                                tlm = Mat::translate(n[0], n[1]).mul(&tlm);
                                tm = tlm;
                            }
                        }
                        "TD" => {
                            let n = nums(&operands);
                            if n.len() >= 2 {
                                self.state.tl = -n[1];
                                tlm = Mat::translate(n[0], n[1]).mul(&tlm);
                                tm = tlm;
                            }
                        }
                        "Tm" => {
                            if let Some(m) = matrix_from(&operands) {
                                tm = m;
                                tlm = m;
                            }
                        }
                        "T*" => {
                            tlm = Mat::translate(0.0, -self.state.tl).mul(&tlm);
                            tm = tlm;
                        }
                        "Tj" | "'" | "\"" => {
                            if op != "Tj" {
                                tlm = Mat::translate(0.0, -self.state.tl).mul(&tlm);
                                tm = tlm;
                            }
                            let strings: Vec<Vec<u8>> = operands
                                .iter()
                                .filter_map(|t| match t {
                                    Tok::Str(s) => Some(s.clone()),
                                    _ => None,
                                })
                                .collect();
                            for s in strings {
                                self.show_text(&s, &mut tm, cff_pages);
                            }
                        }
                        "TJ" => {
                            for t in operands.iter() {
                                match t {
                                    Tok::Str(s) => {
                                        let s = s.clone();
                                        self.show_text(&s, &mut tm, cff_pages);
                                    }
                                    Tok::Num(v) => {
                                        let tx = -v / 1000.0 * self.state.size * self.state.th;
                                        tm = Mat::translate(tx, 0.0).mul(&tm);
                                    }
                                    _ => {}
                                }
                            }
                        }
                        "Do" => {
                            let name = operands.iter().find_map(|t| match t {
                                Tok::Name(n) => Some(n.clone()),
                                _ => None,
                            });
                            if let Some(name) = name {
                                self.do_xobject(&name, cff_pages);
                            }
                        }
                        "gs" | "d" | "j" | "J" | "M" | "ri" | "i" | "cs" | "CS" | "sh" => {}
                        _ => {}
                    }
                    operands.clear();
                }
                other => operands.push(other),
            }
        }
    }

    fn resolve_font(&mut self, name: &[u8], cff_pages: &mut bool) -> Option<usize> {
        let fonts = self
            .resources
            .get(b"Font")
            .ok()
            .and_then(|o| resolve_dict(self.doc, o))?;
        let dict = fonts.get(name).ok().and_then(|o| resolve_dict(self.doc, o))?;
        let font = load_font(self.doc, dict);
        if font.cff {
            *cff_pages = true;
        }
        self.rast.fonts.push(font);
        Some(self.rast.fonts.len() - 1)
    }

    fn do_xobject(&mut self, name: &[u8], cff_pages: &mut bool) {
        if self.depth >= 4 {
            return;
        }
        let xobjects = self
            .resources
            .get(b"XObject")
            .ok()
            .and_then(|o| resolve_dict(self.doc, o));
        let Some(xobjects) = xobjects else { return };
        let object = xobjects.get(name).ok();
        let Some(object) = object else { return };
        let stream = match resolve_stream(self.doc, object) {
            Some(s) => s,
            None => return,
        };
        let subtype = stream
            .dict
            .get(b"Subtype")
            .ok()
            .and_then(|o| o.as_name().ok())
            .map(|n| n.to_vec())
            .unwrap_or_default();
        if subtype == b"Image" {
            self.rast.draw_image(&stream, &self.state.ctm);
            return;
        }
        if subtype != b"Form" {
            return;
        }
        let matrix = stream
            .dict
            .get(b"Matrix")
            .ok()
            .and_then(|o| o.as_array().ok())
            .and_then(|a| {
                if a.len() >= 6 {
                    Some(Mat {
                        a: num_of(&a[0]).unwrap_or(1.0),
                        b: num_of(&a[1]).unwrap_or(0.0),
                        c: num_of(&a[2]).unwrap_or(0.0),
                        d: num_of(&a[3]).unwrap_or(1.0),
                        e: num_of(&a[4]).unwrap_or(0.0),
                        f: num_of(&a[5]).unwrap_or(0.0),
                    })
                } else {
                    None
                }
            })
            .unwrap_or_else(Mat::identity);
        let resources = stream
            .dict
            .get(b"Resources")
            .ok()
            .and_then(|o| resolve_dict(self.doc, o))
            .cloned()
            .unwrap_or_else(|| self.resources.clone());
        let data = stream_data(&stream).unwrap_or_default();
        let ctm = self.state.ctm.mul(&matrix);
        let mut sub = Context {
            doc: self.doc,
            rast: self.rast,
            resources,
            stack: Vec::new(),
            state: State {
                ctm,
                ..self.state.clone()
            },
            polys: Vec::new(),
            current: Vec::new(),
            cur: (0.0, 0.0),
            start: (0.0, 0.0),
            depth: self.depth + 1,
        };
        sub.run(&data, cff_pages);
    }
}

fn nums(operands: &[Tok]) -> Vec<f32> {
    operands
        .iter()
        .filter_map(|t| match t {
            Tok::Num(v) => Some(*v),
            _ => None,
        })
        .collect()
}

fn first_num(operands: &[Tok]) -> Option<f32> {
    operands.iter().find_map(|t| match t {
        Tok::Num(v) => Some(*v),
        _ => None,
    })
}

fn matrix_from(operands: &[Tok]) -> Option<Mat> {
    let n = nums(operands);
    if n.len() < 6 {
        return None;
    }
    Some(Mat { a: n[0], b: n[1], c: n[2], d: n[3], e: n[4], f: n[5] })
}

fn tokenize(content: &[u8]) -> Vec<Tok> {
    let mut out = Vec::new();
    let mut i = 0usize;
    let len = content.len();
    while i < len {
        let b = content[i];
        match b {
            b' ' | b'\n' | b'\r' | b'\t' | 0 | 0x0C => i += 1,
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
                out.push(Tok::Str(bytes));
            }
            b'<' => {
                if i + 1 < len && content[i + 1] == b'<' {
                    i += 2;
                    out.push(Tok::DictStart);
                    continue;
                }
                i += 1;
                let mut hex = String::new();
                while i < len && content[i] != b'>' {
                    if !content[i].is_ascii_whitespace() {
                        hex.push(content[i] as char);
                    }
                    i += 1;
                }
                i += 1;
                out.push(Tok::Str(hex_bytes(&hex)));
            }
            b'>' => {
                if i + 1 < len && content[i + 1] == b'>' {
                    i += 2;
                    out.push(Tok::DictEnd);
                } else {
                    i += 1;
                }
            }
            b'[' => {
                i += 1;
                out.push(Tok::ArrStart);
            }
            b']' => {
                i += 1;
                out.push(Tok::ArrEnd);
            }
            b'/' => {
                i += 1;
                let mut name = Vec::new();
                while i < len && !delimiter(content[i]) {
                    name.push(content[i]);
                    i += 1;
                }
                out.push(Tok::Name(name));
            }
            b'B' if i + 1 < len && content[i + 1] == b'I' => {
                i += 2;
                while i + 1 < len {
                    if content[i] == b'E' && content[i + 1] == b'I' {
                        i += 2;
                        break;
                    }
                    i += 1;
                }
            }
            b')' | b'{' | b'}' => i += 1,
            _ => {
                let mut token = Vec::new();
                while i < len && !delimiter(content[i]) {
                    token.push(content[i]);
                    i += 1;
                }
                if token.is_empty() {
                    i += 1;
                    continue;
                }
                let text = String::from_utf8_lossy(&token).to_string();
                let numeric = token
                    .first()
                    .map(|c| c.is_ascii_digit() || *c == b'-' || *c == b'+' || *c == b'.')
                    .unwrap_or(false);
                if numeric {
                    out.push(Tok::Num(text.parse().unwrap_or(0.0)));
                } else {
                    out.push(Tok::Op(text));
                }
            }
        }
    }
    out
}

fn delimiter(c: u8) -> bool {
    matches!(
        c,
        b' ' | b'\n' | b'\r' | b'\t' | 0 | 0x0C | b'/' | b'[' | b']' | b'<' | b'>' | b'(' | b')' | b'%'
    )
}

fn hex_bytes(hex: &str) -> Vec<u8> {
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

pub fn page_count(src: &std::path::Path) -> Result<usize, String> {
    let doc = Document::load(src).map_err(|e| format!("无法解析该 PDF：{}", e))?;
    Ok(doc.get_pages().len())
}

/// 第一页的宽度与高度（pt），用来给幻灯片 / 图片定版面
pub fn page_size(src: &std::path::Path) -> Result<(f32, f32), String> {
    let doc = Document::load(src).map_err(|e| format!("无法解析该 PDF：{}", e))?;
    let pages = doc.get_pages();
    let Some((_, page_id)) = pages.iter().next() else {
        return Err("该 PDF 没有页面".to_string());
    };
    let dict = doc
        .get_object(*page_id)
        .ok()
        .and_then(|o| o.as_dict().ok())
        .ok_or("页面对象读取失败")?;
    let media = dict
        .get(b"MediaBox")
        .ok()
        .and_then(|o| o.as_array().ok());
    let (x0, y0, x1, y1) = match media {
        Some(a) if a.len() >= 4 => (
            num_of(&a[0]).unwrap_or(0.0),
            num_of(&a[1]).unwrap_or(0.0),
            num_of(&a[2]).unwrap_or(595.0),
            num_of(&a[3]).unwrap_or(842.0),
        ),
        _ => (0.0, 0.0, 595.0, 842.0),
    };
    Ok(((x1 - x0).abs().max(1.0), (y1 - y0).abs().max(1.0)))
}
