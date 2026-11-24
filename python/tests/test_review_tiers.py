"""Every decision is graded: good (no guesses), warning (a guess-like rule was used), error (could not be decided)."""
from __future__ import annotations

import asyncio
from pathlib import Path

from versevision.config import AppConfig
from versevision.workflow.notes_workflow import plan_notes
from versevision.workflow.report import build_report

WESLEY2 = (Path(__file__).parent / "fixtures" / "notes" / "wesley_2026_10_18_part2.txt").read_text()


def plan(text: str, default: str | None = "NKJV"):
    return asyncio.run(plan_notes(text, AppConfig(), default_version=default, no_fetch=True))


def tiers(result):
    return {d.line: d.tier for d in result.decisions}


def test_a_clean_verse_and_point_are_good():
    r = plan("1) Faith\nJohn 3:16 NIV\n")
    assert [d.tier for d in r.decisions] == ["good", "good"]


def test_a_version_taken_from_above_is_a_warning_with_the_reason():
    r = plan("John 3:16 NIV\nRomans 8:28\n")
    assert tiers(r) == {1: "good", 2: "warning"}
    assert "nearest version above" in r.decisions[1].reasons[0]


def test_the_default_version_is_a_warning_too():
    r = plan("Romans 8:28\n")
    assert r.decisions[0].tier == "warning" and "default version" in r.decisions[0].reasons[0]


def test_a_line_that_cannot_be_decided_is_an_error_and_adds_nothing():
    r = plan('1) Point\n"A quote with no reference at all that the notes never explain."\n')
    errors = [d for d in r.decisions if d.tier == "error"]
    assert len(errors) == 1 and errors[0].slide_count == 0 and r.point_count == 1


def test_a_nonexistent_verse_is_an_error():
    r = plan("John 3:99 NIV\n")
    assert any(d.tier == "error" for d in r.decisions) and r.verse_count == 0


def test_a_short_line_with_no_punctuation_is_a_warning_because_it_may_be_a_heading():
    r = plan("1) A point\nOpen with prayer\n")
    assert [d.tier for d in r.decisions] == ["good", "warning"]


def test_stage_cues_are_left_out_firmly_and_a_possible_heading_is_a_warning():
    r = plan("1) A point\nShow slide of the church.\n")
    cue = next(d for d in r.decisions if d.kind == "left_out")
    assert cue.tier == "good"


def test_each_decision_knows_which_slides_it_made():
    r = plan("1) A point\nJohn 3:16 NIV\nPsalm 23:1-2 NIV\n")
    verse_slides = [d.slides if hasattr(d, "slides") else (d.first_slide, d.slide_count) for d in r.decisions]
    assert [d.slide_count for d in r.decisions] == [1, 1, 2] and r.decisions[2].first_slide == 2


def test_the_report_orders_errors_then_warnings_then_good():
    r = plan('1) Point\nJohn 3:16 NIV\nRomans 8:28\n"Orphan quote with no reference anywhere near it in the notes."\n')
    report = build_report(r)
    assert report["summary"]["errors"] == 1 and report["summary"]["warnings"] == 1 and report["summary"]["good"] == 2
    assert set(report["review"]) >= {"error", "warning", "good"}


def test_wesley_part_two_matches_the_slide_list_hunter_approved():
    r = plan(WESLEY2, default="NKJV")
    titles = [d.title for d in r.decisions if d.kind in ("point", "verse")]
    assert r.verse_count == 9 and r.point_count == 31
    assert not [d for d in r.decisions if d.tier == "error"]
    assert titles[0].startswith("1) A Person Must Realize")
    assert [d.text for d in r.decisions if d.kind == "point"][-1].startswith("• May our lives, like Wesley")
