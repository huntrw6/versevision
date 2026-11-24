//! Tests of what the oracle corpus does not reach: review choices (skip, include, reject fixes on points of several slides), the
//! version walk, the texts the planner shows for a verse it could not add, and the pieces of point formatting.
//! Expected strings were checked against the Python functions (`_title_case`, `_strip_period`, `format_point_slides`, ...).

use indexmap::IndexMap;
use pollster::block_on;

use super::*;
use crate::bibles::{
    BibleError, BibleProvider, BibleVerse, InMemoryBible, ProviderHealth, ProviderRegistry,
    ResolvedPassage,
};
use crate::config::{AppConfig, Numbering, VersionSource};
use crate::notes::models::{PointItem, PointRole};
use crate::notes::{TraceLog, TraceSink};
use crate::plan::{
    DecisionKind, NotesImportResult, PlanError, PlanOptions, ReviewChoices, SlideKind, Tier,
};
use crate::references::PassageReference;
use crate::versions::BibleVersion;
use crate::workflow::points::{is_numbered_label, strip_period, title_case};

/// One verse of text for every verse the notes might ask for, in every version: `"<V> <Book> <c>:<v>"`.
fn bible(versions: &[&str], books: &[&str]) -> InMemoryBible {
    let mut table = IndexMap::new();
    for version in versions {
        for book in books {
            for chapter in 1..=3 {
                for verse in 1..=20 {
                    table.insert(
                        format!("{version} {book} {chapter}:{verse}"),
                        format!("{version} text of {book} {chapter}:{verse}."),
                    );
                }
            }
        }
    }
    InMemoryBible::new(table)
}

fn plan_with(
    text: &str,
    config: &AppConfig,
    default_version: Option<&str>,
    bible: &InMemoryBible,
) -> (Result<NotesImportResult, PlanError>, TraceLog) {
    let options = PlanOptions {
        default_version: default_version.map(str::to_string),
        no_fetch: false,
    };
    let mut log = TraceLog::default();
    let result = block_on(plan_notes(text, config, &options, Some(bible), &mut log));
    (result, log)
}

fn plan(text: &str) -> NotesImportResult {
    plan_cfg(text, &AppConfig::default())
}

fn plan_cfg(text: &str, config: &AppConfig) -> NotesImportResult {
    let b = bible(
        &["NKJV", "NIV", "KJV", "ESV"],
        &["John", "Romans", "Psalms", "Genesis"],
    );
    plan_with(text, config, Some("NKJV"), &b).0.unwrap()
}

fn choose(skip: &[usize], include: &[usize], reject: &[usize]) -> ReviewChoices {
    ReviewChoices {
        skip: skip.iter().copied().collect(),
        include: include.iter().copied().collect(),
        reject_fixes: reject.iter().copied().collect(),
    }
}

fn after(result: &NotesImportResult, choices: &ReviewChoices) -> (Vec<String>, TraceLog) {
    let mut log = TraceLog::default();
    let plan = apply_review_choices(result, choices, &mut log);
    (
        plan.slides.iter().map(|s| s.body_text.clone()).collect(),
        log,
    )
}

fn texts(result: &NotesImportResult) -> Vec<String> {
    result
        .slide_plan
        .slides
        .iter()
        .map(|s| s.body_text.clone())
        .collect()
}

fn config_with(settings: &str) -> AppConfig {
    AppConfig::from_toml_str(settings).unwrap()
}

const KEYS: &str = "KEY#1) Faith\nKEY #2) Hope\nKEY #3) Love\nKEY #4) Joy\n";

// ---------------------------------------------------------------------------------------------------------- review choices

#[test]
fn skip_switches_off_every_slide_of_a_decision() {
    let config = config_with("[slides]\nmax_body_lines = 1\nline_width_units = 12.0\n");
    let result = plan_cfg("John 3:1\n1) Faith\nRomans 8:1\n", &config);
    let verse = &result.decisions[0];
    assert!(
        verse.slide_count > 1,
        "the verse is cut into several slides"
    );
    let (slides, _) = after(&result, &choose(&[verse.id], &[], &[]));
    assert_eq!(
        slides.len(),
        result.slide_plan.slides.len() - verse.slide_count
    );
    assert!(slides.iter().all(|s| !s.contains("John 3:1")));
    assert!(slides[0].starts_with("1) Faith"), "{slides:?}");
}

#[test]
fn skip_of_a_point_leaves_the_others_in_order() {
    let result = plan("1) One\n2) Two\n3) Three\n");
    let (slides, log) = after(&result, &choose(&[1], &[], &[]));
    assert_eq!(slides, ["1) One", "3) Three"]);
    assert!(log.records.is_empty(), "skipping writes no trace record");
}

#[test]
fn unknown_ids_and_decisions_without_slides_change_nothing() {
    let result = plan("1) One\n(pause)\n2) Two\n");
    let left_out = result
        .decisions
        .iter()
        .find(|d| d.kind == DecisionKind::LeftOut)
        .unwrap();
    let (slides, _) = after(&result, &choose(&[left_out.id, 99], &[99], &[99]));
    assert_eq!(slides, texts(&result));
}

