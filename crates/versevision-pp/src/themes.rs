//! `themes.py`: ProPresenter themes as the look of the imported slides.
//!
//! ProPresenter keeps every theme it has loaded in `Documents/ProPresenter/Themes` (one `Theme` file per theme, each holding one or more named
//! theme slides). A theme slide is a normal slide: text boxes with their position, font, colour and fill, and a background. VerseVision reads
//! those files and, when a theme is chosen for verses or points, builds each slide from that theme slide instead of from the plain template.
//!
//! Which text box is which: a point uses the theme's largest text box. A verse uses the largest box for the verse text and, when the theme
//! slide has a second text box, the smallest of the others for the reference; a one-box theme gets the reference as the last line of the box.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::bytes::Regex as BytesRegex;
use serde::{Deserialize, Serialize};
use versevision_core::looks::{TextBox, ThemeSlide};

use crate::error::{PpError, Result};
use crate::model::{casefold, element, has_text, rect};
use crate::pb::Message;

/// `Cue.name` of every slide built from a theme, so Replace can tell them apart from the operator's own slides.
pub const MARKER: &str = "VerseVision";
/// A text box of the theme that is neither verse, reference nor point (its sample text is cleared).
pub const EXTRA: &str = "Theme Extra";
/// A second text box counts as a separate reference line only if it does not sit on top of the main one.
pub const REFERENCE_OVERLAP_LIMIT: f64 = 0.25;

pub const THEME_DOCUMENT: &str = "rv.data.Template.Document";

/// A theme slide as read from the file: what is shown to the user, and the slide itself (`base`).
#[derive(Debug, Clone, PartialEq)]
pub struct LoadedSlide {
    pub slide: ThemeSlide,
    pub base: Message,
}

/// The theme slides chosen for verses and for points, ready to build slides from.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LoadedLooks {
    pub verse: Option<LoadedSlide>,
    pub point: Option<LoadedSlide>,
}

impl LoadedLooks {
    /// The `Looks` the plan and the review carry (without the slide data).
    pub fn describe(&self) -> versevision_core::looks::LooksDescription {
        versevision_core::looks::Looks {
            verse: self.verse.as_ref().map(|s| s.slide.clone()),
            point: self.point.as_ref().map(|s| s.slide.clone()),
        }
        .describe()
    }
}

/// `_overlap`: how much of the SMALLER box lies inside the other (0 to 1).
pub fn overlap(a: (f64, f64, f64, f64), b: (f64, f64, f64, f64)) -> f64 {
    let (a_x, a_y, a_w, a_h) = a;
    let (b_x, b_y, b_w, b_h) = b;
    let width = (a_x + a_w).min(b_x + b_w) - a_x.max(b_x);
    let height = (a_y + a_h).min(b_y + b_h) - a_y.max(b_y);
    if width <= 0.0 || height <= 0.0 {
        return 0.0;
    }
    width * height / (a_w * a_h).min(b_w * b_h).max(1.0)
}

fn box_area(b: &TextBox) -> f64 {
    b.w * b.h
}

fn box_rect(b: &TextBox) -> (f64, f64, f64, f64) {
    (b.x, b.y, b.w, b.h)
}

/// `ThemeSlide.has_reference_box`: the slide has a second text box that is clear of the main one.
pub fn has_reference_box(slide: &ThemeSlide) -> bool {
    if slide.boxes.len() < 2 {
        return false;
    }
    let main = slide
        .boxes
        .iter()
        .enumerate()
        .fold(None::<(usize, &TextBox)>, |best, (i, b)| match best {
            Some((_, m)) if box_area(b) <= box_area(m) => best,
            _ => Some((i, b)),
        });
    let (main_index, main) = main.expect("two boxes");
    slide.boxes.iter().enumerate().any(|(i, b)| {
        i != main_index && overlap(box_rect(main), box_rect(b)) <= REFERENCE_OVERLAP_LIMIT
    })
}

/// `ThemeSlide.usable`: it has a text box.
pub fn usable(slide: &ThemeSlide) -> bool {
    !slide.boxes.is_empty()
}

