//! pypdf's `TextExtraction` (`_text_extraction/_text_extractor.py`) and `crlf_space_check` / `get_text_operands` /
//! `get_display_str`: the text of one page (or of a form XObject) from its content stream, in pypdf's order and with pypdf's spaces
//! and line breaks.
use std::collections::HashMap;

use lopdf::{Dictionary, Object};

use super::content::{self, Operand, Operation};
use super::font::{decode_utf16be, Encoding, Font, FontError};
use super::Pdf;

type Matrix = [f64; 6];
const IDENTITY: Matrix = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

/// `mult(m, n)` of `_text_extraction/__init__.py`.
fn mult(m: &Matrix, n: &Matrix) -> Matrix {
    [
        m[0] * n[0] + m[1] * n[2],
        m[0] * n[1] + m[1] * n[3],
        m[2] * n[0] + m[3] * n[2],
        m[2] * n[1] + m[3] * n[3],
        m[4] * n[0] + m[5] * n[2] + n[4],
        m[4] * n[1] + m[5] * n[3] + n[5],
    ]
}

fn orient(m: &Matrix) -> i32 {
    if m[3] > 1e-6 {
        0
    } else if m[3] < -1e-6 {
        180
    } else if m[1] > 0.0 {
        90
    } else {
        270
    }
}

/// CPython's `sum()` of floats since 3.12: Neumaier compensated summation.
fn py_sum(values: impl IntoIterator<Item = f64>) -> f64 {
    let mut result = 0.0f64;
    let mut c = 0.0f64;
    for x in values {
        let t = result + x;
        if !t.is_finite() {
            // inf/nan: plain addition, no compensation (CPython falls back to it)
            result = t;
            c = 0.0;
            continue;
        }
        if result.abs() >= x.abs() {
            c += (result - t) + x;
        } else {
            c += (x - t) + result;
        }
        result = t;
    }
    if c != 0.0 && c.is_finite() {
        result += c;
    }
    result
}

struct Saved {
    cm_matrix: Matrix,
    font: usize,
    font_size: f64,
    tl: f64,
}

/// The fonts of one resource dictionary: `font_resources` / `fonts`, by resource name.
struct Fonts {
    by_name: HashMap<String, Font>,
}

pub struct PageError(pub String);

struct Extraction {
    cm_matrix: Matrix,
    tm_matrix: Matrix,
    cm_stack: Vec<Saved>,
    cm_prev: Matrix,
    tm_prev: Matrix,
    space_width_half: f64,
    str_widths: f64,
    str_height: f64,
    tl: f64,
    font_size: f64,
    text: String,
    output: String,
    rtl_dir: bool,
    font: usize,
    /// font 0 is "NotInitialized", 1 is the unknown-font fallback, the rest are the resource fonts
    fonts: Vec<Font>,
    names: HashMap<String, usize>,
}

/// `PageObject._extract_text(obj, ...)`. `content_of` gives the bytes of the page's content stream (or the XObject's data).
pub fn extract(
    pdf: &Pdf,
    resources: &Dictionary,
    content: &[u8],
    depth: usize,
) -> Result<String, PageError> {
    let fonts = collect_fonts(pdf, resources)?;
    let operations = content::parse(content).map_err(|e| PageError(e.0))?;
    let mut ex = Extraction {
        cm_matrix: IDENTITY,
        tm_matrix: IDENTITY,
        cm_stack: Vec::new(),
        cm_prev: IDENTITY,
        tm_prev: IDENTITY,
        space_width_half: 500.0,
        str_widths: 0.0,
        str_height: 0.0,
        tl: 0.0,
        font_size: 12.0,
        text: String::new(),
        output: String::new(),
        rtl_dir: false,
        font: 0,
        fonts: Vec::new(),
        names: HashMap::new(),
    };
    ex.fonts.push(Font::not_initialized());
    ex.fonts.push(Font::unknown());
    for (name, font) in fonts.by_name {
        ex.names.insert(name, ex.fonts.len());
        ex.fonts.push(font);
    }
    for Operation { operands, operator } in &operations {
        ex.operation(pdf, resources, operands, operator, depth)?;
    }
    ex.output.push_str(&ex.text);
    Ok(ex.output)
}

fn collect_fonts(pdf: &Pdf, resources: &Dictionary) -> Result<Fonts, PageError> {
    let mut by_name = HashMap::new();
    if let Some(Object::Dictionary(font_resources)) = pdf.get(resources, "Font") {
        for (name, value) in font_resources.iter() {
            let Object::Dictionary(dict) = pdf.deref(value) else {
                continue;
            };
            match Font::from_resource(pdf, dict) {
                Ok(mut font) => {
                    // "Override space width, if applicable"
                    let has_space =
                        font.space_char.chars().count() == 1
                            && font.space_char.chars().next().is_some_and(|c| {
                                font.widths.get(&c).copied().unwrap_or(0.0) != 0.0
                            });
                    if !has_space {
                        font.space_width = 200.0;
                    }
                    by_name.insert(format!("/{}", String::from_utf8_lossy(name)), font);
                }
                Err(FontError::Skip) => {}
                Err(FontError::Fail(message)) => return Err(PageError(message)),
            }
        }
    }
    Ok(Fonts { by_name })
}