#[test]
fn include_puts_a_left_out_line_where_it_stood() {
    let result = plan("1) One\n(pause)\n2) Two\n");
    let cue = result
        .decisions
        .iter()
        .find(|d| d.kind == DecisionKind::LeftOut)
        .unwrap();
    assert_eq!(cue.first_slide, 1);
    assert_eq!(cue.add_text, "• (pause)");
    let (slides, log) = after(&result, &choose(&[], &[cue.id], &[]));
    assert_eq!(slides, ["1) One", "• (pause)", "2) Two"]);
    let (stage, fields) = &log.records[0];
    assert_eq!(stage, "review_included");
    assert_eq!(fields["added"][0]["at_slide"], 2);
    assert_eq!(fields["added"][0]["id"], cue.id);
}

#[test]
fn an_included_line_gets_a_source_index_beyond_the_document() {
    let result = plan("1) One\n(pause)\n");
    let cue = result
        .decisions
        .iter()
        .find(|d| d.kind == DecisionKind::LeftOut)
        .unwrap();
    let mut log = TraceLog::default();
    let plan = apply_review_choices(&result, &choose(&[], &[cue.id], &[]), &mut log);
    let added = plan.slides.last().unwrap();
    assert_eq!(added.kind, SlideKind::Point);
    assert_eq!(added.source_passage_index, 100_000 + cue.id);
    assert_eq!(added.reference_text, "");
    assert!(added.verse_keys.is_empty());
}

#[test]
fn include_at_the_end_of_the_document_appends() {
    let result = plan("1) One\n(pause)\n");
    let cue = result
        .decisions
        .iter()
        .find(|d| d.kind == DecisionKind::LeftOut)
        .unwrap();
    assert_eq!(cue.first_slide, result.slide_plan.slides.len());
    let (slides, log) = after(&result, &choose(&[], &[cue.id], &[]));
    assert_eq!(slides, ["1) One", "• (pause)"]);
    assert_eq!(log.records[0].1["added"][0]["at_slide"], 2);
}

#[test]
fn include_of_a_long_line_adds_one_slide_per_piece_in_order() {
    let config = config_with("[points]\nmax_lines_per_slide = 1\nline_width_units = 20.0\n");
    let text = "1) One\n(Trust in the Lord with all your heart and lean not on your own understanding)\n2) Two\n";
    let result = plan_cfg(text, &config);
    let cue = result
        .decisions
        .iter()
        .find(|d| d.kind == DecisionKind::LeftOut)
        .unwrap();
    let pieces: Vec<&str> = cue.add_text.split('\u{c}').collect();
    assert_eq!(pieces.len(), 4, "{:?}", cue.add_text);
    let (slides, log) = after(&result, &choose(&[], &[cue.id], &[]));
    assert_eq!(slides.len(), result.slide_plan.slides.len() + 4);
    assert_eq!(slides[0], "1) One");
    assert_eq!(slides[1..5], pieces);
    assert_eq!(slides[5], "2) Two");
    assert_eq!(
        log.records[0].1["added"][0]["text"],
        cue.add_text.as_str(),
        "one record, the whole text"
    );
}

#[test]
fn include_ignores_a_decision_that_has_slides() {
    let result = plan("1) One\n2) Two\n");
    let (slides, log) = after(&result, &choose(&[], &[0, 1], &[]));
    assert_eq!(slides, texts(&result));
    assert!(log.records.is_empty());
}

#[test]
fn skip_and_include_work_together() {
    let result = plan("1) One\n(pause)\n2) Two\n");
    let cue = result.decisions[1].id;
    let (slides, _) = after(&result, &choose(&[0], &[cue], &[]));
    assert_eq!(slides, ["• (pause)", "2) Two"]);
}

#[test]
fn reject_fixes_puts_the_point_in_as_written() {
    let result = plan(KEYS);
    let fix = &result.decisions[0];
    let correction = fix
        .correction
        .as_ref()
        .expect("KEY#1) is corrected to match the rest");
    assert_eq!(
        (correction.from.as_str(), correction.to.as_str()),
        ("KEY#1)", "KEY #1)")
    );
    assert_eq!(fix.tier, Tier::Error);
    assert_eq!(result.slide_plan.slides[0].body_text, "KEY #1) Faith");
    let (slides, log) = after(&result, &choose(&[], &[], &[fix.id]));
    assert_eq!(slides[0], "KEY#1) Faith");
    assert_eq!(&slides[1..], &texts(&result)[1..]);
    let (stage, fields) = &log.records[0];
    assert_eq!(stage, "review_fix_rejected");
    assert_eq!(fields["kept"], "KEY#1)");
    assert_eq!(fields["instead_of"], "KEY #1)");
    assert_eq!(fields["line"], 1);
}

