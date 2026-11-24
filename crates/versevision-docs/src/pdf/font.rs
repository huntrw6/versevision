//! A port of pypdf's `Font.from_font_resource` and `_cmap.get_encoding` (pypdf 6.14): how a font dictionary turns the bytes of a
//! text string into characters and widths.
use std::collections::HashMap;

use lopdf::{Dictionary, Object};

use super::data::tables;
use super::Pdf;

/// An insertion-ordered `dict[str, str]` (the order matters: `_get_space_char` takes the first key that maps to a space).
#[derive(Default, Debug)]
pub struct CharMap {
    keys: Vec<String>,
    values: HashMap<String, String>,
}

impl CharMap {
    pub fn insert(&mut self, key: String, value: String) {
        if self.values.insert(key.clone(), value).is_none() {
            self.keys.push(key);
        }
    }

    pub fn get(&self, key: &str) -> Option<&String> {
        self.values.get(key)
    }

    fn first_key_with_value(&self, value: &str) -> Option<&String> {
        self.keys
            .iter()
            .find(|k| self.values.get(*k).map(String::as_str) == Some(value))
    }
}

#[derive(Debug, Clone)]
pub enum Encoding {
    /// A Python codec name: `charmap` (Latin-1), `utf-16-be`, or one this port cannot decode (`gbk`, `cp932`, ...).
    Codec(String),
    /// `dict[int, str]` over the codes 0..=255.
    Table(Vec<String>),
}

#[derive(Debug)]
pub struct Font {
    pub encoding: Encoding,
    pub character_map: CharMap,
    pub widths: HashMap<char, f64>,
    pub default_width: f64,
    pub space_width: f64,
    pub space_char: String,
}

/// What stops a font from loading. `Skip` is `(AttributeError, TypeError)` (the page extractor ignores the font);
/// `Fail` is any other exception (the page cannot be extracted).
pub enum FontError {
    Skip,
    Fail(String),
}

impl Font {
    /// `Font(name="NotInitialized", encoding="charmap")`: the font before the first `Tf`.
    pub fn not_initialized() -> Font {
        Font {
            encoding: Encoding::Codec("charmap".into()),
            character_map: CharMap::default(),
            widths: HashMap::new(),
            default_width: 500.0,
            space_width: 250.0,
            space_char: " ".into(),
        }
    }

    /// The font an unknown `Tf` name falls back to.
    pub fn unknown() -> Font {
        Font {
            encoding: Encoding::Table(vec!["\u{fffd}".to_string(); 256]),
            character_map: CharMap::default(),
            widths: HashMap::new(),
            default_width: 500.0,
            space_width: 250.0,
            space_char: " ".into(),
        }
    }

    pub fn text_width(&self, c: char) -> f64 {
        self.widths.get(&c).copied().unwrap_or(self.default_width)
    }

    pub fn is_space_char(&self, c: char) -> bool {
        let mut chars = self.space_char.chars();
        chars.next() == Some(c) && chars.next().is_none()
    }

