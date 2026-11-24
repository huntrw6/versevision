//! `propresenter/selection.py`: pick exactly one presentation by id or name, and suggest the near misses when there is none.
//!
//! The suggestions are `rapidfuzz.process.extract(query, names, scorer=fuzz.WRatio, limit=3)` of the Python app; [`wratio`] is that scorer
//! (rapidfuzz 3.x, no pre-processing: the comparison is case-sensitive).

use versevision_core::references::pytext::casefold;

use crate::api_client::PresentationSummary;
use crate::errors::{VvError, VvResult};

fn chars(text: &str) -> Vec<char> {
    text.chars().collect()
}

/// Length of the longest common subsequence.
fn lcs(a: &[char], b: &[char]) -> usize {
    let mut prev = vec![0usize; b.len() + 1];
    for &x in a {
        let mut cur = vec![0usize; b.len() + 1];
        for (j, &y) in b.iter().enumerate() {
            cur[j + 1] = if x == y {
                prev[j] + 1
            } else {
                prev[j + 1].max(cur[j])
            };
        }
        prev = cur;
    }
    prev[b.len()]
}

/// `100 * (1 - dist / lensum)` with `dist` the insertion/deletion distance (`fuzz.ratio`).
fn norm(dist: usize, lensum: usize) -> f64 {
    if lensum == 0 {
        100.0
    } else {
        100.0 * (1.0 - dist as f64 / lensum as f64)
    }
}

pub fn ratio(a: &[char], b: &[char]) -> f64 {
    let lensum = a.len() + b.len();
    norm(lensum - 2 * lcs(a, b), lensum)
}

/// `fuzz.partial_ratio`: the shorter string against every window of the longer one (the windows that run off either end included).
pub fn partial_ratio(s1: &[char], s2: &[char]) -> f64 {
    let (mut short, mut long) = if s1.len() <= s2.len() {
        (s1, s2)
    } else {
        (s2, s1)
    };
    if short.is_empty() && long.is_empty() {
        return 100.0;
    }
    if short.is_empty() {
        return 0.0;
    }
    let windows = |short: &[char], long: &[char]| -> f64 {
        let n = short.len();
        let mut best = 0.0_f64;
        for i in 1..n {
            best = best.max(ratio(short, &long[..i.min(long.len())]));
        }
        for i in 0..=(long.len() - n) {
            best = best.max(ratio(short, &long[i..i + n]));
        }
        for i in (long.len() - n + 1)..long.len() {
            best = best.max(ratio(short, &long[i..]));
        }
        best
    };
    let mut score = windows(short, long);
    if short.len() == long.len() && score < 100.0 {
        std::mem::swap(&mut short, &mut long);
        score = score.max(windows(short, long));
    }
    score
}

fn tokens(text: &str) -> Vec<String> {
    versevision_core::pystr::split_ws(text)
        .into_iter()
        .map(str::to_string)
        .collect()
}

fn sorted_join(items: &[String]) -> String {
    let mut sorted = items.to_vec();
    sorted.sort();
    sorted.join(" ")
}

fn token_sort_ratio(a: &str, b: &str) -> f64 {
    let (mut ta, mut tb) = (tokens(a), tokens(b));
    ta.sort();
    tb.sort();
    ratio(&chars(&ta.join(" ")), &chars(&tb.join(" ")))
}

fn unique(items: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    items
        .into_iter()
        .filter(|t| seen.insert(t.clone()))
        .collect()
}

fn token_set_ratio(a: &str, b: &str) -> f64 {
    let (ta, tb) = (unique(tokens(a)), unique(tokens(b)));
    let intersect: Vec<String> = ta.iter().filter(|t| tb.contains(t)).cloned().collect();
    let diff_ab: Vec<String> = ta.iter().filter(|t| !tb.contains(t)).cloned().collect();
    let diff_ba: Vec<String> = tb.iter().filter(|t| !ta.contains(t)).cloned().collect();
    if !intersect.is_empty() && (diff_ab.is_empty() || diff_ba.is_empty()) {
        return 100.0;
    }
    let (ab, ba) = (sorted_join(&diff_ab), sorted_join(&diff_ba));
    let (ab_len, ba_len) = (ab.chars().count(), ba.chars().count());
    let sect_len = sorted_join(&intersect).chars().count();
    let sect_ab_len = sect_len + usize::from(sect_len > 0) + ab_len;
    let sect_ba_len = sect_len + usize::from(sect_len > 0) + ba_len;
    let result = ratio(&chars(&ab), &chars(&ba));
    if sect_len == 0 {
        return result;
    }
    let sect_ab = norm(usize::from(sect_len > 0) + ab_len, sect_len + sect_ab_len);
    let sect_ba = norm(usize::from(sect_len > 0) + ba_len, sect_len + sect_ba_len);
    result.max(sect_ab).max(sect_ba)
}