#[test]
fn reject_fixes_on_a_point_of_several_slides_restores_each_slide() {
    let config = config_with("[points]\nmax_lines_per_slide = 1\nline_width_units = 20.0\n");
    let text = "KEY#1) Trust in the Lord with all your heart and lean not\nKEY #2) Hope\nKEY #3) Love\nKEY #4) Joy\n";
    let result = plan_cfg(text, &config);
    let fix = &result.decisions[0];
    assert!(fix.slide_count > 1);
    let original = fix.correction.as_ref().unwrap().original_slides.clone();
    assert_eq!(original.len(), fix.slide_count);
    assert!(original[0].starts_with("KEY#1)"));
    let (slides, _) = after(&result, &choose(&[], &[], &[fix.id]));
    assert_eq!(slides[..fix.slide_count], original[..]);
    assert_eq!(slides[fix.slide_count..], texts(&result)[fix.slide_count..]);
    assert_eq!(slides.len(), result.slide_plan.slides.len());
}

#[test]
fn reject_fixes_on_a_decision_with_no_correction_does_nothing() {
    let result = plan("1) One\n2) Two\n");
    let (slides, log) = after(&result, &choose(&[], &[], &[0, 1]));
    assert_eq!(slides, texts(&result));
    assert!(log.records.is_empty());
}

#[test]
fn skip_wins_over_a_rejected_fix_and_the_result_is_untouched() {
    let result = plan(KEYS);
    let before = result.clone();
    let (slides, log) = after(&result, &choose(&[0], &[], &[0]));
    assert_eq!(slides, ["KEY #2) Hope", "KEY #3) Love", "KEY #4) Joy"]);
    assert_eq!(log.records.len(), 1, "the rejection is still recorded");
    assert_eq!(
        result, before,
        "apply_review_choices does not change the plan it is given"
    );
}

// ---------------------------------------------------------------------------------------------------------- the version walk

fn versions_of(result: &NotesImportResult) -> Vec<String> {
    result
        .slide_plan
        .slides
        .iter()
        .filter(|s| s.kind == SlideKind::Verse)
        .map(|s| s.version_text.clone())
        .collect()
}

#[test]
fn an_explicit_version_carries_down_until_the_next_one() {
    let result = plan("John 3:1\nRomans 3:1 NIV\nJohn 3:2\nPsalm 2:1 KJV\nJohn 3:3\n");
    assert_eq!(versions_of(&result), ["NKJV", "NIV", "NIV", "KJV", "KJV"]);
    let notes: Vec<&str> = result.notices.iter().map(|n| n.problem.as_str()).collect();
    assert_eq!(
        notes,
        [
            "no version listed; used NKJV (default version)",
            "no version listed; used NIV (nearest version above)",
            "no version listed; used KJV (nearest version above)",
        ]
    );
}

#[test]
fn a_comparison_pair_does_not_set_the_version_for_later_verses() {
    let result = plan("John 3:16 NIV\nJohn 3:16 KJV\nRomans 3:1\n");
    assert_eq!(versions_of(&result), ["NIV", "KJV", "NKJV"]);
}

#[test]
fn a_pair_of_different_verses_does_set_it() {
    let result = plan("John 3:16 NIV\nJohn 3:17 KJV\nRomans 3:1\n");
    assert_eq!(versions_of(&result), ["NIV", "KJV", "KJV"]);
}

#[test]
fn version_source_default_ignores_the_version_above() {
    let mut config = AppConfig::default();
    config.verses.version_source = VersionSource::Default;
    let result = plan_cfg("John 3:1 NIV\nJohn 3:2\n", &config);
    assert_eq!(versions_of(&result), ["NIV", "NKJV"]);
    assert_eq!(
        result.notices[0].problem,
        "no version listed; used NKJV (default version)"
    );
}

#[test]
fn version_source_required_refuses_a_verse_without_a_version() {
    let mut config = AppConfig::default();
    config.verses.version_source = VersionSource::Required;
    let result = plan_cfg("John 3:1\nJohn 3:2 NIV\n", &config);
    let why = "no version on this verse, and your setting requires one (verses.version_source = required)";
    assert_eq!(result.failed[0].problem, why);
    assert_eq!(result.decisions[0].kind, DecisionKind::Error);
    assert_eq!(result.decisions[0].reasons, [why]);
    assert_eq!(result.decisions[0].text, "John 3:1");
    assert_eq!(result.decisions[1].tier, Tier::Good);
}

#[test]
fn no_version_anywhere_says_so() {
    let b = bible(&["NKJV"], &["John"]);
    let mut config = AppConfig::default();
    config.bible.default_version = None;
    let (none, _) = plan_with("John 3:1\n", &config, None, &b);
    assert_eq!(
        none.unwrap().failed[0].problem,
        "no version on this verse, none above it, and no default version is set"
    );
    let mut by_default = config.clone();
    by_default.verses.version_source = VersionSource::Default;
    let (none, _) = plan_with("John 3:1\n", &by_default, None, &b);
    assert_eq!(
        none.unwrap().failed[0].problem,
        "no version on this verse and no default version is set"
    );
}