    pub fn from_resource(pdf: &Pdf, font: &Dictionary) -> Result<Font, FontError> {
        let name = pdf
            .name_of(font, "BaseFont")
            .unwrap_or_else(|| "Unknown".to_string());
        let subtype = pdf.name_of(font, "Subtype");
        let (encoding, character_map) = get_encoding(pdf, font);
        let mut widths: HashMap<char, f64> = HashMap::new();
        let mut default: Option<f64> = None;
        let mut flags: i64 = 32;
        let core = tables().core_fonts.get(&name);
        match subtype.as_deref() {
            Some("Type1") | Some("MMType1") | Some("TrueType") | Some("Type3") => {
                let mut interpretable = true;
                if subtype.as_deref() == Some("Type3") && !pdf.has(font, "ToUnicode") {
                    // every CharProcs name must be an Adobe glyph; `or []` for a missing/null /CharProcs
                    interpretable = match pdf.get(font, "CharProcs") {
                        Some(Object::Dictionary(procs)) => procs.iter().all(|(k, _)| {
                            tables()
                                .glyphs
                                .contains_key(&format!("/{}", String::from_utf8_lossy(k)))
                        }),
                        _ => true,
                    };
                }
                if interpretable {
                    if pdf.has(font, "Widths") {
                        collect_simple_widths(pdf, font, &mut widths)?;
                    } else if let Some(core) = core {
                        flags = core.flags;
                        match &encoding {
                            Encoding::Table(table) => {
                                for (code, character) in table.iter().enumerate() {
                                    let mut chars = character.chars();
                                    if let (Some(c), None) = (chars.next(), chars.next()) {
                                        if let Some(&w) = core.widths.get(&c) {
                                            widths.insert(
                                                char::from_u32(code as u32).unwrap_or('\u{fffd}'),
                                                w as f64,
                                            );
                                        }
                                    }
                                }
                            }
                            Encoding::Codec(_) => {
                                for code in 0..256u32 {
                                    let c = char::from_u32(code).unwrap_or('\u{fffd}');
                                    if let Some(&w) = core.widths.get(&c) {
                                        widths.insert(c, w as f64);
                                    }
                                }
                            }
                        }
                    }
                    if let Some(descriptor) = pdf.get(font, "FontDescriptor") {
                        let Object::Dictionary(descriptor) = descriptor else {
                            return Err(FontError::Skip);
                        };
                        if let Some(missing) = pdf.get(descriptor, "MissingWidth") {
                            default = Some(number(missing).ok_or(FontError::Skip)?);
                        }
                        flags = descriptor_flags(pdf, descriptor)?;
                    } else if pdf.has(font, "FontBBox") {
                        flags = 32;
                    }
                }
            }
            _ => {
                let Some(Object::Array(descendants)) = pdf.get(font, "DescendantFonts") else {
                    return Err(FontError::Fail("'/DescendantFonts'".to_string()));
                };
                for d_font in descendants {
                    let Object::Dictionary(d_font) = pdf.deref(d_font) else {
                        return Err(FontError::Skip);
                    };
                    collect_cid_widths(pdf, d_font, &mut widths);
                    if let Some(dw) = pdf.get(d_font, "DW") {
                        default = Some(number(dw).ok_or(FontError::Skip)?);
                    }
                    flags = match pdf.get(d_font, "FontDescriptor") {
                        Some(Object::Dictionary(descriptor)) => descriptor_flags(pdf, descriptor)?,
                        _ => 32,
                    };
                }
            }
        }
        let space_char = get_space_char(&encoding, &character_map);
        let space_char_c = single_char(&space_char);
        let mut default_width = default.unwrap_or(0.0);
        if default_width == 0.0 {
            default_width = add_default_width(&mut widths, flags, space_char_c);
        }
        let space_width = {
            let w = space_char_c
                .and_then(|c| widths.get(&c).copied())
                .unwrap_or(0.0);
            if w != 0.0 {
                w
            } else if flags & 1 == 1 {
                default_width
            } else {
                (default_width / 2.0).floor()
            }
        };
        Ok(Font {
            encoding,
            character_map,
            widths,
            default_width,
            space_width,
            space_char,
        })
    }
}

fn single_char(s: &str) -> Option<char> {
    let mut chars = s.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => Some(c),
        _ => None,
    }
}

/// `int(x)` / `float(x)` of a PDF number.
fn number(object: &Object) -> Option<f64> {
    match object {
        Object::Integer(i) => Some(*i as f64),
        Object::Real(r) => Some(*r as f64),
        _ => None,
    }
}

fn descriptor_flags(pdf: &Pdf, descriptor: &Dictionary) -> Result<i64, FontError> {
    match pdf.get(descriptor, "Flags") {
        Some(Object::Integer(f)) => Ok(*f),
        Some(Object::Real(f)) => Ok(*f as i64),
        Some(_) => Err(FontError::Skip),
        None => Ok(32),
    }
}

