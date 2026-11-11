// 极简 ZIP 读写（docx/xlsx/pptx 都是 ZIP 容器）。
//
// 只做本功能需要的部分：读取中央目录 → 解压条目；重新打包 → deflate 最高压缩。
// 全部基于 flate2（纯 Rust），不引入任何 C 依赖，也不依赖网络下载额外 crate。
//
// 不支持的：ZIP64（单条目 >4GB）、加密条目、非常规压缩算法（bzip2/lzma/zstd）。
// 对本项目的 2GB 上限与 Office 文档来说，这些都不会出现；遇到时明确报错而不是静默损坏。

use flate2::write::DeflateEncoder;
use flate2::Compression;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

const EOCD_SIG: u32 = 0x0605_4b50; // PK\x05\x06
const CEN_SIG: u32 = 0x0201_4b50; // PK\x01\x02
const LOC_SIG: u32 = 0x0403_4b50; // PK\x03\x04

pub struct Entry {
    pub name: String,
    pub method: u16,
    #[allow(dead_code)]
    pub crc: u32,
    #[allow(dead_code)]
    pub comp_size: u32,
    pub uncomp_size: u32,
    pub raw: Vec<u8>, // 压缩后的原始字节
}

fn u16_at(buf: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([buf[at], buf[at + 1]])
}

fn u32_at(buf: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([buf[at], buf[at + 1], buf[at + 2], buf[at + 3]])
}

fn crc32_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    for i in 0..256u32 {
        let mut c = i;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
        }
        table[i as usize] = c;
    }
    table
}

fn crc32(data: &[u8]) -> u32 {
    let table = crc32_table();
    let mut crc: u32 = 0xFFFF_FFFF;
    for b in data {
        crc = table[((crc ^ *b as u32) & 0xFF) as usize] ^ (crc >> 8);
    }
    crc ^ 0xFFFF_FFFF
}

fn deflate(data: &[u8], level: u32) -> Result<Vec<u8>, String> {
    let mut enc = DeflateEncoder::new(Vec::new(), Compression::new(level));
    enc.write_all(data).map_err(|e| format!("压缩失败: {}", e))?;
    enc.finish().map_err(|e| format!("压缩失败: {}", e))
}

fn inflate(raw: &[u8]) -> Result<Vec<u8>, String> {
    let mut dec = flate2::read::DeflateDecoder::new(raw);
    let mut out = Vec::new();
    dec.read_to_end(&mut out)
        .map_err(|e| format!("解压失败: {}", e))?;
    Ok(out)
}

