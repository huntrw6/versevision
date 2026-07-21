import pytest

from versebridge.references import parse_references


@pytest.mark.parametrize(("source", "normalized"), [
    ("John 1:14", "John 1:14"),
    ("John 1:14-18", "John 1:14-18"),
    ("John 1:14–18", "John 1:14-18"),
    ("John 1:14 through 18", "John 1:14-18"),
    ("John 1:14 & 20", "John 1:14 & 20"),
    ("John 1:14-18 & 20-23", "John 1:14-18 & 20-23"),
    ("John 1:14 & 20-23 & 25", "John 1:14 & 20-23 & 25"),
    ("John 1:14-18, 20-23, 25", "John 1:14-18 & 20-23 & 25"),
    ("John 1:14-18 & 2:1-3", "John 1:14-18 & 2:1-3"),
    ("John 3:16 NLT", "John 3:16 NLT"),
    ("John 3:16 (NLT)", "John 3:16 NLT"),
    ("1 John 4:7-8 NIV", "1 John 4:7-8 NIV"),
    ("II Corinthians 5:17 ESV", "2 Corinthians 5:17 ESV"),
    ("Song of Solomon 2:1", "Song of Solomon 2:1"),
    ("Psalm 23", "Psalms 23:1"),
    ("Jude 5", "Jude 1:5"),
    ("Jn. 3:16", "John 3:16"),
])
def test_supported_references(source, normalized):
    result = parse_references(source)
    assert not result.issues
    assert [ref.normalized_text for ref in result.references] == [normalized]


def test_composite_is_one_logical_reference():
    result = parse_references("The message uses John 1:14-18 & 20-23 & 25 NLT.")
    assert len(result.references) == 1
    ref = result.references[0]
    assert ref.version == "NLT"
    assert [(s.start.verse, s.end.verse if s.end else None) for s in ref.spans] == [(14, 18), (20, 23), (25, None)]


def test_semicolon_inherits_book_as_new_passage():
    result = parse_references("John 3:16; 4:1-3")
    assert [r.normalized_text for r in result.references] == ["John 3:16", "John 4:1-3"]


@pytest.mark.parametrize("source", ["Meeting at 3:15", "Example 12:4", "3 Matthew 2:1"])
def test_unrecognized_books_are_ignored(source):
    result = parse_references(source)
    assert result.references == []
    assert result.issues == []


@pytest.mark.parametrize("source", ["John 99:1", "John 1:999", "John 1:20-14"])
def test_invalid_references_are_reported(source):
    result = parse_references(source)
    assert not result.references
    assert result.issues


def test_unrelated_and_is_not_consumed():
    result = parse_references("Romans 8:28 and unrelated prose")
    assert result.references[0].source_text == "Romans 8:28"

