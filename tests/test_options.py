"""Every parsing and formatting option: the default is Hunter's style, and each alternative really changes the result."""
from __future__ import annotations

import asyncio

import pytest

from versevision.config import AppConfig, ConfigurationError, with_options
from versevision.notes.classifier import classify_document
from versevision.workflow.notes_workflow import plan_notes


def cfg(**sets) -> AppConfig:
    return with_options(AppConfig(), sets=[f"{k.replace('__', '.')}={v}" for k, v in sets.items()])


def run(text: str, config: AppConfig | None = None, default="NKJV"):
    return asyncio.run(plan_notes(text, config or AppConfig(), default_version=default, no_fetch=True))


def bodies(result) -> list[str]:
    return [s.body_text for s in result.slide_plan.slides if s.kind == "point"]


NOTES = "Title line here\n1) First point\nA full sentence under it.\nShort heading\n2) Second point\n"


# ------------------------------------------------------------------ notes: reading
def test_defaults_are_the_house_style():
    n = AppConfig().notes
    assert (n.line_breaks, n.unlabeled_sentence, n.unlabeled_short, n.before_first_item, n.stage_cues, n.headings, n.caps_lines) == ("slide", "bullet", "bullet", "skip", "skip", "slide", "heading")
    assert (AppConfig().points.numbering, AppConfig().points.bullet_char, AppConfig().verses.version_source) == ("keep", "•", "above")
    assert bodies(run(NOTES)) == ["Title line here", "1) First point", "• A full sentence under it.", "• Short heading", "2) Second point"]      # the title is a slide with no bullet


def test_paragraph_mode_joins_a_sentence_to_its_point():
    assert bodies(run(NOTES, cfg(notes__line_breaks="paragraph")))[1] == "1) First point\nA full sentence under it."


@pytest.mark.parametrize("mode,expected", [("review", 1), ("skip", 0)])
def test_unlabeled_sentence_review_or_skip(mode, expected):
    r = run("1) P\nA full sentence under it.\n", cfg(notes__unlabeled_sentence=mode))
    assert bodies(r) == ["1) P"] and len([d for d in r.decisions if d.tier == "error"]) == expected


def test_unlabeled_short_line_can_go_to_review():
    r = run("1) P\nShort heading\n", cfg(notes__unlabeled_short="review"))
    assert bodies(r) == ["1) P"] and [d.tier for d in r.decisions] == ["good", "error"]


@pytest.mark.parametrize("mode,slides", [("skip", 1), ("review", 0), ("bullet", 1)])      # skip: a heading slide with no bullet (notes.headings)
def test_lines_before_the_first_item(mode, slides):
    r = run("Pastor Hunter speaks today\n1) P\n", cfg(notes__before_first_item=mode))
    assert len(bodies(r)) - 1 == slides


def test_lines_before_the_first_item_can_be_left_out_with_headings_skip():
    r = run("Pastor Hunter speaks today\n1) P\n", cfg(notes__headings="skip"))
    assert bodies(r) == ["1) P"]


def test_stage_cues_skip_review_or_become_slides():
    text = "1) P\nShow slide of the cross.\n"
    assert bodies(run(text)) == ["1) P"]
    assert [d.tier for d in run(text, cfg(notes__stage_cues="review")).decisions] == ["good", "error"]
    assert bodies(run(text, cfg(notes__stage_cues="bullet")))[-1] == "• Show slide of the cross."


def test_extra_cue_words_and_skip_patterns():
    text = "1) P\nPause here for effect.\nSecret code 42\nKeep this sentence.\n"
    assert "• Pause here for effect." in bodies(run(text))
    r = run(text, cfg(notes__extra_cue_words='["pause"]', notes__skip_patterns='["secret code"]'))
    assert bodies(r) == ["1) P", "• Keep this sentence."]


def test_quote_handling():
    above = '“For God so loved the world.”\nJohn 3:16 NIV\n'
    assert run(above).verse_count == 1 and not [d for d in run(above).decisions if d.tier == "error"]
    assert [d.tier for d in run(above, cfg(notes__quote_above_reference="review")).decisions].count("error") == 1
    alone = '1) P\n“A quotation with nothing near it that names a verse.”\n'
    assert [d.tier for d in run(alone).decisions].count("error") == 1
    assert bodies(run(alone, cfg(notes__quote_alone="skip"))) == ["1) P"]
    assert bodies(run(alone, cfg(notes__quote_alone="bullet")))[-1].startswith("• “A quotation")


def test_a_reference_inside_a_sentence():
    text = "1) P\nAs Psalm 23:1 NIV tells us, we lack nothing in the Lord today.\n"
    assert run(text).verse_count == 0
    assert run(text, cfg(notes__reference_in_text="verse")).verse_count == 1
    assert not [d for d in run(text, cfg(notes__reference_in_text="skip")).decisions if d.tier == "error"]


