"""Restore usable PDF font programs and embed them in a python-docx document."""
import io
import hashlib
import re
import uuid

FONT_FORMAT_VERSION = b"bmp-cmap-regular-v2"


def font_aliases(pdf):
    """Map PDF family names to distinct document-local embedded family names."""
    aliases = {}
    for page in pdf:
        for info in page.get_fonts(full=True):
            xref = info[0]
            name = re.sub(r'^[A-Z]{6}\+', '', info[3])
            if xref and name not in aliases:
                try:
                    data = pdf.extract_font(xref)[3]
                    aliases[name] = 'PDFEmbedded' + hashlib.sha256(FONT_FORMAT_VERSION + b'\0' + name.encode('utf-8') + data).hexdigest()[:20]
                except Exception:
                    pass
    return aliases


def embed_pdf_fonts(document, pdf):
    """Embed supported font programs; return human-readable warnings."""
    from fontTools.ttLib import TTFont
    from docx.opc.packuri import PackURI
    from docx.opc.part import Part
    from docx.oxml import OxmlElement, parse_xml
    from docx.oxml.ns import qn

    warnings = []
    aliases = font_aliases(pdf)
    seen_xrefs = set()
    embedded = set()
    font_table = document.part.part_related_by('http://schemas.openxmlformats.org/officeDocument/2006/relationships/fontTable')
    root = parse_xml(font_table.blob)
    xrefs_by_name = {}
    trace_maps = {}
    conflicts = set()
    for page in pdf:
        for info in page.get_fonts(full=True):
            name = re.sub(r'^[A-Z]{6}\+', '', info[3])
            xrefs_by_name.setdefault(name, set()).add(info[0])
        for span in page.get_texttrace():
            name = re.sub(r'^[A-Z]{6}\+', '', span['font'])
            mapping = trace_maps.setdefault(name, {})
            chars = span['chars']
            for index, char in enumerate(chars):
                code, gid = char[:2]
                # A following -1 denotes continuation of a ligature, whose
                # glyph cannot be mapped to its first Unicode character.
                if gid < 0 or (index + 1 < len(chars) and chars[index + 1][1] < 0):
                    continue
                if code in mapping and mapping[code] != gid:
                    conflicts.add(name)
                mapping[code] = gid
    for page in pdf:
        for info in page.get_fonts(full=True):
            xref, _, _, base = info[:4]
            if not xref or xref in seen_xrefs:
                continue
            seen_xrefs.add(xref)
            name = re.sub(r'^[A-Z]{6}\+', '', base)
            subset = name != base
            if name in embedded:
                if subset:
                    warnings.append(f'字体 {name} 存在多个子集，嵌入字体可能缺字。')
                continue
            try:
                _, ext, _, data = pdf.extract_font(xref)
                if ext.lower() not in ('ttf', 'otf') or not data:
                    raise ValueError('PDF 未包含可嵌入的 TrueType/OpenType 字体')
                font = TTFont(io.BytesIO(data), lazy=True)
                flags = font['OS/2'].fsType if 'OS/2' in font else 0
                cmap = font.getBestCmap()
                if not cmap:
                    refs = xrefs_by_name.get(name, set())
                    programs = {pdf.extract_font(ref)[3] for ref in refs if ref}
                    mapping = trace_maps.get(name, {})
                    if len(programs) != 1 or name in conflicts or not mapping:
                        raise ValueError('font has no Unicode cmap and glyph mapping is ambiguous')
                    from fontTools.ttLib.tables._c_m_a_p import CmapSubtable
                    glyphs = font.getGlyphOrder()
                    if any(gid < 0 or gid >= len(glyphs) for gid in mapping.values()):
                        raise ValueError('invalid PDF glyph mapping')
                    table = CmapSubtable.newSubtable(12)
                    table.platformID, table.platEncID, table.language = 3, 10, 0
                    table.cmap = {code: glyphs[gid] for code, gid in mapping.items() if 0 <= code <= 0x10ffff}
                    if 'cmap' not in font:
                        from fontTools.ttLib import newTable
                        font['cmap'] = newTable('cmap')
                        font['cmap'].tableVersion = 0
                        font['cmap'].tables = []
                    font['cmap'].tables.append(table)
                    bmp = {code: glyph for code, glyph in table.cmap.items() if code < 0xffff}
                    # Older Word-compatible readers may only consult BMP cmap format 4.
                    for platform, encoding in ((0, 3), (3, 1)):
                        subtable = CmapSubtable.newSubtable(4)
                        subtable.platformID, subtable.platEncID, subtable.language = platform, encoding, 0
                        subtable.cmap = bmp.copy()
                        font['cmap'].tables.append(subtable)
                    out = io.BytesIO()
                    font.save(out)
                    data = out.getvalue()
                    warnings.append(f'字体 {name} 已恢复字符映射；新增原文未使用的字符时可能缺字。')
                from fontTools.fontBuilder import FontBuilder
                builder = FontBuilder(font=font)
                if 'OS/2' not in font:
                    builder.setupOS2(fsType=0, sTypoAscender=font['hhea'].ascent,
                                     sTypoDescender=font['hhea'].descent,
                                     usWinAscent=max(0, font['head'].yMax),
                                     usWinDescent=max(0, -font['head'].yMin))
                if 'post' not in font:
                    builder.setupPost()
                if 'name' not in font:
                    builder.setupNameTable({'familyName': name, 'styleName': 'Regular'})
                # Each PDF face is its own family and is stored in the regular slot.
                # Keep outlines and weight class, while avoiding a style mismatch.
                font['head'].macStyle &= ~3
                font['OS/2'].fsSelection = (font['OS/2'].fsSelection & ~0x21) | 0x40
                family = aliases.get(name, name)
                # Word matches embedded fonts by their internal family names.
                # PDF subset prefixes must not leak into those names.
                if 'name' in font:
                    for record in font['name'].names:
                        value = {1: family, 2: 'Regular', 3: family, 4: family, 6: family, 16: family, 17: 'Regular'}.get(record.nameID)
                        if value is not None:
                            try:
                                record.string = value.encode(record.getEncoding())
                            except (UnicodeEncodeError, LookupError):
                                record.string = value.encode('utf-16-be')
                    for name_id, value in {1: family, 2: 'Regular', 3: family, 4: family, 6: family, 16: family, 17: 'Regular'}.items():
                        font['name'].setName(value, name_id, 3, 1, 0x409)
                    out = io.BytesIO()
                    font.save(out)
                    data = out.getvalue()
                font.close()
                if flags & 0x0004:
                    raise ValueError('font permits preview and print only, not editing')
                if flags & 0x0002 or flags & 0x0200:
                    raise ValueError('字体许可限制嵌入或仅允许位图嵌入')
                key = uuid.uuid4()
                mask = bytes.fromhex(key.hex)[::-1]
                blob = bytearray(data)
                for i in range(min(32, len(blob))):
                    blob[i] ^= mask[i % 16]
                partname = PackURI('/word/fonts/' + str(key).upper() + '.odttf')
                part = Part(partname, 'application/vnd.openxmlformats-officedocument.obfuscatedFont', bytes(blob), document.part.package)
                rid = font_table.relate_to(part, 'http://schemas.openxmlformats.org/officeDocument/2006/relationships/font')
                entries = [e for e in root.findall(qn('w:font')) if e.get(qn('w:name')) == family]
                if entries:
                    entry = entries[0]
                else:
                    entry = OxmlElement('w:font')
                    entry.set(qn('w:name'), family)
                    root.append(entry)
                embed = OxmlElement('w:embedRegular')
                embed.set(qn('r:id'), rid)
                embed.set(qn('w:fontKey'), '{' + str(key).upper() + '}')
                if subset:
                    embed.set(qn('w:subsetted'), 'true')
                    warnings.append(f'字体 {name} 为 PDF 子集，编辑时可能缺字。')
                entry.append(embed)
                embedded.add(name)
            except Exception as exc:
                warnings.append(f'无法嵌入字体 {name}: {exc}')
    if embedded:
        settings = document.settings.element
        if settings.find(qn('w:embedTrueTypeFonts')) is None:
            settings.insert(0, OxmlElement('w:embedTrueTypeFonts'))
    from lxml import etree
    font_table._blob = etree.tostring(root, encoding='UTF-8', xml_declaration=True, standalone=True)
    # A later PDF resource can supply a usable program for the same family.
    return [warning for warning in warnings if not any(
        warning.startswith(f'无法嵌入字体 {name}:') for name in embedded
    )]


