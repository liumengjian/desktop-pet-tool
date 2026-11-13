"""Create editable, page-positioned Word tables from regular PDF tables."""
import fitz
from system_fonts import installed_family, name_style
from docx.shared import Pt, RGBColor
from docx.enum.table import WD_ROW_HEIGHT_RULE
from docx.oxml import OxmlElement
from docx.oxml.ns import qn


def add_page_tables(document, page, spans, font_resolver=None):
    consumed, boxes, warnings = set(), [], []
    if page.rotation:
        return 0, consumed, boxes, ['旋转页面的表格保留为原始图形和文本。'] if page.find_tables().tables else []
    drawings = page.get_drawings()
    images = [fitz.Rect(b['bbox']) for b in page.get_text('dict')['blocks'] if b.get('type') == 1]
    for candidate in page.find_tables().tables:
        if candidate.row_count < 2 or candidate.col_count < 2:
            continue
        rect = fitz.Rect(candidate.bbox)
        if any(rect.intersects(image) for image in images):
            continue
        rows = candidate.rows
        if any(len(row.cells) != candidate.col_count or any(c is None for c in row.cells) for row in rows):
            warnings.append('复杂合并表格保留为原始图形和文本。')
            continue
        cells = [[fitz.Rect(c) for c in row.cells] for row in rows]
        xs = [cells[0][0].x0] + [c.x1 for c in cells[0]]
        ys = [row[0].y0 for row in cells] + [cells[-1][0].y1]
        if any(abs(c.x0-xs[j]) > 1 or abs(c.x1-xs[j+1]) > 1 or abs(c.y0-ys[i]) > 1 or abs(c.y1-ys[i+1]) > 1 for i, row in enumerate(cells) for j, c in enumerate(row)):
            warnings.append('不规则表格保留为原始图形和文本。')
            continue
        grouped = [[[] for _ in range(candidate.col_count)] for _ in rows]
        selected = set()
        for idx, span in enumerate(spans):
            bbox = fitz.Rect(span['bbox'])
            center = (bbox.tl + bbox.br) / 2
            for i, row in enumerate(cells):
                for j, cell in enumerate(row):
                    if cell.contains(center):
                        grouped[i][j].append(span)
                        selected.add(idx)
                        break
                if idx in selected:
                    break
        table = document.add_table(rows=len(rows), cols=candidate.col_count)
        table.autofit = False
        tblpr = table._tbl.tblPr
        tblw = tblpr.find(qn('w:tblW'))
        tblw.set(qn('w:w'), str(round(rect.width * 20))); tblw.set(qn('w:type'), 'dxa')
        position = OxmlElement('w:tblpPr')
        for attr, value in {'horzAnchor':'page','vertAnchor':'page','tblpX':str(round(rect.x0*20)),'tblpY':str(round(rect.y0*20)),'leftFromText':'0','rightFromText':'0','topFromText':'0','bottomFromText':'0'}.items():
            position.set(qn('w:'+attr), value)
        tblpr.insert(0, position)
        margins = OxmlElement('w:tblCellMar')
        for side in ('top','left','bottom','right'):
            node = OxmlElement('w:'+side); node.set(qn('w:w'),'0'); node.set(qn('w:type'),'dxa'); margins.append(node)
        tblpr.append(margins)
        for j, col in enumerate(table.columns): col.width = Pt(xs[j+1]-xs[j])
        for i, row in enumerate(table.rows):
            row.height = Pt(ys[i+1]-ys[i]); row.height_rule = WD_ROW_HEIGHT_RULE.EXACTLY
            for j, cell in enumerate(row.cells):
                bounds = cells[i][j]
                cell.width = Pt(bounds.width)
                tcpr = cell._tc.get_or_add_tcPr()
                # Choose the smallest filled rectangle covering the cell center.
                fills = [d for d in drawings if d.get('fill') is not None and fitz.Rect(d['rect']).contains((bounds.tl+bounds.br)/2) and fitz.Rect(d['rect']).intersects(bounds)]
                if fills:
                    fill = min(fills, key=lambda d: abs(fitz.Rect(d['rect']).get_area()-bounds.get_area()))['fill']
                    shade = OxmlElement('w:shd'); shade.set(qn('w:fill'), ''.join(f'{max(0,min(255,round(x*255))):02X}' for x in fill[:3])); tcpr.append(shade)
                borders = OxmlElement('w:tcBorders')
                for side in ('top', 'bottom', 'left', 'right'):
                    target = bounds.y0 if side == 'top' else bounds.y1 if side == 'bottom' else bounds.x0 if side == 'left' else bounds.x1
                    found = None
                    for drawing in drawings:
                        if drawing.get('color') is None:
                            continue
                        segments = []
                        for item in drawing.get('items', []):
                            if item[0] == 'l': segments.append((item[1], item[2]))
                            elif item[0] == 're':
                                r = fitz.Rect(item[1]); segments.extend([(r.tl,r.tr),(r.bl,r.br),(r.tl,r.bl),(r.tr,r.br)])
                        for a, b in segments:
                            if side in ('top','bottom'):
                                match = abs(a.y-target)<1 and abs(b.y-target)<1 and min(a.x,b.x)<=bounds.x0+1 and max(a.x,b.x)>=bounds.x1-1
                            else:
                                match = abs(a.x-target)<1 and abs(b.x-target)<1 and min(a.y,b.y)<=bounds.y0+1 and max(a.y,b.y)>=bounds.y1-1
                            if match: found = drawing; break
                        if found: break
                    edge = OxmlElement('w:'+side)
                    edge.set(qn('w:val'), 'single' if found else 'nil')
                    if found:
                        edge.set(qn('w:sz'), str(max(2, min(96,round(found.get('width',1)*8)))))
                        edge.set(qn('w:color'), ''.join(f'{max(0,min(255,round(c*255))):02X}' for c in found['color'][:3]))
                    borders.append(edge)
                tcpr.append(borders)
                ordered = sorted(grouped[i][j], key=lambda span: (span.get('origin', (0, span['bbox'][3]))[1], span['bbox'][0]))
                lines = []
                for span in ordered:
                    baseline = span.get('origin', (0, span['bbox'][3]))[1]
                    if not lines or abs(baseline - lines[-1][0]) > 2:
                        lines.append([baseline, [span]])
                    else:
                        lines[-1][1].append(span)
                for line_index, (baseline, line_spans) in enumerate(lines):
                    line_spans.sort(key=lambda span: span['bbox'][0])
                    paragraph = cell.paragraphs[0] if line_index == 0 else cell.add_paragraph()
                    fmt = paragraph.paragraph_format
                    fmt.left_indent = Pt(max(0, line_spans[0]['bbox'][0] - bounds.x0))
                    fmt.space_before = Pt(max(0, min(s['bbox'][1] for s in line_spans) - bounds.y0)) if line_index == 0 else Pt(0)
                    fmt.space_after = Pt(0)
                    if line_index + 1 < len(lines):
                        fmt.line_spacing = Pt(max(1, lines[line_index + 1][0] - baseline))
                    for span in line_spans:
                        raw = span.get('font','Arial')
                        font = font_resolver(raw, span['text']) if font_resolver else raw.split('+')[-1]
                        run = paragraph.add_run(span['text']); run.font.name = font; run.font.size = Pt(span['size'])
                        run.font.color.rgb = RGBColor.from_string(f'{span.get("color",0):06X}')
                        normalized = raw.lower().replace('-', '').replace('_', '').replace(' ', '')
                        system = installed_family(raw) == font
                        if span.get('flags',0) & 16 and (system or not any(x in normalized for x in ('bold','black','heavy','demibold','semibold'))): run.bold = True
                        if span.get('flags',0) & 2 and (system or not any(x in normalized for x in ('italic','oblique'))): run.italic = True
                        if not font.startswith('PDFEmbedded'):
                            name_bold, name_italic = name_style(raw)
                            run.bold = bool(span.get('flags', 0) & 16 or name_bold)
                            run.italic = bool(span.get('flags', 0) & 2 or name_italic)
                        fonts = run._element.get_or_add_rPr().rFonts
                        fonts.set(qn('w:eastAsia'), font); fonts.set(qn('w:cs'), font)
        consumed.update(selected); boxes.append(tuple(rect))
    return len(boxes), consumed, boxes, warnings
