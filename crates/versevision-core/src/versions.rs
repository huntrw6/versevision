//! Bible version names (`versions/registry.py`, `models.py`, `resolver.py`) and the quote score the version check is built on
//! (`quote_score` of `workflow/notes_workflow.py`, which `oracle/versions.jsonl` also records).
//!
//! The walk that gives every verse of a document a version (nearest version above, default, required) is the planner's (S3);
//! what is here is what it asks of a single reference or of a pair of them: canonical names, aliases, spelling correction,
//! [`resolve_versions`] for a list of references, [`is_comparison_pair`], and [`quote_score`].

use std::collections::{BTreeSet, HashMap};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use thiserror::Error;

use crate::references::pytext::casefold;
use crate::references::PassageReference;

/// `_NAMES`: the versions the app knows by name, in the registry's order.
const NAMES: &[(&str, &str)] = &[
    ("NLT", "New Living Translation"),
    ("NIV", "New International Version"),
    ("ESV", "English Standard Version"),
    ("KJV", "King James Version"),
    ("NKJV", "New King James Version"),
    ("NASB", "New American Standard Bible"),
    ("NASB1995", "New American Standard Bible 1995"),
    ("NASB2020", "New American Standard Bible 2020"),
    ("CSB", "Christian Standard Bible"),
    ("NRSV", "New Revised Standard Version"),
    ("RSV", "Revised Standard Version"),
    ("AMP", "Amplified Bible"),
    ("MSG", "The Message"),
    ("NET", "New English Translation"),
    ("WEB", "World English Bible"),
    ("ASV", "American Standard Version"),
    ("TPT", "The Passion Translation"),
    ("NIRV", "New International Reader's Version"),
    ("BSB", "Berean Standard Bible"),
    ("LSV", "Literal Standard Version"),
    ("YLT", "Young's Literal Translation"),
];

/// `REAL_VERSIONS` beyond `_NAMES`: real translations that are not necessarily available from a configured provider. A label
/// in this set is never spell-corrected into something else; if no provider has it, the verse is reported as skipped instead.
const OTHER_REAL_VERSIONS: &[&str] = &[
    "HCSB", "NCV", "GNT", "GNB", "CEV", "ISV", "LSB", "MEV", "NABRE", "NAB", "NJB", "JB", "DARBY",
    "DRA", "GNV", "GW", "ICB", "NIRV", "NIVUK", "NLV", "NOG", "NRSVCE", "ERV", "EHV", "EXB", "GWT",
    "TLB", "TLV", "VOICE", "WYC", "TNIV", "NCB", "CJB", "OJB", "TOJB", "RV", "RWV", "WMB", "WEBBE",
    "FBV", "CPDV", "JUB", "ACV", "BRG", "AKJV", "KJ21", "TCENT", "T4T", "PEV", "EASY", "LXX",
    "NABRE", "UKJV", "NMB", "PHILLIPS", "NEB", "REB", "NJPS", "TANAKH", "CEB", "CEV", "ETRV",
    "NIV84", "NIV2011", "NIV11",
];

/// `ALIASES`: spelling variants and exact editions that map to one canonical label (plain NASB means the 2020 edition).
const ALIASES: &[(&str, &str)] = &[
    ("NASB", "NASB2020"),
    ("NASB95", "NASB1995"),
    ("NASB1995", "NASB1995"),
    ("NASB20", "NASB2020"),
    ("NASB2020", "NASB2020"),
    ("NIV11", "NIV"),
    ("NIV2011", "NIV"),
];

/// `DEFAULT_USER_ALIASES`: operator overrides (label typed in documents -> version actually meant).
pub const DEFAULT_USER_ALIASES: &[(&str, &str)] = &[("YLT", "NLT")];

/// `BibleVersion`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BibleVersion {
    pub abbreviation: String,
    pub name: String,
    pub language: String,
    pub provider: Option<String>,
    pub provider_version_id: Option<String>,
    pub copyright: Option<String>,
    pub attribution: Option<String>,
}

/// `canonical_version`: the label the registry uses for `value` (case-insensitive), or `None`.
pub fn canonical_version(value: &str) -> Option<&'static str> {
    let key = casefold(value);
    for (alias, target) in ALIASES {
        if casefold(alias) == key {
            return Some(target);
        }
    }
    NAMES
        .iter()
        .find(|(name, _)| casefold(name) == key)
        .map(|(name, _)| *name)
}