/// `_collect_tt_t1_character_widths`: `chr(index + FirstChar)` to `int(width)`.
fn collect_simple_widths(
    pdf: &Pdf,
    font: &Dictionary,
    widths: &mut HashMap<char, f64>,
) -> Result<(), FontError> {
    let Some(Object::Array(array)) = pdf.get(font, "Widths") else {
        return Err(FontError::Skip);
    };
    let first_char = match pdf.get(font, "FirstChar") {
        None => 0,
        Some(Object::Integer(n)) => *n,
        Some(_) => return Err(FontError::Skip),
    };
    for (index, width) in array.iter().enumerate() {
        // an element that is a reference is `int(IndirectObject)`: a TypeError
        let value = match width {
            Object::Integer(n) => *n as f64,
            Object::Real(r) => r.trunc() as f64,
            _ => return Err(FontError::Skip),
        };
        let code = index as i64 + first_char;
        let Some(c) = u32::try_from(code).ok().and_then(char::from_u32) else {
            // chr() of a surrogate is valid in Python and is a key no text reaches; beyond 0x10FFFF it is a ValueError
            if (0xD800..=0xDFFF).contains(&code) {
                continue;
            }
            return Err(FontError::Fail(
                "chr() arg not in range(0x110000)".to_string(),
            ));
        };
        widths.insert(c, value);
    }
    Ok(())
}

/// `_collect_cid_character_widths`: the `/W` array of a descendant font.
fn collect_cid_widths(pdf: &Pdf, d_font: &Dictionary, widths: &mut HashMap<char, f64>) {
    let Some(Object::Array(w)) = pdf.get(d_font, "W") else {
        return;
    };
    let w: Vec<&Object> = w.iter().map(|o| pdf.deref(o)).collect();
    let mut skip = 0;
    for (index, entry) in w.iter().enumerate() {
        if skip > 0 {
            skip -= 1;
            continue;
        }
        let Some(start) = number(entry) else { continue };
        let next = w.get(index + 1).copied();
        match next {
            Some(Object::Array(list)) => {
                for (offset, item) in list.iter().enumerate() {
                    if let (Some(c), Some(width)) = (
                        char::from_u32((start as i64 + offset as i64).max(0) as u32),
                        number(pdf.deref(item)),
                    ) {
                        widths.insert(c, width);
                    }
                }
                skip = 1;
            }
            Some(next)
                if number(next).is_some()
                    && index + 2 < w.len()
                    && number(w[index + 2]).is_some() =>
            {
                let stop = number(next).unwrap_or(0.0);
                let width = number(w[index + 2]).unwrap_or(0.0);
                let mut cid = start as i64;
                while (cid as f64) < stop + 1.0 {
                    if let Some(c) = u32::try_from(cid).ok().and_then(char::from_u32) {
                        widths.insert(c, width);
                    }
                    cid += 1;
                    if cid > 0x11_0000 {
                        break;
                    }
                }
                skip = 2;
            }
            _ => {}
        }
    }
}

fn get_space_char(encoding: &Encoding, character_map: &CharMap) -> String {
    if let Encoding::Table(table) = encoding {
        for (code, text) in table.iter().enumerate() {
            if text == " " {
                return char::from_u32(code as u32).unwrap_or(' ').to_string();
            }
        }
    }
    if let Some(key) = character_map.first_key_with_value(" ") {
        return key.clone();
    }
    " ".to_string()
}

/// `Font._add_default_width`; returns the default width it chose.
fn add_default_width(widths: &mut HashMap<char, f64>, flags: i64, space_char: Option<char>) -> f64 {
    if widths.is_empty() {
        return 500.0;
    }
    if let Some(space) = space_char
        .and_then(|c| widths.get(&c).copied())
        .filter(|w| *w != 0.0)
    {
        if flags & 1 == 1 {
            return space;
        }
        return (2.0 * space).trunc();
    }
    let valid: Vec<f64> = widths.values().copied().filter(|w| *w > 0.0).collect();
    if valid.is_empty() {
        return 500.0;
    }
    // sum(valid_widths) // len(valid_widths): the order of a Python dict does not matter for integers; for floats it can in the
    // last bit, which a floor division does not see
    (valid.iter().sum::<f64>() / valid.len() as f64).floor()
}

