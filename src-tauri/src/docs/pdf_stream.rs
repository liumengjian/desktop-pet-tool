// PDF 压缩的流式实现：不把整份文件读进内存。
//
// 思路是「带变换的对象拷贝」：
//   1. 读文件尾部 startxref，解析交叉引用表（经典 xref 或 xref 流），得到对象偏移；
//   2. 逐个对象按需读取头部字典（几 KB 窗口），从 /Root 出发做可达性遍历，
//      不可达对象直接不写出 —— 这一步等价于 lopdf 的 prune_objects；
//   3. 写出时按对象号顺序重排，流数据从源文件分块拷贝，只有需要重采样的图片
//      才会把单张图解码进内存；
//   4. 重建经典 xref 表与 trailer。
//
// 内存上限因此只取决于「最大单张图片」与「字典集合」，与文件本身大小无关。
//
// 明确不支持的形态（宁可报错也不产坏文件）：
//   - 加密 PDF；
//   - 使用压缩对象流（/Type /ObjStm）的 PDF —— 对象被压在流里，无法逐个重写。

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;

use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use flate2::Compression;
use image::{DynamicImage, ImageBuffer, Luma, Rgb};

use super::image_ops::encode_jpeg_bytes;

const HEAD_WINDOW_MIN: usize = 8 * 1024;
const HEAD_WINDOW_MAX: usize = 1024 * 1024;
const CHUNK: usize = 1024 * 1024;

/// 遇到压缩对象流时返回这句话，调用方据此决定是否回退到整份载入的实现
pub const OBJSTM_MSG: &str =
    "该 PDF 使用压缩对象流（/ObjStm），流式压缩暂不支持这种结构";

// ---------------- PDF 值 ----------------

#[derive(Clone, Debug)]
enum Val {
    Null,
    Bool(bool),
    Int(i64),
    Real(Vec<u8>),
    Name(Vec<u8>),
    Str(Vec<u8>),
    Hex(Vec<u8>),
    Array(Vec<Val>),
    Dict(Vec<(Vec<u8>, Val)>),
    Ref(u32, u16),
    Op(Vec<u8>),
}

fn is_delim(c: u8) -> bool {
    matches!(
        c,
        b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
    ) || c.is_ascii_whitespace()
}

fn skip_ws(b: &[u8], p: &mut usize) {
    while *p < b.len() {
        let c = b[*p];
        if c.is_ascii_whitespace() {
            *p += 1;
        } else if c == b'%' {
            while *p < b.len() && b[*p] != b'\n' && b[*p] != b'\r' {
                *p += 1;
            }
        } else {
            break;
        }
    }
}

fn read_token<'a>(b: &'a [u8], p: &mut usize) -> &'a [u8] {
    let s = *p;
    while *p < b.len() && !is_delim(b[*p]) {
        *p += 1;
    }
    &b[s..*p]
}

fn parse_i64(tok: &[u8]) -> Option<i64> {
    std::str::from_utf8(tok).ok()?.trim().parse::<i64>().ok()
}

fn parse_val(b: &[u8], p: &mut usize, depth: u8, out: &mut Val) -> Result<(), String> {
    if depth > 32 {
        return Err("对象嵌套过深".to_string());
    }
    skip_ws(b, p);
    if *p >= b.len() {
        return Err("TRUNC".to_string());
    }
    match b[*p] {
        b'/' => {
            *p += 1;
            let name = read_token(b, p).to_vec();
            *out = Val::Name(name);
        }
        b'<' => {
            if *p + 1 < b.len() && b[*p + 1] == b'<' {
                *p += 2;
                let mut entries: Vec<(Vec<u8>, Val)> = Vec::new();
                loop {
                    skip_ws(b, p);
                    if *p >= b.len() {
                        return Err("TRUNC".to_string());
                    }
                    if b[*p] == b'>' && *p + 1 < b.len() && b[*p + 1] == b'>' {
                        *p += 2;
                        break;
                    }
                    if b[*p] != b'/' {
                        // 非法字典键，跳过该值以免死循环
                        let mut junk = Val::Null;
                        parse_val(b, p, depth + 1, &mut junk)?;
                        continue;
                    }
                    *p += 1;
                    let key = read_token(b, p).to_vec();
                    let mut v = Val::Null;
                    parse_val(b, p, depth + 1, &mut v)?;
                    entries.push((key, v));
                }
                *out = Val::Dict(entries);
            } else {
                let start = *p;
                *p += 1;
                while *p < b.len() && b[*p] != b'>' {
                    *p += 1;
                }
                if *p >= b.len() {
                    return Err("TRUNC".to_string());
                }
                *p += 1;
                *out = Val::Hex(b[start..*p].to_vec());
            }
        }
        b'[' => {
            let start = *p;
            *p += 1;
            let mut items = Vec::new();
            loop {
                skip_ws(b, p);
                if *p >= b.len() {
                    return Err("TRUNC".to_string());
                }
                if b[*p] == b']' {
                    *p += 1;
                    break;
                }
                let mut v = Val::Null;
                parse_val(b, p, depth + 1, &mut v)?;
                items.push(v);
            }
            let _ = start;
            *out = Val::Array(items);
        }
        b'(' => {
            let start = *p;
            *p += 1;
            let mut level = 1;
            while *p < b.len() {
                match b[*p] {
                    b'\\' => *p += 2,
                    b'(' => {
                        level += 1;
                        *p += 1;
                    }
                    b')' => {
                        level -= 1;
                        *p += 1;
                        if level == 0 {
                            break;
                        }
                    }
                    _ => *p += 1,
                }
            }
            if level != 0 {
                return Err("TRUNC".to_string());
            }
            *out = Val::Str(b[start..*p].to_vec());
        }
        b'+' | b'-' | b'.' | b'0'..=b'9' => {
            let start = *p;
            let tok = read_token(b, p).to_vec();
            if tok.is_empty() {
                return Err(format!("无法解析的值: {:?}", &b[start..(start + 1).min(b.len())]));
            }
            if let Some(i) = parse_i64(&tok) {
                // 可能是间接引用 "n g R"
                let save = *p;
                let mut q = *p;
                skip_ws(b, &mut q);
                let t2s = q;
                let t2 = read_token(b, &mut q).to_vec();
                let g2 = parse_i64(&t2);
                let mut ok_ref = false;
                let mut r_pos = q;
                if g2.is_some() {
                    let mut r = q;
                    skip_ws(b, &mut r);
                    if r < b.len() && b[r] == b'R' {
                        let after = r + 1;
                        if after >= b.len() || is_delim(b[after]) {
                            ok_ref = true;
                            r_pos = after;
                        }
                    }
                }
                if ok_ref {
                    *p = r_pos;
                    *out = Val::Ref(i as u32, g2.unwrap_or(0) as u16);
                } else {
                    *p = save;
                    let _ = t2s;
                    *out = Val::Int(i);
                }
            } else {
                *out = Val::Real(tok);
            }
        }
        _ => {
            let tok = read_token(b, p).to_vec();
            if tok.is_empty() {
                return Err("无法解析的标记".to_string());
            }
            *out = match tok.as_slice() {
                b"true" => Val::Bool(true),
                b"false" => Val::Bool(false),
                b"null" => Val::Null,
                _ => Val::Op(tok),
            };
        }
    }
    Ok(())
}

