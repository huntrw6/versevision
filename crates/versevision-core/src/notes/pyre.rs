//! Python `re` syntax for the patterns a user types into the config (`notes.skip_patterns`), on top of `fancy-regex`.
//!
//! The Python classifier compiles every pattern with `re.compile(pattern, re.IGNORECASE)`. A Rust regex engine reads a different
//! language (`\Z`, `[[]`, `{,3}`, `\w` and `\s` all mean something else), so the pattern is *parsed the way `re._parser` parses it*
//! (same grammar, same error messages, same positions) and written out again as a `fancy-regex` pattern that means the same.
//! Everything that Python rejects is rejected here with Python's own message (`missing ), unterminated subpattern at position 0`);
//! everything it accepts is translated:
//!
//! * `\d` is `\p{Nd}`; `\s` is Python's `str.isspace` set (`White_Space` plus U+001C..U+001F); `\w` is `[\p{L}\p{N}_]`;
//!   `\b` / `\B` are look-arounds on that word set (and, as in Python 3.13, neither matches the empty string); `$` also matches
//!   before a final `\n`; `\Z` is the end of the text; `{,n}` is `{0,n}`; a `{` that is not a quantifier is a literal; `[` inside a set is a literal.
//! * Named groups become numbered groups (the names are kept for [`PyCaptures::name`]); possessive quantifiers become atomic groups.
//! * `re.IGNORECASE` equates `i`, `I`, `ı` (U+0131) and `İ` (U+0130); the engine does not know the last two, so the text and the pattern
//!   are written with both as `i` first ([`fold_dotted_i`]).
//!
//! Where this port is not Python (found by running 180,000 random patterns through both; `oracle/user_patterns.jsonl` has the ones that matter):
//!
//! * Not supported, so an error from this port that Python does not give (`exception: "Unsupported"`): `\N{...}` character names, a lone
//!   surrogate escape (`\ud800`), the ASCII flag `(?a)` together with IGNORECASE, and whatever `fancy-regex` itself cannot compile.
//! * What a *group* captured when a repeated group can match the empty string (`(a|)*` on `a`: Python keeps `""` from the last empty round,
//!   the engine keeps `"a"`). The span of the whole match, and whether there is a match, are the same.
//! * `ı`/`İ` are folded for the whole pattern or not at all: inside a scoped `(?i:...)` of a case-sensitive pattern, and in a back-reference
//!   (`(?i)(i)\1` on `iı`), Python and this port can differ.
//! * Python has no backtracking limit; here a search that runs into `fancy-regex`'s limit counts as "no match".

use std::collections::HashMap;
use std::fmt;

use fancy_regex::{Regex, RegexBuilder};

const MAXREPEAT: u128 = 4_294_967_295; // 2**32 - 1 on 64-bit CPython
const MAXWIDTH: u128 = 1 << 64;
const MAXGROUPS: usize = 2_147_483_647 / 2;

const F_I: u32 = 2;
const F_L: u32 = 4;
const F_M: u32 = 8;
const F_S: u32 = 16;
const F_U: u32 = 32;
const F_X: u32 = 64;
const F_A: u32 = 256;
const TYPE_FLAGS: u32 = F_A | F_L | F_U;

/// What `re.compile` raised.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatternError {
    /// The Python exception: `re.error` (named `PatternError` since Python 3.13), `OverflowError`, or `Unsupported` for what this port cannot do.
    pub exception: &'static str,
    /// `str(exception)`.
    pub message: String,
}

impl fmt::Display for PatternError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for PatternError {}

/// `re.error(msg, pattern, pos)`: `"msg at position N"`, plus `(line L, column C)` when the pattern has a newline.
fn re_error(msg: &str, pattern: &[char], pos: i64) -> PatternError {
    let mut message = format!("{msg} at position {pos}");
    if pattern.contains(&'\n') {
        let upto = (pos.max(0) as usize).min(pattern.len());
        let lineno = pattern[..upto].iter().filter(|c| **c == '\n').count() + 1;
        let last = pattern[..upto].iter().rposition(|c| *c == '\n');
        let colno = pos - last.map_or(-1, |i| i as i64);
        message.push_str(&format!(" (line {lineno}, column {colno})"));
    }
    PatternError {
        exception: "PatternError",
        message,
    }
}

fn plain_error(msg: &str) -> PatternError {
    PatternError {
        exception: "PatternError",
        message: msg.to_string(),
    }
}

/// `repr(str)` for the small strings that appear in messages.
pub fn py_repr(text: &str) -> String {
    let quote = if text.contains('\'') && !text.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::new();
    out.push(quote);
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if is_printable(c) => out.push(c),
            c if (c as u32) < 0x100 => out.push_str(&format!("\\x{:02x}", c as u32)),
            c if (c as u32) < 0x10000 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push_str(&format!("\\U{:08x}", c as u32)),
        }
    }
    out.push(quote);
    out
}

/// `str.isprintable()` for one character (an approximation by category: controls, separators other than the space, format
/// characters, surrogates, private use and unassigned code points are not printable).
fn is_printable(c: char) -> bool {
    if c == ' ' {
        return true;
    }
    if c.is_control() || crate::pystr::is_space(c) {
        return false;
    }
    let cp = c as u32;
    !matches!(cp,
        0xAD | 0x600..=0x605 | 0x61C | 0x6DD | 0x70F | 0x180E | 0x200B..=0x200F | 0x2028..=0x202E
        | 0x2060..=0x2064 | 0x2066..=0x206F | 0xFEFF | 0xFFF9..=0xFFFB | 0xE000..=0xF8FF
        | 0xF0000..=0x10FFFF)
}

// ------------------------------------------------------------------------------------------------ the tokenizer

struct Source {
    chars: Vec<char>,
    index: usize,
    next: Option<String>,
}

impl Source {
    fn new(pattern: &str) -> Result<Self, PatternError> {
        let mut source = Self {
            chars: pattern.chars().collect(),
            index: 0,
            next: None,
        };
        source.advance()?;
        Ok(source)
    }

    fn advance(&mut self) -> Result<(), PatternError> {
        let index = self.index;
        match self.chars.get(index) {
            None => {
                self.next = None;
            }
            Some('\\') => match self.chars.get(index + 1) {
                None => {
                    return Err(re_error(
                        "bad escape (end of pattern)",
                        &self.chars,
                        self.chars.len() as i64 - 1,
                    ))
                }
                Some(c) => {
                    self.next = Some(format!("\\{c}"));
                    self.index = index + 2;
                }
            },
            Some(c) => {
                self.next = Some(c.to_string());
                self.index = index + 1;
            }
        }
        Ok(())
    }

    fn next_is(&self, text: &str) -> bool {
        self.next.as_deref() == Some(text)
    }

    fn matches(&mut self, text: &str) -> Result<bool, PatternError> {
        if self.next_is(text) {
            self.advance()?;
            return Ok(true);
        }
        Ok(false)
    }

    fn get(&mut self) -> Result<Option<String>, PatternError> {
        let this = self.next.clone();
        self.advance()?;
        Ok(this)
    }