// ------------------------------------------------------------------------------------------------------------- the encoding

const PREDEFINED: &[(&str, &str)] = &[
    ("Identity-H", "utf-16-be"),
    ("Identity-V", "utf-16-be"),
    ("GB-EUC-H", "gbk"),
    ("GB-EUC-V", "gbk"),
    ("GBpc-EUC-H", "gb2312"),
    ("GBpc-EUC-V", "gb2312"),
    ("GBK-EUC-H", "gbk"),
    ("GBK-EUC-V", "gbk"),
    ("GBK2K-H", "gb18030"),
    ("GBK2K-V", "gb18030"),
    ("ETen-B5-H", "cp950"),
    ("ETen-B5-V", "cp950"),
    ("ETenms-B5-H", "cp950"),
    ("ETenms-B5-V", "cp950"),
    ("UniCNS-UTF16-H", "utf-16-be"),
    ("UniCNS-UTF16-V", "utf-16-be"),
    ("UniGB-UTF16-H", "gb18030"),
    ("UniGB-UTF16-V", "gb18030"),
    ("90ms-RKSJ-H", "cp932"),
    ("90ms-RKSJ-V", "cp932"),
    ("UniJIS-UTF16-H", "utf-16-be"),
    ("UniJIS-UTF16-V", "utf-16-be"),
];

fn named_table(name: &str) -> Option<Vec<String>> {
    tables().encodings.get(&format!("/{name}")).cloned()
}

pub fn get_encoding(pdf: &Pdf, font: &Dictionary) -> (Encoding, CharMap) {
    let mut encoding = parse_encoding(pdf, font);
    let (map, int_entry) = parse_to_unicode(pdf, font);
    if let Encoding::Table(table) = &mut encoding {
        for x in int_entry {
            if x <= 255 {
                table[x as usize] = char::from_u32(x as u32).unwrap_or('\u{fffd}').to_string();
            }
        }
    }
    (encoding, map)
}

fn parse_encoding(pdf: &Pdf, font: &Dictionary) -> Encoding {
    let standard = || named_table("StandardEncoding").unwrap_or_default();
    let Some(enc) = pdf.get(font, "Encoding") else {
        let base_font = pdf.name_of(font, "BaseFont");
        if let Some(table) = base_font.as_deref().and_then(named_table) {
            return Encoding::Table(table);
        }
        return Encoding::Codec("charmap".into());
    };
    let mut table: Vec<String>;
    match enc {
        Object::Name(name) => {
            let name = String::from_utf8_lossy(name).into_owned();
            if let Some(t) = named_table(&name) {
                table = t;
            } else if let Some((_, codec)) = PREDEFINED.iter().find(|(n, _)| *n == name) {
                return Encoding::Codec((*codec).to_string());
            } else if name.contains("-UCS2-") {
                return Encoding::Codec("utf-16-be".into());
            } else {
                // "Advanced encoding ... not implemented yet": the name itself is the codec, which no decoder knows
                return Encoding::Codec(format!("/{name}"));
            }
        }
        Object::Dictionary(dict) => {
            table = match pdf.name_of(dict, "BaseEncoding") {
                Some(base) if pdf.has(dict, "BaseEncoding") => {
                    named_table(&base).unwrap_or_else(standard)
                }
                _ => standard(),
            };
            if let Some(Object::Array(differences)) = pdf.get(dict, "Differences") {
                let mut x: i64 = 0;
                for item in differences {
                    match pdf.deref(item) {
                        Object::Integer(n) => x = *n,
                        Object::Real(r) => x = *r as i64,
                        Object::Name(name) => {
                            if x >= 0 && (x as usize) < table.len() {
                                let key = format!("/{}", String::from_utf8_lossy(name));
                                table[x as usize] =
                                    tables().glyphs.get(&key).cloned().unwrap_or(key);
                            }
                            x += 1;
                        }
                        _ => {}
                    }
                }
            }
        }
        _ => table = standard(),
    }
    Encoding::Table(table)
}

