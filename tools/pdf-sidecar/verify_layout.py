"""Check editable text, individual picture objects and page sizes against a PDF.

Run with the sidecar Python: verify_layout.py source.pdf converted.docx
These structural checks do not replace visual verification in Word.
"""
import argparse
import zipfile
from collections import Counter

import fitz
from lxml import etree

NS = {
    'w': 'http://schemas.openxmlformats.org/wordprocessingml/2006/main',
    'wp': 'http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing',
}


def verify(source, output):
    with fitz.open(source) as pdf, zipfile.ZipFile(output) as archive:
        assert archive.testzip() is None, 'Invalid DOCX archive'
        root = etree.fromstring(archive.read('word/document.xml'))
        sizes = root.xpath('.//w:sectPr/w:pgSz', namespaces=NS)
        assert len(sizes) == len(pdf), 'Page section count changed'
        # Every original page has its own content paragraph, including scan-only pages.
        paragraphs = root.xpath('./w:body/w:p[w:r/w:drawing or w:r/w:pict]', namespaces=NS)
        assert len(paragraphs) == len(pdf), 'Page content paragraphs changed'
        assert not root.xpath('.//wp:docPr[starts-with(@name,"PDF graphics ")]', namespaces=NS), 'Page vectors were flattened into a bitmap'
        content_by_page = []
        for child in root.find('w:body', NS):
            if child in paragraphs:
                content_by_page.append([child])
            elif child.tag == '{' + NS['w'] + '}tbl':
                assert content_by_page, 'Table lacks a page anchor'
                content_by_page[-1].append(child)
        ids = root.xpath('.//wp:docPr/@id', namespaces=NS)
        assert len(set(ids)) == len(ids), 'Picture object IDs are not unique'
        characters = photos = 0
        for page, paragraph, size, content in zip(pdf, paragraphs, sizes, content_by_page):
            label = f'Page {page.number + 1}'
            assert abs(int(size.get('{' + NS['w'] + '}w')) / 20 - page.rect.width) < .06, label + ': width changed'
            assert abs(int(size.get('{' + NS['w'] + '}h')) / 20 - page.rect.height) < .06, label + ': height changed'
            blocks = page.get_text('dict')['blocks']
            expected = [s['text'] for b in blocks for line in b.get('lines', []) for s in line.get('spans', []) if s['text']]
            actual = [t for element in content for t in element.xpath('.//w:t/text()', namespaces=NS)]
            assert Counter(actual) == Counter(expected), label + ': editable text changed or duplicated'
            for box in paragraph.xpath('.//w:txbxContent', namespaces=NS):
                assert box.xpath('.//w:fitText', namespaces=NS), label + ': text has no original width constraint'
            pictures = paragraph.xpath('.//wp:anchor[wp:docPr[starts-with(@name,"PDF photo ")]]', namespaces=NS)
            visible_page = page.rect * page.derotation_matrix
            image_blocks = page.get_text('dict', clip=fitz.INFINITE_RECT())['blocks']
            expected_photos = sum(b.get('type') == 1 and fitz.Rect(b['bbox']).intersects(visible_page) for b in image_blocks)
            assert len(pictures) == expected_photos, f'{label}: {len(pictures)} independent photos, expected {expected_photos}'
            for picture in pictures:
                assert picture.find('wp:positionH', NS) is not None, label + ': picture lacks horizontal position'
                assert picture.find('wp:positionV', NS) is not None, label + ': picture lacks vertical position'
            characters += sum(map(len, actual))
            photos += len(pictures)
        print(f'PASS: {len(pdf)} page sections, {characters} editable characters, {photos} independent photos')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('source')
    parser.add_argument('output')
    args = parser.parse_args()
    verify(args.source, args.output)
