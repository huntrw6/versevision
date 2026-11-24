"""Document inputs for the oracle's S4 (document readers) cases: HTML, Word, plain text and files that cannot be read.

Importable with ``python/scripts`` on ``sys.path`` (standard library + python-docx only, nothing here touches the app).
Every function returns ``list[tuple[name, extension, bytes]]``; the bytes are deterministic (fixed zip timestamps and Word
properties, like ``build_docx`` in ``export_oracle.py``). The names are lowercase and hyphenated.

* ``html_cases()``  : sermon notes as HTML, about half of it malformed on purpose (what ``BeautifulSoup(..., "html.parser")`` makes of it).
* ``docx_cases()``  : Word files with runs, tabs, breaks, hyperlinks, tables, headers, tracked changes, content controls ...
* ``text_cases()``  : .txt/.md bytes (line endings, BOM, odd Unicode); the later classifier relies on exactly this text.
* ``error_cases()`` : inputs on which extraction raises (bad UTF-8, not a zip, a zip without the document part ...).
"""
from __future__ import annotations

import datetime as dt
import io
import zipfile

NOTES = "Sermon: The Good Shepherd\nJohn 10:11\nPsalm 23:1-4\n"


def _b(text: str) -> bytes:
    return text.encode("utf-8")


# ---------------------------------------------------------------------------------------------------------------------
# HTML
# ---------------------------------------------------------------------------------------------------------------------

