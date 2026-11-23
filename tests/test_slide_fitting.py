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
