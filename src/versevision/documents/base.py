from pathlib import Path

from versevision.errors import DocumentError


def extract_text(path: Path) -> str:
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

