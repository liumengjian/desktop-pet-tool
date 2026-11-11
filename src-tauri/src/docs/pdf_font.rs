// TrueType 解析 + 子集化 + PDF 内嵌字体。
//
// 只用窗口里自带的那套字体并不可控，所以字库随包分发（Noto Sans SC，OFL 授权）。
// 子集化策略：保留原 GID 编号不变（PDF 用 Identity-H，CID 就是 GID），只把没用到
// 的字形轮廓清空，cmap/hmtx/hhea/maxp 原样复制。这样既不用重编 cmap，也不会因为
// 复合字形引用被抽掉而缺字。

use std::collections::HashSet;
use std::sync::OnceLock;

const FONT_BYTES: &[u8] = include_bytes!("../../assets/NotoSansSC.ttf");

#[derive(Clone, Copy, PartialEq, Eq)]
enum CmapFormat {
    Fmt4,
    Fmt12,
    None,
}

pub struct FontFace {
    pub units_per_em: u16,
    pub ascent: i16,
    pub descent: i16,
    pub cap_height: i16,
    bbox: [i16; 4],
    num_glyphs: u16,
    advances: Vec<u16>,
    glyf: Vec<u8>,
    loca: Vec<u32>,
    cmap_sub: Vec<u8>,
    cmap_format: CmapFormat,
    tables: Vec<(String, Vec<u8>)>,
}

fn u16_at(b: &[u8], off: usize) -> u16 {
    u16::from_be_bytes([b[off], b[off + 1]])
}

