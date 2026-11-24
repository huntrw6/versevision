//! `versevision themes` and `themes use` (`cli_themes.py`).

use serde_json::{json, Value};
use versevision_core::config::AppConfig;
use versevision_core::looks::ThemeSlide;
use versevision_pp::errors::VvError;
use versevision_pp::transport::Transport;

use crate::app::{echo, out, Rt};
use crate::cmds::{dumps, fail, is_json, load_config, save_config};
use crate::def::{ThemesArgs, ThemesCommand};
use crate::rich::{Overflow, Style, Table, R};

/// Python's `format(x, "g")`.
pub fn fmt_g(x: f64) -> String {
    if x == 0.0 {
        return "0".into();
    }
    let exponent = x.abs().log10().floor() as i32;
    let precision = 6;
    if !(-5..precision).contains(&exponent) {
        let text = format!("{:.*e}", precision as usize - 1, x);
        let (mantissa, exp) = text.split_once('e').unwrap_or((&text, "0"));
        let mantissa = if mantissa.contains('.') {
            mantissa.trim_end_matches('0').trim_end_matches('.')
        } else {
            mantissa
        };
        let exp: i32 = exp.parse().unwrap_or(0);
        return format!(
            "{mantissa}e{}{:02}",
            if exp < 0 { '-' } else { '+' },
            exp.abs()
        );
    }
    let decimals = (precision - 1 - exponent).max(0) as usize;
    let text = format!("{x:.decimals$}");
    if text.contains('.') {
        text.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        text
    }
}

/// Python's `round(x)` (half to even).
pub fn pyround(x: f64) -> i64 {
    let floor = x.floor();
    let diff = x - floor;
    if (diff - 0.5).abs() < 1e-12 {
        let f = floor as i64;
        if f % 2 == 0 {
            f
        } else {
            f + 1
        }
    } else {
        x.round() as i64
    }
}

fn largest(slide: &ThemeSlide) -> Option<&versevision_core::looks::TextBox> {
    let mut best: Option<&versevision_core::looks::TextBox> = None;
    for b in &slide.boxes {
        // `max(key=area)`: the first of the equal ones
        if best.is_none_or(|m| b.w * b.h > m.w * m.h) {
            best = Some(b);
        }
    }
    best
}

pub fn fetch_slides(
    rt: &Rt,
    config: &AppConfig,
) -> Result<Vec<versevision_pp::themes::LoadedSlide>, VvError> {
    let transport = crate::runcmds::transport(rt, config)?;
    let dir = tempfile::tempdir().map_err(VvError::from)?;
    transport.fetch_themes(dir.path())?;
    Ok(versevision_pp::themes::read_theme_tree(dir.path()))
}

fn print_error(rt: &Rt, error: &VvError) -> i32 {
    out(&rt
        .term
        .out()
        .print(&format!("[red]{}[/red]", error.message)));
    error.exit_code() as i32
}

pub fn themes(rt: &Rt, a: ThemesArgs) -> i32 {
    match a.command {
        Some(ThemesCommand::Use {
            verses,
            verse_slide,
            points,
            point_slide,
            clear,
            node,
            ..
        }) => use_theme(rt, verses, verse_slide, points, point_slide, clear, node),
        None => list(rt, a.node, &a.output),
    }
}