#[test]
fn an_unrecognized_version_label_is_noted_and_ignored() {
    let result = plan("John 3:1 NIVV\n");
    assert_eq!(versions_of(&result), ["NKJV"]);
    let problems: Vec<&str> = result.notices.iter().map(|n| n.problem.as_str()).collect();
    assert!(
        problems[0].starts_with("unrecognized version label 'NIVV'"),
        "{problems:?}"
    );
    assert!(problems[0]
        .ends_with("; ignored, so the verse uses the nearest version above or the default"));
}

#[test]
fn an_unknown_default_version_is_an_error() {
    let b = bible(&["NKJV"], &["John"]);
    let (result, log) = plan_with("John 3:1\n", &AppConfig::default(), Some("NIVV"), &b);
    let error = result.unwrap_err();
    assert_eq!(error, PlanError::UnknownVersion("NIVV".into()));
    assert_eq!(error.to_string(), "Unknown Bible version: NIVV");
    assert!(log.records.iter().all(|(stage, _)| stage != "version"));
}

#[test]
fn the_saved_default_is_used_when_none_is_given() {
    let b = bible(&["NIV"], &["John"]);
    let mut config = AppConfig::default();
    config.bible.default_version = Some("niv".into());
    let (result, _) = plan_with("John 3:1\n", &config, None, &b);
    assert_eq!(versions_of(&result.unwrap()), ["NIV"]);
}

#[test]
fn the_quote_decides_the_version_when_another_wording_matches() {
    // KJV and NKJV differ; the notes quote the KJV wording of John 3:1 and name no version
    let mut table = IndexMap::new();
    table.insert(
        "NKJV John 3:1".to_string(),
        "Now there was a man of the Pharisees named Nicodemus".to_string(),
    );
    table.insert(
        "KJV John 3:1".to_string(),
        "There was a man of the Pharisees named Nicodemus a ruler".to_string(),
    );
    let b = InMemoryBible::new(table);
    let mut config = AppConfig::default();
    config.verses.quote_versions = vec!["KJV".into()];
    let text = "“There was a man of the Pharisees named Nicodemus a ruler”\nJohn 3:1\n";
    let (result, log) = plan_with(text, &config, Some("NKJV"), &b);
    let result = result.unwrap();
    assert_eq!(versions_of(&result), ["KJV"]);
    let d = &result.decisions[1];
    assert_eq!(d.tier, Tier::Warning);
    assert_eq!(
        d.reasons,
        ["no version listed; the quoted text matches KJV, not NKJV (used KJV; turn off with verses.version_from_quote=off)"]
    );
    let check = log
        .records
        .iter()
        .find(|(s, _)| s == "quote_check")
        .unwrap();
    assert!(check.1["result"]
        .as_str()
        .unwrap()
        .starts_with("the notes quote KJV wording (score 1.00 vs "));
    // the provider was asked twice: NKJV first, then KJV
    assert_eq!(
        log.records.iter().filter(|(s, _)| s == "provider").count(),
        2
    );
}

#[test]
fn a_quote_that_matches_nothing_is_reported_with_the_percentage() {
    let mut table = IndexMap::new();
    table.insert(
        "NKJV John 3:1".to_string(),
        "alpha beta gamma delta".to_string(),
    );
    table.insert(
        "KJV John 3:1".to_string(),
        "alpha beta gamma delta".to_string(),
    );
    let b = InMemoryBible::new(table);
    let mut config = AppConfig::default();
    config.verses.quote_versions = vec!["KJV".into()];
    let text = "“completely different words here today”\nJohn 3:1\n";
    let (result, _) = plan_with(text, &config, Some("NKJV"), &b);
    let d = &block_on(async { result.unwrap() }).decisions[1];
    assert_eq!(
        d.reasons.last().unwrap(),
        "the quoted text in the notes does not match NKJV wording (match 0%); check the version"
    );
}

// ---------------------------------------------------------------------------------------------------------- what the planner says

#[test]
fn a_verse_with_no_text_is_not_added_and_says_which_key() {
    let b = bible(&["NKJV"], &["John"]);
    let (result, _) = plan_with("Romans 8:28 ESV\n", &AppConfig::default(), Some("NKJV"), &b);
    let result = result.unwrap();
    assert_eq!(
        result.failed[0].problem,
        "not added: no verse text for ESV Romans 8:28"
    );
    assert_eq!(result.failed[0].text, "Romans 8:28 ESV");
    assert_eq!(result.decisions[0].kind, DecisionKind::Error);
    assert_eq!(
        result.decisions[0].reasons,
        ["not added: no verse text for ESV Romans 8:28"]
    );
    assert!(result.slide_plan.slides.is_empty());
    assert_eq!(result.verse_count, 0);
}