fn write_val(out: &mut Vec<u8>, v: &Val) {
    match v {
        Val::Null => out.extend_from_slice(b"null"),
        Val::Bool(true) => out.extend_from_slice(b"true"),
        Val::Bool(false) => out.extend_from_slice(b"false"),
        Val::Int(i) => out.extend_from_slice(i.to_string().as_bytes()),
        Val::Real(r) => out.extend_from_slice(r),
        Val::Name(n) => {
            out.push(b'/');
            out.extend_from_slice(n);
        }
        Val::Str(s) | Val::Hex(s) => out.extend_from_slice(s),
        Val::Op(o) => out.extend_from_slice(o),
        Val::Ref(n, g) => {
            out.extend_from_slice(n.to_string().as_bytes());
            out.push(b' ');
            out.extend_from_slice(g.to_string().as_bytes());
            out.extend_from_slice(b" R");
        }
        Val::Array(items) => {
            out.push(b'[');
            for (i, it) in items.iter().enumerate() {
                if i > 0 {
                    out.push(b' ');
                }
                write_val(out, it);
            }
            out.push(b']');
        }
        Val::Dict(entries) => {
            out.extend_from_slice(b"<<");
            for (k, val) in entries {
                out.push(b'/');
                out.extend_from_slice(k);
                out.push(b' ');
                write_val(out, val);
                out.push(b' ');
            }
            out.extend_from_slice(b">>");
        }
    }
}

fn collect_refs(v: &Val, out: &mut Vec<(u32, u16)>) {
    match v {
        Val::Ref(n, g) => out.push((*n, *g)),
        Val::Array(items) => {
            for it in items {
                collect_refs(it, out);
            }
        }
        Val::Dict(entries) => {
            for (_, it) in entries {
                collect_refs(it, out);
            }
        }
        _ => {}
    }
}

fn dget<'a>(d: &'a [(Vec<u8>, Val)], key: &[u8]) -> Option<&'a Val> {
    d.iter().find(|(k, _)| k.as_slice() == key).map(|(_, v)| v)
}

fn dget_i64(d: &[(Vec<u8>, Val)], key: &[u8]) -> Option<i64> {
    match dget(d, key)? {
        Val::Int(i) => Some(*i),
        _ => None,
    }
}

fn dget_name<'a>(d: &'a [(Vec<u8>, Val)], key: &[u8]) -> Option<&'a [u8]> {
    match dget(d, key)? {
        Val::Name(n) => Some(n.as_slice()),
        _ => None,
    }
}

fn dremove(d: &mut Vec<(Vec<u8>, Val)>, key: &[u8]) {
    d.retain(|(k, _)| k.as_slice() != key);
}

fn dset(d: &mut Vec<(Vec<u8>, Val)>, key: &[u8], v: Val) {
    dremove(d, key);
    d.push((key.to_vec(), v));
}

// ---------------- 源文件 ----------------

struct Src {
    f: File,
    len: u64,
}

impl Src {
    fn open(path: &Path) -> Result<Src, String> {
        let f = File::open(path).map_err(|e| format!("无法打开 PDF: {}", e))?;
        let len = f.metadata().map_err(|e| format!("无法读取 PDF 大小: {}", e))?.len();
        Ok(Src { f, len })
    }

