"""Writes the PDF fixtures of the oracle's S4 cases (tests/fixtures/pdf/*.pdf) with ReportLab, which is not an app dependency:

    uv run --no-project --with reportlab --with pikepdf python scripts/make_pdf_fixtures.py [out_dir]

The files are committed; the oracle exporter reads them (so ``export_oracle.py`` needs nothing but the locked environment).
Every file is deterministic (``invariant=1``: no dates, fixed document id). Regenerating them changes nothing but ReportLab's own version.
"""
from __future__ import annotations

import sys
from pathlib import Path

from reportlab.lib import colors
from reportlab.lib.enums import TA_CENTER, TA_JUSTIFY, TA_RIGHT
from reportlab.lib.pagesizes import A4, letter
from reportlab.lib.styles import ParagraphStyle, getSampleStyleSheet
from reportlab.lib.units import inch
from reportlab.pdfbase import pdfmetrics
from reportlab.pdfbase.cidfonts import UnicodeCIDFont
from reportlab.pdfbase.ttfonts import TTFont
from reportlab.pdfgen import canvas
from reportlab.platypus import (BaseDocTemplate, Frame, ListFlowable, ListItem, PageBreak, PageTemplate, Paragraph, Preformatted,
                                SimpleDocTemplate, Spacer, Table, TableStyle)

DEJAVU = "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf"
DEJAVU_BOLD = "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf"
SERIF = "/usr/share/fonts/truetype/dejavu/DejaVuSerif.ttf"
MONO = "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf"

SERMON = [
    "Sermon: The Good Shepherd",
    "Series: Psalm 23 - Whitefield Church",
    "1) The Lord is my shepherd; I shall not want. He makes me lie down in green pastures.",
    "Psalm 23:1-2",
    "2) Even though I walk through the valley of the shadow of death, I will fear no evil, for you are with me.",
    "Psalm 23:4",
    "3) I am the good shepherd. The good shepherd lays down his life for the sheep.",
    "John 10:11 NKJV",
    "Conclusion: Surely goodness and mercy shall follow me all the days of my life.",
]
LONG = ("God saves by grace through faith in Jesus Christ and not by anything we have done, so that no one can boast; "
        "we are his workmanship, created in Christ Jesus to do good works, which God prepared in advance for us to do. ") * 3


def register() -> None:
    pdfmetrics.registerFont(TTFont("DejaVu", DEJAVU))
    pdfmetrics.registerFont(TTFont("DejaVu-Bold", DEJAVU_BOLD))
    pdfmetrics.registerFont(TTFont("DejaVuSerif", SERIF))
    pdfmetrics.registerFont(TTFont("DejaVuMono", MONO))


def new_canvas(path: Path, **kw) -> canvas.Canvas:
    return canvas.Canvas(str(path), invariant=1, pageCompression=kw.pop("compress", 0), **kw)


def lines_page(c: canvas.Canvas, lines: list[str], font: str = "Helvetica", size: int = 11, x: float = 56, top: float = 780, leading: float = 15) -> None:
    c.setFont(font, size)
    y = top
    for line in lines:
        c.drawString(x, y, line)
        y -= leading


def platypus(path: Path, story, **kw) -> None:
    doc = SimpleDocTemplate(str(path), pagesize=kw.pop("pagesize", letter), invariant=1, pageCompression=kw.pop("compress", 0), **kw)
    doc.build(story)


