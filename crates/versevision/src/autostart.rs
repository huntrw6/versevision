//! Start `versevision serve` when the user logs in (`autostart.py`). macOS: a LaunchAgent. Linux: a systemd user service. Windows: a
//! Task Scheduler logon task (`schtasks`), per user, no administrator rights. **The Windows path is unverified on a real Windows machine**,
//! and the macOS and Linux actions are only exercised in tests through a recording runner (the files they write are compared with Python's).

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

pub const LABEL: &str = "org.versevision.serve";
pub const SYSTEMD_UNIT: &str = "versevision-serve.service";
pub const WINDOWS_TASK: &str = "VerseVision";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    MacOs,
    Linux,
    Windows,
}

impl Platform {
    pub fn current() -> Platform {
        if cfg!(target_os = "macos") {
            Platform::MacOs
        } else if cfg!(windows) {
            Platform::Windows
        } else {
            Platform::Linux
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Platform::MacOs => "macos",
            Platform::Linux => "linux",
            Platform::Windows => "windows",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub supported: bool,
    pub enabled: bool,
    pub platform: &'static str,
    pub detail: String,
    pub host: Option<String>,
    pub port: Option<i64>,
    pub file: Option<String>,
}

impl Status {
    /// `AutostartStatus.model_dump_json(indent=2)`.
    pub fn to_value(&self) -> Value {
        json!({
            "supported": self.supported, "enabled": self.enabled, "platform": self.platform, "detail": self.detail,
            "host": self.host, "port": self.port, "file": self.file,
        })
    }
}

fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// `plistlib.dumps` of the LaunchAgent (keys sorted, tab indentation, XML format).
pub fn launch_agent_plist(args: &[String], log_path: &str, home: &Path) -> String {
    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n",
    );
    let path = format!(
        "/usr/local/bin:/opt/homebrew/bin:/usr/bin:/bin:{}/.local/bin",
        home.display()
    );
    out.push_str("\t<key>EnvironmentVariables</key>\n\t<dict>\n\t\t<key>PATH</key>\n");
    out.push_str(&format!(
        "\t\t<string>{}</string>\n\t</dict>\n",
        xml_escape(&path)
    ));
    out.push_str("\t<key>KeepAlive</key>\n\t<dict>\n\t\t<key>SuccessfulExit</key>\n\t\t<false/>\n\t</dict>\n");
    out.push_str(&format!("\t<key>Label</key>\n\t<string>{LABEL}</string>\n"));
    out.push_str("\t<key>ProcessType</key>\n\t<string>Background</string>\n");
    out.push_str("\t<key>ProgramArguments</key>\n\t<array>\n");
    for arg in args {
        out.push_str(&format!("\t\t<string>{}</string>\n", xml_escape(arg)));
    }
    out.push_str("\t</array>\n\t<key>RunAtLoad</key>\n\t<true/>\n");
    out.push_str(&format!(
        "\t<key>StandardErrorPath</key>\n\t<string>{p}</string>\n\t<key>StandardOutPath</key>\n\t<string>{p}</string>\n",
        p = xml_escape(log_path)
    ));
    out.push_str("</dict>\n</plist>\n");
    out
}

fn systemd_quote(arg: &str) -> String {
    if arg.chars().any(|c| matches!(c, ' ' | '\t' | '"' | '\\')) {
        format!("\"{}\"", arg.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        arg.to_string()
    }
}

pub fn systemd_unit(args: &[String]) -> String {
    let exec: Vec<String> = args.iter().map(|a| systemd_quote(a)).collect();
    format!(
        "[Unit]\nDescription=VerseVision web page\nAfter=network-online.target\n\n[Service]\nExecStart={}\nRestart=on-failure\nRestartSec=5\n\n[Install]\nWantedBy=default.target\n",
        exec.join(" ")
    )
}

pub fn windows_task_command(args: &[String]) -> String {
    args.iter()
        .map(|a| {
            if a.contains(' ') {
                format!("\"{a}\"")
            } else {
                a.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// What runs `launchctl`, `systemctl`, `schtasks`: (exit code, stdout+stderr). A test supplies one that records.
pub trait Runner {
    fn run(&self, command: &[String]) -> (i32, String);
    fn has_program(&self, name: &str) -> bool;
}

pub struct SystemRunner;

impl Runner for SystemRunner {
    fn run(&self, command: &[String]) -> (i32, String) {
        let Some((program, rest)) = command.split_first() else {
            return (1, String::new());
        };
        match std::process::Command::new(program).args(rest).output() {
            Ok(out) => {
                let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
                text.push_str(&String::from_utf8_lossy(&out.stderr));
                (out.status.code().unwrap_or(1), text)
            }
            Err(error) => (127, error.to_string()),
        }
    }
    fn has_program(&self, name: &str) -> bool {
        let Some(path) = std::env::var_os("PATH") else {
            return false;
        };
        std::env::split_paths(&path).any(|dir| dir.join(name).is_file())
    }
}

/// Everything autostart needs from the machine.
pub struct Setup<'a> {
    pub platform: Platform,
    pub home: PathBuf,
    pub xdg_config_home: Option<PathBuf>,
    pub os_logs: PathBuf,
    /// The program started at login (this binary).
    pub program: PathBuf,
    pub uid: u32,
    pub runner: &'a dyn Runner,
}

pub fn program_args(program: &Path, host: &str, port: i64) -> Vec<String> {
    vec![
        program.display().to_string(),
        "serve".into(),
        "--host".into(),
        host.into(),
        "--port".into(),
        port.to_string(),
        "--no-open".into(),
    ]
}

impl Setup<'_> {
    fn launch_agent_path(&self) -> PathBuf {
        self.home
            .join("Library")
            .join("LaunchAgents")
            .join(format!("{LABEL}.plist"))
    }

    fn systemd_path(&self) -> PathBuf {
        let base = self
            .xdg_config_home
            .clone()
            .unwrap_or_else(|| self.home.join(".config"));
        base.join("systemd").join("user").join(SYSTEMD_UNIT)
    }

    fn log_path(&self) -> std::io::Result<PathBuf> {
        std::fs::create_dir_all(&self.os_logs)?;
        Ok(self.os_logs.join("serve.log"))
    }

    fn run(&self, command: &[&str], check: bool) -> Result<(i32, String), String> {
        let owned: Vec<String> = command.iter().map(|s| (*s).to_string()).collect();
        let (code, text) = self.runner.run(&owned);
        if check && code != 0 {
            let head: Vec<&str> = command.iter().take(3).copied().collect();
            let tail: String = {
                let t = text.trim();
                let chars: Vec<char> = t.chars().collect();
                chars[chars.len().saturating_sub(300)..].iter().collect()
            };
            return Err(format!("{} failed: {}", head.join(" "), tail));
        }
        Ok((code, text))
    }

    pub fn status(&self) -> Status {
        let platform = self.platform;
        let mut status = Status {
            supported: true,
            enabled: false,
            platform: platform.name(),
            detail: String::new(),
            host: None,
            port: None,
            file: None,
        };
        match platform {
            Platform::MacOs => {
                let path = self.launch_agent_path();
                status.enabled = path.exists();
                status.file = Some(path.display().to_string());
                // Python reads ProgramArguments back out of the plist
                if let Ok(text) = std::fs::read_to_string(&path) {
                    let args = plist_arguments(&text);
                    apply_args(&mut status, &args);
                }
            }
            Platform::Linux => {
                let path = self.systemd_path();
                status.enabled = path.exists()
                    && self
                        .run(&["systemctl", "--user", "is-enabled", SYSTEMD_UNIT], false)
                        .map(|(code, _)| code == 0)
                        .unwrap_or(false);
                status.supported = self.runner.has_program("systemctl");
                status.file = Some(path.display().to_string());
                if let Ok(text) = std::fs::read_to_string(&path) {
                    let line = text
                        .lines()
                        .find(|l| l.starts_with("ExecStart="))
                        .unwrap_or("");
                    let args: Vec<String> = line
                        .trim_start_matches("ExecStart=")
                        .replace('"', "")
                        .split_whitespace()
                        .map(str::to_string)
                        .collect();
                    apply_args(&mut status, &args);
                }
            }
            Platform::Windows => {
                let enabled = self
                    .run(&["schtasks", "/query", "/tn", WINDOWS_TASK], false)
                    .map(|(code, _)| code == 0)
                    .unwrap_or(false);
                status.enabled = enabled;
            }
        }
        status
    }

    pub fn enable(&self, host: &str, port: i64, start_now: bool) -> Result<Status, String> {
        if !(1..=65535).contains(&port) {
            return Err("port must be between 1 and 65535".into());
        }
        if !matches!(host, "127.0.0.1" | "localhost" | "::1") {
            return Err("Autostart only supports the local address (127.0.0.1); a network address needs a private access token each start".into());
        }
        let args = program_args(&self.program, host, port);
        match self.platform {
            Platform::MacOs => {
                let path = self.launch_agent_path();
                std::fs::create_dir_all(path.parent().unwrap_or(Path::new(".")))
                    .map_err(|e| e.to_string())?;
                let log = self.log_path().map_err(|e| e.to_string())?;
                std::fs::write(
                    &path,
                    launch_agent_plist(&args, &log.display().to_string(), &self.home),
                )
                .map_err(|e| e.to_string())?;
                if start_now {
                    let domain = format!("gui/{}", self.uid);
                    let file = path.display().to_string();
                    self.run(&["launchctl", "bootout", &domain, &file], false)?;
                    self.run(&["launchctl", "bootstrap", &domain, &file], true)?;
                }
            }
            Platform::Linux => {
                if !self.runner.has_program("systemctl") {
                    return Err(
                        "systemd was not found; autostart is not supported on this system".into(),
                    );
                }
                let path = self.systemd_path();
                std::fs::create_dir_all(path.parent().unwrap_or(Path::new(".")))
                    .map_err(|e| e.to_string())?;
                std::fs::write(&path, systemd_unit(&args)).map_err(|e| e.to_string())?;
                self.run(&["systemctl", "--user", "daemon-reload"], true)?;
                if start_now {
                    self.run(
                        &["systemctl", "--user", "enable", "--now", SYSTEMD_UNIT],
                        true,
                    )?;
                } else {
                    self.run(&["systemctl", "--user", "enable", SYSTEMD_UNIT], true)?;
                }
            }
            Platform::Windows => {
                let command = windows_task_command(&args);
                self.run(
                    &[
                        "schtasks",
                        "/create",
                        "/f",
                        "/sc",
                        "onlogon",
                        "/tn",
                        WINDOWS_TASK,
                        "/tr",
                        &command,
                    ],
                    true,
                )?;
            }
        }
        Ok(self.status())
    }

    pub fn disable(&self, keep_running: bool) -> Result<Status, String> {
        match self.platform {
            Platform::MacOs => {
                let path = self.launch_agent_path();
                if path.exists() {
                    if !keep_running {
                        let domain = format!("gui/{}", self.uid);
                        self.run(
                            &["launchctl", "bootout", &domain, &path.display().to_string()],
                            false,
                        )?;
                    }
                    std::fs::remove_file(&path).map_err(|e| e.to_string())?;
                }
            }
            Platform::Linux => {
                let path = self.systemd_path();
                if path.exists() {
                    if keep_running {
                        self.run(&["systemctl", "--user", "disable", SYSTEMD_UNIT], false)?;
                    } else {
                        self.run(
                            &["systemctl", "--user", "disable", "--now", SYSTEMD_UNIT],
                            false,
                        )?;
                    }
                    std::fs::remove_file(&path).map_err(|e| e.to_string())?;
                    self.run(&["systemctl", "--user", "daemon-reload"], false)?;
                }
            }
            Platform::Windows => {
                self.run(&["schtasks", "/delete", "/f", "/tn", WINDOWS_TASK], false)?;
            }
        }
        Ok(self.status())
    }
}

fn apply_args(status: &mut Status, args: &[String]) {
    if let Some(i) = args.iter().position(|a| a == "--port") {
        status.port = args.get(i + 1).and_then(|p| p.parse().ok());
    }
    if let Some(i) = args.iter().position(|a| a == "--host") {
        status.host = args.get(i + 1).cloned();
    }
}

/// The `ProgramArguments` strings of a plist this module wrote.
fn plist_arguments(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let Some(start) = text.find("<key>ProgramArguments</key>") else {
        return out;
    };
    let rest = &text[start..];
    let Some(end) = rest.find("</array>") else {
        return out;
    };
    for part in rest[..end].split("<string>").skip(1) {
        if let Some(close) = part.find("</string>") {
            out.push(
                part[..close]
                    .replace("&lt;", "<")
                    .replace("&gt;", ">")
                    .replace("&amp;", "&"),
            );
        }
    }
    out
}
