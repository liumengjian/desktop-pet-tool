# -*- coding: utf-8 -*-
"""pdf2docx 产物后处理：列宽按 PDF 竖线修正、段落对齐按 PDF 坐标校正、页脚迁入 footer 部件。

只做保守修补：能在 PDF 坐标里找到对应依据才改，找不到就保留 pdf2docx 原结果。
"""

import re
from typing import Dict, List, Optional, Tuple

import fitz
from docx.enum.text import WD_ALIGN_PARAGRAPH
from docx.oxml import OxmlElement
from docx.oxml.ns import qn
from docx.shared import Pt

# 页顶/页底阈值（占页高比例）：落在这个区间之外的文字视为页眉 / 页脚
TOP_RATIO = 0.15
BOTTOM_RATIO = 0.95
# 段落中心与页面中心偏差小于该值(pt)判为居中
CENTER_TOL = 15.0

DEFAULT_ASCII_FONT = "Times New Roman"
DEFAULT_EAST_ASIAN_FONT = "宋体"

# PDF 内部 PostScript 名 → Word 认识的字体名（key 已小写、去空格/横线）
_WESTERN_FONT_MAP: Dict[str, str] = {
    "timesnewromanps": "Times New Roman",
    "timesnewromanpsmt": "Times New Roman",
    "timesnewromanpsboldmt": "Times New Roman",
    "timesnewromanpsbold": "Times New Roman",
    "arialmt": "Arial",
    "arialboldmt": "Arial",
    "arial": "Arial",
    "helvetica": "Arial",
    "helveticabold": "Arial",
    "couriernewps": "Courier New",
    "couriernewpsmt": "Courier New",
    "couriernew": "Courier New",
    "courier": "Courier New",
    "symbol": "Symbol",
}

# 西文字体关键字（命中即视为西文，eastAsia 才回退宋体；key 已小写、去空格/横线）
_WESTERN_KEYWORDS = (
    "times", "arial", "helvet", "helv", "courier", "symbol", "calibri",
    "cambria", "georgia", "verdana", "tahoma", "dejavu", "liberation",
    "segoe", "roman", "garamond", "timesnew",
)


def _is_western_font(key: str) -> bool:
    """判断归一化后的字体名是否为西文字体（不含中文字形）。"""
    if key in _WESTERN_FONT_MAP:
        return True
    return any(k in key for k in _WESTERN_KEYWORDS)

W_NS = "{http://schemas.openxmlformats.org/wordprocessingml/2006/main}"


def _normalize_font_name(raw: str) -> str:
    """去掉子集前缀（ABCDEF+）与粗斜体后缀，留下字体本名。"""
    if not raw:
        return ""
    name = raw.split("+")[-1].strip()
    for suffix in ("-BoldItalic", "-Bold", "-Italic", "-Regular"):
        if name.endswith(suffix):
            name = name[: -len(suffix)]
            break
    return name.strip()


def _font_key(name: str) -> str:
    return name.lower().replace(" ", "").replace("-", "")


def _resolve_fonts(raw: str) -> Tuple[str, str]:
    """返回 (西文字体, 中文字体)。

    西文/PS 字体映射到 Word 友好名，eastAsia 回退宋体；其余（自定义中文/中日韩字体）
    在 eastAsia 槽保留原名，不再强制改宋体——引擎已保留原字体名时，本机装了该字体即可原样还原。
    """
    normalized = _normalize_font_name(raw)
    key = _font_key(normalized)
    western = _WESTERN_FONT_MAP.get(key)
    if western:
        return western, DEFAULT_EAST_ASIAN_FONT
    if _is_western_font(key):
        return normalized or DEFAULT_ASCII_FONT, DEFAULT_EAST_ASIAN_FONT
    # 非西文：视为中文字体，ascii/eastAsia 都保留原名
    return normalized or DEFAULT_ASCII_FONT, normalized or DEFAULT_EAST_ASIAN_FONT


def _apply_run_font(run, raw: str) -> None:
    """把归一化后的字体写进 run：ascii/hAnsi 用西文字体，eastAsia/cs 分槽设置。"""
    ascii_font, east_font = _resolve_fonts(raw)
    run.font.name = ascii_font  # setter 同时写 ascii 与 hAnsi
    rpr = run._element.get_or_add_rPr()
    rfonts = rpr.find(W_NS + "rFonts")
    if rfonts is not None:
        rfonts.set(W_NS + "eastAsia", east_font)
        rfonts.set(W_NS + "cs", ascii_font)