def html_cases() -> list[tuple[str, str, bytes]]:
    c: list[tuple[str, str, str]] = []
    add = lambda name, ext, text: c.append((name, ext, text))

    # -- well formed -------------------------------------------------------------------------------------------------
    add("html-simple-notes", "html", "<html><head><title>Notes</title></head><body>\n<h1>The Good Shepherd</h1>\n<p>John 10:11</p>\n<p>Psalm 23:1-4</p>\n</body></html>\n")
    add("html-list-and-table", "html", "<ul>\n<li>Romans 8:28</li>\n<li>Philippians 4:13</li>\n</ul>\n<table><tr><td>Genesis 1:1</td><td>Exodus 20:1-17</td></tr></table>\n")
    add("html-entities-named", "html", "<p>Faith &amp; works &mdash; James 2:17 &lt;read&gt; &quot;slowly&quot; &nbsp;done &copy; 2026</p>\n")
    add("html-entities-numeric", "html", "<p>&#74;ohn &#x33;:16 &#8212; &#128591; &#150; &#x80; &#0; &#xD800; &#1114112;</p>\n")
    add("html-pre-keeps-whitespace", "html", "<h2>Outline</h2>\n<pre>\n  1. Call   (Isaiah 6:8)\n\n  2. Response (Isaiah 6:9)\n</pre>\n<p>   </p>\n<p>\n\n</p>\n")
    add("html-script-style-comment", "html", "<style>p { color: red }</style>\n<script>var verse = \"John 3:16 </b>\"; if (a < b) {}</script>\n<!-- hidden Psalm 1:1 -->\n<p>Visible Psalm 1:2</p>\n")
    add("html-void-elements", "html", "<p>Line one<br>Line two<br/>Line three<hr>Matthew 5:3<img src=\"x.png\" alt=\"Matthew 5:4\"><br></br>done</p>\n")
    add("html-doctype-and-cdata", "html", "<!DOCTYPE html>\n<html><body><![CDATA[Luke 15:11 ]]><p>Luke 15:12</p></body></html>\n")
    add("html-unicode-and-nbsp", "html", "<p>Jésus\u00a0pleura — 日本語 \u201cJohn 11:35\u201d</p>\n<p>\ufeffZero width\u200b mark</p>\n")
    add("html-attributes-quoting", "html", "<p class=\"a b\" id='x' data-ref=John3:16 title=\"Say \"hi\"\" hidden>Text with attributes</p>\n<a href=\"https://example.org/?a=1&b=2\">link text</a>\n")
    add("html-textarea-and-title", "html", "<title>Notes &amp; verses</title>\n<textarea>\n  Ephesians 2:8-9\n  &lt;b&gt;</textarea>\n<p>after</p>\n")
    add("html-ruby-and-template", "html", "<ruby>漢<rt>kan</rt><rp>(</rp>字<rt>ji</rt></ruby>\n<template><p>Template text</p></template>\n<p>Visible text</p>\n")
    add("html-crlf-line-endings", "html", "<p>First line\r\nSecond line\rThird line</p>\r\n<p>Hebrews 11:1</p>\r\n")

    # -- malformed ---------------------------------------------------------------------------------------------------
    add("html-malformed-unclosed", "html", "<div><p>Opening prayer<p>Psalm 51:10<b>create in me<i>a clean heart\n<ul><li>one<li>two</ul>\n")
    add("html-malformed-stray-end-tags", "html", "</p>Intro</div>text</b></br><p>Acts 2:1</p></p></span>\n")
    add("html-malformed-lt-in-text", "html", "<p>If 1 < 2 and 3 > 2 then Romans 3:23 < everything</p><p><3 love</p><p>a<b</p>\n<p>end <\n")
    add("html-malformed-tag-in-tag", "html", "<p<b>Galatians 5:22</b></p>\n<p <i>x</i> >Galatians 5:23</p>\n<a<b>c</a>d\n")
    add("html-malformed-unquoted-and-broken-attrs", "html", "<p class=a b=\"c d=e>Micah 6:8</p>\n<p x='y\n<p =>Micah 6:9</p>\n<span a=\"1\"b=\"2\">tight</span> <i a= >spaced</i>\n")
    add("html-malformed-script-with-end-tag-text", "html", "<script>document.write('</scr' + 'ipt>'); /* </p> */</script>after script\n<script>unclosed script Psalm 100:1\n")
    add("html-malformed-unterminated-comment", "html", "<p>Before comment</p><!-- this comment never ends Psalm 119:105\n<p>after</p>\n")
    add("html-malformed-comment-oddities", "html", "<!--->a<!---->b<!-- x --!>c<!-- y -- >d--><!-->e<!x>f<!>g<!-\n<p>tail</p>\n")
    add("html-malformed-cdata-and-pi", "html", "<![CDATA[ not closed John 1:1\n<p>x</p>\n<?php echo 'Psalm 2'; ?>after pi <?xml version=\"1.0\"?>done <? unterminated pi\n")
    add("html-malformed-entities", "html", "<p>&amp &lt &gt; &notit; &notin; &foo; &#; &#x; &#65 &#x41 &#9a; &AMP; &amp-x; &a.b; &</p>\n<p>&#12\n")
    add("html-malformed-numeric-reference-swallows-document", "html", "<p>start</p>\n<p>&#12ab swallowed Psalm 8:1\n<p>never shown</p>\n")
    add("html-malformed-uppercase-and-mixed-case", "html", "<P>Upper <B>Bold</b></P><BR><Pre>\n  Keep\n</PRE><SCRIPT>x</script><TEXTAREA> t </textarea>\n")
    add("html-malformed-tag-split-across-lines", "html", "<p\n  class=\"x\"\n>Split open</p\n>\n<a href=\n\"y\"\n>link\n</a\n\n>\n")
    add("html-malformed-nested-tables", "html", "<table><tr><td><table><tr><td>inner 1<td>inner 2</table><td>outer\n</table></td></tr>\n<table><td>no row\n")
    add("html-malformed-plaintext-and-xmp", "html", "<xmp><b>raw</b> &amp;</xmp>\n<iframe>Frame <b>text</iframe>\n<plaintext>Everything <b>after</b> is raw\n</p>\n")
    add("html-malformed-whitespace-only-strings", "html", "<p>a</p>   \n\t\n<p> </p><b> </b>\n\n\n<i>\r\n</i><span>\x0c</span>\n")
    add("html-malformed-control-characters", "html", "<p>nul:\x00 bell:\x07 vt:\x0b</p><p>\ufeff</p>\n")
    add("html-malformed-only-text", "html", "Just a line of notes without any tags\nJohn 1:1-5\n")
    add("html-malformed-empty-and-bare-tags", "html", "<>\n</>\n<p></p><//><a>\n</ a>\n</1>\n< p>x</ p>\n")
    add("html-malformed-end-tag-bogus-comment", "html", "<p>one</p x=\"y\">two</p\n<p>three</3>four</ p>five</:>six\n")
    add("html-malformed-eof-in-tag", "html", "<p>Complete</p><p>Cut off <b class=\"x")

    return [(name, ext, _b(text)) for name, ext, text in c]