fn list(rt: &Rt, node: Option<String>, output: &str) -> i32 {
    let config = match crate::runcmds::config_with_node(rt, node.as_deref()) {
        Ok(c) => c,
        Err(e) => return print_error(rt, &e),
    };
    let slides = match fetch_slides(rt, &config) {
        Ok(s) => s,
        Err(e) => return print_error(rt, &e),
    };
    if is_json(output) {
        let items: Vec<Value> = slides
            .iter()
            .map(|l| {
                let s = &l.slide;
                json!({
                    "theme": s.theme_label(), "slide": s.name, "number": s.number, "text_boxes": s.boxes.len(),
                    "other_objects": s.other_objects,
                    "largest_box": largest(s).map(|b| json!([pyround(b.w), pyround(b.h)])),
                })
            })
            .collect();
        echo(&dumps(&Value::Array(items)));
        return 0;
    }
    let marks = [
        ("verse", config.verses.theme.trim().to_lowercase()),
        ("point", config.points.theme.trim().to_lowercase()),
    ];
    let mut table = Table::new(&[
        "Theme",
        "Slide",
        "Text boxes",
        "Main box",
        "Font",
        "In use for",
    ]);
    for l in &slides {
        let s = &l.slide;
        let main = largest(s);
        let used: Vec<&str> = marks
            .iter()
            .filter(|(_, v)| {
                !v.is_empty()
                    && (*v == s.theme.to_lowercase() || *v == s.theme_label().to_lowercase())
            })
            .map(|(k, _)| *k)
            .collect();
        let name = if s.name.is_empty() {
            "(unnamed)".to_string()
        } else {
            s.name.clone()
        };
        let boxes = if s.boxes.is_empty() {
            "[dim]none[/dim]".to_string()
        } else {
            s.boxes.len().to_string()
        };
        let (main_box, font) = match main {
            Some(b) => (
                format!("{}x{}", pyround(b.w), pyround(b.h)),
                format!("{} {}", b.font, fmt_g(b.size)).trim().to_string(),
            ),
            None => (String::new(), String::new()),
        };
        let cells = [
            s.theme_label(),
            format!("{}: {name}", s.number),
            boxes,
            main_box,
            font,
            used.join(", "),
        ];
        table.add_row(
            cells
                .iter()
                .map(|c| R::Text {
                    cells: crate::rich::markup(c, Style::NONE),
                    overflow: Overflow::Ellipsis,
                    no_wrap: false,
                })
                .collect(),
        );
    }
    let console = rt.term.out();
    let mut text = console.print_r(&R::Table(Box::new(table)));
    let verses = if config.verses.theme.is_empty() {
        "the VerseVision template"
    } else {
        &config.verses.theme
    };
    let points = if config.points.theme.is_empty() {
        "the VerseVision template"
    } else {
        &config.points.theme
    };
    text.push_str(&console.print(&format!(
        "Verses: {verses}   Points: {points}\nChoose:  versevision themes use --verses \"The biblke\" --points \"Sermon Points\"     Go back:  versevision themes use --clear"
    )));
    out(&text);
    0
}

fn use_theme(
    rt: &Rt,
    verses: Option<String>,
    verse_slide: String,
    points: Option<String>,
    point_slide: String,
    clear: bool,
    node: Option<String>,
) -> i32 {
    let config = match load_config(rt) {
        Ok(c) => c,
        Err(e) => return print_error(rt, &e),
    };
    let sets: Vec<String> = if clear {
        vec![
            "verses.theme=".into(),
            "verses.theme_slide=".into(),
            "points.theme=".into(),
            "points.theme_slide=".into(),
        ]
    } else {
        let mut s = Vec::new();
        if let Some(v) = &verses {
            s.push(format!("verses.theme={v}"));
            s.push(format!("verses.theme_slide={verse_slide}"));
        }
        if let Some(p) = &points {
            s.push(format!("points.theme={p}"));
            s.push(format!("points.theme_slide={point_slide}"));
        }
        s
    };
    if sets.is_empty() {
        return print_error(
            rt,
            &VvError::general("Choose --verses and/or --points (or --clear)"),
        );
    }
    let refs: Vec<&str> = sets.iter().map(String::as_str).collect();
    let updated = match config.with_options(None, &refs) {
        Ok(u) => u,
        Err(e) => return print_error(rt, &VvError::configuration(e.to_string())),
    };
    let console = rt.term.out();
    if !clear {
        let node_config = match crate::runcmds::config_with_node(rt, node.as_deref()) {
            Ok(c) => c,
            Err(e) => return print_error(rt, &e),
        };
        let slides = match fetch_slides(rt, &node_config) {
            Ok(s) => s,
            Err(e) => return print_error(rt, &e),
        };
        let mut chosen: Vec<(&str, &versevision_pp::themes::LoadedSlide)> = Vec::new();
        if let Some(v) = verses.as_deref().filter(|v| !v.is_empty()) {
            match versevision_pp::themes::find_slide(&slides, v, &verse_slide) {
                Ok(s) => chosen.push(("verses", s)),
                Err(e) => return print_error(rt, &VvError::from(e)),
            }
        }
        if let Some(p) = points.as_deref().filter(|p| !p.is_empty()) {
            match versevision_pp::themes::find_slide(&slides, p, &point_slide) {
                Ok(s) => chosen.push(("points", s)),
                Err(e) => return print_error(rt, &VvError::from(e)),
            }
        }
        for (what, s) in chosen {
            let n = s.slide.boxes.len();
            out(&console.print(&format!(
                "{what}: {}  ({n} text box{})",
                s.slide.label(),
                if n != 1 { "es" } else { "" }
            )));
        }
    }
    match save_config(rt, &updated, true) {
        Ok(_) => {
            out(&console.print(if clear {
                "Back to the VerseVision template slides."
            } else {
                "Saved."
            }));
            0
        }
        Err(e) => fail(rt, &e),
    }
}