    fn read_at(&mut self, off: u64, want: usize) -> Result<Vec<u8>, String> {
        if off >= self.len {
            return Ok(Vec::new());
        }
        let want = (want as u64).min(self.len - off) as usize;
        self.f
            .seek(SeekFrom::Start(off))
            .map_err(|e| format!("读取 PDF 失败: {}", e))?;
        let mut buf = vec![0u8; want];
        let mut got = 0usize;
        while got < want {
            let n = self
                .f
                .read(&mut buf[got..])
                .map_err(|e| format!("读取 PDF 失败: {}", e))?;
            if n == 0 {
                break;
            }
            got += n;
        }
        buf.truncate(got);
        Ok(buf)
    }

    fn find_startxref(&mut self) -> Result<u64, String> {
        let tail_len = 4096usize.min(self.len as usize);
        let tail = self.read_at(self.len - tail_len as u64, tail_len)?;
        let needle = b"startxref";
        let mut found: Option<usize> = None;
        let mut i = 0;
        while i + needle.len() <= tail.len() {
            if &tail[i..i + needle.len()] == needle {
                found = Some(i);
            }
            i += 1;
        }
        let at = found.ok_or_else(|| "找不到 startxref，可能不是有效 PDF".to_string())?;
        let mut p = at + needle.len();
        skip_ws(&tail, &mut p);
        let tok = read_token(&tail, &mut p);
        parse_i64(tok)
            .map(|v| v as u64)
            .ok_or_else(|| "startxref 偏移无法解析".to_string())
    }
}

// ---------------- 交叉引用表 ----------------

#[derive(Clone, Copy, Debug)]
struct Ent {
    kind: u8, // 0 free / 1 偏移 / 2 在对象流内
    a: u64,
    // 解析时保留的原始代次/序号，供后续增量更新使用，当前流式重写未读取
    #[allow(dead_code)]
    b: u16,
}

struct Xref {
    map: HashMap<u32, Ent>,
    #[allow(dead_code)]
    size: u32,
    root: Option<(u32, u16)>,
    info: Option<(u32, u16)>,
    id: Option<Val>,
    encrypt: bool,
}

fn parse_classic_section(
    buf: &[u8],
    map: &mut HashMap<u32, Ent>,
    trailer_out: &mut Option<Vec<(Vec<u8>, Val)>>,
) -> Result<Option<u64>, String> {
    let mut p = 0usize;
    skip_ws(buf, &mut p);
    if buf.get(p..p + 4) != Some(b"xref") {
        return Err("BUFCUT".to_string());
    }
    p += 4;
    loop {
        skip_ws(buf, &mut p);
        if p >= buf.len() {
            return Err("BUFCUT".to_string());
        }
        if buf.get(p..p + 7) == Some(b"trailer") {
            p += 7;
            break;
        }
        let t1s = p;
        let t1 = read_token(buf, &mut p).to_vec();
        let Some(start) = parse_i64(&t1) else {
            let _ = t1s;
            return Err("xref 子节格式异常".to_string());
        };
        skip_ws(buf, &mut p);
        let t2 = read_token(buf, &mut p).to_vec();
        let Some(count) = parse_i64(&t2) else {
            return Err("xref 子节条目数异常".to_string());
        };
        for i in 0..count {
            skip_ws(buf, &mut p);
            if p >= buf.len() {
                return Err("BUFCUT".to_string());
            }
            let eo = read_token(buf, &mut p).to_vec();
            skip_ws(buf, &mut p);
            let eg = read_token(buf, &mut p).to_vec();
            skip_ws(buf, &mut p);
            let et = read_token(buf, &mut p).to_vec();
            let (Some(off), Some(gen)) = (parse_i64(&eo), parse_i64(&eg)) else {
                return Err("xref 条目格式异常".to_string());
            };
            let num = (start + i) as u32;
            if et == b"n" {
                map.insert(
                    num,
                    Ent {
                        kind: 1,
                        a: off as u64,
                        b: gen as u16,
                    },
                );
            } else if !map.contains_key(&num) {
                map.insert(num, Ent { kind: 0, a: 0, b: 0 });
            }
        }
    }
    skip_ws(buf, &mut p);
    let mut trailer = Val::Null;
    parse_val(buf, &mut p, 0, &mut trailer)?;
    if let Val::Dict(entries) = trailer {
        let prev = dget_i64(&entries, b"Prev").map(|v| v as u64);
        *trailer_out = Some(entries);
        return Ok(prev);
    }
    Err("trailer 字典缺失".to_string())
}

fn decode_flate(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut d = ZlibDecoder::new(data);
    let mut out = Vec::new();
    d.read_to_end(&mut out)
        .map_err(|e| format!("Flate 解压失败: {}", e))?;
    Ok(out)
}

