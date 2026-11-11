"""生成文档工具的测试样本（docx / xlsx / pptx / 大图）。"""

import os
import zipfile
from PIL import Image, ImageDraw

ROOT = os.path.dirname(os.path.abspath(__file__))
FIX = os.path.join(ROOT, "fixtures")
os.makedirs(FIX, exist_ok=True)


def make_big_jpeg(path, size=(3200, 2133)):
    img = Image.new("RGB", size)
    draw = ImageDraw.Draw(img)
    for y in range(0, size[1], 8):
        draw.rectangle([0, y, size[0], y + 8],
                       fill=((y * 3) % 255, (y * 7) % 255, 180))
    for x in range(0, size[0], 40):
        draw.line([(x, 0), (x, size[1])], fill=(40, 60, 90), width=3)
    img.save(path, "JPEG", quality=95)


def make_big_png(path, size=(1500, 1000)):
    # 随机噪声：PNG 压不动，体积大，用来验证降采样与重编码
    raw = os.urandom(size[0] * size[1] * 3)
    img = Image.frombytes("RGB", size, raw)
    img.save(path, "PNG")


CT = """<?xml version="1.0" encoding="UTF-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
<Default Extension="png" ContentType="image/png"/>
<Default Extension="jpg" ContentType="image/jpeg"/>
<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
<Default Extension="xml" ContentType="application/xml"/>
<Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>
</Types>"""

RELS = """<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>
</Relationships>"""


def docx(path, image_path):
    paras = ["这是一份用于测试的 Word 文档。", "第二段：纯 Rust 实现，不依赖本机 Office。",
             "第三段：包含中文字符与数字 12345，验证文本提取是否一致。"]
    body = "".join(
        '<w:p><w:r><w:t>%s</w:t></w:r></w:p>' % p for p in paras
    )
    doc = ('<?xml version="1.0" encoding="UTF-8"?>'
           '<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">'
           '<w:body>%s</w:body></w:document>' % body)
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as z:
        z.writestr("[Content_Types].xml", CT)
        z.writestr("_rels/.rels", RELS)
        z.writestr("word/document.xml", doc)
        z.write(image_path, "word/media/image1.png")


def xlsx(path):
    shared = "".join('<si><t>%s</t></si>' % s for s in
                     ["项目", "金额", "备注", "差旅费", "住宿", "餐饮"])
    cells = (
        '<row r="1"><c r="A1" t="s"><v>0</v></c><c r="B1" t="s"><v>1</v></c>'
        '<c r="C1" t="s"><v>2</v></c></row>'
        '<row r="2"><c r="A2" t="s"><v>3</v></c><c r="B2"><v>1280.5</v></c>'
        '<c r="C2" t="s"><v>4</v></c></row>'
        '<row r="3"><c r="A3" t="s"><v>5</v></c><c r="B3"><v>356</v></c></row>'
    )
    sheet = ('<?xml version="1.0" encoding="UTF-8"?>'
             '<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">'
             '<sheetData>%s</sheetData></worksheet>' % cells)
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as z:
        z.writestr("[Content_Types].xml", CT)
        z.writestr("xl/sharedStrings.xml",
                   '<sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">%s</sst>' % shared)
        z.writestr("xl/worksheets/sheet1.xml", sheet)


def pptx(path, image_path):
    slides = []
    for i, text in enumerate(["封面：文档工具演示", "第二页：压缩与转换"], start=1):
        slides.append(('<?xml version="1.0" encoding="UTF-8"?>'
                       '<p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"'
                       ' xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">'
                       '<a:p><a:r><a:t>%s</a:t></a:r></a:p></p:sld>' % text))
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as z:
        z.writestr("[Content_Types].xml", CT)
        for i, xml in enumerate(slides, start=1):
            z.writestr("ppt/slides/slide%d.xml" % i, xml)
        z.write(image_path, "ppt/media/image1.png")


if __name__ == "__main__":
    big_jpg = os.path.join(FIX, "big.jpg")
    big_png = os.path.join(FIX, "big.png")
    make_big_jpeg(big_jpg)
    make_big_png(big_png)
    docx(os.path.join(FIX, "sample.docx"), big_png)
    xlsx(os.path.join(FIX, "sample.xlsx"))
    pptx(os.path.join(FIX, "sample.pptx"), big_png)
    for name in os.listdir(FIX):
        p = os.path.join(FIX, name)
        print("%-14s %8.2f MB" % (name, os.path.getsize(p) / 1048576))
