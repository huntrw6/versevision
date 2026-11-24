"""The oracle of the ProPresenter side (S6b) is repeatable: every scenario of ``oracle/pipeline/scenarios`` is run again (against the scripted
ProPresenter, in virtual time, with seeded ids) and must give byte for byte what is committed. It also checks that the Python app, run that
way, still does what the Rust port is held to: if the Python behaviour changes on purpose, regenerate with ``scripts/export_oracle.py``."""
from __future__ import annotations

import sys
from pathlib import Path

import pytest

SCRIPTS = Path(__file__).resolve().parents[1] / "scripts"
REPO = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(SCRIPTS))

import oracle_pipeline  # noqa: E402

SCENARIOS = sorted(p.parent.name for p in (REPO / "oracle" / "pipeline" / "scenarios").glob("*/scenario.json"))


def _files(folder: Path) -> dict[str, bytes]:
    return {p.relative_to(folder).as_posix(): p.read_bytes() for p in sorted(folder.rglob("*")) if p.is_file()}


def test_there_are_scenarios():
    assert len(SCENARIOS) >= 25


@pytest.mark.parametrize("name", SCENARIOS)
def test_a_scenario_repeats_byte_for_byte(name, tmp_path):
    assert oracle_pipeline.pipeline_oracle(tmp_path, REPO, [name]) == 1
    again = _files(tmp_path / "pipeline" / "scenarios" / name / "expected")
    committed = _files(REPO / "oracle" / "pipeline" / "scenarios" / name / "expected")
    assert again.keys() == committed.keys()
    for key in committed:
        assert again[key] == committed[key], f"{name}/expected/{key} differs from the committed oracle"
