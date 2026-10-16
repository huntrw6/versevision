from versevision.references import parse_references


def test_standalone_references_after_newlines_are_not_skipped() -> None:
    result = parse_references(
        "Opening prose\nLuke 11:1 NKJV\nMore prose\nMatthew 6:9 NIV\nClosing prose"
    )

    assert [reference.normalized_text for reference in result.references] == [
        "Luke 11:1 NKJV",
        "Matthew 6:9 NIV",
    ]


def test_verse_part_suffix_keeps_translation_and_warns() -> None:
    result = parse_references("Luke 11:2a NKJV")

    assert result.references[0].normalized_text == "Luke 11:2 NKJV"
    assert "full verse" in result.references[0].warnings[0]