def test_duplicates_can_be_dropped():
    text = "John 3:16 NIV\n1) P\nJohn 3:16 NIV\n"
    assert run(text).verse_count == 2 and run(text, cfg(notes__duplicates="drop")).verse_count == 1


def test_label_words_and_own_slide():
    text = "1) P\nBottom line: trust Him today\n"
    assert bodies(run(text))[-1] == "• Bottom line: trust Him today"
    custom = cfg(notes__label_words='["bottom line"]')
    r = run(text, custom)
    assert bodies(r)[-1] == "Bottom line: trust Him today" and [d.tier for d in r.decisions] == ["good", "good"]


def test_header_word_limit():
    text = "1) P\nGrace Alone Matters\n"
    assert bodies(run(text))[-1] == "• Grace Alone Matters"
    assert bodies(run(text, cfg(notes__header_max_words=3))) == ["1) P", "Grace Alone Matters"]      # a heading: its own slide, no bullet
    assert bodies(run(text, cfg(notes__header_max_words=3, notes__headings="skip"))) == ["1) P"]


# ------------------------------------------------------------------ points: parsing
def test_marker_kinds_can_be_turned_off():
    assert [p.label for p in classify_document("1) A\na) B\n- C\n", None, None, None).points][:1] == ["1)"]
    c = cfg(points__numbered="false", points__lettered="false", points__bullets="false")
    doc = classify_document("John 3:16 NIV\n1) A short line\nb) other\n- third\n", None, c.notes, c.points)
    assert [(p.label, p.text) for p in doc.points] == [("•", "1) A short line"), ("•", "b) other"), ("•", "- third")]
    on = classify_document("John 3:16 NIV\n1) A short line\nb) other\n- third\n")
    assert [p.text for p in on.points] == ["A short line", "other", "third"]


def test_out_of_order_numbers():
    text = "1) A\n2) B\n5) C\n"
    assert bodies(run(text))[2] == "5) C"
    assert [p.role for p in classify_document(text).points] == ["point", "point", "list_item"]
    c = cfg(points__require_sequence="false")
    assert [p.role for p in classify_document(text, None, c.notes, c.points).points] == ["point"] * 3


# ------------------------------------------------------------------ points: formatting
def test_numbering_strip_and_renumber():
    text = "1) First\n3) Third\n2) Second\n"
    assert bodies(run(text, cfg(points__numbering="strip"))) == ["First", "Third", "Second"]
    r = bodies(run("1) A\n2) B\n3) C\n", cfg(points__numbering="renumber", points__number_style="#{n}:")))
    assert r == ["#1: A", "#2: B", "#3: C"]


def test_bullet_character_and_case():
    text = "1) first point\nplain sentence here.\n"
    assert bodies(run(text, cfg(points__bullet_char="")))[-1] == "plain sentence here."
    assert bodies(run(text, cfg(points__bullet_char="-")))[-1] == "- plain sentence here."
    assert bodies(run(text, cfg(points__case="upper")))[0] == "1) FIRST POINT"
    assert bodies(run("1) the grace of god\n", cfg(points__case="title")))[0] == "1) The Grace of God"


# ------------------------------------------------------------------ verses
def test_version_source():
    text = "John 3:16 NIV\nRomans 8:28\n"
    above = run(text, default="ESV")
    assert "NIV" in above.decisions[1].reasons[0]
    default = run(text, cfg(verses__version_source="default"), default="ESV")
    assert "ESV" in default.decisions[1].reasons[0] and "default version" in default.decisions[1].reasons[0]
    required = run(text, cfg(verses__version_source="required"), default="ESV")
    assert required.decisions[1].tier == "error" and required.verse_count == 1


# ------------------------------------------------------------------ the options themselves
def test_profiles_and_set_overrides():
    base = AppConfig(profiles={"outline": {"points": {"numbering": "strip"}, "notes": {"line_breaks": "paragraph"}}})
    c = with_options(base, profile="outline")
    assert c.points.numbering == "strip" and c.notes.line_breaks == "paragraph" and base.points.numbering == "keep"
    assert with_options(base, profile="outline", sets=["points.numbering=keep"]).points.numbering == "keep"
    with pytest.raises(ConfigurationError, match="No profile"):
        with_options(base, profile="nope")
    with pytest.raises(ConfigurationError, match="Unknown setting"):
        with_options(base, sets=["notes.bogus=1"])
    with pytest.raises(ConfigurationError, match="Invalid option"):
        with_options(base, sets=["notes.line_breaks=sideways"])


def test_a_text_option_stays_text_even_when_it_looks_like_a_number():
    c = with_options(AppConfig(), sets=["verses.theme_slide=2", "points.bullet_char=-"])
    assert c.verses.theme_slide == "2" and c.points.bullet_char == "-"


def test_verse_numbers_are_off_by_default_and_can_be_turned_on():
    assert AppConfig().slides.include_verse_numbers is False
