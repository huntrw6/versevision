//! `BeautifulSoup(markup, "html.parser").get_text(" ", strip=True)`: the text of an HTML snippet, the way API.Bible's verse text is read
//! (`bibles/api_bible.py`).
//!
//! It is a port of the two pieces that decide the result: CPython's `html.parser.HTMLParser` (3.13.15, `convert_charrefs=False`, as
//! BeautifulSoup runs it: start, end and `<x/>` tags, comments, `<!...>` declarations, `<?...>`, character and entity references,
//! the raw text of `<script>`/`<style>`) and BeautifulSoup's handling of what the parser reports (entity names looked up in the
//! HTML5 table, numeric references with the Windows-1252 and surrogate rules, the tag stack, strings inside `<script>`, `<style>`,
//! `<template>`, `<rt>` and `<rp>` left out, comments, declarations and processing instructions left out, CDATA kept). The differential
//! test (`oracle/providers/html_text.jsonl`) compares it with Python.

use fancy_regex::Regex;
use std::sync::OnceLock;
use versevision_core::pystr;

use crate::entities::ENTITIES;

fn re(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).expect("pattern"))
}

// `locatetagend`, `tagfind_tolerant`, `attrfind_tolerant` (html/parser.py).
fn locatetagend() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    re(
        &R,
        concat!(
            r#"[a-zA-Z][^\t\n\r\x0c />]*"#,
            r#"[\t\n\r\x0c /]*"#,
            r#"(?:(?<=['"\t\n\r\x0c /])[^\t\n\r\x0c />][^\t\n\r\x0c /=>]*"#,
            r#"(?:[\t\n\r\x0c ]*=[\t\n\r\x0c ]*"#,
            r#"(?:'[^']*'|"[^"]*"|(?!['"])[^>\t\n\r\x0c ]*)"#,
            r#")?"#,
            r#"[\t\n\r\x0c /]*"#,
            r#")*"#,
            r#">?"#,
        ),
    )
}

fn tagfind_tolerant() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    re(
        &R,
        r#"([a-zA-Z][^\t\n\r\x0c />]*)(?:[\t\n\r\x0c ]|/(?!>))*"#,
    )
}

fn attrfind_tolerant() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    re(
        &R,
        concat!(
            r#"((?<=['"\t\n\r\x0c /])[^\t\n\r\x0c />][^\t\n\r\x0c /=>]*)"#,
            r#"([\t\n\r\x0c ]*=[\t\n\r\x0c ]*"#,
            r#"('[^']*'|"[^"]*"|(?!['"])[^>\t\n\r\x0c ]*)"#,
            r#")?"#,
            r#"(?:[\t\n\r\x0c ]|/(?!>))*"#,
        ),
    )
}

fn commentclose() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    re(&R, r"--!?>")
}

/// `regex.match(text, pos)` (anchored at `pos`): the match, when it starts there.
fn match_at<'t>(
    regex: &Regex,
    text: &'t str,
    pos: usize,
) -> Option<fancy_regex::Captures<'t, str>> {
    let caps = regex.captures_from_pos(text, pos).ok().flatten()?;
    (caps.get(0)?.start() == pos).then_some(caps)
}

const CDATA_CONTENT_ELEMENTS: [&str; 6] =
    ["script", "style", "xmp", "iframe", "noembed", "noframes"];
const RCDATA_CONTENT_ELEMENTS: [&str; 2] = ["textarea", "title"];

/// What a string is to BeautifulSoup: only `Plain` (NavigableString) and `CData` are in `get_text`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Plain,
    CData,
    /// Comment, Doctype, Declaration, ProcessingInstruction, and the strings of `script`, `style`, `template`, `rt`, `rp`.
    Other,
}

struct Soup {
    current: Vec<String>,
    strings: Vec<(String, Kind)>,
    /// Open tags (names) and, for those that hold a special kind of string, that they do.
    stack: Vec<String>,
    containers: Vec<usize>,
}

impl Soup {
    fn new() -> Self {
        Self {
            current: Vec::new(),
            strings: Vec::new(),
            stack: Vec::new(),
            containers: Vec::new(),
        }
    }

    fn is_container(name: &str) -> bool {
        matches!(name, "rt" | "rp" | "style" | "script" | "template")
    }

