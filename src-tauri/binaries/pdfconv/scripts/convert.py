# -*- coding: utf-8 -*-
"""PDF → DOCX sidecar 入口（随包 embeddable Python 调用）。

用法:
    python.exe convert.py --src <input.pdf> --dst <output.docx> --report <report.json>

退出码:
    0  转换成功（含后处理警告，详见 report.json）
    1  其他失败（report.json 里写 message）
"""

import argparse
import json
import os
import sys
import traceback

# 保证同目录的 postprocess 可被导入（embeddable 环境下脚本目录不一定在 sys.path）
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

# 外置 site-packages（由 Rust 侧从 OSS 下载到用户数据目录），通过环境变量传入
_ext_pkgs = os.environ.get("PDFCONV_SITE_PACKAGES")
if _ext_pkgs:
    sys.path.insert(0, _ext_pkgs)

ENGINE = "pymupdf-docx"
ENGINE_VERSION = "4"



def _covers(first, second) -> bool:
    """两个 bbox 是否几乎重合（同一个字被描边/阴影重复画了两层）。"""
    overlap_x = min(first[2], second[2]) - max(first[0], second[0])
    overlap_y = min(first[3], second[3]) - max(first[1], second[1])
    if overlap_x <= 0 or overlap_y <= 0:
        return False
    areas = ((first[2]-first[0])*(first[3]-first[1]), (second[2]-second[0])*(second[3]-second[1]))
    smallest = min(areas)
    if smallest <= 0:
        return False
    return overlap_x * overlap_y / smallest > 0.6


def _luminance(span) -> int:
    color = span.get("color") or 0
    return (color >> 16 & 255) + (color >> 8 & 255) + (color & 255)


def _overlap_ratio(bbox, rect) -> float:
    """bbox 有多大比例落在 rect 里。"""
    left = max(bbox[0], rect.x0)
    top = max(bbox[1], rect.y0)
    right = min(bbox[2], rect.x1)
    bottom = min(bbox[3], rect.y1)
    if right <= left or bottom <= top:
        return 0.0
    area = (bbox[2] - bbox[0]) * (bbox[3] - bbox[1])
    if area <= 0:
        return 0.0
    return (right - left) * (bottom - top) / area


def _merge_boxes(boxes):
    """把相互重叠的矩形并成一块，避免同一处图形被贴两次。"""
    groups = []
    for box in boxes:
        for i, group in enumerate(groups):
            if (group & box).get_area() > 0:
                groups[i] = group | box
                break
        else:
            groups.append(box)
    return groups