fn token_ratio(a: &str, b: &str) -> f64 {
    token_sort_ratio(a, b).max(token_set_ratio(a, b))
}

fn partial_token_ratio(a: &str, b: &str) -> f64 {
    let (ta, tb) = (unique(tokens(a)), unique(tokens(b)));
    if ta.iter().any(|t| tb.contains(t)) {
        return 100.0;
    }
    let diff_ab: Vec<String> = ta.iter().filter(|t| !tb.contains(t)).cloned().collect();
    let diff_ba: Vec<String> = tb.iter().filter(|t| !ta.contains(t)).cloned().collect();
    let result = partial_ratio(&chars(&sorted_join(&ta)), &chars(&sorted_join(&tb)));
    if ta.len() == diff_ab.len() && tb.len() == diff_ba.len() {
        return result;
    }
    result.max(partial_ratio(
        &chars(&sorted_join(&diff_ab)),
        &chars(&sorted_join(&diff_ba)),
    ))
}

/// `fuzz.WRatio(a, b)`.
pub fn wratio(a: &str, b: &str) -> f64 {
    const UNBASE_SCALE: f64 = 0.95;
    let (ca, cb) = (chars(a), chars(b));
    let (len1, len2) = (ca.len(), cb.len());
    if len1 == 0 || len2 == 0 {
        return 0.0;
    }
    let len_ratio = if len1 > len2 {
        len1 as f64 / len2 as f64
    } else {
        len2 as f64 / len1 as f64
    };
    let end_ratio = ratio(&ca, &cb);
    if len_ratio < 1.5 {
        return end_ratio.max(token_ratio(a, b) * UNBASE_SCALE);
    }
    let partial_scale = if len_ratio <= 8.0 { 0.9 } else { 0.6 };
    let end_ratio = end_ratio.max(partial_ratio(&ca, &cb) * partial_scale);
    end_ratio.max(partial_token_ratio(a, b) * UNBASE_SCALE * partial_scale)
}

/// `process.extract(query, choices, scorer=WRatio, limit=limit)`: `(choice, score, index)` best first, equal scores in list order.
pub fn extract(query: &str, choices: &[String], limit: usize) -> Vec<(String, f64, usize)> {
    let mut scored: Vec<(String, f64, usize)> = choices
        .iter()
        .enumerate()
        .map(|(i, c)| (c.clone(), wratio(query, c), i))
        .collect();
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(limit);
    scored
}

/// `select_unique_presentation`.
pub fn select_unique_presentation(
    presentations: &[PresentationSummary],
    uuid: Option<&str>,
    name: Option<&str>,
    library: Option<&str>,
) -> VvResult<PresentationSummary> {
    let uuid = uuid.filter(|u| !u.is_empty());
    let name = name.filter(|n| !n.is_empty());
    let library = library.filter(|l| !l.is_empty());
    let mut candidates: Vec<&PresentationSummary> = presentations.iter().collect();
    if let Some(library) = library {
        candidates.retain(|item| {
            casefold(item.library_name.as_deref().unwrap_or("")) == casefold(library)
        });
    }
    let matches: Vec<&PresentationSummary> = if let Some(uuid) = uuid {
        candidates
            .iter()
            .copied()
            .filter(|i| casefold(&i.uuid) == casefold(uuid))
            .collect()
    } else if let Some(name) = name {
        candidates
            .iter()
            .copied()
            .filter(|i| casefold(&i.name) == casefold(name))
            .collect()
    } else {
        return Err(VvError::configuration(
            "A target UUID or presentation name is required in non-interactive mode",
        ));
    };
    if matches.len() == 1 {
        return Ok(matches[0].clone());
    }
    if matches.len() > 1 {
        let locations: Vec<String> = matches
            .iter()
            .map(|i| {
                format!(
                    "{}/{} ({})",
                    i.library_name.as_deref().unwrap_or("None"),
                    i.name,
                    i.uuid
                )
            })
            .collect();
        return Err(VvError::configuration(format!(
            "Presentation target is ambiguous: {}",
            locations.join(", ")
        )));
    }
    let names: Vec<String> = candidates.iter().map(|i| i.name.clone()).collect();
    let query = name.or(uuid).unwrap_or("");
    let hint: Vec<String> = extract(query, &names, 3)
        .into_iter()
        .filter(|(_, score, _)| *score >= 60.0)
        .map(|(value, _, _)| value)
        .collect();
    let hint = hint.join(", ");
    Err(VvError::configuration(format!(
        "Presentation target was not found{}",
        if hint.is_empty() {
            String::new()
        } else {
            format!(". Possible matches: {hint}")
        }
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_and_disjoint_strings() {
        assert_eq!(wratio("Verses", "Verses"), 100.0);
        assert_eq!(wratio("", "x"), 0.0);
        assert!(wratio("abc", "xyz") < 60.0);
    }
}
