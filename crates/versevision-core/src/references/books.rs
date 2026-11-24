//! `books.py`: the 66 books, their aliases and the book-name matcher.
//!
//! Python builds one regex per alias (`re.escape(alias)` with every space turned into `\s*`, then `\.?(?![A-Za-z])`, `re.IGNORECASE`)
//! and tries them longest key first. Here each alias is matched by hand with the same meaning: letters and digits compare
//! case-insensitively, a space matches any run of whitespace (also none), then an optional full stop, then no letter.

use std::sync::OnceLock;

use indexmap::IndexMap;

use super::book_data::BOOK_DATA;
use super::pytext::{casefold, fold, is_letter_ic};
use crate::pystr::{flatten, is_space};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Book {
    pub id: &'static str,
    pub name: &'static str,
    pub chapters: u32,
    pub aliases: &'static [&'static str],
}

/// `BOOKS`: all 66, in canon order.
pub fn books() -> &'static [Book] {
    BOOK_DATA
}

/// `_key`: case-folded, full stops removed, whitespace collapsed to one space, trimmed.
fn key(value: &str) -> String {
    flatten(&casefold(value).replace('.', ""))
}

struct AliasTable {
    /// `ALIASES`: key -> index into `BOOK_DATA` (a repeated key keeps its first position and takes the last book).
    by_key: IndexMap<String, usize>,
    /// `ALIASES_BY_LENGTH`: the keys, longest first, equal lengths in `ALIASES` order (Python's sort is stable).
    by_length: Vec<(Vec<char>, usize)>,
}

fn table() -> &'static AliasTable {
    static TABLE: OnceLock<AliasTable> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut by_key: IndexMap<String, usize> = IndexMap::new();
        for (index, book) in BOOK_DATA.iter().enumerate() {
            for alias in std::iter::once(&book.name).chain(book.aliases.iter()) {
                by_key.insert(key(alias), index);
            }
        }
        let mut by_length: Vec<(Vec<char>, usize)> = by_key
            .iter()
            .map(|(k, &index)| (k.chars().collect(), index))
            .collect();
        by_length.sort_by_key(|(chars, _)| std::cmp::Reverse(chars.len()));
        AliasTable { by_key, by_length }
    })
}

/// The book an alias key stands for (`ALIASES[key]`); the key is looked up as given, so pass it through `alias_key` first.
pub fn book_for_alias(alias: &str) -> Option<&'static Book> {
    table().by_key.get(&key(alias)).map(|&i| &BOOK_DATA[i])
}

/// The number of alias keys (`len(ALIASES)`).
pub fn alias_count() -> usize {
    table().by_key.len()
}

/// The book with this id (`GEN`, `1CO`, ...).
pub fn book_by_id(id: &str) -> Option<&'static Book> {
    BOOK_DATA.iter().find(|book| book.id == id)
}

/// Does `alias` (a key: lower case, single spaces) match `text` at `start`? Returns the end of the match.
fn match_alias(text: &[char], start: usize, alias: &[char]) -> Option<usize> {
    let n = text.len();
    let mut p = start;
    for &want in alias {
        if want == ' ' {
            while p < n && is_space(text[p]) {
                p += 1;
            }
        } else if p < n && fold(text[p]) == want {
            p += 1;
        } else {
            return None;
        }
    }
    // `\.?(?![A-Za-z])`: a full stop is taken unless a letter follows it; without one no letter may follow the name.
    if p < n && text[p] == '.' {
        return Some(if p + 1 < n && is_letter_ic(text[p + 1]) {
            p
        } else {
            p + 1
        });
    }
    if p < n && is_letter_ic(text[p]) {
        return None;
    }
    Some(p)
}

