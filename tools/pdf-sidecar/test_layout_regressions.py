"""Regression: PDF drawing order and unclipped editable titles across object types."""
import subprocess
import sys
import tempfile
import zipfile
from pathlib import Path
from lxml import etree
import fitz

script = Path(__file__).resolve().parents[2] / 'src-tauri/binaries/pdfconv/scripts/convert.py'
ns = {
    'w': 'http://schemas.openxmlformats.org/wordprocessingml/2006/main',
    'wp': 'http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing',
    'wps': 'http://schemas.microsoft.com/office/word/2010/wordprocessingShape',
}
with tempfile.TemporaryDirectory(dir=Path.cwd()) as tmp:
    tmp = Path(tmp)
    pdf = fitz.open()
    page = pdf.new_page(width=400, height=500)
    page.draw_rect((20, 140, 350, 400), fill=(1, .9, .7))
    pix = fitz.Pixmap(fitz.csRGB, 10, 10, bytes([10, 80, 120]) * 100, False)
    page.insert_image((40, 160, 300, 380), pixmap=pix)
    # Deliberate foreground decoration must remain above the image as in PDF.
    page.draw_rect((50, 170, 100, 190), fill=(1, 0, 0))
    page.insert_text((25, 80), 'Chongqing Insights', fontsize=32)
    pdf.save(tmp / 'source.pdf')
    subprocess.run([sys.executable, str(script), '--src', str(tmp/'source.pdf'),
                    '--dst', str(tmp/'result.docx'), '--report', str(tmp/'report.json')], check=True)
    with zipfile.ZipFile(tmp/'result.docx') as archive:
        root = etree.fromstring(archive.read('word/document.xml'))
        anchors = root.xpath('.//wp:anchor', namespaces=ns)
        pictures = [a for a in anchors if a.find('wp:docPr', ns).get('name').startswith('PDF photo ')]
        vectors = [a for a in anchors if a.find('wp:docPr', ns).get('name').startswith('PDF vector ')]
        assert len(pictures) == 1 and len(vectors) == 2
        assert all(a.get('behindDoc') == '1' for a in pictures + vectors)
        rank = lambda a: int(a.get('relativeHeight'))
        assert rank(vectors[0]) < rank(pictures[0]) < rank(vectors[1]), 'PDF paint order changed'
        body = root.find('.//wps:bodyPr[@wrap="none"]', ns)
        assert body is not None and body.get('horzOverflow') == body.get('vertOverflow') == 'overflow'
        assert 'Chongqing Insights' in root.xpath('.//w:t/text()', namespaces=ns)
        assert not root.xpath('.//w:pict', namespaces=ns), 'Mixed VML/DrawingML layering returned'
    pdf.close()
print('PDF mixed-object paint order and non-clipping title frames: OK')
