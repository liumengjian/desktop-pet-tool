"""Read image clipping ancestry from MuPDF's SVG display list."""
import re
import xml.etree.ElementTree as ET
import fitz

_NUM = r'[-+]?(?:\d*\.\d+|\d+\.?\d*)(?:[eE][-+]?\d+)?'

def image_clip_rects(page):
    root = ET.fromstring(page.get_svg_image(text_as_path=False))
    tag = lambda element: element.tag.rsplit('}', 1)[-1]
    clips = {}
    for node in root.iter():
        if tag(node) != 'clipPath':
            continue
        children = list(node)
        if len(children) != 1 or tag(children[0]) != 'path':
            clips[node.get('id')] = None; continue
        shape = children[0]
        path = shape.get('d', '')
        # Only axis-aligned rectangular M H V H Z paths are represented exactly.
        match = re.fullmatch(r'\s*M\s*('+_NUM+r')[ ,]+('+_NUM+r')\s*H\s*('+_NUM+r')\s*V\s*('+_NUM+r')\s*H\s*('+_NUM+r')\s*[zZ]\s*', path)
        transform = re.fullmatch(r'\s*matrix\s*\(([^)]+)\)\s*', shape.get('transform', ''))
        if not match or not transform:
            clips[node.get('id')] = None; continue
        x0,y0,x1,y1,x2 = map(float,match.groups())
        values = [float(v) for v in re.findall(_NUM, transform.group(1))]
        if len(values)!=6 or abs(x2-x0)>0.01 or abs(values[1])>1e-8 or abs(values[2])>1e-8:
            clips[node.get('id')] = None; continue
        clips[node.get('id')] = fitz.Rect(min(x0,x1),min(y0,y1),max(x0,x1),max(y0,y1)) * fitz.Matrix(values)
    result = []
    def walk(node, active):
        if tag(node) == 'defs': return
        match = re.fullmatch(r'url\(#([^\)]+)\)',node.get('clip-path',''))
        if match:
            active = active + [clips.get(match.group(1))]
        if tag(node) in ('image', 'use'):
            rect = fitz.Rect(page.rect)
            for clip in active:
                if clip is None:
                    rect = None; break
                rect &= clip
            result.append(rect)
        for child in node: walk(child, active)
    walk(root, [])
    return result
