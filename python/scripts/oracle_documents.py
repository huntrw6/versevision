"""The oracle of the document readers (S4): what ``versevision.documents.extract_text`` returns for a corpus of files.

Written by ``export_oracle.py`` into ``oracle/documents/<case>/`` (never by hand), one folder per file::

    input.<ext>                the document
    expected/extracted.txt     the text the reader returned            (when it returned)
    expected/extract.json      the ``extract`` record of the run trace  (when it returned)
    expected/error.json        {"kind", "message"}                      (when it raised; kind is ``document`` for a VerseVisionError,
                               ``unexpected`` for any other exception, which the web page shows as ``Unexpected problem: <message>``)

The inputs: HTML (well formed and malformed), Word files (runs, tabs, breaks, hyperlinks, tables, headers, tracked changes, ...), text
and Markdown files, PDFs (``python/tests/fixtures/pdf``: made with ReportLab, plus the generated and hand-made ones of
``oracle_pdf_cases.py``) and files that cannot be read. The reader is run on a file called ``upload.<ext>`` like the web layer's
upload, so the one message that contains the path of the file (python-docx: ``file '<path>' is not a Word file``) is reproducible.
"""
from __future__ import annotations

import json
import logging
import shutil
import tempfile
from pathlib import Path

logging.getLogger("pypdf").setLevel(logging.CRITICAL)
from oracle_doc_cases import docx_cases, error_cases, html_cases, text_cases
from oracle_pdf_cases import generated_pdfs, handmade_pdfs
from versevision.documents.base import extract_text, last_extract
from versevision.errors import VerseVisionError


def fixture_pdfs(repo_root: Path) -> list[tuple[str, str, bytes]]:
    folder = repo_root / "python" / "tests" / "fixtures" / "pdf"
    return [(f"pdf-{path.stem}", "pdf", path.read_bytes()) for path in sorted(folder.glob("*.pdf"))]


def all_document_cases(repo_root: Path) -> list[tuple[str, str, bytes]]:
    cases = html_cases() + docx_cases() + text_cases() + error_cases() + fixture_pdfs(repo_root) + generated_pdfs() + handmade_pdfs()
    names = [name for name, _, _ in cases]
    assert len(names) == len(set(names)), "duplicate document case names"
    return cases


def dumps(value) -> str:
    return json.dumps(value, ensure_ascii=False, indent=2, sort_keys=True) + "\n"


def run_document_case(ext: str, data: bytes) -> dict[str, str]:
    folder = Path(tempfile.mkdtemp(prefix="oracle-doc-"))
    try:
        path = folder / f"upload.{ext}"
        path.write_bytes(data)
        try:
            text = extract_text(path)
        except Exception as exc:
            message = str(exc).replace(str(path), f"upload.{ext}")
            assert str(folder) not in message, message
            kind = "document" if isinstance(exc, VerseVisionError) else "unexpected"
            return {"expected/error.json": dumps({"kind": kind, "message": message})}
        return {"expected/extracted.txt": text, "expected/extract.json": dumps(last_extract())}
    finally:
        shutil.rmtree(folder, ignore_errors=True)


def documents_oracle(out: Path, repo_root: Path) -> dict[str, int]:
    """Writes ``out/documents`` and returns how many cases it holds per input type and outcome."""
    target = out / "documents"
    if target.exists():
        shutil.rmtree(target)
    counts: dict[str, int] = {}
    for name, ext, data in all_document_cases(repo_root):
        files = {f"input.{ext}": data, **run_document_case(ext, data)}
        for relative, content in files.items():
            file = target / name / relative
            file.parent.mkdir(parents=True, exist_ok=True)
            file.write_bytes(content if isinstance(content, bytes) else content.encode("utf-8"))
        outcome = "error" if "expected/error.json" in files else "ok"
        key = f"{ext} {outcome}"
        counts[key] = counts.get(key, 0) + 1
    return dict(sorted(counts.items()))
