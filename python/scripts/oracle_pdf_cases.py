"""PDF inputs for the oracle's S4 (document readers) cases, written with the standard library only (deterministic bytes).

* ``generated_pdfs()``  : 40 pseudo-random but fixed PDFs (seeds 0..39): several fonts per file (standard Type1 with and without an
  encoding, TrueType with ``/Widths`` and ``/ToUnicode``, Type0 ``Identity-H`` with a CID width array and a ``/ToUnicode`` CMap
  with ``bfchar`` and ``bfrange`` entries, ``/Differences``, Type3, core-font metrics under an alias name, a missing font), text
  operators in every combination (``Td TD Tm T* TL ' " Tj TJ cm q Q Do``, rotated matrices, kerning numbers around the space
  threshold), form XObjects, an inline image, comments, split ``/Contents`` arrays, flate-compressed streams and inherited
  resources. Typeset-looking pages (lines of words) and garbage pages alternate.
* ``handmade_pdfs()``   : small readable PDFs, one feature each, and files that are not readable.

The text pypdf makes of them is what the Rust reader must repeat; none of them is meant to look good.
"""
from __future__ import annotations

import random
import struct
import zlib


def esc(b: bytes) -> bytes:
    out = bytearray()
    for c in b:
        if c in b"()\\": out += b"\\" + bytes([c])
        elif c < 32 or c > 126: out += b"\\%03o" % c
        else: out.append(c)
    return bytes(out)

def hexs(b): return b"<" + b.hex().upper().encode() + b">"

class Pdf:
    def __init__(self): self.objs = []  # list of bytes bodies (1-based)
    def add(self, body: bytes) -> int:
        self.objs.append(body); return len(self.objs)
    def reserve(self) -> int:
        self.objs.append(None); return len(self.objs)
    def set(self, n, body): self.objs[n-1] = body
    def stream(self, data: bytes, extra=b"", compress=False) -> int:
        if compress:
            data = zlib.compress(data); extra += b" /Filter /FlateDecode"
        return self.add(b"<< /Length %d %s >>\nstream\n" % (len(data), extra) + data + b"\nendstream")
    def build(self, root: int) -> bytes:
        out = bytearray(b"%PDF-1.5\n%\xe2\xe3\xcf\xd3\n"); offs = []
        for i, body in enumerate(self.objs, 1):
            offs.append(len(out)); out += b"%d 0 obj\n" % i + body + b"\nendobj\n"
        x = len(out)
        out += b"xref\n0 %d\n0000000000 65535 f \n" % (len(self.objs) + 1)
        for o in offs: out += b"%010d 00000 n \n" % o
        out += b"trailer\n<< /Size %d /Root %d 0 R >>\nstartxref\n%d\n%%%%EOF\n" % (len(self.objs) + 1, root, x)
        return bytes(out)

def tounicode(rnd, kind):
    parts = [b"/CIDInit /ProcSet findresource begin 12 dict begin begincmap /CMapName /Adobe-Identity-UCS def"]
    if kind == "1byte":
        parts.append(b"1 begincodespacerange <00> <FF> endcodespacerange")
        chars = []
        for code in rnd.sample(range(32, 256), 30):
            u = rnd.choice([code, 0x2019, 0x201C, 0x2013, 0xFB01, 0xE9, 0x20, 0x41 + code % 26])
            chars.append(b"<%02X> <%04X>" % (code, u))
        parts.append(b"%d beginbfchar\n" % len(chars) + b"\n".join(chars) + b"\nendbfchar")
        a = rnd.randint(32, 200)
        parts.append(b"2 beginbfrange\n<%02X> <%02X> <%04X>\n<%02X> <%02X> [<0041> <0042> <FB02>]\nendbfrange" % (a, a + 5, 0x61 + rnd.randint(0, 10), a + 10, a + 12))
    else:
        parts.append(b"1 begincodespacerange <0000> <FFFF> endcodespacerange")
        chars = []
        for code in range(1, 40):
            u = rnd.choice([0x41 + code % 26, 0x20, 0x61 + code % 26, 0xFB01, 0x2019, 0x5D0 + code % 20, 0x627 + code % 20, 0x4E00 + code])
            if code == 3: u = 0x20
            chars.append(b"<%04X> <%04X>" % (code, u))
        parts.append(b"%d beginbfchar\n" % len(chars) + b"\n".join(chars) + b"\nendbfchar")
        parts.append(b"1 beginbfrange\n<0030> <0040> <0061>\nendbfrange")
        if rnd.random() < .3:
            parts.append(b"1 beginbfrange\n<0050> <0052> [<0041> <00420043> <0044>]\nendbfrange")
    parts.append(b"endcmap CMapName currentdict /CMap defineresource pop end end")
    return b"\n".join(parts)