def _norm(text: str) -> str:
    """归一化文本用于匹配：压缩空白、去首尾、小写。"""
    return re.sub(r"\s+", " ", text or "").strip().lower()


def _collect_pdf_blocks(doc: fitz.Document) -> Dict[str, dict]:
    """按归一化文本收集 PDF 文本块的对齐 / 字号 / 字体 / 加粗 / 所属区域。

    返回 {norm_text: {align, size, font, bold, zone}}，zone ∈ header/body/footer。
    """
    blocks: Dict[str, dict] = {}
    for page in doc:
        pw = page.rect.width
        ph = page.rect.height
        center = pw / 2.0
        d = page.get_text("dict")
        for b in d.get("blocks", []):
            if b.get("type") != 0:
                continue
            for line in b.get("lines", []):
                spans = [s for s in line.get("spans", []) if s["text"].strip()]
                if not spans:
                    continue
                text = "".join(s["text"] for s in spans).strip()
                if not text:
                    continue
                x0 = min(s["bbox"][0] for s in spans)
                x1 = max(s["bbox"][2] for s in spans)
                y0 = min(s["bbox"][1] for s in spans)
                y1 = max(s["bbox"][3] for s in spans)
                cx = (x0 + x1) / 2.0
                if y0 < ph * TOP_RATIO:
                    zone = "header"
                elif y1 > ph * BOTTOM_RATIO:
                    zone = "footer"
                else:
                    zone = "body"

                width_ratio = (x1 - x0) / pw if pw > 0 else 1.0
                left_margin = x0 <= pw * 0.15
                right_edge = x1 >= pw * 0.85
                if left_margin and right_edge:
                    align = "justify"
                elif left_margin:
                    align = "left"
                elif right_edge and x0 > pw * 0.30:
                    align = "right"
                elif width_ratio <= 0.70 and abs(cx - center) <= CENTER_TOL:
                    align = "center"
                else:
                    # 证据不足时兜底左对齐，不贸然判居中（整宽行中心天然≈页中心）
                    align = "left"

                size = max(s["size"] for s in spans)
                font = spans[0]["font"]
                bold = any("Bold" in s["font"] for s in spans)
                blocks[_norm(text)] = {
                    "align": align,
                    "size": size,
                    "font": font,
                    "bold": bold,
                    "zone": zone,
                }
    return blocks


def _collect_vlines(doc: fitz.Document) -> List[List[float]]:
    """逐页提取竖线 x 坐标（去重排序），作为表格列边界。"""
    result: List[List[float]] = []
    for page in doc:
        xs: List[float] = []
        for draw in page.get_drawings():
            for item in draw.get("items", []):
                if item[0] == "l":
                    p1, p2 = item[1], item[2]
                    if abs(p1.x - p2.x) < 1.0 and abs(p1.y - p2.y) > 20.0:
                        xs.append(round(p1.x, 1))
                elif item[0] == "re":
                    r = item[1]
                    if r.width < 1.5 and r.height > 20.0:
                        xs.append(round(r.x0, 1))
        xs = sorted(set(xs))
        # 合并相距 <3pt 的重复线
        merged: List[float] = []
        for x in xs:
            if not merged or x - merged[-1] > 3.0:
                merged.append(x)
        result.append(merged)
    return result


def _align_enum(name: str) -> WD_ALIGN_PARAGRAPH:
    return {
        "center": WD_ALIGN_PARAGRAPH.CENTER,
        "left": WD_ALIGN_PARAGRAPH.LEFT,
        "right": WD_ALIGN_PARAGRAPH.RIGHT,
        "justify": WD_ALIGN_PARAGRAPH.JUSTIFY,
    }.get(name, WD_ALIGN_PARAGRAPH.LEFT)