def _write_report(path: str, payload: dict) -> None:
    try:
        with open(path, "w", encoding="utf-8") as f:
            json.dump(payload, f, ensure_ascii=False, indent=2)
    except Exception:
        pass


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--src", required=True)
    parser.add_argument("--dst", required=True)
    parser.add_argument("--report", required=True)
    args = parser.parse_args()

    report = {
        "status": "error",
        "engine": ENGINE,
        "engine_version": ENGINE_VERSION,
        "pages": 0,
        "tables": 0,
        "warnings": [],
        "message": "",
    }

    # 延迟到这里再 import，保证 report 能在依赖缺失时写出
    try:
        import fitz
    except Exception as e:
        report["message"] = f"转换引擎初始化失败: {e}"
        _write_report(args.report, report)
        return 1

    try:
        from io import BytesIO
        from docx import Document
        from docx.enum.section import WD_SECTION
        from docx.oxml import OxmlElement, parse_xml
        from docx.oxml.ns import qn
        from docx.shared import Pt

        def add_picture(paragraph, data, x, y, width, height, name, layer, rotation=0, flip=False, crop=None):
            inline = paragraph.add_run().add_picture(BytesIO(data), width=Pt(width), height=Pt(height))._inline
            inline.find(qn("wp:docPr")).set("name", name)
            if crop and any(crop):
                blip_fill = inline.find('.//' + qn('pic:blipFill'))
                source_rect = OxmlElement('a:srcRect')
                for side, value in zip(('l', 't', 'r', 'b'), crop):
                    source_rect.set(side, str(round(value * 100000)))
                blip_fill.insert(1, source_rect)
            transform = inline.find('.//' + qn('a:xfrm'))
            if transform is not None:
                if rotation:
                    transform.set('rot', str(round(rotation * 60000)))
                if flip:
                    transform.set('flipV', '1')
            anchor = OxmlElement("wp:anchor")
            for key, value in {"distT":"0", "distB":"0", "distL":"0", "distR":"0", "simplePos":"0", "relativeHeight":str(layer), "behindDoc":"1", "locked":"0", "layoutInCell":"1", "allowOverlap":"1"}.items():
                anchor.set(key, value)
            simple = OxmlElement("wp:simplePos")
            simple.set("x", "0"); simple.set("y", "0")
            anchor.append(simple)
            for axis, coordinate in (("H", x), ("V", y)):
                position = OxmlElement("wp:position" + axis)
                position.set("relativeFrom", "page")
                offset = OxmlElement("wp:posOffset")
                offset.text = str(round(coordinate * 12700))
                position.append(offset)
                anchor.append(position)
            for tag in ("extent", "effectExtent"):
                child = inline.find(qn("wp:" + tag))
                if child is not None:
                    anchor.append(child)
            anchor.append(OxmlElement("wp:wrapNone"))
            for tag in ("docPr", "cNvGraphicFramePr"):
                child = inline.find(qn("wp:" + tag))
                if child is not None:
                    anchor.append(child)
            anchor.append(inline.find(qn("a:graphic")))
            inline.getparent().replace(inline, anchor)

        document = Document()
        text_count = 0
        from image_clips import image_clip_rects
        from vector_shapes import add_page_vectors
        from native_tables import add_page_tables
        from text_boxes import add_textbox, add_para_box, paragraph_groups
        from embed_fonts import embed_pdf_fonts, font_aliases
        from system_fonts import installed_family
        with fitz.open(args.src) as pdf:
            report["pages"] = pdf.page_count
            if not pdf.page_count:
                raise ValueError("PDF 没有页面")
            report["warnings"].extend(embed_pdf_fonts(document, pdf))
            aliases = font_aliases(pdf)
            font_table = document.part.part_related_by("http://schemas.openxmlformats.org/officeDocument/2006/relationships/fontTable")
            available = {node.get(qn("w:name")) for node in parse_xml(font_table.blob)}
            def resolve_font(raw, text=''):
                name = raw.split("+")[-1]
                installed = installed_family(name)
                if installed:
                    return installed
                # User preference: missing original Latin fonts use Microsoft YaHei.
                if any('A' <= char <= 'Z' or 'a' <= char <= 'z' for char in text):
                    return 'Microsoft YaHei'
                alias = aliases.get(name)
                return alias if alias in available else name
            for index, page in enumerate(pdf):
                section = document.sections[0] if index == 0 else document.add_section(WD_SECTION.NEW_PAGE)
                section.page_width = Pt(page.rect.width)
                section.page_height = Pt(page.rect.height)
                section.left_margin = section.right_margin = Pt(0)
                section.top_margin = section.bottom_margin = Pt(0)
                section.header_distance = section.footer_distance = Pt(0)
                paragraph = document.add_paragraph()
                paragraph.paragraph_format.space_before = Pt(0)
                paragraph.paragraph_format.space_after = Pt(0)
                paragraph.paragraph_format.line_spacing = Pt(1)
                spans = []
                blocks = page.get_text("dict")["blocks"]
                photos = [block for block in page.get_text("dict", clip=fitz.INFINITE_RECT())["blocks"] if block.get("type") == 1]
                # 按 block → line → span 记录层级：后面的段落分组要靠它知道哪几行属于同一段
                page_blocks = []
                for block in blocks:
                    lines = []
                    for line in block.get("lines", []):
                        candidates = [span for span in line.get("spans", []) if span.get("text", "")]
                        if not candidates:
                            continue
                        kept = []
                        for span in candidates:
                            # 描边/阴影文字会画两层（深色底 + 白色主字）。两层都写进 Word
                            # 会叠成重影，这里保留更亮的那一层（白色主字），丢掉深色投影。
                            duplicate = next((k for k in kept if _covers(k["bbox"], span["bbox"])), None)
                            if duplicate is None:
                                kept.append(span)
                            elif _luminance(span) > _luminance(duplicate):
                                kept[kept.index(duplicate)] = span
                        indexes = []
                        for span in kept:
                            indexes.append(len(spans))
                            spans.append(span)
                            text_count += 1
                        lines.append({"bbox": line.get("bbox"), "indexes": indexes})
                    if lines:
                        page_blocks.append(lines)
                # Lines and filled regions must remain individual editable objects.
                # A page-sized raster background would merge table borders and decorations.
                table_count, table_spans, table_boxes, table_warnings = add_page_tables(document, page, spans, font_resolver=resolve_font)
                report["tables"] += table_count
                report["warnings"].extend(table_warnings)
                vector_count, vector_warnings = add_page_vectors(paragraph, page, exclude_rects=table_boxes)
                report["warnings"].extend(vector_warnings)
                import math
                # Drawing and image sequence numbers refer to the same PDF display list.
                svg_clips = image_clip_rects(page)
                if len(svg_clips) != len(photos):
                    report["warnings"].append(f"第 {index + 1} 页的图片裁剪顺序无法确定，部分图片可能显示不完整。")
                    svg_clips = [None] * len(photos)
                image_layers = [(seq + 1, fitz.Rect(box)) for seq, (kind, box) in enumerate(page.get_bboxlog()) if kind == 'fill-image']
                unmatched = []
                strips = []
                for photo_index, photo in enumerate(photos):
                    matching = next((n for n, (_, rect) in enumerate(image_layers) if all(abs(a-b)<.1 for a,b in zip(rect,photo["bbox"]))), None)
                    if matching is None:
                        box = fitz.Rect(photo["bbox"]) & page.rect
                        if box.height <= 24 and box.width / max(box.height, 1) >= 2.5:
                            # 又宽又扁的一条：排版软件常把「标题条」整体画成一张图
                            # （蓝底 + 白色标题）。整条贴回，重叠的真实文字稍后剔除。
                            strips.append(box)
                            continue
                        # 页面显示列表里查不到这张图（内联小图、表单里的阴影/蒙版底图）。
                        # 逐张导出会丢掉蒙版语义，变成不透明色块盖住图标，
                        # 所以这一组图整块交给栅格，保证画面和原 PDF 一致。
                        unmatched.append(box)
                        continue
                    layer = image_layers.pop(matching)[0]
                    pixmap = fitz.Pixmap(photo["image"])
                    if photo.get("mask"):
                        # 合成软蒙版时 color pixmap 必须不带 alpha：PDF 里取出的图常是 RGBA，
                        # 直接喂给 fz_new_pixmap_from_color_and_mask 会抛 code=4 并让整页失败。
                        try:
                            if pixmap.alpha:
                                pixmap = fitz.Pixmap(pixmap, 0)
                            pixmap = fitz.Pixmap(pixmap, fitz.Pixmap(photo["mask"]))
                        except Exception as mask_error:
                            # 蒙版合成失败不该连累整份文档，按不带蒙版导出并记一条警告。
                            report["warnings"].append(f"图像蒙版合成失败，已按不带蒙版导出: {mask_error}")
                    if pixmap.colorspace and pixmap.colorspace.n > 3:
                        pixmap = fitz.Pixmap(fitz.csRGB, pixmap)
                    data = pixmap.tobytes("png")
                    matrix = fitz.Matrix(photo["transform"]) * page.rotation_matrix
                    full_width = math.hypot(matrix.a, matrix.b)
                    full_height = math.hypot(matrix.c, matrix.d)
                    if not full_width or not full_height:
                        report["warnings"].append("跳过尺寸为零的图片。")
                        continue
                    angle = math.degrees(math.atan2(matrix.b, matrix.a)) % 360
                    visible = (fitz.Rect(photo["bbox"]) * page.rotation_matrix) & page.rect
                    if svg_clips[photo_index] is not None:
                        visible &= svg_clips[photo_index]
                    else:
                        report["warnings"].append(f"第 {index + 1} 页包含无法还原的复杂图片裁剪。")
                    if visible.is_empty:
                        continue
                    # Orthogonal transforms preserve rectangular crop regions under inversion.
                    if abs((angle % 90)) < .01 or abs((angle % 90) - 90) < .01:
                        inverse = ~matrix
                        source = visible * inverse
                        u0 = max(0.0, min(1.0, source.x0)); v0 = max(0.0, min(1.0, source.y0))
                        u1 = max(u0, min(1.0, source.x1)); v1 = max(v0, min(1.0, source.y1))
                        crop = (u0, v0, 1-u1, 1-v1)
                        width = full_width * (u1-u0)
                        height = full_height * (v1-v0)
                        x = (visible.x0 + visible.x1 - width) / 2
                        y = (visible.y0 + visible.y1 - height) / 2
                    else:
                        # Non-orthogonal clip paths are not rectangular in image coordinates.
                        crop = None
                        width, height = full_width, full_height
                        center = fitz.Point(.5, .5) * matrix
                        x, y = center.x - width/2, center.y - height/2
                        report["warnings"].append("倾斜图片的复杂裁剪可能有偏差。")
                    if width < .01 or height < .01:
                        continue
                    add_picture(paragraph, data, x, y, width, height,
                                f"PDF photo {index + 1}-{photo_index + 1}", layer, angle,
                                matrix.a * matrix.d - matrix.b * matrix.c < 0, crop)
                if unmatched:
                    for group_number, box in enumerate(_merge_boxes(unmatched)):
                        if box.is_empty or page.rotation != 0:
                            continue
                        raster = page.get_pixmap(clip=box, matrix=fitz.Matrix(2, 2))
                        add_picture(paragraph, raster.tobytes("png"), box.x0, box.y0,
                                    box.width, box.height, f"PDF art {index + 1}-{group_number + 1}", 2)
                    if page.rotation == 0:
                        report["warnings"].append(
                            f"第 {index + 1} 页有一组无法逐张还原的图（阴影/蒙版），已合并保留为一块图像。")
                if strips:
                    for group_number, box in enumerate(_merge_boxes(strips)):
                        if box.is_empty or page.rotation != 0:
                            continue
                        raster = page.get_pixmap(clip=box, matrix=fitz.Matrix(2, 2))
                        add_picture(paragraph, raster.tobytes("png"), box.x0, box.y0,
                                    box.width, box.height, f"PDF strip {index + 1}-{group_number + 1}", 200)
                    # 标题已经随栅格贴回，同位置的文字再写一遍就是重影，这里剔掉。
                    kept_blocks = []
                    for lines in page_blocks:
                        kept_lines = []
                        for line in lines:
                            kept_indexes = [i for i in line["indexes"]
                                            if max(_overlap_ratio(spans[i]["bbox"], box) for box in strips) <= .3]
                            if kept_indexes:
                                kept_lines.append(dict(line, indexes=kept_indexes))
                        if kept_lines:
                            kept_blocks.append(kept_lines)
                    page_blocks = kept_blocks
                # 一段文字只放一个文本框：先按行聚成段落，再整段写入，
                # 行数多的段落会保留换行，不会变成「一行一个框」。
                groups = (paragraph_groups(page_blocks, spans, table_spans) if not page.rotation
                          else [[{"bbox": span["bbox"], "indexes": [i]}]
                                for i, span in enumerate(spans) if i not in table_spans])
                for group_number, group in enumerate(groups):
                    lines = [dict(line, spans=[spans[i] for i in line["indexes"]]) for line in group]
                    if len(lines) == 1 and len(lines[0]["spans"]) == 1:
                        span = lines[0]["spans"][0]
                        add_textbox(paragraph, page, span,
                                    resolve_font(span.get("font", "Arial"), span["text"]),
                                    1000 + group_number)
                    else:
                        add_para_box(paragraph, page, lines,
                                     resolve_font, 1000 + group_number)
        if not text_count:
            report["warnings"].append("该 PDF 没有可提取的文字，生成的文档没有可编辑文本。")
        document.save(args.dst)
        report["warnings"] = list(dict.fromkeys(report["warnings"]))
        report["warnings"].append("照片、线条、色块分别保留为独立对象，规则表格保留为可编辑表格；复杂裁剪或图文交错可能有偏差。")

        report["status"] = "ok"
        _write_report(args.report, report)
        return 0

    except Exception as e:
        report["message"] = f"{e}"
        report["warnings"].append(traceback.format_exc(limit=3))
        _write_report(args.report, report)
        return 1


if __name__ == "__main__":
    sys.exit(main())
