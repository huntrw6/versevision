//! Where the command line keeps its files: the OS defaults (`platformdirs` for the app name `versevision`, which is what the Python app
//! used) and Python's overrides (`VERSEVISION_CONFIG`, `VERSEVISION_HISTORY`, `VERSEVISION_LOGS`, `VERSEVISION_KEYS`).
//! macOS and Linux are compared with `platformdirs` in `oracle/cli/dirs.json`; **Windows is not verified on a Windows machine**.

use std::path::{Path, PathBuf};

use versevision_pp::dirs::{Dirs, DirsEnv, Os};

#[derive(Debug, Clone)]
pub struct Paths {
    pub home: PathBuf,
    pub config_file: PathBuf,
    pub history_file: PathBuf,
    /// The folder that holds `runs/` (`VERSEVISION_LOGS` or the OS log folder).
    pub logs: PathBuf,
    /// `audit.jsonl` always sits in the OS log folder (Python ignores `VERSEVISION_LOGS` for it).
    pub os_logs: PathBuf,
    pub keys_file: PathBuf,
    pub cache_dir: PathBuf,
    pub os_dirs: Dirs,
}

fn expanduser(text: &str, home: &Path) -> PathBuf {
    if text == "~" {
        return home.to_path_buf();
    }
    if let Some(rest) = text.strip_prefix("~/").or_else(|| text.strip_prefix("~\\")) {
        return home.join(rest);
    }
    PathBuf::from(text)
}

/// `platformdirs.user_cache_path("versevision")`.
pub fn cache_dir(os: Os, env: &DirsEnv) -> PathBuf {
    match os {
        Os::Linux => std::env::var_os("XDG_CACHE_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| env.home.join(".cache"))
            .join("versevision"),
        Os::MacOs => env.home.join("Library/Caches/versevision"),
        Os::Windows => env
            .local_app_data
            .clone()
            .unwrap_or_else(|| env.home.join("AppData/Local"))
            .join("versevision")
            .join("versevision")
            .join("Cache"),
    }
}

impl Paths {
    pub fn from_process() -> Paths {
        let env = DirsEnv::from_process();
        Paths::with(Os::current(), &env, &|name| {
            std::env::var(name).ok().filter(|v| !v.is_empty())
        })
    }

    pub fn with(os: Os, env: &DirsEnv, var: &dyn Fn(&str) -> Option<String>) -> Paths {
        let os_dirs = Dirs::platform_defaults(os, env);
        let home = env.home.clone();
        let config_file = var("VERSEVISION_CONFIG")
            .map(|v| expanduser(&v, &home))
            .unwrap_or_else(|| os_dirs.config_file());
        let history_file = var("VERSEVISION_HISTORY")
            .map(|v| expanduser(&v, &home))
            .unwrap_or_else(|| os_dirs.history_path());
        let logs = var("VERSEVISION_LOGS")
            .map(|v| expanduser(&v, &home))
            .unwrap_or_else(|| os_dirs.logs.clone());
        let keys_file = var("VERSEVISION_KEYS")
            .map(|v| expanduser(&v, &home))
            .unwrap_or_else(|| os_dirs.keys_file());
        Paths {
            cache_dir: cache_dir(os, env),
            home,
            config_file,
            history_file,
            os_logs: os_dirs.logs.clone(),
            logs,
            keys_file,
            os_dirs,
        }
    }

    pub fn runs_dir(&self) -> PathBuf {
        self.logs.join("runs")
    }

    /// The `Dirs` the pipeline wants: this run's logs folder, the history's folder, the keys' folder.
    pub fn pipeline_dirs(&self) -> Dirs {
        Dirs {
            config: self
                .config_file
                .parent()
                .map_or_else(PathBuf::new, Path::to_path_buf),
            data: self
                .history_file
                .parent()
                .map_or_else(PathBuf::new, Path::to_path_buf),
            logs: self.logs.clone(),
            keys: self
                .keys_file
                .parent()
                .map_or_else(PathBuf::new, Path::to_path_buf),
        }
    }
}