    /// `getwhile(n, charset)`: up to `n` tokens that are in the set.
    fn getwhile(&mut self, n: usize, set: &str) -> Result<String, PatternError> {
        let mut result = String::new();
        for _ in 0..n {
            match &self.next {
                Some(c) if !c.is_empty() && set.contains(c.as_str()) => {
                    result.push_str(c);
                    self.advance()?;
                }
                _ => break,
            }
        }
        Ok(result)
    }

    fn getuntil(&mut self, terminator: &str, name: &str) -> Result<String, PatternError> {
        let mut result = String::new();
        loop {
            let c = self.next.clone();
            self.advance()?;
            match c {
                None => {
                    if result.is_empty() {
                        return Err(self.error(&format!("missing {name}"), 0));
                    }
                    return Err(self.error(
                        &format!("missing {terminator}, unterminated name"),
                        result.chars().count(),
                    ));
                }
                Some(c) if c == terminator => {
                    if result.is_empty() {
                        return Err(self.error(&format!("missing {name}"), 1));
                    }
                    break;
                }
                Some(c) => result.push_str(&c),
            }
        }
        Ok(result)
    }

    fn tell(&self) -> usize {
        self.index - self.next.as_ref().map_or(0, |n| n.chars().count())
    }

    fn seek(&mut self, index: usize) -> Result<(), PatternError> {
        self.index = index;
        self.advance()
    }

    fn error(&self, msg: &str, offset: usize) -> PatternError {
        re_error(msg, &self.chars, self.tell() as i64 - offset as i64)
    }

    fn check_group_name(&self, name: &str, offset: usize) -> Result<(), PatternError> {
        if !is_identifier(name) {
            let msg = format!("bad character in group name {}", py_repr(name));
            return Err(self.error(&msg, name.chars().count() + offset));
        }
        Ok(())
    }
}

/// `str.isidentifier()` (letters and `_`, then letters, digits and `_`).
fn is_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c == '_' || c.is_alphabetic() => {}
        _ => return false,
    }
    chars.all(|c| c == '_' || c.is_alphanumeric())
}

// ------------------------------------------------------------------------------------------------ the parse result

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Literal,
    In,
    Any,
    /// `^ $ \A \Z \b \B`: cannot be repeated.
    At,
    Repeat,
    Subpattern,
    Atomic,
    Branch,
    Assert,
    GroupRef,
    GroupRefExists,
    Failure,
}

#[derive(Debug, Clone)]
struct Item {
    op: Op,
    /// The same thing as a `fancy-regex` pattern (one atom).
    frag: String,
    lo: u128,
    hi: u128,
}

#[derive(Debug, Clone, Default)]
struct Seq {
    items: Vec<Item>,
}

impl Seq {
    fn frag(&self) -> String {
        self.items.iter().map(|i| i.frag.as_str()).collect()
    }

    fn width(&self) -> (u128, u128) {
        let (mut lo, mut hi) = (0u128, 0u128);
        for item in &self.items {
            lo += item.lo;
            hi += item.hi;
        }
        (lo.min(MAXWIDTH), hi.min(MAXWIDTH))
    }

    fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

struct State {
    flags: u32,
    groupdict: HashMap<String, usize>,
    groupwidths: Vec<Option<(u128, u128)>>,
    lookbehindgroups: Option<usize>,
    grouprefpos: Vec<(usize, usize)>,
    /// `name -> index` in the order the groups were opened.
    names: Vec<(String, usize)>,
    /// The first error `re.compile` raises after parsing (a look-behind of variable width).
    compile_error: Option<PatternError>,
}

impl State {
    fn groups(&self) -> usize {
        self.groupwidths.len()
    }

    fn open_group(&mut self, name: Option<&str>) -> Result<usize, PatternError> {
        let gid = self.groups();
        self.groupwidths.push(None);
        if self.groups() > MAXGROUPS {
            return Err(plain_error("too many groups"));
        }
        if let Some(name) = name {
            if let Some(ogid) = self.groupdict.get(name) {
                return Err(plain_error(&format!(
                    "redefinition of group name {} as group {gid}; was group {ogid}",
                    py_repr(name)
                )));
            }
            self.groupdict.insert(name.to_string(), gid);
            self.names.push((name.to_string(), gid));
        }
        Ok(gid)
    }

    fn check_group(&self, gid: usize) -> bool {
        gid < self.groups() && self.groupwidths[gid].is_some()
    }