/// 二次贝塞尔离散成折线（不做精细容差控制，够用即可）
fn flatten_quad_basic(
    p0: (f32, f32),
    c: (f32, f32),
    p1: (f32, f32),
    out: &mut Vec<(f32, f32)>,
) {
    let steps = 6;
    for i in 1..=steps {
        let t = i as f32 / steps as f32;
        let mt = 1.0 - t;
        let x = mt * mt * p0.0 + 2.0 * mt * t * c.0 + t * t * p1.0;
        let y = mt * mt * p0.1 + 2.0 * mt * t * c.1 + t * t * p1.1;
        out.push((x, y));
    }
}fn i16_at(b: &[u8], off: usize) -> i16 {
    u16_at(b, off) as i16
}
fn u32_at(b: &[u8], off: usize) -> u32 {
    u32::from_be_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

impl FontFace {
    pub fn parse(bytes: &[u8]) -> Result<FontFace, String> {
        if bytes.len() < 12 {
            return Err("字体文件过短".to_string());
        }
        let tag = u32_at(bytes, 0);
        // 0x00010000 = TrueType 轮廓；'true' / 'OTTO' 分别代表旧式与 CFF，CFF 不在本实现范围
        if tag != 0x0001_0000 {
            return Err(format!(
                "字体不是 TrueType 轮廓（sfnt tag = {:#010x}）",
                tag
            ));
        }
        let num_tables = u16_at(bytes, 4) as usize;

        let mut tables: Vec<(String, Vec<u8>)> = Vec::new();
        let mut head = None;
        let mut hhea = None;
        let mut maxp = None;
        let mut hmtx = None;
        let mut cmap_raw = None;
        let mut os2 = None;

        for i in 0..num_tables {
            let base = 12 + i * 16;
            if base + 16 > bytes.len() {
                return Err("字体表目录越界".to_string());
            }
            let name_bytes = &bytes[base..base + 4];
            let name = String::from_utf8_lossy(name_bytes).to_string();
            let offset = u32_at(bytes, base + 8) as usize;
            let length = u32_at(bytes, base + 12) as usize;
            if offset + length > bytes.len() {
                return Err(format!("字体表 {} 越界", name));
            }
            let data = bytes[offset..offset + length].to_vec();
            match name.as_str() {
                "head" => head = Some(data.clone()),
                "hhea" => hhea = Some(data.clone()),
                "maxp" => maxp = Some(data.clone()),
                "hmtx" => hmtx = Some(data.clone()),
                "cmap" => cmap_raw = Some(data.clone()),
                "OS/2" => os2 = Some(data.clone()),
                "glyf" | "loca" => {}
                _ => {}
            }
            tables.push((name, data));
        }

        let head = head.ok_or("字体缺少 head 表")?;
        let hhea = hhea.ok_or("字体缺少 hhea 表")?;
        let maxp = maxp.ok_or("字体缺少 maxp 表")?;
        let hmtx = hmtx.ok_or("字体缺少 hmtx 表")?;
        let cmap_raw = cmap_raw.ok_or("字体缺少 cmap 表")?;

        let units_per_em = u16_at(&head, 18);
        if units_per_em == 0 {
            return Err("head.unitsPerEm 为 0".to_string());
        }
        let bbox = [
            i16_at(&head, 36),
            i16_at(&head, 38),
            i16_at(&head, 40),
            i16_at(&head, 42),
        ];
        let long_loca = i16_at(&head, 50) != 0;
        let ascent = i16_at(&hhea, 4);
        let descent = i16_at(&hhea, 6);
        let num_hmetrics = u16_at(&hhea, 34) as usize;
        let num_glyphs = u16_at(&maxp, 4);

        let cap_height = if let Some(os2) = &os2 {
            if os2.len() >= 90 && u16_at(os2, 0) >= 2 {
                let v = i16_at(os2, 88);
                if v > 0 {
                    v
                } else {
                    ascent
                }
            } else {
                ascent
            }
        } else {
            ascent
        };

        // 字形偏移
        let loca_table = tables
            .iter()
            .find(|(n, _)| n == "loca")
            .map(|(_, d)| d.clone())
            .ok_or("字体缺少 loca 表")?;
        let mut loca = Vec::with_capacity(num_glyphs as usize + 1);
        for i in 0..=num_glyphs as usize {
            let off = if long_loca {
                if i * 4 + 4 > loca_table.len() {
                    0
                } else {
                    u32_at(&loca_table, i * 4)
                }
            } else if i * 2 + 2 > loca_table.len() {
                0
            } else {
                u16_at(&loca_table, i * 2) as u32 * 2
            };
            loca.push(off);
        }
        let glyf = tables
            .iter()
            .find(|(n, _)| n == "glyf")
            .map(|(_, d)| d.clone())
            .ok_or("字体缺少 glyf 表")?;

        // 字宽
        let mut advances = vec![0u16; num_glyphs as usize];
        for gid in 0..num_glyphs as usize {
            advances[gid] = if gid < num_hmetrics {
                let off = gid * 4;
                if off + 2 <= hmtx.len() {
                    u16_at(&hmtx, off)
                } else {
                    0
                }
            } else if num_hmetrics > 0 {
                u16_at(&hmtx, (num_hmetrics - 1) * 4)
            } else {
                0
            };
        }

        // cmap：优先 (3,10) 的 format 12，其次 (3,1) 的 format 4
        let num_sub = u16_at(&cmap_raw, 2) as usize;
        let mut best: Option<(u8, usize)> = None; // (score, offset)
        for i in 0..num_sub {
            let rec = 4 + i * 8;
            if rec + 8 > cmap_raw.len() {
                break;
            }
            let platform = u16_at(&cmap_raw, rec);
            let encoding = u16_at(&cmap_raw, rec + 2);
            let offset = u32_at(&cmap_raw, rec + 4) as usize;
            if offset + 2 > cmap_raw.len() {
                continue;
            }
            let format = u16_at(&cmap_raw, offset);
            let score = match (platform, encoding, format) {
                (3, 10, 12) => 10,
                (0, 4, 12) | (0, 6, 12) => 9,
                (3, 1, 4) => 8,
                (0, 3, 4) => 7,
                _ => 0,
            };
            if score > 0 && best.map(|(s, _)| score > s).unwrap_or(true) {
                best = Some((score, offset));
            }
        }
        let (cmap_sub, cmap_format) = match best {
            Some((_, offset)) => {
                let format = u16_at(&cmap_raw, offset);
                let len = if format == 12 {
                    u32_at(&cmap_raw, offset + 4) as usize
                } else {
                    u16_at(&cmap_raw, offset + 2) as usize
                };
                let end = (offset + len).min(cmap_raw.len());
                let sub = cmap_raw[offset..end].to_vec();
                (sub, if format == 12 { CmapFormat::Fmt12 } else { CmapFormat::Fmt4 })
            }
            None => (Vec::new(), CmapFormat::None),
        };

        Ok(FontFace {
            units_per_em,
            ascent,
            descent,
            cap_height,
            bbox,
            num_glyphs,
            advances,
            glyf,
            loca,
            cmap_sub,
            cmap_format,
            tables,
        })
    }

    // 由烟测工程调用（tools/docs-smoke 用 include! 复用本文件），主工程内暂未使用
    #[allow(dead_code)]
    pub fn num_glyphs(&self) -> u16 {
        self.num_glyphs
    }

    /// 字形轮廓：返回若干条闭合折线（字体单位，y 向上）。
    /// 只处理 glyf 里的简单字形与复合字形（平移/缩放/2×2 变换），无法解析时返回 None。
    pub fn outline(&self, gid: u16) -> Option<Vec<Vec<(f32, f32)>>> {
        self.outline_depth(gid, 0)
    }

    fn outline_depth(&self, gid: u16, depth: u8) -> Option<Vec<Vec<(f32, f32)>>> {
        if depth > 4 || gid as usize + 1 >= self.loca.len() {
            return None;
        }
        let start = self.loca[gid as usize] as usize;
        let end = self.loca[gid as usize + 1] as usize;
        if end <= start || end > self.glyf.len() {
            return Some(Vec::new()); // 空字形
        }
        let data = &self.glyf[start..end];
        let contours_count = i16_at(data, 0);
        if contours_count < 0 {
            return self.outline_composite(data, depth);
        }
        let contours = contours_count as usize;
        if contours == 0 {
            return Some(Vec::new());
        }
        if 10 + contours * 2 + 2 > data.len() {
            return None;
        }
        let mut end_points = Vec::with_capacity(contours);
        for i in 0..contours {
            end_points.push(u16_at(data, 10 + i * 2) as usize);
        }
        let num_points = end_points.last().copied()? + 1;
        let ins_len = u16_at(data, 10 + contours * 2) as usize;
        let mut pos = 10 + contours * 2 + 2 + ins_len;
        if pos > data.len() {
            return None;
        }

        // 读 flag
        let mut flags = Vec::with_capacity(num_points);
        while flags.len() < num_points {
            if pos >= data.len() {
                return None;
            }
            let f = data[pos];
            pos += 1;
            flags.push(f);
            if f & 0x08 != 0 {
                if pos >= data.len() {
                    return None;
                }
                let repeat = data[pos];
                pos += 1;
                for _ in 0..repeat {
                    flags.push(f);
                }
            }
        }
        flags.truncate(num_points);

        // x 坐标
        let mut xs = Vec::with_capacity(num_points);
        let mut x = 0i32;
        for flag in flags.iter() {
            if flag & 0x02 != 0 {
                if pos >= data.len() {
                    return None;
                }
                let delta = data[pos] as i32;
                pos += 1;
                x += if flag & 0x10 != 0 { delta } else { -delta };
            } else if flag & 0x10 == 0 {
                if pos + 2 > data.len() {
                    return None;
                }
                x += i16_at(data, pos) as i32;
                pos += 2;
            }
            xs.push(x as f32);
        }
        // y 坐标
        let mut ys = Vec::with_capacity(num_points);
        let mut y = 0i32;
        for flag in flags.iter() {
            if flag & 0x04 != 0 {
                if pos >= data.len() {
                    return None;
                }
                let delta = data[pos] as i32;
                pos += 1;
                y += if flag & 0x20 != 0 { delta } else { -delta };
            } else if flag & 0x20 == 0 {
                if pos + 2 > data.len() {
                    return None;
                }
                y += i16_at(data, pos) as i32;
                pos += 2;
            }
            ys.push(y as f32);
        }

        // 组合轮廓，把二次曲线离散成折线
        let mut result: Vec<Vec<(f32, f32)>> = Vec::with_capacity(contours);
        let mut first = 0usize;
        for end_point in end_points.iter() {
            let last = *end_point;
            if last >= num_points || last < first {
                first = last + 1;
                continue;
            }
            let mut pts: Vec<(f32, f32, bool)> = Vec::new();
            for i in first..=last {
                pts.push((xs[i], ys[i], flags[i] & 0x01 != 0));
            }
            first = last + 1;
            if pts.is_empty() {
                continue;
            }
            // 找起点：优先 on-curve 点
            let start_idx = pts.iter().position(|p| p.2).unwrap_or(0);
            let mut poly: Vec<(f32, f32)> = Vec::new();
            let n = pts.len();
            let mut cur = if pts[start_idx].2 {
                (pts[start_idx].0, pts[start_idx].1)
            } else {
                // 全是 off-curve：取前后中点
                let prev = pts[(start_idx + n - 1) % n];
                ((prev.0 + pts[start_idx].0) / 2.0, (prev.1 + pts[start_idx].1) / 2.0)
            };
            poly.push(cur);
            let mut steps = 0usize;
            while steps < n {
                let idx = (start_idx + steps + 1) % n;
                let pt = pts[idx];
                let next = pts[(idx + 1) % n];
                if pt.2 {
                    poly.push((pt.0, pt.1));
                    cur = (pt.0, pt.1);
                    steps += 1;
                } else {
                    let end = if next.2 {
                        (next.0, next.1)
                    } else {
                        ((pt.0 + next.0) / 2.0, (pt.1 + next.1) / 2.0)
                    };
                    flatten_quad_basic(cur, (pt.0, pt.1), end, &mut poly);
                    cur = end;
                    steps += 1;
                }
            }
            if poly.len() >= 2 {
                if let Some(first) = poly.first().copied() {
                    poly.push(first);
                }
                result.push(poly);
            }
        }
        Some(result)
    }

    fn outline_composite(&self, data: &[u8], depth: u8) -> Option<Vec<Vec<(f32, f32)>>> {
        let mut pos = 10usize;
        let mut result: Vec<Vec<(f32, f32)>> = Vec::new();
        loop {
            if pos + 4 > data.len() {
                break;
            }
            let flags = u16_at(data, pos);
            let glyph_index = u16_at(data, pos + 2);
            pos += 4;
            let (dx, dy) = if flags & 0x0001 != 0 {
                if pos + 4 > data.len() {
                    break;
                }
                let v = (i16_at(data, pos) as f32, i16_at(data, pos + 2) as f32);
                pos += 4;
                v
            } else {
                if pos + 2 > data.len() {
                    break;
                }
                let v = (data[pos] as i8 as f32, data[pos + 1] as i8 as f32);
                pos += 2;
                v
            };
            let (a, b, c, d) = if flags & 0x0008 != 0 {
                if pos + 2 > data.len() {
                    break;
                }
                let s = i16_at(data, pos) as f32 / 16384.0;
                pos += 2;
                (s, 0.0, 0.0, s)
            } else if flags & 0x0040 != 0 {
                if pos + 4 > data.len() {
                    break;
                }
                let sx = i16_at(data, pos) as f32 / 16384.0;
                let sy = i16_at(data, pos + 2) as f32 / 16384.0;
                pos += 4;
                (sx, 0.0, 0.0, sy)
            } else if flags & 0x0080 != 0 {
                if pos + 8 > data.len() {
                    break;
                }
                let v = (
                    i16_at(data, pos) as f32 / 16384.0,
                    i16_at(data, pos + 2) as f32 / 16384.0,
                    i16_at(data, pos + 4) as f32 / 16384.0,
                    i16_at(data, pos + 6) as f32 / 16384.0,
                );
                pos += 8;
                v
            } else {
                (1.0, 0.0, 0.0, 1.0)
            };
            if let Some(sub) = self.outline_depth(glyph_index, depth + 1) {
                for contour in sub {
                    let mut mapped: Vec<(f32, f32)> = contour
                        .iter()
                        .map(|(x, y)| (a * *x + c * *y + dx, b * *x + d * *y + dy))
                        .collect();
                    if mapped.len() >= 2 {
                        if let Some(first) = mapped.first().copied() {
                            mapped.push(first);
                        }
                        result.push(mapped);
                    }
                }
            }
            if flags & 0x0020 == 0 {
                break;
            }
        }
        Some(result)
    }

    /// Unicode 码点 → 字形编号；找不到返回 0（.notdef）
    pub fn gid(&self, c: char) -> u16 {
        let code = c as u32;
        match self.cmap_format {
            CmapFormat::Fmt12 => {
                if self.cmap_sub.len() < 16 {
                    return 0;
                }
                let n = u32_at(&self.cmap_sub, 12) as usize;
                let mut lo = 0usize;
                let mut hi = n;
                while lo < hi {
                    let mid = (lo + hi) / 2;
                    let g = 16 + mid * 12;
                    if g + 12 > self.cmap_sub.len() {
                        return 0;
                    }
                    let start = u32_at(&self.cmap_sub, g);
                    let end = u32_at(&self.cmap_sub, g + 4);
                    if code < start {
                        hi = mid;
                    } else if code > end {
                        lo = mid + 1;
                    } else {
                        return (u32_at(&self.cmap_sub, g + 8) + (code - start)) as u16;
                    }
                }
                0
            }
            CmapFormat::Fmt4 => {
                if code > 0xFFFF || self.cmap_sub.len() < 16 {
                    return 0;
                }
                let code = code as u16;
                let seg_count = (u16_at(&self.cmap_sub, 6) / 2) as usize;
                let end_base = 14;
                let start_base = end_base + seg_count * 2 + 2;
                let delta_base = start_base + seg_count * 2;
                let range_base = delta_base + seg_count * 2;
                for i in 0..seg_count {
                    let e = end_base + i * 2;
                    if e + 2 > self.cmap_sub.len() {
                        return 0;
                    }
                    if code > u16_at(&self.cmap_sub, e) {
                        continue;
                    }
                    let start = u16_at(&self.cmap_sub, start_base + i * 2);
                    if code < start {
                        return 0;
                    }
                    let delta = u16_at(&self.cmap_sub, delta_base + i * 2) as i32;
                    let ro_pos = range_base + i * 2;
                    let ro = u16_at(&self.cmap_sub, ro_pos) as usize;
                    if ro == 0 {
                        return ((code as i32 + delta) & 0xFFFF) as u16;
                    }
                    let pos = ro_pos + ro + (code - start) as usize * 2;
                    if pos + 2 > self.cmap_sub.len() {
                        return 0;
                    }
                    let gid = u16_at(&self.cmap_sub, pos);
                    if gid == 0 {
                        return 0;
                    }
                    return (((gid as i32 + delta) & 0xFFFF) as u16).max(0);
                }
                0
            }
            CmapFormat::None => 0,
        }
    }

    /// 字形在 1000 em 单位下的宽度（PDF /W 用）
    pub fn width_1000(&self, gid: u16) -> u16 {
        let adv = self.advance(gid) as u64;
        ((adv * 1000) / self.units_per_em as u64) as u16
    }

    pub fn advance(&self, gid: u16) -> u16 {
        self.advances
            .get(gid as usize)
            .copied()
            .unwrap_or_else(|| self.advances.first().copied().unwrap_or(0))
    }

    fn glyph(&self, gid: u16) -> &[u8] {
        let start = self.loca.get(gid as usize).copied().unwrap_or(0) as usize;
        let end = self
            .loca
            .get(gid as usize + 1)
            .copied()
            .unwrap_or(0) as usize;
        if start >= end || end > self.glyf.len() {
            return &[];
        }
        &self.glyf[start..end]
    }

    fn components(&self, gid: u16, out: &mut Vec<u16>) {
        let g = self.glyph(gid);
        if g.len() < 10 {
            return;
        }
        if i16_at(g, 0) >= 0 {
            return; // 简单字形
        }
        let mut pos = 10usize;
        loop {
            if pos + 4 > g.len() {
                return;
            }
            let flags = u16_at(g, pos);
            let comp = u16_at(g, pos + 2);
            out.push(comp);
            pos += 4;
            pos += if flags & 0x0001 != 0 { 4 } else { 2 };
            if flags & 0x0008 != 0 {
                pos += 2;
            } else if flags & 0x0040 != 0 {
                pos += 4;
            } else if flags & 0x0080 != 0 {
                pos += 8;
            }
            if flags & 0x0020 == 0 {
                return;
            }
        }
    }

    /// 把用到的字形保留、其余清空，产出可直接放进 /FontFile2 的 TrueType 字节
    pub fn subset(&self, used: &HashSet<u16>) -> Vec<u8> {
        // 复合字形引用的部件必须一起留下
        let mut keep = used.clone();
        loop {
            let mut added = Vec::new();
            for &gid in keep.clone().iter() {
                let mut comps = Vec::new();
                self.components(gid, &mut comps);
                for c in comps {
                    if !keep.contains(&c) {
                        added.push(c);
                    }
                }
            }
            if added.is_empty() {
                break;
            }
            for c in added {
                keep.insert(c);
            }
        }
        keep.insert(0);

        let mut glyf_bytes: Vec<u8> = Vec::new();
        let mut new_loca: Vec<u32> = Vec::with_capacity(self.num_glyphs as usize + 1);
        for gid in 0..self.num_glyphs {
            new_loca.push(glyf_bytes.len() as u32);
            if keep.contains(&gid) {
                glyf_bytes.extend_from_slice(self.glyph(gid));
                while glyf_bytes.len() % 4 != 0 {
                    glyf_bytes.push(0);
                }
            }
        }
        new_loca.push(glyf_bytes.len() as u32);
        let mut loca_bytes = Vec::with_capacity(new_loca.len() * 4);
        for off in &new_loca {
            loca_bytes.extend_from_slice(&off.to_be_bytes());
        }

        let mut out_tables: Vec<(String, Vec<u8>)> = Vec::new();
        // 只保留 PDF 阅读器真正需要的表：GSUB/GPOS/GDEF/gvar/fvar/vmtx 等排版与变量
        // 表对静态字形渲染无用，照抄会让几百个字形的子集涨到好几 MB。
        const KEEP: [&str; 8] = ["head", "hhea", "maxp", "hmtx", "cmap", "OS/2", "name", "post"];
        for (name, data) in &self.tables {
            match name.as_str() {
                "glyf" => out_tables.push(("glyf".to_string(), glyf_bytes.clone())),
                "loca" => out_tables.push(("loca".to_string(), loca_bytes.clone())),
                "head" => {
                    let mut d = data.clone();
                    if d.len() >= 54 {
                        d[8..12].copy_from_slice(&[0, 0, 0, 0]); // checksumAdjustment 先清零
                        d[50..52].copy_from_slice(&1i16.to_be_bytes()); // indexToLocFormat = long
                    }
                    out_tables.push(("head".to_string(), d));
                }
                "post" => out_tables.push(("post".to_string(), post_v3())),
                other if KEEP.contains(&other) => {
                    out_tables.push((other.to_string(), data.clone()))
                }
                _ => {}
            }
        }
        // 确保 glyf/loca 一定在表目录里
        for (need, data) in [
            ("glyf", glyf_bytes.clone()),
            ("loca", loca_bytes.clone()),
        ] {
            if !out_tables.iter().any(|(n, _)| n == need) {
                out_tables.push((need.to_string(), data));
            }
        }
        out_tables.sort_by(|a, b| a.0.cmp(&b.0));

        build_sfnt(out_tables)
    }

    pub fn to_unicode_cmap(&self, glyphs: &[(u16, char)]) -> Vec<u8> {
        let mut s = String::new();
        s.push_str("/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n");
        s.push_str("/CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n");
        s.push_str("/CMapName /Adobe-Identity-UCS def\n/CMapType 2 def\n");
        s.push_str("1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n");
        let mut sorted: Vec<(u16, char)> = glyphs.to_vec();
        sorted.sort();
        for chunk in sorted.chunks(100) {
            s.push_str(&format!("{} beginbfchar\n", chunk.len()));
            for (gid, ch) in chunk {
                let mut utf16: Vec<u8> = Vec::new();
                let mut buf = [0u16; 2];
                for unit in ch.encode_utf16(&mut buf) {
                    utf16.extend_from_slice(&unit.to_be_bytes());
                }
                s.push_str(&format!("<{:04X}> <{}>\n", gid, hex_upper(&utf16)));
            }
            s.push_str("endbfchar\n");
        }
        s.push_str("endcmap\nCMapName currentdict /CMap definedresource\nend\nend\n");
        s.into_bytes()
    }

    pub fn descriptor_numbers(&self) -> ([i16; 4], i16, i16, i16) {
        let upem = self.units_per_em as f32;
        let scale = |v: i16| (v as f32 * 1000.0 / upem).round() as i16;
        (
            [
                scale(self.bbox[0]),
                scale(self.bbox[1]),
                scale(self.bbox[2]),
                scale(self.bbox[3]),
            ],
            scale(self.ascent),
            scale(self.descent),
            scale(self.cap_height),
        )
    }
}

fn hex_upper(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02X}", b)).collect()
}

