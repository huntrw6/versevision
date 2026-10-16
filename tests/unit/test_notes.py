from versebridge.notes import classify_document
from versebridge.references import parse_references
from versebridge.presentation.protobuf_writer import _replace_rtf_text


def kinds(text):
    return [(i.kind, getattr(i, "role", getattr(i, "category", ""))) for i in classify_document(text).items]


def test_ylt_is_read_as_nlt_with_warning():
    ref = parse_references("Hebrews 11:3 (YLT) text").references[0]
    assert ref.version == "NLT" and any("YLT" in w for w in ref.warnings)


def test_real_but_unavailable_version_is_never_corrected():
    assert parse_references("John 3:16 CSB").references[0].version == "CSB"
    assert parse_references("John 3:16 [HCSB]").references[0].version == "HCSB"


def test_typo_version_is_corrected_and_unknown_is_not():
    ref = parse_references("Psalm 23:1 [NTL]").references[0]
    assert ref.version == "NLT"
    assert parse_references("Psalm 23:1 [XYZQ]").references[0].version is None


def test_nasb_variants():
    assert parse_references("Gen 1:1 NASB").references[0].version == "NASB2020"
    assert parse_references("Gen 1:1 NASB1995").references[0].version == "NASB1995"


def test_out_of_sequence_numbers_are_list_items_and_keep_numbers():
    doc = classify_document("1) First point\n21) Thesis text\n2) Second point")
    assert [(p.role, p.label) for p in doc.points] == [("point", "1)"), ("list_item", "21)"), ("point", "2)")]


def test_stage_notes_and_headers_are_skipped_not_slides():
    doc = classify_document("Part 2: Title\nShow Photo of Luther\nWhen I say Here I Stand put the photo up\n1) Point")
    assert [i.category for i in doc.skipped] == ["header", "stage_note", "stage_note"]
    assert len(doc.points) == 1


def test_slide_tag_is_bulleted_and_duplicates_kept():
    doc = classify_document("[slide] Faith = pistis\nJohn 3:16 NIV\nJohn 3:16 NIV")
    assert doc.points[0].display.startswith("• ") and len(doc.verses) == 2


def test_rtf_without_cb_marker():
    rtf = b"{\\rtf1\\ansi{\\fonttbl\\f0\\fnil Helvetica;}\n{\\colortbl;\\red0\\green0\\blue0;}\n\\pard\\f0\\fs20 \\cf1 old text}"
    assert _replace_rtf_text(rtf, "new").endswith(b"\\cf1 new}")


def test_esv_provider_is_never_cached_and_label_stays():
    from versebridge.bibles.esv import EsvProvider
    assert EsvProvider.cacheable is False


def test_comparison_pair_does_not_carry_version_forward():
    from versebridge.workflow.notes_workflow import resolve_note_versions
    doc = classify_document("Psalm 46:1 NKJV\nPsalm 81:10 ESV\nPsalm 81:10 TPT\nPsalm 46:2\nGalatians 3:3")
    resolved, _ = resolve_note_versions(doc, default=None)
    assert [ref.version for _, ref, _ in resolved] == ["NKJV", "ESV", "TPT", "NKJV", "NKJV"]