impl Extraction {
    fn font(&self) -> &Font {
        &self.fonts[self.font]
    }

    fn operation(
        &mut self,
        pdf: &Pdf,
        resources: &Dictionary,
        operands: &[Operand],
        operator: &[u8],
        depth: usize,
    ) -> Result<(), PageError> {
        match operator {
            b"'" => {
                self.process(b"T*", &[])?;
                self.process(b"Tj", operands)?;
            }
            b"\"" if operands.len() >= 3 => {
                self.process(b"Tw", &operands[..1])?;
                self.process(b"Tc", &operands[1..2])?;
                self.process(b"T*", &[])?;
                self.process(b"Tj", &operands[2..])?;
            }
            b"TJ" => {
                // "The space width may be smaller than the font width, so the width should be 95%."
                let confirm_space_width = self.space_width_half * 0.95;
                if let Some(Operand::Array(items)) = operands.first() {
                    for op in items {
                        match op {
                            Operand::Bytes(_) | Operand::Name(_) | Operand::Text(_) => {
                                self.process(b"Tj", std::slice::from_ref(op))?;
                            }
                            Operand::Number(n)
                                if n.abs() >= confirm_space_width
                                    && !self.text.is_empty()
                                    && !self.text.ends_with(' ') =>
                            {
                                self.process(b"Tj", &[Operand::Text(" ".into())])?;
                            }
                            _ => {}
                        }
                    }
                }
            }
            b"TD" if operands.len() >= 2 => {
                let leading = operands[1]
                    .as_float()
                    .ok_or_else(|| PageError("bad TD operand".into()))?;
                self.process(b"TL", &[Operand::Number(-leading)])?;
                self.process(b"Td", operands)?;
            }
            b"Do" => {
                self.output.push_str(&self.text);
                if let Some(last) = self.output.chars().last() {
                    if last != '\n' {
                        self.output.push('\n');
                    }
                }
                // a failure of the XObject (missing, not a form, a read error) is only a logged warning in pypdf
                if let Some(Operand::Name(name)) = operands.first() {
                    if let Ok(Some(text)) = self.xobject(pdf, resources, name, depth) {
                        self.output.push_str(&text);
                    }
                }
                self.text.clear();
            }
            _ => self.process(operator, operands)?,
        }
        Ok(())
    }

    fn xobject(
        &self,
        pdf: &Pdf,
        resources: &Dictionary,
        name: &str,
        depth: usize,
    ) -> Result<Option<String>, ()> {
        if depth > 8 {
            return Ok(Some(String::new()));
        }
        let Some(Object::Dictionary(xobjects)) = pdf.get(resources, "XObject") else {
            return Err(());
        };
        let Some(Object::Stream(form)) = pdf.get(xobjects, name) else {
            return Err(());
        };
        match pdf.name_of(&form.dict, "Subtype").as_deref() {
            Some("Image") => return Ok(None),
            None => return Err(()),
            _ => {}
        }
        // `get_inherited("/Resources")` of a form: its own dictionary has no /Parent
        let Some(Object::Dictionary(inner)) = pdf.get(&form.dict, "Resources") else {
            return Ok(Some(String::new()));
        };
        if inner.is_empty() {
            return Ok(Some(String::new()));
        }
        let Ok(data) = pdf.stream_data(form) else {
            return Err(());
        };
        match extract(pdf, inner, &data, depth + 1) {
            Ok(text) => Ok(Some(text)),
            Err(_) => Err(()),
        }
    }

