import fitz  # PyMuPDF

font_path = r"E:\project\desktop-pet-tool\src-tauri\assets\NotoSansSC.ttf"
out = r"E:\project\desktop-pet-tool\tools\docs-smoke\fixtures\real-cjk.pdf"

doc = fitz.open()
page = doc.new_page()
page.insert_font(fontname="noto", fontfile=font_path)
page.insert_text((60, 90), "真实第三方 PDF 取字测试", fontname="noto", fontsize=22)
page.insert_text((60, 130), "这一行用来确认多行文本与换行是否正常。", fontname="noto", fontsize=12)
page.insert_text((60, 155), "Mixed English words 和数字 12345 都要能抽出来。", fontname="noto", fontsize=12)

page2 = doc.new_page()
page2.insert_font(fontname="noto", fontfile=font_path)
page2.insert_text((60, 90), "第二页标题", fontname="noto", fontsize=20)
page2.insert_text((60, 130), "第二页正文内容，用于验证分页。", fontname="noto", fontsize=12)

doc.save(out)
print("wrote", out)