# ---------------------------------------------------------------------------------------------------------------------
# Word
# ---------------------------------------------------------------------------------------------------------------------

def _finish(document) -> bytes:
    """Save with fixed properties and a normalised zip so the bytes do not change between runs."""
    props = document.core_properties
    fixed = dt.datetime(2026, 1, 1, 0, 0, 0)
    props.author, props.last_modified_by, props.title, props.comments = "oracle", "oracle", "oracle", ""
    props.created, props.modified, props.last_printed, props.revision = fixed, fixed, fixed, 1
    buffer = io.BytesIO()
    document.save(buffer)
    out = io.BytesIO()
    with zipfile.ZipFile(io.BytesIO(buffer.getvalue())) as source, zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as target:
        for name in sorted(source.namelist()):
            info = zipfile.ZipInfo(name, date_time=(2026, 1, 1, 0, 0, 0))
            info.compress_type, info.external_attr = zipfile.ZIP_DEFLATED, 0o600 << 16
            target.writestr(info, source.read(name))
    return out.getvalue()


def _raw(document, xml: str, at: int | None = None) -> None:
    """Insert raw WordprocessingML (one or more block elements) into the body, before the section properties."""
    from docx.oxml import parse_xml
    from docx.oxml.ns import nsdecls
    body = document.element.body
    wrapper = parse_xml(f"<w:wrap {nsdecls('w')} xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\">{xml}</w:wrap>")
    index = len(body) - 1 if at is None and len(body) and body[-1].tag.endswith("}sectPr") else (len(body) if at is None else at)
    for element in list(wrapper):
        body.insert(index, element)
        index += 1


def _docx(build) -> bytes:
    from docx import Document
    document = Document()
    build(document)
    return _finish(document)


