"""生成 RTF / ODT / EPUB 夹具（纯手写，不依赖 Office）。"""
import os, zipfile, textwrap

HERE = os.path.dirname(os.path.abspath(__file__))
FIX = os.path.join(HERE, "fixtures")
os.makedirs(FIX, exist_ok=True)

# ---------- RTF ----------
def rtf_escape(text):
    out = []
    for ch in text:
        code = ord(ch)
        if code < 128:
            out.append(ch)
        else:
            out.append("\\u%d?" % code)
    return "".join(out)

rtf = (
    "{\\rtf1\\ansi\\ansicpg936\\deff0{\\fonttbl{\\f0\\fnil\\fcharset134 SimSun;}}"
    "\\viewkind4\\uc1\\pard\\f0\\fs24 "
    + rtf_escape("RTF 标题行") + "\\par\n"
    "\\pard " + rtf_escape("第一段中文内容，用来验证 RTF 取字。") + "\\par\n"
    "\\pard " + rtf_escape("Mixed English and 数字 12345 都要保留。") + "\\par\n"
    "}"
)
with open(os.path.join(FIX, "sample.rtf"), "w", encoding="ascii") as f:
    f.write(rtf)

# ---------- ODT ----------
content_odt = """<?xml version="1.0" encoding="UTF-8"?>
<office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0"
 xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0"
 xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0">
<office:body><office:text>
<text:h text:outline-level="1">ODT 一级标题</text:h>
<text:p>这是 ODT 的正文第一段。</text:p>
<text:p>第二段包含 Mixed English。</text:p>
<text:list><text:list-item><text:p>列表项一</text:p></text:list-item></text:list>
</office:text></office:body></office:document-content>
"""

# ---------- ODS ----------
content_ods = """<?xml version="1.0" encoding="UTF-8"?>
<office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0"
 xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0"
 xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0">
<office:body><office:spreadsheet><table:table table:name="Sheet1">
<table:table-row><table:table-cell><text:p>城市</text:p></table:table-cell><table:table-cell><text:p>人数</text:p></table:table-cell></table:table-row>
<table:table-row><table:table-cell><text:p>重庆</text:p></table:table-cell><table:table-cell><text:p>32</text:p></table:table-cell></table:table-row>
<table:table-row><table:table-cell><text:p>成都</text:p></table:table-cell><table:table-cell><text:p>18</text:p></table:table-cell></table:table-row>
</table:table></office:spreadsheet></office:body></office:document-content>
"""

def write_odf(name, content):
    path = os.path.join(FIX, name)
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as z:
        z.writestr("mimetype", "application/vnd.oasis.opendocument.text", zipfile.ZIP_STORED)
        z.writestr("content.xml", content)
    return path

write_odf("sample.odt", content_odt)
write_odf("sample.ods", content_ods)

# ---------- EPUB ----------
chapter = """<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml"><head><title>c1</title><style>p{}</style></head>
<body>
<h1>EPUB 第一章</h1>
<p>这是电子书正文的第一段。</p>
<p>第二段带 <em>行内标签</em> 与 &amp; 实体。</p>
<ul><li>要点一</li><li>要点二</li></ul>
</body></html>
"""
container = """<?xml version="1.0" encoding="UTF-8"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
<rootfiles><rootfile full-path="OEBPS/chapter1.xhtml" media-type="application/xhtml+xml"/></rootfiles>
</container>
"""
opf = """<?xml version="1.0" encoding="UTF-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="2.0"><metadata/><manifest/><spine/></package>
"""
epub_path = os.path.join(FIX, "sample.epub")
with zipfile.ZipFile(epub_path, "w", zipfile.ZIP_DEFLATED) as z:
    z.writestr("mimetype", "application/epub+zip", zipfile.ZIP_STORED)
    z.writestr("META-INF/container.xml", container)
    z.writestr("OEBPS/content.opf", opf)
    z.writestr("OEBPS/chapter1.xhtml", chapter)

for p in ("sample.rtf", "sample.odt", "sample.ods", "sample.epub"):
    full = os.path.join(FIX, p)
    print(p, os.path.getsize(full))