def _fix_column_widths(docx, vlines: List[List[float]], warnings: List[str]) -> None:
    """按 PDF 竖线坐标修正表格列宽；列数与某页竖线数-1 匹配才应用。"""
    tables = docx.tables
    page_idx = 0
    for tbl in tables:
        n_cols = len(tbl.columns)
        lines: Optional[List[float]] = None
        # 找列数匹配的一页竖线
        for vl in vlines:
            if len(vl) - 1 == n_cols:
                lines = vl
                break
        if not lines or n_cols < 2:
            continue
        widths = [lines[i + 1] - lines[i] for i in range(n_cols)]
        total = sum(widths)
        if total <= 0:
            continue
        try:
            for col_idx, col in enumerate(tbl.columns):
                w_pt = widths[col_idx]
                col.width = Pt(w_pt)
                for cell in col.cells:
                    cell.width = Pt(w_pt)
        except Exception as e:  # 列宽修正失败不阻断主流程
            warnings.append(f"列宽修正失败: {e}")
        page_idx += 1


def _iter_all_paragraphs(docx):
    """遍历正文与表格单元格内的所有段落。"""
    for para in docx.paragraphs:
        yield para
    for tbl in docx.tables:
        for row in tbl.rows:
            for cell in row.cells:
                for para in cell.paragraphs:
                    yield para


def _fix_alignment(docx, blocks: Dict[str, dict], warnings: List[str]) -> None:
    """按 PDF 坐标校正段落对齐；命中 header/footer 区域的段落记录下来。"""
    for para in _iter_all_paragraphs(docx):
        key = _norm(para.text)
        if not key:
            continue
        info = blocks.get(key)
        if not info:
            # 退化：取前缀匹配
            for k, v in blocks.items():
                if key.startswith(k) or k.startswith(key):
                    info = v
                    break
        if not info:
            continue
        try:
            para.alignment = _align_enum(info["align"])
        except Exception as e:
            warnings.append(f"对齐修正失败: {e}")


def _copy_run_style(src_run, dst_run) -> None:
    """复制 run 文本之外的样式，含归一化后的中西文字体。"""
    _apply_run_font(dst_run, src_run.font.name or "")
    if src_run.font.size:
        dst_run.font.size = src_run.font.size
    dst_run.font.bold = src_run.font.bold


def _fix_fonts(docx, warnings: List[str]) -> None:
    """归一化正文与表格内所有 run 的字体名：PS 名映射到 Word 友好名，中西文分槽。"""
    fixed = 0
    for para in _iter_all_paragraphs(docx):
        for run in para.runs:
            try:
                _apply_run_font(run, run.font.name or "")
                fixed += 1
            except Exception as e:
                warnings.append(f"字体修正失败: {e}")
    if fixed:
        warnings.append(f"已修正 {fixed} 处字体")


def _move_footers(docx, blocks: Dict[str, dict], warnings: List[str]) -> None:
    """把 PDF 底部区域、且在正文里的段落按出现次序迁入各 section 的 footer 部件。

    标题类 header 段落常被包进无边框表格，本期不强行迁移，仅校正对齐。
    """
    footer_map = {k: v for k, v in blocks.items() if v["zone"] == "footer"}
    if not footer_map:
        return

    sections = docx.sections
    moved = 0
    sec_idx = 0
    for para in list(docx.paragraphs):
        key = _norm(para.text)
        if key not in footer_map:
            continue
        info = footer_map[key]
        section = sections[min(sec_idx, len(sections) - 1)]
        footer = section.footer
        footer.is_linked_to_previous = False
        target = footer.paragraphs[0] if footer.paragraphs else footer.add_paragraph()

        # 去重：该 footer 已有相同文本就不再追加
        if _norm(target.text) != key:
            target.alignment = _align_enum(info["align"])
            for run in para.runs:
                if not run.text:
                    continue
                nr = target.add_run(run.text)
                _copy_run_style(run, nr)

        # 清空原正文段落
        for run in list(para.runs):
            run._element.getparent().remove(run._element)
        sec_idx += 1
        moved += 1

    if moved:
        warnings.append(f"已将 {moved} 段页脚迁入 footer 部件")


def _local_tag(tag: str) -> str:
    return tag.split("}")[-1]


def _extent_pt(el) -> Tuple[float, float]:
    ext = el.find(qn("wp:extent"))
    if ext is None:
        return 0.0, 0.0
    try:
        return int(ext.get("cx")) / 12700.0, int(ext.get("cy")) / 12700.0
    except (TypeError, ValueError):
        return 0.0, 0.0