def main(out: Path) -> None:
    out.mkdir(parents=True, exist_ok=True)
    register()
    styles = getSampleStyleSheet()
    body = styles["BodyText"]
    h1 = styles["Heading1"]
    h2 = styles["Heading2"]

    # 01 plain lines, standard Helvetica
    c = new_canvas(out / "lines-helvetica.pdf"); lines_page(c, SERMON); c.save()
    # 02 the same in Times, compressed
    c = new_canvas(out / "lines-times-compressed.pdf", compress=1); lines_page(c, SERMON, "Times-Roman", 12); c.save()
    # 03 Courier (fixed pitch)
    c = new_canvas(out / "lines-courier.pdf"); lines_page(c, SERMON, "Courier", 10); c.save()
    # 04 wrapped paragraphs (platypus)
    platypus(out / "wrapped-paragraphs.pdf", [Paragraph("The Good Shepherd", h1), Paragraph(LONG, body), Paragraph("Psalm 23:1-4", body), Paragraph(LONG, body)])
    # 05 justified paragraphs (word spacing: Tw)
    just = ParagraphStyle("just", parent=body, alignment=TA_JUSTIFY, fontSize=11, leading=15)
    platypus(out / "justified.pdf", [Paragraph(LONG, just), Paragraph(LONG, just)])
    # 06 centered and right aligned
    ctr = ParagraphStyle("ctr", parent=body, alignment=TA_CENTER)
    rgt = ParagraphStyle("rgt", parent=body, alignment=TA_RIGHT)
    platypus(out / "aligned.pdf", [Paragraph("Centered title", ctr), Paragraph("John 3:16 right aligned", rgt), Paragraph(LONG, ctr)])
    # 07 two columns (two frames)
    path = out / "two-columns.pdf"
    doc = BaseDocTemplate(str(path), pagesize=letter, invariant=1, pageCompression=0)
    frames = [Frame(40, 40, 260, 700, id="a"), Frame(312, 40, 260, 700, id="b")]
    doc.addPageTemplates([PageTemplate(id="two", frames=frames)])
    doc.build([Paragraph("Left column heading", h2), Paragraph(LONG, body), Paragraph(LONG, body), Paragraph("Right side text continues here", body), Paragraph(LONG, body)])
    # 08 bullets
    items = [ListItem(Paragraph(t, body)) for t in ("Faith comes by hearing - Romans 10:17", "Grace alone - Ephesians 2:8-9", "Christ alone - John 14:6")]
    platypus(out / "bullets.pdf", [Paragraph("Outline", h2), ListFlowable(items, bulletType="bullet"), ListFlowable(items, bulletType="1")])
    # 09 bullets drawn with a symbol character in a TTF
    c = new_canvas(out / "bullets-unicode-ttf.pdf")
    c.setFont("DejaVu", 11)
    y = 760
    for line in ("\u2022 Romans 8:28 - all things work together", "\u2022 Philippians 4:13", "\u25e6 sub point one", "\u2013 dash point", "\u00b7 middle dot point"):
        c.drawString(60, y, line); y -= 16
    c.save()
    # 10 ligature characters in a TTF (U+FB00..FB06 drawn directly)
    c = new_canvas(out / "ligatures-ttf.pdf")
    lines_page(c, ["of\ufb01ce \ufb00 e\ufb00ort \ufb01nd \ufb02ow \ufb03 su\ufb03x \ufb04 ba\ufb04e \ufb05 \ufb06 last", "normal line after ligatures"], "DejaVu")
    c.save()
    # 11 ligature characters in standard Type1 (WinAnsi cannot show them: the glyph is dropped or replaced)
    c = new_canvas(out / "ligatures-type1.pdf")
    lines_page(c, ["of\ufb01ce and e\ufb00ort in Times", "second line"], "Times-Roman")
    c.save()
    # 12 symbols: Symbol and ZapfDingbats (their own encodings)
    c = new_canvas(out / "symbol-zapf.pdf")
    c.setFont("Symbol", 12); c.drawString(60, 760, "abgd alpha beta gamma 123 ()")
    c.setFont("ZapfDingbats", 12); c.drawString(60, 740, "4 8 ABC check marks")
    c.setFont("Helvetica", 12); c.drawString(60, 720, "plain after symbols")
    c.save()
    # 13 non-embedded fonts with special WinAnsi characters (curly quotes, dashes, euro, accents)
    c = new_canvas(out / "winansi-specials.pdf")
    lines_page(c, ["\u201cSmart quotes\u201d \u2018single\u2019 \u2013 en \u2014 em \u2026 \u20ac5 caf\u00e9 na\u00efve \u00fcber \u00a9 2026", "Jos\u00e9 Mu\u00f1oz said: \u201cJohn 3:16\u201d"], "Helvetica-Bold", 12)
    c.save()
    # 14 TTF with accents / Cyrillic / Greek
    c = new_canvas(out / "ttf-unicode.pdf")
    lines_page(c, ["Caf\u00e9 na\u00efve \u00fcber \u00e9l\u00e8ve", "\u041f\u0440\u0438\u0432\u0435\u0442 \u043c\u0438\u0440 \u03b1\u03b2\u03b3 \u03b4\u03b5", "Jes\u00fas lloró \u2014 \u201cJuan 11:35\u201d"], "DejaVu")
    c.save()
    # 15 TTF bold + serif + mono mixed on lines
    c = new_canvas(out / "ttf-mixed-fonts.pdf")
    c.setFont("DejaVu-Bold", 14); c.drawString(60, 760, "Title in bold")
    c.setFont("DejaVuSerif", 11); c.drawString(60, 740, "Serif body text that is rather long to fill the line up to the margin of the page")
    c.setFont("DejaVuMono", 10); c.drawString(60, 720, "mono    spaced   text")
    c.save()
    # 16 rotated page (/Rotate 90)
    c = new_canvas(out / "rotated-page-attribute.pdf"); lines_page(c, SERMON[:4]); c.setPageRotation(90); c.save()
    # 17 text drawn rotated 90 degrees through the matrix
    c = new_canvas(out / "rotated-text-90.pdf")
    c.saveState(); c.translate(300, 100); c.rotate(90); c.setFont("Helvetica", 12)
    c.drawString(0, 0, "Rotated text line one"); c.drawString(0, -16, "Rotated text line two"); c.restoreState()
    lines_page(c, ["Horizontal line afterwards"], top=700)
    c.save()
    # 18 text rotated 180 and 270
    c = new_canvas(out / "rotated-text-180-270.pdf")
    c.saveState(); c.translate(400, 600); c.rotate(180); c.setFont("Helvetica", 12); c.drawString(0, 0, "Upside down line"); c.restoreState()
    c.saveState(); c.translate(100, 700); c.rotate(270); c.setFont("Helvetica", 12); c.drawString(0, 0, "Turned right line"); c.restoreState()
    c.save()
    # 19 scanned: an image and no text (error case)
    from reportlab.lib.utils import ImageReader
    import io
    raw = bytes([(x * 7 + y * 13) % 256 for y in range(64) for x in range(64)])
    c = new_canvas(out / "scanned-image-only.pdf")
    c.drawImage(ImageReader(_png(64, 64, raw)), 56, 400, width=300, height=300)
    c.save()
    # 20 empty page
    c = new_canvas(out / "blank-page.pdf"); c.showPage(); c.save()
    # 21 multiple pages
    c = new_canvas(out / "multi-page.pdf")
    for n in range(1, 5):
        lines_page(c, [f"Page {n} line one", f"Page {n} line two - John {n}:16", ""]); c.showPage()
    c.save()
    # 22 table
    data = [["Verse", "Topic", "Note"], ["John 3:16", "Love", "God so loved"], ["Romans 8:28", "Providence", "All things"], ["Psalm 23:1", "Shepherd", "I shall not want"]]
    t = Table(data); t.setStyle(TableStyle([("GRID", (0, 0), (-1, -1), 0.5, colors.black)]))
    platypus(out / "table.pdf", [Paragraph("Verses", h2), t])
    # 23 preformatted text
    platypus(out / "preformatted.pdf", [Preformatted("1) First point\n    indented detail\n2) Second point\n\nPsalm 1:1", styles["Code"])])
    # 24 text object with char and word spacing, leading and T* / quote operators
    c = new_canvas(out / "text-object-spacing.pdf")
    to = c.beginText(60, 760); to.setFont("Helvetica", 12); to.setCharSpace(1.5); to.setWordSpace(6); to.setLeading(18)
    to.textLine("Spaced characters and words"); to.textLine("second line of the object"); to.setCharSpace(0); to.setWordSpace(0)
    to.textLines("third\nfourth\nfifth"); c.drawText(to); c.save()
    # 25 horizontal scaling and rise
    c = new_canvas(out / "text-object-scale-rise.pdf")
    to = c.beginText(60, 760); to.setFont("Helvetica", 12); to.setHorizScale(80); to.textLine("Condensed text line"); to.setHorizScale(100)
    to.setRise(5); to.textOut("raised"); to.setRise(0); to.textOut(" normal again"); c.drawText(to); c.save()
    # 26 many small drawString calls on one line (words placed individually)
    c = new_canvas(out / "word-by-word.pdf"); c.setFont("Helvetica", 12)
    x = 60
    for word in "The Lord is my shepherd I shall not want".split():
        c.drawString(x, 760, word); x += pdfmetrics.stringWidth(word + " ", "Helvetica", 12)
    x = 60
    for word in "gaps  wider than a space".split():
        c.drawString(x, 740, word); x += pdfmetrics.stringWidth(word, "Helvetica", 12) + 20
    c.save()
    # 27 big and small font sizes mixed on a baseline
    c = new_canvas(out / "mixed-sizes.pdf")
    c.setFont("Helvetica", 24); c.drawString(60, 740, "Big"); c.setFont("Helvetica", 8); c.drawString(110, 740, "small beside big"); c.setFont("Helvetica", 16); c.drawString(60, 715, "Medium line")
    c.save()
    # 28 colour, shapes, and a clipped path around text
    c = new_canvas(out / "graphics-and-text.pdf")
    c.setFillColor(colors.red); c.rect(50, 700, 200, 60, fill=1); c.setFillColor(colors.black); c.setFont("Helvetica", 12); c.drawString(60, 725, "Text over a red box")
    c.line(50, 690, 300, 690); c.drawString(60, 670, "After the line")
    c.save()
    # 29 form XObject (a template reused)
    c = new_canvas(out / "form-xobject.pdf")
    c.beginForm("note"); c.setFont("Helvetica", 11); c.drawString(0, 0, "Text inside a form XObject"); c.endForm()
    c.drawString(60, 760, "Before the form"); c.saveState(); c.translate(60, 740); c.doForm("note"); c.restoreState(); c.setFont("Helvetica", 11); c.drawString(60, 700, "After the form")
    c.save()
    # 30 Japanese / Chinese with predefined CMaps (not embedded)
    pdfmetrics.registerFont(UnicodeCIDFont("HeiseiMin-W3"))
    c = new_canvas(out / "cid-japanese.pdf"); c.setFont("HeiseiMin-W3", 14); c.drawString(60, 740, "\u3053\u3093\u306b\u3061\u306f John 3:16"); c.save()
    pdfmetrics.registerFont(UnicodeCIDFont("STSong-Light"))
    c = new_canvas(out / "cid-chinese.pdf"); c.setFont("STSong-Light", 14); c.drawString(60, 740, "\u4f60\u597d\u4e16\u754c John 3:16"); c.save()
    # 31 long wrapped sentence cut by the page (the oracle's format-pdf-wrapped shape)
    c = new_canvas(out / "wrapped-by-page.pdf")
    lines_page(c, ["Title 11/22/26 Grace", "1) God saves by grace through faith in Jesus Christ and", "not by anything we have done.", "Ephesians 2:8-9 NKJV"])
    c.save()
    # 32 A4 page, 8.5pt dense text
    c = new_canvas(out / "a4-dense.pdf", pagesize=A4)
    lines_page(c, [(LONG[:110] + f" {n}") for n in range(40)], size=8, top=820, leading=10)
    c.save()
    # 33 paragraph with inline bold / italic / sup markup (several fonts per line)
    platypus(out / "inline-markup.pdf", [Paragraph("Plain <b>bold words</b> and <i>italic words</i> then <super>sup</super> and <sub>sub</sub> text, <u>underlined</u> too " * 3, body)])
    # 34 accents with a TTF at large size and a heading with letter spacing
    c = new_canvas(out / "heading-charspace.pdf")
    c.setFont("DejaVu-Bold", 20); c.setFillColor(colors.navy); c.drawCentredString(300, 740, "SERMON NOTES"); c.drawRightString(560, 710, "Right edge")
    c.save()
    # 36 encrypted with an empty user password (RC4 40 and 128 bit): readable without a password
    from reportlab.lib.pdfencrypt import StandardEncryption
    for bits in (40, 128):
        c = canvas.Canvas(str(out / f"encrypted-rc4-{bits}.pdf"), invariant=1, pageCompression=0,
                          encrypt=StandardEncryption("", ownerPassword="owner", strength=bits))
        lines_page(c, SERMON[:5]); c.save()
    # 35 text lines with trailing/leading spaces and tabs written as spaces
    c = new_canvas(out / "odd-spaces.pdf")
    lines_page(c, ["  leading spaces", "trailing spaces   ", "double  spaces  inside", "", "after blank"])
    c.save()


