//! API keys: where they are kept and how a provider gets one.
//!
//! The host app injects the place ([`FileKeyStore::new`] takes the path of its `keys.env`). The file is Python's: lines of
//! `NAME=value`, `#` comments, blank lines, a value may be wrapped in quotes (`credentials._from_keys_file`). An environment
//! variable of the same name wins over the file (`provider_api_key`). Keys are write-only to a user interface: it can
//! [`KeyStore::set_key`], ask [`KeyStore::has_key`] and [`KeyStore::remove_key`], and nothing in this crate hands a key back for
//! display. Only a provider, when it builds a request, calls [`KeyStore::resolve`].
//!
//! The file is written only with owner-only access: mode `0600` on Unix (and `0700` for a directory this code creates), a private
//! per-user ACL on Windows (`icacls`: inheritance removed, a full-control entry for the current user; the system and administrator
//! accounts that Windows gives every new file of this user stay, and can take the file anyway). The new content is written to a private
//! temporary file in the same directory first and renamed over the old one, so there is no moment when the keys are readable by
//! others and no half-written file.

use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use crate::secret::Secret;

/// Why a key could not be stored or removed. The text never holds a key.
#[derive(Debug, thiserror::Error)]
pub enum KeyError {
    #[error(
        "not a valid key name (letters, digits and underscores, not starting with a digit): {0:?}"
    )]
    InvalidName(String),
    #[error("a key must be one line, not empty, with no spaces or quote marks at either end")]
    InvalidValue,
    #[error("the keys file {path}: {kind}")]
    Io {
        path: PathBuf,
        kind: std::io::ErrorKind,
    },
    #[error("could not make the keys file private to this user ({0})")]
    Private(String),
}

/// A place keys are kept. Implementations never print a key, in `Debug` or anywhere else.
pub trait KeyStore: fmt::Debug + Send + Sync {
    /// Store `value` under `name` (replacing a key of that name). It is write-only: nothing reads it back for display.
    fn set_key(&self, name: &str, value: Secret) -> Result<(), KeyError>;
    /// Is there a key called `name` (in the environment or the store)?
    fn has_key(&self, name: &str) -> bool;
    /// Forget the stored key `name`. Returns whether there was one. An environment variable is not touched.
    fn remove_key(&self, name: &str) -> Result<bool, KeyError>;
    /// The key itself, for a provider to put in a request header. **Not for display**; a user interface must not call this.
    fn resolve(&self, name: &str) -> Option<Secret>;
}

pub(crate) fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        && name.len() <= 128
}

fn valid_value(value: &str) -> bool {
    let edge = |c: char| c == '"' || c == '\'' || c.is_whitespace();
    !value.is_empty()
        && !value.contains(['\n', '\r', '\0'])
        && !value.starts_with(edge)
        && !value.ends_with(edge)
}

/// `os.getenv(name)`: set and not empty.
fn from_environment(name: &str) -> Option<Secret> {
    match std::env::var(name) {
        Ok(value) if !value.is_empty() => Some(Secret::new(value)),
        _ => None,
    }
}

/// `credentials._from_keys_file`: the value of `name` in the text of a keys file (the first line that has it).
fn parse_value(content: &str, name: &str) -> Option<String> {
    for line in versevision_core::pystr::splitlines(content) {
        let line = versevision_core::pystr::strip(line);
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if versevision_core::pystr::strip(key) == name {
            let value = versevision_core::pystr::strip(value).trim_matches(['"', '\'']);
            return if value.is_empty() {
                None
            } else {
                Some(value.to_string())
            };
        }
    }
    None
}

/// The line's key, when it is a `NAME=value` line.
fn line_name(line: &str) -> Option<&str> {
    let line = versevision_core::pystr::strip(line);
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    line.split_once('=')
        .map(|(key, _)| versevision_core::pystr::strip(key))
}

/// Keys in a `keys.env` file, with an environment variable of the same name taking precedence.
pub struct FileKeyStore {
    path: PathBuf,
    /// One writer at a time inside this process (two processes writing at once are not coordinated).
    write: Mutex<()>,
}

impl fmt::Debug for FileKeyStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FileKeyStore")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

