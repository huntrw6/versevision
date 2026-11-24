//! `with_options`: a named profile and `section.option=value` overrides applied to a config for one run.

use serde_json::{Map, Value};
use thiserror::Error;

use super::validate::{coerce, obj, ValidationError};
use super::AppConfig;
use crate::pystr::strip;

/// Everything that can go wrong with the config. The texts of the `with_options` errors are Python's, word for word.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum ConfigError {
    /// `--set` was not `section.option=value`.
    #[error("Use section.option=value, for example notes.line_breaks=paragraph (got '{0}')")]
    BadSet(String),
    /// `--set a.b=v` where `a` is not a section (the text is the key as written).
    #[error("Unknown setting section '{0}'")]
    UnknownSection(String),
    /// `--set a.b=v` where `b` is not an option of `a`.
    #[error("Unknown setting '{0}'")]
    UnknownSetting(String),
    #[error("No profile named '{profile}' ({known}). Add [profiles.{profile}.notes] etc. to the config.")]
    UnknownProfile { profile: String, known: String },
    /// The result of the overrides is not a valid config (the first error).
    #[error("Invalid option: {} at {}", .0.msg, .0.location())]
    Invalid(ValidationError),
    #[error("{0}")]
    Toml(String),
}

/// Python's `_merge`: `extra` into `base`, tables merged key by key, everything else replaced.
fn merge(base: &mut Map<String, Value>, extra: &Map<String, Value>) {
    for (key, value) in extra {
        match (value, base.get_mut(key)) {
            (Value::Object(inner), Some(Value::Object(existing))) => merge(existing, inner),
            _ => {
                base.insert(key.clone(), value.clone());
            }
        }
    }
}

/// The config with a named profile and/or `section.option=value` overrides applied (for one run; nothing is saved).
///
/// Same rules as Python: the profile is merged first; then each `--set` walks the dumped config (`Unknown setting
/// section` / `Unknown setting` when the path is not there), a text option takes the text as it is and every other
/// option takes `_coerce(text)` (true/yes/on, false/no/off, int, float, a JSON list, else text); the result is validated
/// as a whole and only the first error is reported.
pub fn with_options(
    config: &AppConfig,
    profile: Option<&str>,
    sets: &[&str],
) -> Result<AppConfig, ConfigError> {
    let mut data = match config.to_value() {
        Value::Object(map) => map,
        _ => Map::new(),
    };
    if let Some(name) = profile.filter(|p| !p.is_empty()) {
        let Some(overrides) = config.profiles.get(name) else {
            let mut names: Vec<&str> = config.profiles.keys().map(String::as_str).collect();
            names.sort_unstable();
            let known = if names.is_empty() {
                "none defined".to_string()
            } else {
                names.join(", ")
            };
            return Err(ConfigError::UnknownProfile {
                profile: name.to_string(),
                known,
            });
        };
        if let Value::Object(extra) = overrides {
            merge(&mut data, extra);
        }
    }
    for item in sets {
        let (key, value) = match item.split_once('=') {
            Some((key, value)) => (key, value),
            None => return Err(ConfigError::BadSet((*item).to_string())),
        };
        let parts: Vec<&str> = strip(key).split('.').collect();
        if parts.len() < 2 {
            return Err(ConfigError::BadSet((*item).to_string()));
        }
        let (last, path) = parts.split_last().unwrap_or((&"", &[]));
        let mut node = &mut data;
        for part in path {
            match node.get_mut(*part) {
                Some(next) if obj(next).is_some() => {
                    node = match next.as_object_mut() {
                        Some(map) => map,
                        None => return Err(ConfigError::UnknownSection(key.to_string())),
                    };
                }
                _ => return Err(ConfigError::UnknownSection(key.to_string())),
            }
        }
        let Some(slot) = node.get_mut(*last) else {
            return Err(ConfigError::UnknownSetting(key.to_string()));
        };
        *slot = if slot.is_string() {
            Value::String(value.to_string())
        } else {
            coerce(value)
        };
    }
    AppConfig::from_value(&Value::Object(data)).map_err(ConfigError::Invalid)
}

/// The fields that differ between two dumped configs, as `{"a.b": new_value}` plus the paths that are gone.
/// Tables are flattened to their leaves (a list or a scalar is one leaf); used by the oracle files.
pub fn changed_fields(before: &Value, after: &Value) -> (Map<String, Value>, Vec<String>) {
    fn leaves(prefix: &str, value: &Value, out: &mut Vec<(String, Value)>) {
        match value {
            Value::Object(map) if !map.is_empty() => {
                for (key, inner) in map {
                    let path = if prefix.is_empty() {
                        key.clone()
                    } else {
                        format!("{prefix}.{key}")
                    };
                    leaves(&path, inner, out);
                }
            }
            other => out.push((prefix.to_string(), other.clone())),
        }
    }
    let (mut old, mut new) = (Vec::new(), Vec::new());
    leaves("", before, &mut old);
    leaves("", after, &mut new);
    let old: std::collections::BTreeMap<String, Value> = old.into_iter().collect();
    let new: std::collections::BTreeMap<String, Value> = new.into_iter().collect();
    let mut changed = Map::new();
    for (path, value) in &new {
        if old.get(path) != Some(value) {
            changed.insert(path.clone(), value.clone());
        }
    }
    let removed = old
        .keys()
        .filter(|p| !new.contains_key(*p))
        .cloned()
        .collect();
    (changed, removed)
}
