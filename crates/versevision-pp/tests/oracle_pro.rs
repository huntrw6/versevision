//! The ProPresenter file writer and the theme reader against the Python app (`oracle/pro/`, written by `python/scripts/oracle_pro.py`):
//! every case must produce the same bytes (or the same error) as Python.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::{json, Value};
use versevision_core::config::PresentationTemplateConfig;
use versevision_core::plan::SlidePlan;
use versevision_pp::template::{
    inspect_point_candidates, inspect_template_candidates, validate_template,
};
use versevision_pp::themes::{self, LoadedLooks};
use versevision_pp::writer::{self, PresentationWriter, SeededUuids};
use versevision_pp::{model, Message, PpError};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn pro() -> PathBuf {
    repo().join("oracle/pro")
}

#[derive(Debug, Deserialize)]
struct LooksSpec {
    themes: String,
    #[serde(default)]
    verse_theme: String,
    #[serde(default)]
    verse_slide: String,
    #[serde(default)]
    point_theme: String,
    #[serde(default)]
    point_slide: String,
}

#[derive(Debug, Deserialize)]
struct Options {
    mode: String,
    uuid_seed: String,
    now: i64,
    definition: PresentationTemplateConfig,
    base: Option<String>,
    template: Option<String>,
    #[serde(default)]
    name: String,
    #[serde(default)]
    clear_generated: bool,
    #[serde(default)]
    keep_uuid: bool,
    #[serde(default)]
    remove: Vec<String>,
    #[serde(default)]
    body: String,
    #[serde(default)]
    label: String,
    looks: Option<LooksSpec>,
}

fn resolve(reference: &str) -> PathBuf {
    match reference.strip_prefix("fixture:") {
        Some(rest) => repo().join("tests/fixtures").join(rest),
        None => pro().join(format!("{reference}.pro")),
    }
}

fn read(reference: &str) -> Vec<u8> {
    fs::read(resolve(reference)).unwrap_or_else(|e| panic!("{reference}: {e}"))
}

fn run(options: &Options, plan: &SlidePlan) -> Result<(Vec<u8>, Value), PpError> {
    let looks: Option<LoadedLooks> = match &options.looks {
        None => None,
        Some(spec) => {
            let slides = themes::read_theme_tree(&pro().join(&spec.themes));
            let pick = |theme: &str, slide: &str| -> Result<Option<themes::LoadedSlide>, PpError> {
                if theme.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(themes::find_slide(&slides, theme, slide)?.clone()))
                }
            };
            Some(LoadedLooks {
                verse: pick(&spec.verse_theme, &spec.verse_slide)?,
                point: pick(&spec.point_theme, &spec.point_slide)?,
            })
        }
    };
    let mut writer = PresentationWriter::new(SeededUuids::new(&options.uuid_seed));
    let base = options.base.as_deref().map(read);
    let template = options.template.as_deref().map(read);
    let definition = &options.definition;
    let looks = looks.as_ref();
    let plan_result = |w: writer::Written| {
        (
            w.bytes,
            serde_json::to_value(w.result).expect("result as json"),
        )
    };
    match options.mode.as_str() {
        "append" => writer
            .append_plan(
                &base.expect("base"),
                plan,
                definition,
                looks,
                template.as_deref(),
                Some(&options.remove),
            )
            .map(plan_result),
        "replace" => writer
            .replace_generated(
                &base.expect("base"),
                plan,
                definition,
                looks,
                template.as_deref(),
            )
            .map(plan_result),
        "derive" => writer
            .derive_presentation(
                &base.expect("base"),
                template.as_deref(),
                plan,
                definition,
                looks,
                &options.name,
                options.clear_generated,
                options.keep_uuid,
                options.now,
            )
            .map(plan_result),
        "create" => writer
            .create_presentation(
                &base.expect("base"),
                plan,
                definition,
                looks,
                &options.name,
                options.now,
            )
            .map(plan_result),
        "template_file" => writer
            .create_template_file(&base.expect("base"), definition, &options.name, options.now)
            .map(plan_result),
        "synthetic" => writer
            .append_synthetic_slide(&base.expect("base"), &options.body, &options.label)
            .map(|(bytes, r)| (bytes, serde_json::to_value(r).expect("result as json"))),
        other => panic!("unknown mode {other}"),
    }
}