/// post 表 version 3：不含字形名，固定 32 字节
fn post_v3() -> Vec<u8> {
    let mut v = Vec::with_capacity(32);
    v.extend_from_slice(&0x0003_0000u32.to_be_bytes());
    v.extend_from_slice(&0u32.to_be_bytes()); // italicAngle
    v.extend_from_slice(&(-100i16).to_be_bytes()); // underlinePosition
    v.extend_from_slice(&50i16.to_be_bytes()); // underlineThickness
    v.extend_from_slice(&0u32.to_be_bytes()); // isFixedPitch
    for _ in 0..4 {
        v.extend_from_slice(&0u32.to_be_bytes()); // min/maxMemType42/1
    }
    v
}

fn checksum(data: &[u8]) -> u32 {
    let mut sum: u32 = 0;
    let mut i = 0;
    while i < data.len() {
        let mut word = [0u8; 4];
        for k in 0..4 {
            if i + k < data.len() {
                word[k] = data[i + k];
            }
        }
        sum = sum.wrapping_add(u32::from_be_bytes(word));
        i += 4;
    }
    sum
}

fn build_sfnt(mut tables: Vec<(String, Vec<u8>)>) -> Vec<u8> {
    let count = tables.len();
    let mut pow = 1u16;
    let mut entry_selector = 0u16;
    while pow * 2 <= count as u16 {
        pow *= 2;
        entry_selector += 1;
    }
    let search_range = pow * 16;
    let range_shift = (count as u16) * 16 - search_range;

    let mut header = Vec::new();
    header.extend_from_slice(&0x0001_0000u32.to_be_bytes());
    header.extend_from_slice(&(count as u16).to_be_bytes());
    header.extend_from_slice(&search_range.to_be_bytes());
    header.extend_from_slice(&entry_selector.to_be_bytes());
    header.extend_from_slice(&range_shift.to_be_bytes());

    let dir_size = 12 + count * 16;
    let mut offset = dir_size;
    let mut records: Vec<u8> = Vec::new();
    let mut body: Vec<u8> = Vec::new();
    let mut head_pos: Option<usize> = None;

    // 先算每张表的位置与校验和，head 的 checksumAdjustment 留到最后回填
    let mut entries: Vec<(String, Vec<u8>, usize, u32)> = Vec::new();
    for (name, mut data) in tables.drain(..) {
        while data.len() % 4 != 0 {
            data.push(0);
        }
        let sum = checksum(&data);
        entries.push((name, data, offset, sum));
        offset += entries.last().map(|(_, d, _, _)| d.len()).unwrap_or(0);
    }

    for (index, (name, data, off, sum)) in entries.iter().enumerate() {
        if name == "head" {
            head_pos = Some(index);
        }
        records.extend_from_slice(name.as_bytes());
        records.extend_from_slice(&(*sum).to_be_bytes());
        records.extend_from_slice(&(*off as u32).to_be_bytes());
        records.extend_from_slice(&(data.len() as u32).to_be_bytes());
        body.extend_from_slice(data);
    }

    let total = checksum(&records)
        .wrapping_add(checksum(&body))
        .wrapping_add(checksum(&header));
    if let Some(index) = head_pos {
        let adjust = 0xB1B0_AFBAu32.wrapping_sub(total);
        let off_in_body = entries[index].2 - dir_size;
        if off_in_body + 12 <= body.len() {
            body[off_in_body + 8..off_in_body + 12].copy_from_slice(&adjust.to_be_bytes());
        }
    }

    let mut out = header;
    out.extend_from_slice(&records);
    out.extend_from_slice(&body);
    out
}

static DEFAULT_FACE: OnceLock<Option<FontFace>> = OnceLock::new();

/// 随包分发的内置字库（首次调用时解析，之后复用）
pub fn default_face() -> Option<&'static FontFace> {
    DEFAULT_FACE
        .get_or_init(|| match FontFace::parse(FONT_BYTES) {
            Ok(f) => Some(f),
            Err(e) => {
                eprintln!("[docs] 内置字体解析失败: {}", e);
                None
            }
        })
        .as_ref()
}
