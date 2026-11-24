//! Line fitting: width measure, wrapping, the even cuts of a long verse, balanced point lines.
//!
//! A port of `python/src/versevision/slides/fitting.py`. The arithmetic is f64 in the same order as Python (so the same
//! sums, the same ties), the loops run in the same order and a candidate only replaces the best one when strictly cheaper.
//! The output has to be byte-identical to `oracle/fitting.jsonl`.
//!
//! Where Python and Rust could differ, and what was done:
//! * `str.isspace`/`split()`/`strip()`/`splitlines()`: [`crate::pystr`] (Python also counts U+001C..U+001F as space).
//! * `str.upper()`/`lower()`: `to_uppercase`/`to_lowercase` are the same full Unicode mappings (`ß` -> `SS`, final sigma);
//!   the two languages ship different Unicode versions, so a character added after Unicode 15.1 could differ.
//! * `sum()` of floats is compensated in Python 3.12+ ([`crate::pystr::sum`]); `x ** 2` is `x * x`.
//! * `"" in "abc"` is true in Python: a word made only of closing quotes counts as ending at punctuation.
//! * Python's `lru_cache` is only a speed-up (the functions are pure), so a plain memo table is used.
//! * Not reproducible (Python raises): `max_lines == 0` in [`split_verse_text`] (ZeroDivisionError) returns the flat text;
//!   in [`balance_lines`] it skips the shrink step.

use std::collections::HashMap;

use crate::pystr::{flatten, is_space, split_ws, splitlines, strip, sum};

const NARROW: &str = ".,:;!'`|ijlI1()[]";
const WIDE: &str = "MW@%QOwm";
const WEAK_END: [&str; 20] = [
    "a", "an", "the", "of", "to", "in", "on", "at", "it", "is", "as", "by", "or", "and", "for",
    "but", "—", "–", "-", "&",
];
/// Characters a closing quotation may be followed by before the punctuation test (`rstrip("\"'”’)")`).
const CLOSERS: [char; 5] = ['"', '\'', '”', '’', ')'];
const PUNCT_END: &str = ".,;:?!—–-";
const STRIP_TAIL: [char; 11] = ['.', ',', ';', ':', '!', '?', '"', '\'', '”', '’', ')'];

/// The width of `value` in "average letters": spaces 0.52, narrow marks 0.58, wide letters 1.28, the rest 1.
pub fn text_width_units(value: &str) -> f64 {
    let mut total = 0.0;
    for c in value.chars() {
        if is_space(c) {
            total += 0.52;
        } else if NARROW.contains(c) {
            total += 0.58;
        } else if WIDE.contains(c) {
            total += 1.28;
        } else {
            total += 1.0;
        }
    }
    total
}

/// Greedy wrap: every line of `text` is wrapped on its own; an empty line stays an empty line.
pub fn wrap_body_lines(text: &str, line_width_units: f64) -> Vec<String> {
    let mut lines = Vec::new();
    let paragraphs = splitlines(text);
    let paragraphs = if paragraphs.is_empty() {
        vec![""]
    } else {
        paragraphs
    };
    for paragraph in paragraphs {
        let words = split_ws(paragraph);
        let Some((first, rest)) = words.split_first() else {
            lines.push(String::new());
            continue;
        };
        let mut current = (*first).to_string();
        for word in rest {
            let candidate = format!("{current} {word}");
            if text_width_units(&candidate) <= line_width_units {
                current = candidate;
            } else {
                lines.push(std::mem::replace(&mut current, (*word).to_string()));
            }
        }
        lines.push(current);
    }
    lines
}

/// How good a place to cut a long verse after `before` (the text up to the cut) is: a full stop 4, `;` `:` 3, a comma or dash 2.
fn break_priority(before: &str) -> i32 {
    match before.trim_end_matches(is_space).chars().last() {
        None => 0,
        Some('.' | '?' | '!') => 4,
        Some(';' | ':') => 3,
        Some(',' | '—' | '–' | '-') => 2,
        Some(_) => 1,
    }
}