fn first_difference(a: &[u8], b: &[u8]) -> String {
    match a.iter().zip(b).position(|(x, y)| x != y) {
        Some(at) => format!(
            "first difference at byte {at} (rust {} bytes, python {} bytes)",
            a.len(),
            b.len()
        ),
        None => format!(
            "one is a prefix of the other (rust {} bytes, python {} bytes)",
            a.len(),
            b.len()
        ),
    }
}

fn check_case(dir: &Path) -> Result<(), String> {
    let options: Options =
        toml::from_str(&fs::read_to_string(dir.join("options.toml")).map_err(|e| e.to_string())?)
            .map_err(|e| format!("options.toml: {e}"))?;
    let plan: SlidePlan = serde_json::from_str(
        &fs::read_to_string(dir.join("plan.json")).map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("plan.json: {e}"))?;
    let expected: Value = serde_json::from_str(
        &fs::read_to_string(dir.join("expected.json")).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    match (run(&options, &plan), expected["error"].as_str()) {
        (Ok((bytes, result)), None) => {
            let want = fs::read(dir.join("expected.pro")).map_err(|e| e.to_string())?;
            if bytes != want {
                return Err(first_difference(&bytes, &want));
            }
            if result != expected["result"] {
                return Err(format!(
                    "result differs:\n rust   {result}\n python {}",
                    expected["result"]
                ));
            }
        }
        (Err(error), Some(want)) => {
            if error.python_form() != want {
                return Err(format!(
                    "error differs: rust {:?}, python {want:?}",
                    error.python_form()
                ));
            }
        }
        (Ok(_), Some(want)) => return Err(format!("Rust wrote a file, Python raised {want:?}")),
        (Err(error), None) => {
            return Err(format!(
                "Rust raised {:?}, Python wrote a file",
                error.python_form()
            ))
        }
    }
    // validate_template on the file that holds the template
    if let Some(want) = expected["validate"].as_str() {
        let target = options
            .template
            .as_deref()
            .or(options.base.as_deref())
            .expect("a file");
        let message =
            Message::decode_named(model::PRESENTATION, &read(target)).map_err(|e| e.to_string())?;
        let got = match validate_template(&message, &options.definition) {
            Ok(_) => "ok".to_string(),
            Err(error) => error.python_form(),
        };
        if got != want {
            return Err(format!(
                "validate_template differs: rust {got:?}, python {want:?}"
            ));
        }
    }
    Ok(())
}

#[test]
fn oracle_pro_cases_are_byte_identical() {
    let mut dirs: Vec<PathBuf> = fs::read_dir(pro().join("cases"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    dirs.sort();
    assert!(dirs.len() >= 25, "{} cases", dirs.len());
    let mut failures = Vec::new();
    let mut files = 0;
    let mut errors = 0;
    for dir in &dirs {
        let name = dir.file_name().unwrap().to_string_lossy().to_string();
        match check_case(dir) {
            Ok(()) => {
                if dir.join("expected.pro").exists() {
                    files += 1;
                } else {
                    errors += 1;
                }
            }
            Err(why) => failures.push(format!("{name}: {why}")),
        }
    }
    println!(
        "oracle pro: {}/{} cases byte-identical ({files} files, {errors} identical errors)",
        dirs.len() - failures.len(),
        dirs.len()
    );
    assert!(
        failures.is_empty(),
        "{} cases differ:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

fn slide_json(slide: &themes::LoadedSlide, root: &Path) -> Value {
    let s = &slide.slide;
    let relative = Path::new(&s.path)
        .strip_prefix(root)
        .unwrap()
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/");
    json!({
        "group": s.group, "theme": s.theme, "name": s.name, "number": s.number, "path": relative,
        "boxes": s.boxes.iter().map(|b| json!({"name": b.name, "x": b.x, "y": b.y, "w": b.w, "h": b.h, "font": b.font, "size": b.size})).collect::<Vec<_>>(),
        "other_objects": s.other_objects, "size": [s.size.0, s.size.1],
        "theme_label": s.theme_label(), "label": s.label(), "has_reference_box": themes::has_reference_box(s), "usable": themes::usable(s),
    })
}

#[test]
fn oracle_themes_match_python() {
    let root = pro().join("themes");
    let expected: Value =
        serde_json::from_str(&fs::read_to_string(pro().join("themes.json")).unwrap()).unwrap();
    let slides = themes::read_theme_tree(&root);
    let mut checked = 0;
    let mut bad = Vec::new();
    let want_slides = expected["slides"].as_array().unwrap();
    if slides.len() != want_slides.len() {
        bad.push(format!(
            "{} theme slides, Python has {}",
            slides.len(),
            want_slides.len()
        ));
    }
    for (got, want) in slides.iter().zip(want_slides) {
        checked += 1;
        let got = slide_json(got, &root);
        if &got != want {
            bad.push(format!("slide differs:\n rust   {got}\n python {want}"));
        }
    }
    let rows: Vec<Value> = themes::picker_rows(&slides)
        .iter()
        .map(|r| serde_json::to_value(r).unwrap())
        .collect();
    checked += 1;
    if rows != *expected["picker"].as_array().unwrap() {
        bad.push(format!(
            "picker rows differ:\n rust   {}\n python {}",
            Value::Array(rows),
            expected["picker"]
        ));
    }
    for want in expected["finds"].as_array().unwrap() {
        checked += 1;
        let (theme, slide) = (
            want["theme"].as_str().unwrap(),
            want["slide"].as_str().unwrap(),
        );
        let got = match themes::find_slide(&slides, theme, slide) {
            Ok(found) => {
                json!({"ok": {"label": found.slide.label(), "number": found.slide.number, "theme_label": found.slide.theme_label()}})
            }
            Err(error) => json!({"error": error.to_string()}),
        };
        let mut want_core = json!({});
        for key in ["ok", "error"] {
            if want.get(key).is_some() {
                want_core[key] = want[key].clone();
            }
        }
        if got != want_core {
            bad.push(format!(
                "find_slide({theme:?}, {slide:?}): rust {got}, python {want_core}"
            ));
        }
    }
    println!(
        "oracle themes: {}/{} checks identical ({} theme slides, {} finds)",
        checked - bad.len(),
        checked,
        slides.len(),
        expected["finds"].as_array().unwrap().len()
    );
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[test]
fn oracle_template_candidates_match_python() {
    let expected: Value =
        serde_json::from_str(&fs::read_to_string(pro().join("candidates.json")).unwrap()).unwrap();
    let mut checked = 0;
    let mut bad = Vec::new();
    for (name, want) in expected.as_object().unwrap() {
        let path = if name == "sample" {
            repo().join("tests/fixtures/sample/Sample.pro")
        } else {
            pro().join(format!("files/{name}.pro"))
        };
        let message = Message::decode_named(model::PRESENTATION, &fs::read(path).unwrap()).unwrap();
        let candidates =
            serde_json::to_value(inspect_template_candidates(&message, "Reference", "Verse"))
                .unwrap();
        let points: Vec<Value> = inspect_point_candidates(&message, "Sermon Point")
            .iter()
            .map(|(n, u, f)| json!([n, u, f]))
            .collect();
        checked += 1;
        if want.get("error").is_some() {
            continue;
        }
        if candidates != want["templates"] || Value::Array(points.clone()) != want["points"] {
            bad.push(format!(
                "{name}: rust {candidates} {}, python {} {}",
                Value::Array(points),
                want["templates"],
                want["points"]
            ));
        }
    }
    println!(
        "oracle candidates: {}/{} files identical",
        checked - bad.len(),
        checked
    );
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}
