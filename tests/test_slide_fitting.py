from versevision.slides.fitting import split_verse_text, wrap_body_lines


def test_long_verse_is_cut_into_nearly_equal_slides_and_prefers_punctuation():
    text = (
        "This is the first naturally complete thought, with enough words to occupy several lines. "
        "This is the second naturally complete thought, and it also contains enough words to overflow the slide."
    )
    pieces = split_verse_text(text, max_lines=2, line_width_units=32)
    assert len(pieces) > 1 and " ".join(pieces) == text
    assert all(len(wrap_body_lines(piece, 32)) <= 2 for piece in pieces)
    assert max(len(x) for x in pieces) <= 1.35 * min(len(x) for x in pieces)
    # two sentences of about the same length: the cut is at the full stop
    sentence = "The Lord is faithful to every word He has spoken, and He keeps His promises to those who wait on Him with patience."
    halves = split_verse_text(sentence + " " + sentence.replace("Lord", "King"), max_lines=3, line_width_units=48)
    assert len(halves) == 2 and halves[0].endswith("patience.")


ESTHER = ("Then were the king's scribes called at that time in the third month, that is, the month Sivan, on the three and twentieth day thereof; "
          "and it was written according to all that Mordecai commanded unto the Jews, and to the lieutenants, and the deputies and rulers of the provinces "
          "which are from India unto Ethiopia, an hundred twenty and seven provinces, unto every province according to the writing thereof, and unto every "
          "people after their language, and to the Jews according to their writing, and according to their language.")


def test_pieces_may_be_25_percent_off_the_average_when_they_start_and_end_at_punctuation():
    pieces = split_verse_text(ESTHER)
    assert len(pieces) == 3 and " ".join(pieces) == ESTHER
    assert [x[-1] for x in pieces] == [";", ",", "."]               # natural breaks, not "...according to all that | Mordecai"
    average = sum(len(x) for x in pieces) / 3
    assert all(abs(len(x) - average) <= 0.25 * average + 8 for x in pieces)


def test_without_the_punctuation_allowance_pieces_are_within_15_percent_even_mid_phrase():
    pieces = split_verse_text(ESTHER, punctuation_tolerance=0.15)
    average = sum(len(x) for x in pieces) / 3
    assert all(abs(len(x) - average) <= 0.15 * average + 8 for x in pieces) and " ".join(pieces) == ESTHER


def test_a_long_point_is_broken_into_even_lines_and_never_leaves_a_one_word_last_line():
    from versevision.slides.fitting import balance_lines, text_width_units
    out = balance_lines("• It should make us more loving, more compassionate, more truthful, more forgiving, and more committed to righteousness.", 33.0, glue_first=2)
    lines = out.split("\n")
    assert len(lines) == 4 and lines[0].startswith("• It") and " ".join(lines).replace("• ", "• ") == out.replace("\n", " ")
    widths = [text_width_units(x.upper()) for x in lines]
    assert max(widths) <= 33.0 and min(widths) >= 0.7 * max(widths)
    assert balance_lines("• We need revival in our pulpits.", 33.0, glue_first=2) == "• We need revival in our pulpits."      # fits: left alone
    assert balance_lines("KEY #2) PREACHING MUST CONFRONT SIN AND POINT PEOPLE TO THE SAVIOR", 33.0, glue_first=2).split("\n")[0].startswith("KEY #2) PREACHING")


def test_short_verse_remains_one_slide() -> None:
    assert split_verse_text("A short synthetic verse.") == ["A short synthetic verse."]


def test_point_lines_may_be_25_percent_uneven_to_break_at_punctuation():
    from versevision.slides.fitting import balance_lines
    point = "• Pray without ceasing, give thanks in everything, and rejoice always in the Lord."
    natural = balance_lines(point, 33.0, glue_first=2)
    assert natural.split("\n") == ["• Pray without ceasing,", "give thanks in everything,", "and rejoice always in the Lord."]
    even = balance_lines(point, 33.0, glue_first=2, punctuation_tolerance=0.15)
    assert even.split("\n")[1].endswith("and")        # without the allowance the lines are evener but cut mid-phrase


POINT_NOTES = """Title 11/8/26

1) Trust the Lord with all your heart, lean not on your own understanding, and in all your ways acknowledge Him, and He will direct your paths.
2) Be still.
• Pray without ceasing, give thanks in everything, and rejoice always in the Lord, because this is the will of God for you.
"""


def _point_slides(limit, notes=POINT_NOTES):
    import asyncio
    from versevision.config import AppConfig, PointsConfig
    from versevision.workflow.notes_workflow import plan_notes
    result = asyncio.run(plan_notes(notes, AppConfig(points=PointsConfig(max_lines_per_slide=limit)), default_version="NKJV", no_fetch=True))
    return result, [s.body_text for s in result.slide_plan.slides if s.kind == "point"]


def test_a_point_longer_than_the_line_limit_continues_on_more_slides():
    _, unlimited = _point_slides(0)
    result, limited = _point_slides(2)
    assert len(limited) > len(unlimited)
    assert all(len(text.split("\n")) <= 2 for text in limited)
    flat = lambda texts: " ".join(" ".join(texts).split()).upper()
    assert flat(limited).replace("•", "").split() == flat(unlimited).replace("•", "").split()      # no words lost or moved
    assert limited[0].startswith("1)") and not limited[1].startswith(("1)", "•"))                   # only the first slide carries the label
    assert [d.slide_count for d in result.decisions if d.kind == "point"] == [3, 1, 2]


def test_the_default_has_no_point_line_limit():
    _, slides = _point_slides(0)
    assert len(slides) == 3 and max(len(t.split("\n")) for t in slides) >= 4


def test_a_line_added_in_the_review_also_respects_the_point_line_limit():
    from versevision.config import PointsConfig
    from versevision.workflow.notes_workflow import added_point_text
    text = "Pray without ceasing, give thanks in everything, and rejoice always in the Lord, because this is the will of God for you."
    parts = added_point_text(text, PointsConfig(max_lines_per_slide=2)).split("\f")
    assert len(parts) == 2 and all(len(p.split("\n")) <= 2 for p in parts)


def test_a_reference_too_long_for_one_line_is_flagged_in_the_review():
    import asyncio
    from versevision.config import AppConfig
    from versevision.workflow.notes_workflow import plan_notes
    notes = "Title 11/8/26\n\nEphesians 4:5 NKJV\n\nMatthew 7:16-20, 21-23, 24-27 NKJV\n"
    result = asyncio.run(plan_notes(notes, AppConfig(), default_version="NKJV", no_fetch=True))
    verses = [d for d in result.decisions if d.kind == "verse"]
    assert not any("reference box" in r for r in verses[0].reasons)
    cfg = AppConfig(); cfg.slides.reference_width_units = 12.0
    narrow = asyncio.run(plan_notes(notes, cfg, default_version="NKJV", no_fetch=True))
    assert any("reference box" in r for d in narrow.decisions if d.kind == "verse" for r in d.reasons)