#[test]
fn an_inherited_version_adds_a_hint_to_the_error() {
    let b = bible(&["NKJV"], &["John"]);
    let (result, _) = plan_with(
        "John 3:1 NKJV\nRomans 8:28\n",
        &AppConfig::default(),
        Some("NKJV"),
        &b,
    );
    let result = result.unwrap();
    assert_eq!(
        result.failed[0].problem,
        "not added: no verse text for NKJV Romans 8:28 (version NKJV was taken from the verse above; add a version to this verse to change it)"
    );
}

#[test]
fn a_span_across_chapters_names_the_first_missing_verse() {
    let mut table = IndexMap::new();
    table.insert("NKJV Genesis 1:31".to_string(), "x".to_string());
    let b = InMemoryBible::new(table);
    let (result, _) = plan_with(
        "Genesis 1:31-2:2\n",
        &AppConfig::default(),
        Some("NKJV"),
        &b,
    );
    assert_eq!(
        result.unwrap().failed[0].problem,
        "not added: no verse text for NKJV Genesis 2:1 (version NKJV was taken from the verse above; add a version to this verse to change it)"
    );
}

#[test]
fn a_provider_that_returns_nothing_is_an_error_of_the_verse() {
    struct Empty;
    impl crate::bibles::BiblePassages for Empty {
        async fn fetch(
            &self,
            reference: &PassageReference,
            _: &mut dyn TraceSink,
        ) -> Result<ResolvedPassage, BibleError> {
            Ok(ResolvedPassage {
                reference: reference.clone(),
                resolved_version: reference.version.clone().unwrap_or_default(),
                verses: Vec::new(),
            })
        }
        fn description(&self) -> String {
            "empty".into()
        }
    }
    let mut log = TraceLog::default();
    let result = block_on(plan_notes(
        "John 3:1\n",
        &AppConfig::default(),
        &PlanOptions {
            default_version: Some("NKJV".into()),
            no_fetch: false,
        },
        Some(&Empty),
        &mut log,
    ))
    .unwrap();
    assert!(
        result.failed[0]
            .problem
            .starts_with("not added: provider returned no verses ("),
        "{}",
        result.failed[0].problem
    );
}

#[test]
fn a_run_with_no_source_and_no_no_fetch_is_refused() {
    let mut log = TraceLog::default();
    let none: Option<&InMemoryBible> = None;
    let error = block_on(plan_notes(
        "John 3:1\n",
        &AppConfig::default(),
        &PlanOptions::default(),
        none,
        &mut log,
    ))
    .unwrap_err();
    assert_eq!(error, PlanError::NoBibleSource);
}

#[test]
fn no_fetch_plans_placeholders_without_asking_anyone() {
    let mut log = TraceLog::default();
    let none: Option<&InMemoryBible> = None;
    let options = PlanOptions {
        default_version: Some("NKJV".into()),
        no_fetch: true,
    };
    let result = block_on(plan_notes(
        "John 3:16-17\n",
        &AppConfig::default(),
        &options,
        none,
        &mut log,
    ))
    .unwrap();
    assert!(!result.fetched);
    assert_eq!(result.slide_plan.slides.len(), 2);
    assert!(result
        .slide_plan
        .slides
        .iter()
        .all(|s| s.body_text.is_empty()));
    assert!(log.records.iter().all(|(stage, _)| stage != "provider"));
}

#[test]
fn a_number_style_that_cannot_be_formatted_stops_the_run() {
    let config = config_with("[points]\nnumbering = \"renumber\"\nnumber_style = \"{x}\"\n");
    let b = bible(&["NKJV"], &["John"]);
    let (result, _) = plan_with("1) One\n2) Two\n", &config, Some("NKJV"), &b);
    assert!(
        matches!(result, Err(PlanError::NumberStyle { .. })),
        "{result:?}"
    );
    let ok = config_with("[points]\nnumbering = \"renumber\"\nnumber_style = \"#{n}:\"\n");
    let result = plan_cfg("1) One\n2) Two\n", &ok);
    assert_eq!(texts(&result), ["#1: One", "#2: Two"]);
}

#[test]
fn a_whole_chapter_and_an_invalid_reference_are_listed_for_the_review() {
    let result = plan("Psalm 23\n1) One\n");
    assert_eq!(
        result.review[0].problem,
        "whole-chapter reference; add verse numbers to include it"
    );
    assert_eq!(result.decisions[0].kind, DecisionKind::Error);
    assert_eq!(result.decisions[0].add_text, "• Psalm 23");
    assert_eq!(result.decisions[0].first_slide, 0);
}

#[test]
fn a_long_reference_warns_that_it_may_be_cut_off() {
    let config = config_with("[slides]\nreference_width_units = 6.0\nmax_reference_lines = 1\n");
    let result = plan_cfg("Romans 3:13\n", &config);
    let reason = result.decisions[0].reasons.last().unwrap();
    assert_eq!(
        reason,
        "the reference \"Romans 3:13 (NKJV)\" is longer than 1 line in the slide's reference box and may be cut off or shrink"
    );
    let config = config_with("[slides]\nreference_width_units = 6.0\nmax_reference_lines = 2\n");
    let result = plan_cfg("Romans 3:13\n", &config);
    assert!(result.decisions[0]
        .reasons
        .last()
        .unwrap()
        .contains("is longer than 2 lines in"));
}

