// 各类文档 → PDF：抽取结构后用内置排版器输出。
//
// 纯 Rust：不调本机 Office/WPS/LibreOffice，字形来自随包的 Noto Sans SC 子集。

use std::path::Path;

use super::pdf_text::{self, Block, ImageItem, Item};
use super::text_ops::{self, Block as DocBlock, FlowBlock};

fn file_title(src: &Path) -> String {
    src.file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "文档".to_string())
}

fn map_block(block: DocBlock) -> Item {
    match block {
        DocBlock::Heading(level, text) => Item::Text(Block::heading(text, level)),
        DocBlock::Para(text) => Item::Text(Block::body(text)),
        DocBlock::Item(text) => {
            let mut b = Block::body(format!("· {}", text));
            b.indent = 14.0;
            Item::Text(b)
        }
        DocBlock::Table(rows) => Item::Table(rows),
    }
}

fn map_flow(flow: Vec<FlowBlock>) -> Vec<Item> {
    flow.into_iter()
        .map(|node| match node {
            FlowBlock::Text(block) => map_block(block),
            FlowBlock::Image { bytes, w_pt, h_pt } => Item::Image(ImageItem {
                data: bytes,
                w: w_pt,
                h: h_pt,
            }),
        })
        .collect()
}

/// 已经抽好的结构块 → PDF（供 RTF / ODF / EPUB 等走同一条排版路径）
pub fn from_blocks(
    title: &str,
    blocks: Vec<DocBlock>,
    dst: &Path,
    cancelled: &dyn Fn() -> bool,
) -> Result<pdf_text::Stats, String> {
    if blocks.is_empty() {
        return Err("没有可排版的内容".to_string());
    }
    let mut items: Vec<Item> = vec![Item::Text(Block::heading(title.to_string(), 0))];
    for block in blocks {
        if cancelled() {
            return Err("已取消".to_string());
        }
        items.push(map_block(block));
    }
    pdf_text::items_to_pdf(&items, dst, cancelled)
}

/// kind 取 classify() 里的分类名：word / sheet / slides / markdown / text / html / csv
pub fn convert(
    kind: &str,
    src: &Path,
    dst: &Path,
    cancelled: &dyn Fn() -> bool,
) -> Result<pdf_text::Stats, String> {
    let mut items: Vec<Item> = vec![Item::Text(Block::heading(file_title(src), 0))];

    match kind {
        "word" => {
            let flow = text_ops::docx_flow(src)?;
            if flow.is_empty() {
                return Err("docx 里没有可提取的内容".to_string());
            }
            for item in map_flow(flow) {
                if cancelled() {
                    return Err("已取消".to_string());
                }
                items.push(item);
            }
        }
        "sheet" => {
            let grid = text_ops::xlsx_grid(src)?;
            if grid.is_empty() {
                return Err("xlsx 里没有可提取的内容".to_string());
            }
            items.push(Item::Table(grid));
        }
        "slides" => {
            let flow = text_ops::pptx_flow(src)?;
            for item in map_flow(flow) {
                if cancelled() {
                    return Err("已取消".to_string());
                }
                items.push(item);
            }
        }
        "markdown" => {
            for block in text_ops::md_items(src)? {
                if cancelled() {
                    return Err("已取消".to_string());
                }
                items.push(map_block(block));
            }
        }
        "html" => {
            for block in text_ops::html_items(src)? {
                if cancelled() {
                    return Err("已取消".to_string());
                }
                items.push(map_block(block));
            }
        }
        "text" => {
            for block in text_ops::txt_items(src)? {
                if cancelled() {
                    return Err("已取消".to_string());
                }
                items.push(map_block(block));
            }
        }
        "csv" => {
            let grid = text_ops::csv_grid(src)?;
            if grid.is_empty() {
                return Err("CSV 里没有可提取的内容".to_string());
            }
            items.push(Item::Table(grid));
        }
        other => return Err(format!("{} → PDF：暂不支持", other)),
    }

    let stats = pdf_text::items_to_pdf(&items, dst, cancelled)?;
    Ok(stats)
}
