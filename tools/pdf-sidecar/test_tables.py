"""Check that regular PDF grids produce editable floating Word tables."""
import sys
from pathlib import Path
from xml.etree import ElementTree as ET
import fitz
from docx import Document
sys.path.insert(0, str(Path(__file__).resolve().parents[2] / 'src-tauri/binaries/pdfconv/scripts'))
from native_tables import add_page_tables
pdf = fitz.open(); page = pdf.new_page(width=300, height=400)
page.draw_rect(fitz.Rect(20,20,100,50), color=None, fill=(.4,.8,.4))
for x in (20,100,180): page.draw_line((x,20),(x,80), color=(0,0,0))
for y in (20,50,80): page.draw_line((20,y),(180,y), color=(0,0,0))
page.insert_text((30,40), 'First', fontsize=10)
page.insert_text((110,70), 'Second', fontsize=10)
spans = [s for b in page.get_text('dict')['blocks'] for line in b.get('lines',[]) for s in line['spans']]
doc = Document()
# Supply the known grid to isolate Word reconstruction from detector heuristics.
class Row:
    def __init__(self, y): self.cells = [(20,y,100,y+30),(100,y,180,y+30)]
class Candidate:
    row_count=2; col_count=2; bbox=(20,20,180,80); rows=[Row(20),Row(50)]
class PageProxy:
    def __getattr__(self, name): return getattr(page, name)
    def find_tables(self): return type('Result', (), {'tables':[Candidate()]})()
count, consumed, boxes, warnings = add_page_tables(doc,PageProxy(),spans,lambda name, text: 'Resolved Font')
assert count == 1 and len(consumed) == 2 and len(boxes) == 1 and not warnings
root = ET.fromstring(doc._element.xml)
ns={'w':'http://schemas.openxmlformats.org/wordprocessingml/2006/main','a':'http://schemas.openxmlformats.org/drawingml/2006/main'}
assert len(root.findall('.//w:tbl',ns)) == 1
assert len(root.findall('.//w:tc',ns)) == 4
assert root.find('.//w:tblpPr',ns) is not None
assert [x.text for x in root.findall('.//w:tbl//w:t',ns)] == ['First','Second']
assert root.find('.//w:shd',ns).get('{%s}fill' % ns['w']) == '66CC66'
assert root.find('.//w:tbl//w:rFonts',ns).get('{%s}ascii' % ns['w']) == 'Resolved Font'
assert not root.findall('.//w:tbl//a:blip',ns)
print('editable floating table, cells, fill and fonts: OK')