// ------------------------------------------------------------------------------------------------------------- /ToUnicode

/// A `UnicodeDecodeError` (any `ValueError`) inside the CMap parser: the rest of the line is skipped.
struct Bad;

fn unhexlify(text: &[u8]) -> Result<Vec<u8>, Bad> {
    if !text.len().is_multiple_of(2) {
        return Err(Bad);
    }
    text.chunks(2)
        .map(|pair| {
            let hi = (pair[0] as char).to_digit(16).ok_or(Bad)?;
            let lo = (pair[1] as char).to_digit(16).ok_or(Bad)?;
            Ok((hi * 16 + lo) as u8)
        })
        .collect()
}

/// `bytes.decode("utf-16-be", "surrogatepass")`: an odd length is an error; a lone surrogate (kept by Python) becomes U+FFFD.
pub fn decode_utf16be(bytes: &[u8]) -> Result<String, Utf16Error> {
    if !bytes.len().is_multiple_of(2) {
        return Err(Utf16Error);
    }
    let units: Vec<u16> = bytes
        .chunks(2)
        .map(|p| u16::from_be_bytes([p[0], p[1]]))
        .collect();
    Ok(char::decode_utf16(units)
        .map(|r| r.unwrap_or('\u{fffd}'))
        .collect())
}

pub struct Utf16Error;

fn latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| b as char).collect()
}

fn decode_key(bytes: &[u8], code_len: usize) -> Result<String, Bad> {
    if code_len == 1 {
        Ok(latin1(bytes))
    } else {
        decode_utf16be(bytes).map_err(|_| Bad)
    }
}

/// Python `int(text, 16)` (digits only here).
fn parse_hex(text: &[u8]) -> Result<i64, Bad> {
    let s = std::str::from_utf8(text).map_err(|_| Bad)?;
    let s = s
        .strip_prefix("0x")
        .or_else(|| s.strip_prefix("0X"))
        .unwrap_or(s);
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(Bad);
    }
    i64::from_str_radix(s, 16).map_err(|_| Bad)
}

struct CMapState {
    /// `map_dict[-1]`: the number of bytes of a code.
    code_len: Option<usize>,
    map: CharMap,
    int_entry: Vec<i64>,
}

fn parse_to_unicode(pdf: &Pdf, font: &Dictionary) -> (CharMap, Vec<i64>) {
    let Some(tu) = pdf.get(font, "ToUnicode") else {
        if pdf.name_of(font, "Subtype").as_deref() == Some("Type1") {
            return type1_alternative(pdf, font);
        }
        return (CharMap::default(), Vec::new());
    };
    let cm = match tu {
        Object::Stream(stream) => stream
            .decompressed_content()
            .unwrap_or_else(|_| stream.content.clone()),
        _ => b"beginbfrange\n<0000> <0001> <0000>\nendbfrange".to_vec(),
    };
    let cm = prepare_cm(&cm);
    let mut state = CMapState {
        code_len: None,
        map: CharMap::default(),
        int_entry: Vec::new(),
    };
    let mut process_rg = false;
    let mut process_char = false;
    let mut multiline: Option<(i64, i64)> = None;
    for line in cm.split(|&b| b == b'\n') {
        let line = strip(line, b" \t");
        if line.is_empty() || line[0] == b'%' {
            continue;
        }
        let line: Vec<u8> = line
            .iter()
            .map(|&b| if b == b'\t' { b' ' } else { b })
            .collect();
        if contains(&line, b"beginbfrange") {
            process_rg = true;
        } else if contains(&line, b"endbfrange") {
            process_rg = false;
        } else if contains(&line, b"beginbfchar") {
            process_char = true;
        } else if contains(&line, b"endbfchar") {
            process_char = false;
        } else if process_rg {
            if let Ok(next) = parse_bfrange(&line, &mut state, multiline) {
                multiline = next;
            }
        } else if process_char {
            let _ = parse_bfchar(&line, &mut state);
        }
    }
    (state.map, state.int_entry)
}

