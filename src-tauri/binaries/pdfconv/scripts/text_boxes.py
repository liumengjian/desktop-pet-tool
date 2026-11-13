"""Editable text frames anchored to the page, with wrap and overflow control."""
from xml.sax.saxutils import escape, quoteattr
from docx.oxml import parse_xml
from system_fonts import name_style


def _emu(points) -> int:
    return round(points * 12700)


def _run_xml(span, font, identifier, fit_width=None, scale=1.0):
    """单个文字 run：字体、粗细、颜色、字号都跟着 PDF 的 span 走。"""
    size = span['size'] * scale
    # PDF 子集名常把字重写在名字里（Arial,Bold / MicrosoftYaHei-Bold），
    # 光看 flags 会漏掉粗体，光看名字会丢掉真正的字重标志，两者取或。
    name_bold, name_italic = name_style(span.get('font', ''))
    bold = '<w:b/>' if (span.get('flags', 0) & 16 or name_bold) else ''
    italic = '<w:i/>' if (span.get('flags', 0) & 2 or name_italic) else ''
    if font.startswith('PDFEmbedded'):
        # Each embedded family already contains the source face's styled outlines.
        # Asking for a missing Bold/Italic slot can trigger a different fallback font.
        bold, italic = '<w:b w:val="0"/>', '<w:i w:val="0"/>'
    fit = f'<w:fitText w:val="{fit_width}" w:id="{identifier}"/>' if fit_width else ''
    return (
        f'<w:r><w:rPr>'
        f'<w:rFonts w:ascii={quoteattr(font)} w:hAnsi={quoteattr(font)} '
        f'w:eastAsia={quoteattr(font)} w:cs={quoteattr(font)}/>'
        f'{bold}{italic}<w:color w:val="{span.get("color", 0):06X}"/>'
        f'<w:sz w:val="{max(1, round(size * 2))}"/><w:szCs w:val="{max(1, round(size * 2))}"/>'
        f'{fit}</w:rPr>'
        f'<w:t xml:space="preserve">{escape(span["text"])}</w:t></w:r>'
    )


def _frame(page, x0, y0, width, height, identifier, layer, rotation, body, wrap=False):
    # 单行小框不换行，靠 fitText 压回原宽；段落框允许换行，避免整行跑出页面被裁掉。
    if wrap:
        flow = 'wrap="square" horzOverflow="overflow"'
    else:
        flow = 'wrap="none" horzOverflow="overflow"'
    return f'''<w:drawing xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"
      xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing"
      xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"
      xmlns:wps="http://schemas.microsoft.com/office/word/2010/wordprocessingShape">
      <wp:anchor distT="0" distB="0" distL="0" distR="0" simplePos="0"
        relativeHeight="{layer}" behindDoc="0" locked="0" layoutInCell="0" allowOverlap="1">
        <wp:simplePos x="0" y="0"/>
        <wp:positionH relativeFrom="page"><wp:posOffset>{_emu(x0)}</wp:posOffset></wp:positionH>
        <wp:positionV relativeFrom="page"><wp:posOffset>{_emu(y0)}</wp:posOffset></wp:positionV>
        <wp:extent cx="{_emu(width)}" cy="{_emu(height)}"/><wp:wrapNone/>
        <wp:docPr id="{identifier}" name="PDF text {page.number + 1}-{identifier}"/>
        <wp:cNvGraphicFramePr/>
        <a:graphic><a:graphicData uri="http://schemas.microsoft.com/office/word/2010/wordprocessingShape">
          <wps:wsp><wps:cNvSpPr txBox="1"/><wps:spPr>
            <a:xfrm rot="{rotation * 60000}"><a:off x="0" y="0"/><a:ext cx="{_emu(width)}" cy="{_emu(height)}"/></a:xfrm>
            <a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:noFill/><a:ln><a:noFill/></a:ln>
          </wps:spPr><wps:txbx><w:txbxContent>{body}</w:txbxContent></wps:txbx>
          <wps:bodyPr rot="0" vert="horz" {flow} lIns="0" tIns="0" rIns="0" bIns="0"
            anchor="t" vertOverflow="overflow"><a:noAutofit/></wps:bodyPr>
          </wps:wsp></a:graphicData></a:graphic>
      </wp:anchor></w:drawing>'''


def add_textbox(paragraph, page, span, font, layer):
    """单行单样式：保留 fitText，让文字宽度和 PDF 里的排版尽量一致。"""
    size = span['size']
    x0, y0, x1, y1 = span['bbox']
    # The PDF glyph box is not a Word line box. Leave room for line metrics,
    # and allow overflow rather than hiding the end of a title on font fallback.
    width = max(1, x1 - x0 + max(6, size * .3))
    height = max(y1 - y0 + 6, size * 2)
    rotation = page.rotation % 360
    if rotation:
        import fitz
        rect = fitz.Rect(span['bbox']) * page.rotation_matrix
        x0, y0 = (rect.x0 + rect.x1 - width) / 2, (rect.y0 + rect.y1 - height) / 2
    identifier = paragraph.part.next_id
    body = (f'<w:p><w:pPr><w:spacing w:before="0" w:after="0"/>'
            f'<w:snapToGrid w:val="0"/><w:wordWrap w:val="0"/></w:pPr>'
            f'{_run_xml(span, font, identifier, max(1, round((x1 - x0) * 20)))}</w:p>')
    paragraph.add_run()._r.append(parse_xml(
        _frame(page, x0, y0, width, height, identifier, layer, rotation, body)))