    fn check_lookbehind_group(&self, gid: usize, source: &Source) -> Result<(), PatternError> {
        if let Some(lookbehind) = self.lookbehindgroups {
            if !self.check_group(gid) {
                return Err(source.error("cannot refer to an open group", 0));
            }
            if gid >= lookbehind {
                return Err(source.error(
                    "cannot refer to group defined in the same lookbehind subpattern",
                    0,
                ));
            }
        }
        Ok(())
    }
}

// ------------------------------------------------------------------------------------------------ writing the translation

/// A character as a `fancy-regex` literal (inside or outside a set).
fn lit(c: char) -> String {
    if c.is_ascii_alphanumeric() {
        c.to_string()
    } else {
        format!("\\x{{{:x}}}", c as u32)
    }
}

/// `\d \D \s \S \w \W` as the inside of a set (`negated`: it needs its own nested set).
fn category_in_set(category: char, ascii: bool) -> String {
    match (category, ascii) {
        ('d', false) => "\\p{Nd}".to_string(),
        ('D', false) => "\\P{Nd}".to_string(),
        ('d', true) => "0-9".to_string(),
        ('D', true) => "[^0-9]".to_string(),
        ('s', false) => "\\s\\x{1c}-\\x{1f}".to_string(),
        ('S', false) => "[^\\s\\x{1c}-\\x{1f}]".to_string(),
        ('s', true) => "\\x{9}-\\x{d}\\x{20}".to_string(),
        ('S', true) => "[^\\x{9}-\\x{d}\\x{20}]".to_string(),
        ('w', false) => "\\p{L}\\p{N}_".to_string(),
        ('W', false) => "[^\\p{L}\\p{N}_]".to_string(),
        ('w', true) => "a-zA-Z0-9_".to_string(),
        _ => "[^a-zA-Z0-9_]".to_string(),
    }
}

fn category_frag(category: char, ascii: bool) -> String {
    match category {
        'D' | 'S' | 'W' => category_in_set(category, ascii),
        _ => format!("[{}]", category_in_set(category, ascii)),
    }
}

fn word_set(ascii: bool) -> String {
    format!("[{}]", category_in_set('w', ascii))
}

/// `\b` and `\B` (Python 3.13: neither matches in the empty string).
fn boundary_frag(non: bool, ascii: bool) -> String {
    let w = word_set(ascii);
    if non {
        format!("(?:(?<={w})(?={w})|(?<!{w})(?!{w}))(?!\\A\\z)")
    } else {
        format!("(?:(?<={w})(?!{w})|(?<!{w})(?={w}))")
    }
}

fn is_ascii_mode(flags: u32) -> bool {
    flags & F_A != 0
}

// ------------------------------------------------------------------------------------------------ the parser

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SetElem {
    Literal(char),
    Category(char),
    Range(char, char),
}

/// What an escape is: a character, a category inside or outside sets, an anchor, or a group reference.
enum Escape {
    Literal(char),
    Category(char),
    At(char),
    GroupRef(usize),
}

const OCTDIGITS: &str = "01234567";
const DIGITS: &str = "0123456789";
const HEXDIGITS: &str = "0123456789abcdefABCDEF";

fn is_py_whitespace(this: &str) -> bool {
    matches!(this, " " | "\t" | "\n" | "\r" | "\u{b}" | "\u{c}")
}

fn single(this: &str) -> Option<char> {
    let mut chars = this.chars();
    let c = chars.next()?;
    chars.next().is_none().then_some(c)
}

fn escape_char(c: char) -> Option<char> {
    Some(match c {
        'a' => '\u{7}',
        'b' => '\u{8}',
        'f' => '\u{c}',
        'n' => '\n',
        'r' => '\r',
        't' => '\t',
        'v' => '\u{b}',
        '\\' => '\\',
        _ => return None,
    })
}

/// `_class_escape`: an escape inside a set.
fn class_escape(source: &mut Source, escape: &str) -> Result<SetElem, PatternError> {
    let mut chars = escape.chars();
    chars.next(); // the backslash
    let c = chars.next().unwrap_or('\\');
    if let Some(code) = escape_char(c) {
        return Ok(SetElem::Literal(code));
    }
    if matches!(c, 'd' | 'D' | 's' | 'S' | 'w' | 'W') {
        return Ok(SetElem::Category(c));
    }
    numeric_escape(source, escape, c, false).map(|esc| match esc {
        Escape::Literal(c) => SetElem::Literal(c),
        _ => SetElem::Literal('\0'),
    })
}

/// The escapes both `_class_escape` and `_escape` read: `\xhh \uhhhh \Uhhhhhhhh \N{..} \ooo` and an escaped punctuation character.
/// (`in_expression`: a decimal digit is an octal escape *or* a group reference; the caller handles that before it gets here.)
fn numeric_escape(
    source: &mut Source,
    escape: &str,
    c: char,
    in_expression: bool,
) -> Result<Escape, PatternError> {
    let mut escape = escape.to_string();
    let len = |e: &str| e.chars().count();
    match c {
        'x' => {
            escape.push_str(&source.getwhile(2, HEXDIGITS)?);
            if len(&escape) != 4 {
                return Err(source.error(&format!("incomplete escape {escape}"), len(&escape)));
            }
            let value = u32::from_str_radix(&escape[2..], 16).unwrap_or(0);
            return Ok(Escape::Literal(char::from_u32(value).unwrap_or('\u{fffd}')));
        }
        'u' => {
            escape.push_str(&source.getwhile(4, HEXDIGITS)?);
            if len(&escape) != 6 {
                return Err(source.error(&format!("incomplete escape {escape}"), len(&escape)));
            }
            let value = u32::from_str_radix(&escape[2..], 16).unwrap_or(0);
            return code_point(value, source, &escape);
        }
        'U' => {
            escape.push_str(&source.getwhile(8, HEXDIGITS)?);
            if len(&escape) != 10 {
                return Err(source.error(&format!("incomplete escape {escape}"), len(&escape)));
            }
            let value = u32::from_str_radix(&escape[2..], 16).unwrap_or(u32::MAX);
            if value > 0x10FFFF {
                // `chr(c)` raises ValueError: the message of the exception, not a re.error
                return Err(PatternError {
                    exception: "ValueError",
                    message: "chr() arg not in range(0x110000)".to_string(),
                });
            }
            return code_point(value, source, &escape);
        }
        'N' => {
            if !source.matches("{")? {
                return Err(source.error("missing {", 0));
            }
            let name = source.getuntil("}", "character name")?;
            return Err(PatternError {
                exception: "Unsupported",
                message: format!(
                    "\\N{{{name}}}: character names are not supported by the Rust port of re"
                ),
            });
        }
        _ => {}
    }
    if !in_expression && OCTDIGITS.contains(c) {
        // octal escape (up to three digits)
        escape.push_str(&source.getwhile(2, OCTDIGITS)?);
        let value = u32::from_str_radix(&escape[1..], 8).unwrap_or(0);
        if value > 0o377 {
            return Err(source.error(
                &format!("octal escape value {escape} outside of range 0-0o377"),
                len(&escape),
            ));
        }
        return Ok(Escape::Literal(char::from_u32(value).unwrap_or('\0')));
    }
    if !in_expression && DIGITS.contains(c) {
        return Err(source.error(&format!("bad escape {escape}"), len(&escape)));
    }
    if len(&escape) == 2 {
        if c.is_ascii_alphabetic() {
            return Err(source.error(&format!("bad escape {escape}"), len(&escape)));
        }
        return Ok(Escape::Literal(c));
    }
    Err(source.error(&format!("bad escape {escape}"), len(&escape)))
}

fn code_point(value: u32, source: &Source, escape: &str) -> Result<Escape, PatternError> {
    match char::from_u32(value) {
        Some(c) => Ok(Escape::Literal(c)),
        // a lone surrogate: Python reads it, a Rust string cannot hold it
        None => Err(PatternError {
            exception: "Unsupported",
            message: format!(
                "{escape}: a surrogate code point is not supported by the Rust port of re (at position {})",
                source.tell()
            ),
        }),
    }
}

/// `_escape`: an escape in the expression.
fn expression_escape(
    source: &mut Source,
    escape: &str,
    state: &State,
) -> Result<Escape, PatternError> {
    let mut chars = escape.chars();
    chars.next();
    let c = chars.next().unwrap_or('\\');
    match c {
        'A' | 'b' | 'B' | 'Z' => return Ok(Escape::At(c)),
        'd' | 'D' | 's' | 'S' | 'w' | 'W' => return Ok(Escape::Category(c)),
        _ => {}
    }
    if let Some(code) = escape_char(c) {
        return Ok(Escape::Literal(code));
    }
    if c == '0' {
        // octal escape
        let mut escape = escape.to_string();
        escape.push_str(&source.getwhile(2, OCTDIGITS)?);
        let value = u32::from_str_radix(&escape[1..], 8).unwrap_or(0);
        return Ok(Escape::Literal(char::from_u32(value).unwrap_or('\0')));
    }
    if DIGITS.contains(c) {
        // octal escape *or* decimal group reference (sigh)
        let mut escape = escape.to_string();
        if source.next.as_deref().is_some_and(|n| DIGITS.contains(n)) {
            escape.push_str(&source.get()?.unwrap_or_default());
            let e: Vec<char> = escape.chars().collect();
            if OCTDIGITS.contains(e[1])
                && OCTDIGITS.contains(e[2])
                && source
                    .next
                    .as_deref()
                    .is_some_and(|n| OCTDIGITS.contains(n))
            {
                // got three octal digits; this is an octal escape
                escape.push_str(&source.get()?.unwrap_or_default());
                let value = u32::from_str_radix(&escape[1..], 8).unwrap_or(0);
                if value > 0o377 {
                    return Err(source.error(
                        &format!("octal escape value {escape} outside of range 0-0o377"),
                        escape.chars().count(),
                    ));
                }
                return Ok(Escape::Literal(char::from_u32(value).unwrap_or('\0')));
            }
        }
        // not an octal escape, so this is a group reference
        let group: usize = escape[1..].parse().unwrap_or(usize::MAX);
        if group < state.groups() {
            if !state.check_group(group) {
                return Err(source.error("cannot refer to an open group", escape.chars().count()));
            }
            state.check_lookbehind_group(group, source)?;
            return Ok(Escape::GroupRef(group));
        }
        return Err(source.error(
            &format!("invalid group reference {group}"),
            escape.chars().count() - 1,
        ));
    }
    numeric_escape(source, escape, c, true)
}

/// What `(` turned out to be.
enum Group {
    Item(Item),
    /// A comment: nothing is added.
    Nothing,
    /// Global flags at the start: nothing is added, but the verbose setting may have changed.
    GlobalFlags,
}

struct Parser {
    source: Source,
    state: State,
    /// The flags in force at this point of the parse (global flags plus the scoped ones of the groups around it).
    scope: Vec<u32>,
}

impl Parser {
    fn flags(&self) -> u32 {
        *self.scope.last().unwrap_or(&0)
    }