// ---------------------------------------------------------------------------------------------------------- the provider registry

struct Fake {
    versions: Vec<BibleVersion>,
}

impl BibleProvider for Fake {
    async fn list_versions(&self) -> Result<Vec<BibleVersion>, BibleError> {
        Ok(self.versions.clone())
    }
    async fn fetch_passage(
        &self,
        version: &str,
        book_id: &str,
        _: &[crate::references::VerseSpan],
    ) -> Result<Vec<BibleVerse>, BibleError> {
        Ok(vec![BibleVerse::new(
            book_id,
            3,
            16,
            "For God so loved",
            version,
        )])
    }
    async fn health_check(&self) -> ProviderHealth {
        ProviderHealth {
            healthy: true,
            message: "ok".into(),
            provider: Some("fake".into()),
            versions: vec![],
        }
    }
}

fn available(abbreviation: &str, id: Option<&str>) -> BibleVersion {
    BibleVersion {
        abbreviation: abbreviation.into(),
        name: abbreviation.into(),
        language: "en".into(),
        provider: Some("fake".into()),
        provider_version_id: id.map(str::to_string),
        copyright: None,
        attribution: None,
    }
}

fn registry(routes: &[(&str, &str)], versions: Vec<BibleVersion>) -> ProviderRegistry<Fake> {
    let routes = routes
        .iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect();
    let mut providers = IndexMap::new();
    providers.insert("fake".to_string(), Fake { versions });
    ProviderRegistry::new(routes, providers)
}

fn fetch_text(
    registry: &ProviderRegistry<Fake>,
    version: Option<&str>,
) -> (Result<String, BibleError>, TraceLog) {
    use crate::bibles::BiblePassages;
    let mut reference = crate::references::parse_references_with("John 3:16", None)
        .unwrap()
        .references
        .remove(0);
    reference.version = version.map(str::to_string);
    let mut log = TraceLog::default();
    let result = block_on(registry.fetch(&reference, &mut log))
        .map(|p| format!("{}:{}", p.resolved_version, p.verses[0].text));
    (result, log)
}

#[test]
fn the_registry_routes_a_version_by_name_ignoring_case() {
    let r = registry(&[("niv", "fake")], vec![available("NIV", Some("x"))]);
    let (result, log) = fetch_text(&r, Some("NIV"));
    assert_eq!(result.unwrap(), "NIV:For God so loved");
    let fields = &log.records[0].1;
    assert_eq!(
        (fields["provider"].as_str(), fields["source"].as_str()),
        (Some("fake"), Some("live"))
    );
}

#[test]
fn the_registry_error_texts() {
    let r = registry(
        &[("NIV", "fake"), ("KJV", "gone")],
        vec![available("NIV", None), available("ESV", Some("e"))],
    );
    let say = |version: Option<&str>| fetch_text(&r, version).0.unwrap_err();
    assert_eq!(
        say(Some("ESV")).to_string(),
        "No Bible provider route is configured for ESV"
    );
    assert_eq!(say(Some("ESV")).python_exception(), "ConfigurationError");
    assert_eq!(
        say(Some("KJV")).to_string(),
        "Bible provider route KJV points to unknown provider gone"
    );
    assert_eq!(
        say(Some("NIV")).to_string(),
        "Provider fake does not expose NIV"
    );
    assert_eq!(
        say(None).to_string(),
        "Cannot fetch a reference without a resolved version"
    );
    assert_eq!(say(None).python_exception(), "BibleProviderError");
    let (_, log) = fetch_text(&r, Some("ESV"));
    assert_eq!(
        log.records[0].1["error"],
        "No Bible provider route is configured for ESV"
    );
    assert_eq!(log.records[0].1["version"], "ESV");
}

// ---------------------------------------------------------------------------------------------------------- point formatting

fn point(label: &str, role: PointRole, text: &str) -> PointItem {
    PointItem {
        line: 1,
        role,
        label: label.into(),
        text: text.into(),
        warnings: vec![],
        detail_end: 1,
        label_fixed_from: String::new(),
    }
}

#[test]
fn numbered_labels() {
    for yes in [
        "1)",
        "#2:",
        "a)",
        "KEY #3)",
        "KEY#3-",
        "AB1.",
        "12:",
        "KEY  # 3)",
        "٣)",
    ] {
        assert!(is_numbered_label(yes), "{yes}");
    }
    for no in ["a", "1", "#", "A)b", "•", "", "K1)", "1 )", "KEY #3))"] {
        assert!(!is_numbered_label(no), "{no}");
    }
}