    fn handle_data(&mut self, data: &str) {
        self.current.push(data.to_string());
    }

    /// `endData(containerClass)`.
    fn end_data(&mut self, class: Option<Kind>) {
        if self.current.is_empty() {
            return;
        }
        let text: String = self.current.concat();
        self.current.clear();
        let kind = match class {
            Some(kind) => kind,
            None if !self.containers.is_empty() => Kind::Other,
            None => Kind::Plain,
        };
        self.strings.push((text, kind));
    }

    fn start_tag(&mut self, name: &str) {
        self.end_data(None);
        self.stack.push(name.to_string());
        if Self::is_container(name) {
            self.containers.push(self.stack.len() - 1);
        }
    }

    fn end_tag(&mut self, name: &str) {
        self.end_data(None);
        // `_popToTag`: up to and including the most recent open tag of that name; nothing when none is open.
        let Some(at) = self.stack.iter().rposition(|open| open == name) else {
            return;
        };
        self.stack.truncate(at);
        while self.containers.last().is_some_and(|&index| index >= at) {
            self.containers.pop();
        }
    }

    fn text(mut self) -> String {
        self.end_data(None);
        let parts: Vec<&str> = self
            .strings
            .iter()
            .filter(|(_, kind)| *kind != Kind::Other)
            .map(|(text, _)| pystr::strip(text))
            .filter(|text| !text.is_empty())
            .collect();
        parts.join(" ")
    }
}

/// `html.parser.HTMLParser` with BeautifulSoup's handlers.
struct Parser {
    raw: String,
    soup: Soup,
    cdata_elem: Option<String>,
    escapable: bool,
    plaintext: bool,
}

impl Parser {
    fn goahead(&mut self, end: bool) {
        let raw = std::mem::take(&mut self.raw);
        let n = raw.len();
        let mut i = 0usize;
        while i < n {
            // `self.interesting.search(rawdata, i)`
            let found = self.find_interesting(&raw, i);
            let j = match found {
                Some(j) => j,
                None => {
                    if self.cdata_elem.is_some() {
                        break;
                    }
                    n
                }
            };
            if i < j {
                self.soup.handle_data(&raw[i..j]);
            }
            i = j;
            if i == n {
                break;
            }
            let rest = &raw[i..];
            if rest.starts_with('<') {
                let mut k: isize;
                let next = rest.chars().nth(1);
                if next.is_some_and(|c| c.is_ascii_alphabetic()) {
                    k = self.parse_starttag(&raw, i);
                } else if rest.starts_with("</") {
                    k = self.parse_endtag(&raw, i);
                } else if rest.starts_with("<!--") {
                    k = self.parse_comment(&raw, i);
                } else if rest.starts_with("<?") {
                    k = self.parse_pi(&raw, i);
                } else if rest.starts_with("<!") {
                    k = self.parse_html_declaration(&raw, i);
                } else if i + 1 < n || end {
                    self.soup.handle_data("<");
                    k = (i + 1) as isize;
                } else {
                    break;
                }
                if k < 0 {
                    if !end {
                        break;
                    }
                    if next.is_some_and(|c| c.is_ascii_alphabetic()) {
                        // an unterminated start tag: its text is dropped (the `pass` of CPython)
                    } else if rest.starts_with("</") {
                        if i + 2 == n {
                            self.soup.handle_data("</");
                        } else if rest.chars().nth(2).is_some_and(|c| c.is_ascii_alphabetic()) {
                            // nothing
                        } else {
                            self.handle_comment();
                        }
                    } else if rest.starts_with("<!--") {
                        self.handle_comment();
                    } else if rest.starts_with("<![CDATA[") {
                        self.unknown_decl(&raw[i + 3..]);
                    } else if rest
                        .get(..9)
                        .is_some_and(|head| head.eq_ignore_ascii_case("<!doctype"))
                    {
                        self.handle_decl();
                    } else if rest.starts_with("<!") {
                        self.handle_comment();
                    } else if rest.starts_with("<?") {
                        self.handle_pi();
                    }
                    k = n as isize;
                }
                i = k as usize;
            } else if rest.starts_with("&#") {
                // charref = `&#(?:[0-9]+|[xX][0-9a-fA-F]+)[^0-9a-fA-F]`
                if let Some((name, length)) = charref(rest) {
                    self.handle_charref(&name);
                    // the terminator is consumed only when it is `;`
                    i += length;
                    continue;
                }
                if incomplete_charref(rest) {
                    if end {
                        self.handle_charref(&raw[i + 2..]);
                        i = n;
                        break;
                    }
                    break;
                } else if i + 3 < n {
                    self.soup.handle_data("&#");
                    i += 2;
                } else {
                    break;
                }
            } else if rest.starts_with('&') {
                if let Some((name, length)) = entityref(rest) {
                    self.handle_entityref(&name);
                    i += length;
                    continue;
                }
                if rest
                    .chars()
                    .nth(1)
                    .is_some_and(|c| c.is_ascii_alphabetic() || c == '#')
                {
                    if end {
                        self.handle_entityref(&raw[i + 1..]);
                        i = n;
                        break;
                    }
                    break;
                } else if i + 1 < n {
                    self.soup.handle_data("&");
                    i += 1;
                } else {
                    break;
                }
            }
        }
        if end && i < n {
            self.soup.handle_data(&raw[i..n]);
            i = n;
        }
        self.raw = raw[i..].to_string();
    }