    fn literal(&self, c: char) -> String {
        if self.flags() & F_I != 0 {
            lit(fold_char(c))
        } else {
            lit(c)
        }
    }

    fn unit(op: Op, frag: String) -> Item {
        Item {
            op,
            frag,
            lo: 1,
            hi: 1,
        }
    }

    /// `_parse_sub`: an alternation.
    fn parse_sub(&mut self, mut verbose: bool, nested: usize) -> Result<Seq, PatternError> {
        let mut items: Vec<Seq> = Vec::new();
        loop {
            let first = nested == 0 && items.is_empty();
            let seq = self.parse(verbose, nested + 1, first)?;
            items.push(seq);
            if !self.source.matches("|")? {
                break;
            }
            if nested == 0 {
                verbose = self.state.flags & F_X != 0;
            }
        }
        if items.len() == 1 {
            return Ok(items.pop().unwrap_or_default());
        }
        let lo = items.iter().map(|s| s.width().0).min().unwrap_or(0);
        let hi = items.iter().map(|s| s.width().1).max().unwrap_or(0);
        let frag = format!(
            "(?:{})",
            items.iter().map(Seq::frag).collect::<Vec<_>>().join("|")
        );
        Ok(Seq {
            items: vec![Item {
                op: Op::Branch,
                frag,
                lo,
                hi,
            }],
        })
    }

    /// `_parse`: a sequence.
    fn parse(
        &mut self,
        mut verbose: bool,
        nested: usize,
        first: bool,
    ) -> Result<Seq, PatternError> {
        let mut subpattern = Seq::default();
        // (the loop ends at the end of the pattern, or at `|` or `)`: the end of this subpattern)
        while let Some(this) = self.source.next.clone() {
            if this == "|" || this == ")" {
                break;
            }
            self.source.get()?;
            if verbose {
                // skip whitespace and comments
                if is_py_whitespace(&this) {
                    continue;
                }
                if this == "#" {
                    loop {
                        let c = self.source.get()?;
                        if c.is_none() || c.as_deref() == Some("\n") {
                            break;
                        }
                    }
                    continue;
                }
            }
            if this.starts_with('\\') {
                match expression_escape(&mut self.source, &this, &self.state)? {
                    Escape::Literal(c) => subpattern
                        .items
                        .push(Self::unit(Op::Literal, self.literal(c))),
                    Escape::Category(c) => {
                        let frag = category_frag(c, is_ascii_mode(self.flags()));
                        subpattern.items.push(Self::unit(Op::In, frag));
                    }
                    Escape::At(c) => {
                        let ascii = is_ascii_mode(self.flags());
                        let frag = match c {
                            'A' => "\\A".to_string(),
                            'Z' => "\\z".to_string(),
                            'b' => boundary_frag(false, ascii),
                            _ => boundary_frag(true, ascii),
                        };
                        subpattern.items.push(Item {
                            op: Op::At,
                            frag,
                            lo: 0,
                            hi: 0,
                        });
                    }
                    Escape::GroupRef(group) => {
                        let (lo, hi) = self.state.groupwidths[group].unwrap_or((0, 0));
                        subpattern.items.push(Item {
                            op: Op::GroupRef,
                            frag: format!("(?:\\{group})"),
                            lo,
                            hi,
                        });
                    }
                }
                continue;
            }
            match this.as_str() {
                "[" => {
                    let item = self.parse_set(nested)?;
                    subpattern.items.push(item);
                }
                "*" | "+" | "?" | "{" => self.parse_repeat(&this, &mut subpattern)?,
                "." => {
                    subpattern.items.push(Self::unit(Op::Any, ".".to_string()));
                }
                "(" => match self.parse_group(verbose, nested, first, &subpattern)? {
                    Group::Item(item) => subpattern.items.push(item),
                    Group::Nothing => {}
                    Group::GlobalFlags => verbose = self.state.flags & F_X != 0,
                },
                "^" => subpattern.items.push(Item {
                    op: Op::At,
                    frag: "^".to_string(),
                    lo: 0,
                    hi: 0,
                }),
                "$" => {
                    let frag = if self.flags() & F_M != 0 {
                        "$".to_string()
                    } else {
                        // Python's `$` also matches just before a final newline
                        "(?:\\z|(?=\\n\\z))".to_string()
                    };
                    subpattern.items.push(Item {
                        op: Op::At,
                        frag,
                        lo: 0,
                        hi: 0,
                    });
                }
                other => {
                    let c = single(other).unwrap_or('\0');
                    subpattern
                        .items
                        .push(Self::unit(Op::Literal, self.literal(c)));
                }
            }
        }
        Ok(subpattern)
    }

    /// A character set: `[...]`.
    fn parse_set(&mut self, _nested: usize) -> Result<Item, PatternError> {
        let here = self.source.tell() - 1;
        let mut set: Vec<SetElem> = Vec::new();
        let negate = self.source.matches("^")?;
        loop {
            let Some(this) = self.source.get()? else {
                return Err(self
                    .source
                    .error("unterminated character set", self.source.tell() - here));
            };
            if this == "]" && !set.is_empty() {
                break;
            }
            let code1 = if this.starts_with('\\') {
                class_escape(&mut self.source, &this)?
            } else {
                SetElem::Literal(single(&this).unwrap_or('\0'))
            };
            if self.source.matches("-")? {
                // potential range
                let Some(that) = self.source.get()? else {
                    return Err(self
                        .source
                        .error("unterminated character set", self.source.tell() - here));
                };
                if that == "]" {
                    set.push(code1);
                    set.push(SetElem::Literal('-'));
                    break;
                }
                let code2 = if that.starts_with('\\') {
                    class_escape(&mut self.source, &that)?
                } else {
                    SetElem::Literal(single(&that).unwrap_or('\0'))
                };
                let (SetElem::Literal(lo), SetElem::Literal(hi)) = (code1, code2) else {
                    let msg = format!("bad character range {this}-{that}");
                    return Err(self
                        .source
                        .error(&msg, this.chars().count() + 1 + that.chars().count()));
                };
                if hi < lo {
                    let msg = format!("bad character range {this}-{that}");
                    return Err(self
                        .source
                        .error(&msg, this.chars().count() + 1 + that.chars().count()));
                }
                set.push(SetElem::Range(lo, hi));
            } else {
                set.push(code1);
            }
        }
        let ascii = is_ascii_mode(self.flags());
        let mut frag = String::from("[");
        if negate {
            frag.push('^');
        }
        for element in &set {
            match element {
                SetElem::Literal(c) => frag.push_str(&lit(if self.flags() & F_I != 0 {
                    fold_char(*c)
                } else {
                    *c
                })),
                SetElem::Range(lo, hi) => {
                    frag.push_str(&lit(*lo));
                    frag.push('-');
                    frag.push_str(&lit(*hi));
                }
                SetElem::Category(c) => frag.push_str(&category_in_set(*c, ascii)),
            }
        }
        frag.push(']');
        Ok(Self::unit(Op::In, frag))
    }

