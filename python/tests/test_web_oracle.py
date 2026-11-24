"""The oracle of the web app's data routes (S7a) is repeatable: regenerating ``oracle/web`` gives byte for byte what is committed, so the Rust
router is held to what the Python server does today. If the Python behaviour changes on purpose, regenerate with ``scripts/export_oracle.py``."""
from __future__ import annotations

import sys
from pathlib import Path

SCRIPTS = Path(__file__).resolve().parents[1] / "scripts"
REPO = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(SCRIPTS))

import oracle_web  # noqa: E402


def _files(folder: Path) -> dict[str, bytes]:
    return {p.relative_to(folder).as_posix(): p.read_bytes() for p in sorted(folder.rglob("*")) if p.is_file()}


def test_the_web_oracle_repeats_byte_for_byte(tmp_path):
    assert oracle_web.web_oracle(tmp_path, REPO) >= 40
    again, committed = _files(tmp_path / "web"), _files(REPO / "oracle" / "web")
    assert again.keys() == committed.keys()
    for key in committed:
        assert again[key] == committed[key], f"oracle/web/{key} differs from the committed oracle"
