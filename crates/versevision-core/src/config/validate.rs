//! pydantic's validation (lax mode) over a `serde_json::Value`: the checks, the coercions and the exact error texts,
//! reporting only the first error, in the order pydantic finds it (`exc.errors()[0]`).

use std::cmp::Ordering;

use serde_json::{Map, Number, Value};

use super::pynum::{ascii_float, casefold, py_float, pydantic_int_from_str, PyInt};
use super::schema::{Def, Field, Model, Ty, PROVIDERS};
use crate::pystr::strip;

/// The first validation error: where (`loc`, joined with `.`) and what (`msg`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationError {
    pub loc: Vec<String>,
    pub msg: String,
}

impl ValidationError {
    pub fn location(&self) -> String {
        self.loc.join(".")
    }
}

type Loc = Vec<String>;
type Check<T> = Result<T, ValidationError>;

fn fail<T>(loc: &Loc, msg: impl Into<String>) -> Check<T> {
    Err(ValidationError {
        loc: loc.clone(),
        msg: msg.into(),
    })
}

/// Validate `input` as one model; the result holds every field of the model (defaults filled in), in model order.
pub fn validate_model(model: &Model, input: &Value, loc: &mut Loc) -> Check<Map<String, Value>> {
    let Some(object) = obj(input) else {
        return fail(
            loc,
            format!(
                "Input should be a valid dictionary or instance of {}",
                model.name
            ),
        );
    };
    fields_of(model, object, loc)
}

fn fields_of(
    model: &Model,
    object: &Map<String, Value>,
    loc: &mut Loc,
) -> Check<Map<String, Value>> {
    let mut out = Map::new();
    for field in model.fields {
        loc.push(field.name.to_string());
        let value = match object.get(field.name) {
            Some(given) => validate_field(field, given, loc)?,
            None => default_of(field, loc)?,
        };
        loc.pop();
        out.insert(field.name.to_string(), value);
    }
    Ok(out)
}

fn default_of(field: &Field, loc: &mut Loc) -> Check<Value> {
    Ok(match field.default {
        Def::Bool(b) => Value::Bool(b),
        Def::Int(i) => Value::from(i),
        Def::Float(x) => float_value(x),
        Def::Str(s) => Value::String(s.to_string()),
        Def::Null => Value::Null,
        Def::StrList(items) => Value::Array(
            items
                .iter()
                .map(|s| Value::String((*s).to_string()))
                .collect(),
        ),
        Def::StrMap(pairs) => Value::Object(
            pairs
                .iter()
                .map(|(k, v)| ((*k).to_string(), Value::String((*v).to_string())))
                .collect(),
        ),
        Def::EmptyMap => Value::Object(Map::new()),
        Def::Model => match field.ty {
            Ty::Model(model) => Value::Object(fields_of(model, &Map::new(), loc)?),
            _ => Value::Null,
        },
        Def::Required => return fail(loc, "Field required"),
    })
}

/// The JSON number for a float (every float of the config is finite once it passed its range check).
pub fn float_value(x: f64) -> Value {
    Number::from_f64(x).map_or(Value::Null, Value::Number)
}