fn parse_xref_stream(
    dict: &[(Vec<u8>, Val)],
    data: &[u8],
    map: &mut HashMap<u32, Ent>,
) -> Result<Option<u64>, String> {
    let w = match dget(dict, b"W") {
        Some(Val::Array(a)) if a.len() == 3 => {
            let mut v = [0usize; 3];
            for i in 0..3 {
                match a[i] {
                    Val::Int(n) => v[i] = n as usize,
                    _ => return Err("xref 流 /W 非法".to_string()),
                }
            }
            v
        }
        _ => return Err("xref 流缺少 /W".to_string()),
    };
    let row = w[0] + w[1] + w[2];
    if row == 0 || row > 24 {
        return Err("xref 流行宽非法".to_string());
    }
    let size = dget_i64(dict, b"Size").unwrap_or(0) as usize;
    let index: Vec<usize> = match dget(dict, b"Index") {
        Some(Val::Array(a)) => {
            let mut v = Vec::new();
            for it in a {
                match it {
                    Val::Int(n) => v.push(*n as usize),
                    _ => return Err("xref 流 /Index 非法".to_string()),
                }
            }
            v
        }
        _ => vec![0, size],
    };
    let take = |b: &[u8], n: usize| -> u64 {
        let mut v = 0u64;
        for i in 0..n {
            v = (v << 8) | b[i] as u64;
        }
        v
    };
    let mut pos = 0usize;
    let mut seg = 0usize;
    while seg + 1 < index.len() {
        let start = index[seg];
        let count = index[seg + 1];
        for i in 0..count {
            if pos + row > data.len() {
                return Err("xref 流数据长度不足".to_string());
            }
            let row_b = &data[pos..pos + row];
            pos += row;
            let t = take(row_b, w[0]);
            let f2 = take(&row_b[w[0]..], w[1]);
            let f3 = take(&row_b[w[0] + w[1]..], w[2]);
            let num = (start + i) as u32;
            match t {
                0 => {
                    if !map.contains_key(&num) {
                        map.insert(num, Ent { kind: 0, a: 0, b: f3 as u16 });
                    }
                }
                1 => {
                    map.insert(num, Ent { kind: 1, a: f2, b: f3 as u16 });
                }
                2 => {
                    map.insert(num, Ent { kind: 2, a: f2, b: f3 as u16 });
                }
                _ => {}
            }
        }
        seg += 2;
    }
    let prev = dget_i64(dict, b"Prev").map(|v| v as u64);
    Ok(prev)
}

struct ObjHead {
    num: u32,
    gen: u16,
    val: Val,
    stream_start: Option<u64>,
    dict_only: Option<Vec<(Vec<u8>, Val)>>,
    /// 值解析结束在窗口内的位置
    body_end: usize,
}

enum ObjErr {
    Trunc,
    Fatal(String),
}

fn parse_obj_head(off: u64, b: &[u8]) -> Result<ObjHead, ObjErr> {
    let mut p = 0usize;
    skip_ws(b, &mut p);
    if p >= b.len() {
        return Err(ObjErr::Trunc);
    }
    let t1 = read_token(b, &mut p).to_vec();
    let Some(num) = parse_i64(&t1) else {
        return Err(ObjErr::Fatal("对象头段号非法".to_string()));
    };
    skip_ws(b, &mut p);
    let t2 = read_token(b, &mut p).to_vec();
    let gen = parse_i64(&t2).unwrap_or(0);
    skip_ws(b, &mut p);
    let t3 = read_token(b, &mut p).to_vec();
    if t3 != b"obj" {
        return Err(ObjErr::Fatal("对象头缺少 obj 关键字".to_string()));
    }
    let mut val = Val::Null;
    parse_val(b, &mut p, 0, &mut val).map_err(|e| {
        if e == "TRUNC" {
            ObjErr::Trunc
        } else {
            ObjErr::Fatal(e)
        }
    })?;
    let dict_only = match &val {
        Val::Dict(entries) => Some(entries.clone()),
        _ => None,
    };
    let body_end = p;
    let mut stream_start = None;
    if dict_only.is_some() {
        skip_ws(b, &mut p);
        if p >= b.len() {
            return Err(ObjErr::Trunc);
        }
        let mut q = p;
        let tok = read_token(b, &mut q).to_vec();
        if tok == b"stream" {
            // stream 关键字后跟 CRLF 或 LF
            if q < b.len() && b[q] == b'\r' {
                q += 1;
            }
            if q < b.len() && b[q] == b'\n' {
                q += 1;
            }
            if q >= b.len() {
                return Err(ObjErr::Trunc);
            }
            stream_start = Some(off + q as u64);
        }
    }
    Ok(ObjHead {
        num: num as u32,
        gen: gen as u16,
        val,
        stream_start,
        dict_only,
        body_end,
    })
}

struct Obj {
    // 对象号：解析时记录，当前端到端重写按 map 迭代，不单独读取
    #[allow(dead_code)]
    num: u32,
    gen: u16,
    val: Val,
    /// 流数据起点与长度
    stream: Option<(u64, u64)>,
    /// 对象在源文件中的结束位置（含 endobj）
    end: u64,
}

