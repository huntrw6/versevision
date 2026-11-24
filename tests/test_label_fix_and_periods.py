import asyncio
from pathlib import Path

from versevision.config import AppConfig
from versevision.workflow.notes_workflow import _strip_period, apply_review_choices, plan_notes
from versevision.workflow.report import build_report

NOTES = """Title 11/8/26

KEY#1) SPEAK THE BORN AGAIN MESSAGE
KEY #2) PREACHING MUST CONFRONT SIN.
KEY #3) DON'T BE THE ALMOST CHRISTIAN!
KEY #4) WHAT IS REVIVAL?
"""


def _plan(notes=NOTES, **points):
    config = AppConfig()
    for key, value in points.items():
        setattr(config.points, key, value)
    return asyncio.run(plan_notes(notes, config, default_version="NKJV", no_fetch=True))


def test_a_header_written_unlike_the_others_is_corrected_and_raised_as_an_error_to_approve():
    result = _plan()
    texts = [s.body_text for s in result.slide_plan.slides if s.kind == "point"]
    assert texts[0].startswith("KEY #1)")                      # the fixed spelling is used
    fixed = [d for d in result.decisions if d.correction]
    assert len(fixed) == 1 and fixed[0].tier == "error"
    assert fixed[0].correction["from"] == "KEY#1)" and fixed[0].correction["to"] == "KEY #1)"
    assert "3 of 4" in fixed[0].reasons[0]
    report = build_report(result)
    assert report["summary"]["fixes"] == 1 and report["summary"]["errors"] >= 1
    assert any(row["status"] == "needs_approval" for row in report["flagged"])          # the CLI stops until --yes
    assert report["decisions"][fixed[0].id]["correction"] == {"from": "KEY#1)", "to": "KEY #1)"}


def test_an_unapproved_fix_goes_in_as_written():
    result = _plan()
    d = next(d for d in result.decisions if d.correction)
    plan = apply_review_choices(result, set(), set(), {d.id})
    assert [s.body_text for s in plan.slides if s.kind == "point"][0].startswith("KEY#1)")
    assert [s.body_text for s in plan.slides if s.kind == "point"][1].startswith("KEY #2)")


def test_the_fix_can_be_switched_off_and_needs_a_clear_majority():
    assert not any(d.correction for d in _plan(fix_label_typos=False).decisions)
    two = "Title 11/8/26\n\nKEY#1) ONE\nKEY #2) TWO\n"
    assert not any(d.correction for d in _plan(two).decisions)                                  # too few to know which is right
    tie = "Title 11/8/26\n\nKEY#1) ONE\nKEY #2) TWO\nKEY#3) THREE\nKEY #4) FOUR\n"
    assert not any(d.correction for d in _plan(tie).decisions)                                  # no majority


def test_a_trailing_period_is_removed_but_other_endings_stay():
    assert _strip_period("Pray without ceasing.") == "Pray without ceasing"
    assert _strip_period("Is this the way?") == "Is this the way?"
    assert _strip_period("Praise Him!") == "Praise Him!"
    assert _strip_period("Wait...") == "Wait..."
    assert _strip_period("He said, \u201cBe still.\u201d") == "He said, \u201cBe still\u201d"
    assert _strip_period("Love God, love people, etc.") == "Love God, love people, etc."
    assert _strip_period("Made in the U.S.") == "Made in the U.S."
    assert _strip_period("God is good. He is great.") == "God is good. He is great"             # only the closing one
    texts = [s.body_text for s in _plan().slide_plan.slides if s.kind == "point"]
    assert not any(t.rstrip().endswith(".") for t in texts) and any(t.rstrip().endswith("!") for t in texts) and any(t.rstrip().endswith("?") for t in texts)
    kept = [s.body_text for s in _plan(strip_trailing_period=False).slide_plan.slides if s.kind == "point"]
    assert any(t.rstrip().endswith(".") for t in kept)


def test_the_whitefield_notes_get_the_missing_space_fixed_once_and_for_the_recap_too():
    src = Path(__file__).parent.joinpath("fixtures", "whitefield_notes.txt").read_text(encoding="utf-8")
    result = _plan(src)
    assert sum(1 for d in result.decisions if d.correction) >= 1
    assert not any("KEY#" in s.body_text for s in result.slide_plan.slides)
