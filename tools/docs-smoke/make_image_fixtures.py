"""生成含内嵌图片的 docx / pptx 夹具，用于验证「图片进 PDF」这条链路。

docx 用 python-docx 直接插图；pptx 没有 python-pptx，直接对 sample.pptx 做 zip 手术：
加 ppt/media/testimg.png、在 slide1 的 rels 里挂一条图片关系、在 spTree 末尾插一个 <p:pic>。
"""

import io
import os
import shutil
import zipfile

HERE = os.path.dirname(os.path.abspath(__file__))
FIX = os.path.join(HERE, "fixtures")


def make_docx():
    from docx import Document
    from docx.shared import Pt

    doc = Document()
    doc.add_heading("图文混排测试", level=1)
    doc.add_paragraph("下面插入一张图片，检查它是否进入 PDF 版面：")
    doc.add_picture(os.path.join(FIX, "big.png"), width=Pt(300))
    doc.add_paragraph("图片之后的一段中文正文，用于确认文字与图片的相对顺序。")
    doc.add_paragraph("再补一段文字，让内容超过一页时图片能随页流转。")
    out = os.path.join(FIX, "sample-image.docx")
    doc.save(out)
    print("wrote", out, os.path.getsize(out))


PIC_XML = (
    '<p:pic><p:nvPicPr><p:cNvPr id="12" name="Picture 12"/><p:cNvPicPr/><p:nvPr/></p:nvPicPr>'
    '<p:blipFill><a:blip r:embed="rId2"/><p:stretch><a:fillRect/></p:stretch></p:blipFill>'
    '<p:spPr><a:xfrm><a:off x="914400" y="914400"/><a:ext cx="3048000" cy="2032000"/></a:xfrm>'
    '<a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr></p:pic>'
)

SLIDE_XML = (
    '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
    '<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" '
    'xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" '
    'xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">'
    "<p:cSld><p:spTree>"
    '<p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr>'
    "<p:grpSpPr/>"
    '<p:sp><p:nvSpPr><p:cNvPr id="3" name="Title 3"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr>'
    '<p:spPr/><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>图文幻灯片测试</a:t></a:r></a:p></p:txBody></p:sp>'
    '<p:sp><p:nvSpPr><p:cNvPr id="4" name="Body 4"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr>'
    '<p:spPr/><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>这一页带一张内嵌图片。</a:t></a:r></a:p></p:txBody></p:sp>'
    + PIC_XML
    + "</p:spTree></p:cSld></p:sld>"
)

REL_XML = (
    '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
    '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
    '<Relationship Id="rId2" '
    'Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" '
    'Target="../media/pic.png"/>'
    "</Relationships>"
)

CONTENT_TYPES = (
    '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
    '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
    '<Default Extension="xml" ContentType="application/xml"/>'
    '<Default Extension="png" ContentType="image/png"/>'
    '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>'
    '<Override PartName="/ppt/slides/slide1.xml" '
    'ContentType="application/vnd.openxmlformats-officedocument.presentationml.slide+xml"/>'
    "</Types>"
)


def make_pptx():
    out = os.path.join(FIX, "sample-image.pptx")
    png_bytes = open(os.path.join(FIX, "big.png"), "rb").read()
    with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as zout:
        zout.writestr("[Content_Types].xml", CONTENT_TYPES)
        zout.writestr("ppt/slides/slide1.xml", SLIDE_XML)
        zout.writestr("ppt/slides/_rels/slide1.xml.rels", REL_XML)
        zout.writestr("ppt/media/pic.png", png_bytes)
    print("wrote", out, os.path.getsize(out))


if __name__ == "__main__":
    make_docx()
    make_pptx()