def _anchor_to_inline(anchor) -> object:
    """把浮动锚图 wp:anchor 转成内嵌 wp:inline，去掉绝对定位 / 环绕 / 衬于文字下方。"""
    inline = OxmlElement("wp:inline")
    for attr in ("distT", "distB", "distL", "distR"):
        inline.set(attr, "0")
    keep = {"extent", "docPr", "cNvGraphicFramePr", "graphic"}
    for child in list(anchor):
        if _local_tag(child.tag) in keep:
            inline.append(child)  # 移动子节点
    anchor.getparent().replace(anchor, inline)
    return inline


def _collect_usable_width(pdf: fitz.Document) -> float:
    """按源 PDF 正文文字块的左右边界算可用版心宽度（pt）。

    docx 常缺 pgMar，按 Word 默认边距会把可用宽度算小；满幅背景图/横幅会顶到页边，
    故只取文字跨度作为版心依据，避免被背景图拉成整页宽。
    """
    min_x: Optional[float] = None
    max_x: Optional[float] = None
    page_w = 346.0
    for page in pdf:
        page_w = page.rect.width
        for b in page.get_text("dict").get("blocks", []):
            for line in b.get("lines", []):
                for sp in line.get("spans", []):
                    if not sp["text"].strip():
                        continue
                    x0, _, x1, _ = sp["bbox"]
                    min_x = x0 if min_x is None else min(min_x, x0)
                    max_x = x1 if max_x is None else max(max_x, x1)
    if min_x is None or max_x is None:
        return page_w - 36.0
    return max(150.0, min(page_w, max_x - min_x))


def _scale_drawing(inline, scale: float) -> None:
    """等比缩放内嵌图的 extent 与 pic:spPr 尺寸。"""
    if abs(scale - 1.0) <= 1e-6:
        return
    ext = inline.find(qn("wp:extent"))
    if ext is not None:
        ext.set("cx", str(round(int(ext.get("cx")) * scale)))
        ext.set("cy", str(round(int(ext.get("cy")) * scale)))
    for sp_ext in inline.iter(qn("a:ext")):
        sp_ext.set("cx", str(round(int(sp_ext.get("cx")) * scale)))
        sp_ext.set("cy", str(round(int(sp_ext.get("cy")) * scale)))


def _build_side_by_side_table(
    body, para, entries: List[Tuple[object, object, float, float]], usable_pt: float
) -> None:
    """把同一自然段里的多张图搬进一个无边框 1×N 表格，按图片宽度比例分列；总宽超出可用页宽时等比缩小。"""
    n = len(entries)
    total = sum(e[2] for e in entries) or 1.0
    scale = min(1.0, usable_pt / total)
    scaled_cx = [cx * scale for _, _, cx, _ in entries]
    scaled_total = sum(scaled_cx) or 1.0
    total_twips = 9360  # 正文可用宽度（twips），按比例分配即可

    tbl = OxmlElement("w:tbl")
    tbl_pr = OxmlElement("w:tblPr")
    tbl_w = OxmlElement("w:tblW")
    tbl_w.set(qn("w:w"), "5000")
    tbl_w.set(qn("w:type"), "pct")
    tbl_pr.append(tbl_w)

    borders = OxmlElement("w:tblBorders")
    for edge in ("top", "left", "bottom", "right", "insideH", "insideV"):
        b = OxmlElement(f"w:{edge}")
        b.set(qn("w:val"), "none")
        b.set(qn("w:sz"), "0")
        b.set(qn("w:space"), "0")
        borders.append(b)
    tbl_pr.append(borders)

    cell_mar = OxmlElement("w:tblCellMar")
    for side, w in (("left", 0), ("right", 0)):
        m = OxmlElement(f"w:{side}")
        m.set(qn("w:w"), str(w))
        m.set(qn("w:type"), "dxa")
        cell_mar.append(m)
    tbl_pr.append(cell_mar)

    layout = OxmlElement("w:tblLayout")
    layout.set(qn("w:type"), "autofit")
    tbl_pr.append(layout)
    tbl.append(tbl_pr)

    grid = OxmlElement("w:tblGrid")
    widths: List[int] = []
    for cx in scaled_cx:
        w = max(200, round(cx / scaled_total * total_twips))
        widths.append(w)
        col = OxmlElement("w:gridCol")
        col.set(qn("w:w"), str(w))
        grid.append(col)
    tbl.append(grid)

    tr = OxmlElement("w:tr")
    for (drawing, inline, _cx, _cy), w in zip(entries, widths):
        _scale_drawing(inline, scale)
        tc = OxmlElement("w:tc")
        tc_pr = OxmlElement("w:tcPr")
        tc_w = OxmlElement("w:tcW")
        tc_w.set(qn("w:w"), str(w))
        tc_w.set(qn("w:type"), "dxa")
        tc_pr.append(tc_w)
        v_align = OxmlElement("w:vAlign")
        v_align.set(qn("w:val"), "center")
        tc_pr.append(v_align)
        tc.append(tc_pr)

        para_cell = OxmlElement("w:p")
        p_pr = OxmlElement("w:pPr")
        jc = OxmlElement("w:jc")
        jc.set(qn("w:val"), "center")
        p_pr.append(jc)
        para_cell.append(p_pr)

        run = drawing.getparent()  # w:r
        para_cell.append(run)
        tc.append(para_cell)
        tr.append(tc)
    tbl.append(tr)

    body.replace(para, tbl)


