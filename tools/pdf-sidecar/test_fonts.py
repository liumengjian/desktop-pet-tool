"""Regression test: python tools/pdf-sidecar/test_fonts.py"""
import io
import sys
import uuid
import zipfile
import fitz
from pathlib import Path

from docx import Document
from fontTools.fontBuilder import FontBuilder
from fontTools.pens.ttGlyphPen import TTGlyphPen
from fontTools.ttLib import TTFont

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / 'src-tauri/binaries/pdfconv/scripts'))
from embed_fonts import embed_pdf_fonts, font_aliases
import embed_fonts


def fixture():
    fb = FontBuilder(1000, isTTF=True)
    names = ['.notdef', 'r', 'ligature']
    fb.setupGlyphOrder(names)
    fb.setupCharacterMap({114: 'r'})
    glyphs = {}
    for name in names:
        pen = TTGlyphPen(None)
        if name != '.notdef':
            pen.moveTo((0, 0)); pen.lineTo((400, 0)); pen.lineTo((400, 700)); pen.lineTo((0, 700)); pen.closePath()
        glyphs[name] = pen.glyph()
    fb.setupGlyf(glyphs)
    fb.setupHorizontalMetrics({name: (500, 0) for name in names})
    fb.setupHorizontalHeader(ascent=800, descent=-200)
    fb.setupNameTable({'familyName': 'ABCDEF+TestFont', 'styleName': 'Regular'})
    fb.setupOS2(fsType=0)
    fb.setupPost()
    fb.setupMaxp()
    # The user's PDF subsets omit all these tables, not just the Unicode cmap.
    for table in ('cmap', 'name', 'OS/2', 'post'):
        del fb.font[table]
    buf = io.BytesIO()
    fb.save(buf)
    return buf.getvalue()


class Page:
    def get_fonts(self, full=True):
        return [(1, 'ttf', 'Type0', 'ABCDEF+TestFont', 'F1', 'Identity-H')]

    def get_texttrace(self):
        # The second r begins a ligature; its continuation has gid -1.
        return [{'font': 'TestFont', 'chars': [(114, 1), (114, 2), (105, -1)]}]


FONT_BYTES = fixture()

class Pdf:
    def __iter__(self):
        return iter([Page()])

    def extract_font(self, xref):
        return ('ABCDEF+TestFont', 'ttf', 'TrueType', FONT_BYTES)


doc = Document()
warnings = embed_pdf_fonts(doc, Pdf())
buf = io.BytesIO()
doc.save(buf)
with zipfile.ZipFile(buf) as archive:
    assert b'embedTrueTypeFonts' in archive.read('word/settings.xml')
    path = next(name for name in archive.namelist() if name.endswith('.odttf'))
    data = bytearray(archive.read(path))
    key = uuid.UUID(Path(path).stem).bytes[::-1]
    for index in range(32):
        data[index] ^= key[index % 16]
font = TTFont(io.BytesIO(data))
alias = font_aliases(Pdf())['TestFont']
assert alias == font_aliases(Pdf())['TestFont']
version = embed_fonts.FONT_FORMAT_VERSION
embed_fonts.FONT_FORMAT_VERSION = b'previous-format'
assert alias != font_aliases(Pdf())['TestFont']
embed_fonts.FONT_FORMAT_VERSION = version
assert font['name'].getDebugName(1) == alias
assert font['name'].getDebugName(3) == alias
assert font['name'].getDebugName(6) == alias
assert font.getBestCmap()[114] == font.getGlyphName(1)
for platform, encoding in ((0, 3), (3, 1)):
    bmp = next(t for t in font['cmap'].tables if t.platformID == platform and t.platEncID == encoding and t.format == 4)
    assert bmp.cmap == {c: g for c, g in font.getBestCmap().items() if c < 0xffff}
assert font['head'].macStyle & 3 == 0
assert font['OS/2'].fsSelection & 0x61 == 0x40
assert all(table in font for table in ('cmap', 'name', 'OS/2', 'post'))
engine_font = fitz.Font(fontbuffer=bytes(data))
assert engine_font.has_glyph(114) != 0
assert len(font['glyf'][font.getBestCmap()[114]].coordinates) == 4
print('Font required tables, engine loading, names and ligature cmap regression passed')