/// `word.rstrip("\"'”’)")[-1:] in ".,;:?!—–-"`.
fn ends_at_punctuation(word: &str) -> bool {
    match word.trim_end_matches(CLOSERS).chars().last() {
        None => true, // Python: "" in "..." is True
        Some(c) => PUNCT_END.contains(c),
    }
}

/// A yes/no question about the words `a..b`, remembered (the answer never changes).
struct Memo<'a> {
    cache: HashMap<(usize, usize), bool>,
    ask: Box<dyn FnMut(usize, usize) -> bool + 'a>,
}

impl<'a> Memo<'a> {
    fn new(ask: impl FnMut(usize, usize) -> bool + 'a) -> Self {
        Memo {
            cache: HashMap::new(),
            ask: Box::new(ask),
        }
    }
    fn get(&mut self, a: usize, b: usize) -> bool {
        if let Some(&known) = self.cache.get(&(a, b)) {
            return known;
        }
        let answer = (self.ask)(a, b);
        self.cache.insert((a, b), answer);
        answer
    }
}

fn ceil_div(a: usize, b: usize) -> usize {
    a.div_ceil(b)
}

/// Split a long verse into the fewest pieces that fit `max_lines` lines each, with about the same amount of text in
/// each, cutting after punctuation where that costs little.
pub fn split_verse_text(
    text: &str,
    max_lines: usize,
    line_width_units: f64,
    prefer_punctuation: bool,
    tolerance: f64,
    punctuation_tolerance: f64,
) -> Vec<String> {
    let flat = flatten(text);
    if flat.is_empty() {
        return vec![String::new()];
    }
    let flat_lines = wrap_body_lines(&flat, line_width_units).len();
    if flat_lines <= max_lines {
        return vec![flat];
    }
    if max_lines == 0 {
        return vec![flat]; // Python: ZeroDivisionError
    }
    let words: Vec<String> = flat.split(' ').map(str::to_string).collect();
    let space = text_width_units(" ");
    let mut prefix = vec![0.0];
    for word in &words {
        let last = prefix[prefix.len() - 1];
        prefix.push(last + text_width_units(word) + space);
    }
    let mut fits = Memo::new(|a, b| {
        wrap_body_lines(&words[a..b].join(" "), line_width_units).len() <= max_lines
    });
    let pieces_needed = 2.max(ceil_div(flat_lines, max_lines));
    for tol in [tolerance, tolerance.max(0.35), 10.0] {
        let punct = if prefer_punctuation {
            punctuation_tolerance.max(tol)
        } else {
            tol
        };
        if let Some(found) = even_cuts(
            &words,
            &prefix,
            pieces_needed,
            &mut fits,
            prefer_punctuation,
            tol,
            punct,
        ) {
            return found;
        }
    }
    vec![flat]
}