def make_fonts(rnd, pdf: Pdf):
    """returns list of (resource name, obj number, kind) and a dict of kind-> byte alphabets."""
    fonts = []
    def fnt(name, body, kind, alphabet): fonts.append((name, pdf.add(body), kind, alphabet))
    printable = bytes(range(32, 127))
    high = bytes(range(128, 256))
    fnt("F1", b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>", "t1", printable + high)
    fnt("F2", b"<< /Type /Font /Subtype /Type1 /BaseFont /Times-Roman >>", "t1", printable + high)
    fnt("F3", b"<< /Type /Font /Subtype /Type1 /BaseFont /Symbol >>", "t1", printable + high)
    widths = b" ".join((b"%d" % rnd.randint(200, 900) if rnd.random() < .8 else b"%.1f" % rnd.uniform(200, 900)) for _ in range(95))
    tu = pdf.stream(tounicode(rnd, "1byte"))
    fnt("F4", b"<< /Type /Font /Subtype /TrueType /BaseFont /ABCDEF+Foo /FirstChar 32 /LastChar 126 /Widths [%s] /Encoding /WinAnsiEncoding /ToUnicode %d 0 R /FontDescriptor << /Type /FontDescriptor /FontName /ABCDEF+Foo /Flags 32 >> >>" % (widths, tu), "tt", printable + high)
    tu2 = pdf.stream(tounicode(rnd, "2byte"), compress=rnd.random() < .5)
    fnt("F5", b"<< /Type /Font /Subtype /Type0 /BaseFont /ABCDEF+Cid /Encoding /Identity-H /DescendantFonts [<< /Type /Font /Subtype /CIDFontType2 /BaseFont /ABCDEF+Cid /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> /DW %d /W [1 [500 600 700] 5 20 450 21 [300 310.5]] /FontDescriptor << /Type /FontDescriptor /FontName /ABCDEF+Cid /Flags 4 >> >>] /ToUnicode %d 0 R >>" % (rnd.choice([1000, 500, 0, 800]), tu2), "cid", b"")
    fnt("F6", b"<< /Type /Font /Subtype /Type1 /BaseFont /Palatino-Roman /Encoding << /Type /Encoding /BaseEncoding /MacRomanEncoding /Differences [32 /space /fi /fl 65 /Euro /g17 /uni0041 /Aacute 200 /quoteright /bullet /unknownglyph] >> >>", "t1", printable + high)
    fnt("F7", b"<< /Type /Font /Subtype /Courier /BaseFont /Courier-Bold /Encoding /WinAnsiEncoding /FontDescriptor << /Type /FontDescriptor /FontName /Courier-Bold /Flags 33 /MissingWidth 600 >> >>".replace(b"/Subtype /Courier", b"/Subtype /Type1"), "t1", printable + high)
    fnt("F8", b"<< /Type /Font /Subtype /Type1 /BaseFont /Arial,Bold /Encoding /WinAnsiEncoding >>", "t1", printable + high)
    fnt("F9", b"<< /Type /Font /Subtype /Type1 /BaseFont /Unknown-Font /Encoding /MacRomanEncoding /FirstChar 32 /Widths [250 333 408] /FontDescriptor << /Type /FontDescriptor /FontName /Unknown-Font /Flags 34 /MissingWidth 400 >> >>", "t1", printable + high)
    cp1 = pdf.stream(b"0 0 500 700 d1 0 0 500 700 re f")
    fnt("F10", b"<< /Type /Font /Subtype /Type3 /FontBBox [0 0 500 700] /FontMatrix [0.001 0 0 0.001 0 0] /CharProcs << /a %d 0 R /b %d 0 R >> /Encoding << /Type /Encoding /Differences [97 /a /b] >> /FirstChar 97 /LastChar 98 /Widths [500 600] >>" % (cp1, cp1), "t3", bytes(range(97, 99)) + b" ")
    tu3 = pdf.stream(tounicode(rnd, "1byte"))
    fnt("F11", b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /ToUnicode %d 0 R >>" % tu3, "t1", printable + high)
    fnt("F12", b"<< /Type /Font /Subtype /TrueType /BaseFont /Georgia /Encoding /Identity-H /FirstChar 0 /Widths [%s] >>" % b" ".join(b"%d" % rnd.randint(300, 800) for _ in range(30)), "t1", b"\x00\x01\x02\x03\x04")
    return fonts

def rand_string(rnd, font):
    name, num, kind, alpha = font
    if kind == "cid":
        n = rnd.randint(1, 8)
        codes = [rnd.choice([1,2,3,4,5,6,7,10,20,21,0x30,0x31,0x35,0x50,0x51,0x52,0x7F, 9999]) for _ in range(n)]
        return b"".join(struct.pack(">H", c) for c in codes)
    n = rnd.randint(0, 14)
    words = []
    s = bytearray()
    for _ in range(n):
        s.append(rnd.choice(alpha) if (alpha and rnd.random() < .85) else 32)
    return bytes(s)

def num(rnd, lo=-50, hi=50):
    r = rnd.random()
    if r < .45: return b"%d" % rnd.randint(lo, hi)
    if r < .9: return (b"%.3f" % rnd.uniform(lo, hi)).rstrip(b"0").rstrip(b".") or b"0"
    return rnd.choice([b"0", b".5", b"-.25", b"+3", b"1e3"][:4])

def content(rnd, fonts, forms, depth=0):
    ops = []
    def add(*parts): ops.append(b" ".join(parts))
    names = [f[0] for f in fonts] + [b"F99".decode()]
    fontmap = {f[0]: f for f in fonts}
    cur = fonts[0]
    in_bt = False
    for _ in range(rnd.randint(5, 60)):
        r = rnd.random()
        if r < .06:
            add(b"BT"); in_bt = True
        elif r < .09:
            add(b"ET"); in_bt = False
        elif r < .16:
            nm = rnd.choice(names); cur = fontmap.get(nm, cur)
            add(b"/" + nm.encode(), rnd.choice([b"12", b"10", b"11.5", b"9", b"14", b"0", b"-12", b"1"]), b"Tf")
        elif r < .26:
            add(num(rnd, -30, 120), num(rnd, -40, 40), b"Td")
        elif r < .31:
            add(num(rnd, -30, 120), num(rnd, -40, 40), b"TD")
        elif r < .36:
            add(rnd.choice([b"1 0 0 1", b"0 1 -1 0", b"-1 0 0 -1", b"0 -1 1 0", b"1.2 0 0 1.2", b"0.5 0.5 -0.5 0.5", b"2 0 0 -2"]), num(rnd, 0, 500), num(rnd, 0, 700), b"Tm")
        elif r < .40:
            add(b"T*")
        elif r < .43:
            add(num(rnd, 0, 30), b"TL")
        elif r < .53:
            add(b"(" + esc(rand_string(rnd, cur)) + b")", b"Tj")
        elif r < .60:
            add(b"(" + esc(rand_string(rnd, cur)) + b")", b"'")
        elif r < .63:
            add(num(rnd, 0, 5), num(rnd, 0, 5), b"(" + esc(rand_string(rnd, cur)) + b")", b'"')
        elif r < .78:
            items = []
            for _ in range(rnd.randint(1, 6)):
                if rnd.random() < .6: items.append(b"(" + esc(rand_string(rnd, cur)) + b")" if rnd.random() < .7 else hexs(rand_string(rnd, cur)))
                else: items.append(rnd.choice([b"-250", b"-120", b"-1000", b"30", b"-600.5", b"-50", b"200", b"-300"]))
            add(b"[" + b" ".join(items) + b"]", b"TJ")
        elif r < .83:
            add(b"q")
        elif r < .87:
            add(b"Q")
        elif r < .90:
            add(rnd.choice([b"1 0 0 1", b"0 1 -1 0", b"2 0 0 2", b"1 0 0 -1", b"0.5 0 0 0.5"]), num(rnd, 0, 100), num(rnd, 0, 100), b"cm")
        elif r < .92:
            add(rnd.choice([b"0 0 100 20 re", b"10 10 m", b"0 0 m 100 100 l S", b"f", b"W n", b"0.5 g", b"1 0 0 rg"]))
        elif r < .93:
            add(num(rnd, 0, 3), b"Tc"); 
        elif r < .94:
            add(num(rnd, 0, 8), b"Tw"); 
        elif r < .95:
            add(rnd.choice([b"50", b"100", b"120"]), b"Tz")
        elif r < .96 and forms and depth == 0:
            add(b"/" + rnd.choice(forms).encode(), b"Do")
        elif r < .97:
            add(b"/Span <</MCID 3 /Lang (en)>> BDC"); add(b"EMC")
        elif r < .98:
            add(b"% a comment (with parens")
        elif r < .985:
            add(b"BI /W 2 /H 2 /BPC 8 /CS /G ID abcd EI")
    return b"\n".join(ops) + b"\n"

WORDS = "The Lord is my shepherd I shall not want He makes me lie down in green pastures leads beside still waters restores soul John 3:16 Psalm 23:1-4 Romans grace faith 1) 2) \u2022 God so loved the world that he gave".split()

def typeset(rnd, fonts):
    ops = []
    fs = rnd.choice([9, 10, 10.5, 11, 12, 14])
    leading = fs * rnd.choice([1.0, 1.15, 1.2, 1.5])
    x0 = rnd.choice([36, 54, 72, 72.5, 90.25])
    y = 760 + rnd.random()
    f = rnd.choice([x for x in fonts if x[2] in ("t1", "tt")] or fonts)
    ops.append(b"BT /%s %s Tf" % (f[0].encode(), (b"%g" % fs)))
    mode = rnd.choice(["td", "tm", "tl"])
    if mode == "tl": ops.append(b"%g TL %g %g Td" % (leading, x0, y))
    for line in range(rnd.randint(3, 25)):
        words = [rnd.choice(WORDS) for _ in range(rnd.randint(1, 12))]
        if mode == "td": ops.append(b"1 0 0 1 %g %g Tm" % (x0, round(y - line * leading, rnd.choice([0, 1, 3]))))
        elif mode == "tm": ops.append(b"%g 0 0 %g %g %g Tm" % (rnd.choice([1, 1, 0.9]), rnd.choice([1, 1, 0.9]), x0, y - line * leading))
        kind = rnd.choice(["tj", "tjwords", "tjkern", "tjkern2"])
        enc = lambda w: w.encode("cp1252", "replace")
        if kind == "tj": ops.append(b"(" + esc(enc(" ".join(words))) + b") Tj")
        elif kind == "tjwords":
            for w in words: ops.append(b"(" + esc(enc(w)) + b") Tj %g 0 Td" % (len(w) * fs * .5 + rnd.choice([0, 0, fs * .3, fs * .15, 4])))
        else:
            parts = []
            for i, w in enumerate(words):
                parts.append(b"(" + esc(enc(w)) + b")")
                if i < len(words) - 1: parts.append(rnd.choice([b"-250", b"-333", b"-200", b"-180", b"-120.5", b"-1000", b"-278", b"12", b"-30"]) if kind == "tjkern" else rnd.choice([b"-310", b"-190", b"-210", b"-600", b"5", b"-40"]))
            ops.append(b"[" + b" ".join(parts) + b"] TJ")
        if mode == "tl": ops.append(b"T*")
        elif mode == "td" and rnd.random() < .1: ops.append(b"0 %g Td" % -leading)
    ops.append(b"ET")
    return b"\n".join(ops) + b"\n"

def make_pdf(seed: int) -> bytes:
    rnd = random.Random(seed)
    pdf = Pdf()
    fonts = make_fonts(rnd, pdf)
    use = rnd.sample(fonts, rnd.randint(2, len(fonts)))
    fontres = b"<< " + b" ".join(b"/%s %d 0 R" % (f[0].encode(), f[1]) for f in use) + b" >>"
    # form xobjects
    forms = []
    xobj = b""
    for k in range(rnd.randint(0, 2)):
        body = content(rnd, use, [], depth=1)
        n = pdf.stream(body, b"/Type /XObject /Subtype /Form /BBox [0 0 600 800] /Resources << /Font %s >>" % fontres, compress=rnd.random() < .3)
        forms.append("Fm%d" % k); xobj += b"/Fm%d %d 0 R " % (k, n)
    res = b"<< /Font %s /XObject << %s >> >>" % (fontres, xobj)
    pages_n = pdf.reserve()
    page_objs = []
    inherit = rnd.random() < .3
    for p in range(rnd.randint(1, 3)):
        body = typeset(rnd, use) if rnd.random() < .5 else content(rnd, use, forms)
        r = rnd.random()
        if r < .25:
            half = len(body) // 2
            cut = body.find(b"\n", half)
            parts = [body[:cut + 1], body[cut + 1:]]
            cs = b"[" + b" ".join(b"%d 0 R" % pdf.stream(x, compress=rnd.random() < .5) for x in parts) + b"]"
        else:
            cs = b"%d 0 R" % pdf.stream(body, compress=rnd.random() < .5)
        page_objs.append(pdf.add(b"<< /Type /Page /Parent %d 0 R /MediaBox [0 0 612 792] /Contents %s %s >>" % (pages_n, cs, b"" if inherit else b"/Resources " + res)))
    pdf.set(pages_n, b"<< /Type /Pages /Kids [%s] /Count %d %s >>" % (b" ".join(b"%d 0 R" % p for p in page_objs), len(page_objs), (b"/Resources " + res) if inherit else b""))
    root = pdf.add(b"<< /Type /Catalog /Pages %d 0 R >>" % pages_n)
    return pdf.build(root)



def generated_pdfs(count: int = 40) -> list[tuple[str, str, bytes]]:
    return [(f"pdf-generated-{seed:02d}", "pdf", make_pdf(seed)) for seed in range(count)]


def _one_page(content: bytes, resources: bytes, extra_objects=()) -> bytes:
    pdf = Pdf()
    objs = [pdf.add(o) for o in extra_objects]  # objects 1..n referenced by number in `resources`
    stream = pdf.stream(content)
    pages = pdf.reserve()
    page = pdf.add(b"<< /Type /Page /Parent %d 0 R /MediaBox [0 0 612 792] /Contents %d 0 R /Resources %s >>" % (pages, stream, resources))
    pdf.set(pages, b"<< /Type /Pages /Kids [%d 0 R] /Count 1 >>" % page)
    return pdf.build(pdf.add(b"<< /Type /Catalog /Pages %d 0 R >>" % pages))


HELV = b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"


def handmade_pdfs() -> list[tuple[str, str, bytes]]:
    out: list[tuple[str, str, bytes]] = []
    add = lambda name, data: out.append((name, "pdf", data))
    # kerning arrays: gaps below, at and above the space threshold decide where a space is inserted
    helv = b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"
    res = b"<< /Font << /F1 %s >> >>" % helv
    add("pdf-tj-kerning-spaces", _one_page(b"BT /F1 12 Tf 50 700 Td [(The) -150 (Lord) -250 (is) -400 (my) 20 (shep) -30 (herd) -1000 (I shall not) 300 (want)] TJ ET\n", res))
    add("pdf-quote-operators", _one_page(b"BT /F1 12 Tf 14 TL 50 700 Td (first line) Tj (second line via quote) ' 2 1 (third line via double quote) \" T* (after T*) Tj ET\n", res))
    add("pdf-td-leading", _one_page(b"BT /F1 12 Tf 50 700 Td (one) Tj 0 -14 TD (two) Tj T* (three) Tj 0 -30 Td (four, a bigger gap) Tj ET\n", res))
    add("pdf-rotated-matrices", _one_page(b"BT /F1 12 Tf 1 0 0 1 50 700 Tm (upright) Tj ET BT /F1 12 Tf 0 1 -1 0 300 200 Tm (turned left) Tj 0 -14 Td (second turned line) Tj ET BT /F1 12 Tf -1 0 0 -1 400 600 Tm (upside down) Tj ET BT /F1 12 Tf 0 -1 1 0 100 700 Tm (turned right) Tj ET\n", res))
    add("pdf-cm-and-q", _one_page(b"q 1 0 0 1 100 100 cm BT /F1 10 Tf 0 0 Td (inside q) Tj ET Q BT /F1 10 Tf 50 50 Td (after Q) Tj ET Q Q BT /F1 10 Tf 50 30 Td (stray Q first) Tj ET\n", res))
    add("pdf-unknown-font-name", _one_page(b"BT /Nope 12 Tf 50 700 Td (text in a font that is not there) Tj ET BT /F1 12 Tf 50 680 Td (known font again) Tj ET\n", res))
    add("pdf-no-font-resources", _one_page(b"BT /F1 12 Tf 50 700 Td (no resources at all) Tj ET\n", b"<< >>"))
    add("pdf-text-without-bt", _one_page(b"/F1 12 Tf 50 700 Td (Td and Tj outside BT) Tj\n", res))
    add("pdf-escapes-in-strings", _one_page(b"BT /F1 12 Tf 50 700 Td (parens \\(nested\\) (balanced) back\\\\slash \\101\\102\\60 octal \\\nline continued tab\\there) Tj ET\n", res))
    add("pdf-hex-strings", _one_page(b"BT /F1 12 Tf 50 700 Td <48656C6C6F20776F726C64> Tj <4> Tj [<48 65> -300 <6C6C6F>] TJ ET\n", res))
    # fonts
    diffs = b"<< /Type /Font /Subtype /Type1 /BaseFont /Palatino-Roman /Encoding << /Type /Encoding /BaseEncoding /MacRomanEncoding /Differences [32 /space /fi /fl 65 /Euro /g17 /uni0041 /Aacute 200 /quoteright /bullet /unknownglyph] >> >>"
    add("pdf-font-differences", _one_page(b"BT /F1 12 Tf 50 700 Td (AB CD\\310\\311\\312 !\\042) Tj ET\n", b"<< /Font << /F1 %s >> >>" % diffs))
    add("pdf-font-standard-no-encoding", _one_page(b"BT /F1 12 Tf 50 700 Td (Times without an encoding \\351\\222) Tj ET BT /F2 12 Tf 50 680 Td (abcdefg Symbol) Tj ET\n", b"<< /Font << /F1 << /Type /Font /Subtype /Type1 /BaseFont /Times-Roman >> /F2 << /Type /Font /Subtype /Type1 /BaseFont /Symbol >> >> >>"))
    add("pdf-font-arial-alias-widths", _one_page(b"BT /F1 12 Tf 50 700 Td [(Arial) -250 (core) -250 (alias)] TJ ET\n", b"<< /Font << /F1 << /Type /Font /Subtype /TrueType /BaseFont /Arial,Bold /Encoding /WinAnsiEncoding >> >> >>"))
    add("pdf-font-widths-and-missing-width", _one_page(b"BT /F1 12 Tf 50 700 Td [(wide) -100 (spaces) -100 (here)] TJ ET\n", b"<< /Font << /F1 << /Type /Font /Subtype /TrueType /BaseFont /Foo /FirstChar 32 /LastChar 34 /Widths [800 300 300] /FontDescriptor << /Type /FontDescriptor /FontName /Foo /Flags 34 /MissingWidth 450 >> >> >> >>"))
    cmap1 = (b"/CIDInit /ProcSet findresource begin 12 dict begin begincmap\n1 begincodespacerange <00> <FF> endcodespacerange\n"
             b"3 beginbfchar <41> <0058> <42> <0059> <20> <0020> endbfchar\n1 beginbfrange <61> <63> <0041> endbfrange\nendcmap end end")
    simple = b"<< /Type /Font /Subtype /TrueType /BaseFont /ABCDEF+Sub /FirstChar 32 /LastChar 98 /Widths [250 %s] /Encoding /WinAnsiEncoding /ToUnicode 1 0 R >>" % b" ".join([b"600"] * 66)
    add("pdf-tounicode-simple-font", _one_page(b"BT /F1 12 Tf 50 700 Td (AB abc ABC) Tj ET\n", b"<< /Font << /F1 %s >> >>" % simple, (b"<< /Length %d >>\nstream\n" % len(cmap1) + cmap1 + b"\nendstream",)))
    cmap2 = (b"/CIDInit /ProcSet findresource begin 12 dict begin begincmap\n1 begincodespacerange <0000> <FFFF> endcodespacerange\n"
             b"4 beginbfchar <0001> <0054> <0002> <0068> <0003> <0020> <0004> <FB01> endbfchar\n"
             b"2 beginbfrange <0005> <0007> <0061> <0010> <0012> [<0041> <00420043> <0044>] endbfrange\nendcmap end end")
    cid = (b"<< /Type /Font /Subtype /Type0 /BaseFont /ABCDEF+Cid /Encoding /Identity-H /DescendantFonts [<< /Type /Font /Subtype /CIDFontType2 /BaseFont /ABCDEF+Cid /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> /DW 1000 /W [1 [600 700] 3 4 350 5 [500 510.5]] /FontDescriptor << /Type /FontDescriptor /FontName /ABCDEF+Cid /Flags 4 >> >>] /ToUnicode 1 0 R >>")
    add("pdf-type0-identity-h", _one_page(b"BT /F1 12 Tf 50 700 Td <0001000200030004000500060007> Tj 0 -14 Td <001000110012> Tj 0 -14 Td [<00010002> -300 <0004000500030001>] TJ ET\n", b"<< /Font << /F1 %s >> >>" % cid, (b"<< /Length %d >>\nstream\n" % len(cmap2) + cmap2 + b"\nendstream",)))
    cmap3 = (b"/CIDInit /ProcSet findresource begin 12 dict begin begincmap\n1 begincodespacerange <0000> <FFFF> endcodespacerange\n"
             b"5 beginbfchar <0001> <05E9> <0002> <05DC> <0003> <05D5> <0004> <05DD> <0005> <0020> endbfchar\nendcmap end end")
    add("pdf-rtl-hebrew-tounicode", _one_page(b"BT /F1 12 Tf 50 700 Td <0001000200030004> Tj 0 -14 Td <000100050002> Tj ET\n", b"<< /Font << /F1 %s >> >>" % cid, (b"<< /Length %d >>\nstream\n" % len(cmap3) + cmap3 + b"\nendstream",)))
    t3 = (b"<< /Type /Font /Subtype /Type3 /FontBBox [0 0 500 700] /FontMatrix [0.001 0 0 0.001 0 0] /CharProcs << /a 1 0 R /b 1 0 R >> /Encoding << /Type /Encoding /Differences [97 /a /b] >> /FirstChar 97 /LastChar 98 /Widths [500 600] >>")
    add("pdf-type3-font", _one_page(b"BT /F1 12 Tf 50 700 Td (abba ab) Tj ET\n", b"<< /Font << /F1 %s >> >>" % t3, (b"<< /Length 30 >>\nstream\n0 0 500 700 d1 0 0 500 700 re f\nendstream",)))
    # Type0 fonts with a predefined CMap and no /ToUnicode: pypdf decodes the string with the CMap's Python codec (and falls back to Latin-1)
    def cjk(cmap: bytes, data: bytes) -> bytes:
        font = (b"<< /Type /Font /Subtype /Type0 /BaseFont /Sample-%s /Encoding /%s /DescendantFonts [<< /Type /Font /Subtype /CIDFontType0 /BaseFont /Sample "
                b"/CIDSystemInfo << /Registry (Adobe) /Ordering (Japan1) /Supplement 2 >> /DW 1000 >>] >>" % (cmap, cmap))
        return _one_page(b"BT /F1 12 Tf 50 700 Td <" + data.hex().encode() + b"> Tj 0 -14 Td (ascii after) Tj ET\n", b"<< /Font << /F1 %s >> >>" % font)
    add("pdf-cmap-90ms-rksj", cjk(b"90ms-RKSJ-H", "こんにちは John 3:16".encode("cp932")))
    add("pdf-cmap-gbk-euc", cjk(b"GBK-EUC-H", "你好世界 abc".encode("gbk")))
    add("pdf-cmap-eten-b5", cjk(b"ETen-B5-H", "你好世界 abc".encode("cp950")))
    add("pdf-cmap-gbk2k", cjk(b"GBK2K-H", "你好世界 \U00020000".encode("gb18030")))
    add("pdf-cmap-unigb-utf16", cjk(b"UniGB-UTF16-H", "你好".encode("utf-16-be")))
    add("pdf-cmap-ucs2", cjk(b"UniJIS-UCS2-H", "こんにちは".encode("utf-16-be")))
    add("pdf-cmap-invalid-bytes-fall-back", cjk(b"90ms-RKSJ-H", b"\x82\xb1\x81 \xff abc"))
    add("pdf-cmap-unknown-name", cjk(b"Some-Unknown-H", b"\x41\x42\x43"))
    # xobjects
    form = b"BT /F1 10 Tf 0 0 Td (text in a form) Tj ET"
    add("pdf-handmade-form-xobject", _one_page(b"BT /F1 12 Tf 50 700 Td (before) Tj ET q 1 0 0 1 50 650 cm /Fm1 Do Q BT /F1 12 Tf 50 600 Td (after) Tj ET\n", b"<< /Font << /F1 %s >> /XObject << /Fm1 1 0 R >> >>" % helv, (b"<< /Type /XObject /Subtype /Form /BBox [0 0 600 800] /Resources << /Font << /F1 %s >> >> /Length %d >>\nstream\n" % (helv, len(form)) + form + b"\nendstream",)))
    add("pdf-image-xobject-and-inline-image", _one_page(b"BT /F1 12 Tf 50 700 Td (above) Tj ET q 100 0 0 100 50 500 cm /Im1 Do Q q 20 0 0 20 50 450 cm BI /W 2 /H 2 /BPC 8 /CS /G ID abcd EI Q BT /F1 12 Tf 50 400 Td (below) Tj ET\n", b"<< /Font << /F1 %s >> /XObject << /Im1 1 0 R >> >>" % helv, (b"<< /Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 /Length 1 >>\nstream\n\x00\nendstream",)))
    # filters on the content stream
    pdf = Pdf()
    raw = b"BT /F1 12 Tf 50 700 Td (Flate and ASCII85 and hex) Tj ET\n"
    import base64
    a85 = base64.a85encode(zlib.compress(raw), adobe=True)
    hexed = zlib.compress(raw).hex().encode() + b">"
    s1 = pdf.stream(a85, b"/Filter [/ASCII85Decode /FlateDecode]")
    s2 = pdf.stream(hexed, b"/Filter [/ASCIIHexDecode /FlateDecode]")
    s3 = pdf.stream(b"BT /F1 12 Tf 50 650 Td (uncompressed second stream) Tj ET\n")
    pages = pdf.reserve()
    page = pdf.add(b"<< /Type /Page /Parent %d 0 R /MediaBox [0 0 612 792] /Contents [%d 0 R %d 0 R %d 0 R] /Resources %s >>" % (pages, s1, s2, s3, res))
    pdf.set(pages, b"<< /Type /Pages /Kids [%d 0 R] /Count 1 >>" % page)
    add("pdf-content-filters-and-array", pdf.build(pdf.add(b"<< /Type /Catalog /Pages %d 0 R >>" % pages)))
    # an operator split over two streams of the /Contents array, and comments in the content
    pdf = Pdf()
    s1 = pdf.stream(b"BT /F1 12 Tf 50 700 Td (first) Tj % a comment (with a paren\n(second) Tj")
    s2 = pdf.stream(b"ET BT /F1 12 Tf 50 680 Td (third) Tj ET")
    pages = pdf.reserve()
    page = pdf.add(b"<< /Type /Page /Parent %d 0 R /MediaBox [0 0 612 792] /Contents [%d 0 R %d 0 R] /Resources %s >>" % (pages, s1, s2, res))
    pdf.set(pages, b"<< /Type /Pages /Kids [%d 0 R] /Count 1 >>" % page)
    add("pdf-contents-array-and-comments", pdf.build(pdf.add(b"<< /Type /Catalog /Pages %d 0 R >>" % pages)))
    # page tree: nested /Pages, resources inherited, a page without text in the middle
    pdf = Pdf()
    f1 = pdf.add(helv)
    c1 = pdf.stream(b"BT /F1 12 Tf 50 700 Td (page one) Tj ET")
    c2 = pdf.stream(b"0 0 100 100 re f")
    c3 = pdf.stream(b"BT /F1 12 Tf 50 700 Td (page three) Tj ET")
    root_pages = pdf.reserve(); inner = pdf.reserve()
    p1 = pdf.add(b"<< /Type /Page /Parent %d 0 R /Contents %d 0 R >>" % (root_pages, c1))
    p2 = pdf.add(b"<< /Type /Page /Parent %d 0 R /Contents %d 0 R >>" % (inner, c2))
    p3 = pdf.add(b"<< /Type /Page /Parent %d 0 R /Contents %d 0 R >>" % (inner, c3))
    pdf.set(inner, b"<< /Type /Pages /Parent %d 0 R /Kids [%d 0 R %d 0 R] /Count 2 >>" % (root_pages, p2, p3))
    pdf.set(root_pages, b"<< /Type /Pages /Kids [%d 0 R %d 0 R] /Count 3 /MediaBox [0 0 612 792] /Resources << /Font << /F1 %d 0 R >> >> >>" % (p1, inner, f1))
    add("pdf-page-tree-inherited-resources", pdf.build(pdf.add(b"<< /Type /Catalog /Pages %d 0 R >>" % root_pages)))
    # not a readable PDF
    add("pdf-error-empty-file", b"")
    add("pdf-error-not-a-pdf", b"This is a text file renamed to .pdf\nJohn 3:16\n")
    add("pdf-error-header-only", b"%PDF-1.4\n")
    add("pdf-error-no-text-layer", _one_page(b"0.5 g 50 50 200 200 re f\n", res))
    add("pdf-error-only-spaces", _one_page(b"BT /F1 12 Tf 50 700 Td (    ) Tj 0 -14 Td ( ) Tj ET\n", res))
    return out