def add_para_box(paragraph, page, lines, resolve_font, layer):
    """把同一段落的若干行放进同一个文本框：段内换行，不再是「一行一个框」。"""
    spans = [span for line in lines for span in line['spans']]
    size = max(span['size'] for span in spans)
    x0 = min(span['bbox'][0] for span in spans)
    x1 = max(span['bbox'][2] for span in spans)
    y0 = lines[0]['bbox'][1]
    y1 = lines[-1]['bbox'][3]
    width = max(1, x1 - x0 + max(8, size * .5))
    # 字体替换后同样的文字会变宽，框太宽会让整行溢出到页面外面，收窄到页内。
    width = min(width, max(1, page.rect.width - x0 - 1))
    height = max(y1 - y0 + 6, size * 2)
    pitch = 0
    if len(lines) > 1:
        pitch = (lines[-1]['bbox'][1] - lines[0]['bbox'][1]) / (len(lines) - 1)
        if pitch > 0:
            height = max(height, pitch * (len(lines) - 1) + size * 1.45)
    # 字体替换后同样的字符会更宽（Word 里 fitText 还能压回去，WPS 直接忽略），
    # 所以超宽的行按比例把字号收一点，保证整行还在页面里，不做换行也不改版式。
    extra = 0
    lefts = [line['bbox'][0] for line in lines]
    centers = [(line['bbox'][0] + line['bbox'][2]) / 2 for line in lines]
    if max(lefts) - min(lefts) <= 1.6:
        align = 'left'
    elif max(centers) - min(centers) <= 1.6:
        align = 'center'
    else:
        align = 'left'
    identifier = paragraph.part.next_id
    runs = []
    for number, line in enumerate(lines):
        if number:
            runs.append('<w:r><w:br/></w:r>')
        line_width = max(span['bbox'][2] for span in line['spans']) - line['bbox'][0]
        estimate = line_width * 1.10
        scale = min(1.0, width / estimate) if estimate > width > 0 else 1.0
        for span in line['spans']:
            font = resolve_font(span.get('font', 'Arial'), span['text'])
            runs.append(_run_xml(span, font, identifier, scale=scale))
    spacing = (f'<w:spacing w:line="{max(1, round(pitch * 20))}" w:lineRule="exact"/>'
               if pitch > 0 else '<w:spacing w:before="0" w:after="0"/>')
    body = (f'<w:p><w:pPr>{spacing}<w:jc w:val="{align}"/>'
            f'<w:snapToGrid w:val="0"/></w:pPr>'
            f'{"".join(runs)}</w:p>')
    paragraph.add_run()._r.append(parse_xml(
        _frame(page, x0, y0, width, height, identifier, layer, page.rotation % 360, body)))


def paragraph_groups(page_blocks, spans, table_spans):
    """把属于同一段的多行合成一个组。

    排版软件经常把每一行单独放进一个文本块，因此不能只按 block 归组：
    这里先按阅读顺序展平所有行，再按行距、字号、对齐和横向重叠判断换段。
    """
    lines = [line for block in page_blocks for line in block]
    lines.sort(key=lambda line: (round(line['bbox'][1], 1), line['bbox'][0]))
    groups = []
    current = []
    for line in lines:
        if any(index in table_spans for index in line['indexes']):
            if current:
                groups.append(current)
                current = []
            continue
        if current:
            prev = current[-1]
            prev_size = max(spans[i]['size'] for i in prev['indexes'])
            line_size = max(spans[i]['size'] for i in line['indexes'])
            height = max(prev['bbox'][3] - prev['bbox'][1], 1)
            gap = line['bbox'][1] - prev['bbox'][3]
            same_left = abs(line['bbox'][0] - prev['bbox'][0]) <= 1.6
            same_center = abs((line['bbox'][0] + line['bbox'][2]) / 2
                              - (prev['bbox'][0] + prev['bbox'][2]) / 2) <= 1.6
            overlap = min(line['bbox'][2], prev['bbox'][2]) - max(line['bbox'][0], prev['bbox'][0])
            # 行距明显变大、字号跳变、对不齐、或两行不在同一横向区间（分栏），都说明已经是另一段。
            if (gap > max(2.5, height * 0.65)
                    or gap < -1.0
                    or abs(line_size - prev_size) > max(2.0, prev_size * 0.3)
                    or not (same_left or same_center)
                    or overlap <= 0):
                groups.append(current)
                current = []
        current.append(line)
    if current:
        groups.append(current)
    return groups
