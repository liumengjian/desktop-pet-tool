"""Check editable vector objects emitted for lines, rectangles and curves."""
import sys
from pathlib import Path
from xml.etree import ElementTree as ET
import fitz
from docx import Document
sys.path.insert(0, str(Path(__file__).resolve().parents[2] / 'src-tauri/binaries/pdfconv/scripts'))
from vector_shapes import add_page_vectors

pdf = fitz.open()
page = pdf.new_page(width=300, height=500)
page.draw_line((20, 30), (200, 30), color=(0, .5, 0), width=2)
page.draw_rect(fitz.Rect(20, 50, 100, 90), fill=(.1, .8, .1), fill_opacity=.5)
shape = page.new_shape()
shape.draw_bezier((30, 100), (40, 110), (70, 120), (90, 100))
shape.finish(color=(1, 0, 0), width=1)
shape.commit()
page.set_rotation(90)
doc = Document()
paragraph = doc.add_paragraph()
count, warnings = add_page_vectors(paragraph, page)
assert count == 3 and not warnings
root = ET.fromstring(paragraph._p.xml)
ns = {'wp':'http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing', 'a':'http://schemas.openxmlformats.org/drawingml/2006/main'}
shapes = root.findall('.//wp:anchor', ns)
assert len(shapes) == 3
assert all(s.get('behindDoc') == '1' for s in shapes)
assert [int(s.get('relativeHeight')) for s in shapes] == [1 + d['seqno'] for d in page.get_drawings()]
assert all(s.find('.//a:path',ns) is not None for s in shapes)
assert any(s.find('.//a:cubicBezTo',ns) is not None for s in shapes)
assert any(s.find('.//a:alpha[@val="50000"]',ns) is not None for s in shapes)
assert not root.findall('.//a:blip', ns)
print('independent rotated line, rectangle and curve shapes: OK')