fn validate_field(field: &Field, value: &Value, loc: &mut Loc) -> Check<Value> {
    match field.ty {
        Ty::Bool => Ok(Value::Bool(boolean(value, loc)?)),
        Ty::Int { ge, le } => {
            let n = integer(value, loc)?;
            if let Some(le) = le {
                if n > le {
                    return fail(loc, format!("Input should be less than or equal to {le}"));
                }
            }
            if let Some(ge) = ge {
                if n < ge {
                    return fail(
                        loc,
                        format!("Input should be greater than or equal to {ge}"),
                    );
                }
            }
            Ok(Value::from(n))
        }
        Ty::Float { gt, ge, le } => {
            let x = float(value, loc)?;
            // pydantic-core checks le, lt, ge, gt in this order; a NaN fails the first comparison it meets
            if let Some(le) = le {
                if !is_le(x, le) {
                    return fail(
                        loc,
                        format!("Input should be less than or equal to {}", bound_text(le)),
                    );
                }
            }
            if let Some(ge) = ge {
                if !is_ge(x, ge) {
                    return fail(
                        loc,
                        format!(
                            "Input should be greater than or equal to {}",
                            bound_text(ge)
                        ),
                    );
                }
            }
            if let Some(gt) = gt {
                if !is_gt(x, gt) {
                    return fail(
                        loc,
                        format!("Input should be greater than {}", bound_text(gt)),
                    );
                }
            }
            Ok(float_value(x))
        }
        Ty::Str => text(value, loc).map(Value::String),
        Ty::OptStr => match value {
            Value::Null => Ok(Value::Null),
            other => text(other, loc).map(Value::String),
        },
        Ty::Path => match value {
            Value::String(s) => Ok(Value::String(normalize_path(s))),
            _ => fail(
                loc,
                "Input is not a valid path for <class 'pathlib._local.Path'>",
            ),
        },
        Ty::Literal(allowed) => match value {
            Value::String(s) if allowed.contains(&s.as_str()) => Ok(value.clone()),
            _ => fail(loc, literal_message(allowed)),
        },
        Ty::StrList => {
            let Some(items) = value.as_array() else {
                return fail(loc, "Input should be a valid list");
            };
            let mut out = Vec::with_capacity(items.len());
            for (i, item) in items.iter().enumerate() {
                loc.push(i.to_string());
                out.push(Value::String(text(item, loc)?));
                loc.pop();
            }
            Ok(Value::Array(out))
        }
        Ty::StrMap => {
            let Some(object) = obj(value) else {
                return fail(loc, "Input should be a valid dictionary");
            };
            let mut out = Map::new();
            for (key, item) in object {
                loc.push(key.clone());
                out.insert(key.clone(), Value::String(text(item, loc)?));
                loc.pop();
            }
            Ok(Value::Object(out))
        }
        Ty::Model(model) => validate_model(model, value, loc).map(Value::Object),
        Ty::OptModel(model) => match value {
            Value::Null => Ok(Value::Null),
            other => validate_model(model, other, loc).map(Value::Object),
        },
        Ty::Providers => {
            let Some(object) = obj(value) else {
                return fail(loc, "Input should be a valid dictionary");
            };
            let mut out = Map::new();
            for (key, item) in object {
                loc.push(key.clone());
                out.insert(key.clone(), Value::Object(provider(item, loc)?));
                loc.pop();
            }
            Ok(Value::Object(out))
        }
        Ty::Profiles => {
            let Some(object) = obj(value) else {
                return fail(loc, "Input should be a valid dictionary");
            };
            for (key, item) in object {
                if obj(item).is_none() {
                    loc.push(key.clone());
                    return fail(loc, "Input should be a valid dictionary");
                }
            }
            Ok(value.clone())
        }
    }
}

/// One entry of `bible.providers`: the `type` picks the model.
fn provider(value: &Value, loc: &mut Loc) -> Check<Map<String, Value>> {
    let Some(object) = obj(value) else {
        return fail(
            loc,
            "Input should be a valid dictionary or object to extract fields from",
        );
    };
    let Some(tag) = object.get("type") else {
        return fail(loc, "Unable to extract tag using discriminator 'type'");
    };
    let expected: Vec<String> = PROVIDERS
        .iter()
        .map(|m| format!("'{}'", tag_of(m)))
        .collect();
    let chosen = tag
        .as_str()
        .and_then(|t| PROVIDERS.iter().find(|m| tag_of(m) == t));
    let Some(model) = chosen else {
        let shown = match tag {
            Value::String(s) => format!("'{s}'"),
            other => other.to_string(),
        };
        return fail(
            loc,
            format!(
                "Input tag {shown} found using 'type' does not match any of the expected tags: {}",
                expected.join(", ")
            ),
        );
    };
    loc.push(tag_of(model).to_string());
    let result = fields_of(model, object, loc)?;
    loc.pop();
    Ok(result)
}