/// `is_real_version`: a known translation, whether or not the app has a provider for it.
pub fn is_real_version(value: &str) -> bool {
    let upper = value.to_uppercase();
    NAMES.iter().any(|(name, _)| *name == upper)
        || OTHER_REAL_VERSIONS.contains(&upper.as_str())
        || canonical_version(value).is_some()
}

/// `_edit_distance_one`: the strings differ by one insert, delete, substitute or adjacent swap (or are equal).
fn edit_distance_one(a: &[char], b: &[char]) -> bool {
    if a == b {
        return true;
    }
    if a.len().abs_diff(b.len()) > 1 {
        return false;
    }
    if a.len() == b.len() {
        let diffs: Vec<usize> = (0..a.len()).filter(|&i| a[i] != b[i]).collect();
        if diffs.len() == 1 {
            return true;
        }
        return diffs.len() == 2
            && diffs[1] == diffs[0] + 1
            && a[diffs[0]] == b[diffs[1]]
            && a[diffs[1]] == b[diffs[0]];
    }
    let (short, long) = if a.len() < b.len() { (a, b) } else { (b, a) };
    (0..long.len()).any(|i| long[..i].iter().chain(&long[i + 1..]).eq(short.iter()))
}

/// `correct_version`: spell-correct an unrecognized label. `None` when the label is a real version (kept as typed, never
/// altered) or when there is no single unambiguous near match among the known names and `available`.
pub fn correct_version(value: &str, available: Option<&BTreeSet<String>>) -> Option<String> {
    let token = value.to_uppercase();
    if is_real_version(&token) {
        return None;
    }
    let mut pool: BTreeSet<String> = available
        .map(|set| set.iter().map(|item| item.to_uppercase()).collect())
        .unwrap_or_default();
    pool.extend(NAMES.iter().map(|(name, _)| (*name).to_string()));
    let token_chars: Vec<char> = token.chars().collect();
    let matches: BTreeSet<String> = pool
        .iter()
        .filter(|item| {
            let chars: Vec<char> = item.chars().collect();
            chars.len() >= 3 && edit_distance_one(&token_chars, &chars)
        })
        .map(|item| canonical_version(item).map_or_else(|| item.clone(), str::to_string))
        .collect();
    if matches.len() == 1 {
        matches.into_iter().next()
    } else {
        None
    }
}

/// `known_versions`: every named version, sorted by abbreviation.
pub fn known_versions() -> Vec<BibleVersion> {
    let mut all: Vec<BibleVersion> = NAMES
        .iter()
        .map(|(abbreviation, name)| BibleVersion {
            abbreviation: (*abbreviation).to_string(),
            name: (*name).to_string(),
            language: "eng".to_string(),
            provider: None,
            provider_version_id: None,
            copyright: None,
            attribution: None,
        })
        .collect();
    all.sort_by(|a, b| a.abbreviation.cmp(&b.abbreviation));
    all
}

/// Python's `ConfigurationError` (a bad setting, or a missing default).
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{0}")]
pub struct ConfigurationError(pub String);

/// `resolve_versions`: give every reference a version, its own or the default (the CLI value, else the saved one).
///
/// A reference with no version and no default is an error naming how many there are.
pub fn resolve_versions(
    references: &[PassageReference],
    cli_default: Option<&str>,
    saved_default: Option<&str>,
    non_interactive: bool,
) -> Result<Vec<PassageReference>, ConfigurationError> {
    let default_raw = cli_default
        .filter(|v| !v.is_empty())
        .or(saved_default.filter(|v| !v.is_empty()));
    let default = match default_raw {
        Some(raw) => Some(
            canonical_version(raw)
                .ok_or_else(|| ConfigurationError(format!("Unknown Bible version: {raw}")))?,
        ),
        None => None,
    };
    let mut resolved = Vec::new();
    let mut missing = 0;
    for reference in references {
        if reference.version.as_deref().is_some_and(|v| !v.is_empty()) {
            resolved.push(reference.clone());
        } else if let Some(default) = default {
            let mut copy = reference.clone();
            copy.version = Some(default.to_string());
            copy.warnings.push("default version applied".to_string());
            resolved.push(copy);
        } else {
            missing += 1;
        }
    }
    if missing > 0 {
        let mode = if non_interactive {
            "non-interactive mode"
        } else {
            "this workflow"
        };
        return Err(ConfigurationError(format!(
            "{missing} reference(s) do not specify a Bible version and no default is configured for {mode}"
        )));
    }
    Ok(resolved)
}