pub fn read_zip(path: &Path) -> Result<Vec<Entry>, String> {
    let mut file = File::open(path).map_err(|e| format!("打开文件失败: {}", e))?;
    let len = file
        .metadata()
        .map_err(|e| format!("读取文件信息失败: {}", e))?
        .len();
    if len < 22 {
        return Err("不是有效的 ZIP 容器".to_string());
    }

    // 从尾部找 EOCD
    let tail_len = len.min(65_557) as usize;
    let mut tail = vec![0u8; tail_len];
    file.seek(SeekFrom::Start((len - tail_len as u64) as u64))
        .map_err(|e| e.to_string())?;
    file.read_exact(&mut tail).map_err(|e| e.to_string())?;

    let mut eocd_pos = None;
    let mut i = tail.len().saturating_sub(22);
    loop {
        if u32_at(&tail, i) == EOCD_SIG {
            eocd_pos = Some(i);
            break;
        }
        if i == 0 {
            break;
        }
        i -= 1;
    }
    let eocd = eocd_pos.ok_or_else(|| "不是有效的 ZIP 容器（找不到中央目录）".to_string())?;

    let total_entries = u16_at(&tail, eocd + 10) as usize;
    if total_entries == 0xFFFF {
        return Err("暂不支持 ZIP64 格式".to_string());
    }
    let cd_size = u32_at(&tail, eocd + 12) as u64;
    let cd_offset = u32_at(&tail, eocd + 16) as u64;
    if cd_offset == 0xFFFF_FFFF {
        return Err("暂不支持 ZIP64 格式".to_string());
    }

    let mut cd = vec![0u8; cd_size as usize];
    file.seek(SeekFrom::Start(cd_offset))
        .map_err(|e| e.to_string())?;
    file.read_exact(&mut cd)
        .map_err(|e| format!("读取中央目录失败: {}", e))?;

    let mut entries = Vec::with_capacity(total_entries);
    let mut pos = 0usize;
    for _ in 0..total_entries {
        if pos + 46 > cd.len() || u32_at(&cd, pos) != CEN_SIG {
            return Err("中央目录损坏".to_string());
        }
        let flags = u16_at(&cd, pos + 8);
        let method = u16_at(&cd, pos + 10);
        let crc = u32_at(&cd, pos + 16);
        let comp_size = u32_at(&cd, pos + 20);
        let uncomp_size = u32_at(&cd, pos + 24);
        let name_len = u16_at(&cd, pos + 28) as usize;
        let extra_len = u16_at(&cd, pos + 30) as usize;
        let comment_len = u16_at(&cd, pos + 32) as usize;
        let local_offset = u32_at(&cd, pos + 42) as u64;

        if flags & 0x0001 != 0 {
            return Err("不支持加密的 Office 文档".to_string());
        }
        if comp_size == 0xFFFF_FFFF || uncomp_size == 0xFFFF_FFFF || local_offset == 0xFFFF_FFFF {
            return Err("暂不支持 ZIP64 格式".to_string());
        }

        let name_bytes = &cd[pos + 46..pos + 46 + name_len];
        let name = String::from_utf8_lossy(name_bytes).to_string();
        let next = pos + 46 + name_len + extra_len + comment_len;

        // 读本地头，定位真实数据起点
        let mut header = [0u8; 30];
        file.seek(SeekFrom::Start(local_offset))
            .map_err(|e| e.to_string())?;
        file.read_exact(&mut header)
            .map_err(|e| format!("读取本地头失败: {}", e))?;
        if u32_at(&header, 0) != LOC_SIG {
            return Err("本地文件头损坏".to_string());
        }
        let loc_name_len = u16_at(&header, 26) as usize;
        let loc_extra_len = u16_at(&header, 28) as usize;
        let data_start = local_offset + 30 + loc_name_len as u64 + loc_extra_len as u64;

        let mut raw = vec![0u8; comp_size as usize];
        file.seek(SeekFrom::Start(data_start))
            .map_err(|e| e.to_string())?;
        file.read_exact(&mut raw)
            .map_err(|e| format!("读取条目失败: {}", e))?;

        entries.push(Entry {
            name,
            method,
            crc,
            comp_size,
            uncomp_size,
            raw,
        });
        pos = next;
    }
    Ok(entries)
}

impl Entry {
    pub fn is_dir(&self) -> bool {
        self.name.ends_with('/')
    }

    pub fn bytes(&self) -> Result<Vec<u8>, String> {
        match self.method {
            0 => Ok(self.raw.clone()),
            8 => {
                let data = inflate(&self.raw)?;
                if data.len() != self.uncomp_size as usize {
                    return Err(format!("条目 {} 解压长度不符", self.name));
                }
                Ok(data)
            }
            other => Err(format!("不支持的压缩算法 {}（条目 {}）", other, self.name)),
        }
    }
}

pub struct OutEntry {
    pub name: String,
    pub data: Vec<u8>,
}

