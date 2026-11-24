"""Writes crates/versevision-docs/src/pdf_data.txt: the pypdf tables the Rust PDF reader needs (Adobe glyph names, the six
built-in encodings, the widths of the 14 core fonts). Run with the locked environment: ``uv run python scripts/gen_pdf_data.py``.

Lines:  ``G <name> <cp>[,<cp>]``  glyph name (with its slash) -> code points (hex);
        ``E <encoding> <256 entries>``  each entry is its code points (hex) joined by ``,``;
        ``F <font name> <flags> <default width or -> <char>=<width> ...``  (``<char>`` is the code point in hex).
"""
from __future__ import annotations

import sys
from pathlib import Path

from pypdf._codecs import adobe_glyphs, charset_encoding
from pypdf._codecs.core_font_metrics import CORE_FONT_METRICS


def cps(text: str) -> str:
    return ",".join(f"{ord(c):x}" for c in text)


def main() -> None:
    out = Path(__file__).resolve().parents[2] / "crates" / "versevision-docs" / "src" / "pdf_data.txt"
    lines: list[str] = []
    for name in sorted(adobe_glyphs):
        lines.append(f"G {name} {cps(adobe_glyphs[name])}")
    for name in sorted(charset_encoding):
        lines.append(f"E {name} " + " ".join(cps(c) for c in charset_encoding[name]))
    for name in sorted(CORE_FONT_METRICS):
        metrics = CORE_FONT_METRICS[name]
        widths = dict(metrics.character_widths)
        default = widths.pop("default", None)
        parts = [f"{ord(k):x}={v}" for k, v in sorted(widths.items())]
        lines.append(f"F {name} {metrics.font_descriptor.flags} {'-' if default is None else default} " + " ".join(parts))
    out.write_text("\n".join(lines) + "\n", encoding="utf-8")
    print(f"{out}: {len(lines)} lines, {out.stat().st_size} bytes", file=sys.stderr)


if __name__ == "__main__":
    main()