/// `same_passage` of `resolve_note_versions`: the same book and the same spans (the version is not looked at).
pub fn same_passage(a: &PassageReference, b: &PassageReference) -> bool {
    a.book_id == b.book_id && a.spans == b.spans
}

/// `comparison` of `resolve_note_versions`: the verse at `index` is the same passage as its neighbour before or after, written
/// in another version (a study comparison; such a pair never sets the version for later verses).
pub fn is_comparison_pair(references: &[&PassageReference], index: usize) -> bool {
    let here = references[index];
    [index.checked_sub(1), index.checked_add(1)]
        .into_iter()
        .flatten()
        .filter(|&other| other < references.len())
        .any(|other| {
            same_passage(here, references[other]) && references[other].version != here.version
        })
}

/// `_words`: lower case, curly apostrophes straight, anything but `a-z 0-9 '` becomes a space, split on spaces.
fn words(text: &str) -> Vec<String> {
    text.to_lowercase()
        .replace(['’', '‘'], "'")
        .chars()
        .map(|c| {
            if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '\'' {
                c
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split(' ')
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect()
}

/// Port of `difflib.SequenceMatcher(None, a, b, autojunk=False).get_matching_blocks()`, returning only the total size of the
/// blocks (all `quote_score` needs). `a` and `b` are sequences of word ids.
fn matching_total(a: &[usize], b: &[usize]) -> usize {
    let mut b2j: HashMap<usize, Vec<usize>> = HashMap::new();
    for (j, &w) in b.iter().enumerate() {
        b2j.entry(w).or_default().push(j);
    }
    let longest = |alo: usize, ahi: usize, blo: usize, bhi: usize| -> (usize, usize, usize) {
        let (mut besti, mut bestj, mut bestsize) = (alo, blo, 0);
        let mut j2len: HashMap<usize, usize> = HashMap::new();
        for (i, word) in a.iter().enumerate().take(ahi).skip(alo) {
            let mut new_j2len: HashMap<usize, usize> = HashMap::new();
            if let Some(js) = b2j.get(word) {
                for &j in js {
                    if j < blo {
                        continue;
                    }
                    if j >= bhi {
                        break;
                    }
                    let k = j
                        .checked_sub(1)
                        .and_then(|p| j2len.get(&p))
                        .copied()
                        .unwrap_or(0)
                        + 1;
                    new_j2len.insert(j, k);
                    if k > bestsize {
                        besti = i + 1 - k;
                        bestj = j + 1 - k;
                        bestsize = k;
                    }
                }
            }
            j2len = new_j2len;
        }
        // Python then extends the block over equal neighbours (junk handling); with no junk and no popular elements
        // the block is already maximal, but the loops are cheap and keep the port literal.
        while besti > alo && bestj > blo && a[besti - 1] == b[bestj - 1] {
            besti -= 1;
            bestj -= 1;
            bestsize += 1;
        }
        while besti + bestsize < ahi
            && bestj + bestsize < bhi
            && a[besti + bestsize] == b[bestj + bestsize]
        {
            bestsize += 1;
        }
        (besti, bestj, bestsize)
    };
    let mut queue = vec![(0, a.len(), 0, b.len())];
    let mut total = 0;
    while let Some((alo, ahi, blo, bhi)) = queue.pop() {
        let (i, j, k) = longest(alo, ahi, blo, bhi);
        if k > 0 {
            total += k;
            if alo < i && blo < j {
                queue.push((alo, i, blo, j));
            }
            if i + k < ahi && j + k < bhi {
                queue.push((i + k, ahi, j + k, bhi));
            }
        }
    }
    total
}

/// `quote_score`: how much of the quoted words appear, in order, in a version's wording of the passage (0 to 1). Verse
/// numbers, punctuation and case do not count; it is the share of the QUOTE found, so a quote may be only part of the verse.
pub fn quote_score(quoted: &str, verse_texts: &[&str]) -> f64 {
    let wanted = words(quoted);
    let have = words(&verse_texts.join(" "));
    if wanted.is_empty() || have.is_empty() {
        return 0.0;
    }
    let mut table: HashMap<String, usize> = HashMap::new();
    let mut intern = |w: &String| -> usize {
        let next = table.len();
        *table.entry(w.clone()).or_insert(next)
    };
    let a: Vec<usize> = wanted.iter().map(&mut intern).collect();
    let b: Vec<usize> = have.iter().map(&mut intern).collect();
    matching_total(&a, &b) as f64 / a.len() as f64
}

/// A row of `oracle/versions.jsonl` this code cannot read.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum VersionLineError {
    #[error("unknown kind of versions.jsonl row: {0}")]
    UnknownKind(String),
    #[error("versions.jsonl row has no usable `{0}`")]
    MissingField(&'static str),
}

fn opt(value: Option<impl Into<Value>>) -> Value {
    value.map_or(Value::Null, Into::into)
}

/// One line of `oracle/versions.jsonl` (`kind` = `version`, `version_available` or `quote_score`): the `out` it should give.
pub fn version_line(row: &Value) -> Result<Value, VersionLineError> {
    let kind = row["kind"].as_str().unwrap_or_default();
    let input = row["input"].as_str();
    match kind {
        "version" => {
            let label = input.ok_or(VersionLineError::MissingField("input"))?;
            Ok(json!({
                "canonical": opt(canonical_version(label)),
                "corrected": opt(correct_version(label, None)),
                "is_real": is_real_version(label),
            }))
        }
        "version_available" => {
            let label = input.ok_or(VersionLineError::MissingField("input"))?;
            let available: BTreeSet<String> = row["available"]
                .as_array()
                .ok_or(VersionLineError::MissingField("available"))?
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect();
            Ok(json!({"corrected": opt(correct_version(label, Some(&available)))}))
        }
        "quote_score" => {
            let quoted = row["quoted"]
                .as_str()
                .ok_or(VersionLineError::MissingField("quoted"))?;
            let texts: Vec<&str> = row["verse_texts"]
                .as_array()
                .ok_or(VersionLineError::MissingField("verse_texts"))?
                .iter()
                .filter_map(Value::as_str)
                .collect();
            Ok(json!(quote_score(quoted, &texts)))
        }
        other => Err(VersionLineError::UnknownKind(other.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::references::parse_references_with;

    fn set(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn canonical_names_and_aliases() {
        // every value below was read off the Python registry
        for (label, want) in [
            ("nasb", Some("NASB2020")), // plain NASB means the 2020 edition
            ("NASB", Some("NASB2020")),
            ("NASB95", Some("NASB1995")),
            ("NASB20", Some("NASB2020")),
            ("niv11", Some("NIV")),
            ("Niv2011", Some("NIV")),
            ("YLT", Some("YLT")),
            ("amp", Some("AMP")),
            ("NIRV", Some("NIRV")),
            ("hcsb", None), // real, but not one the app names
            ("NIV84", None),
            ("nlt ", None),
            ("", None),
        ] {
            assert_eq!(canonical_version(label), want, "{label:?}");
        }
        assert!(is_real_version("hcsb") && is_real_version("NIV84") && is_real_version("nasb"));
        assert!(!is_real_version("xyz") && !is_real_version("NIVV") && !is_real_version(""));
    }

    #[test]
    fn spelling_correction() {
        assert_eq!(correct_version("NKJ", None).as_deref(), Some("NKJV"));
        assert_eq!(correct_version("KJ", None).as_deref(), Some("KJV"));
        assert_eq!(correct_version("nlt ", None).as_deref(), Some("NLT"));
        assert_eq!(correct_version("NIVV", None), None); // NIV and NIRV are both one edit away
        assert_eq!(correct_version("esb", None), None); // ESV and BSB
        assert_eq!(correct_version("KJV", None), None); // real: never altered
        assert_eq!(correct_version("HCSB", None), None);
        assert_eq!(
            correct_version("CSBB", Some(&set(&["csb", "CSV"]))).as_deref(),
            Some("CSB")
        );
        assert_eq!(correct_version("ES", Some(&set(&["ESV", "ESB"]))), None);
        // an available label that is not a known name is returned as it is
        assert_eq!(
            correct_version("ABCD", Some(&set(&["ABCE"]))).as_deref(),
            Some("ABCE")
        );
    }

    #[test]
    fn edit_distance() {
        let d = |a: &str, b: &str| {
            edit_distance_one(
                &a.chars().collect::<Vec<_>>(),
                &b.chars().collect::<Vec<_>>(),
            )
        };
        assert!(
            d("NIV", "NIV")
                && d("NIV", "NIVV")
                && d("NIV", "NV")
                && d("NIV", "NVI")
                && d("NIV", "NIX")
        );
        assert!(!d("NIV", "NVX") && !d("NIV", "N") && !d("ABC", "CBA"));
    }

    #[test]
    fn the_registry_lists_every_name() {
        let all = known_versions();
        assert_eq!(all.len(), 21);
        assert_eq!(all[0].abbreviation, "AMP");
        assert_eq!(all[0].language, "eng");
        assert_eq!(all.last().unwrap().abbreviation, "YLT");
        assert!(NAMES.iter().all(|(name, _)| is_real_version(name)));
    }

    fn refs(line: &str) -> Vec<PassageReference> {
        parse_references_with(line, None).unwrap().references
    }

    #[test]
    fn resolve_versions_applies_the_default() {
        let parsed = refs("John 3:16 NIV; Romans 8:28");
        let out = resolve_versions(&parsed, None, Some("esv"), false).unwrap();
        assert_eq!(out[0].version.as_deref(), Some("NIV"));
        assert_eq!(out[0].warnings, Vec::<String>::new());
        assert_eq!(out[1].version.as_deref(), Some("ESV"));
        assert_eq!(out[1].warnings, vec!["default version applied"]);
        // the command-line default wins over the saved one; an empty one counts as none
        let out = resolve_versions(&parsed, Some("kjv"), Some("esv"), false).unwrap();
        assert_eq!(out[1].version.as_deref(), Some("KJV"));
        let out = resolve_versions(&parsed, Some(""), Some("esv"), false).unwrap();
        assert_eq!(out[1].version.as_deref(), Some("ESV"));
    }

    #[test]
    fn resolve_versions_errors_are_pythons() {
        let parsed = refs("John 3:16; Romans 8:28");
        assert_eq!(
            resolve_versions(&parsed, None, None, true).unwrap_err().to_string(),
            "2 reference(s) do not specify a Bible version and no default is configured for non-interactive mode"
        );
        assert_eq!(
            resolve_versions(&parsed[..1], None, None, false).unwrap_err().to_string(),
            "1 reference(s) do not specify a Bible version and no default is configured for this workflow"
        );
        assert_eq!(
            resolve_versions(&parsed, Some("HCSB"), None, false)
                .unwrap_err()
                .to_string(),
            "Unknown Bible version: HCSB"
        );
    }

    #[test]
    fn a_comparison_pair_is_the_same_passage_in_two_versions() {
        let parsed = refs("John 3:16 NKJV; John 3:16 NIV; Mark 1:1 NIV; Romans 8:28; Romans 8:28 NIV; Romans 8:28 NIV");
        let all: Vec<&PassageReference> = parsed.iter().collect();
        assert!(is_comparison_pair(&all, 0));
        assert!(is_comparison_pair(&all, 1));
        assert!(!is_comparison_pair(&all, 2)); // both neighbours are other passages
        assert!(is_comparison_pair(&all, 3)); // no version written next to the same passage with one: they differ
        assert!(is_comparison_pair(&all, 4));
        assert!(!is_comparison_pair(&all, 5)); // the same version as the one before
        assert!(same_passage(all[4], all[5]) && !same_passage(all[0], all[2]));
    }

    #[test]
    fn quote_scores() {
        assert_eq!(quote_score("", &["x"]), 0.0);
        assert_eq!(quote_score("something", &[]), 0.0);
        assert_eq!(quote_score("something", &[""]), 0.0);
        assert_eq!(
            quote_score("FOR BY GRACE", &["For by grace are ye saved"]),
            1.0
        );
        // curly quotes and verse numbers do not count
        assert_eq!(
            quote_score("it’s the gift of God", &["It's the gift of God."]),
            1.0
        );
        assert_eq!(quote_score("1 2 3 four", &["1. two 3 four"]), 0.75);
        // the quote may be a part of the verse, but the words must come in order
        assert_eq!(
            quote_score(
                "the earth the beginning",
                &["In the beginning God created the heavens and the earth."]
            ),
            0.5
        );
        assert!((quote_score("Except a man be born again, he cannot see the kingdom of God",
            &["Jesus answered and said to him, Most assuredly, I say to you, unless one is born again, he cannot see the kingdom of God."]) - 9.0 / 13.0).abs() < 1e-12);
    }

    #[test]
    fn version_lines() {
        assert_eq!(
            version_line(&json!({"kind": "version", "input": "NKJ"})).unwrap(),
            json!({"canonical": null, "corrected": "NKJV", "is_real": false})
        );
        assert_eq!(
            version_line(
                &json!({"kind": "version_available", "input": "NLTT", "available": ["NLT"]})
            )
            .unwrap(),
            json!({"corrected": "NLT"})
        );
        assert!(version_line(&json!({"kind": "other"})).is_err());
        assert!(version_line(&json!({"kind": "version"})).is_err());
    }
}