def _reflow_photos(docx, usable_pt: float, warnings: List[str]) -> None:
    """正文浮动照片转内嵌；同段多张并排图改无边框表格。封面背景 / 徽章保持锚定。"""
    page_h = docx.sections[0].page_height.pt if docx.sections else 792.0
    body = docx.element.body
    cover_done = False
    converted = grouped = kept_bg = 0

    for child in list(body.iterchildren()):
        if child.tag != qn("w:p"):
            continue
        drawings = child.findall(".//" + qn("w:drawing"))
        if not drawings:
            continue

        # 第一个含图段落视为封面：背景大图与右上徽章保持浮动，不参与重排
        if not cover_done:
            cover_done = True
            continue

        entries: List[Tuple[object, object, float, float]] = []
        for drawing in drawings:
            anchor = drawing.find(qn("wp:anchor"))
            if anchor is not None:
                _cx, cy = _extent_pt(anchor)
                if cy > page_h * 1.3:
                    kept_bg += 1  # 整页背景，保留浮动
                    continue
                inline = _anchor_to_inline(anchor)
                converted += 1
            else:
                inline = drawing.find(qn("wp:inline"))
            if inline is None:
                continue
            cx, cy = _extent_pt(inline)
            entries.append((drawing, inline, cx, cy))

        if len(entries) >= 2:
            _build_side_by_side_table(body, child, entries, usable_pt)
            grouped += 1
        else:
            # 单张宽图按可用页宽等比封顶
            for _drawing, inline, cx, _cy in entries:
                if cx > usable_pt:
                    _scale_drawing(inline, usable_pt / cx)

    warnings.append(f"浮动照片转内嵌 {converted} 张，并排分组 {grouped} 行，保留背景 {kept_bg} 张")


def fix(src_pdf: str, dst_docx: str) -> List[str]:
    """主入口：读取 PDF 坐标依据，修补 docx。返回警告列表。"""
    warnings: List[str] = []
    try:
        pdf = fitz.open(src_pdf)
    except Exception as e:
        warnings.append(f"PDF 坐标读取失败，跳过后处理: {e}")
        return warnings

    blocks = _collect_pdf_blocks(pdf)
    vlines = _collect_vlines(pdf)

    from docx import Document

    try:
        docx = Document(dst_docx)
    except Exception as e:
        warnings.append(f"DOCX 打开失败，跳过后处理: {e}")
        pdf.close()
        return warnings

    _fix_column_widths(docx, vlines, warnings)
    _fix_alignment(docx, blocks, warnings)
    _fix_fonts(docx, warnings)
    _move_footers(docx, blocks, warnings)
    usable_pt = _collect_usable_width(pdf)
    _reflow_photos(docx, usable_pt, warnings)

    try:
        docx.save(dst_docx)
    except Exception as e:
        warnings.append(f"后处理保存失败: {e}")
    pdf.close()
    return warnings