/// The cheapest way to cut `words` into `pieces_needed` (or one more) pieces: a dynamic programme over the cut positions.
/// Cost of a piece = squared distance of its width from the mean, minus a bonus for a cut after punctuation.
fn even_cuts(
    words: &[String],
    prefix: &[f64],
    pieces_needed: usize,
    fits: &mut Memo<'_>,
    prefer_punctuation: bool,
    tolerance: f64,
    punctuation_tolerance: f64,
) -> Option<Vec<String>> {
    let count = words.len();
    // cut_ok[i]: a cut before word i falls after punctuation (the two ends of the verse count as clean cuts)
    let mut cut_ok = vec![true];
    cut_ok.extend((1..count).map(|i| ends_at_punctuation(&words[i - 1])));
    cut_ok.push(true);
    // priority[b]: how good a cut after word b-1 is (what `_break_priority(" ".join(words[:b]) + " ", ...)` returns)
    let priority: Vec<i32> = (0..=count)
        .map(|b| {
            let last = words[..b].iter().rev().find_map(|w| {
                let t = w.trim_end_matches(is_space);
                (!t.is_empty()).then_some(t)
            });
            last.map_or(0, break_priority)
        })
        .collect();
    let space = text_width_units(" ");
    let width = |a: usize, b: usize| prefix[b] - prefix[a] - space;
    let last_k = count.min(pieces_needed + 1);
    for k in pieces_needed..=last_k {
        let mean = prefix[count] / k as f64;
        let inf = f64::INFINITY;
        let mut best = vec![vec![inf; count + 1]; k + 1];
        let mut back = vec![vec![0_usize; count + 1]; k + 1];
        best[0][0] = 0.0;
        for j in 1..=k {
            for b in j..=count {
                for a in (j - 1)..b {
                    if best[j - 1][a] == inf || !fits.get(a, b) {
                        continue;
                    }
                    let limit = if cut_ok[a] && cut_ok[b] {
                        punctuation_tolerance
                    } else {
                        tolerance
                    };
                    if (width(a, b) - mean).abs() > limit * mean {
                        continue;
                    }
                    let mut bonus = 0.0;
                    if prefer_punctuation && b < count {
                        bonus = 0.3 * mean * mean * f64::from(priority[b] - 1) / 3.0;
                    }
                    let diff = width(a, b) - mean;
                    let cost = best[j - 1][a] + diff * diff - bonus;
                    if cost < best[j][b] {
                        best[j][b] = cost;
                        back[j][b] = a;
                    }
                }
            }
        }
        if best[k][count] < inf {
            let mut cuts = Vec::new();
            let mut b = count;
            for j in (1..=k).rev() {
                let a = back[j][b];
                cuts.push((a, b));
                b = a;
            }
            cuts.reverse();
            return Some(
                cuts.into_iter()
                    .map(|(a, b)| words[a..b].join(" "))
                    .collect(),
            );
        }
    }
    None
}

/// Cut one sermon point into the fewest slides of at most `max_lines` lines (`max_lines` 0 = no limit). The first
/// `glue_first` words stay together.
pub fn split_point_text(
    text: &str,
    line_width_units: f64,
    max_lines: usize,
    upper: bool,
    glue_first: usize,
    tolerance: f64,
    punctuation_tolerance: f64,
) -> Vec<String> {
    let flat = flatten(text);
    let mut words: Vec<String> = if flat.is_empty() {
        Vec::new()
    } else {
        flat.split(' ').map(str::to_string).collect()
    };
    if max_lines == 0 || words.len() < 2 {
        return vec![flat];
    }
    if glue_first > 1 && words.len() > glue_first {
        let glued = words[..glue_first].join(" ");
        words.splice(..glue_first, [glued]);
    }
    let shown: Vec<String> = words
        .iter()
        .map(|w| if upper { w.to_uppercase() } else { w.clone() })
        .collect();
    let space = text_width_units(" ");
    let lines_in = |a: usize, b: usize| -> usize {
        let mut count = 1;
        let mut current: Option<f64> = None;
        for word in &shown[a..b] {
            let width = text_width_units(word);
            match current {
                None => current = Some(width),
                Some(c) if c + space + width <= line_width_units => {
                    current = Some(c + space + width)
                }
                Some(_) => {
                    count += 1;
                    current = Some(width);
                }
            }
        }
        count
    };
    let total_lines = lines_in(0, words.len());
    if total_lines <= max_lines {
        return vec![words.join(" ")];
    }
    let mut prefix = vec![0.0];
    for word in &shown {
        let last = prefix[prefix.len() - 1];
        prefix.push(last + text_width_units(word) + space);
    }
    let needed = 2.max(ceil_div(total_lines, max_lines));
    let mut fits = Memo::new(|a, b| lines_in(a, b) <= max_lines);
    for tol in [tolerance, tolerance.max(0.35), 10.0] {
        if let Some(found) = even_cuts(
            &words,
            &prefix,
            needed,
            &mut fits,
            true,
            tol,
            punctuation_tolerance.max(tol),
        ) {
            return found;
        }
    }
    vec![words.join(" ")]
}

