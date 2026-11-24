//! Tests of our own for what `oracle/references.jsonl` does not cover. Every expected value below was also produced by running
//! the Python parser (`python/src/versevision/references`) on the same line.

use indexmap::IndexMap;

use super::*;

fn parse(line: &str) -> ParseResult {
    parse_references_with(line, None).expect("parses")
}

/// `(normalized texts, issue messages)` of a line.
fn shown(line: &str) -> (Vec<String>, Vec<String>) {
    let result = parse(line);
    (
        result
            .references
            .iter()
            .map(|r| r.normalized_text.clone())
            .collect(),
        result.issues.iter().map(|i| i.message.clone()).collect(),
    )
}

fn normalized(line: &str) -> Vec<String> {
    shown(line).0
}

#[test]
fn references_the_notes_write() {
    for (line, want) in [
        ("Ps 23:1-3,5", vec!["Psalms 23:1-3 & 5"]),
        ("Psalm 23:1–3", vec!["Psalms 23:1-3"]), // en dash
        ("Ps. 23:1—3", vec!["Psalms 23:1-3"]),   // em dash, dotted abbreviation
        ("Gen 1:30-2:3", vec!["Genesis 1:30-2:3"]),
        ("Genesis 1:30 – 2:3 NKJV", vec!["Genesis 1:30-2:3 NKJV"]),
        ("John 3:16 through 4:2", vec!["John 3:16-4:2"]),
        ("John 3:16 to 18", vec!["John 3:16-18"]),
        ("Rom. 8:28-30; 9:1", vec!["Romans 8:28-30", "Romans 9:1"]),
        ("eph 2:9", vec!["Ephesians 2:9"]),
        ("ROMANS 8:28", vec!["Romans 8:28"]),
        ("1 Cor 13:4-7, 13", vec!["1 Corinthians 13:4-7 & 13"]),
        ("2Cor 5:17", vec!["2 Corinthians 5:17"]),
        ("I John 1:9", vec!["1 John 1:9"]),
        ("First Samuel 17:45", vec!["1 Samuel 17:45"]),
        ("Third John 4", vec!["3 John 1:4"]),
        ("Mt 5:3-12", vec!["Matthew 5:3-12"]),
        ("Song 2:17", vec!["Song of Solomon 2:17"]),
        ("John 3:16, 17 and 18 KJV", vec!["John 3:16 & 17 & 18 KJV"]),
        ("Phil 4:13 niv", vec!["Philippians 4:13 NIV"]),
        ("Rev 22:21 AMP", vec!["Revelation 22:21 AMP"]),
        ("Eph 2:8-9 (ESV)", vec!["Ephesians 2:8-9 ESV"]),
        (
            "See John 3:16 and Romans 8:28 today",
            vec!["John 3:16", "Romans 8:28"],
        ),
        ("Job 42:17", vec!["Job 42:17"]),
        ("Ps 119:176", vec!["Psalms 119:176"]),
    ] {
        assert_eq!(normalized(line), want, "{line}");
    }
}

#[test]
fn one_chapter_books_take_a_bare_number_as_a_verse() {
    assert_eq!(normalized("Jude 5"), vec!["Jude 1:5"]);
    assert_eq!(normalized("Jude 3-4"), vec!["Jude 1:3-4"]);
    assert_eq!(normalized("Obad 15"), vec!["Obadiah 1:15"]);
    assert_eq!(normalized("3 John 14"), vec!["3 John 1:14"]);
}

#[test]
fn a_bare_number_in_a_long_book_is_a_whole_chapter() {
    let result = parse("Romans 8");
    assert_eq!(result.references[0].normalized_text, "Romans 8:1");
    assert_eq!(
        result.references[0].warnings,
        vec!["whole chapter reference; provider must expand chapter verses"]
    );
    assert_eq!(normalized("1Cor. 13"), vec!["1 Corinthians 13:1"]);
    assert_eq!(normalized("Matthew 5-7"), vec!["Matthew 5:1-7"]);
}

#[test]
fn offsets_are_character_positions_in_the_source() {
    let result = parse("Rom. 8:28-30; 9:1");
    let spans: Vec<(usize, usize, &str)> = result
        .references
        .iter()
        .map(|r| (r.source_start, r.source_end, r.source_text.as_str()))
        .collect();
    assert_eq!(spans, vec![(0, 12, "Rom. 8:28-30"), (14, 17, "9:1")]);
    let result = parse("(John 3:16)");
    assert_eq!(
        (
            result.references[0].source_start,
            result.references[0].source_end
        ),
        (1, 10)
    );
    // offsets count characters, not bytes
    let result = parse("Verse – John 3:16");
    assert_eq!(
        (
            result.references[0].source_start,
            result.references[0].source_end
        ),
        (8, 17)
    );
    assert_eq!(result.references[0].source_text, "John 3:16");
}

