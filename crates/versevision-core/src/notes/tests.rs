//! Unit tests of the classifier: the cases of `python/tests/unit/test_notes.py` and `python/tests/test_options.py` that the oracle
//! documents do not already show, and the places where Python raises.

use indexmap::IndexMap;

use super::classifier::{classify_document_traced, classify_with_trace, ClassifyError, Shortcuts};
use super::classify_document;
use super::models::{DocumentItem, NoteDocument, PointRole, SkipCategory};
use super::trace::{clean_value, NoTrace};
use crate::config::{
    Duplicates, Handling, Headings, LineBreaks, NotesConfig, PointsConfig, QuoteAboveReference,
    ReferenceInText,
};

fn classify(text: &str) -> NoteDocument {
    classify_document(text, None, None, None).expect("classifies")
}

fn with(text: &str, edit: impl FnOnce(&mut NotesConfig)) -> NoteDocument {
    let mut notes = NotesConfig::default();
    edit(&mut notes);
    classify_document(text, None, Some(&notes), None).expect("classifies")
}

fn cautious(text: &str) -> NoteDocument {
    classify_document_traced(
        text,
        None,
        None,
        None,
        Shortcuts {
            line_mode: Some(false),
            unlabeled_bullets: Some(false),
        },
        &mut NoTrace,
    )
    .expect("classifies")
}

fn displays(doc: &NoteDocument) -> Vec<String> {
    doc.points().iter().map(|p| p.display()).collect()
}

#[test]
fn out_of_sequence_numbers_are_list_items_and_keep_numbers() {
    let doc = classify("1) First point\n21) Thesis text\n2) Second point");
    let got: Vec<(PointRole, String)> = doc
        .points()
        .iter()
        .map(|p| (p.role, p.label.clone()))
        .collect();
    assert_eq!(
        got,
        vec![
            (PointRole::Point, "1)".to_string()),
            (PointRole::ListItem, "21)".to_string()),
            (PointRole::Point, "2)".to_string())
        ]
    );
}

#[test]
fn stage_notes_and_headers_are_skipped_not_slides() {
    let doc = classify(
        "Part 2: Title\nShow Photo of Luther\nWhen I say Here I Stand put the photo up\n1) Point",
    );
    assert!(doc
        .skipped()
        .iter()
        .all(|s| s.category == SkipCategory::StageNote));
    assert_eq!(doc.skipped().len(), 2);
    assert_eq!(displays(&doc), vec!["Part 2: Title", "1) Point"]); // the title is a slide (no bullet); cues stay out
}

#[test]
fn slide_tag_is_bulleted_and_duplicates_kept() {
    let doc = classify("[slide] Faith = pistis\nJohn 3:16 NIV\nJohn 3:16 NIV");
    assert!(doc.points()[0].display().starts_with("• "));
    assert_eq!(doc.verses().len(), 2);
}

const WRAPPED: &str = "JPL 1/1/26 Title of the message\n\
“For God so loved the world that he gave his one and only Son, that whoever believes in him\n\
shall not perish but have eternal life.” \n\
John 3:16 NIV\n\
1) First point title here\n\
Be still and know that I am God, I will be exalted among the nations, exalted in the earth. \n\
Psalm 46:10 ESV\n\
Next verse text without quote marks, long enough to reach the margin of the page here ok. \n\
Romans 8:28 ESV\n\
2. Second point\n";

#[test]
fn hard_wrapped_quote_then_reference_is_one_verse_and_no_review() {
    let doc = classify(WRAPPED);
    let verses: Vec<String> = doc
        .verses()
        .iter()
        .map(|v| v.reference.normalized_text.clone())
        .collect();
    assert_eq!(
        verses,
        vec!["John 3:16 NIV", "Psalms 46:10 ESV", "Romans 8:28 ESV"]
    );
    assert_eq!(
        displays(&doc),
        vec!["1) First point title here", "2. Second point"]
    );
    assert!(doc.review().is_empty());
}