/// Break text into the fewest lines that fit, then spread the words so the lines are as even in width as possible
/// (see the Python docstring for the parameters). Lines already in the text are balanced on their own.
pub fn balance_lines(
    text: &str,
    line_width_units: f64,
    upper: bool,
    glue_first: usize,
    max_lines: usize,
    tolerance: f64,
    punctuation_tolerance: f64,
) -> String {
    let punct = tolerance.max(punctuation_tolerance);
    text.split('\n')
        .enumerate()
        .map(|(i, line)| {
            balance_one(
                line,
                line_width_units,
                upper,
                if i == 0 { glue_first } else { 0 },
                max_lines,
                tolerance,
                punct,
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Balance one line (no newline inside).
pub fn balance_one(
    line: &str,
    limit: f64,
    upper: bool,
    glue_first: usize,
    max_lines: usize,
    tolerance: f64,
    punct_tolerance: f64,
) -> String {
    let mut limit = limit;
    let words = split_ws(line);
    if words.len() < 2 {
        return strip(line).to_string();
    }
    let shown: Vec<String> = words
        .iter()
        .map(|w| {
            if upper {
                w.to_uppercase()
            } else {
                (*w).to_string()
            }
        })
        .collect();
    let space = text_width_units(" ");
    let widths: Vec<f64> = shown.iter().map(|w| text_width_units(w)).collect();
    let total = sum(&widths) + space * (words.len() - 1) as f64;
    if total <= limit {
        return words.join(" ");
    }
    // words that must stay together count as one unit
    let mut units: Vec<(usize, usize)> = Vec::new();
    let mut start = 0;
    if glue_first > 1 && words.len() > glue_first {
        units.push((0, glue_first));
        start = glue_first;
    }
    units.extend((start..words.len()).map(|i| (i, i + 1)));
    let uw: Vec<f64> = units
        .iter()
        .map(|&(a, b)| sum(&widths[a..b]) + space * (b - a - 1) as f64)
        .collect();

    let greedy = |cap: f64| -> usize {
        let mut count = 1;
        let mut current = uw[0];
        for &width in &uw[1..] {
            if current + space + width <= cap {
                current += space + width;
            } else {
                count += 1;
                current = width;
            }
        }
        count
    };

    let mut lines_needed = greedy(limit);
    if lines_needed == 1 {
        return words.join(" ");
    }
    if lines_needed > max_lines && max_lines > 0 {
        // shrunk to fit n_lines in the box height: the first n_lines the shrunk width manages in that many lines
        let shrunk = |n_lines: usize| limit * n_lines as f64 / max_lines as f64 * 0.95;
        if let Some(n_lines) = ((max_lines + 1)..=lines_needed).find(|&n| greedy(shrunk(n)) <= n) {
            limit = shrunk(n_lines);
            lines_needed = n_lines;
        }
    }
    let n = units.len();
    let target = total / lines_needed as f64;
    let mut prefix = vec![0.0];
    for &width in &uw {
        let last = prefix[prefix.len() - 1];
        prefix.push(last + width + space);
    }
    let span = |a: usize, b: usize| prefix[b] - prefix[a] - space;
    // the last word of unit i
    let unit_last = |i: usize| words[units[i].1 - 1];
    // clean[i]: a break before unit i comes right after punctuation (the two ends of the text count as clean)
    let mut clean = vec![true];
    clean.extend((1..n).map(|i| ends_at_punctuation(unit_last(i - 1))));
    clean.push(true);
    // dangling[b]: a line ending with unit b-1 ends on "it", "a", "to", a dash...
    let dangling_end: Vec<bool> = (0..=n)
        .map(|b| {
            b > 0 && {
                let tail = unit_last(b - 1)
                    .trim_matches(&STRIP_TAIL[..])
                    .to_lowercase();
                WEAK_END.contains(&tail.as_str())
            }
        })
        .collect();
    let inf = f64::INFINITY;
    let mut best: Vec<Vec<f64>> = Vec::new();
    let mut back: Vec<Vec<usize>> = Vec::new();
    for (tol, ptol) in [(tolerance, punct_tolerance), (0.35, 0.35), (10.0, 10.0)] {
        best = vec![vec![inf; n + 1]; lines_needed + 1];
        back = vec![vec![0; n + 1]; lines_needed + 1];
        best[0][0] = 0.0;
        for k in 1..=lines_needed {
            for b in k..=n {
                for a in (k - 1)..b {
                    if best[k - 1][a] == inf {
                        continue;
                    }
                    let width = span(a, b);
                    let allowed = if clean[a] && clean[b] { ptol } else { tol };
                    if (width - target).abs() > allowed * target {
                        continue;
                    }
                    let over = if width - limit > 0.0 {
                        width - limit
                    } else {
                        0.0
                    };
                    let dangling = if b < n && dangling_end[b] { 25.0 } else { 0.0 };
                    let natural = if b < n && clean[b] {
                        -0.06 * target * target
                    } else {
                        0.0
                    };
                    let diff = width - target;
                    let cost =
                        best[k - 1][a] + diff * diff + 1000.0 * over * over + dangling + natural;
                    if cost < best[k][b] {
                        best[k][b] = cost;
                        back[k][b] = a;
                    }
                }
            }
        }
        if best[lines_needed][n] < inf {
            break;
        }
    }
    if best[lines_needed][n] == inf {
        return words.join(" "); // not reachable: the last tolerance (10.0) admits every cut
    }
    let mut cuts = Vec::new();
    let mut b = n;
    for k in (1..=lines_needed).rev() {
        let a = back[k][b];
        cuts.push((a, b));
        b = a;
    }
    cuts.reverse();
    cuts.into_iter()
        .map(|(a, b)| words[units[a].0..units[b - 1].1].join(" "))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn width_counts_like_python() {
        assert_eq!(text_width_units(""), 0.0);
        assert!((text_width_units("Wi, a") - (1.28 + 0.58 + 0.58 + 0.52 + 1.0)).abs() < 1e-12);
        assert_eq!(text_width_units("\u{a0}"), 0.52);
    }

    #[test]
    fn wrap_keeps_empty_lines() {
        assert_eq!(wrap_body_lines("", 10.0), vec![String::new()]);
        assert_eq!(wrap_body_lines("a\n\nb", 10.0), vec!["a", "", "b"]);
        assert_eq!(
            wrap_body_lines("aaaa bbbb cccc", 8.4),
            vec!["aaaa", "bbbb", "cccc"]
        );
        assert_eq!(
            wrap_body_lines("aaaa bbbb cccc", 8.6),
            vec!["aaaa bbbb", "cccc"]
        );
    }

    #[test]
    fn short_text_is_left_alone() {
        assert_eq!(
            balance_lines("  one  ", 33.0, true, 0, 4, 0.15, 0.25),
            "one"
        );
        assert_eq!(
            balance_lines("two words", 33.0, true, 0, 4, 0.15, 0.25),
            "two words"
        );
        assert_eq!(
            split_verse_text("  ", 5, 48.0, true, 0.15, 0.25),
            vec![String::new()]
        );
        assert_eq!(
            split_point_text("one", 33.0, 2, true, 0, 0.15, 0.25),
            vec!["one"]
        );
    }

    #[test]
    fn upper_case_is_the_full_unicode_mapping() {
        // Python: "straße".upper() == "STRASSE" (7 letters wide, not 6)
        let lines = balance_lines("straße straße straße", 14.0, true, 0, 4, 0.15, 0.25);
        assert!(lines.contains('\n'), "{lines}");
        assert_eq!(text_width_units(&"straße".to_uppercase()), 7.0);
    }

    #[test]
    fn a_long_verse_is_cut_into_even_pieces() {
        let verse = "For God so loved the world, that he gave his only begotten Son, that whoever believes in him should not perish, but have eternal life. For God did not send his Son into the world to condemn the world, but that the world might be saved through him.";
        let pieces = split_verse_text(verse, 2, 36.0, true, 0.15, 0.25);
        assert!(pieces.len() >= 2);
        assert_eq!(pieces.join(" "), verse);
        for piece in &pieces {
            assert!(wrap_body_lines(piece, 36.0).len() <= 2, "{piece}");
        }
    }
}
