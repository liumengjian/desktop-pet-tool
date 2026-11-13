"""Convert PDF vector drawings to independently editable Word DrawingML shapes."""
from docx.oxml import parse_xml
import fitz


def _is_white(color):
    return all(channel >= 0.97 for channel in color[:3])


def add_page_vectors(paragraph, page, layer_base=1, exclude_rects=()):
    """Add individual drawing paths and return (shape_count, warnings)."""
    warnings = []
    count = 0
    matrix = page.rotation_matrix
    # 之前画过的非白色填充块：只有落在它们上面的白块才是「有意义的白底」
    colored_fills = []
    for drawing in page.get_drawings():
        if any((fitz.Rect(rect) + (-1, -1, 1, 1)).contains(fitz.Rect(drawing['rect'])) for rect in exclude_rects):
            continue
        commands = []
        points = []
        unsupported = False

        def point(p):
            p = fitz.Point(p) * matrix
            points.append((p.x, p.y))
            return len(points) - 1

        def move(p):
            commands.append(('m', [point(p)]))

        def line(p):
            commands.append(('l', [point(p)]))

        last = None
        for item in drawing.get('items', []):
            op = item[0]
            if op == 'l':
                if last is None or fitz.Point(last) != fitz.Point(item[1]):
                    move(item[1])
                line(item[2]); last = item[2]
            elif op == 'c':
                if last is None or fitz.Point(last) != fitz.Point(item[1]):
                    move(item[1])
                commands.append(('c', [point(p) for p in item[2:5]])); last = item[4]
            elif op == 're':
                rect = fitz.Rect(item[1])
                corners = [rect.tl, rect.tr, rect.br, rect.bl]
                if len(item) > 2 and item[2] < 0:
                    corners.reverse()
                move(corners[0])
                for p in corners[1:]: line(p)
                commands.append(('x', [])); last = None
            elif op == 'qu':
                quad = item[1]
                corners = [quad.ul, quad.ur, quad.lr, quad.ll]
                move(corners[0])
                for p in corners[1:]: line(p)
                commands.append(('x', [])); last = None
            else:
                unsupported = True
        if unsupported:
            warnings.append('PDF 页面包含暂不支持的矢量路径，部分图形可能缺失。')
        if not points:
            continue
        if drawing.get('closePath'):
            commands.append(('x', []))
        x0 = min(p[0] for p in points); y0 = min(p[1] for p in points)
        width = max(max(p[0] for p in points) - x0, .01)
        height = max(max(p[1] for p in points) - y0, .01)
        scale = 1000
        path = []
        for op, indexes in commands:
            pts = ''.join(f'<a:pt x="{round((points[i][0]-x0)*scale)}" y="{round((points[i][1]-y0)*scale)}"/>' for i in indexes)
            tags = {'m':'moveTo', 'l':'lnTo', 'c':'cubicBezTo', 'x':'close'}
            path.append(f'<a:{tags[op]}>{pts}</a:{tags[op]}>')
        def color(value):
            return ''.join(f'{max(0,min(255,round(c*255))):02X}' for c in value[:3])
        def solid(value, opacity):
            if value is None:
                return '<a:noFill/>'
            return f'<a:solidFill><a:srgbClr val="{color(value)}"><a:alpha val="{round(opacity*100000)}"/></a:srgbClr></a:solidFill>'
        if drawing.get('dashes') not in (None, '[] 0', '[] 0.0'):
            warnings.append('PDF 页面包含虚线，线型可能有偏差。')
        stroke = drawing.get('color')
        fill = drawing.get('fill')
        rect = fitz.Rect(drawing['rect'])
        if fill is not None:
            # 页面底色本来就是白：再把白块当成独立形状放上去，只会按层级压住
            # 照片和表格线（这正是「空白图层盖住图片 / 白色色块遮住边框」的成因）。
            if _is_white(fill) and drawing.get('fill_opacity', 1) > 0.9:
                if not any(prev.contains(rect) for prev in colored_fills):
                    fill = None
            else:
                colored_fills.append(rect)
        cx = max(1, round(width*12700)); cy = max(1, round(height*12700))
        rank = max(0, layer_base + drawing.get('seqno', count))
        shape_id = paragraph.part.next_id
        fill_xml = solid(fill, drawing.get('fill_opacity', 1))
        line_xml = solid(stroke, drawing.get('stroke_opacity', 1))
        xml = (f'<w:drawing xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" '
               f'xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" '
               f'xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" '
               f'xmlns:wps="http://schemas.microsoft.com/office/word/2010/wordprocessingShape">'
               f'<wp:anchor distT="0" distB="0" distL="0" distR="0" simplePos="0" relativeHeight="{rank}" '
               f'behindDoc="1" locked="0" layoutInCell="1" allowOverlap="1">'
               f'<wp:simplePos x="0" y="0"/><wp:positionH relativeFrom="page"><wp:posOffset>{round(x0*12700)}</wp:posOffset></wp:positionH>'
               f'<wp:positionV relativeFrom="page"><wp:posOffset>{round(y0*12700)}</wp:posOffset></wp:positionV>'
               f'<wp:extent cx="{cx}" cy="{cy}"/><wp:wrapNone/><wp:docPr id="{shape_id}" name="PDF vector {page.number + 1}-{count + 1}"/>'
               f'<a:graphic><a:graphicData uri="http://schemas.microsoft.com/office/word/2010/wordprocessingShape">'
               f'<wps:wsp><wps:cNvSpPr/><wps:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="{cx}" cy="{cy}"/></a:xfrm>'
               f'<a:custGeom><a:avLst/><a:gdLst/><a:ahLst/><a:cxnLst/><a:rect l="l" t="t" r="r" b="b"/>'
               f'<a:pathLst><a:path w="{max(1,round(width*scale))}" h="{max(1,round(height*scale))}">{"".join(path)}</a:path></a:pathLst></a:custGeom>'
               f'{fill_xml}<a:ln w="{max(1,round((drawing.get("width") or 1)*12700))}">{line_xml}</a:ln>'
               f'</wps:spPr><wps:bodyPr/></wps:wsp></a:graphicData></a:graphic></wp:anchor></w:drawing>')
        paragraph.add_run()._r.append(parse_xml(xml))
        count += 1
    return count, list(dict.fromkeys(warnings))