/// The `type` literal of a provider model.
fn tag_of(model: &Model) -> &'static str {
    match model.fields[0].default {
        Def::Str(tag) => tag,
        _ => "",
    }
}

// A NaN is neither below, above nor equal to anything: it fails every bound, like in Python.
fn is_le(x: f64, bound: f64) -> bool {
    matches!(
        x.partial_cmp(&bound),
        Some(Ordering::Less | Ordering::Equal)
    )
}

fn is_ge(x: f64, bound: f64) -> bool {
    matches!(
        x.partial_cmp(&bound),
        Some(Ordering::Greater | Ordering::Equal)
    )
}

fn is_gt(x: f64, bound: f64) -> bool {
    matches!(x.partial_cmp(&bound), Some(Ordering::Greater))
}

fn bound_text(x: f64) -> String {
    if x.fract() == 0.0 {
        format!("{}", x as i64)
    } else {
        format!("{x}")
    }
}

fn literal_message(allowed: &[&str]) -> String {
    let quoted: Vec<String> = allowed.iter().map(|a| format!("'{a}'")).collect();
    match quoted.as_slice() {
        [one] => format!("Input should be {one}"),
        [init @ .., last] => format!("Input should be {} or {last}", init.join(", ")),
        [] => "Input should be a valid literal".to_string(),
    }
}

fn text(value: &Value, loc: &Loc) -> Check<String> {
    match value {
        Value::String(s) => Ok(s.clone()),
        _ => fail(loc, "Input should be a valid string"),
    }
}

/// pydantic's lax bool: bools, 0/1 (int or float), and the words 0 off f false n no 1 on t true y yes in any case.
fn boolean(value: &Value, loc: &Loc) -> Check<bool> {
    match value {
        Value::Bool(b) => Ok(*b),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                return match i {
                    0 => Ok(false),
                    1 => Ok(true),
                    _ => fail(
                        loc,
                        "Input should be a valid boolean, unable to interpret input",
                    ),
                };
            }
            match n.as_f64() {
                Some(0.0) => Ok(false),
                Some(1.0) => Ok(true),
                // a whole number other than 0 and 1 is read as a number and refused; a fraction is not a boolean at all
                Some(x) if n.is_u64() || x.fract() == 0.0 => fail(
                    loc,
                    "Input should be a valid boolean, unable to interpret input",
                ),
                _ => fail(loc, "Input should be a valid boolean"),
            }
        }
        Value::String(s) => match s.to_lowercase().as_str() {
            "0" | "off" | "f" | "false" | "n" | "no" => Ok(false),
            "1" | "on" | "t" | "true" | "y" | "yes" => Ok(true),
            _ => fail(
                loc,
                "Input should be a valid boolean, unable to interpret input",
            ),
        },
        _ => fail(loc, "Input should be a valid boolean"),
    }
}

fn clamp_f64(x: f64) -> i64 {
    // `as` saturates at the i64 limits, which is all a range check needs
    x as i64
}

/// pydantic's lax int: bool, int, a float without a fraction, a numeric string.
fn integer(value: &Value, loc: &Loc) -> Check<i64> {
    match value {
        Value::Bool(b) => Ok(i64::from(*b)),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                return Ok(i);
            }
            if let Some(u) = n.as_u64() {
                return Ok(i64::try_from(u).unwrap_or(i64::MAX));
            }
            match n.as_f64() {
                Some(x) if !x.is_finite() => fail(loc, "Input should be a finite number"),
                Some(x) if x.fract() != 0.0 => fail(
                    loc,
                    "Input should be a valid integer, got a number with a fractional part",
                ),
                Some(x) => Ok(clamp_f64(x)),
                None => fail(loc, "Input should be a valid integer"),
            }
        }
        Value::Object(_) if nonfinite(value).is_some() => {
            fail(loc, "Input should be a finite number")
        }
        Value::String(s) => match pydantic_int_from_str(s) {
            Some(PyInt::Small(i)) => Ok(i),
            Some(PyInt::Huge(x)) => Ok(clamp_f64(x)),
            None => fail(
                loc,
                "Input should be a valid integer, unable to parse string as an integer",
            ),
        },
        _ => fail(loc, "Input should be a valid integer"),
    }
}

