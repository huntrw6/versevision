//! The "Bible keys" section of the Config page: write-only fields for the API.Bible, YouVersion and ESV keys, a Test button for each and
//! `bible.source`. A key goes in through [`save`] and into the key store ([`versevision_bible::KeyStore`], an owner-only file in the real
//! one); nothing here ever hands one back. The page only learns `set` or `not set`. The test makes one request through the matching
//! provider (its own health check: a list of translations for API.Bible and YouVersion, one verse for the ESV, thrown away) with the key the
//! person typed or the stored one, and answers `ok`, `rejected` or `unreachable` in words that never hold the key or any verse text.

use std::sync::Arc;

use serde_json::{json, Value};
use versevision_bible::{
    ApiBibleProvider, EsvProvider, KeyError, KeyStore, Secret, YouVersionProvider,
};
use versevision_core::bibles::BibleProvider;
use versevision_core::config::{AppConfig, BibleSource, ProviderConfig};

use crate::state::WebState;

struct Kind {
    id: &'static str,
    label: &'static str,
    /// The `type` word of `[bible.providers.NAME]`.
    provider_type: &'static str,
    /// The key's name when no provider of this type is configured (the schema's default).
    default_env: &'static str,
    help: &'static str,
}

const KINDS: [Kind; 3] = [
    Kind {
        id: "api_bible",
        label: "API.Bible",
        provider_type: "api_bible",
        default_env: "API_BIBLE_API_KEY",
        help: "Many translations (scripture.api.bible)",
    },
    Kind {
        id: "youversion",
        label: "YouVersion",
        provider_type: "youversion",
        default_env: "YOUVERSION_API_KEY",
        help: "YouVersion's licensed translations",
    },
    Kind {
        id: "esv",
        label: "ESV",
        provider_type: "esv",
        default_env: "ESV_API_KEY",
        help: "The ESV API; its text is never stored",
    },
];

const SOURCES: [(&str, &str, &str); 3] = [
    ("auto", "Automatic", "My own key if saved, else StagePilot"),
    (
        "stagepilot",
        "StagePilot service",
        "Only the StagePilot service",
    ),
    (
        "own_keys",
        "My own keys",
        "Only my own keys and local files",
    ),
];

fn kind(id: &str) -> Option<&'static Kind> {
    KINDS.iter().find(|k| k.id == id)
}

/// The configured provider of this kind (the first one in the file), or the schema's own defaults for a kind.
fn provider_config(kind: &Kind, config: &AppConfig) -> ProviderConfig {
    let wanted = |p: &&ProviderConfig| {
        matches!(
            (kind.provider_type, p),
            ("api_bible", ProviderConfig::ApiBible(_))
                | ("youversion", ProviderConfig::YouVersion(_))
                | ("esv", ProviderConfig::Esv(_))
        )
    };
    if let Some(found) = config.bible.providers.values().find(wanted) {
        return found.clone();
    }
    let input = json!({"bible": {"providers": {"test": {"type": kind.provider_type}}}});
    AppConfig::from_value(&input)
        .ok()
        .and_then(|c| c.bible.providers.get("test").cloned())
        .expect("the schema has a default for every provider type")
}

fn key_name(kind: &Kind, config: &AppConfig) -> String {
    match provider_config(kind, config) {
        ProviderConfig::ApiBible(c) | ProviderConfig::YouVersion(c) => c.api_key_env,
        ProviderConfig::Esv(c) => c.api_key_env,
        ProviderConfig::Usfm(_) => kind.default_env.to_string(),
    }
}

/// The section as the page draws it. Never holds a key.
pub fn describe(state: &WebState) -> Value {
    let config = state.config();
    let keys: Vec<Value> = KINDS
        .iter()
        .map(|k| {
            let env = key_name(k, &config);
            json!({
                "id": k.id, "label": k.label, "env": env,
                "set": state.inner.keys.has_key(&env), "help": k.help,
            })
        })
        .collect();
    let available = state.inner.auth.as_ref().is_some_and(|auth| {
        auth.installation_id().is_some_and(|id| !id.is_empty())
            && auth.credential().is_some_and(|c| !c.is_empty())
    });
    let current = match config.bible.source {
        BibleSource::Auto => "auto",
        BibleSource::StagePilot => "stagepilot",
        BibleSource::OwnKeys => "own_keys",
    };
    json!({
        "source": current,
        "sources": SOURCES.iter().map(|(id, label, help)| json!({"id": id, "label": label, "help": help})).collect::<Vec<_>>(),
        "service": {
            "available": available,
            "help": if available {
                "This computer is enrolled in StagePilot, so its Bible service can supply verse text."
            } else {
                "Not available: this computer is not enrolled in StagePilot (or VerseVision runs on its own)."
            },
        },
        "keys": keys,
    })
}

fn describe_key_error(error: &KeyError) -> String {
    // the messages of `KeyError` never hold a key
    error.to_string()
}

