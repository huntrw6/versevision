//! `normalizer.py`: NFKC, no-break space to space, the dash family to `-`, fullwidth ampersand to `&`.

use unicode_normalization::UnicodeNormalization;

pub fn normalize_fragment(text: &str) -> String {
    text.nfkc()
        .map(|c| match c {
            '\u{a0}' => ' ',
            '–' | '—' | '‒' | '−' => '-',
            '＆' => '&',
            other => other,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::normalize_fragment;

    #[test]
    fn dashes_and_spaces() {
        assert_eq!(normalize_fragment("John 3:16–18"), "John 3:16-18");
        assert_eq!(normalize_fragment("a\u{a0}b—c−d‒e"), "a b-c-d-e");
        assert_eq!(normalize_fragment("１ Ｃｏｒ ＆"), "1 Cor &");
        assert_eq!(normalize_fragment("½"), "1⁄2");
    }
}
