//! `node/transport.py`, the LOCAL part: where ProPresenter runs (this computer) and the file moves that are safe to make: every move is
//! checked by SHA-256 and refuses to overwrite a file that changed since it was read.
//!
//! **Not ported (S6b scope): `HermesRemoteTransport`** (ProPresenter on another Mac through `hermes-remote`) and `make_transport`'s
//! choice of it. A remote-node install stays on the Python app until the cutover decides what replaces it.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::Value;
use sha2::{Digest, Sha256};
use versevision_core::config::{NodeConfig, ProPresenterConfig};

use crate::backup::{create_backup, restore_backup};
use crate::clock::Clock;
use crate::errors::{Kind, VvError, VvResult};
use crate::http::HttpApi;
use crate::model::read_presentation;
use crate::process::ProcessProbe;

/// `sha256_file(path)`.
pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut file = fs::File::open(path)?;
    let mut digest = Sha256::new();
    let mut block = vec![0u8; 1 << 20];
    loop {
        let n = std::io::Read::read(&mut file, &mut block)?;
        if n == 0 {
            break;
        }
        digest.update(&block[..n]);
    }
    Ok(digest
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

fn sha(path: &Path) -> VvResult<String> {
    sha256_file(path).map_err(VvError::from)
}

/// What the pipeline needs from the computer that runs ProPresenter (the `Transport` protocol of Python).
pub trait Transport {
    fn description(&self) -> String;
    fn api_get(&self, route: &str) -> VvResult<Value>;
    fn api_put(&self, route: &str, body: &Value) -> VvResult<()>;
    fn is_running(&self) -> bool;
    fn quit_app(&self) -> VvResult<()>;
    fn launch_app(&self) -> VvResult<()>;
    fn fetch_file(&self, remote_path: &str, local_path: &Path) -> VvResult<String>;
    fn install_file(
        &self,
        local_path: &Path,
        remote_path: &str,
        expected_remote_sha: &str,
    ) -> VvResult<String>;
    fn find_presentation(&self, name: &str, library: Option<&str>) -> Vec<String>;
    fn library_folder(&self, library: &str) -> String;
    fn fetch_themes(&self, dest: &Path) -> VvResult<()>;
    fn create_file(&self, local_path: &Path, remote_path: &str) -> VvResult<String>;
    fn delete_created_file(&self, remote_path: &str, expected_cues: usize) -> VvResult<String>;
    fn rename_file(&self, old_path: &str, new_path: &str) -> VvResult<()>;
    fn delete_file(&self, remote_path: &str, expected_sha: &str) -> VvResult<String>;
    fn backup_file(&self, remote_path: &str) -> VvResult<String>;
    fn restore_backup(&self, backup_path: &str, remote_path: &str) -> VvResult<()>;
}

/// `os.path.expandvars` (`$NAME`, `${NAME}`; on Windows also `%NAME%`) followed by `expanduser`. A name that is not set stays as written.
/// `HOME` falls back to the host-supplied home folder (Windows has no `HOME`: **unverified on Windows** whether a user's own config
/// keeps `$HOME` there; the defaults below are what Python's defaults mean on a Mac).
pub fn expand_path(text: &str, home: &Path, lookup: &dyn Fn(&str) -> Option<String>) -> PathBuf {
    let var = |name: &str| -> Option<String> {
        lookup(name).or_else(|| {
            if name == "HOME" && !home.as_os_str().is_empty() {
                Some(home.to_string_lossy().into_owned())
            } else {
                None
            }
        })
    };
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    let is_name = |c: char| c.is_ascii_alphanumeric() || c == '_';
    while i < chars.len() {
        let c = chars[i];
        if c == '$' && i + 1 < chars.len() {
            if chars[i + 1] == '{' {
                if let Some(end) = chars[i + 2..].iter().position(|&x| x == '}') {
                    let name: String = chars[i + 2..i + 2 + end].iter().collect();
                    match var(&name) {
                        Some(value) => out.push_str(&value),
                        None => out.extend(&chars[i..i + 3 + end]),
                    }
                    i += 3 + end;
                    continue;
                }
            } else if is_name(chars[i + 1]) {
                let mut j = i + 1;
                while j < chars.len() && is_name(chars[j]) {
                    j += 1;
                }
                let name: String = chars[i + 1..j].iter().collect();
                match var(&name) {
                    Some(value) => out.push_str(&value),
                    None => out.extend(&chars[i..j]),
                }
                i = j;
                continue;
            }
        }
        if cfg!(windows) && c == '%' {
            if let Some(end) = chars[i + 1..].iter().position(|&x| x == '%') {
                let name: String = chars[i + 1..i + 1 + end].iter().collect();
                if !name.is_empty() {
                    if let Some(value) = var(&name) {
                        out.push_str(&value);
                        i += 2 + end;
                        continue;
                    }
                }
            }
        }
        out.push(c);
        i += 1;
    }
    if out == "~" {
        return home.to_path_buf();
    }
    if let Some(rest) = out.strip_prefix("~/").or_else(|| out.strip_prefix("~\\")) {
        return home.join(rest);
    }
    PathBuf::from(out)
}

/// `shutil.copy2`: the bytes and the modification time.
pub fn copy2(from: &Path, to: &Path) -> std::io::Result<()> {
    fs::copy(from, to)?;
    if let Ok(modified) = fs::metadata(from).and_then(|m| m.modified()) {
        if let Ok(file) = fs::OpenOptions::new().write(true).open(to) {
            let _ = file.set_modified(modified);
        }
    }
    Ok(())
}

fn temp_beside(target: &Path) -> VvResult<tempfile::NamedTempFile> {
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let dir = target.parent().unwrap_or(Path::new("."));
    tempfile::Builder::new()
        .prefix(&format!(".{name}.versevision-"))
        .suffix(".tmp")
        .tempfile_in(dir)
        .map_err(VvError::from)
}

fn write_temp(bytes: &[u8], target: &Path) -> VvResult<tempfile::NamedTempFile> {
    let mut temp = temp_beside(target)?;
    temp.write_all(bytes)?;
    temp.flush()?;
    temp.as_file().sync_all()?;
    Ok(temp)
}

/// Looks an environment variable up (`os.environ.get`).
pub type EnvLookup = Box<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// ProPresenter runs on this computer.
pub struct LocalTransport {
    pp: ProPresenterConfig,
    node: NodeConfig,
    api: HttpApi,
    clock: Arc<dyn Clock>,
    probe: Arc<dyn ProcessProbe>,
    home: PathBuf,
    env: EnvLookup,
}

impl LocalTransport {
    /// `home` is the user's home folder (for `~` and `$HOME`), supplied by the host.
    pub fn new(
        pp: &ProPresenterConfig,
        node: &NodeConfig,
        clock: Arc<dyn Clock>,
        probe: Arc<dyn ProcessProbe>,
        home: PathBuf,
    ) -> LocalTransport {
        LocalTransport {
            api: HttpApi::new(&pp.host, pp.port, pp.timeout_seconds),
            pp: pp.clone(),
            node: node.clone(),
            clock,
            probe,
            home,
            env: Box::new(|name| std::env::var(name).ok()),
        }
    }

    /// Replace the environment lookup (tests).
    pub fn with_env(
        mut self,
        env: impl Fn(&str) -> Option<String> + Send + Sync + 'static,
    ) -> Self {
        self.env = Box::new(env);
        self
    }

    pub fn api(&self) -> &HttpApi {
        &self.api
    }

    fn expand(&self, text: &str) -> PathBuf {
        expand_path(text, &self.home, &*self.env)
    }

    /// `/bin/sh -c command`, stdout and stderr together.
    fn sh(command: &str, timeout: Duration) -> String {
        let child = Command::new("/bin/sh")
            .arg("-c")
            .arg(command)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();
        let Ok(mut child) = child else {
            return String::new();
        };
        let started = Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if started.elapsed() < timeout => {
                    std::thread::sleep(Duration::from_millis(20))
                }
                _ => {
                    let _ = child.kill();
                    break;
                }
            }
        }
        match child.wait_with_output() {
            Ok(out) => format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            ),
            Err(_) => String::new(),
        }
    }

    fn app_name(&self) -> String {
        Path::new(&self.node.app_path)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    /// The `vvdialog` helper (it answers ProPresenter's "Are you sure you want to quit?" box); compiled once with `swiftc` (macOS only).
    fn ensure_helper(&self) -> Option<String> {
        let helper_dir = self.expand(&self.node.helper_dir);
        let helper = helper_dir.join("vvdialog");
        if is_executable(&helper) {
            return Some(shell_quote(&helper.to_string_lossy()));
        }
        if !cfg!(target_os = "macos") {
            return None;
        }
        fs::create_dir_all(&helper_dir).ok()?;
        let source = helper_dir.join("vvdialog.swift");
        fs::write(&source, include_str!("../assets/vvdialog.swift")).ok()?;
        Self::sh(
            &format!(
                "swiftc -O -o {} {}",
                shell_quote(&helper.to_string_lossy()),
                shell_quote(&source.to_string_lossy())
            ),
            Duration::from_secs(240),
        );
        let _ = fs::remove_file(&source);
        is_executable(&helper).then(|| shell_quote(&helper.to_string_lossy()))
    }
}

fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path)
            .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

/// `shlex.quote`.
pub fn shell_quote(value: &str) -> String {
    if !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "@%+=:,./-_".contains(c))
    {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

impl Transport for LocalTransport {
    fn description(&self) -> String {
        format!("this computer ({}:{})", self.pp.host, self.pp.port)
    }

    fn api_get(&self, route: &str) -> VvResult<Value> {
        self.api.api_get(route)
    }

    fn api_put(&self, route: &str, body: &Value) -> VvResult<()> {
        self.api.api_put(route, body)
    }

    fn is_running(&self) -> bool {
        self.probe.is_running()
    }

    /// Close ProPresenter. Its "Are you sure you want to quit?" box is answered; "Save changes?" never is. **macOS only** (as in Python).
    fn quit_app(&self) -> VvResult<()> {
        if !cfg!(target_os = "macos") {
            return Err(VvError::configuration(
                "Automatic quit is only available on macOS; close ProPresenter yourself",
            ));
        }
        let app = shell_quote(&self.app_name());
        let _ = Command::new("osascript")
            .arg("-e")
            .arg(format!("tell application \"{}\" to quit", self.app_name()))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        if !self.node.confirm_quit {
            return Ok(());
        }
        let Some(helper) = self.ensure_helper() else {
            return Ok(());
        };
        let question = shell_quote("Are you sure you want to quit");
        let name = shell_quote(&self.node.process_name);
        for _ in 0..15 {
            let out = Self::sh(
                &format!("{helper} {name} {question} OK"),
                Duration::from_secs(60),
            );
            if out.contains("PRESSED") || out.contains("NOT_RUNNING") {
                return Ok(());
            }
            self.clock.sleep(1.0);
        }
        // a scripted quit can be cancelled by the app ("User canceled"); a real Cmd-Q goes through the menu path
        Self::sh(
            &format!(
                "osascript -e 'tell application {app} to activate'; sleep 1; \
                 (osascript -e 'tell application \"System Events\" to keystroke \"q\" using command down' >/dev/null 2>&1 &); \
                 for i in 1 2 3 4 5 6 7 8 9 10; do r=$({helper} {name} {question} OK); case \"$r\" in PRESSED|NOT_RUNNING) break;; esac; sleep 0.5; done"
            ),
            Duration::from_secs(60),
        );
        Ok(())
    }

    fn launch_app(&self) -> VvResult<()> {
        if !cfg!(target_os = "macos") {
            return Err(VvError::configuration(
                "Automatic launch is only available on macOS; reopen ProPresenter yourself",
            ));
        }
        let _ = Command::new("open").arg(&self.node.app_path).status();
        Ok(())
    }

    fn fetch_file(&self, remote_path: &str, local_path: &Path) -> VvResult<String> {
        if let Some(parent) = local_path.parent() {
            fs::create_dir_all(parent)?;
        }
        copy2(Path::new(remote_path), local_path)?;
        sha(local_path)
    }

    fn install_file(
        &self,
        local_path: &Path,
        remote_path: &str,
        expected_remote_sha: &str,
    ) -> VvResult<String> {
        let target = Path::new(remote_path);
        if sha(target)? != expected_remote_sha {
            return Err(VvError::write(
                "The presentation changed after VerseVision read it; no files changed",
            ));
        }
        let temp = write_temp(&fs::read(local_path)?, target)?;
        temp.persist(target).map_err(|e| VvError::from(e.error))?;
        sha(target)
    }

    fn library_folder(&self, library: &str) -> String {
        self.expand(&self.node.library_root)
            .join(library)
            .to_string_lossy()
            .into_owned()
    }

    /// Copy every ProPresenter theme file (not the media) into `dest`, keeping the folder layout.
    fn fetch_themes(&self, dest: &Path) -> VvResult<()> {
        let root = self.expand(&self.node.theme_root);
        if !root.is_dir() {
            return Err(VvError::connection(format!(
                "No Themes folder at {}",
                root.display()
            )));
        }
        fn walk(root: &Path, dir: &Path, dest: &Path) -> VvResult<()> {
            let mut entries: Vec<_> = fs::read_dir(dir)?.filter_map(Result::ok).collect();
            entries.sort_by_key(|e| e.file_name());
            for entry in entries {
                let path = entry.path();
                if path.is_dir() {
                    walk(root, &path, dest)?;
                } else if path.is_file() && entry.file_name() == "Theme" {
                    let relative = path.strip_prefix(root).unwrap_or(&path);
                    let target = dest.join(relative);
                    if let Some(parent) = target.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    copy2(&path, &target)?;
                }
            }
            Ok(())
        }
        walk(&root, &root, dest)
    }

    fn delete_created_file(&self, remote_path: &str, expected_cues: usize) -> VvResult<String> {
        let target = Path::new(remote_path);
        if !target.is_file() {
            return Ok("The presentation file was already gone".to_string());
        }
        let message = read_presentation(&fs::read(target)?)?;
        if message.len("cues") != expected_cues {
            return Err(VvError::write(format!(
                "{} has {} slides now, not the {expected_cues} VerseVision added, so it was edited. \
                 It was not deleted; remove it in ProPresenter if you no longer want it",
                target.file_name().unwrap_or_default().to_string_lossy(),
                message.len("cues")
            )));
        }
        fs::remove_file(target)?;
        Ok("Presentation removed".to_string())
    }

    /// Copy a new file into place. Never replaces an existing file.
    fn create_file(&self, local_path: &Path, remote_path: &str) -> VvResult<String> {
        let target = Path::new(remote_path);
        if target.exists() {
            return Err(VvError::write(format!(
                "{} already exists; nothing was changed",
                target.file_name().unwrap_or_default().to_string_lossy()
            )));
        }
        let temp = write_temp(&fs::read(local_path)?, target)?;
        fs::hard_link(temp.path(), target)?;
        drop(temp);
        sha(target)
    }

    /// Plain rename (a hard link + unlink leaves ProPresenter with a wrong id), refusing to replace an existing file.
    fn rename_file(&self, old_path: &str, new_path: &str) -> VvResult<()> {
        let (old, new) = (Path::new(old_path), Path::new(new_path));
        let name = |p: &Path| {
            p.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        };
        if !old.is_file() {
            return Err(VvError::write(format!(
                "{} is not there to rename",
                name(old)
            )));
        }
        if new.exists() {
            return Err(VvError::write(format!(
                "{} already exists; nothing was renamed",
                name(new)
            )));
        }
        fs::rename(old, new)?;
        Ok(())
    }

    /// Delete a file only if it is still exactly the one we expect.
    fn delete_file(&self, remote_path: &str, expected_sha: &str) -> VvResult<String> {
        let target = Path::new(remote_path);
        if !target.is_file() {
            return Ok("already gone".to_string());
        }
        if sha(target)? != expected_sha {
            return Err(VvError::write(format!(
                "{} changed since VerseVision read it, so it was not deleted",
                target.file_name().unwrap_or_default().to_string_lossy()
            )));
        }
        fs::remove_file(target)?;
        Ok("removed".to_string())
    }

    fn find_presentation(&self, name: &str, library: Option<&str>) -> Vec<String> {
        let root = self.expand(&self.node.library_root);
        if !root.exists() {
            return Vec::new();
        }
        let folders: Vec<PathBuf> = match library {
            Some(library) if !library.is_empty() => vec![root.join(library)],
            _ => fs::read_dir(&root)
                .map(|entries| {
                    entries
                        .filter_map(Result::ok)
                        .map(|e| e.path())
                        .filter(|p| p.is_dir())
                        .collect()
                })
                .unwrap_or_default(),
        };
        let mut found: Vec<String> = folders
            .iter()
            .map(|folder| folder.join(format!("{name}.pro")))
            .filter(|p| p.is_file())
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        found.sort();
        found
    }

    fn backup_file(&self, remote_path: &str) -> VvResult<String> {
        let now = self.clock.now_micros().div_euclid(1_000_000);
        Ok(create_backup(Path::new(remote_path), now)?
            .to_string_lossy()
            .into_owned())
    }

    fn restore_backup(&self, backup_path: &str, remote_path: &str) -> VvResult<()> {
        restore_backup(Path::new(remote_path), Path::new(backup_path)).map_err(|e| {
            let error = VvError::from(e);
            VvError::new(Kind::Other("ValueError"), error.message)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variables_expand_like_python() {
        let home = Path::new("/home/u");
        let env = |name: &str| (name == "X").then(|| "x".to_string());
        assert_eq!(
            expand_path("$HOME/Documents/${X}/$NOPE/$X", home, &env),
            PathBuf::from("/home/u/Documents/x/$NOPE/x")
        );
        assert_eq!(expand_path("~/a", home, &env), PathBuf::from("/home/u/a"));
        assert_eq!(expand_path("/a/b", home, &env), PathBuf::from("/a/b"));
    }

    #[test]
    fn shell_quote_matches_shlex() {
        assert_eq!(shell_quote("abc-1.2/x"), "abc-1.2/x");
        assert_eq!(shell_quote("a b"), "'a b'");
        assert_eq!(shell_quote("it's"), "'it'\"'\"'s'");
        assert_eq!(shell_quote(""), "''");
    }
}