    /// The position of the next `<` or `&`, or (in raw-text mode) of the closing tag of the element.
    fn find_interesting(&self, raw: &str, from: usize) -> Option<usize> {
        if self.plaintext {
            return None;
        }
        match &self.cdata_elem {
            None => raw[from..].find(['<', '&']).map(|at| from + at),
            Some(name) => {
                let bytes = raw.as_bytes();
                let mut at = from;
                while at < bytes.len() {
                    let c = bytes[at];
                    if self.escapable && c == b'&' {
                        return Some(at);
                    }
                    if c == b'<' && bytes.get(at + 1) == Some(&b'/') {
                        let start = at + 2;
                        let end = start + name.len();
                        if end <= bytes.len()
                            && bytes[start..end].eq_ignore_ascii_case(name.as_bytes())
                            && matches!(
                                bytes.get(end),
                                Some(b'\t' | b'\n' | b'\r' | b'\x0c' | b' ' | b'/' | b'>')
                            )
                        {
                            return Some(at);
                        }
                    }
                    at += 1;
                }
                None
            }
        }
    }

    fn handle_comment(&mut self) {
        self.soup.end_data(None);
        self.soup.end_data(Some(Kind::Other)); // the (discarded) comment text never reaches `get_text`
    }

    fn handle_decl(&mut self) {
        self.handle_comment();
    }

    fn handle_pi(&mut self) {
        self.handle_comment();
    }

    /// bs4 `unknown_decl`: `CDATA[` text is kept, any other declaration is not.
    fn unknown_decl(&mut self, data: &str) {
        self.soup.end_data(None);
        if data
            .get(..6)
            .is_some_and(|head| head.eq_ignore_ascii_case("CDATA["))
        {
            self.soup.handle_data(&data[6..]);
            self.soup.end_data(Some(Kind::CData));
        } else {
            self.soup.handle_data(data);
            self.soup.end_data(Some(Kind::Other));
        }
    }

    fn parse_html_declaration(&mut self, raw: &str, i: usize) -> isize {
        let rest = &raw[i..];
        if rest.starts_with("<!--") {
            self.parse_comment(raw, i)
        } else if rest.starts_with("<![CDATA[") {
            match raw[i + 9..].find("]]>") {
                None => -1,
                Some(at) => {
                    let j = i + 9 + at;
                    self.unknown_decl(&raw[i + 3..j]);
                    (j + 3) as isize
                }
            }
        } else if rest
            .get(..9)
            .is_some_and(|head| head.eq_ignore_ascii_case("<!doctype"))
        {
            match raw[i + 9..].find('>') {
                None => -1,
                Some(at) => {
                    self.handle_decl();
                    (i + 9 + at + 1) as isize
                }
            }
        } else {
            self.parse_bogus_comment(raw, i)
        }
    }

