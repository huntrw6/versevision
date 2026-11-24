#!/usr/bin/env python3
"""Generate crates/versevision-docs/src/html_data.txt: the two tables the Rust HTML reader needs.

    E <name> <hex>[,<hex>...]   bs4 EntitySubstitution.HTML_ENTITY_TO_CHARACTER (html.entities.html5 without the ';',
                                the first name in sorted(html5.items()) order wins, exactly as bs4 builds it)
    W <hex> <hex>               bs4 UnicodeDammit.WINDOWS_1252_TO_UTF8: numeric references 0x80-0x9f (the only range bs4 consults)

Run with the python/ virtualenv (needs bs4 only for the Windows-1252 table):
    python/.venv/bin/python python/scripts/gen_html_entities.py [OUT]
"""
import sys
from html.entities import html5
from pathlib import Path

from bs4.dammit import UnicodeDammit

out = Path(sys.argv[1]) if len(sys.argv) > 1 else Path(__file__).resolve().parents[2] / "crates/versevision-docs/src/html_data.txt"
names: dict[str, str] = {}
for key, character in sorted(html5.items()):
    name = key[:-1] if key.endswith(";") else key
    names.setdefault(name, character)
lines = [f"E {name} {','.join(f'{ord(c):x}' for c in names[name])}" for name in sorted(names)]
windows = {n: raw for n, raw in sorted(UnicodeDammit.WINDOWS_1252_TO_UTF8.items()) if 0x80 <= n <= 0x9F}  # the only range bs4 consults
for number, raw in windows.items():
    lines.append(f"W {number:x} {ord(raw.decode('utf8')):x}")
out.write_text("\n".join(lines) + "\n", encoding="utf-8")
print(f"{len(names)} entities, {len(windows)} windows-1252 entries -> {out}")
