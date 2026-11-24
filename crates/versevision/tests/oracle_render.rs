//! The pure parts of the command line against what Python produced: the report tables of `run` (`render.jsonl`), the login-item files
//! (`autostart.json`) and the OS folders (`dirs.json`, from `platformdirs`).

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;
use versevision_cli::autostart::{self, Platform, Runner, Setup};
use versevision_cli::paths::Paths;
use versevision_cli::render::print_report;
use versevision_cli::rich::Console;
use versevision_pp::dirs::{DirsEnv, Os};

fn oracle() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../oracle")
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|s| s.as_str().unwrap().to_string())
        .collect()
}

#[test]
fn report_tables_are_pythons() {
    let text = fs::read_to_string(oracle().join("cli/render.jsonl")).unwrap();
    let mut count = 0;
    for line in text.lines() {
        let case: Value = serde_json::from_str(line).unwrap();
        let name = case["name"].as_str().unwrap();
        let (scenario, run) = name.split_once('/').unwrap();
        let report_file = oracle()
            .join("pipeline/scenarios")
            .join(scenario)
            .join("expected/runs")
            .join(run)
            .join("report.json");
        let report: Value =
            serde_json::from_str(&fs::read_to_string(report_file).unwrap()).unwrap();
        let width = case["width"].as_u64().unwrap() as usize;
        let got = print_report(&Console::new(width, false), &report);
        assert_eq!(
            got,
            case["expected"].as_str().unwrap(),
            "{name} at {width} columns"
        );
        count += 1;
    }
    assert!(count > 50, "only {count} report cases");
    eprintln!("render.jsonl: {count} report printouts identical to Python");
}

#[test]
fn login_item_files_are_pythons() {
    let data: Value =
        serde_json::from_str(&fs::read_to_string(oracle().join("cli/autostart.json")).unwrap())
            .unwrap();
    for case in data["plist"].as_array().unwrap() {
        let args = strings(&case["args"]);
        let got = autostart::launch_agent_plist(
            &args,
            case["log"].as_str().unwrap(),
            Path::new("/home/u"),
        );
        assert_eq!(
            got,
            case["expected"].as_str().unwrap(),
            "plist for {args:?}"
        );
    }
    for case in data["systemd"].as_array().unwrap() {
        let args = strings(&case["args"]);
        assert_eq!(
            autostart::systemd_unit(&args),
            case["expected"].as_str().unwrap(),
            "unit for {args:?}"
        );
    }
    for case in data["windows"].as_array().unwrap() {
        let args = strings(&case["args"]);
        assert_eq!(
            autostart::windows_task_command(&args),
            case["expected"].as_str().unwrap(),
            "task for {args:?}"
        );
    }
}

#[test]
fn os_folders_are_platformdirs() {
    let rows: Value =
        serde_json::from_str(&fs::read_to_string(oracle().join("cli/dirs.json")).unwrap()).unwrap();
    for row in rows.as_array().unwrap() {
        let os = match row["os"].as_str().unwrap() {
            "macos" => Os::MacOs,
            _ => Os::Linux,
        };
        let env = DirsEnv {
            home: PathBuf::from(row["home"].as_str().unwrap()),
            ..Default::default()
        };
        let paths = Paths::with(os, &env, &|_| None);
        let show = |p: &Path| p.to_string_lossy().replace('\\', "/");
        assert_eq!(show(&paths.os_dirs.config), row["config"].as_str().unwrap());
        assert_eq!(show(&paths.os_dirs.data), row["data"].as_str().unwrap());
        assert_eq!(show(&paths.os_dirs.logs), row["logs"].as_str().unwrap());
        assert_eq!(show(&paths.cache_dir), row["cache"].as_str().unwrap());
        assert_eq!(
            show(&paths.config_file),
            format!("{}/config.toml", row["config"].as_str().unwrap())
        );
        assert_eq!(
            show(&paths.history_file),
            format!("{}/imports.json", row["data"].as_str().unwrap())
        );
        assert_eq!(
            show(&paths.runs_dir()),
            format!("{}/runs", row["logs"].as_str().unwrap())
        );
    }
}

#[test]
fn python_overrides_win() {
    let env = DirsEnv {
        home: PathBuf::from("/home/u"),
        ..Default::default()
    };
    let var = |name: &str| match name {
        "VERSEVISION_CONFIG" => Some("~/c/conf.toml".to_string()),
        "VERSEVISION_HISTORY" => Some("/h/imports.json".to_string()),
        "VERSEVISION_LOGS" => Some("/l".to_string()),
        "VERSEVISION_KEYS" => Some("/k/keys.env".to_string()),
        _ => None,
    };
    let paths = Paths::with(Os::Linux, &env, &var);
    assert_eq!(paths.config_file, PathBuf::from("/home/u/c/conf.toml"));
    assert_eq!(paths.history_file, PathBuf::from("/h/imports.json"));
    assert_eq!(paths.runs_dir(), PathBuf::from("/l/runs"));
    assert_eq!(paths.keys_file, PathBuf::from("/k/keys.env"));
    // the audit log stays in the OS folder, as in Python
    assert_eq!(
        paths.os_logs,
        PathBuf::from("/home/u/.local/state/versevision/log")
    );
}