#[test]
fn the_pdf_style_wrap_is_traced_with_its_reason() {
    let (_, log) = classify_with_trace(WRAPPED, Some(&IndexMap::new()), None, None).unwrap();
    let start = &log.records[0];
    assert_eq!(start.0, "parse_start");
    assert_eq!(start.1["hard_wrapped"], true);
    let join = log
        .records
        .iter()
        .find(|(stage, f)| stage == "line_join" && f["line"] == 7)
        .expect("line 7 is joined to line 6");
    assert_eq!(join.1["into_paragraph"], 6);
    assert!(join.1["why"]
        .as_str()
        .unwrap()
        .contains("sentence has not ended"));
}

#[test]
fn unlabeled_prose_is_flagged_for_review_in_the_cautious_mode() {
    let doc = cautious("1) A point\nThis is a long sentence of ordinary prose that is not a point, a reference, or a cue at all.");
    assert!(!doc.review().is_empty() || doc.points()[0].warnings.join(" ").contains("joined"));
}

#[test]
fn unlabeled_sentence_becomes_its_own_bullet_slide_with_no_warning() {
    let doc = classify("1) A point\nThis is a long sentence of ordinary prose that is not a point, a reference, or a cue at all.");
    let labels: Vec<&str> = doc.points().iter().map(|p| p.label.as_str()).collect();
    assert_eq!(labels, vec!["1)", "•"]);
    assert!(doc.points()[1].warnings.is_empty() && doc.review().is_empty());
}

#[test]
fn a_short_unpunctuated_line_becomes_a_bullet_but_with_a_warning_because_it_may_be_a_heading() {
    let doc = classify("1) A point\nOpen with prayer");
    assert_eq!(doc.points()[1].text, "Open with prayer");
    assert!(!doc.points()[1].warnings.is_empty());
}

#[test]
fn reference_inside_a_point_is_not_a_verse_slide() {
    let doc = classify("1. We rest in him, see Psalm 46:10 for more.\nJohn 3:16 NIV");
    assert_eq!(doc.verses().len(), 1);
    assert!(!doc.verses()[0].reference.normalized_text.contains("Psalm"));
}

#[test]
fn title_with_chapter_and_date_is_a_header() {
    let doc = classify("Psalm 42–8/2/26\nJohn 3:16 NIV");
    assert_eq!(doc.skipped()[0].category, SkipCategory::Header);
    assert_eq!(doc.verses().len(), 1);
}

#[test]
fn verse_fragment_is_not_a_point_marker() {
    assert!(classify("2 Corinthians\n12:9 NKJV").points().is_empty());
}

#[test]
fn bullets_keep_their_bullet_and_lettered_items_keep_letters() {
    let doc = classify("- Point one\n• Point two\nA. First\nB) Second");
    assert_eq!(
        displays(&doc),
        vec!["• Point one", "• Point two", "A. First", "B) Second"]
    );
}

#[test]
fn numbered_reference_is_a_verse_not_a_point() {
    let doc = classify("1. John 3:16 NIV\n2. Romans 8:28 NKJV");
    assert_eq!(doc.verses().len(), 2);
    assert!(doc.points().is_empty());
}

#[test]
fn notes_to_self_are_skipped() {
    let doc = classify("(ask Amy to cue the video)\nNOTE: remember offering\nTODO fix this");
    assert_eq!(doc.skipped().len(), 3);
    assert!(doc
        .skipped()
        .iter()
        .all(|s| s.category == SkipCategory::StageNote));
    assert!(doc.review().is_empty());
}

#[test]
fn bare_chapter_and_midsentence_reference_go_to_review() {
    let doc = classify("Psalm 23\nSee Romans 8:28 for context");
    assert_eq!(doc.review().len(), 2);
    assert!(doc.verses().is_empty());
}

#[test]
fn short_line_under_a_point_is_flagged_not_merged_but_sentences_are_merged_in_the_cautious_mode() {
    let doc = cautious("1) A point\nOpen with prayer");
    assert_eq!(doc.points()[0].text, "A point");
    assert_eq!(doc.review().len(), 1);
    let doc = cautious("1) A point\nTo obey God rather than human beings.");
    assert!(doc.points()[0].text.contains("obey God") && doc.review().is_empty());
}

