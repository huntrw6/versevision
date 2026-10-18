from versevision.notes import classify_document
from versevision.references import parse_references
from versevision.presentation.protobuf_writer import _replace_rtf_text


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
    from versevision.bibles.esv import EsvProvider
    assert EsvProvider.cacheable is False


def test_comparison_pair_does_not_carry_version_forward():
    from versevision.workflow.notes_workflow import resolve_note_versions
    doc = classify_document("Psalm 46:1 NKJV\nPsalm 81:10 ESV\nPsalm 81:10 TPT\nPsalm 46:2\nGalatians 3:3")
    resolved, _ = resolve_note_versions(doc, default=None)
    assert [ref.version for _, ref, _ in resolved] == ["NKJV", "ESV", "TPT", "NKJV", "NKJV"]


WRAPPED = (
    "JPL 1/1/26 Title of the message\n"
    "“For God so loved the world that he gave his one and only Son, that whoever believes in him\n"
    "shall not perish but have eternal life.” \n"
    "John 3:16 NIV\n"
    "1) First point title here\n"
    "Be still and know that I am God, I will be exalted among the nations, exalted in the earth. \n"
    "Psalm 46:10 ESV\n"
    "Next verse text without quote marks, long enough to reach the margin of the page here ok. \n"
    "Romans 8:28 ESV\n"
    "2. Second point\n"
)


def test_hard_wrapped_quote_then_reference_is_one_verse_and_no_review():
    doc = classify_document(WRAPPED)
    assert [v.reference.normalized_text for v in doc.verses] == ["John 3:16 NIV", "Psalms 46:10 ESV", "Romans 8:28 ESV"]
    assert [p.display for p in doc.points] == ["1) First point title here", "2. Second point"]
    assert not doc.review


def test_unlabeled_prose_is_flagged_for_review_not_guessed():
    doc = classify_document("1) A point\nThis is a long sentence of ordinary prose that is not a point, a reference, or a cue at all.")
    assert doc.review or "joined" in " ".join(doc.points[0].warnings)


def test_reference_inside_a_point_is_not_a_verse_slide():
    doc = classify_document("1. We rest in him, see Psalm 46:10 for more.\nJohn 3:16 NIV")
    assert len(doc.verses) == 1 and "Psalm" not in doc.verses[0].reference.normalized_text


def test_title_with_chapter_and_date_is_a_header():
    doc = classify_document("Psalm 42–8/2/26\nJohn 3:16 NIV")
    assert doc.skipped[0].category == "header" and len(doc.verses) == 1


def test_verse_fragment_is_not_a_point_marker():
    doc = classify_document("2 Corinthians\n12:9 NKJV")
    assert not doc.points


def test_bullets_keep_their_bullet_and_lettered_items_keep_letters():
    doc = classify_document("- Point one\n• Point two\nA. First\nB) Second")
    assert [p.display for p in doc.points] == ["• Point one", "• Point two", "A. First", "B) Second"]


def test_numbered_reference_is_a_verse_not_a_point():
    doc = classify_document("1. John 3:16 NIV\n2. Romans 8:28 NKJV")
    assert len(doc.verses) == 2 and not doc.points


def test_notes_to_self_are_skipped():
    doc = classify_document("(ask Amy to cue the video)\nNOTE: remember offering\nTODO fix this")
    assert [i.category for i in doc.skipped] == ["stage_note"] * 3 and not doc.review


def test_bare_chapter_and_midsentence_reference_go_to_review():
    doc = classify_document("Psalm 23\nSee Romans 8:28 for context")
    assert len(doc.review) == 2 and not doc.verses


def test_short_line_under_a_point_is_flagged_not_merged_but_sentences_are_merged():
    doc = classify_document("1) A point\nOpen with prayer")
    assert doc.points[0].text == "A point" and len(doc.review) == 1
    doc = classify_document("1) A point\nTo obey God rather than human beings.")
    assert "obey God" in doc.points[0].text and not doc.review


# ----------------------------------------------------------------------------- 10/18/26 John Wesley (real document)
WESLEY = (__import__("pathlib").Path(__file__).parents[1] / "fixtures" / "notes" / "wesley_2026_10_18.txt").read_text()


def _wesley():
    from versevision.notes.classifier import classify_document
    return classify_document(WESLEY, {})


def test_wesley_every_verse_and_point_is_found_in_document_order():
    got = []
    for item in _wesley().items:
        if item.kind == "verse":
            got.append(item.reference.normalized_text)
        elif item.kind == "point":
            got.append(item.display)
    assert got == [
        "1) A Person Must Realize They are Lost Before they are Saved.",
        "Luke 23:39-43 NKJV",
        "Romans 8:16",
        "2) You Must Be Born Again and Set On Fire",
        "Luke 2:49 NKJV",
        "James 2:17 KJV",
        "3) A Disciplined Faith Must Become a Living Faith",
        "Luke 14:23",
        "4) Identify Your Go",
        "5) One Surrendered Life Can Influence Generations",
        "Conclusion: Will You Let God Use Your Life?",
        "Romans 12:1 KJV",
    ]


def test_a_comma_before_the_version_belongs_to_the_reference():
    from versevision.references import parse_references
    ref = parse_references("— James 2:17, KJV", {}).references[0]
    assert ref.version == "KJV" and ref.normalized_text == "James 2:17 KJV"
    # but a comma between verses is still a list of verses
    again = parse_references("John 3:16, 17", {}).references[0]
    assert again.version is None and len(again.spans) == 2


def test_a_reference_alone_in_brackets_is_a_verse_not_a_stage_note():
    from versevision.notes.classifier import classify_document
    doc = classify_document('1) Point\n"Quote."\n(Romans 8:16)\n', {})
    assert [v.reference.normalized_text for v in doc.verses] == ["Romans 8:16"]
    assert not [s for s in doc.skipped if s.category == "stage_note"]
    # a parenthetical remark that merely mentions a verse is still a note
    other = classify_document("1) Point\n(see me about this later)\n", {})
    assert not other.verses


def test_a_conclusion_line_is_its_own_slide_not_joined_to_the_point_above():
    doc = _wesley()
    points = [p.display for p in doc.points]
    assert "5) One Surrendered Life Can Influence Generations" in points
    assert "Conclusion: Will You Let God Use Your Life?" in points


def test_a_version_less_verse_uses_the_nearest_version_above():
    from types import SimpleNamespace
    from versevision.workflow.notes_workflow import resolve_note_versions
    resolved, notices = resolve_note_versions(_wesley(), default="NKJV")
    by_ref = {r.normalized_text.split(" ")[0] + r.normalized_text.split(" ")[1]: r.version for _, r, _ in resolved if r}
    assert by_ref["Romans8:16"] == "NKJV"      # nearest above is Luke 23 NKJV
    assert by_ref["Luke14:23"] == "KJV"        # nearest above is James 2:17 KJV


def test_a_past_tense_stage_cue_is_not_a_quote_or_a_slide():
    doc = _wesley()
    cue = [s for s in doc.skipped if s.text.startswith("When I mentioned")]
    assert cue and cue[0].category == "stage_note"
    assert not doc.review


def test_a_wrapped_quote_line_starting_with_remember_stays_in_its_quote():
    doc = _wesley()
    assert len(doc.verses) >= 5
    assert not [r for r in doc.review if "remember me" in r.text]
