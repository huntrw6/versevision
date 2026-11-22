from pathlib import Path

from versevision import runlog
from versevision.errors import DocumentError


_LIGATURES = {0xFB00: "ff", 0xFB01: "fi", 0xFB02: "fl", 0xFB03: "ffi", 0xFB04: "ffl", 0xFB05: "st", 0xFB06: "st"}   # PDFs store "fi" as one character


def extract_text(path: Path) -> str:
    """Read a document as text. How it was read is recorded in the run's trace: this is the first place a document can go wrong."""
    text = _extract(path).translate(_LIGATURES)
    suffix = path.suffix.casefold()
    lines = text.splitlines()
    info = dict(type=suffix, method={".txt": "plain text", ".md": "plain text", ".html": "BeautifulSoup get_text (one line per block)", ".htm": "BeautifulSoup get_text",
                                                ".docx": "python-docx: one line per Word paragraph (tables, headers and text boxes are not read)",
                                                ".pdf": "pypdf extract_text (the text layer; lines are cut where the PDF cut them)"}.get(suffix, suffix),
                 characters=len(text), lines=len(lines), blank_lines=sum(1 for x in lines if not x.strip()),
                 odd_characters=sorted({c for c in text if ord(c) > 127 and not c.isalpha()})[:40])
    _LAST.clear()
    _LAST.update(info)
    runlog.trace("extract", **info)
    return text


_LAST: dict = {}


def last_extract() -> dict:
    """How the most recent document was read (the command line reads the file before its run log exists, then records this)."""
    return dict(_LAST)


def _extract(path: Path) -> str:
    suffix = path.suffix.casefold()
    if suffix in {".txt", ".md"}:
        return path.read_text(encoding="utf-8")
    if suffix in {".html", ".htm"}:
        from bs4 import BeautifulSoup
        return BeautifulSoup(path.read_text(encoding="utf-8"), "html.parser").get_text("\n")
    if suffix == ".docx":
        from docx import Document
        return "\n".join(paragraph.text for paragraph in Document(path).paragraphs)
    if suffix == ".pdf":
        from pypdf import PdfReader
        text = "\n".join(page.extract_text() or "" for page in PdfReader(path).pages)
        if not text.strip():
            raise DocumentError("No usable embedded text was found. OCR is not enabled.")
        return text
    raise DocumentError(f"Unsupported document type: {suffix or '(none)'}")

