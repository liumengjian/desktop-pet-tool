"""Regression checks for independent picture objects in PDF to Word conversion."""
import math
import subprocess
import sys
import tempfile
import zipfile
from pathlib import Path
from xml.etree import ElementTree as ET
import fitz

script = Path(__file__).resolve().parents[2] / 'src-tauri/binaries/pdfconv/scripts/convert.py'
with tempfile.TemporaryDirectory(dir=Path.cwd()) as tmp:
    tmp = Path(tmp)
    pdf_path, docx_path, report = (tmp / 'sample.pdf', tmp / 'sample.docx', tmp / 'report.json')
    pdf = fitz.open()
    page = pdf.new_page(width=300, height=500)
    pix = fitz.Pixmap(fitz.csRGB, 20, 20, bytes([100, 0, 0, 100]) * 400, True)
    xref = page.insert_image(fitz.Rect(10, 20, 60, 70), pixmap=pix)
    page.insert_image(fitz.Rect(80, 20, 130, 70), xref=xref, rotate=90)
    page.insert_image(fitz.Rect(150, 20, 200, 70), xref=xref)
    content = page.get_contents()[-1]
    stream = pdf.xref_stream(content)
    assert b"50 0 0 50 150 430 cm" in stream
    pdf.update_stream(content, stream.replace(b"50 0 0 50 150 430 cm", b"-50 0 0 50 200 430 cm"))
    page.set_rotation(90)
    pdf.save(pdf_path)
    expected = pdf[0].get_text('dict')['blocks']
    expected = [b for b in expected if b['type'] == 1]
    subprocess.run([sys.executable, str(script), '--src', str(pdf_path), '--dst', str(docx_path), '--report', str(report)], check=True)
    with zipfile.ZipFile(docx_path) as archive:
        root = ET.fromstring(archive.read('word/document.xml'))
        ns = {'wp':'http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing', 'a':'http://schemas.openxmlformats.org/drawingml/2006/main'}
        pictures = [a for a in root.findall('.//wp:anchor', ns) if a.find('wp:docPr', ns).get('name').startswith('PDF photo ')]
        assert len(pictures) == len(expected) == 3
        for anchor, block in zip(pictures, expected):
            matrix = fitz.Matrix(block['transform']) * pdf[0].rotation_matrix
            extent = anchor.find('wp:extent', ns)
            assert abs(int(extent.get('cx')) / 12700 - math.hypot(matrix.a, matrix.b)) < .01
            assert abs(int(extent.get('cy')) / 12700 - math.hypot(matrix.c, matrix.d)) < .01
            bbox = fitz.Rect(block['bbox']) * pdf[0].rotation_matrix
            for axis, center, size in [('H', (bbox.x0+bbox.x1)/2, math.hypot(matrix.a,matrix.b)), ('V', (bbox.y0+bbox.y1)/2, math.hypot(matrix.c,matrix.d))]:
                offset = int(anchor.find(f'wp:position{axis}/wp:posOffset', ns).text) / 12700
                assert abs(offset - (center-size/2)) < .01
        assert pictures[-1].find('.//a:xfrm', ns).get('flipV') == '1'
        assert any(fitz.Pixmap(archive.read(name)).alpha for name in archive.namelist() if name.startswith('word/media/'))
    print('independent repeated transparent and mirrored images on rotated page: OK')


# A page-clipped source image must map the exact visible source pixels to the page.
with tempfile.TemporaryDirectory(dir=Path.cwd()) as directory:
    tmp = Path(directory)
    source, output, report = tmp/'clip.pdf', tmp/'clip.docx', tmp/'clip.json'
    pdf = fitz.open(); pg = pdf.new_page(width=100, height=100)
    pixels = bytes(channel for y in range(200) for x in range(100) for channel in (x*2, y, (x+y)%256))
    pix = fitz.Pixmap(fitz.csRGB,100,200,pixels,False)
    pg.insert_image(fitz.Rect(0,-50,100,150),pixmap=pix)
    content = pg.get_contents()[-1]
    pdf.update_stream(content, b'q 20 20 60 50 re W n\n' + pdf.xref_stream(content) + b'\nQ')
    pdf.save(source)
    subprocess.run([sys.executable,str(script),'--src',str(source),'--dst',str(output),'--report',str(report)],check=True)
    with zipfile.ZipFile(output) as archive:
        root=ET.fromstring(archive.read('word/document.xml'))
        ns={'wp':'http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing','a':'http://schemas.openxmlformats.org/drawingml/2006/main','r':'http://schemas.openxmlformats.org/officeDocument/2006/relationships','rel':'http://schemas.openxmlformats.org/package/2006/relationships'}
        anchor=next(a for a in root.findall('.//wp:anchor',ns) if a.find('wp:docPr',ns).get('name').startswith('PDF photo'))
        crop=anchor.find('.//a:srcRect',ns)
        assert crop is not None
        values=[int(crop.get(k,'0'))/100000 for k in ('l','t','r','b')]
        extent=anchor.find('wp:extent',ns)
        x=int(anchor.find('wp:positionH/wp:posOffset',ns).text)/12700; y=int(anchor.find('wp:positionV/wp:posOffset',ns).text)/12700
        w=int(extent.get('cx'))/12700; h=int(extent.get('cy'))/12700
        assert abs(x-20)<.1 and abs(y-30)<.1 and abs(w-60)<.1 and abs(h-50)<.1
        l,t,r,b=values; fullw=w/(1-l-r); fullh=h/(1-t-b)
        image_id=anchor.find('.//a:blip',ns).get('{%s}embed'%ns['r'])
        rels=ET.fromstring(archive.read('word/_rels/document.xml.rels'))
        target=next(n.get('Target') for n in rels if n.get('Id')==image_id)
        image_data=archive.read('word/'+target)
        reconstructed=fitz.open(); target_page=reconstructed.new_page(width=100,height=100)
        target_page.insert_image(fitz.Rect(x-l*fullw,y-t*fullh,x+(1-l)*fullw,y+(1-t)*fullh),stream=image_data)
        content=target_page.get_contents()[-1]
        reconstructed.update_stream(content, b'q 20 20 60 50 re W n\n' + reconstructed.xref_stream(content) + b'\nQ')
        expected=pdf[0].get_pixmap(alpha=False).samples
        actual=target_page.get_pixmap(alpha=False).samples
        assert expected==actual, 'cropped photo pixels do not match source PDF'
    print('internal-clipped image visible pixels: exact match')
