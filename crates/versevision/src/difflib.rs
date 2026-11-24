//! `difflib.get_close_matches`, for "Did you mean ...?" (the suggestions click and typer make).

fn longest_match(
    a: &[char],
    b: &[char],
    alo: usize,
    ahi: usize,
    blo: usize,
    bhi: usize,
) -> (usize, usize, usize) {
    let (mut best_i, mut best_j, mut best_size) = (alo, blo, 0usize);
    // j2len: length of the match ending at b[j] for the previous row of a
    let mut j2len: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();
    #[allow(clippy::needless_range_loop)]
    for i in alo..ahi {
        let mut new: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();
        #[allow(clippy::needless_range_loop)]
        for j in blo..bhi {
            if a[i] == b[j] {
                let k = j2len.get(&j.wrapping_sub(1)).copied().unwrap_or(0) + 1;
                new.insert(j, k);
                if k > best_size {
                    best_i = i + 1 - k;
                    best_j = j + 1 - k;
                    best_size = k;
                }
            }
        }
        j2len = new;
    }
    (best_i, best_j, best_size)
}

fn matching(
    a: &[char],
    b: &[char],
    alo: usize,
    ahi: usize,
    blo: usize,
    bhi: usize,
    total: &mut usize,
) {
    let (i, j, k) = longest_match(a, b, alo, ahi, blo, bhi);
    if k > 0 {
        *total += k;
        if alo < i && blo < j {
            matching(a, b, alo, i, blo, j, total);
        }
        if i + k < ahi && j + k < bhi {
            matching(a, b, i + k, ahi, j + k, bhi, total);
        }
    }
}

/// `SequenceMatcher(None, a, b).ratio()` (no junk).
pub fn ratio(a: &str, b: &str) -> f64 {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let length = a.len() + b.len();
    if length == 0 {
        return 1.0;
    }
    let mut total = 0;
    matching(&a, &b, 0, a.len(), 0, b.len(), &mut total);
    2.0 * total as f64 / length as f64
}

/// The best (at most `n`) of `possibilities` whose ratio to `word` is at least `cutoff`, best first (`word` is the second sequence).
pub fn get_close_matches(
    word: &str,
    possibilities: &[String],
    n: usize,
    cutoff: f64,
) -> Vec<String> {
    let mut scored: Vec<(f64, &String)> = possibilities
        .iter()
        .filter_map(|p| {
            let r = ratio(p, word);
            (r >= cutoff).then_some((r, p))
        })
        .collect();
    // heapq.nlargest on (score, x): higher score first, then the larger string
    scored.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.1.cmp(a.1))
    });
    scored.into_iter().take(n).map(|(_, p)| p.clone()).collect()
}