    fn parse_comment(&mut self, raw: &str, i: usize) -> isize {
        // `commentabruptclose` (`-?>`) at i+4 wins over a later close.
        let body = &raw[i + 4..];
        let end = if body.starts_with("->") {
            Some(i + 4 + 2)
        } else if body.starts_with('>') {
            Some(i + 4 + 1)
        } else {
            commentclose()
                .find_from_pos(raw, i + 4)
                .ok()
                .flatten()
                .map(|m| m.end())
        };
        match end {
            None => -1,
            Some(end) => {
                self.handle_comment();
                end as isize
            }
        }
    }

    fn parse_bogus_comment(&mut self, raw: &str, i: usize) -> isize {
        match raw[i + 2..].find('>') {
            None => -1,
            Some(at) => {
                self.handle_comment();
                (i + 2 + at + 1) as isize
            }
        }
    }

    fn parse_pi(&mut self, raw: &str, i: usize) -> isize {
        match raw[i + 2..].find('>') {
            None => -1,
            Some(at) => {
                self.handle_pi();
                (i + 2 + at + 1) as isize
            }
        }
    }

    fn parse_starttag(&mut self, raw: &str, i: usize) -> isize {
        // check_for_whole_start_tag
        let Some(located) = match_at(locatetagend(), raw, i + 1) else {
            return -1;
        };
        let endpos = located.get(0).map_or(0, |m| m.end());
        if !raw[..endpos].ends_with('>') {
            return -1;
        }
        let Some(found) = match_at(tagfind_tolerant(), raw, i + 1) else {
            return -1;
        };
        let mut k = found.get(0).map_or(0, |m| m.end());
        let tag = found
            .get(1)
            .map_or(String::new(), |m| m.as_str().to_lowercase());
        while k < endpos {
            let Some(attr) = match_at(attrfind_tolerant(), raw, k) else {
                break;
            };
            let next = attr.get(0).map_or(k, |m| m.end());
            if next == k {
                break;
            }
            k = next;
        }
        let tail = pystr::strip(&raw[k.min(endpos)..endpos]);
        if tail != ">" && tail != "/>" {
            self.soup.handle_data(&raw[i..endpos]);
            return endpos as isize;
        }
        if tail.ends_with("/>") {
            self.soup.start_tag(&tag);
            self.soup.end_tag(&tag);
        } else {
            self.soup.start_tag(&tag);
            if CDATA_CONTENT_ELEMENTS.contains(&tag.as_str()) {
                self.set_cdata_mode(&tag, false);
            } else if tag == "plaintext" {
                self.set_cdata_mode(&tag, false);
                self.plaintext = true;
            } else if RCDATA_CONTENT_ELEMENTS.contains(&tag.as_str()) {
                self.set_cdata_mode(&tag, true);
            }
        }
        endpos as isize
    }

    fn set_cdata_mode(&mut self, name: &str, escapable: bool) {
        self.cdata_elem = Some(name.to_lowercase());
        self.escapable = escapable;
    }

    fn clear_cdata_mode(&mut self) {
        self.cdata_elem = None;
        self.escapable = true;
        self.plaintext = false;
    }

    fn parse_endtag(&mut self, raw: &str, i: usize) -> isize {
        if !raw[i + 2..].contains('>') {
            return -1;
        }
        let letter = raw[i + 2..]
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic());
        if !letter {
            if raw[i + 2..].starts_with('>') {
                return (i + 3) as isize;
            }
            return self.parse_bogus_comment(raw, i);
        }
        let Some(located) = match_at(locatetagend(), raw, i + 2) else {
            return -1;
        };
        let j = located.get(0).map_or(0, |m| m.end());
        if !raw[..j].ends_with('>') {
            return -1;
        }
        let Some(found) = match_at(tagfind_tolerant(), raw, i + 2) else {
            return -1;
        };
        let tag = found
            .get(1)
            .map_or(String::new(), |m| m.as_str().to_lowercase());
        self.soup.end_tag(&tag);
        self.clear_cdata_mode();
        j as isize
    }

    /// bs4 `handle_charref`.
    fn handle_charref(&mut self, name: &str) {
        let (dereferenced, extra) = dereference_numeric(name);
        self.soup.handle_data(&dereferenced);
        if let Some(extra) = extra {
            self.soup.handle_data(&extra);
        }
    }

    /// bs4 `handle_entityref`: a name in the HTML5 table is its text, anything else stays `&name`.
    fn handle_entityref(&mut self, name: &str) {
        match ENTITIES.binary_search_by(|(known, _)| (*known).cmp(name)) {
            Ok(at) => self.soup.handle_data(ENTITIES[at].1),
            Err(_) => self.soup.handle_data(&format!("&{name}")),
        }
    }
}

