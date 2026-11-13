"""Prefer installed Windows font families over reconstructed PDF subsets."""
import os
from pathlib import Path

FONT_DIR = Path(os.environ.get('WINDIR', 'C:/Windows')) / 'Fonts'

# (归一化后的名前缀, Windows 字体族名, 用来证明该族已安装的字体文件之一)
FAMILIES = (
    ('microsoftyahei', 'Microsoft YaHei', ('msyh.ttc', 'msyh.ttf')),
    ('simsun', 'SimSun', ('simsun.ttc', 'simsun.ttf')),
    ('nsimsun', 'NSimSun', ('simsun.ttc', 'simsun.ttf')),
    ('simhei', 'SimHei', ('simhei.ttf',)),
    ('simkai', 'KaiTi', ('simkai.ttf',)),
    ('kaiti', 'KaiTi', ('simkai.ttf',)),
    ('simfang', 'FangSong', ('simfang.ttf',)),
    ('fangsong', 'FangSong', ('simfang.ttf',)),
    ('pmingliu', 'PMingLiU', ('pmingliu.ttc', 'mingliu.ttc')),
    ('mingliu', 'MingLiU', ('mingliu.ttc',)),
    ('arial', 'Arial', ('arial.ttf',)),
    ('helvetica', 'Arial', ('arial.ttf',)),
    ('calibri', 'Calibri', ('calibri.ttf',)),
    ('timesnewroman', 'Times New Roman', ('times.ttf',)),
    ('times', 'Times New Roman', ('times.ttf',)),
    ('couriernew', 'Courier New', ('cour.ttf',)),
    ('courier', 'Courier New', ('cour.ttf',)),
    ('wingdings', 'Wingdings', ('wingding.ttf',)),
    ('symbol', 'Symbol', ('symbol.ttf',)),
    ('tahoma', 'Tahoma', ('tahoma.ttf',)),
    ('verdana', 'Verdana', ('verdana.ttf',)),
    ('georgia', 'Georgia', ('georgia.ttf',)),
    ('cambria', 'Cambria', ('cambria.ttc', 'cambria.ttf')),
    ('segoeui', 'Segoe UI', ('segoeui.ttf',)),
)


def _basename(raw):
    """PDF 族名 → 小写基名：去掉子集前缀、逗号后的样式、风格后缀。"""
    name = raw.split('+')[-1].split(',')[0].strip().lower()
    return name.split('-')[0].strip()


def installed_family(raw):
    """PDF 里的族名映射到本机已装的 Windows 字体族，未安装返回 None。"""
    base = _basename(raw)
    for prefix, family, files in FAMILIES:
        if base.startswith(prefix) and any((FONT_DIR / f).is_file() for f in files):
            return family
    return None


def name_style(raw):
    """从字体名推断粗体 / 斜体（PDF 子集名常把字重写在名字里，flags 未必可靠）。"""
    low = raw.split('+')[-1].lower()
    bold = any(key in low for key in ('bold', 'black', 'heavy', 'semibold', 'demibold'))
    italic = any(key in low for key in ('italic', 'oblique'))
    return bold, italic
