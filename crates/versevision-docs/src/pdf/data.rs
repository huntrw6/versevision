//! The tables pypdf carries (`pypdf/_codecs`): Adobe glyph names, the six built-in encodings, the widths of the 14 core fonts.
//! `pdf_data.txt` is written by `python/scripts/gen_pdf_data.py` from the locked pypdf; nothing in it is edited by hand.
use std::collections::HashMap;
use std::sync::OnceLock;

const DATA: &str = include_str!("../pdf_data.txt");

/// Widths of one of the core fonts (`CORE_FONT_METRICS`).
pub struct CoreFont {
    pub flags: i64,
    pub widths: HashMap<char, i64>,
}

pub struct Tables {
    /// `adobe_glyphs`: `"/A"` to its text.
    pub glyphs: HashMap<String, String>,
    /// `charset_encoding`: `"/WinAnsiEncoding"` to the 256 entries.
    pub encodings: HashMap<String, Vec<String>>,
    pub core_fonts: HashMap<String, CoreFont>,
}

fn text_of(codes: &str) -> String {
    codes
        .split(',')
        .filter(|c| !c.is_empty())
        .map(|c| {
            u32::from_str_radix(c, 16)
                .ok()
                .and_then(char::from_u32)
                .unwrap_or('\u{fffd}')
        })
        .collect()
}

pub fn tables() -> &'static Tables {
    static TABLES: OnceLock<Tables> = OnceLock::new();
    TABLES.get_or_init(|| {
        let mut glyphs = HashMap::new();
        let mut encodings = HashMap::new();
        let mut core_fonts = HashMap::new();
        for line in DATA.lines() {
            let mut parts = line.split(' ');
            match parts.next() {
                Some("G") => {
                    if let (Some(name), Some(codes)) = (parts.next(), parts.next()) {
                        glyphs.insert(name.to_string(), text_of(codes));
                    }
                }
                Some("E") => {
                    if let Some(name) = parts.next() {
                        encodings.insert(name.to_string(), parts.map(text_of).collect::<Vec<_>>());
                    }
                }
                Some("F") => {
                    let name = parts.next().unwrap_or_default().to_string();
                    let flags = parts.next().and_then(|f| f.parse().ok()).unwrap_or(32);
                    let _default = parts.next();
                    let widths = parts
                        .filter_map(|entry| {
                            let (code, width) = entry.split_once('=')?;
                            Some((
                                char::from_u32(u32::from_str_radix(code, 16).ok()?)?,
                                width.parse().ok()?,
                            ))
                        })
                        .collect();
                    core_fonts.insert(name, CoreFont { flags, widths });
                }
                _ => {}
            }
        }
        Tables {
            glyphs,
            encodings,
            core_fonts,
        }
    })
}