    /// `*`, `+`, `?` and `{m,n}`: repeat the item before.
    fn parse_repeat(&mut self, this: &str, subpattern: &mut Seq) -> Result<(), PatternError> {
        let here = self.source.tell();
        let (min, max): (u128, u128);
        match this {
            "?" => (min, max) = (0, 1),
            "*" => (min, max) = (0, MAXREPEAT),
            "+" => (min, max) = (1, MAXREPEAT),
            _ => {
                if self.source.next_is("}") {
                    subpattern
                        .items
                        .push(Self::unit(Op::Literal, self.literal('{')));
                    return Ok(());
                }
                let mut lo = String::new();
                let mut hi = String::new();
                while self
                    .source
                    .next
                    .as_deref()
                    .is_some_and(|n| DIGITS.contains(n))
                {
                    lo.push_str(&self.source.get()?.unwrap_or_default());
                }
                if self.source.matches(",")? {
                    while self
                        .source
                        .next
                        .as_deref()
                        .is_some_and(|n| DIGITS.contains(n))
                    {
                        hi.push_str(&self.source.get()?.unwrap_or_default());
                    }
                } else {
                    hi = lo.clone();
                }
                if !self.source.matches("}")? {
                    subpattern
                        .items
                        .push(Self::unit(Op::Literal, self.literal('{')));
                    self.source.seek(here)?;
                    return Ok(());
                }
                let mut min_value = 0u128;
                let mut max_value = MAXREPEAT;
                let too_large = || PatternError {
                    exception: "OverflowError",
                    message: "the repetition number is too large".to_string(),
                };
                if !lo.is_empty() {
                    min_value = lo.parse::<u128>().map_err(|_| too_large())?;
                    if min_value >= MAXREPEAT {
                        return Err(too_large());
                    }
                }
                if !hi.is_empty() {
                    max_value = hi.parse::<u128>().map_err(|_| too_large())?;
                    if max_value >= MAXREPEAT {
                        return Err(too_large());
                    }
                    if max_value < min_value {
                        return Err(self.source.error(
                            "min repeat greater than max repeat",
                            self.source.tell() - here,
                        ));
                    }
                }
                (min, max) = (min_value, max_value);
            }
        }
        // figure out which item to repeat
        let offset = self.source.tell() - here + this.chars().count();
        let Some(item) = subpattern.items.last().cloned() else {
            return Err(self.source.error("nothing to repeat", offset));
        };
        if item.op == Op::At {
            return Err(self.source.error("nothing to repeat", offset));
        }
        if item.op == Op::Repeat {
            return Err(self.source.error("multiple repeat", offset));
        }
        let quantifier = if min == 0 && max == 1 {
            "?".to_string()
        } else if min == 0 && max == MAXREPEAT {
            "*".to_string()
        } else if min == 1 && max == MAXREPEAT {
            "+".to_string()
        } else if max == MAXREPEAT {
            format!("{{{min},}}")
        } else if min == max {
            format!("{{{min}}}")
        } else {
            format!("{{{min},{max}}}")
        };
        let frag = item.frag.clone();
        let (ilo, ihi) = (item.lo, item.hi);
        let (mut lo, mut hi) = (ilo * min, 0);
        if max == MAXREPEAT && ihi > 0 {
            hi = MAXWIDTH;
        } else {
            hi += ihi * max;
        }
        lo = lo.min(MAXWIDTH);
        hi = hi.min(MAXWIDTH);
        let lazy = self.source.matches("?")?;
        let possessive = !lazy && self.source.matches("+")?;
        let new_frag = if ihi == 0 {
            // an item that consumes nothing (an assertion, an empty group): repeating it is the item once, or the item or nothing when
            // zero repeats are allowed (written so that its groups stay; the engine refuses to repeat an empty pattern)
            if min == 0 && lazy {
                format!("(?:|{frag})")
            } else if min == 0 {
                format!("(?:{frag}|)")
            } else {
                frag
            }
        } else if lazy {
            format!("{frag}{quantifier}?")
        } else if possessive {
            format!("(?>{frag}{quantifier})")
        } else {
            format!("{frag}{quantifier}")
        };
        let last = subpattern.items.len() - 1;
        subpattern.items[last] = Item {
            op: Op::Repeat,
            frag: new_frag,
            lo,
            hi,
        };
        Ok(())
    }