def restructure(out: Path) -> None:
    """Optional (needs pikepdf): the same files with object streams and cross-reference streams, and linearized."""
    try:
        import pikepdf
    except ImportError:
        return
    for name in ("wrapped-paragraphs", "two-columns", "ttf-unicode", "bullets"):
        with pikepdf.open(out / f"{name}.pdf") as pdf:
            pdf.save(out / f"objstm-{name}.pdf", object_stream_mode=pikepdf.ObjectStreamMode.generate, deterministic_id=True, compress_streams=True)
    with pikepdf.open(out / "lines-times-compressed.pdf") as pdf:
        pdf.save(out / "linearized-lines-times.pdf", linearize=True, deterministic_id=True)


def _png(width: int, height: int, gray: bytes):
    """A tiny grayscale PNG (no Pillow needed)."""
    import io
    import struct
    import zlib
    rows = b"".join(b"\x00" + gray[y * width:(y + 1) * width] for y in range(height))
    def chunk(tag: bytes, data: bytes) -> bytes:
        return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
    png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 0, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(rows, 9)) + chunk(b"IEND", b"")
    return io.BytesIO(png)


if __name__ == "__main__":
    target = Path(sys.argv[1]) if len(sys.argv) > 1 else Path(__file__).resolve().parents[1] / "tests" / "fixtures" / "pdf"
    main(target)
    restructure(target)