def docx_cases() -> list[tuple[str, str, bytes]]:
    cases: list[tuple[str, str, bytes]] = []

    def case(name: str, build) -> None:
        cases.append((name, "docx", _docx(build)))

    def plain(document):
        for line in NOTES.rstrip("\n").split("\n"):
            document.add_paragraph(line)
    case("docx-plain-paragraphs", plain)

    def runs(document):
        paragraph = document.add_paragraph("John ")
        paragraph.add_run("3").bold = True
        paragraph.add_run(":16").italic = True
        paragraph.add_run(" For God so loved the world")
        document.add_paragraph().add_run("Second paragraph, one run")
    case("docx-runs-formatting", runs)

    def tabs_breaks(document):
        paragraph = document.add_paragraph()
        paragraph.add_run("Point one\tPsalm 23:1")
        paragraph.add_run().add_break()
        paragraph.add_run("continued after a line break")
        document.add_paragraph("Tab\tseparated\tcolumns\nand a newline typed in the text")
        _raw(document, '<w:p><w:r><w:t>A</w:t><w:cr/><w:t>B</w:t><w:noBreakHyphen/><w:t>C</w:t><w:ptab w:relativeTo="margin" w:alignment="left" w:leader="none"/><w:t>D</w:t></w:r></w:p>')
    case("docx-tabs-and-line-breaks", tabs_breaks)

    def page_break(document):
        document.add_paragraph("Before the page break")
        document.add_page_break()
        _raw(document, '<w:p><w:r><w:t>column</w:t><w:br w:type="column"/><w:t>break</w:t><w:br w:type="page"/><w:t>page</w:t><w:br w:type="textWrapping" w:clear="all"/><w:t>wrap</w:t></w:r></w:p>')
        document.add_paragraph("After the page break")
    case("docx-page-and-column-breaks", page_break)

    def hyperlinks(document):
        _raw(document,
             '<w:p><w:r><w:t xml:space="preserve">Read </w:t></w:r>'
             '<w:hyperlink r:id="rId99" w:history="1"><w:r><w:t>Romans 8:28</w:t></w:r><w:r><w:t xml:space="preserve"> online</w:t></w:r></w:hyperlink>'
             '<w:r><w:t xml:space="preserve"> today</w:t></w:r></w:p>'
             '<w:p><w:hyperlink w:anchor="top"><w:r><w:t>Anchor link</w:t></w:r></w:hyperlink></w:p>'
             '<w:p><w:hyperlink w:anchor="a"><w:ins w:id="1" w:author="a"><w:r><w:t>inserted inside link (not read)</w:t></w:r></w:ins></w:hyperlink></w:p>')
    case("docx-hyperlinks", hyperlinks)

    def tables(document):
        document.add_paragraph("Before the table")
        table = document.add_table(rows=2, cols=2)
        table.cell(0, 0).text = "Cell Genesis 1:1"
        table.cell(0, 1).text = "Cell Exodus 3:14"
        table.cell(1, 0).text = "Cell Leviticus 19:18"
        document.add_paragraph("After the table")
    case("docx-table-text-not-read", tables)

    def header_footer(document):
        section = document.sections[0]
        section.header.paragraphs[0].text = "Header: First Church"
        section.footer.paragraphs[0].text = "Footer: page 1"
        document.add_paragraph("Body text only: Acts 17:11")
    case("docx-header-footer-not-read", header_footer)

    def empty_paragraphs(document):
        document.add_paragraph("Start")
        document.add_paragraph("")
        document.add_paragraph("")
        document.add_paragraph("   ")
        _raw(document, "<w:p/>")
        document.add_paragraph("End")
    case("docx-empty-paragraphs", empty_paragraphs)

    def spaces(document):
        document.add_paragraph("  leading and trailing spaces  ")
        _raw(document,
             '<w:p><w:r><w:t xml:space="preserve">  kept  </w:t></w:r><w:r><w:t> </w:t></w:r><w:r><w:t>x</w:t></w:r></w:p>'
             '<w:p><w:r><w:t>   </w:t></w:r></w:p>'
             '<w:p><w:r><w:t xml:space="default">  default  </w:t></w:r></w:p>'
             '<w:p><w:r><w:t>two  inner   spaces</w:t></w:r></w:p>')
    case("docx-spaces-and-xml-space", spaces)

    def unicode_text(document):
        document.add_paragraph("Jésus pleura — “John 11:35” 日本語 \u00a0nbsp \u2028 line separator")
        document.add_paragraph("emoji 🙏 and ligature ﬁnal ﬂow")
        _raw(document, '<w:p><w:r><w:t>&lt;tag&gt; &amp; &#x41;&#66; \ufeffbom</w:t></w:r></w:p>')
    case("docx-unicode-text", unicode_text)

    def tracked(document):
        _raw(document,
             '<w:p><w:r><w:t>Kept </w:t></w:r>'
             '<w:ins w:id="1" w:author="pastor"><w:r><w:t>inserted (not read) </w:t></w:r></w:ins>'
             '<w:del w:id="2" w:author="pastor"><w:r><w:delText>deleted </w:delText></w:r></w:del>'
             '<w:r><w:t>end</w:t></w:r></w:p>'
             '<w:p><w:smartTag w:uri="urn:x" w:element="place"><w:r><w:t>smart tag run (not read)</w:t></w:r></w:smartTag><w:r><w:t>plain run</w:t></w:r></w:p>')
    case("docx-tracked-changes-and-smart-tags", tracked)

    def sdt(document):
        _raw(document,
             '<w:sdt><w:sdtPr><w:alias w:val="Title"/></w:sdtPr><w:sdtContent><w:p><w:r><w:t>Content control paragraph (not read)</w:t></w:r></w:p></w:sdtContent></w:sdt>'
             '<w:p><w:sdt><w:sdtContent><w:r><w:t>inline control run (not read)</w:t></w:r></w:sdtContent></w:sdt><w:r><w:t>after control</w:t></w:r></w:p>')
    case("docx-content-controls", sdt)

    def fields(document):
        _raw(document,
             '<w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> PAGE </w:instrText></w:r>'
             '<w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>7</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>'
             '<w:p><w:fldSimple w:instr="DATE"><w:r><w:t>simple field (not read)</w:t></w:r></w:fldSimple><w:r><w:t>text</w:t></w:r></w:p>'
             '<w:p><w:bookmarkStart w:id="0" w:name="b"/><w:r><w:t>bookmarked</w:t></w:r><w:bookmarkEnd w:id="0"/><w:proofErr w:type="spellStart"/></w:p>')
    case("docx-fields-and-bookmarks", fields)

    def textbox_and_alternate(document):
        _raw(document,
             '<w:p><w:r><w:pict><v:textbox xmlns:v="urn:schemas-microsoft-com:vml"><w:txbxContent><w:p><w:r><w:t>Text box (not read)</w:t></w:r></w:p></w:txbxContent></v:textbox></w:pict></w:r><w:r><w:t>around</w:t></w:r></w:p>'
             '<w:p><w:r><mc:AlternateContent xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006"><mc:Choice Requires="wps"><w:t>choice</w:t></mc:Choice><mc:Fallback><w:t>fallback</w:t></mc:Fallback></mc:AlternateContent></w:r><w:r><w:t>end</w:t></w:r></w:p>')
    case("docx-text-box-and-alternate-content", textbox_and_alternate)

    def odd_text_elements(document):
        _raw(document,
             '<w:p><w:r><w:t>a<!-- comment -->b</w:t></w:r></w:p>'
             '<w:p><w:r><w:t><![CDATA[cdata <kept>]]></w:t></w:r></w:p>'
             '<w:p><w:r><w:t>  <w:x/>after child</w:t></w:r></w:p>'
             '<w:p><w:r><w:t xml:space="preserve">  <w:x/>after child preserved</w:t></w:r></w:p>'
             '<w:p><w:r><w:sym w:font="Wingdings" w:char="F0FC"/><w:t>sym</w:t></w:r></w:p>')
    case("docx-odd-text-elements", odd_text_elements)

    def headings_and_lists(document):
        document.add_heading("The Good Shepherd", level=1)
        document.add_paragraph("John 10:11", style="List Bullet")
        document.add_paragraph("Psalm 23:1-4", style="List Number")
        document.add_paragraph("Quote: Isaiah 40:11", style="Quote")
    case("docx-headings-and-lists", headings_and_lists)

    return cases


