//! `tokenizer.py`: `\s+ | \d+ | [A-Za-z]+ | [:;,&()\[\]-] | .`, whitespace dropped. Offsets are character (code point) offsets, as in Python.

use super::pytext::{decimal_value, is_alpha, is_digit};
use crate::pystr::is_space;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    Number,
    Word,
    /// Any other character, which is its own kind (`:`, `-`, `&`, `(`, ...).
    Punct(char),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Token {
    pub kind: Kind,
    pub value: String,
    pub start: usize,
    pub end: usize,
}

/// Tokens of `text`; `offset` is added to every position.
pub(super) fn tokenize(text: &[char], offset: usize) -> Vec<Token> {
    let mut tokens = Vec::new();
    let n = text.len();
    let mut i = 0;
    while i < n {
        let c = text[i];
        if is_space(c) {
            while i < n && is_space(text[i]) {
                i += 1;
            }
            continue;
        }
        let start = i;
        let kind = if decimal_value(c).is_some() {
            while i < n && decimal_value(text[i]).is_some() {
                i += 1;
            }
            Kind::Number
        } else if c.is_ascii_alphabetic() {
            while i < n && text[i].is_ascii_alphabetic() {
                i += 1;
            }
            Kind::Word
        } else {
            i += 1;
            if matches!(c, ':' | ';' | ',' | '&' | '(' | ')' | '[' | ']' | '-') {
                Kind::Punct(c)
            } else if is_digit(c) {
                Kind::Number
            } else if is_alpha(c) {
                Kind::Word
            } else {
                Kind::Punct(c)
            }
        };
        tokens.push(Token {
            kind,
            value: text[start..i].iter().collect(),
            start: offset + start,
            end: offset + i,
        });
    }
    tokens
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(text: &str) -> Vec<(Kind, String)> {
        let chars: Vec<char> = text.chars().collect();
        tokenize(&chars, 0)
            .into_iter()
            .map(|t| (t.kind, t.value))
            .collect()
    }

    #[test]
    fn splits_like_python() {
        let k = kinds(" 3:16-18, NIV2 é.");
        assert_eq!(k[0], (Kind::Number, "3".into()));
        assert_eq!(k[1].0, Kind::Punct(':'));
        assert_eq!(k[5].0, Kind::Punct(','));
        assert_eq!(k[6], (Kind::Word, "NIV".into()));
        assert_eq!(k[7], (Kind::Number, "2".into()));
        assert_eq!(k[8], (Kind::Word, "é".into()));
        assert_eq!(k[9], (Kind::Punct('.'), ".".into()));
    }

    #[test]
    fn offsets_are_characters() {
        let chars: Vec<char> = "é 5".chars().collect();
        let t = tokenize(&chars, 10);
        assert_eq!((t[1].start, t[1].end), (12, 13));
    }
}