    fn process(&mut self, operator: &[u8], operands: &[Operand]) -> Result<(), PageError> {
        let str_widths = match operator {
            b"BT" => {
                self.tm_matrix = IDENTITY;
                self.flush();
                return Ok(());
            }
            b"ET" => {
                self.flush();
                return Ok(());
            }
            b"q" => {
                self.cm_stack.push(Saved {
                    cm_matrix: self.cm_matrix,
                    font: self.font,
                    font_size: self.font_size,
                    tl: self.tl,
                });
                return Ok(());
            }
            b"Q" => {
                match self.cm_stack.pop() {
                    Some(saved) => {
                        self.cm_matrix = saved.cm_matrix;
                        self.font = saved.font;
                        self.font_size = saved.font_size;
                        self.tl = saved.tl;
                    }
                    None => self.cm_matrix = IDENTITY,
                }
                return Ok(());
            }
            b"cm" => {
                self.output.push_str(&self.text);
                self.text.clear();
                let parsed: Option<Vec<f64>> =
                    operands.iter().take(6).map(Operand::as_float).collect();
                self.cm_matrix = match parsed {
                    Some(v) if v.len() == 6 => {
                        mult(&[v[0], v[1], v[2], v[3], v[4], v[5]], &self.cm_matrix)
                    }
                    _ => IDENTITY,
                };
                return Ok(());
            }
            b"Tz" | b"Tw" => {
                // `float(operands[0])` raises on a non-number; the scales themselves are never used
                if let Some(op) = operands.first() {
                    if op.as_float().is_none() {
                        return Err(PageError(format!("could not convert {op:?} to float")));
                    }
                }
                return Ok(());
            }
            b"TL" => {
                let scale_x = (self.tm_matrix[0] * self.tm_matrix[0]
                    + self.tm_matrix[2] * self.tm_matrix[2])
                    .sqrt();
                let value = match operands.first() {
                    Some(op) => op
                        .as_float()
                        .ok_or_else(|| PageError(format!("could not convert {op:?} to float")))?,
                    None => 0.0,
                };
                self.tl = value * self.font_size * scale_x;
                return Ok(());
            }
            b"Tf" => {
                if !self.text.is_empty() {
                    self.output.push_str(&self.text);
                }
                self.text.clear();
                self.font = match operands.first() {
                    Some(Operand::Name(name)) => {
                        self.names.get(&format!("/{name}")).copied().unwrap_or(1)
                    }
                    _ => 1,
                };
                self.space_width_half = self.fonts[self.font].space_width / 2.0;
                if let Some(size) = operands.get(1).and_then(Operand::as_float) {
                    self.font_size = size;
                }
                return Ok(());
            }
            b"Td" => {
                let tx = operands
                    .first()
                    .map(|o| o.as_float().ok_or_else(|| PageError("bad Td".into())))
                    .transpose()?
                    .unwrap_or(0.0);
                let ty = operands
                    .get(1)
                    .map(|o| o.as_float().ok_or_else(|| PageError("bad Td".into())))
                    .transpose()?
                    .unwrap_or(0.0);
                self.tm_matrix[4] += tx * self.tm_matrix[0] + ty * self.tm_matrix[2];
                self.tm_matrix[5] += tx * self.tm_matrix[1] + ty * self.tm_matrix[3];
                self.take_str_widths()
            }
            b"Tm" => {
                let parsed: Option<Vec<f64>> =
                    operands.iter().take(6).map(Operand::as_float).collect();
                self.tm_matrix = match parsed {
                    Some(v) if v.len() == 6 => [v[0], v[1], v[2], v[3], v[4], v[5]],
                    _ => IDENTITY,
                };
                self.take_str_widths()
            }
            b"T*" => {
                self.tm_matrix[4] -= self.tl * self.tm_matrix[2];
                self.tm_matrix[5] -= self.tl * self.tm_matrix[3];
                self.take_str_widths()
            }
            b"Tj" => {
                self.show(operands)?;
                0.0
            }
            _ => return Ok(()),
        };
        self.post_process(str_widths);
        Ok(())
    }

    fn take_str_widths(&mut self) -> f64 {
        let widths = self.str_widths / 1000.0;
        self.str_widths = 0.0;
        widths
    }

    fn flush(&mut self) {
        self.output.push_str(&self.text);
        self.text.clear();
    }

    /// `_post_process_text_operation` -> `crlf_space_check`
    fn post_process(&mut self, str_widths: f64) {
        let spacewidth = self.font_size * self.space_width_half / 1000.0;
        let m_prev = mult(&self.tm_prev, &self.cm_prev);
        let m = mult(&self.tm_matrix, &self.cm_matrix);
        let orientation = orient(&m);
        let delta_x = m[4] - m_prev[4];
        let delta_y = m[5] - m_prev[5];
        let tm_prev = self.tm_prev;
        let tm = self.tm_matrix;
        let scale_prev_x = (tm_prev[0] * tm_prev[0] + tm_prev[1] * tm_prev[1]).sqrt();
        let scale_prev_y = (tm_prev[2] * tm_prev[2] + tm_prev[3] * tm_prev[3]).sqrt();
        let scale_y = (tm[2] * tm[2] + tm[3] * tm[3]).sqrt();
        let (moved_height, moved_width) = if orientation == 0 || orientation == 180 {
            (delta_y, delta_x)
        } else {
            (delta_x, delta_y)
        };
        let last = self
            .text
            .chars()
            .last()
            .or_else(|| self.output.chars().last());
        if moved_height.abs()
            > 0.8 * f64::min(self.str_height * scale_prev_y, self.font_size * scale_y)
        {
            if let Some(last) = last {
                if last != '\n' {
                    self.output.push_str(&self.text);
                    self.output.push('\n');
                    self.text.clear();
                }
            }
        } else if moved_width >= (spacewidth + str_widths) * scale_prev_x {
            if let Some(last) = last {
                if last != ' ' {
                    self.text.push(' ');
                }
            }
        }
        self.tm_prev = self.tm_matrix;
        self.cm_prev = self.cm_matrix;
    }