/// Save (or replace) the key of `id`.
pub fn save(state: &WebState, id: &str, key: &str) -> Result<Value, String> {
    let kind = kind(id).ok_or_else(|| "Unknown Bible provider".to_string())?;
    let env = key_name(kind, &state.config());
    state
        .inner
        .keys
        .set_key(&env, Secret::new(key.trim()))
        .map_err(|e| describe_key_error(&e))?;
    Ok(describe(state))
}

/// Forget the stored key of `id` (an environment variable of the same name is not ours to remove).
pub fn remove(state: &WebState, id: &str) -> Result<Value, String> {
    let kind = kind(id).ok_or_else(|| "Unknown Bible provider".to_string())?;
    let env = key_name(kind, &state.config());
    state
        .inner
        .keys
        .remove_key(&env)
        .map_err(|e| describe_key_error(&e))?;
    Ok(describe(state))
}

/// Only the key that was typed, for one name: unlike the stored keys it ignores the environment, so the test is of what was typed.
struct Supplied {
    name: String,
    value: Secret,
}

impl std::fmt::Debug for Supplied {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Supplied")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

impl KeyStore for Supplied {
    fn set_key(&self, _name: &str, _value: Secret) -> Result<(), KeyError> {
        Err(KeyError::InvalidValue)
    }
    fn has_key(&self, name: &str) -> bool {
        name == self.name
    }
    fn remove_key(&self, _name: &str) -> Result<bool, KeyError> {
        Ok(false)
    }
    fn resolve(&self, name: &str) -> Option<Secret> {
        (name == self.name).then(|| Secret::new(self.value.expose()))
    }
}

/// One tiny request through the provider of `id`, with the `supplied` key or else the stored one.
/// `{"ok", "status": "ok" | "rejected" | "unreachable" | "not_set", "message"}`.
pub async fn test(state: &WebState, id: &str, supplied: Option<&str>) -> Result<Value, String> {
    let kind = kind(id).ok_or_else(|| "Unknown Bible provider".to_string())?;
    let config = state.config();
    let env = key_name(kind, &config);
    let supplied = supplied.map(str::trim).filter(|k| !k.is_empty());
    let keys: Arc<dyn KeyStore> = match supplied {
        Some(key) => {
            let one = Supplied {
                name: env.clone(),
                value: Secret::new(key),
            };
            // the same rules as saving: a key with a space or a quote mark at an end is refused before any request
            if let Err(error) = check_shape(key) {
                return Ok(answer("rejected", &error, &[]));
            }
            Arc::new(one)
        }
        None => state.inner.keys.clone(),
    };
    let Some(secret) = keys.resolve(&env) else {
        return Ok(answer(
            "not_set",
            &format!("No {} key is saved yet.", kind.label),
            &[],
        ));
    };
    let health = match provider_config(kind, &config) {
        ProviderConfig::ApiBible(c) => {
            ApiBibleProvider::new("test", c, keys.clone())
                .health_check()
                .await
        }
        ProviderConfig::YouVersion(c) => {
            YouVersionProvider::new("test", c, keys.clone())
                .health_check()
                .await
        }
        ProviderConfig::Esv(c) => {
            EsvProvider::new("test", c, keys.clone())
                .health_check()
                .await
        }
        ProviderConfig::Usfm(_) => return Err("Unknown Bible provider".into()),
    };
    if health.healthy {
        return Ok(answer("ok", &secret.redact(&health.message), &[]));
    }
    let message = secret.redact(&health.message);
    let status = if message.contains("invalid API key") {
        "rejected"
    } else {
        "unreachable"
    };
    Ok(answer(status, &message, &[]))
}

fn answer(status: &str, message: &str, _extra: &[(&str, Value)]) -> Value {
    json!({"ok": status == "ok", "status": status, "message": message})
}

/// The store's own rule for a key (one line, no spaces or quote marks at either end), checked without storing it.
pub(crate) fn check_shape(key: &str) -> Result<(), String> {
    let edge = |c: char| c == '"' || c == '\'' || c.is_whitespace();
    if key.is_empty()
        || key.contains(['\n', '\r', '\0'])
        || key.starts_with(edge)
        || key.ends_with(edge)
    {
        Err(KeyError::InvalidValue.to_string())
    } else {
        Ok(())
    }
}

/// Change `bible.source` (and save the config file), as the radio buttons do.
pub fn set_source(state: &WebState, value: &str) -> Result<Value, String> {
    let control = state
        .inner
        .control
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if control.busy {
        return Err("An import is running; change settings when it finishes".into());
    }
    let mut config = state
        .inner
        .config
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut data = config.to_value();
    if let Some(Value::Object(bible)) = data.get_mut("bible") {
        bible.insert("source".into(), json!(value));
    }
    let new = AppConfig::from_value(&data).map_err(|e| format!("{}: {}", e.location(), e.msg))?;
    crate::save_config(&state.inner.dirs, &new)?;
    *config = new;
    drop(config);
    drop(control);
    Ok(describe(state))
}