/// `charref.match(rest)`: the reference name (`x41`, `65`) and how much of `rest` it takes (the terminator only when it is `;`).
fn charref(rest: &str) -> Option<(String, usize)> {
    let bytes = rest.as_bytes();
    // &# then [0-9]+ or [xX][0-9a-fA-F]+ then one char that is not a hex digit
    let mut at = 2;
    // the terminator must not be a hex digit: backtracking over decimal digits is pointless (they are all hex digits)
    let digits_end = if bytes.get(at).is_some_and(u8::is_ascii_digit) {
        while bytes.get(at).is_some_and(u8::is_ascii_digit) {
            at += 1;
        }
        at
    } else if matches!(bytes.get(at), Some(b'x' | b'X'))
        && bytes.get(at + 1).is_some_and(u8::is_ascii_hexdigit)
    {
        at += 1;
        while bytes.get(at).is_some_and(u8::is_ascii_hexdigit) {
            at += 1;
        }
        at
    } else {
        return None;
    };
    let terminator = rest[digits_end..].chars().next()?;
    if terminator.is_ascii_hexdigit() {
        return None;
    }
    let name = rest[2..digits_end].to_string();
    let length = if terminator == ';' {
        digits_end + 1
    } else {
        digits_end
    };
    Some((name, length))
}

/// `incomplete_charref`: `&#` followed by a digit or `x`/`X` and a hex digit.
fn incomplete_charref(rest: &str) -> bool {
    let bytes = rest.as_bytes();
    match bytes.get(2) {
        Some(b) if b.is_ascii_digit() => true,
        Some(b'x' | b'X') => bytes.get(3).is_some_and(u8::is_ascii_hexdigit),
        _ => false,
    }
}

/// `entityref.match(rest)`: `&([a-zA-Z][-.a-zA-Z0-9]*)[^a-zA-Z0-9]`; the terminator is consumed only when it is `;`.
fn entityref(rest: &str) -> Option<(String, usize)> {
    let bytes = rest.as_bytes();
    if !bytes.get(1).is_some_and(u8::is_ascii_alphabetic) {
        return None;
    }
    let mut at = 2;
    while bytes
        .get(at)
        .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'-' || *b == b'.')
    {
        at += 1;
    }
    // [^a-zA-Z0-9] after a greedy run: if the run ended on '-' or '.', those are name characters and the next char is not
    // alphanumeric (the run stopped), so the terminator is that char; backtracking could give a shorter name whose next
    // char is '-' or '.', which is also a legal terminator, but the greedy match is tried first and succeeds when any char follows.
    let terminator = rest[at..].chars().next();
    match terminator {
        Some(c) => {
            let name = rest[1..at].to_string();
            let length = if c == ';' { at + 1 } else { at };
            Some((name, length))
        }
        None => {
            // end of text right after the name: the regex backtracks to the last `-` or `.` of the name (not the first
            // character) and takes it as the terminator
            let cut = (2..at)
                .rev()
                .find(|&p| bytes[p] == b'-' || bytes[p] == b'.')?;
            Some((rest[1..cut].to_string(), cut))
        }
    }
}

const WINDOWS_1252: [(u32, char); 27] = [
    (0x80, '\u{20ac}'),
    (0x82, '\u{201a}'),
    (0x83, '\u{0192}'),
    (0x84, '\u{201e}'),
    (0x85, '\u{2026}'),
    (0x86, '\u{2020}'),
    (0x87, '\u{2021}'),
    (0x88, '\u{02c6}'),
    (0x89, '\u{2030}'),
    (0x8a, '\u{0160}'),
    (0x8b, '\u{2039}'),
    (0x8c, '\u{0152}'),
    (0x8e, '\u{017d}'),
    (0x91, '\u{2018}'),
    (0x92, '\u{2019}'),
    (0x93, '\u{201c}'),
    (0x94, '\u{201d}'),
    (0x95, '\u{2022}'),
    (0x96, '\u{2013}'),
    (0x97, '\u{2014}'),
    (0x98, '\u{02dc}'),
    (0x99, '\u{2122}'),
    (0x9a, '\u{0161}'),
    (0x9b, '\u{203a}'),
    (0x9c, '\u{0153}'),
    (0x9e, '\u{017e}'),
    (0x9f, '\u{0178}'),
];