impl FileKeyStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            write: Mutex::new(()),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn io(&self, error: &std::io::Error) -> KeyError {
        KeyError::Io {
            path: self.path.clone(),
            kind: error.kind(),
        }
    }

    fn read(&self) -> Result<String, KeyError> {
        match fs::read(&self.path) {
            Ok(bytes) => Ok(String::from_utf8_lossy(&bytes)
                .trim_start_matches('\u{feff}')
                .to_string()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
            Err(e) => Err(self.io(&e)),
        }
    }

    fn read_value(&self, name: &str) -> Option<Secret> {
        let content = fs::read(&self.path).ok()?;
        parse_value(&String::from_utf8_lossy(&content), name).map(Secret::new)
    }

    /// Replace the whole file with `content`, owner-only from the first byte.
    fn write_private(&self, content: &str) -> Result<(), KeyError> {
        let dir = self
            .path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        if !dir.exists() {
            create_private_dir(dir).map_err(|e| self.io(&e))?;
        }
        let temp = dir.join(format!(
            ".keys-{}-{}.tmp",
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let result = (|| {
            let mut file = create_private_file(&temp).map_err(|e| self.io(&e))?;
            restrict_to_user(&temp)?;
            file.write_all(content.as_bytes())
                .and_then(|()| file.sync_all())
                .map_err(|e| self.io(&e))?;
            drop(file);
            fs::rename(&temp, &self.path).map_err(|e| self.io(&e))
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temp);
        }
        result
    }
}

impl KeyStore for FileKeyStore {
    fn set_key(&self, name: &str, value: Secret) -> Result<(), KeyError> {
        if !valid_name(name) {
            return Err(KeyError::InvalidName(name.to_string()));
        }
        if !valid_value(value.expose()) {
            return Err(KeyError::InvalidValue);
        }
        let _guard = self.write.lock().unwrap_or_else(|e| e.into_inner());
        let existing = self.read()?;
        let mut out = String::new();
        let mut written = false;
        for line in versevision_core::pystr::splitlines(&existing) {
            if line_name(line) == Some(name) {
                if !written {
                    out.push_str(&format!("{name}={}\n", value.expose()));
                    written = true;
                }
                continue;
            }
            out.push_str(line);
            out.push('\n');
        }
        if !written {
            out.push_str(&format!("{name}={}\n", value.expose()));
        }
        self.write_private(&out)
    }

    fn has_key(&self, name: &str) -> bool {
        self.resolve(name).is_some()
    }

    fn remove_key(&self, name: &str) -> Result<bool, KeyError> {
        if !valid_name(name) {
            return Err(KeyError::InvalidName(name.to_string()));
        }
        let _guard = self.write.lock().unwrap_or_else(|e| e.into_inner());
        let existing = self.read()?;
        let mut removed = false;
        let mut out = String::new();
        for line in versevision_core::pystr::splitlines(&existing) {
            if line_name(line) == Some(name) {
                removed = true;
                continue;
            }
            out.push_str(line);
            out.push('\n');
        }
        if removed {
            self.write_private(&out)?;
        }
        Ok(removed)
    }

    fn resolve(&self, name: &str) -> Option<Secret> {
        from_environment(name).or_else(|| self.read_value(name))
    }
}

/// Keys held in memory (tests, and hosts that keep keys elsewhere). The environment override applies here too.
#[derive(Default)]
pub struct MemoryKeyStore {
    keys: Mutex<HashMap<String, Secret>>,
}

impl MemoryKeyStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl fmt::Debug for MemoryKeyStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MemoryKeyStore").finish_non_exhaustive()
    }
}

impl KeyStore for MemoryKeyStore {
    fn set_key(&self, name: &str, value: Secret) -> Result<(), KeyError> {
        if !valid_name(name) {
            return Err(KeyError::InvalidName(name.to_string()));
        }
        if !valid_value(value.expose()) {
            return Err(KeyError::InvalidValue);
        }
        self.keys
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(name.to_string(), value);
        Ok(())
    }

    fn has_key(&self, name: &str) -> bool {
        self.resolve(name).is_some()
    }

    fn remove_key(&self, name: &str) -> Result<bool, KeyError> {
        Ok(self
            .keys
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(name)
            .is_some())
    }

    fn resolve(&self, name: &str) -> Option<Secret> {
        from_environment(name).or_else(|| {
            self.keys
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(name)
                .cloned()
        })
    }
}

#[cfg(unix)]
fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
}

#[cfg(not(unix))]
fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dir)
}

#[cfg(unix)]
fn create_private_file(path: &Path) -> std::io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}

#[cfg(not(unix))]
fn create_private_file(path: &Path) -> std::io::Result<fs::File> {
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
}

#[cfg(unix)]
fn restrict_to_user(_path: &Path) -> Result<(), KeyError> {
    Ok(())
}

/// Windows: remove the inherited permissions and grant the current user full control (`icacls`, part of every Windows install).
#[cfg(windows)]
fn restrict_to_user(path: &Path) -> Result<(), KeyError> {
    use std::os::windows::process::CommandExt;
    let user = std::env::var("USERNAME").map_err(|_| KeyError::Private("no USERNAME".into()))?;
    let account = match std::env::var("USERDOMAIN") {
        Ok(domain) if !domain.is_empty() => format!("{domain}\\{user}"),
        _ => user,
    };
    let output = std::process::Command::new("icacls")
        .arg(path)
        .args(["/inheritance:r", "/grant:r"])
        .arg(format!("{account}:(F)"))
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .output()
        .map_err(|e| KeyError::Private(format!("icacls: {}", e.kind())))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(KeyError::Private(format!(
            "icacls exit {}",
            output.status.code().unwrap_or(-1)
        )))
    }
}

#[cfg(not(any(unix, windows)))]
fn restrict_to_user(_path: &Path) -> Result<(), KeyError> {
    Ok(())
}