/// A runner that records what would be run and answers like a machine where everything works.
struct Recording(std::sync::Mutex<Vec<String>>);

impl Runner for Recording {
    fn run(&self, command: &[String]) -> (i32, String) {
        self.0.lock().unwrap().push(command.join(" "));
        (0, String::new())
    }
    fn has_program(&self, _: &str) -> bool {
        true
    }
}

fn setup<'a>(root: &Path, platform: Platform, runner: &'a dyn Runner) -> Setup<'a> {
    Setup {
        platform,
        home: root.join("home"),
        xdg_config_home: None,
        os_logs: root.join("logs"),
        program: PathBuf::from("/usr/local/bin/versevision"),
        uid: 501,
        runner,
    }
}

#[test]
fn macos_autostart_writes_the_launch_agent_and_loads_it() {
    let root = tempfile::tempdir().unwrap();
    let recording = Recording(Default::default());
    let s = setup(root.path(), Platform::MacOs, &recording);
    let status = s.enable("127.0.0.1", 8777, true).unwrap();
    assert!(status.enabled);
    assert_eq!(
        (status.host.as_deref(), status.port),
        (Some("127.0.0.1"), Some(8777))
    );
    let plist = root
        .path()
        .join("home")
        .join("Library")
        .join("LaunchAgents")
        .join("org.versevision.serve.plist");
    let text = fs::read_to_string(&plist).unwrap();
    let log = root
        .path()
        .join("logs")
        .join("serve.log")
        .display()
        .to_string();
    assert_eq!(
        text,
        autostart::launch_agent_plist(
            &autostart::program_args(Path::new("/usr/local/bin/versevision"), "127.0.0.1", 8777),
            &log,
            &root.path().join("home")
        )
    );
    let commands = recording.0.lock().unwrap().clone();
    assert_eq!(
        commands[0],
        format!("launchctl bootout gui/501 {}", plist.display())
    );
    assert_eq!(
        commands[1],
        format!("launchctl bootstrap gui/501 {}", plist.display())
    );
    let off = s.disable(false).unwrap();
    assert!(!off.enabled);
    assert!(!plist.exists());
}

#[test]
fn linux_autostart_writes_the_user_unit_and_enables_it() {
    let root = tempfile::tempdir().unwrap();
    let recording = Recording(Default::default());
    let s = setup(root.path(), Platform::Linux, &recording);
    let status = s.enable("127.0.0.1", 9000, true).unwrap();
    assert_eq!(status.port, Some(9000));
    let unit = root
        .path()
        .join("home/.config/systemd/user/versevision-serve.service");
    assert!(fs::read_to_string(&unit).unwrap().contains(
        "ExecStart=/usr/local/bin/versevision serve --host 127.0.0.1 --port 9000 --no-open"
    ));
    let commands = recording.0.lock().unwrap().clone();
    assert!(commands.contains(&"systemctl --user daemon-reload".to_string()));
    assert!(
        commands.contains(&"systemctl --user enable --now versevision-serve.service".to_string())
    );
    s.disable(false).unwrap();
    assert!(!unit.exists());
}

#[test]
fn windows_autostart_makes_a_logon_task_unverified_on_windows() {
    let root = tempfile::tempdir().unwrap();
    let recording = Recording(Default::default());
    let s = setup(root.path(), Platform::Windows, &recording);
    s.enable("localhost", 8777, true).unwrap();
    let commands = recording.0.lock().unwrap().clone();
    assert_eq!(
        commands[0],
        "schtasks /create /f /sc onlogon /tn VerseVision /tr /usr/local/bin/versevision serve --host localhost --port 8777 --no-open"
    );
    s.disable(false).unwrap();
    assert!(recording
        .0
        .lock()
        .unwrap()
        .iter()
        .any(|c| c == "schtasks /delete /f /tn VerseVision"));
}

#[test]
fn autostart_refuses_a_network_address_and_a_bad_port() {
    let root = tempfile::tempdir().unwrap();
    let recording = Recording(Default::default());
    let s = setup(root.path(), Platform::Linux, &recording);
    assert!(s
        .enable("0.0.0.0", 8777, true)
        .unwrap_err()
        .starts_with("Autostart only supports the local address"));
    assert_eq!(
        s.enable("127.0.0.1", 0, true).unwrap_err(),
        "port must be between 1 and 65535"
    );
    assert!(recording.0.lock().unwrap().is_empty());
}
