from versebridge.slides.fitting import split_verse_text, wrap_body_lines


def test_long_verse_prefers_balanced_punctuation_break_and_repeats_safely() -> None:
    text = (
        "This is the first naturally complete thought, with enough words to occupy several lines. "
        "This is the second naturally complete thought, and it also contains enough words to overflow the slide."
    )
    pieces = split_verse_text(text, max_lines=2, line_width_units=32)

    assert len(pieces) > 1
    assert pieces[0][-1] in ".,;:?!"
    assert " ".join(pieces) == text
    assert all(len(wrap_body_lines(piece, 32)) <= 2 for piece in pieces)


def test_short_verse_remains_one_slide() -> None:
    assert split_verse_text("A short synthetic verse.") == ["A short synthetic verse."]