/// pydantic's lax float: bool, int, float, a numeric string (`inf` and `nan` included).
fn float(value: &Value, loc: &Loc) -> Check<f64> {
    match value {
        Value::Bool(b) => Ok(f64::from(u8::from(*b))),
        Value::Number(n) => n
            .as_f64()
            .map_or_else(|| fail(loc, "Input should be a valid number"), Ok),
        Value::Object(_) if nonfinite(value).is_some() => Ok(nonfinite(value).unwrap_or(f64::NAN)),
        Value::String(s) => ascii_float(s).map_or_else(
            || {
                fail(
                    loc,
                    "Input should be a valid number, unable to parse string as a number",
                )
            },
            Ok,
        ),
        _ => fail(loc, "Input should be a valid number"),
    }
}

/// `pathlib.Path(text)` as `model_dump` shows it: repeated and trailing slashes and `.` parts go, an empty path is `.`.
fn normalize_path(path: &str) -> String {
    let rooted = path.starts_with('/');
    let double = path.starts_with("//") && !path.starts_with("///");
    let parts: Vec<&str> = path
        .split('/')
        .filter(|p| !p.is_empty() && *p != ".")
        .collect();
    let joined = parts.join("/");
    match (rooted, joined.is_empty()) {
        (true, true) => {
            if double {
                "//".to_string()
            } else {
                "/".to_string()
            }
        }
        (true, false) => format!("{}{joined}", if double { "//" } else { "/" }),
        (false, true) => ".".to_string(),
        (false, false) => joined,
    }
}

/// Python's `_coerce` for the words of `--set section.option=value` (used for every field that is not text).
pub fn coerce(text: &str) -> Value {
    let low = casefold(strip(text));
    if matches!(low.as_str(), "true" | "yes" | "on") {
        return Value::Bool(true);
    }
    if matches!(low.as_str(), "false" | "no" | "off") {
        return Value::Bool(false);
    }
    if let Some(i) = super::pynum::py_int(text) {
        return match i {
            PyInt::Small(v) => Value::from(v),
            PyInt::Huge(x) => float_value(x),
        };
    }
    if let Some(x) = py_float(text) {
        return float_value_any(x);
    }
    if strip(text).starts_with('[') {
        if let Ok(list) = serde_json::from_str::<Value>(text) {
            return list;
        }
    }
    Value::String(text.to_string())
}

/// JSON cannot hold an infinite or NaN float (`--set slides.line_width_units=inf` is one in Python), so one is kept as
/// the object `{"\u0000float": "inf"}`; every check below treats it as a number and nothing else.
const NONFINITE_KEY: &str = "\u{0}float";

fn float_value_any(x: f64) -> Value {
    if x.is_finite() {
        return float_value(x);
    }
    let word = if x.is_nan() {
        "nan"
    } else if x > 0.0 {
        "inf"
    } else {
        "-inf"
    };
    let mut marker = Map::new();
    marker.insert(NONFINITE_KEY.to_string(), Value::String(word.to_string()));
    Value::Object(marker)
}

fn nonfinite(value: &Value) -> Option<f64> {
    match value.as_object()?.get(NONFINITE_KEY)?.as_str()? {
        "nan" => Some(f64::NAN),
        "inf" => Some(f64::INFINITY),
        "-inf" => Some(f64::NEG_INFINITY),
        _ => None,
    }
}

/// `value` as a dictionary, unless it is the marker of a non-finite float.
pub fn obj(value: &Value) -> Option<&Map<String, Value>> {
    match value.as_object() {
        Some(map) if map.contains_key(NONFINITE_KEY) && nonfinite(value).is_some() => None,
        other => other,
    }
}