fn read_object(
    src: &mut Src,
    map: &HashMap<u32, Ent>,
    off: u64,
    expect: Option<u32>,
) -> Result<Obj, String> {
    let mut win = HEAD_WINDOW_MIN;
    loop {
        let buf = src.read_at(off, win)?;
        match parse_obj_head(off, &buf) {
            Ok(h) => {
                if let Some(e) = expect {
                    if h.num != e {
                        return Err(format!(
                            "对象号不匹配：xref 指向 {}，实际读到 {}",
                            e, h.num
                        ));
                    }
                }
                let stream_len = match (h.stream_start, &h.dict_only) {
                    (Some(_), Some(dict)) => Some(resolve_length(src, map, dict)?),
                    _ => None,
                };
                let end = match (h.stream_start, stream_len) {
                    (Some(start), Some(len)) => {
                        // 用 /Length 精确跳过流数据，再确认 endstream / endobj
                        let tail = src.read_at(start + len, 64)?;
                        let mut p = 0usize;
                        skip_ws(&tail, &mut p);
                        let t = read_token(&tail, &mut p).to_vec();
                        if t != b"endstream" {
                            return Err(format!(
                                "对象 {} 的 /Length 与实际流长度不符（流内可能有 endstream）",
                                h.num
                            ));
                        }
                        skip_ws(&tail, &mut p);
                        let t2 = read_token(&tail, &mut p).to_vec();
                        if t2 != b"endobj" {
                            return Err(format!("对象 {} 缺少 endobj", h.num));
                        }
                        start + len + p as u64
                    }
                    _ => {
                        // 无流对象：字典之后紧跟 endobj（从值结束位置继续，不能从头再解析）
                        let mut p = h.body_end;
                        skip_ws(&buf, &mut p);
                        let t = read_token(&buf, &mut p).to_vec();
                        if t != b"endobj" {
                            return Err(format!("对象 {} 缺少 endobj", h.num));
                        }
                        off + p as u64
                    }
                };
                return Ok(Obj {
                    num: h.num,
                    gen: h.gen,
                    val: h.val,
                    stream: h.stream_start.zip(stream_len),
                    end,
                });
            }
            Err(ObjErr::Trunc) => {
                if win >= HEAD_WINDOW_MAX {
                    return Err(format!("对象头超出 {} 字节窗口", HEAD_WINDOW_MAX));
                }
                win = (win * 4).min(HEAD_WINDOW_MAX);
            }
            Err(ObjErr::Fatal(m)) => return Err(m),
        }
    }
}

fn resolve_length(
    src: &mut Src,
    map: &HashMap<u32, Ent>,
    dict: &[(Vec<u8>, Val)],
) -> Result<u64, String> {
    match dget(dict, b"Length") {
        Some(Val::Int(n)) if *n >= 0 => Ok(*n as u64),
        Some(Val::Ref(n, _g)) => {
            // /Length 指向一个独立的整数对象，按 xref 定位后单独读一次
            let ent = map
                .get(n)
                .copied()
                .ok_or_else(|| format!("间接 /Length 对象 {} 不存在", n))?;
            if ent.kind != 1 {
                return Err(format!("间接 /Length 对象 {} 位于对象流内", n));
            }
            let buf = src.read_at(ent.a, HEAD_WINDOW_MAX)?;
            let h = match parse_obj_head(ent.a, &buf) {
                Ok(h) => h,
                Err(_) => return Err(format!("间接 /Length 对象 {} 无法解析", n)),
            };
            match h.val {
                Val::Int(v) if v >= 0 => Ok(v as u64),
                _ => Err(format!("间接 /Length 不是整数（对象 {}）", n)),
            }
        }
        _ => Err("对象缺少可解析的 /Length，无法定位流边界".to_string()),
    }
}

// ---------------- 主流程 ----------------