# ---------------------------------------------------------------------------------------------------------------------
# plain text
# ---------------------------------------------------------------------------------------------------------------------

def text_cases() -> list[tuple[str, str, bytes]]:
    return [
        ("text-crlf-line-endings", "txt", _b("Sermon: Grace\r\nJohn 1:14\r\nEphesians 2:8-9\r\n")),
        ("text-lone-cr-line-endings", "txt", _b("Sermon: Peace\rJohn 14:27\r\rPhilippians 4:7\r")),
        ("text-bom-and-no-trailing-newline", "txt", b"\xef\xbb\xbf" + _b("Sermon: Hope\nRomans 15:13")),
        ("text-tabs-and-indent", "md", _b("# Outline\n\n\t1. Call\tIsaiah 6:8\n\t2. Response\tIsaiah 6:9\n   - spaces   inside   \n")),
        ("text-nbsp-and-odd-unicode", "txt", _b("Jésus\u00a0pleura\nJohn\u00a011:35\n\u201cSmart quotes\u201d \u2014 dash \u2026 ellipsis\n\u200bzero width\u200b\nfi\ufb01 ligature\n\u2028line separator\u2029paragraph separator\n日本語 🙏\n")),
        ("text-mixed-line-endings-and-blank-lines", "md", _b("Line one\r\n\r\nLine two\n\n\rLine three\r\n   \nLast")),
    ]