static FONT_SIZE: LazyLock<BytesRegex> =
    LazyLock::new(|| BytesRegex::new(r"(?-u)\\fs([0-9]+)").expect("size pattern"));
static FONT_TABLE: LazyLock<BytesRegex> = LazyLock::new(|| {
    BytesRegex::new(
        r"(?-u)\{\\fonttbl[ \t\n\r\x0b\x0c]*\{\\f[0-9]+[^;]*?[ \t\n\r\x0b\x0c]([^;{}\\]+);",
    )
    .expect("font table pattern")
});

/// `_font_of(element)`: the font name and size of a text box (the attributes win over the RTF).
fn font_of(element: &Message) -> (String, f64) {
    let text = element.msg("text");
    let rtf = text.map(|t| t.bytes("rtf_data")).unwrap_or(&[]);
    let mut size = 0.0;
    let mut name = String::new();
    if let Some(m) = FONT_SIZE.captures(rtf) {
        size = std::str::from_utf8(&m[1])
            .ok()
            .and_then(|d| d.parse::<f64>().ok())
            .unwrap_or(0.0)
            / 2.0;
    }
    if let Some(m) = FONT_TABLE.captures(rtf) {
        let decoded: String = m[1].iter().map(|b| *b as char).collect(); // latin-1
        name = versevision_core::pystr::strip(&decoded).to_string();
    }
    if let Some(font) = text
        .and_then(|t| t.msg("attributes"))
        .and_then(|a| a.msg("font"))
    {
        let attr_name = font.str("name");
        if !attr_name.is_empty() {
            name = attr_name.to_string();
        }
        let attr_size = font.float("size");
        if attr_size != 0.0 {
            size = attr_size;
        }
    }
    (name, size)
}

/// `read_theme_file(path, group, theme)`.
pub fn read_theme_file(path: &Path, group: &str, theme: &str) -> Result<Vec<LoadedSlide>> {
    let bytes = fs::read(path).map_err(PpError::io)?;
    let document = Message::decode_named(THEME_DOCUMENT, &bytes)?;
    Ok(slides_of_document(&document, path, group, theme))
}

/// The theme slides of a decoded `Template.Document`.
pub fn slides_of_document(
    document: &Message,
    path: &Path,
    group: &str,
    theme: &str,
) -> Vec<LoadedSlide> {
    let mut slides = Vec::new();
    for (index, item) in document.msgs("slides").into_iter().enumerate() {
        let base = item
            .msg("base_slide")
            .cloned()
            .unwrap_or_else(|| Message::new_named("rv.data.Slide").expect("Slide in the schema"));
        let mut boxes = Vec::new();
        let mut others = 0;
        for wrapped in base.msgs("elements") {
            let el = element(wrapped);
            match el {
                Some(el) if has_text(wrapped) => {
                    let (font, size) = font_of(el);
                    let (x, y, w, h) = rect(wrapped);
                    boxes.push(TextBox {
                        name: el.str("name").to_string(),
                        x,
                        y,
                        w,
                        h,
                        font,
                        size,
                    });
                }
                _ => others += 1,
            }
        }
        let size = base.msg("size");
        let dimension = |name: &str, fallback: u32| {
            let value = size.map(|s| s.float(name)).unwrap_or(0.0) as i64;
            if value == 0 {
                fallback
            } else {
                value as u32
            }
        };
        slides.push(LoadedSlide {
            slide: ThemeSlide {
                group: group.to_string(),
                theme: theme.to_string(),
                name: item.str("name").to_string(),
                number: (index + 1) as u32,
                path: path.display().to_string(),
                boxes,
                other_objects: others,
                size: (dimension("width", 1920), dimension("height", 1080)),
            },
            base,
        });
    }
    slides
}

fn find_theme_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            find_theme_files(&path, out);
        }
        if path.file_name().is_some_and(|n| n == "Theme") && path.is_file() {
            out.push(path);
        }
    }
}