/// `UnicodeDammit.numeric_character_reference`.
fn numeric_character_reference(numeric: u64) -> char {
    const REPLACEMENT: char = '\u{fffd}';
    if numeric == 0 || numeric > 0x10ffff || (0xd800..=0xdfff).contains(&numeric) {
        return REPLACEMENT;
    }
    let numeric = numeric as u32;
    if (0xfdd0..=0xfdef).contains(&numeric) || (numeric & 0xfffe) == 0xfffe {
        return char::from_u32(numeric).unwrap_or(REPLACEMENT);
    }
    if (0x80..=0x9f).contains(&numeric) {
        if let Some((_, c)) = WINDOWS_1252.iter().find(|(n, _)| *n == numeric) {
            return *c;
        }
    }
    char::from_u32(numeric).unwrap_or(REPLACEMENT)
}

/// Python's `int(text, base)` for the shapes that reach it: optional surrounding whitespace and sign, digits with single `_`
/// between them, and a `0x` prefix in base 16. `None` is a `ValueError`. Values past `u64` are capped (they are out of Unicode range anyway).
fn py_int(text: &str, base: u32) -> Option<u64> {
    let mut t = pystr::strip(text);
    let negative = t.starts_with('-');
    if t.starts_with(['+', '-']) {
        t = &t[1..];
    }
    if base == 16 && (t.starts_with("0x") || t.starts_with("0X")) {
        t = &t[2..];
        if t.starts_with('_') {
            t = &t[1..];
        }
    }
    if t.is_empty() || t.starts_with('_') || t.ends_with('_') || t.contains("__") {
        return None;
    }
    let mut value: u64 = 0;
    for c in t.chars().filter(|c| *c != '_') {
        let digit = c.to_digit(base)?;
        value = value
            .saturating_mul(u64::from(base))
            .saturating_add(u64::from(digit));
    }
    // a negative number is out of Unicode range as far as `chr` is concerned: handled by the caller as the replacement
    if negative && value != 0 {
        return Some(u64::MAX);
    }
    Some(value)
}

/// bs4 `_dereference_numeric_character_reference`: the character, and the data after a reference that was not terminated.
fn dereference_numeric(name: &str) -> (String, Option<String>) {
    let (digits, base) = if name.starts_with(['x', 'X']) {
        (&name[1..], 16)
    } else {
        (name, 10)
    };
    let mut extra: Option<String> = None;
    let mut real = py_int(digits, base);
    if real.is_none() {
        // `^([0-9]+)(.*)` (decimal) or `^([0-9a-f]+)(.*)` (hex, lower case only); `.` does not match a newline
        let is_digit = |c: char| {
            if base == 16 {
                c.is_ascii_digit() || ('a'..='f').contains(&c)
            } else {
                c.is_ascii_digit()
            }
        };
        let split = digits.find(|c: char| !is_digit(c)).unwrap_or(digits.len());
        if split > 0 {
            real = py_int(&digits[..split], base);
            let tail = &digits[split..];
            let line = tail.split('\n').next().unwrap_or("");
            extra = Some(line.to_string());
        }
    }
    match real {
        None => (String::new(), Some(digits.to_string())),
        Some(number) => (numeric_character_reference(number).to_string(), extra),
    }
}

/// `BeautifulSoup(html, "html.parser").get_text(" ", strip=True)`.
pub fn soup_text(markup: &str) -> String {
    let mut parser = Parser {
        raw: markup.to_string(),
        soup: Soup::new(),
        cdata_elem: None,
        escapable: true,
        plaintext: false,
    };
    // `feed` (goahead(0)), then `close` (goahead(1)) on what is left.
    parser.goahead(false);
    parser.goahead(true);
    parser.soup.text()
}