pub fn write_zip(entries: Vec<OutEntry>, dst: &Path) -> Result<(), String> {
    let mut out = File::create(dst).map_err(|e| format!("写入失败: {}", e))?;
    let mut pos: u64 = 0;
    let mut central: Vec<(u64, u16, u32, u32, u32, String)> = Vec::new();

    for entry in &entries {
        let name = entry.name.as_str();
        let raw_crc = crc32(&entry.data);
        let compressed = deflate(&entry.data, 9)?;
        let (method, payload) = if compressed.len() < entry.data.len() {
            (8u16, compressed)
        } else {
            (0u16, entry.data.clone())
        };
        let payload_len = payload.len() as u32;
        let uncomp_len = entry.data.len() as u32;
        if payload_len >= u32::MAX || uncomp_len >= u32::MAX {
            return Err("单条目超过 4GB，暂不支持".to_string());
        }

        central.push((
            pos,
            method,
            raw_crc,
            payload_len,
            uncomp_len,
            entry.name.clone(),
        ));

        // 本地文件头
        let mut header = Vec::with_capacity(30 + name.len());
        header.extend_from_slice(&LOC_SIG.to_le_bytes());
        header.extend_from_slice(&20u16.to_le_bytes()); // version needed
        header.extend_from_slice(&0x0800u16.to_le_bytes()); // UTF-8 文件名
        header.extend_from_slice(&method.to_le_bytes());
        header.extend_from_slice(&0u16.to_le_bytes()); // 时间
        header.extend_from_slice(&0u16.to_le_bytes()); // 日期
        header.extend_from_slice(&raw_crc.to_le_bytes());
        header.extend_from_slice(&payload_len.to_le_bytes());
        header.extend_from_slice(&uncomp_len.to_le_bytes());
        header.extend_from_slice(&(name.len() as u16).to_le_bytes());
        header.extend_from_slice(&0u16.to_le_bytes()); // extra
        header.extend_from_slice(name.as_bytes());
        out.write_all(&header).map_err(|e| e.to_string())?;
        out.write_all(&payload).map_err(|e| e.to_string())?;
        pos += header.len() as u64 + payload.len() as u64;
    }

    let cd_start = pos;
    for (offset, method, crc, comp, uncomp, name) in &central {
        let mut rec = Vec::with_capacity(46 + name.len());
        rec.extend_from_slice(&CEN_SIG.to_le_bytes());
        rec.extend_from_slice(&20u16.to_le_bytes()); // version made by
        rec.extend_from_slice(&20u16.to_le_bytes()); // version needed
        rec.extend_from_slice(&0x0800u16.to_le_bytes());
        rec.extend_from_slice(&method.to_le_bytes());
        rec.extend_from_slice(&0u16.to_le_bytes());
        rec.extend_from_slice(&0u16.to_le_bytes());
        rec.extend_from_slice(&crc.to_le_bytes());
        rec.extend_from_slice(&comp.to_le_bytes());
        rec.extend_from_slice(&uncomp.to_le_bytes());
        rec.extend_from_slice(&(name.len() as u16).to_le_bytes());
        rec.extend_from_slice(&0u16.to_le_bytes()); // extra
        rec.extend_from_slice(&0u16.to_le_bytes()); // comment
        rec.extend_from_slice(&0u16.to_le_bytes()); // disk
        rec.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
        rec.extend_from_slice(&0u32.to_le_bytes()); // external attrs
        rec.extend_from_slice(&(*offset as u32).to_le_bytes());
        rec.extend_from_slice(name.as_bytes());
        out.write_all(&rec).map_err(|e| e.to_string())?;
        pos += rec.len() as u64;
    }
    let cd_size = pos - cd_start;

    let mut eocd = Vec::new();
    eocd.extend_from_slice(&EOCD_SIG.to_le_bytes());
    eocd.extend_from_slice(&0u16.to_le_bytes());
    eocd.extend_from_slice(&0u16.to_le_bytes());
    eocd.extend_from_slice(&(central.len() as u16).to_le_bytes());
    eocd.extend_from_slice(&(central.len() as u16).to_le_bytes());
    eocd.extend_from_slice(&(cd_size as u32).to_le_bytes());
    eocd.extend_from_slice(&(cd_start as u32).to_le_bytes());
    eocd.extend_from_slice(&0u16.to_le_bytes()); // comment
    out.write_all(&eocd).map_err(|e| e.to_string())?;
    out.flush().map_err(|e| e.to_string())?;
    Ok(())
}
