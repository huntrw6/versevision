"""Name a new presentation after the document it came from."""
from __future__ import annotations

import re
from pathlib import Path

from versevision.references import parse_references

_EMAIL_HEADER = re.compile(r"^\s*(from|to|cc|bcc|subject|date|sent)\s*:|<[^>\s]+@[^>\s]+>|@|^\[quoted text hidden\]$", re.IGNORECASE)
_DATE = re.compile(r"\d{1,2}/\d{1,2}/\d{2,4}")
_NOT_A_TITLE = re.compile(
    r"^\s*(\[\s*slide\s*\]|#\s*\d|\d{1,3}\s*[).:]|[a-zA-Z]\s*[).]\s|[-•▪◦●*]\s|show\b|play\b|start\b|when i\b|note\b|todo\b|recap\b|https?://)",
    re.IGNORECASE,
)
_ILLEGAL = re.compile(r'[\\/:*?"<>|\x00-\x1f]')
MAX_NAME = 100


def _plausible(line: str) -> bool:
    line = line.strip()
    if not (3 <= len(line) <= 100) or line.endswith((".", "?", "!", "”", '"')):
        return False
    if len(line.split()) > 14 or line.startswith(("“", '"', "‘")):
        return False
    if _NOT_A_TITLE.match(line):
        return False
    if _DATE.search(line):
        return True
    refs = parse_references(line).references
    return not (refs and refs[0].source_start <= 2)


def safe_presentation_name(title: str, fallback: str = "Sermon Notes") -> str:
    """A name that is safe as a file name on macOS, Windows and Linux (and shows well in ProPresenter)."""
    name = title.replace("\xa0", " ").replace("/", "-").replace("\\", "-").replace(":", " -")
    name = _ILLEGAL.sub("", name)
    name = re.sub(r"\s+", " ", name).strip(" .-")
    name = re.sub(r"\s+-\s+-\s+", " - ", name)
    if len(name) > MAX_NAME:
        name = name[:MAX_NAME].rsplit(" ", 1)[0].strip(" .-") or name[:MAX_NAME]
    return name or fallback


def title_from_filename(filename: str) -> str:
    stem = Path(filename).stem if filename and filename != "text" else ""
    stem = re.sub(r"[_]+", " ", stem)
    stem = re.sub(r"\s+", " ", stem).strip()
    return stem


def document_title(text: str, filename: str = "") -> str:
    """The first line of the document when it reads like a title, otherwise the file name."""
    for raw in text.replace("\xa0", " ").splitlines():
        line = raw.strip()
        if not line:
            continue
        if _EMAIL_HEADER.search(line):
            continue                      # skip email From/To/Cc lines and keep looking
        if _plausible(line):
            return safe_presentation_name(line)
        break                             # the first real line is not a title: use the file name
    return safe_presentation_name(title_from_filename(filename) or "Sermon Notes")


def unique_name(name: str, existing: set[str]) -> str:
    """'Name', then 'Name (2)', 'Name (3)'... so an existing presentation is never overwritten."""
    taken = {item.casefold() for item in existing}
    if name.casefold() not in taken:
        return name
    counter = 2
    while f"{name} ({counter})".casefold() in taken:
        counter += 1
    return f"{name} ({counter})"