pub fn compress(
    src_path: &Path,
    dst: &Path,
    dpi: f32,
    quality: u8,
    cancelled: &dyn Fn() -> bool,
    progress: &dyn Fn(f32, &str),
) -> Result<(), String> {
    progress(3.0, "读取 PDF 结构");
    let mut src = Src::open(src_path)?;
    let xref = load_xref(&mut src)?;

    if xref.encrypt {
        return Err("加密 PDF 暂不支持压缩".to_string());
    }
    if xref.map.values().any(|e| e.kind == 2) {
        return Err(OBJSTM_MSG.to_string());
    }
    let (root_n, _) = xref.root.ok_or_else(|| "PDF 缺少 /Root".to_string())?;

    // 第一遍：可达性遍历 + 记录页面尺寸
    let mut reachable: HashSet<u32> = HashSet::new();
    let mut queue: Vec<u32> = vec![root_n];
    if let Some((n, _)) = xref.info {
        queue.push(n);
    }
    let mut max_pt: f32 = 0.0;
    let total_est = xref.map.len().max(1);
    let mut visited = 0usize;

    while let Some(num) = queue.pop() {
        if !reachable.insert(num) {
            continue;
        }
        visited += 1;
        if visited % 512 == 0 {
            if cancelled() {
                return Err("已取消".to_string());
            }
            progress(
                3.0 + 37.0 * (visited as f32 / total_est as f32).min(1.0),
                &format!("扫描对象 {}/{}", visited, total_est),
            );
        }
        let Some(ent) = xref.map.get(&num).copied() else {
            continue;
        };
        if ent.kind != 1 {
            continue;
        }
        let obj = match read_object(&mut src, &xref.map, ent.a, Some(num)) {
            Ok(o) => o,
            Err(e) => return Err(format!("对象 {} 读取失败: {}", num, e)),
        };
        if let Val::Dict(d) = &obj.val {
            if dget_name(d, b"Type") == Some(b"Page") {
                if let Some(Val::Array(a)) = dget(d, b"MediaBox") {
                    if a.len() == 4 {
                        let nums: Vec<f32> = a
                            .iter()
                            .filter_map(|v| match v {
                                Val::Int(i) => Some(*i as f32),
                                Val::Real(r) => std::str::from_utf8(r).ok()?.parse().ok(),
                                _ => None,
                            })
                            .collect();
                        if nums.len() == 4 {
                            let w = (nums[2] - nums[0]).abs();
                            let h = (nums[3] - nums[1]).abs();
                            max_pt = max_pt.max(w.max(h));
                        }
                    }
                }
            }
            let mut refs = Vec::new();
            collect_refs(&obj.val, &mut refs);
            for (rn, _) in refs {
                if !reachable.contains(&rn) {
                    queue.push(rn);
                }
            }
        }
    }

    let cap_px = if max_pt > 0.0 {
        ((max_pt / 72.0) * dpi).round().clamp(800.0, 4096.0) as u32
    } else {
        2000
    };

    if cancelled() {
        return Err("已取消".to_string());
    }
    progress(42.0, "重写对象");

    let file = File::create(dst).map_err(|e| format!("创建输出文件失败: {}", e))?;
    let mut out = BufWriter::with_capacity(1 << 20, file);
    let mut pos: u64 = 0;
    let mut offsets: HashMap<u32, (u64, u16)> = HashMap::new();
    let mut removed = 0usize;
    let mut images_replaced = 0usize;
    let mut image_bytes_saved: i64 = 0;

    write_all(&mut out, &mut pos, b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n")?;

    let mut nums: Vec<u32> = reachable.iter().copied().collect();
    nums.sort_unstable();
    let total_nums = nums.len().max(1);

    for (idx, num) in nums.iter().enumerate() {
        if cancelled() {
            return Err("已取消".to_string());
        }
        if idx % 256 == 0 {
            progress(
                42.0 + 50.0 * (idx as f32 / total_nums as f32),
                &format!("重写对象 {}/{}", idx, total_nums),
            );
        }
        let Some(ent) = xref.map.get(num).copied() else {
            continue;
        };
        let obj = match read_object(&mut src, &xref.map, ent.a, Some(*num)) {
            Ok(o) => o,
            Err(e) => return Err(format!("对象 {} 读取失败: {}", num, e)),
        };

        let mut dict = match &obj.val {
            Val::Dict(d) => d.clone(),
            _ => {
                // 非字典对象（整数 / 数组 / 字符串）：整段原样拷贝
                let obj_start = pos;
                write_raw_obj(&mut out, &mut pos, &mut src, ent.a, obj.end)?;
                offsets.insert(*num, (obj_start, obj.gen));
                continue;
            }
        };

        // 1) 剥离私有数据 / 缩略图；目录对象另去 Metadata
        let is_catalog = dget_name(&dict, b"Type") == Some(b"Catalog");
        let had_private = dget(&dict, b"PieceInfo").is_some() || dget(&dict, b"Thumb").is_some();
        dremove(&mut dict, b"PieceInfo");
        dremove(&mut dict, b"Thumb");
        if is_catalog {
            dremove(&mut dict, b"Metadata");
        }
        if had_private {
            removed += 1;
        }

        // 2) 图片重采样 / 内容流压缩
        let mut new_stream: Option<Vec<u8>> = None;
        if let Some((start, len)) = obj.stream {
            if is_image(&dict) {
                if let Some(jpeg) = try_resample(
                    &mut src,
                    &mut dict,
                    start,
                    len,
                    cap_px,
                    quality,
                    cancelled,
                )? {
                    image_bytes_saved += len as i64 - jpeg.len() as i64;
                    images_replaced += 1;
                    new_stream = Some(jpeg);
                }
            } else if can_deflate(&dict) {
                let data = src.read_at(start, len as usize)?;
                let compressed = deflate(&data)?;
                if compressed.len() < data.len() {
                    dset(&mut dict, b"Filter", Val::Name(b"FlateDecode".to_vec()));
                    new_stream = Some(compressed);
                }
            }
        }

        // 3) 写出对象
        let obj_start = pos;
        let header = format!("{} {} obj\n", num, obj.gen);
        write_all(&mut out, &mut pos, header.as_bytes())?;

        let stream_len = match (&new_stream, obj.stream) {
            (Some(bytes), _) => bytes.len() as u64,
            (None, Some((_s, l))) => l,
            (None, None) => 0,
        };
        if obj.stream.is_some() {
            dset(&mut dict, b"Length", Val::Int(stream_len as i64));
        }
        let mut dict_bytes = Vec::new();
        write_val(&mut dict_bytes, &Val::Dict(dict));
        write_all(&mut out, &mut pos, &dict_bytes)?;

        if obj.stream.is_some() {
            write_all(&mut out, &mut pos, b"\nstream\n")?;
            match &new_stream {
                Some(bytes) => write_all(&mut out, &mut pos, bytes)?,
                None => {
                    let (start, len) = obj.stream.unwrap();
                    copy_range(&mut src, &mut out, &mut pos, start, len)?;
                }
            }
            write_all(&mut out, &mut pos, b"\nendstream\nendobj\n")?;
        } else {
            write_all(&mut out, &mut pos, b"\nendobj\n")?;
        }
        offsets.insert(*num, (obj_start, obj.gen));
    }

    // 4) 重建 xref
    progress(94.0, "写入索引");
    let xref_pos = pos;
    let size = xref
        .map
        .keys()
        .copied()
        .max()
        .unwrap_or(0)
        .max(nums.last().copied().unwrap_or(0))
        + 1;
    let mut table = Vec::new();
    table.extend_from_slice(format!("xref\n0 {}\n", size).as_bytes());
    table.extend_from_slice(b"0000000000 65535 f \n");
    for n in 1..size {
        match offsets.get(&n) {
            Some((off, gen)) if *gen <= 9999 => {
                table.extend_from_slice(format!("{:010} {:05} n \n", off, gen).as_bytes());
            }
            _ => table.extend_from_slice(b"0000000000 65535 f \n"),
        }
    }
    write_all(&mut out, &mut pos, &table)?;

    let mut trailer = Vec::new();
    trailer.extend_from_slice(b"trailer\n<< /Size ");
    trailer.extend_from_slice(size.to_string().as_bytes());
    trailer.extend_from_slice(b" /Root ");
    trailer.extend_from_slice(format!("{} 0 R", root_n).as_bytes());
    if let Some((n, g)) = xref.info {
        trailer.extend_from_slice(b" /Info ");
        trailer.extend_from_slice(format!("{} {} R", n, g).as_bytes());
    }
    if let Some(id) = &xref.id {
        trailer.extend_from_slice(b" /ID ");
        write_val(&mut trailer, id);
    }
    trailer.extend_from_slice(b" >>\nstartxref\n");
    trailer.extend_from_slice(xref_pos.to_string().as_bytes());
    trailer.extend_from_slice(b"\n%%EOF\n");
    out.write_all(&trailer).map_err(|e| format!("写入失败: {}", e))?;
    out.flush().map_err(|e| format!("写入失败: {}", e))?;

    eprintln!(
        "[docs] pdf stream: objects_written={} private_dicts_stripped={} images_replaced={} image_bytes_saved={}",
        nums.len(),
        removed,
        images_replaced,
        image_bytes_saved
    );
    progress(100.0, "完成");
    Ok(())
}

fn write_all(out: &mut BufWriter<File>, pos: &mut u64, bytes: &[u8]) -> Result<(), String> {
    out.write_all(bytes)
        .map_err(|e| format!("写入失败: {}", e))?;
    *pos += bytes.len() as u64;
    Ok(())
}

fn is_image(dict: &[(Vec<u8>, Val)]) -> bool {
    dget_name(dict, b"Subtype") == Some(b"Image")
}

fn can_deflate(dict: &[(Vec<u8>, Val)]) -> bool {
    dget(dict, b"Type").is_none()
        && dget(dict, b"Subtype").is_none()
        && dget(dict, b"Filter").is_none()
        && dget(dict, b"Length").is_some()
}

fn deflate(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut e = ZlibEncoder::new(Vec::new(), Compression::best());
    e.write_all(data).map_err(|e2| format!("压缩失败: {}", e2))?;
    e.finish().map_err(|e2| format!("压缩失败: {}", e2))
}

fn single_filter(dict: &[(Vec<u8>, Val)]) -> Option<Vec<u8>> {
    match dget(dict, b"Filter") {
        Some(Val::Name(n)) => Some(n.clone()),
        Some(Val::Array(a)) if a.len() == 1 => match &a[0] {
            Val::Name(n) => Some(n.clone()),
            _ => None,
        },
        _ => None,
    }
}

fn try_resample(
    src: &mut Src,
    dict: &mut Vec<(Vec<u8>, Val)>,
    start: u64,
    len: u64,
    cap_px: u32,
    quality: u8,
    cancelled: &dyn Fn() -> bool,
) -> Result<Option<Vec<u8>>, String> {
    if dget(dict, b"SMask").is_some() || dget(dict, b"Mask").is_some() || dget(dict, b"ImageMask").is_some()
    {
        return Ok(None);
    }
    let (Some(w), Some(h)) = (dget_i64(dict, b"Width"), dget_i64(dict, b"Height")) else {
        return Ok(None);
    };
    if w <= 0 || h <= 0 {
        return Ok(None);
    }
    let (w, h) = (w as u32, h as u32);
    if w.max(h) <= cap_px || w.max(h) / cap_px > 64 {
        return Ok(None);
    }
    // 超大图解码会吃掉数 GB 内存，超过 200M 像素直接跳过
    if w as u64 * h as u64 > 200_000_000 {
        return Ok(None);
    }
    if dget_i64(dict, b"BitsPerComponent").unwrap_or(8) != 8 {
        return Ok(None);
    }
    if let Some(Val::Dict(dp)) = dget(dict, b"DecodeParms") {
        if dget_i64(dp, b"Predictor").unwrap_or(1) != 1 {
            return Ok(None);
        }
    }
    let cs = match dget_name(dict, b"ColorSpace") {
        Some(c) => c.to_vec(),
        None => return Ok(None),
    };
    if cs == b"DeviceCMYK" || cs == b"Indexed" || cs == b"Separation" || cs == b"Lab" {
        return Ok(None);
    }
    if len > 512 * 1024 * 1024 {
        return Ok(None);
    }
    let Some(filter) = single_filter(dict) else {
        return Ok(None);
    };
    if cancelled() {
        return Err("已取消".to_string());
    }

    let decoded: Option<DynamicImage> = if filter == b"DCTDecode" {
        let bytes = src.read_at(start, len as usize)?;
        image::load_from_memory(&bytes).ok()
    } else if filter == b"FlateDecode" {
        let bytes = src.read_at(start, len as usize)?;
        let raw = decode_flate(&bytes)?;
        match cs.as_slice() {
            b"DeviceRGB" => ImageBuffer::<Rgb<u8>, Vec<u8>>::from_raw(w, h, raw)
                .map(DynamicImage::ImageRgb8),
            b"DeviceGray" => ImageBuffer::<Luma<u8>, Vec<u8>>::from_raw(w, h, raw)
                .map(DynamicImage::ImageLuma8),
            _ => None,
        }
    } else {
        None
    };
    let Some(img) = decoded else {
        return Ok(None);
    };

    let long = w.max(h);
    let scale = cap_px as f32 / long as f32;
    let nw = ((w as f32 * scale).round() as u32).max(1);
    let nh = ((h as f32 * scale).round() as u32).max(1);
    let resized = img.resize_exact(nw, nh, image::imageops::FilterType::Lanczos3);
    let jpeg = match encode_jpeg_bytes(&resized, quality) {
        Ok(b) => b,
        Err(_) => return Ok(None),
    };
    if jpeg.len() >= len as usize {
        return Ok(None);
    }

    dset(dict, b"Filter", Val::Name(b"DCTDecode".to_vec()));
    dset(dict, b"Width", Val::Int(nw as i64));
    dset(dict, b"Height", Val::Int(nh as i64));
    dset(dict, b"ColorSpace", Val::Name(b"DeviceRGB".to_vec()));
    dset(dict, b"BitsPerComponent", Val::Int(8));
    dremove(dict, b"Decode");
    dremove(dict, b"DecodeParms");
    Ok(Some(jpeg))
}

fn write_raw_obj(
    out: &mut BufWriter<File>,
    pos: &mut u64,
    src: &mut Src,
    start: u64,
    end: u64,
) -> Result<(), String> {
    copy_range(src, out, pos, start, end - start)
}

fn copy_range(
    src: &mut Src,
    out: &mut BufWriter<File>,
    pos: &mut u64,
    start: u64,
    len: u64,
) -> Result<(), String> {
    let mut left = len;
    let mut at = start;
    let mut buf = vec![0u8; CHUNK];
    while left > 0 {
        let want = (left as usize).min(CHUNK);
        let got = src.read_at(at, want)?;
        if got.is_empty() {
            return Err("复制流数据时提前到达文件末尾".to_string());
        }
        out.write_all(&got).map_err(|e| format!("写入失败: {}", e))?;
        *pos += got.len() as u64;
        at += got.len() as u64;
        left -= got.len() as u64;
        let _ = &mut buf;
    }
    Ok(())
}

// ---------------- xref 载入 ----------------

fn load_xref(src: &mut Src) -> Result<Xref, String> {
    let start = src.find_startxref()?;
    let mut map: HashMap<u32, Ent> = HashMap::new();
    let mut size: u32 = 0;
    let mut root = None;
    let mut info = None;
    let mut id = None;
    let mut encrypt = false;

    let mut cursor = Some(start);
    let mut guard = 0;
    while let Some(off) = cursor {
        guard += 1;
        if guard > 64 {
            return Err("xref 链过深".to_string());
        }
        let head = src.read_at(off, 16)?;
        let mut p = 0usize;
        skip_ws(&head, &mut p);
        let is_classic = head.get(p..p + 4) == Some(b"xref");
        if is_classic {
            let mut win = 1 << 20;
            let trailer = loop {
                let buf = src.read_at(off, win)?;
                let mut t: Option<Vec<(Vec<u8>, Val)>> = None;
                match parse_classic_section(&buf, &mut map, &mut t) {
                    Ok(prev) => {
                        let d = t.ok_or_else(|| "trailer 缺失".to_string())?;
                        break (d, prev);
                    }
                    Err(e) if e == "BUFCUT" && win < (32 << 20) => {
                        win *= 2;
                        continue;
                    }
                    Err(e) => return Err(e),
                }
            };
            let (d, prev) = trailer;
            absorb_trailer(&d, &mut size, &mut root, &mut info, &mut id, &mut encrypt);
            cursor = prev;
        } else {
            let obj = read_object(src, &map, off, None)?;
            let dict = match &obj.val {
                Val::Dict(d) => d.clone(),
                _ => return Err("startxref 指向的对象不是 xref 流".to_string()),
            };
            if dget_name(&dict, b"Type") != Some(b"XRef") {
                return Err("startxref 指向的对象既不是 xref 表也不是 xref 流".to_string());
            }
            let (s, l) = obj
                .stream
                .ok_or_else(|| "xref 流缺少数据".to_string())?;
            let raw = src.read_at(s, l as usize)?;
            let data = match single_filter(&dict) {
                Some(f) if f == b"FlateDecode" => decode_flate(&raw)?,
                Some(_) => return Err("xref 流使用了不支持的过滤器".to_string()),
                None => raw,
            };
            let prev = parse_xref_stream(&dict, &data, &mut map)?;
            absorb_trailer(&dict, &mut size, &mut root, &mut info, &mut id, &mut encrypt);
            cursor = prev;
        }
    }

    Ok(Xref {
        map,
        size,
        root,
        info,
        id,
        encrypt,
    })
}

fn absorb_trailer(
    d: &[(Vec<u8>, Val)],
    size: &mut u32,
    root: &mut Option<(u32, u16)>,
    info: &mut Option<(u32, u16)>,
    id: &mut Option<Val>,
    encrypt: &mut bool,
) {
    if let Some(n) = dget_i64(d, b"Size") {
        *size = (*size).max(n as u32);
    }
    if root.is_none() {
        if let Some(Val::Ref(n, g)) = dget(d, b"Root") {
            *root = Some((*n, *g));
        }
    }
    if info.is_none() {
        if let Some(Val::Ref(n, g)) = dget(d, b"Info") {
            *info = Some((*n, *g));
        }
    }
    if id.is_none() {
        if let Some(v) = dget(d, b"ID") {
            *id = Some(v.clone());
        }
    }
    if dget(d, b"Encrypt").is_some() {
        *encrypt = true;
    }
}