/// `recognize_book(text, start)`: the book written at `start` and the position after its name (and full stop).
pub fn recognize_book(text: &[char], start: usize) -> Option<(&'static Book, usize)> {
    table().by_length.iter().find_map(|(alias, index)| {
        match_alias(text, start, alias).map(|end| (&BOOK_DATA[*index], end))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recognize(text: &str) -> Option<(&'static str, usize)> {
        let chars: Vec<char> = text.chars().collect();
        recognize_book(&chars, 0).map(|(book, end)| (book.id, end))
    }

    #[test]
    fn the_table_has_the_python_shape() {
        assert_eq!(books().len(), 66);
        assert_eq!(books().iter().map(|b| b.chapters).sum::<u32>(), 1189);
        assert_eq!(alias_count(), 177);
        assert_eq!(book_by_id("EPH").unwrap().name, "Ephesians");
        assert_eq!(book_by_id("SNG").unwrap().aliases[3], "Canticles");
    }

    /// The order the matcher tries aliases in (longest key first, ties in table order) is Python's `ALIASES_BY_LENGTH`:
    /// FNV-1a of `key<TAB>book id` lines, computed from the Python lists.
    #[test]
    fn alias_order_is_pythons() {
        let mut text = String::new();
        for (chars, index) in &table().by_length {
            text.push_str(&format!(
                "{}\t{}\n",
                chars.iter().collect::<String>(),
                BOOK_DATA[*index].id
            ));
        }
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for byte in text.bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        assert_eq!(hash, 0xdb69fe9c3c2cc392);
    }

    #[test]
    fn aliases_spot_checks() {
        for (text, id) in [
            ("Genesis", "GEN"),
            ("gen", "GEN"),
            ("Gen.", "GEN"),
            ("Ex", "EXO"),
            ("Dt", "DEU"),
            ("Jdgs", "JDG"),
            ("I Samuel", "1SA"),
            ("second samuel", "2SA"),
            ("1Kgs", "1KI"),
            ("2 chron", "2CH"),
            ("Ps", "PSA"),
            ("Psalm", "PSA"),
            ("Psalms", "PSA"),
            ("SOS", "SNG"),
            ("Song of Songs", "SNG"),
            ("Canticles", "SNG"),
            ("Ezek", "EZK"),
            ("Mt", "MAT"),
            ("Jn", "JHN"),
            ("1 Jn", "1JN"),
            ("III John", "3JN"),
            ("Third John", "3JN"),
            ("Phlm", "PHM"),
            ("Php", "PHP"),
            ("2Cor", "2CO"),
            ("First Corinthians", "1CO"),
            ("The Revelation", "REV"),
            ("Rev", "REV"),
            ("Jud", "JUD"),
        ] {
            assert_eq!(recognize(text).map(|(id, _)| id), Some(id), "{text}");
        }
    }

    #[test]
    fn longest_alias_wins_and_boundaries() {
        // "Jude" is a name, "Jud" an alias: the whole word is taken
        assert_eq!(recognize("Jude 5"), Some(("JUD", 4)));
        assert_eq!(recognize("Judges 3"), Some(("JDG", 6)));
        assert_eq!(recognize("Jon 1"), Some(("JON", 3)));
        // a letter right after the name means it is another word
        assert_eq!(recognize("Johnny 3"), None);
        assert_eq!(recognize("Genesisx"), None);
        // a full stop belongs to the name unless a letter follows it
        assert_eq!(recognize("Gen. 1"), Some(("GEN", 4)));
        assert_eq!(recognize("Gen.x"), Some(("GEN", 3)));
        // spaces inside a name are optional and any whitespace counts
        assert_eq!(recognize("1Cor 3"), Some(("1CO", 4)));
        assert_eq!(recognize("1\t Cor 3"), Some(("1CO", 6)));
        assert_eq!(recognize("Song  of   Solomon"), Some(("SNG", 18)));
        assert_eq!(recognize("Matthew"), Some(("MAT", 7)));
        assert_eq!(recognize("Esther"), Some(("EST", 6)));
    }

    #[test]
    fn dotted_i_matches_like_python_ignorecase() {
        // re.IGNORECASE equates ı and İ with i
        assert_eq!(recognize("Isaıah").map(|(id, _)| id), Some("ISA"));
        assert_eq!(recognize("İsa").map(|(id, _)| id), Some("ISA"));
    }
}