    /// `(`: a group, an assertion, a flag group, a comment, a conditional. `None` when nothing is added (comment, global flags).
    fn parse_group(
        &mut self,
        verbose: bool,
        nested: usize,
        first: bool,
        subpattern: &Seq,
    ) -> Result<Group, PatternError> {
        let start = self.source.tell() - 1;
        let mut capture = true;
        let mut atomic = false;
        let mut name: Option<String> = None;
        let mut add_flags = 0u32;
        let mut del_flags = 0u32;
        if self.source.matches("?")? {
            // options
            let Some(char_) = self.source.get()? else {
                return Err(self.source.error("unexpected end of pattern", 0));
            };
            let mut char_ = char_;
            if char_ == "P" {
                // python extensions
                if self.source.matches("<")? {
                    // named group: skip forward to end of name
                    let n = self.source.getuntil(">", "group name")?;
                    self.source.check_group_name(&n, 1)?;
                    name = Some(n);
                } else if self.source.matches("=")? {
                    // named backreference
                    let n = self.source.getuntil(")", "group name")?;
                    self.source.check_group_name(&n, 1)?;
                    let Some(gid) = self.state.groupdict.get(&n).copied() else {
                        let msg = format!("unknown group name {}", py_repr(&n));
                        return Err(self.source.error(&msg, n.chars().count() + 1));
                    };
                    if !self.state.check_group(gid) {
                        return Err(self
                            .source
                            .error("cannot refer to an open group", n.chars().count() + 1));
                    }
                    self.state.check_lookbehind_group(gid, &self.source)?;
                    let (lo, hi) = self.state.groupwidths[gid].unwrap_or((0, 0));
                    return Ok(Group::Item(Item {
                        op: Op::GroupRef,
                        frag: format!("(?:\\{gid})"),
                        lo,
                        hi,
                    }));
                } else {
                    let Some(c) = self.source.get()? else {
                        return Err(self.source.error("unexpected end of pattern", 0));
                    };
                    return Err(self
                        .source
                        .error(&format!("unknown extension ?P{c}"), c.chars().count() + 2));
                }
            } else if char_ == ":" {
                capture = false; // non-capturing group
            } else if char_ == "#" {
                // comment
                loop {
                    if self.source.next.is_none() {
                        return Err(self.source.error(
                            "missing ), unterminated comment",
                            self.source.tell() - start,
                        ));
                    }
                    if self.source.get()?.as_deref() == Some(")") {
                        break;
                    }
                }
                return Ok(Group::Nothing);
            } else if char_ == "=" || char_ == "!" || char_ == "<" {
                // lookahead assertions
                let mut behind = false;
                let mut previous_lookbehind: Option<Option<usize>> = None;
                if char_ == "<" {
                    let Some(c) = self.source.get()? else {
                        return Err(self.source.error("unexpected end of pattern", 0));
                    };
                    if c != "=" && c != "!" {
                        return Err(self
                            .source
                            .error(&format!("unknown extension ?<{c}"), c.chars().count() + 2));
                    }
                    char_ = c;
                    behind = true;
                    previous_lookbehind = Some(self.state.lookbehindgroups);
                    if self.state.lookbehindgroups.is_none() {
                        self.state.lookbehindgroups = Some(self.state.groups());
                    }
                }
                let p = self.parse_sub(verbose, nested + 1)?;
                if behind {
                    if let Some(None) = previous_lookbehind {
                        self.state.lookbehindgroups = None;
                    }
                }
                if !self.source.matches(")")? {
                    return Err(self.source.error(
                        "missing ), unterminated subpattern",
                        self.source.tell() - start,
                    ));
                }
                if behind {
                    let (lo, hi) = p.width();
                    if lo != hi && self.state.compile_error.is_none() {
                        self.state.compile_error =
                            Some(plain_error("look-behind requires fixed-width pattern"));
                    }
                }
                let frag = if char_ == "=" {
                    format!("(?{}{})", if behind { "<=" } else { "=" }, p.frag())
                } else if !p.is_empty() {
                    format!("(?{}{})", if behind { "<!" } else { "!" }, p.frag())
                } else {
                    "(*FAIL)".to_string()
                };
                return Ok(Group::Item(Item {
                    op: if char_ == "=" || !p.is_empty() {
                        Op::Assert
                    } else {
                        Op::Failure
                    },
                    frag,
                    lo: 0,
                    hi: 0,
                }));
            } else if char_ == "(" {
                // conditional backreference group
                let condname = self.source.getuntil(")", "group name")?;
                let decimal = !condname.is_empty() && condname.chars().all(|c| c.is_ascii_digit());
                let condgroup: usize;
                if !decimal {
                    self.source.check_group_name(&condname, 1)?;
                    let Some(g) = self.state.groupdict.get(&condname).copied() else {
                        let msg = format!("unknown group name {}", py_repr(&condname));
                        return Err(self.source.error(&msg, condname.chars().count() + 1));
                    };
                    condgroup = g;
                } else {
                    condgroup = condname.parse().unwrap_or(usize::MAX);
                    if condgroup == 0 {
                        return Err(self
                            .source
                            .error("bad group number", condname.chars().count() + 1));
                    }
                    if condgroup >= MAXGROUPS {
                        let msg = format!("invalid group reference {condgroup}");
                        return Err(self.source.error(&msg, condname.chars().count() + 1));
                    }
                    if !self.state.grouprefpos.iter().any(|(g, _)| *g == condgroup) {
                        self.state
                            .grouprefpos
                            .push((condgroup, self.source.tell() - condname.chars().count() - 1));
                    }
                }
                self.state.check_lookbehind_group(condgroup, &self.source)?;
                let item_yes = self.parse(verbose, nested + 1, false)?;
                let item_no = if self.source.matches("|")? {
                    let no = self.parse(verbose, nested + 1, false)?;
                    if self.source.next_is("|") {
                        return Err(self
                            .source
                            .error("conditional backref with more than two branches", 0));
                    }
                    Some(no)
                } else {
                    None
                };
                if !self.source.matches(")")? {
                    return Err(self.source.error(
                        "missing ), unterminated subpattern",
                        self.source.tell() - start,
                    ));
                }
                let (mut lo, mut hi) = item_yes.width();
                match &item_no {
                    Some(no) => {
                        let (l, h) = no.width();
                        lo = lo.min(l);
                        hi = hi.max(h);
                    }
                    None => lo = 0,
                }
                let frag = match &item_no {
                    Some(no) => format!("(?({condgroup}){}|{})", item_yes.frag(), no.frag()),
                    None => format!("(?({condgroup}){})", item_yes.frag()),
                };
                return Ok(Group::Item(Item {
                    op: Op::GroupRefExists,
                    frag,
                    lo,
                    hi,
                }));
            } else if char_ == ">" {
                // non-capturing, atomic group
                capture = false;
                atomic = true;
            } else if matches!(char_.as_str(), "i" | "L" | "m" | "s" | "x" | "a" | "u")
                || char_ == "-"
            {
                // flags
                let parsed = self.parse_flags(&char_)?;
                match parsed {
                    None => {
                        // global flags
                        if !first || !subpattern.is_empty() {
                            return Err(self.source.error(
                                "global flags not at the start of the expression",
                                self.source.tell() - start,
                            ));
                        }
                        let flags = self.state.flags;
                        if let Some(top) = self.scope.last_mut() {
                            *top |= flags;
                        }
                        return Ok(Group::GlobalFlags);
                    }
                    Some((add, del)) => {
                        add_flags = add;
                        del_flags = del;
                        capture = false;
                    }
                }
            } else {
                return Err(self.source.error(
                    &format!("unknown extension ?{char_}"),
                    char_.chars().count() + 1,
                ));
            }
        }

        // parse group contents
        let group = if capture {
            match self.state.open_group(name.as_deref()) {
                Ok(g) => Some(g),
                Err(err) => {
                    let n = name.as_deref().map_or(0, |n| n.chars().count());
                    return Err(self.source.error(&err.message, n + 1));
                }
            }
        } else {
            None
        };
        let sub_verbose = (verbose || add_flags & F_X != 0) && del_flags & F_X == 0;
        let outer = self.flags();
        let inner = (outer | add_flags) & !del_flags;
        self.scope.push(inner);
        let p = self.parse_sub(sub_verbose, nested + 1);
        self.scope.pop();
        let p = p?;
        if !self.source.matches(")")? {
            return Err(self.source.error(
                "missing ), unterminated subpattern",
                self.source.tell() - start,
            ));
        }
        let (lo, hi) = p.width();
        if let Some(group) = group {
            self.state.groupwidths[group] = Some((lo, hi));
        }
        let body = p.frag();
        let frag = if atomic {
            format!("(?>{body})")
        } else if group.is_some() {
            format!("({body})")
        } else {
            let mut flags = String::new();
            for (bit, letter) in [(F_I, 'i'), (F_M, 'm'), (F_S, 's')] {
                if add_flags & bit != 0 {
                    flags.push(letter);
                }
            }
            let mut off = String::new();
            for (bit, letter) in [(F_I, 'i'), (F_M, 'm'), (F_S, 's')] {
                if del_flags & bit != 0 {
                    off.push(letter);
                }
            }
            if off.is_empty() {
                format!("(?{flags}:{body})")
            } else {
                format!("(?{flags}-{off}:{body})")
            }
        };
        Ok(Group::Item(Item {
            op: if atomic { Op::Atomic } else { Op::Subpattern },
            frag,
            lo,
            hi,
        }))
    }

