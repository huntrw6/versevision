from hypothesis import given, strategies as st

from versebridge.references import parse_references


@given(
    start=st.integers(min_value=1, max_value=30),
    width=st.integers(min_value=0, max_value=5),
    separator=st.sampled_from(["-", "–", "—", " to ", " through "]),
    version=st.sampled_from(["NLT", "nlt", "Nlt", "ESV", "esv"]),
)
def test_range_delimiters_and_version_case_are_stable(start, width, separator, version):
    end = min(start + width, 36)
    result = parse_references(f"John 3:{start}{separator}{end} ({version})")
    assert not result.issues
    assert len(result.references) == 1
    reparsed = parse_references(result.references[0].normalized_text)
    assert reparsed.references[0].model_dump(exclude={"source_text", "source_start", "source_end"}) == \
        result.references[0].model_dump(exclude={"source_text", "source_start", "source_end"})