/// `read_theme_tree(root)`: every theme slide under a copy of ProPresenter's Themes folder (`Group/Theme/Theme`, or `Theme/Theme`), in the
/// order of the sorted paths. A theme file that cannot be read is simply not offered.
pub fn read_theme_tree(root: &Path) -> Vec<LoadedSlide> {
    let mut files = Vec::new();
    find_theme_files(root, &mut files);
    let mut keyed: Vec<(Vec<String>, PathBuf)> = files
        .into_iter()
        .filter_map(|file| {
            let relative = file.strip_prefix(root).ok()?;
            let parts: Vec<String> = relative
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect();
            Some((parts, file))
        })
        .collect();
    keyed.sort_by(|a, b| a.0.cmp(&b.0));
    let mut found = Vec::new();
    for (parts, file) in keyed {
        let folders = &parts[..parts.len() - 1];
        if folders.is_empty() {
            continue;
        }
        let (group, theme) = (
            folders[..folders.len() - 1].join("/"),
            folders[folders.len() - 1].clone(),
        );
        if let Ok(slides) = read_theme_file(&file, &group, &theme) {
            found.extend(slides);
        }
    }
    found
}

/// `ThemeSlide.theme_label`.
pub fn theme_label(slide: &ThemeSlide) -> String {
    slide.theme_label()
}

/// `find_slide(slides, theme, slide)`: the theme slide named by config. `theme` is "Name" or "Group/Name" (any case); `slide` is a name, a
/// 1-based number, or empty (the first with a text box).
pub fn find_slide<'a>(
    slides: &'a [LoadedSlide],
    theme: &str,
    slide: &str,
) -> Result<&'a LoadedSlide> {
    let wanted = casefold(versevision_core::pystr::strip(theme));
    let pool: Vec<&LoadedSlide> = slides
        .iter()
        .filter(|s| {
            wanted == casefold(&s.slide.theme) || wanted == casefold(&s.slide.theme_label())
        })
        .collect();
    if pool.is_empty() {
        let mut names: Vec<String> = slides.iter().map(|s| s.slide.theme_label()).collect();
        names.sort();
        names.dedup();
        let listed = if names.is_empty() {
            "none found".to_string()
        } else {
            names.join(", ")
        };
        return Err(PpError::theme(format!(
            "No ProPresenter theme named '{theme}'. Loaded themes: {listed}"
        )));
    }
    let mut labels: Vec<String> = pool.iter().map(|s| s.slide.theme_label()).collect();
    labels.sort();
    labels.dedup();
    if labels.len() > 1 {
        return Err(PpError::theme(format!(
            "'{theme}' matches several themes ({}); use Group/Name",
            labels.join(", ")
        )));
    }
    let spec = versevision_core::pystr::strip(slide);
    if spec.is_empty() {
        return pool
            .iter()
            .copied()
            .find(|s| usable(&s.slide))
            .ok_or_else(|| {
                PpError::theme(format!(
                    "Theme '{}' has no slide with a text box",
                    pool[0].slide.theme_label()
                ))
            });
    }
    let chosen = if !spec.is_empty() && spec.chars().all(|c| c.is_ascii_digit()) {
        let number = spec.parse::<u64>().ok();
        pool.iter()
            .copied()
            .find(|s| Some(u64::from(s.slide.number)) == number)
    } else {
        pool.iter()
            .copied()
            .find(|s| casefold(&s.slide.name) == casefold(spec))
    };
    let Some(chosen) = chosen else {
        let slides_list: Vec<String> = pool
            .iter()
            .map(|s| {
                format!(
                    "{}={}",
                    s.slide.number,
                    if s.slide.name.is_empty() {
                        "(unnamed)"
                    } else {
                        &s.slide.name
                    }
                )
            })
            .collect();
        return Err(PpError::theme(format!(
            "Theme '{}' has no slide '{spec}'. Its slides: {}",
            pool[0].slide.theme_label(),
            slides_list.join(", ")
        )));
    };
    if !usable(&chosen.slide) {
        return Err(PpError::theme(format!(
            "Slide '{}' has no text box, so it cannot show a verse or point",
            chosen.slide.label()
        )));
    }
    Ok(chosen)
}