#[test]
fn title_case_matches_python() {
    assert_eq!(
        title_case("the king of kings and lord of lords"),
        "The King of Kings and Lord of Lords"
    );
    assert_eq!(title_case("A TALE of two cities"), "A Tale of Two Cities");
    assert_eq!(
        title_case("mixed Case Of the lord"),
        "Mixed Case Of the Lord"
    );
    assert_eq!(title_case("ß strasse"), "SS Strasse");
    assert_eq!(title_case("x  y"), "X  Y");
    assert_eq!(title_case("the"), "The");
}

#[test]
fn the_closing_full_stop_goes_unless_it_belongs() {
    for (text, want) in [
        ("Go to the Lord.", "Go to the Lord"),
        ("Amen.)", "Amen)"),
        ("He said \"stop.\"", "He said \"stop\""),
        ("I am.", "I am"),
        ("end. ", "end"),
        ("(5.)", "(5)"),
        ("See Dr.", "See Dr."),
        ("etc.", "etc."),
        ("wait...", "wait..."),
        ("Chapter 12.", "Chapter 12."),
        ("Vol. 2.", "Vol. 2."),
        ("a.b.", "a.b."),
        ("x.", "x."),
        ("it was 5.", "it was 5."),
    ] {
        assert_eq!(strip_period(text), want, "{text:?}");
    }
}

#[test]
fn renumbering_keeps_a_key_word_and_uses_the_style_otherwise() {
    let cfg = config_with("[points]\nnumbering = \"renumber\"\nnumber_style = \"{n}.\"\n").points;
    let say = |label: &str, role| {
        format_point(
            &point(label, role, "Trust the Lord with all your heart."),
            &cfg,
            3,
        )
        .unwrap()
    };
    assert_eq!(
        say("KEY #7)", PointRole::Point),
        "KEY #3) Trust the Lord\nwith all your heart"
    );
    assert_eq!(
        say("4)", PointRole::Point),
        "3. Trust the Lord\nwith all your heart"
    );
    assert_eq!(
        say("a)", PointRole::ListItem),
        "a) Trust the Lord\nwith all your heart"
    );
    assert_eq!(
        say("•", PointRole::ListItem),
        "• Trust the Lord\nwith all your heart"
    );
    assert_eq!(
        say("", PointRole::SlideNote),
        "Trust the Lord with all your heart"
    );
    assert_eq!(cfg.numbering, Numbering::Renumber);
}

#[test]
fn a_point_longer_than_max_lines_continues_on_the_next_slide() {
    let cfg = config_with("[points]\nmax_lines_per_slide = 1\nline_width_units = 20.0\n").points;
    let item = point(
        "1)",
        PointRole::Point,
        "Trust in the Lord with all your heart and lean not on your own understanding",
    );
    assert_eq!(
        format_point_slides(&item, &cfg, 1).unwrap(),
        [
            "1) Trust in the Lord",
            "with all your heart",
            "and lean not on your",
            "own understanding"
        ]
    );
    assert_eq!(
        added_point_text("  Trust in the Lord with all your heart and lean not  on your own understanding.", &cfg).unwrap(),
        "• Trust in the Lord\u{c}with all your heart\u{c}and lean not on your\u{c}own understanding"
    );
}

#[test]
fn label_typos_need_three_headers_and_a_clear_majority() {
    let three = plan("KEY#1) A\nKEY #2) B\nKEY #3) C\n");
    assert!(
        three.decisions[0].correction.is_some(),
        "1 of 3 differs from a majority of 2"
    );
    let split = plan("KEY#1) A\nKEY #2) B\nKEY#3) C\nKEY #4) D\n");
    assert!(
        split.decisions.iter().all(|d| d.correction.is_none()),
        "2 against 2 is no majority"
    );
    let two = plan("KEY#1) A\nKEY #2) B\n");
    assert!(
        two.decisions.iter().all(|d| d.correction.is_none()),
        "fewer than 3 headers"
    );
    let off = plan_cfg(KEYS, &config_with("[points]\nfix_label_typos = false\n"));
    assert!(off.decisions.iter().all(|d| d.correction.is_none()));
    assert_eq!(off.slide_plan.slides[0].body_text, "KEY#1) Faith");
}

#[test]
fn the_report_counts_what_the_review_page_shows() {
    let result = plan(&format!(
        "(pause)\n{KEYS}Psalm 23\nJohn 3:1\nRomans 8:28 NIVV\n"
    ));
    let report = build_report(&result, "notes.txt");
    assert_eq!(report.source, "notes.txt");
    assert_eq!(report.summary.slides, result.slide_plan.slides.len());
    assert_eq!(report.summary.fixes, 1);
    assert_eq!(report.summary.needs_review, 1);
    assert_eq!(report.summary.flagged, report.flagged.len());
    assert!(
        report.flagged.windows(2).all(|w| w[0].line <= w[1].line),
        "flagged is sorted by line"
    );
    assert_eq!(report.clean, report.flagged.is_empty());
    assert!(!report.clean);
    let cue = report.decisions.iter().find(|d| d.addable).unwrap();
    assert_eq!(cue.slides, Vec::<usize>::new());
}