#[test]
fn every_line_is_its_own_slide_by_default() {
    let doc = classify("1) A point\nOpen with prayer\nTo obey God rather than human beings.");
    let texts: Vec<&str> = doc.points().iter().map(|p| p.text.as_str()).collect();
    assert_eq!(
        texts,
        vec![
            "A point",
            "Open with prayer",
            "To obey God rather than human beings."
        ]
    );
    assert!(doc.review().is_empty());
}

#[test]
fn a_reference_alone_in_brackets_is_a_verse_not_a_stage_note() {
    let doc = classify_document(
        "1) Point\n\"Quote.\"\n(Romans 8:16)\n",
        Some(&IndexMap::new()),
        None,
        None,
    )
    .unwrap();
    let verses: Vec<&str> = doc
        .verses()
        .iter()
        .map(|v| v.reference.normalized_text.as_str())
        .collect();
    assert_eq!(verses, vec!["Romans 8:16"]);
    assert!(!doc
        .skipped()
        .iter()
        .any(|s| s.category == SkipCategory::StageNote));
    // a parenthetical remark that merely mentions a verse is still a note
    assert!(classify("1) Point\n(see me about this later)\n")
        .verses()
        .is_empty());
}

#[test]
fn marker_kinds_can_be_turned_off() {
    let points = PointsConfig {
        numbered: false,
        lettered: false,
        bullets: false,
        ..PointsConfig::default()
    };
    let doc = classify_document(
        "John 3:16 NIV\n1) A short line\nb) other\n- third\n",
        None,
        None,
        Some(&points),
    )
    .unwrap();
    let got: Vec<(&str, &str)> = doc
        .points()
        .iter()
        .map(|p| (p.label.as_str(), p.text.as_str()))
        .collect();
    assert_eq!(
        got,
        vec![
            ("•", "1) A short line"),
            ("•", "b) other"),
            ("•", "- third")
        ]
    );
    let on = classify("John 3:16 NIV\n1) A short line\nb) other\n- third\n");
    let texts: Vec<&str> = on.points().iter().map(|p| p.text.as_str()).collect();
    assert_eq!(texts, vec!["A short line", "other", "third"]);
}

#[test]
fn out_of_order_numbers() {
    let text = "1) A\n2) B\n5) C\n";
    let roles: Vec<PointRole> = classify(text).points().iter().map(|p| p.role).collect();
    assert_eq!(
        roles,
        vec![PointRole::Point, PointRole::Point, PointRole::ListItem]
    );
    let points = PointsConfig {
        require_sequence: false,
        ..PointsConfig::default()
    };
    let doc = classify_document(text, None, None, Some(&points)).unwrap();
    assert!(doc.points().iter().all(|p| p.role == PointRole::Point));
}

#[test]
fn header_word_limit() {
    let text = "1) P\nGrace Alone Matters\n";
    assert_eq!(displays(&classify(text))[1], "• Grace Alone Matters");
    let doc = with(text, |n| n.header_max_words = 3);
    assert_eq!(displays(&doc), vec!["1) P", "Grace Alone Matters"]); // a heading: its own slide, no bullet
    let doc = with(text, |n| {
        n.header_max_words = 3;
        n.headings = Headings::Skip;
    });
    assert_eq!(displays(&doc), vec!["1) P"]);
}

#[test]
fn label_words_are_options() {
    let text = "1) P\nBottom line: trust Him today\n";
    assert_eq!(
        displays(&classify(text))[1],
        "• Bottom line: trust Him today"
    );
    let doc = with(text, |n| n.label_words = vec!["bottom line".to_string()]);
    assert_eq!(displays(&doc)[1], "Bottom line: trust Him today");
}