/// `resolve_looks(config, transport)` once the theme folder is read: the slides chosen by `verses.theme`/`points.theme`, or `None` when
/// neither is set.
pub fn resolve_looks(
    slides: &[LoadedSlide],
    verse_theme: &str,
    verse_slide: &str,
    point_theme: &str,
    point_slide: &str,
) -> Result<Option<LoadedLooks>> {
    let (verse_theme, point_theme) = (
        versevision_core::pystr::strip(verse_theme),
        versevision_core::pystr::strip(point_theme),
    );
    if verse_theme.is_empty() && point_theme.is_empty() {
        return Ok(None);
    }
    let verse = if verse_theme.is_empty() {
        None
    } else {
        Some(find_slide(slides, verse_theme, verse_slide)?.clone())
    };
    let point = if point_theme.is_empty() {
        None
    } else {
        Some(find_slide(slides, point_theme, point_slide)?.clone())
    };
    Ok(Some(LoadedLooks { verse, point }))
}

/// One row of the look picker (`web/server.py: list_looks`): the theme slides that have a text box.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PickerRow {
    pub key: String,
    pub label: String,
    pub group: String,
    pub theme: String,
    pub slide: String,
    pub number: u32,
    pub boxes: usize,
    pub reference_box: bool,
}

pub fn picker_rows(slides: &[LoadedSlide]) -> Vec<PickerRow> {
    slides
        .iter()
        .filter(|s| usable(&s.slide))
        .map(|s| PickerRow {
            key: format!("{}::{}", s.slide.theme_label(), s.slide.number),
            label: s.slide.label(),
            group: s.slide.group.clone(),
            theme: s.slide.theme.clone(),
            slide: if s.slide.name.is_empty() {
                format!("Slide {}", s.slide.number)
            } else {
                s.slide.name.clone()
            },
            number: s.slide.number,
            boxes: s.slide.boxes.len(),
            reference_box: has_reference_box(&s.slide),
        })
        .collect()
}

/// `_config_with_looks`: a picker key "Theme name::slide number" split into the two config values (`rpartition("::")`).
pub fn split_look_key(key: &str) -> (String, String) {
    match key.rfind("::") {
        Some(at) => (key[..at].to_string(), key[at + 2..].to_string()),
        None => (String::new(), key.to_string()),
    }
}

fn round_half_even(value: f64) -> i64 {
    value.round_ties_even() as i64
}

fn latin1_replace(text: &str) -> Vec<u8> {
    text.chars()
        .map(|c| {
            if (c as u32) < 256 {
                c as u32 as u8
            } else {
                b'?'
            }
        })
        .collect()
}

