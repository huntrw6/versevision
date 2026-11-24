//! Where the app keeps its files. The host passes this in (nothing here reads `~` or the environment when a path is given); the
//! defaults ([`Dirs::platform_defaults`]) are what Python's `platformdirs` returns for the app name `versevision`, so a machine that ran
//! the Python app keeps its history, run logs and settings after the change.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dirs {
    /// `config.toml` lives here (`VERSEVISION_CONFIG` of Python points at the file itself).
    pub config: PathBuf,
    /// `imports.json` (the history) and its lock live here.
    pub data: PathBuf,
    /// `runs/<id>/` and `audit.jsonl` live here.
    pub logs: PathBuf,
    /// `keys.env` lives here.
    pub keys: PathBuf,
}

/// The operating systems the defaults are known for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Linux,
    MacOs,
    Windows,
}

impl Os {
    pub fn current() -> Os {
        if cfg!(target_os = "macos") {
            Os::MacOs
        } else if cfg!(windows) {
            Os::Windows
        } else {
            Os::Linux
        }
    }
}

/// The environment a default needs, so the host (or a test) passes it instead of this module reading it.
#[derive(Debug, Clone, Default)]
pub struct DirsEnv {
    pub home: PathBuf,
    pub xdg_data_home: Option<PathBuf>,
    pub xdg_config_home: Option<PathBuf>,
    pub xdg_state_home: Option<PathBuf>,
    /// Windows: `%LOCALAPPDATA%`.
    pub local_app_data: Option<PathBuf>,
}

impl DirsEnv {
    /// The process environment.
    pub fn from_process() -> DirsEnv {
        let path = |name: &str| {
            std::env::var_os(name)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        };
        DirsEnv {
            home: path("HOME")
                .or_else(|| path("USERPROFILE"))
                .unwrap_or_default(),
            xdg_data_home: path("XDG_DATA_HOME"),
            xdg_config_home: path("XDG_CONFIG_HOME"),
            xdg_state_home: path("XDG_STATE_HOME"),
            local_app_data: path("LOCALAPPDATA"),
        }
    }
}

impl Dirs {
    /// One folder for everything (a portable install, a test).
    pub fn under(root: &Path) -> Dirs {
        Dirs {
            config: root.join("config"),
            data: root.join("data"),
            logs: root.join("logs"),
            keys: root.join("config"),
        }
    }

    /// `platformdirs.user_*_path("versevision")` for the given system.
    ///
    /// Linux and macOS follow platformdirs exactly. **Windows is unverified**: it is what platformdirs documents
    /// (`%LOCALAPPDATA%\versevision\versevision`, logs in its `Logs` folder), not checked on a machine that ran the Python app.
    pub fn platform_defaults(os: Os, env: &DirsEnv) -> Dirs {
        let app = "versevision";
        match os {
            Os::Linux => {
                let data = env
                    .xdg_data_home
                    .clone()
                    .unwrap_or_else(|| env.home.join(".local/share"))
                    .join(app);
                let config = env
                    .xdg_config_home
                    .clone()
                    .unwrap_or_else(|| env.home.join(".config"))
                    .join(app);
                let logs = env
                    .xdg_state_home
                    .clone()
                    .unwrap_or_else(|| env.home.join(".local/state"))
                    .join(app)
                    .join("log");
                Dirs {
                    keys: config.clone(),
                    config,
                    data,
                    logs,
                }
            }
            Os::MacOs => {
                let support = env.home.join("Library/Application Support").join(app);
                Dirs {
                    keys: support.clone(),
                    config: support.clone(),
                    data: support,
                    logs: env.home.join("Library/Logs").join(app),
                }
            }
            Os::Windows => {
                let root = env
                    .local_app_data
                    .clone()
                    .unwrap_or_else(|| env.home.join("AppData/Local"))
                    .join(app)
                    .join(app);
                Dirs {
                    keys: root.clone(),
                    config: root.clone(),
                    data: root.clone(),
                    logs: root.join("Logs"),
                }
            }
        }
    }

    /// `history_path()` without the `VERSEVISION_HISTORY` override.
    pub fn history_path(&self) -> PathBuf {
        self.data.join("imports.json")
    }
    /// `runlog.runs_dir()` without the `VERSEVISION_LOGS` override.
    pub fn runs_dir(&self) -> PathBuf {
        self.logs.join("runs")
    }
    pub fn audit_path(&self) -> PathBuf {
        self.logs.join("audit.jsonl")
    }
    pub fn config_file(&self) -> PathBuf {
        self.config.join("config.toml")
    }
    pub fn keys_file(&self) -> PathBuf {
        self.keys.join("keys.env")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_follow_platformdirs() {
        let env = DirsEnv {
            home: PathBuf::from("/home/u"),
            ..Default::default()
        };
        let linux = Dirs::platform_defaults(Os::Linux, &env);
        assert_eq!(
            linux.data,
            PathBuf::from("/home/u/.local/share/versevision")
        );
        assert_eq!(
            linux.logs,
            PathBuf::from("/home/u/.local/state/versevision/log")
        );
        assert_eq!(linux.config, PathBuf::from("/home/u/.config/versevision"));
        let mac = Dirs::platform_defaults(Os::MacOs, &env);
        assert_eq!(mac.logs, PathBuf::from("/home/u/Library/Logs/versevision"));
        assert_eq!(
            mac.history_path(),
            PathBuf::from("/home/u/Library/Application Support/versevision/imports.json")
        );
        let win = Dirs::platform_defaults(
            Os::Windows,
            &DirsEnv {
                local_app_data: Some(PathBuf::from("C:/Users/u/AppData/Local")),
                ..Default::default()
            },
        );
        assert_eq!(
            win.logs,
            PathBuf::from("C:/Users/u/AppData/Local/versevision/versevision/Logs")
        );
    }
}