    /// `_handle_tj`
    fn show(&mut self, operands: &[Operand]) -> Result<(), PageError> {
        let mut shown = String::new();
        let mut is_str_operands = false;
        if !operands.is_empty() {
            match &operands[0] {
                Operand::Text(text) => {
                    shown = text.clone();
                    is_str_operands = true;
                }
                Operand::Name(name) => {
                    shown = format!("/{name}");
                    is_str_operands = true;
                }
                Operand::Bytes(bytes) => shown = self.decode(bytes),
                _ => return Err(PageError("a text operator without a string".into())),
            }
        }
        let font_widths = if is_str_operands {
            self.text.push_str(&shown);
            let font = self.font();
            py_sum(shown.chars().map(|c| {
                if font.is_space_char(c) {
                    font.space_width
                } else {
                    font.text_width(c)
                }
            }))
        } else {
            self.display(&shown)
        };
        self.str_widths += font_widths * self.font_size;
        self.str_height = self.font_size;
        Ok(())
    }

    /// `get_text_operands`: the bytes of a string to text through the font's encoding.
    fn decode(&self, bytes: &[u8]) -> String {
        match &self.font().encoding {
            Encoding::Codec(codec) => match codec.as_str() {
                "charmap" => bytes.iter().map(|&b| b as char).collect(),
                "utf-16-be" => decode_utf16be(bytes)
                    .unwrap_or_else(|_| bytes.iter().map(|&b| b as char).collect()),
                // the East Asian codecs of `_predefined_cmap`: a byte sequence the codec rejects is Python's UnicodeDecodeError, and
                // the fallback is "charmap"
                "gbk" | "gb2312" | "gb18030" | "cp950" | "cp932" => {
                    let codec = match codec.as_str() {
                        "gb18030" => encoding_rs::GB18030,
                        "cp950" => encoding_rs::BIG5,
                        "cp932" => encoding_rs::SHIFT_JIS,
                        _ => encoding_rs::GBK,
                    };
                    match codec.decode_without_bom_handling_and_without_replacement(bytes) {
                        Some(text) => text.into_owned(),
                        None => bytes.iter().map(|&b| b as char).collect(),
                    }
                }
                // an unknown codec name: Python raises LookupError and falls back to "charmap"
                _ => bytes.iter().map(|&b| b as char).collect(),
            },
            Encoding::Table(table) => bytes.iter().map(|&b| table[b as usize].as_str()).collect(),
        }
    }

    /// `get_display_str`: appends the characters, mapped through the font's character map, in reading order.
    fn display(&mut self, operand_text: &str) -> f64 {
        let mut widths_list = Vec::new();
        let mut text = std::mem::take(&mut self.text);
        let mut rtl_dir = self.rtl_dir;
        {
            let font = &self.fonts[self.font];
            for raw in operand_text.chars() {
                widths_list.push(if font.is_space_char(raw) {
                    font.space_width
                } else {
                    font.text_width(raw)
                });
                let mut buffer = [0u8; 4];
                let raw_str: &str = raw.encode_utf8(&mut buffer);
                let x: &str = font
                    .character_map
                    .get(raw_str)
                    .map(String::as_str)
                    .unwrap_or(raw_str);
                let mut chars = x.chars();
                let xx: u32 = match (chars.next(), chars.next()) {
                    (Some(c), None) => c as u32,
                    _ => 1,
                };
                if xx <= 0x2F
                    || (0x3A..=0x40).contains(&xx)
                    || (0x2000..=0x206F).contains(&xx)
                    || (0x20A0..=0x21FF).contains(&xx)
                {
                    if rtl_dir {
                        text.insert_str(0, x);
                    } else {
                        text.push_str(x);
                    }
                } else if (0x0590..=0x08FF).contains(&xx)
                    || (0xFB1D..=0xFDFF).contains(&xx)
                    || (0xFE70..=0xFEFF).contains(&xx)
                {
                    if !rtl_dir {
                        rtl_dir = true;
                        text.clear();
                    }
                    text.insert_str(0, x);
                } else {
                    if rtl_dir {
                        rtl_dir = false;
                        text.clear();
                    }
                    text.push_str(x);
                }
            }
        }
        self.text = text;
        self.rtl_dir = rtl_dir;
        py_sum(widths_list)
    }
}