/// `_ensure_run_formatting(text)`: some theme text boxes hold an EMPTY rtf (no font, colour or size run: the look lives only in the box's
/// attributes). Text typed into such a box renders invisible (default black on the output), so write the look into the rtf as well.
pub fn ensure_run_formatting(text: &mut Message) {
    let rtf = text.bytes("rtf_data");
    let contains = |needle: &[u8]| rtf.windows(needle.len()).any(|w| w == needle);
    if contains(b"\\f0") && contains(b"\\cf") {
        return;
    }
    let attributes = text.msg("attributes");
    let fill = attributes.and_then(|a| a.msg("text_solid_fill"));
    let channel = |name: &str| {
        round_half_even(fill.map(|f| f.float(name)).unwrap_or(0.0).clamp(0.0, 1.0) * 255.0)
    };
    let (red, green, blue) = if fill.is_some() {
        (channel("red"), channel("green"), channel("blue"))
    } else {
        (255, 255, 255)
    };
    let font_message = attributes.and_then(|a| a.msg("font"));
    let name = font_message
        .map(|f| f.str("name"))
        .filter(|n| !n.is_empty())
        .unwrap_or("Helvetica");
    let font = latin1_replace(name);
    let raw_size = font_message
        .map(|f| f.float("size"))
        .filter(|s| *s != 0.0)
        .unwrap_or(42.0);
    let size = round_half_even(raw_size * 2.0);
    let alignment = attributes
        .and_then(|a| a.msg("paragraph_style"))
        .map(|p| p.i32("alignment"))
        .unwrap_or(0);
    let align: &[u8] = match alignment {
        0 => b"\\ql",
        1 => b"\\qr",
        2 => b"\\qc",
        3 => b"\\qj",
        _ => b"\\qc",
    };
    let lower: Vec<u8> = font.iter().map(|b| b.to_ascii_lowercase()).collect();
    let bold: &[u8] = if lower.windows(4).any(|w| w == b"bold") {
        b"\\b"
    } else {
        b""
    };
    let scaled = |v: i64| round_half_even(v as f64 / 255.0 * 100000.0);
    let mut out = Vec::new();
    out.extend_from_slice(b"{\\rtf1\\ansi\\ansicpg1252\\cocoartf2577\n\\cocoatextscaling0\\cocoaplatform0{\\fonttbl\\f0\\fnil\\fcharset0 ");
    out.extend_from_slice(&font);
    out.extend_from_slice(b";}\n");
    out.extend_from_slice(
        format!(
            "{{\\colortbl;\\red255\\green255\\blue255;\\red{red}\\green{green}\\blue{blue};}}\n{{\\*\\expandedcolortbl;;\\csgenericrgb\\c{}\\c{}\\c{};}}\n",
            scaled(red),
            scaled(green),
            scaled(blue)
        )
        .as_bytes(),
    );
    out.extend_from_slice(b"\\pard\\pardirnatural\\partightenfactor0");
    out.extend_from_slice(align);
    out.extend_from_slice(b"\n\n\\f0");
    out.extend_from_slice(bold);
    out.extend_from_slice(format!("\\fs{size} \\cf2 }}").as_bytes());
    text.set_bytes("rtf_data", out);
}

/// `apply_look(slide_action, look, kind, ...)`: replace the look of one generated slide with the theme slide's. Returns true when the slide
/// has a separate reference box.
pub fn apply_look(
    slide_action: &mut Message,
    look: &LoadedSlide,
    is_point: bool,
    reference: &str,
    body: &str,
    point: &str,
) -> Result<bool> {
    let base = crate::model::base_slide_mut(slide_action);
    *base = look.base.clone();
    let mut elements = base.msgs_mut("elements");
    let texts: Vec<usize> = (0..elements.len())
        .filter(|i| has_text(elements[*i]))
        .collect();
    if texts.is_empty() {
        return Err(PpError::theme(format!(
            "Theme slide '{}' has no text box",
            look.slide.label()
        )));
    }
    for i in &texts {
        ensure_run_formatting(elements[*i].msg_mut("element").msg_mut("text"));
    }
    let area = |wrapper: &Message| {
        let (_, _, w, h) = rect(wrapper);
        w * h
    };
    let mut ranked: Vec<(usize, f64)> = texts.iter().map(|i| (*i, area(elements[*i]))).collect();
    ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)); // stable, largest first
    let main = ranked[0].0;
    let others: Vec<usize> = ranked[1..].iter().map(|r| r.0).collect();
    let mut reference_box = None;
    if is_point {
        elements[main].msg_mut("element").set_str("name", point);
    } else {
        elements[main].msg_mut("element").set_str("name", body);
        let main_rect = rect(elements[main]);
        let clear: Vec<usize> = others
            .iter()
            .copied()
            .filter(|o| overlap(main_rect, rect(elements[*o])) <= REFERENCE_OVERLAP_LIMIT)
            .collect();
        reference_box = clear.iter().copied().fold(None::<(usize, f64)>, |best, o| {
            let a = area(elements[o]);
            match best {
                Some((_, b)) if a >= b => best,
                _ => Some((o, a)),
            }
        });
        if let Some((index, _)) = reference_box {
            elements[index]
                .msg_mut("element")
                .set_str("name", reference);
        }
    }
    for extra in others {
        if reference_box.map(|r| r.0) != Some(extra) {
            elements[extra].msg_mut("element").set_str("name", EXTRA);
        }
    }
    Ok(reference_box.is_some())
}
