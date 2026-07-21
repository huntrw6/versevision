from versebridge.references.models import PassageReference


def test_warnings_are_not_shared():
    payload = dict(book_id="JHN", book_display_name="John", spans=[], source_text="", source_start=0,
                   source_end=0, normalized_text="")
    first = PassageReference(**payload)
    second = PassageReference(**payload)
    first.warnings.append("x")
    assert second.warnings == []