# ---------------------------------------------------------------------------------------------------------------------
# inputs that raise
# ---------------------------------------------------------------------------------------------------------------------

_CONTENT_TYPES = (
    '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
    '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
    '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>'
    '<Default Extension="xml" ContentType="application/xml"/>'
    '<Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>'
    "</Types>"
)
_RELS = (
    '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
    '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
    '<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>'
    "</Relationships>"
)
_DOCUMENT = (
    '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
    '<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>'
    "<w:p><w:r><w:t>John 3:16</w:t></w:r></w:p><w:p><w:r><w:t>Psalm 23:1</w:t></w:r></w:p>"
    "</w:body></w:document>"
)


def _zip(files: dict[str, str | bytes]) -> bytes:
    out = io.BytesIO()
    with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as target:
        for name, content in files.items():
            info = zipfile.ZipInfo(name, date_time=(2026, 1, 1, 0, 0, 0))
            info.compress_type, info.external_attr = zipfile.ZIP_DEFLATED, 0o600 << 16
            target.writestr(info, content if isinstance(content, bytes) else content.encode("utf-8"))
    return out.getvalue()


def _package(**overrides: str | bytes | None) -> dict[str, str | bytes]:
    files: dict[str, str | bytes] = {"[Content_Types].xml": _CONTENT_TYPES, "_rels/.rels": _RELS, "word/document.xml": _DOCUMENT}
    mapping = {"content_types": "[Content_Types].xml", "rels": "_rels/.rels", "document": "word/document.xml"}
    for key, value in overrides.items():
        if value is None:
            files.pop(mapping[key], None)
        else:
            files[mapping[key]] = value
    return files


def error_cases() -> list[tuple[str, str, bytes]]:
    good = _zip(_package())
    cases: list[tuple[str, str, bytes]] = [
        ("error-txt-invalid-start-byte", "txt", b"\xff\xfeSermon notes saved as UTF-16\n"),
        ("error-txt-latin1-accent", "txt", b"Caf\xe9 sermon\nJohn 3:16\n"),
        ("error-md-truncated-multibyte", "md", _b("Notes\nPsalm 23\n") + "€".encode()[:2]),
        ("error-md-invalid-continuation", "md", b"abc\xe2\x28\xa1 broken sequence\n"),
        ("error-html-invalid-utf8", "html", b"<p>Cr\xe8me br\xfbl\xe9e</p>\n"),
        ("error-docx-not-a-zip", "docx", b"This is a text file renamed to .docx\nJohn 3:16\n"),
        ("error-docx-empty-file", "docx", b""),
        ("error-docx-truncated-zip", "docx", good[: len(good) // 2]),
        ("error-docx-zip-without-content-types", "docx", _zip(_package(content_types=None))),
        ("error-docx-zip-without-package-relationships", "docx", _zip(_package(rels=None))),
        ("error-docx-zip-without-document-part", "docx", _zip(_package(document=None))),
        ("error-docx-empty-zip", "docx", _zip({})),
        ("error-docx-corrupt-document-xml", "docx", _zip(_package(document=_DOCUMENT[: len(_DOCUMENT) - 30]))),
        ("error-docx-document-xml-tag-mismatch", "docx", _zip(_package(document=_DOCUMENT.replace("</w:body>", "</w:bodi>")))),
        ("error-docx-empty-document-xml", "docx", _zip(_package(document=""))),
        ("error-docx-corrupt-content-types", "docx", _zip(_package(content_types="<Types"))),
        ("error-docx-document-without-body", "docx", _zip(_package(document=_DOCUMENT.replace("<w:body>", "<w:x>").replace("</w:body>", "</w:x>")))),
        ("error-docx-not-a-word-document", "docx", _zip(_package(content_types=_CONTENT_TYPES.replace("document.main", "template.main")))),
        ("error-docx-relationship-to-missing-part", "docx", _zip(_package(rels=_RELS.replace("</Relationships>", '<Relationship Id="rId2" Type="http://x/y" Target="media/gone.png"/></Relationships>')))),
    ]
    return cases