#[test]
fn left_out_reasons_are_firm_or_a_warning() {
    assert_eq!(left_out_tier("stage/production cue"), Tier::Good);
    assert_eq!(
        left_out_tier("quoted text with no reference"),
        Tier::Warning
    );
    assert_eq!(left_out_tier("header: title / date line"), Tier::Good);
    assert_eq!(left_out_tier("something else"), Tier::Warning);
}

// ---------------------------------------------------------------------------------------------------------- slides and the trace

/// A provider whose first verse carries a copyright line and whose second one only an attribution.
struct Licensed;

impl crate::bibles::BiblePassages for Licensed {
    async fn fetch(
        &self,
        reference: &PassageReference,
        _: &mut dyn TraceSink,
    ) -> Result<ResolvedPassage, BibleError> {
        let version = reference.version.clone().unwrap_or_default();
        let mut one = BibleVerse::new("JHN", 3, 16, "For God so loved the world", &version);
        one.copyright = Some("(c) A Publisher".into());
        let mut two = BibleVerse::new("JHN", 3, 17, "that He gave", &version);
        two.copyright = Some(String::new());
        two.attribution = Some("From a Library".into());
        Ok(ResolvedPassage {
            reference: reference.clone(),
            resolved_version: version,
            verses: vec![one, two],
        })
    }
    fn description(&self) -> String {
        "licensed".into()
    }
}

#[test]
fn copyright_comes_from_the_verse_or_its_attribution() {
    let mut log = TraceLog::default();
    let options = PlanOptions {
        default_version: Some("NKJV".into()),
        no_fetch: false,
    };
    let result = block_on(plan_notes(
        "John 3:16-17\n",
        &AppConfig::default(),
        &options,
        Some(&Licensed),
        &mut log,
    ))
    .unwrap();
    let slides = &result.slide_plan.slides;
    assert_eq!(slides[0].copyright_text.as_deref(), Some("(c) A Publisher"));
    assert_eq!(
        slides[1].copyright_text.as_deref(),
        Some("From a Library"),
        "an empty copyright falls back to the attribution"
    );
    let fetched = log
        .records
        .iter()
        .find(|(s, _)| s == "fetch_result")
        .unwrap();
    assert_eq!(fetched.1["copyright"], "(c) A Publisher");
    assert_eq!(fetched.1["verses"], 2);
}

#[test]
fn reference_label_modes() {
    let say = |mode: &str, text: &str| {
        let config = config_with(&format!("[slides]\nreference_label_mode = \"{mode}\"\n"));
        let result = plan_cfg(text, &config);
        result
            .slide_plan
            .slides
            .iter()
            .map(|s| s.reference_text.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(say("verse", "John 3:1-2 NIV\n"), ["John 3:1", "John 3:2"]);
    assert_eq!(
        say("segment", "John 3:1-2 NIV\n"),
        ["John 3:1-2", "John 3:1-2"]
    );
    assert_eq!(
        say("source", "John 3:1-2 NIV\n"),
        ["John 3:1-2", "John 3:1-2"]
    );
    assert_eq!(say("segment", "John 3:1, 3\n"), ["John 3:1", "John 3:3"]);
}

#[test]
fn a_continuation_slide_has_no_reference_when_asked() {
    let config = config_with("[slides]\nmax_body_lines = 1\nline_width_units = 12.0\nrepeat_reference_on_continuation = false\n");
    let result = plan_cfg("John 3:1\n", &config);
    let refs: Vec<&str> = result
        .slide_plan
        .slides
        .iter()
        .map(|s| s.reference_text.as_str())
        .collect();
    assert!(refs.len() > 1);
    assert_eq!(refs[0], "John 3:1");
    assert!(refs[1..].iter().all(|r| r.is_empty()));
}

#[test]
fn the_trace_cuts_long_lists_and_strings_like_the_run_log() {
    let notes: String = (1..=305).map(|n| format!("- point {n}\n")).collect();
    let b = bible(&["NKJV"], &["John"]);
    let (result, log) = plan_with(&notes, &AppConfig::default(), Some("NKJV"), &b);
    assert_eq!(result.unwrap().slide_plan.slides.len(), 305);
    let plan = &log.records.last().unwrap();
    assert_eq!(plan.0, "plan");
    assert_eq!(plan.1["slide_count"], 305);
    assert_eq!(plan.1["slides"].as_array().unwrap().len(), 300);
    let long = format!("- {}\n", "word ".repeat(500));
    let (result, log) = plan_with(&long, &AppConfig::default(), Some("NKJV"), &b);
    assert!(result.is_ok());
    let formatted = log
        .records
        .iter()
        .find(|(s, _)| s == "point_format")
        .unwrap();
    let text = formatted.1["text_in_notes"].as_str().unwrap();
    assert!(
        text.ends_with("... [499 more characters]"),
        "{}",
        &text[text.len() - 40..]
    );
}
