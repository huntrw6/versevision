"""Headings are slides without bullets, "KEY #n)" is a point header, and a quoted verse tells which Bible version the notes use."""
from __future__ import annotations

import asyncio
from pathlib import Path

from versevision.bibles.models import BibleVerse, ResolvedPassage
from versevision.config import AppConfig, NotesConfig, PointsConfig
from versevision.notes import classify_document
from versevision.workflow.notes_workflow import plan_notes

NOTES = (Path(__file__).parent / "fixtures" / "whitefield_notes.txt").read_text(encoding="utf-8")


def slides(**notes):
    cfg = AppConfig(notes=NotesConfig(**notes))
    plan = asyncio.run(plan_notes(NOTES, cfg, default_version="NKJV", no_fetch=True))
    return plan, [s.body_text for s in plan.slide_plan.slides if s.kind == "point"]


def test_key_headers_are_the_points_header_so_they_get_no_bullet():
    plan, points = slides()
    assert "KEY #1) SPEAK THE BORN AGAIN MESSAGE".replace("#1", "#1") in [p.replace("KEY#1)", "KEY #1)") for p in points]
    assert "KEY #2) PREACHING MUST CONFRONT SIN AND POINT PEOPLE TO THE SAVIOR" in points
    assert not any(p.startswith("•") and "KEY" in p for p in points)
    keys = [p for p in points if p.replace(" ", "").startswith("KEY#")]
    assert len(keys) == 12                       # six in the notes, six again in the recap
    decision = next(d for d in plan.decisions if d.title.startswith("KEY #2"))
    assert decision.tier == "good"               # no more "short line made a bullet; may be a heading" guess


def test_keyed_numbering_can_be_stripped_or_renumbered():
    cfg = AppConfig(points=PointsConfig(numbering="strip"))
    plan = asyncio.run(plan_notes(NOTES, cfg, default_version="NKJV", no_fetch=True))
    assert "SPEAK THE BORN AGAIN MESSAGE" in [s.body_text for s in plan.slide_plan.slides]
    cfg = AppConfig(points=PointsConfig(numbering="renumber"))
    plan = asyncio.run(plan_notes(NOTES, cfg, default_version="NKJV", no_fetch=True))
    assert "KEY #2) PREACHING MUST CONFRONT SIN AND POINT PEOPLE TO THE SAVIOR" in [s.body_text for s in plan.slide_plan.slides]


def test_titles_and_headings_are_slides_with_no_bullet_by_default():
    plan, points = slides()
    assert "6 KEYS TO A NEW AWAKENING" in points
    assert "RECAP: KEYS TO A NEW AWAKENING" in points
    assert "A SPIRITUAL AWAKENING CAN UNITE PEOPLE WHO HAVE BEEN DIVIDED" in points
    assert next(d for d in plan.decisions if d.title == "6 KEYS TO A NEW AWAKENING").tier == "good"
    # the date/speaker line and the photo cues stay out (and can still be ticked in the review)
    left = {d.line for d in plan.decisions if d.kind == "left_out"}
    assert 1 in left and 3 in left and 28 in left and 29 in left and 2 not in left and 99 not in left


def test_headings_can_be_switched_back_to_left_out():
    plan, points = slides(headings="skip", caps_lines="line")
    assert "6 KEYS TO A NEW AWAKENING" not in points and "RECAP: KEYS TO A NEW AWAKENING" not in points
    left = {d.line for d in plan.decisions if d.kind == "left_out"}
    assert 2 in left and 99 in left


# ------------------------------------------------------------------------------------------ version from the quoted text
KJV = {"Ephesians 2:8": "For by grace are ye saved through faith; and that not of yourselves: it is the gift of God: Not of works, lest any man should boast.",
       "John 3:3": "Jesus answered and said unto him, Verily, verily, I say unto thee, Except a man be born again, he cannot see the kingdom of God."}
NKJV = {"Ephesians 2:8": "For by grace you have been saved through faith, and that not of yourselves; it is the gift of God, not of works, lest anyone should boast.",
        "John 3:3": "Jesus answered and said to him, Most assuredly, I say to you, unless one is born again, he cannot see the kingdom of God."}


class FakeRegistry:
    def __init__(self):
        self.asked = []

    async def fetch(self, reference):
        version = reference.version or "NKJV"
        self.asked.append(version)
        table = {"KJV": KJV, "NKJV": NKJV}.get(version)
        if table is None:
            raise RuntimeError("no such version here")
        key = f"{reference.book_display_name} {reference.spans[0].start.chapter}:{reference.spans[0].start.verse}"
        v = reference.spans[0].start
        return ResolvedPassage(reference=reference, resolved_version=version,
                               verses=[BibleVerse(book_id=reference.book_id, chapter=v.chapter, verse=v.verse, text=table[key], version=version)])


DOC = '1) Faith\n"By grace are ye saved through faith; and that not of yourselves: it is the gift of God: Not of works, lest any man should boast" (Ephesians 2:8).\n' \
      '"Jesus answered and said to him, Most assuredly, I say to you, unless one is born again, he cannot see the kingdom of God."\nJohn 3:3\n'


def test_a_verse_quoted_in_another_versions_wording_uses_that_version():
    registry = FakeRegistry()
    plan = asyncio.run(plan_notes(DOC, AppConfig(), default_version="NKJV", provider_registry=registry))
    texts = {d.title: d for d in plan.decisions if d.kind == "verse"}
    eph = next(d for t, d in texts.items() if t.startswith("Ephesians"))
    assert "(KJV)" in eph.title and eph.tier == "warning" and "matches KJV, not NKJV" in eph.reasons[0] and len(eph.reasons) == 1
    verse_slides = [s for s in plan.slide_plan.slides if s.kind == "verse"]
    assert verse_slides[0].version_text == "KJV" and "ye saved" in verse_slides[0].body_text
    # the NKJV quote is left on NKJV, and no other version is even asked about
    john = next(d for t, d in texts.items() if t.startswith("John"))
    assert not any("KJV" in r and "matches" in r for r in john.reasons) and "NKJV" in [s.version_text for s in verse_slides]


def test_the_quote_check_can_be_turned_off_and_never_overrides_a_written_version():
    from versevision.config import VersesConfig
    registry = FakeRegistry()
    plan = asyncio.run(plan_notes(DOC, AppConfig(verses=VersesConfig(version_from_quote="off")), default_version="NKJV", provider_registry=registry))
    assert {s.version_text for s in plan.slide_plan.slides if s.kind == "verse"} == {"NKJV"}
    registry = FakeRegistry()
    written = DOC.replace("(Ephesians 2:8).", "(Ephesians 2:8 NKJV).")
    plan = asyncio.run(plan_notes(written, AppConfig(), default_version="NKJV", provider_registry=registry))
    assert "KJV" not in registry.asked and {s.version_text for s in plan.slide_plan.slides if s.kind == "verse"} == {"NKJV"}