fn strip<'a>(line: &'a [u8], chars: &[u8]) -> &'a [u8] {
    let mut start = 0;
    let mut end = line.len();
    while start < end && chars.contains(&line[start]) {
        start += 1;
    }
    while end > start && chars.contains(&line[end - 1]) {
        end -= 1;
    }
    &line[start..end]
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

fn replace(data: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut i = 0;
    while i < data.len() {
        if data[i..].starts_with(from) {
            out.extend_from_slice(to);
            i += from.len();
        } else {
            out.push(data[i]);
            i += 1;
        }
    }
    out
}

/// Python `bytes.strip()`: ASCII whitespace.
fn strip_ascii_whitespace(data: &[u8]) -> &[u8] {
    strip(data, b" \t\n\r\x0b\x0c")
}

fn prepare_cm(cm: &[u8]) -> Vec<u8> {
    let mut cm = strip_ascii_whitespace(cm).to_vec();
    for (from, to) in [
        (&b"beginbfchar"[..], &b"\nbeginbfchar\n"[..]),
        (b"endbfchar", b"\nendbfchar\n"),
        (b"beginbfrange", b"\nbeginbfrange\n"),
        (b"endbfrange", b"\nendbfrange\n"),
        (b"<<", b"\n{\n"),
        (b">>", b"\n}\n"),
    ] {
        cm = replace(&cm, from, to);
    }
    let mut pieces: Vec<Vec<u8>> = cm.split(|&b| b == b'<').map(<[u8]>::to_vec).collect();
    for piece in &mut pieces {
        if let Some(j) = piece.iter().position(|&b| b == b'>') {
            let mut content: Vec<u8> = if j == 0 {
                b".".to_vec()
            } else {
                piece[..j].iter().copied().filter(|&b| b != b' ').collect()
            };
            content.push(b' ');
            content.extend_from_slice(&piece[j + 1..]);
            *piece = content;
        }
    }
    let joined = pieces.join(&b' ');
    let joined = replace(&joined, b"[", b" [ ");
    let joined = replace(&joined, b"]", b" ]\n ");
    replace(&joined, b"\r", b"\n")
}

fn split_words(line: &[u8]) -> Vec<&[u8]> {
    line.split(|&b| b == b' ')
        .filter(|w| !w.is_empty())
        .collect()
}

fn hex_width(value: i64, width: usize) -> Vec<u8> {
    format!("{value:0width$X}").into_bytes()
}

fn parse_bfrange(
    line: &[u8],
    state: &mut CMapState,
    multiline: Option<(i64, i64)>,
) -> Result<Option<(i64, i64)>, Bad> {
    let lst = split_words(line);
    let mut closure_found = false;
    let (mut a, b);
    if let Some((ma, mb)) = multiline {
        let code_len = state.code_len.ok_or(Bad)?;
        a = ma;
        b = mb;
        for sq in lst {
            if sq == b"]" {
                closure_found = true;
                break;
            }
            let key = decode_key(&unhexlify(&hex_width(a, code_len * 2))?, code_len)?;
            let value = decode_utf16be(&unhexlify(sq)?).map_err(|_| Bad)?;
            state.map.insert(key, value);
            state.int_entry.push(a);
            a += 1;
        }
    } else {
        if lst.len() < 2 {
            return Err(Bad);
        }
        a = parse_hex(lst[0])?;
        b = parse_hex(lst[1])?;
        let nbi = lst[0].len().max(lst[1].len());
        let code_len = nbi.div_ceil(2);
        state.code_len = Some(code_len);
        if lst.len() < 3 {
            return Err(Bad);
        }
        if lst[2] == b"[" {
            for sq in &lst[3..] {
                if *sq == b"]" {
                    closure_found = true;
                    break;
                }
                let key = decode_key(&unhexlify(&hex_width(a, code_len * 2))?, code_len)?;
                let value = decode_utf16be(&unhexlify(sq)?).map_err(|_| Bad)?;
                state.map.insert(key, value);
                state.int_entry.push(a);
                a += 1;
            }
        } else {
            let mut c = parse_hex(lst[2])?;
            let width2 = lst[2].len().max(4);
            closure_found = true;
            if b - a + 1 > 100_000 || state.int_entry.len() as i64 + (b - a + 1).max(0) > 100_000 {
                // LimitReachedError: the page cannot be read; treated as a broken table (the lines so far stay)
                return Err(Bad);
            }
            while a <= b {
                let key = decode_key(&unhexlify(&hex_width(a, code_len * 2))?, code_len)?;
                let value = decode_utf16be(&unhexlify(&hex_width(c, width2))?).map_err(|_| Bad)?;
                state.map.insert(key, value);
                state.int_entry.push(a);
                a += 1;
                c += 1;
            }
        }
    }
    Ok(if closure_found { None } else { Some((a, b)) })
}