    /// `_parse_flags`. `None`: global flags (applied to the state).
    fn parse_flags(&mut self, first_char: &str) -> Result<Option<(u32, u32)>, PatternError> {
        let flag_of = |c: &str| -> Option<u32> {
            match c {
                "i" => Some(F_I),
                "L" => Some(F_L),
                "m" => Some(F_M),
                "s" => Some(F_S),
                "x" => Some(F_X),
                "a" => Some(F_A),
                "u" => Some(F_U),
                _ => None,
            }
        };
        let mut char_ = first_char.to_string();
        let mut add_flags = 0u32;
        let mut del_flags = 0u32;
        if char_ != "-" {
            loop {
                let flag = flag_of(&char_).unwrap_or(0);
                if char_ == "L" {
                    return Err(self.source.error(
                        "bad inline flags: cannot use 'L' flag with a str pattern",
                        0,
                    ));
                }
                add_flags |= flag;
                if flag & TYPE_FLAGS != 0 && add_flags & TYPE_FLAGS != flag {
                    return Err(self.source.error(
                        "bad inline flags: flags 'a', 'u' and 'L' are incompatible",
                        0,
                    ));
                }
                let Some(next) = self.source.get()? else {
                    return Err(self.source.error("missing -, : or )", 0));
                };
                char_ = next;
                if char_ == ")" || char_ == "-" || char_ == ":" {
                    break;
                }
                if flag_of(&char_).is_none() {
                    let msg = if char_.chars().next().is_some_and(char::is_alphabetic) {
                        "unknown flag"
                    } else {
                        "missing -, : or )"
                    };
                    return Err(self.source.error(msg, char_.chars().count()));
                }
            }
        }
        if char_ == ")" {
            self.state.flags |= add_flags;
            return Ok(None);
        }
        if char_ == "-" {
            let Some(next) = self.source.get()? else {
                return Err(self.source.error("missing flag", 0));
            };
            char_ = next;
            if flag_of(&char_).is_none() {
                let msg = if char_.chars().next().is_some_and(char::is_alphabetic) {
                    "unknown flag"
                } else {
                    "missing flag"
                };
                return Err(self.source.error(msg, char_.chars().count()));
            }
            loop {
                let flag = flag_of(&char_).unwrap_or(0);
                if flag & TYPE_FLAGS != 0 {
                    return Err(self.source.error(
                        "bad inline flags: cannot turn off flags 'a', 'u' and 'L'",
                        0,
                    ));
                }
                del_flags |= flag;
                let Some(next) = self.source.get()? else {
                    return Err(self.source.error("missing :", 0));
                };
                char_ = next;
                if char_ == ":" {
                    break;
                }
                if flag_of(&char_).is_none() {
                    let msg = if char_.chars().next().is_some_and(char::is_alphabetic) {
                        "unknown flag"
                    } else {
                        "missing :"
                    };
                    return Err(self.source.error(msg, char_.chars().count()));
                }
            }
        }
        if add_flags & del_flags != 0 {
            return Err(self
                .source
                .error("bad inline flags: flag turned on and off", 1));
        }
        Ok(Some((add_flags, del_flags)))
    }
}

/// `re.IGNORECASE` equates `ı` and `İ` with `i`; the engine does not, so both sides are written with `i`.
pub fn fold_char(c: char) -> char {
    match c {
        'ı' | 'İ' => 'i',
        _ => c,
    }
}

/// The text with `ı` and `İ` written as `i` (the same number of characters).
pub fn fold_dotted_i(text: &str) -> String {
    text.chars().map(fold_char).collect()
}

// ------------------------------------------------------------------------------------------------ the compiled pattern

/// A Python pattern, compiled (`re.compile(pattern, re.IGNORECASE)` or without the flag).
pub struct PyRegex {
    regex: Regex,
    fold: bool,
    names: HashMap<String, usize>,
    /// The pattern as written (`re.Pattern.pattern`).
    pub pattern: String,
}

impl fmt::Debug for PyRegex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PyRegex({})", py_repr(&self.pattern))
    }
}

/// What a search found, read from the original text (`re.Match`).
#[derive(Debug, Clone)]
pub struct PyCaptures {
    groups: Vec<Option<(usize, usize, String)>>,
    names: HashMap<String, usize>,
}

impl PyCaptures {
    /// `match.group(i)`: the text, or `None` when the group did not take part.
    pub fn group(&self, index: usize) -> Option<&str> {
        self.groups
            .get(index)
            .and_then(|g| g.as_ref())
            .map(|(_, _, text)| text.as_str())
    }

    /// `match.group(name)`.
    pub fn name(&self, name: &str) -> Option<&str> {
        self.names.get(name).and_then(|i| self.group(*i))
    }

    /// `match.start()` / `match.end()` in characters.
    pub fn span(&self) -> (usize, usize) {
        self.groups
            .first()
            .and_then(|g| g.as_ref())
            .map_or((0, 0), |(s, e, _)| (*s, *e))
    }

    /// `match.span(i)` in characters (`None`: the group did not take part).
    pub fn group_span(&self, index: usize) -> Option<(usize, usize)> {
        self.groups
            .get(index)
            .and_then(|g| g.as_ref())
            .map(|(s, e, _)| (*s, *e))
    }

    pub fn len(&self) -> usize {
        self.groups.len()
    }

    pub fn is_empty(&self) -> bool {
        self.groups.is_empty()
    }
}

impl PyRegex {
    /// `re.compile(pattern, re.IGNORECASE if ignore_case else 0)`.
    pub fn new(pattern: &str, ignore_case: bool) -> Result<Self, PatternError> {
        let translated = translate(pattern, ignore_case)?;
        let mut builder = RegexBuilder::new(&translated.pattern);
        builder.backtrack_limit(50_000_000);
        let regex = builder.build().map_err(|e| PatternError {
            exception: "Unsupported",
            message: format!("{e}"),
        })?;
        Ok(Self {
            regex,
            fold: translated.fold,
            names: translated.names,
            pattern: pattern.to_string(),
        })
    }

    /// `pattern.search(text)`.
    pub fn search(&self, text: &str) -> Option<PyCaptures> {
        let folded;
        let haystack: &str = if self.fold {
            folded = fold_dotted_i(text);
            &folded
        } else {
            text
        };
        let caps = self.regex.captures(haystack).ok().flatten()?;
        // byte offsets in `haystack` -> char offsets (the fold keeps the number of characters)
        let char_index = |byte: usize| haystack[..byte].chars().count();
        let original: Vec<char> = text.chars().collect();
        let groups = (0..caps.len())
            .map(|i| {
                caps.get(i).map(|m| {
                    let (s, e) = (char_index(m.start()), char_index(m.end()));
                    (s, e, original[s..e].iter().collect::<String>())
                })
            })
            .collect();
        Some(PyCaptures {
            groups,
            names: self.names.clone(),
        })
    }