#[test]
fn trailing_punctuation_and_dangling_pieces() {
    assert_eq!(normalized("John 3:16."), vec!["John 3:16"]);
    assert_eq!(normalized("John 3:16;"), vec!["John 3:16"]);
    assert_eq!(normalized("Acts 2:1-4,"), vec!["Acts 2:1-4"]);
    assert_eq!(normalized("(John 3:16)"), vec!["John 3:16"]);
    // a range with no end is dropped altogether, as in Python
    assert_eq!(shown("Heb 11:1-"), (vec![], vec![]));
    // "3rd" is not a book number: the match starts at "John"
    assert_eq!(normalized("3rd John 4"), vec!["John 4:1"]);
}

#[test]
fn verse_part_suffixes_are_noted_and_dropped() {
    let result = parse("John 3:16b-18c NIV");
    assert_eq!(result.references[0].normalized_text, "John 3:16-18 NIV");
    assert_eq!(
        result.references[0].warnings,
        vec![
            "verse-part suffix 'b' detected; the provider will retrieve the full verse",
            "verse-part suffix 'c' detected; the provider will retrieve the full verse",
        ]
    );
}

#[test]
fn error_messages_are_pythons() {
    for (line, message) in [
        ("John 99:1", "John has no chapter 99"),
        ("Ps 151:1", "Psalms has no chapter 151"),
        ("Revelation 23:1", "Revelation has no chapter 23"),
        ("Ps 119:178", "Psalms 119 has no verse 178"),
        ("Luke 1:82", "Luke 1 has no verse 82"),
        ("3 John 16", "3 John 1 has no verse 16"),
        ("Song of Solomon 2:19", "Song of Solomon 2 has no verse 19"),
        ("Job 42:19", "Job 42 has no verse 19"),
        ("John 3:18-16", "range end precedes range start"),
        (
            "John 99:1-100:2",
            "John has no chapter 99; John has no chapter 100",
        ),
    ] {
        let (references, issues) = shown(line);
        assert!(references.is_empty(), "{line}");
        assert_eq!(issues, vec![message.to_string()], "{line}");
    }
}

#[test]
fn one_verse_of_slack_over_the_kjv_count() {
    // KJV John 3 has 36 verses, Luke 1 has 80, 3 John has 14: one more is allowed (modern versions differ by a verse)
    assert_eq!(normalized("Luke 1:80"), vec!["Luke 1:80"]);
    assert_eq!(normalized("Luke 1:81"), vec!["Luke 1:81"]);
    assert_eq!(normalized("3 John 15"), vec!["3 John 1:15"]);
    assert_eq!(normalized("John 3:37"), vec!["John 3:37"]);
    assert_eq!(shown("John 3:38").1, vec!["John 3 has no verse 38"]);
}

#[test]
fn overlapping_spans_are_an_issue_but_a_new_passage_may_go_back() {
    assert_eq!(shown("John 3:16; 3:16").0, vec!["John 3:16", "John 3:16"]); // two passages (`;`)
    assert_eq!(
        shown("John 3:16 & 3:16").1,
        vec!["duplicate or overlapping spans"]
    );
    assert_eq!(
        shown("John 3:16 & 15").1,
        vec!["duplicate or overlapping spans"]
    );
    assert_eq!(normalized("John 4:1; 3:16"), vec!["John 4:1", "John 3:16"]);
}

#[test]
fn zero_chapter_or_verse_raises_like_pydantic() {
    for line in [
        "John 0:1",
        "John 1:0",
        "Psalm 0",
        "John 1:1-0",
        "John 3:1-0:2",
    ] {
        let error = parse_references_with(line, None).expect_err(line);
        assert_eq!(error.python_exception(), "ValidationError", "{line}");
    }
    assert!(parse_references_with("John 0:1", None)
        .unwrap_err()
        .to_string()
        .contains("greater than 0"));
}

#[test]
fn versions_after_the_reference() {
    let one = |line: &str| {
        let r = parse(line).references.remove(0);
        (r.version, r.normalized_text, r.warnings)
    };
    assert_eq!(one("John 3:16 NASB").0.as_deref(), Some("NASB2020"));
    assert_eq!(one("John 3:16 NASB").2, vec!["NASB read as NASB2020"]);
    assert_eq!(one("John 3:16, KJV").0.as_deref(), Some("KJV"));
    assert_eq!(one("John 3:16 NASB1995").0.as_deref(), Some("NASB1995"));
    assert_eq!(one("John 3:16 [esv]").0.as_deref(), Some("ESV"));
    // the default operator alias
    assert_eq!(
        one("John 3:16 YLT"),
        (
            Some("NLT".into()),
            "John 3:16 NLT".into(),
            vec!["version 'YLT' read as NLT (configured alias)".to_string()]
        )
    );
    // a spelling fix, and a label that is only noted
    assert_eq!(
        one("John 3:16 NKJ").2,
        vec!["version 'NKJ' read as NKJV (spelling corrected)"]
    );
    let r = parse("John 3:16 (NIVV)").references.remove(0);
    assert_eq!(r.version, None);
    assert_eq!(r.warnings, vec!["unrecognized version label 'NIVV'"]);
    // a version in front of the book is not read
    assert_eq!(parse("NASB John 3:16").references[0].version, None);
    // real versions we have no name for are kept as typed
    assert_eq!(one("John 3:16 HCSB").0.as_deref(), Some("HCSB"));
}