#[test]
fn the_options_decide_what_a_cue_a_quote_or_a_mention_becomes() {
    let doc = with("1) P\nShow the photo\n", |n| {
        n.stage_cues = Handling::Bullet
    });
    assert_eq!(
        doc.points()[1].warnings,
        vec!["stage/production cue made a slide because of your setting".to_string()]
    );
    let doc = with("“A quote.”\nJohn 3:16 NIV\n", |n| {
        n.quote_above_reference = QuoteAboveReference::Review
    });
    assert_eq!(doc.review().len(), 1);
    let doc = with("As John 3:16 says\n", |n| {
        n.reference_in_text = ReferenceInText::Verse
    });
    assert_eq!(doc.verses().len(), 1);
    let doc = with("John 3:16 NIV\nJohn 3:16 NIV\n", |n| {
        n.duplicates = Duplicates::Drop
    });
    assert_eq!((doc.verses().len(), doc.skipped().len()), (1, 1));
    let doc = with("1) A\nA detail.\n", |n| {
        n.line_breaks = LineBreaks::Paragraph
    });
    assert_eq!(doc.points()[0].text, "A\nA detail.");
}

// ------------------------------------------------------------------------------------------------ where Python raises

#[test]
fn a_numbered_whole_chapter_reproduces_the_python_attribute_error() {
    // Python: AttributeError: 'ReviewItem' object has no attribute 'warnings' (a bug of the Python app; see oracle/document_errors.jsonl)
    let error = classify_document("1. Psalm 23\n", None, None, None).unwrap_err();
    assert_eq!(error.python_exception(), "AttributeError");
    assert_eq!(
        error.to_string(),
        "'ReviewItem' object has no attribute 'warnings'"
    );
    let notes = NotesConfig {
        duplicates: Duplicates::Drop,
        ..NotesConfig::default()
    };
    let error =
        classify_document("John 3:16\n1. John 3:16\n", None, Some(&notes), None).unwrap_err();
    assert_eq!(
        error.to_string(),
        "'SkippedItem' object has no attribute 'warnings'"
    );
}

#[test]
fn chapter_zero_raises_like_the_reference_parser() {
    let error = classify_document("John 0:1\n", None, None, None).unwrap_err();
    assert!(matches!(error, ClassifyError::Reference(_)));
    assert_eq!(error.python_exception(), "ValidationError");
}

#[test]
fn a_bad_skip_pattern_raises_the_re_error_text() {
    let notes = NotesConfig {
        skip_patterns: vec!["ok".to_string(), "(".to_string()],
        ..NotesConfig::default()
    };
    let error = classify_document("x", None, Some(&notes), None).unwrap_err();
    assert_eq!(error.python_exception(), "PatternError");
    assert_eq!(
        error.to_string(),
        "missing ), unterminated subpattern at position 0"
    );
}

#[test]
fn an_empty_skip_pattern_matches_every_line_but_is_not_a_hit() {
    // `starts_new` counts it (every line starts a new paragraph); `_label` does not (an empty pattern is falsy)
    let notes = NotesConfig {
        skip_patterns: vec![String::new()],
        ..NotesConfig::default()
    };
    let doc = classify_document("1) A\nB line here\n", None, Some(&notes), None).unwrap();
    assert!(doc.skipped().is_empty());
}

#[test]
fn trace_values_are_cut_like_the_python_run_log() {
    let long = "x".repeat(2500);
    let cleaned = clean_value(
        serde_json::json!({"text": long, "list": (0..400).collect::<Vec<_>>(), "api_key": "k"}),
        2000,
    );
    let text = cleaned["text"].as_str().unwrap();
    assert!(
        text.ends_with("... [500 more characters]")
            && text.chars().count() == 2000 + "... [500 more characters]".chars().count()
    );
    assert_eq!(cleaned["list"].as_array().unwrap().len(), 300);
    assert_eq!(cleaned["api_key"], "[REDACTED]");
}

#[test]
fn items_serialise_like_the_python_model() {
    let doc = classify("1) One\nJohn 3:16 NIV\n");
    let value = serde_json::to_value(&doc).unwrap();
    assert_eq!(value["items"][0]["kind"], "point");
    assert_eq!(value["items"][0]["label_fixed_from"], "");
    assert_eq!(value["items"][1]["kind"], "verse");
    assert_eq!(
        value["items"][1]["reference"]["normalized_text"],
        "John 3:16 NIV"
    );
    assert!(matches!(doc.items[1], DocumentItem::Verse(_)));
}