    /// `pattern.search(text) is not None`.
    pub fn is_match(&self, text: &str) -> bool {
        let folded;
        let haystack: &str = if self.fold {
            folded = fold_dotted_i(text);
            &folded
        } else {
            text
        };
        self.regex.is_match(haystack).unwrap_or(false)
    }
}

struct Translated {
    pattern: String,
    names: HashMap<String, usize>,
    /// The text has to be written with `ı` and `İ` as `i` (the whole pattern is case-insensitive).
    fold: bool,
}

fn translate(pattern: &str, ignore_case: bool) -> Result<Translated, PatternError> {
    let source = Source::new(pattern)?;
    let flags = if ignore_case { F_I } else { 0 };
    let mut parser = Parser {
        source,
        state: State {
            flags,
            groupdict: HashMap::new(),
            groupwidths: vec![None],
            lookbehindgroups: None,
            grouprefpos: Vec::new(),
            names: Vec::new(),
            compile_error: None,
        },
        scope: vec![flags],
    };
    let verbose = flags & F_X != 0;
    let body = parser.parse_sub(verbose, 0)?;
    if parser.source.next.is_some() {
        return Err(parser.source.error("unbalanced parenthesis", 0));
    }
    for (group, position) in &parser.state.grouprefpos {
        if *group >= parser.state.groups() {
            return Err(re_error(
                &format!("invalid group reference {group}"),
                &parser.source.chars,
                *position as i64,
            ));
        }
    }
    if let Some(error) = parser.state.compile_error.take() {
        return Err(error);
    }
    let flags = parser.state.flags;
    // `fix_flags`: the checks `re.compile` makes once the flags are all known
    if flags & F_A != 0 && flags & F_U != 0 {
        return Err(PatternError {
            exception: "ValueError",
            message: "ASCII and UNICODE flags are incompatible".to_string(),
        });
    }
    if flags & F_A != 0 && flags & F_I != 0 {
        return Err(PatternError {
            exception: "Unsupported",
            message: "the ASCII flag (?a) together with IGNORECASE is not supported by the Rust port of re".to_string(),
        });
    }
    let mut prefix = String::new();
    for (bit, letter) in [(F_I, 'i'), (F_M, 'm'), (F_S, 's')] {
        if flags & bit != 0 {
            prefix.push(letter);
        }
    }
    let mut out = String::new();
    if !prefix.is_empty() {
        out.push_str(&format!("(?{prefix})"));
    }
    out.push_str("(?:");
    out.push_str(&body.frag());
    out.push(')');
    Ok(Translated {
        pattern: out,
        names: parser.state.names.into_iter().collect(),
        fold: flags & F_I != 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn err(pattern: &str) -> String {
        PyRegex::new(pattern, true).unwrap_err().to_string()
    }

    #[test]
    fn python_error_messages() {
        assert_eq!(err("("), "missing ), unterminated subpattern at position 0");
        assert_eq!(err(")"), "unbalanced parenthesis at position 0");
        assert_eq!(err("a**"), "multiple repeat at position 2");
        assert_eq!(err("[a"), "unterminated character set at position 0");
        assert_eq!(
            err("a{2,1}"),
            "min repeat greater than max repeat at position 2"
        );
        assert_eq!(err("\\"), "bad escape (end of pattern) at position 0");
        assert_eq!(
            err("(?P<1>a)"),
            "bad character in group name '1' at position 4"
        );
        assert_eq!(err("(?<a)"), "unknown extension ?<a at position 1");
        assert_eq!(err("[z-a]"), "bad character range z-a at position 1");
        assert_eq!(err("\\y"), "bad escape \\y at position 0");
        assert_eq!(
            err("(?i)a(?i)b"),
            "global flags not at the start of the expression at position 5"
        );
        assert_eq!(err("(?<=a+)b"), "look-behind requires fixed-width pattern");
        assert_eq!(err("\\1"), "invalid group reference 1 at position 1");
        assert_eq!(err("(a)\\2"), "invalid group reference 2 at position 4");
        assert_eq!(err("[\\d-z]"), "bad character range \\d-z at position 1");
        assert_eq!(err("x{99999999999}"), "the repetition number is too large");
        assert_eq!(
            err("(?P<a>x)(?P<a>y)"),
            "redefinition of group name 'a' as group 2; was group 1 at position 12"
        );
        assert_eq!(err("*a"), "nothing to repeat at position 0");
        assert_eq!(
            err("a\n(b"),
            "missing ), unterminated subpattern at position 2 (line 2, column 1)"
        );
    }

    #[test]
    fn python_meaning() {
        let re = PyRegex::new(r"^\s*(?P<word>show|play)\b", true).unwrap();
        let caps = re.search("  Show the photo").unwrap();
        assert_eq!(caps.name("word"), Some("Show"));
        assert!(re.search("showing").is_none());
        // `\Z`, `{,2}`, a `{` that is not a quantifier, `[` inside a set
        assert!(PyRegex::new(r"a{,2}b\Z", false).unwrap().is_match("aab"));
        assert!(PyRegex::new(r"x{y", false).unwrap().is_match("x{y"));
        assert!(PyRegex::new(r"[[]a", false).unwrap().is_match("[a"));
        // `$` matches before a final newline
        assert!(PyRegex::new(r"a$", false).unwrap().is_match("a\n"));
        // IGNORECASE: dotless and dotted i are `i`; the Kelvin sign is `k`
        assert!(PyRegex::new("tbd", true).unwrap().is_match("TBD"));
        assert!(PyRegex::new("conclusion", true)
            .unwrap()
            .is_match("CONCLUSİON"));
        assert!(PyRegex::new("ı", true).unwrap().is_match("I"));
        assert!(PyRegex::new("k", true).unwrap().is_match("\u{212a}"));
        // a combining mark ends a word for Python's `\b`
        assert!(PyRegex::new(r"show\b", true)
            .unwrap()
            .is_match("show\u{301}"));
        // an assertion or an empty group can be repeated; a group number is not glued to the digit after it
        assert!(PyRegex::new(r"(?=a)*a", false).unwrap().is_match("a"));
        assert!(PyRegex::new(r"(?:)+?a(?<=a)*", false)
            .unwrap()
            .is_match("a"));
        assert!(PyRegex::new(r"(a)\1[1]", false).unwrap().is_match("aa1"));
        // backreference and conditional
        assert!(PyRegex::new(r"(a)\1", false).unwrap().is_match("aa"));
        assert!(PyRegex::new(r"(a)?(?(1)b|c)", false)
            .unwrap()
            .is_match("ab"));
        // positions of captures are characters
        let caps = PyRegex::new(r"é(x)", false).unwrap().search("ééx").unwrap();
        assert_eq!(caps.span(), (1, 3));
        assert_eq!(caps.group(1), Some("x"));
    }

    #[test]
    fn repr_of_patterns() {
        assert_eq!(py_repr("^Closing"), "'^Closing'");
        assert_eq!(py_repr("it's"), "\"it's\"");
        assert_eq!(py_repr("a\\b\n"), "'a\\\\b\\n'");
        assert_eq!(py_repr("\u{1}é"), "'\\x01é'");
    }
}