fn parse_bfchar(line: &[u8], state: &mut CMapState) -> Result<(), Bad> {
    let mut lst = split_words(line);
    if lst.is_empty() {
        return Err(Bad);
    }
    state.code_len = Some(lst[0].len() / 2);
    while lst.len() > 1 {
        let mut map_to = String::new();
        if lst[1] != b"." {
            if let Ok(raw) = unhexlify(lst[1]) {
                map_to = if lst[1].len() < 4 {
                    latin1(&raw)
                } else {
                    decode_utf16be(&raw).map_err(|_| Bad)?
                };
            }
        }
        let code_len = state.code_len.unwrap_or(0);
        let key = decode_key(&unhexlify(lst[0])?, code_len)?;
        state.map.insert(key, map_to);
        state.int_entry.push(parse_hex(lst[0])?);
        lst = lst[2..].to_vec();
    }
    Ok(())
}

fn type1_alternative(pdf: &Pdf, font: &Dictionary) -> (CharMap, Vec<i64>) {
    let mut map = CharMap::default();
    let mut ints = Vec::new();
    let Some(Object::Dictionary(descriptor)) = pdf.get(font, "FontDescriptor") else {
        return (map, ints);
    };
    let Some(Object::Stream(file)) = pdf.get(descriptor, "FontFile") else {
        return (map, ints);
    };
    let data = file
        .decompressed_content()
        .unwrap_or_else(|_| file.content.clone());
    // txt.split(b"eexec\n")[0]
    let clear = match find(&data, b"eexec\n") {
        Some(i) => &data[..i],
        None => &data[..],
    };
    // txt.split(b"/Encoding")[1]
    let Some(first) = find(clear, b"/Encoding") else {
        return (map, ints);
    };
    let after = &clear[first + b"/Encoding".len()..];
    let part = match find(after, b"/Encoding") {
        Some(i) => &after[..i],
        None => after,
    };
    let normalised = replace(part, b"\r", b"\n");
    for li in normalised.split(|&b| b == b'\n') {
        if !li.starts_with(b"dup") {
            continue;
        }
        let words = split_words(li);
        if words.len() < 3 || (words.len() > 3 && words[3] != b"put") {
            continue;
        }
        let Ok(i) = std::str::from_utf8(words[1]).unwrap_or("").parse::<i64>() else {
            continue;
        };
        let Ok(name) = std::str::from_utf8(words[2]) else {
            continue;
        };
        let value = match tables().glyphs.get(name) {
            Some(v) => v.clone(),
            None if name.starts_with("/uni") => match parse_hex(&words[2][4..]) {
                Ok(v) => match u32::try_from(v).ok().and_then(char::from_u32) {
                    Some(c) => c.to_string(),
                    None => continue,
                },
                Err(Bad) => continue,
            },
            None => continue,
        };
        let Some(key) = u32::try_from(i).ok().and_then(char::from_u32) else {
            continue;
        };
        map.insert(key.to_string(), value);
        ints.push(i);
    }
    (map, ints)
}

fn find(data: &[u8], needle: &[u8]) -> Option<usize> {
    data.windows(needle.len()).position(|w| w == needle)
}