#[test]
fn configured_aliases_replace_the_default() {
    let aliases: IndexMap<String, String> = [("NKJ".to_string(), "NKJV".to_string())].into();
    let r = parse_references_with("John 3:16 YLT", Some(&aliases))
        .unwrap()
        .references
        .remove(0);
    // without the default {"YLT": "NLT"} the label is just YLT, a version we know
    assert_eq!(r.version.as_deref(), Some("YLT"));
    let r = parse_references_with("John 3:16 NKJ", Some(&aliases))
        .unwrap()
        .references
        .remove(0);
    assert_eq!(r.version.as_deref(), Some("NKJV"));
    assert_eq!(
        r.warnings,
        vec!["version 'NKJ' read as NKJV (configured alias)"]
    );
    // keys match without regard to case; an alias to something unknown is ignored
    let aliases: IndexMap<String, String> = [
        ("ylt".to_string(), "NIV".to_string()),
        ("XYZ".to_string(), "NOPE".to_string()),
    ]
    .into();
    let r = parse_references_with("John 3:16 YLT", Some(&aliases))
        .unwrap()
        .references
        .remove(0);
    assert_eq!(r.version.as_deref(), Some("NIV"));
    let r = parse_references_with("John 3:16 (XYZ)", Some(&aliases))
        .unwrap()
        .references
        .remove(0);
    assert_eq!(r.version, None);
}

#[test]
fn unicode_is_read_the_way_python_reads_it() {
    // fullwidth digits and punctuation go through NFKC; offsets are into the normalized text, applied to the source
    assert_eq!(normalized("John ３：１６"), vec!["John 3:16"]);
    assert_eq!(normalized("John 3:16\u{a0}KJV"), vec!["John 3:16 KJV"]);
    assert_eq!(normalized("John 3:16−18"), vec!["John 3:16-18"]); // minus sign
                                                                  // Arabic-Indic digits are digits for `\d` and `int()`
    assert_eq!(normalized("John ٣:١٦"), vec!["John 3:16"]);
    // re.IGNORECASE equates dotless i and dotted capital I with i
    assert_eq!(normalized("Isaıah 53:5"), vec!["Isaiah 53:5"]);
    // a letter that is not a letter of the alphabet breaks the word
    assert_eq!(normalized("Johné 3:16"), Vec::<String>::new());
}

#[test]
fn numbers_too_big_for_64_bits_are_still_an_issue() {
    let (references, issues) = shown("John 99999999999999999999:1");
    assert!(references.is_empty());
    assert_eq!(issues.len(), 1);
    assert!(issues[0].starts_with("John has no chapter "));
}

#[test]
fn the_json_shape() {
    let value = parse_references("Eph 2:8-9 NIV", None).unwrap();
    assert_eq!(
        value,
        serde_json::json!({
            "references": [{
                "book_display_name": "Ephesians", "book_id": "EPH", "normalized_text": "Ephesians 2:8-9 NIV",
                "source_end": 13, "source_start": 0, "source_text": "Eph 2:8-9 NIV",
                "spans": [{"end": {"chapter": 2, "verse": 9}, "start": {"chapter": 2, "verse": 8}}],
                "version": "NIV", "warnings": []
            }],
            "issues": []
        })
    );
}

#[test]
fn verse_keys() {
    let verse = BibleVerse::new("EPH", 2, 9, "not of works", "NKJV");
    assert_eq!(verse.key(), "EPH.2.9");
    let json = serde_json::to_value(&verse).unwrap();
    assert_eq!(json["poetry_indent"], 0);
    assert!(json["copyright"].is_null());
}

#[test]
fn format_reference_chains_chapters_like_python() {
    let mut r = parse("John 3:16-18 & 20 & 4:1-5:2 KJV")
        .references
        .remove(0);
    assert_eq!(r.normalized_text, "John 3:16-18 & 20 & 4:1-5:2 KJV");
    r.version = None;
    assert_eq!(format_reference(&r), "John 3:16-18 & 20 & 4:1-5:2");
}
