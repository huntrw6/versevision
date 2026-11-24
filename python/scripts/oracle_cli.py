"""The command line oracle (S7b): what `versevision ...` prints and returns, written as `oracle/cli/`.

    oracle/cli/cases.jsonl     one case per line: setup, steps (argv, stdin), and what the Python app answered (exit code, stdout, stderr)
                               plus the files it left behind. The Rust binary runs the same steps in the same kind of sandbox and must print
                               the same bytes (crates/versevision/tests/oracle_cli.rs).
    oracle/cli/render.jsonl    the printing functions on their own (report tables), fed with reports of the S6b scenarios
    oracle/cli/autostart.json  the login-item files Python writes (launchd plist, systemd unit, Windows task command)
    oracle/cli/dirs.json       `platformdirs` for macOS, Windows and Linux with a made-up home

Everything runs in a sandbox (`{ROOT}`): HOME, the config, history, logs and keys files all live there. A case is run in two sandboxes with
root paths of different lengths; when the answers differ (a path made the text wrap) it is run again 4000 columns wide and marked `columns`.
"""
from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import tempfile
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

PY_ROOT = Path(__file__).resolve().parents[1]
LAUNCH = ("import sys; sys.argv[0] = 'versevision'; from versevision.cli import app; app()")
THEME_FIXTURES = PY_ROOT / "tests" / "fixtures" / "themes"


def _python() -> str:
    return sys.executable


def _env(root: Path, columns: int | None, extra: dict) -> dict:
    env = {
        "PATH": "/usr/bin:/bin",
        "HOME": str(root / "home"),
        "LANG": "C.UTF-8",
        "PYTHONIOENCODING": "utf-8",
        "PYTHONPATH": str(PY_ROOT / "src"),
        "VERSEVISION_CONFIG": str(root / "config.toml"),
        "VERSEVISION_HISTORY": str(root / "imports.json"),
        "VERSEVISION_LOGS": str(root / "logs"),
        "VERSEVISION_KEYS": str(root / "keys.env"),
    }
    if columns:
        env["COLUMNS"] = str(columns)
    env.update({k: v.replace("{ROOT}", str(root)) for k, v in extra.items()})
    return env


def _setup(root: Path, ops: list, repo: Path) -> None:
    (root / "home").mkdir(parents=True, exist_ok=True)
    for op in ops:
        kind = op["op"]
        if kind == "write":
            target = root / op["path"]
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text(op["text"].replace("{ROOT}", str(root)), encoding="utf-8")
        elif kind == "copy":
            source = repo / op["from"]
            target = root / op["to"]
            if source.is_dir():
                shutil.copytree(source, target, dirs_exist_ok=True)
            else:
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_bytes(source.read_bytes())
        elif kind == "seed_runs":              # the run folders of an S6b scenario, under their own names
            base = repo / "oracle" / "pipeline" / "scenarios" / op["scenario"] / "expected" / "runs"
            for run in sorted(base.iterdir()):
                name = (run / "name").read_text(encoding="utf-8").strip()
                dest = root / "logs" / "runs" / name
                dest.mkdir(parents=True, exist_ok=True)
                for item in run.iterdir():
                    if item.name != "name":
                        dest.joinpath(item.name).write_bytes(item.read_bytes())
        elif kind == "seed_history":
            source = repo / "oracle" / "pipeline" / "scenarios" / op["scenario"] / "expected" / "history.json"
            (root / "imports.json").write_text(source.read_text(encoding="utf-8").replace("<ROOT>", str(root)), encoding="utf-8")
        else:
            raise ValueError(kind)


def _run_case(case: dict, root: Path, columns: int | None, repo: Path) -> dict:
    if root.exists():
        shutil.rmtree(root)
    root.mkdir(parents=True)
    _setup(root, case.get("setup", []), repo)
    steps = []
    for step in case["steps"]:
        argv = [a.replace("{ROOT}", str(root)) for a in step["argv"]]
        done = subprocess.run([_python(), "-c", LAUNCH, *argv], input=step.get("stdin", "").encode(), capture_output=True,
                              env=_env(root, columns, case.get("env", {})), timeout=120, cwd=str(root))
        norm = lambda b: b.decode("utf-8").replace(str(root), "{ROOT}")                 # noqa: E731
        steps.append({"exit": done.returncode, "stdout": norm(done.stdout), "stderr": norm(done.stderr)})
    files = {}
    for rel in case.get("files", []):
        path = root / rel
        files[rel] = path.read_text(encoding="utf-8").replace(str(root), "{ROOT}") if path.exists() else None
    return {"steps": steps, "files": files}


def _answer(case: dict, repo: Path, tmp: Path) -> dict:
    short, long = tmp / "a" / "r", tmp / "bbbbbbbbbbbbbbbbbbbbbbbbb" / "rr"
    first = _run_case(case, short, None, repo)
    second = _run_case(case, long, None, repo)
    if first == second:
        return {**case, "expected": first}
    wide_a = _run_case(case, short, 4000, repo)
    wide_b = _run_case(case, long, 4000, repo)
    if wide_a != wide_b:
        raise SystemExit(f"case {case['name']} is not deterministic:\n{json.dumps(wide_a, indent=1)}\n{json.dumps(wide_b, indent=1)}")
    return {**case, "columns": 4000, "expected": wide_a}


def _config_text(extra: str = "") -> str:
    return extra


def _cases() -> list[dict]:
    import typer
    from versevision.cli import app
    cases: list[dict] = []

    def add(name, steps, setup=None, files=None, env=None):
        cases.append({"name": name, "setup": setup or [], "steps": [s if isinstance(s, dict) else {"argv": s} for s in steps],
                      "files": files or [], "env": env or {}})

    root = typer.main.get_command(app)

    def walk(cmd, path):
        yield path, cmd
        if hasattr(cmd, "commands"):
            for n, c in cmd.commands.items():
                yield from walk(c, path + [n])

    for path, _cmd in walk(root, []):
        add("help-" + ("-".join(path) or "root"), [[*path, "--help"]])
    add("version", [["--version"]])
    add("root-no-args", [[]])
    add("bad-command", [["frobnicate"]])
    add("bad-option", [["history", "--frobnicate"]])
    add("missing-argument", [["config", "set", "points.bullet_char"]])
    add("bad-int", [["run", "--text", "x", "--add-line", "abc"]])

    # config
    add("config-init", [["config", "init"], ["config", "init"], ["config", "init", "--force"]], files=["config.toml"])
    add("config-show", [["config", "show"], ["config", "show", "--output", "json"], ["config", "show", "--output", "JSON"]])
    add("config-show-after-init", [["config", "init"], ["config", "show", "--output", "json"]])
    add("config-options", [["config", "options"], ["config", "options", "notes"], ["config", "options", "points"],
                           ["config", "options", "verses"], ["config", "options", "slides"], ["config", "options", "bogus"]])
    add("config-set", [["config", "set", "notes.line_breaks", "paragraph"], ["config", "set", "points.bullet_char", "-"],
                       ["config", "set", "notes.extra_cue_words", '["pause", "amen"]'], ["config", "set", "slides.include_verse_numbers", "no"],
                       ["config", "show", "--output", "json"]], files=["config.toml"])
    add("config-set-errors", [["config", "set", "nonsense", "1"], ["config", "set", "notes.nothing", "1"],
                              ["config", "set", "notes.line_breaks", "sideways"], ["config", "set", "slides.max_lines", "many"],
                              ["config", "set", "notes.skip_patterns", '["("]']], files=["config.toml"])
    add("config-invalid-file", [["config", "show"], ["config", "set", "notes.line_breaks", "slide"]],
        setup=[{"op": "write", "path": "config.toml", "text": "[notes]\nline_breaks = 'sideways'\n"}])
    add("config-broken-toml", [["config", "show"]], setup=[{"op": "write", "path": "config.toml", "text": "[notes\n"}])
    add("config-rich-file", [["config", "show", "--output", "json"], ["config", "options", "notes"]],
        setup=[{"op": "copy", "from": "oracle/config_rich.toml", "to": "config.toml"}])

    # versions, parse
    add("versions-list", [["versions", "list"], ["versions", "list", "--output", "json"]])
    add("parse-text", [["parse", "--text", "John 3:16; Romans 8:28 ESV; Gen 1:1-3"], ["parse", "--text", "John 3:16 NIV", "--output", "json"]])
    add("parse-issues", [["parse", "--text", "Hezekiah 3:99 and John 3:16"], ["parse", "--text", "Hezekiah 3:99", "--output", "json"]])
    add("parse-errors", [["parse"], ["parse", "--text", "x", "--output", "xml"], ["parse", "{ROOT}/missing.txt"], ["parse", "a.txt", "--text", "x"]])
    add("parse-stdin", [{"argv": ["parse", "-"], "stdin": "See Psalm 23:1 and Acts 2:38 (KJV).\n"}])
    add("parse-file", [["parse", "{ROOT}/notes.txt"], ["parse", "{ROOT}/notes.md", "--output", "json"]],
        setup=[{"op": "write", "path": "notes.txt", "text": "Mark 1:15\nLuke 2:10-11\n"}, {"op": "write", "path": "notes.md", "text": "# Title\n\nJohn 1:1\n"}])
    add("cache-clear", [["cache", "clear"]])

    # history, log, explain, replay (seeded from the S6b scenarios)
    runs = [{"op": "seed_runs", "scenario": "undo"}, {"op": "seed_history", "scenario": "undo"}]
    add("history-empty", [["history"], ["history", "--output", "json"]])
    add("history-seeded", [["history"], ["history", "--output", "json"]], setup=[{"op": "seed_history", "scenario": "as-copy"}])
    add("history-undo-scenario", [["history"]], setup=[{"op": "seed_history", "scenario": "undo"}])
    add("log-list", [["log", "list"], ["log", "list", "-n", "3"], ["log", "list", "--count", "2", "--output", "json"], ["log"]], setup=runs)
    add("log-list-empty", [["log", "list"], ["log"], ["log", "show"], ["log", "list", "--output", "json"]])
    add("log-show", [["log", "show"], ["log", "show", "--events"], ["log", "show", "--problems"], ["log", "show", "--path"],
                     ["log", "show", "nosuchrun"]], setup=runs)
    add("explain-basic", [["explain"], ["explain", "--output", "json"], ["explain", "--line", "4"], ["explain", "-l", "2"],
                          ["explain", "--tier", "error"], ["explain", "--tier", "good"], ["explain", "nosuchrun"]],
        setup=[{"op": "seed_runs", "scenario": "explain-replay"}])
    add("explain-no-runs", [["explain"], ["replay"], ["replay", "--recent", "2"]])
    add("replay-basic", [["replay"], ["replay", "--todays-options"], ["replay", "-n", "2"], ["replay", "nosuchrun"]],
        setup=[{"op": "seed_runs", "scenario": "explain-replay"}])
    add("replay-many", [["replay", "--recent", "3"]], setup=runs)
    add("explain-failed-run", [["explain"], ["replay"]], setup=[{"op": "seed_runs", "scenario": "target-errors"}])
    for name in ("dry-run", "as-copy", "replace"):
        add("explain-" + name, [["explain"], ["replay"], ["log", "show"]], setup=[{"op": "seed_runs", "scenario": name}])

    # themes (the fixture themes live where `node.theme_root` points)
    theme_setup = [{"op": "copy", "from": "python/tests/fixtures/themes", "to": "themes"},
                   {"op": "write", "path": "config.toml", "text": '[node]\ntheme_root = "{ROOT}/themes"\n'}]
    add("themes-list", [["themes"], ["themes", "--output", "json"]], setup=theme_setup)
    add("themes-use", [["themes", "use", "--verses", "The biblke", "--points", "Sermon Points"], ["themes"],
                       ["themes", "use", "--clear"], ["themes"]], setup=theme_setup, files=["config.toml"])
    add("themes-errors", [["themes", "use"], ["themes", "use", "--verses", "Nope"], ["themes", "use", "--verses", "The biblke", "--verse-slide", "99"],
                          ["themes", "use", "--points", "Scripture"]], setup=theme_setup, files=["config.toml"])
    add("themes-no-folder", [["themes"], ["themes", "use", "--verses", "X"]],
        setup=[{"op": "write", "path": "config.toml", "text": '[node]\ntheme_root = "{ROOT}/nothing-here"\n'}])
    # ProPresenter is not running: what the commands say
    refused = [{"op": "write", "path": "config.toml", "text": '[propresenter]\nhost = "127.0.0.1"\nport = 1\ntimeout_seconds = 2\n'}]
    for name, argv in (("presentations", ["presentations"]), ("presentations-json", ["presentations", "--output", "json"]),
                       ("playlists", ["playlists"]), ("node", ["node"]), ("doctor", ["doctor"]),
                       ("propresenter-libraries", ["propresenter", "libraries"]), ("template-check", ["template", "check"]),
                       ("template-validate", ["template", "validate"]), ("template-inspect", ["template", "inspect", "--target-presentation", "x"]),
                       ("template-setup", ["template", "setup"]), ("undo-unknown", ["undo", "abc123"]),
                       ("undo-unknown-json", ["undo", "abc123", "--output", "json"])):
        add("offline-" + name, [argv], setup=refused)
    add("providers-none", [["providers", "test"]])
    # run: errors that need no ProPresenter
    notes = {"op": "write", "path": "notes.txt", "text": "Title 11/8/26\n\n1) ONE\nJohn 3:16 NIV\n"}
    add("run-argument-errors", [["run"], ["run", "{ROOT}/notes.txt", "--text", "x"], ["run", "--text", "John 3:16", "--replace"],
                                ["run", "--text", "John 3:16", "-p", "Sample"], ["run", "--text", "John 3:16", "-p", "S", "--replace", "--append"],
                                ["run", "{ROOT}/missing.txt"], ["run", "--text", "x", "--set", "bogus"], ["run", "--text", "x", "--set", "notes.line_breaks=sideways"],
                                ["run", "--text", "x", "--profile", "nope"], ["run", "--text", "x", "--output", "json", "--profile", "nope"]],
        setup=[notes], env={})
    add("run-offline", [["run", "{ROOT}/notes.txt", "--dry-run"], ["run", "{ROOT}/notes.txt", "--dry-run", "--output", "json"]],
        setup=[notes, *refused])
    add("import-errors", [["import", "--text", "John 3:16"], ["import", "--text", "John 3:16", "--no-fetch"], ["import"],
                          ["import", "--text", "John 3:16", "--dry-run", "--no-fetch"]])
    add("template-select-errors", [["template", "select-point"], ["template", "validate"]])
    add("group-no-args", [["config"], ["versions"], ["providers"], ["cache"], ["template"], ["propresenter"], ["autostart"]])
    add("import-dry-run", [["import", "--text", "John 3:16 NIV; Romans 8:28 KJV", "--dry-run", "--no-fetch"],
                           ["import", "--text", "John 3:16 NIV", "--dry-run", "--no-fetch", "--output", "json"],
                           ["import", "--text", "John 3:16", "--default-version", "ESV", "--dry-run", "--no-fetch"],
                           ["import", "--text", "John 3:16", "--dry-run", "--no-fetch", "--non-interactive"],
                           ["import", "--text", "Nonsense 99:99 NIV", "--dry-run", "--no-fetch"]])
    add("import-notes-dry-run", [["import", "--notes", "--text", "1) ONE\nJohn 3:16 NIV\nsome stray words here\n", "--dry-run", "--no-fetch"],
                                 ["import", "--notes", "--text", "1) ONE\nJohn 3:16 NIV\n", "--dry-run", "--no-fetch", "--output", "json"]])
    add("autostart-status", [["autostart", "status", "--output", "json"]])
    add("autostart-enable-errors", [["autostart", "enable", "--host", "0.0.0.0"], ["autostart", "enable", "--port", "0"],
                                    ["autostart", "enable", "--port", "70000"]])
    return cases


def _render_cases(repo: Path) -> list[dict]:
    """`_print_report` and `_print_notes_report` on the reports of the S6b scenarios."""
    from rich.console import Console
    from versevision import cli_run
    out = []
    base = repo / "oracle" / "pipeline" / "scenarios"
    for scenario in sorted(p.name for p in base.iterdir()):
        runs = base / scenario / "expected" / "runs"
        if not runs.exists():
            continue
        for run in sorted(runs.iterdir()):
            report_file = run / "report.json"
            if not report_file.exists():
                continue
            report = json.loads(report_file.read_text(encoding="utf-8"))
            if "summary" not in report:
                continue
            import io
            for width in (80, 100):
                buf = io.StringIO()
                saved = cli_run.console
                cli_run.console = Console(file=buf, width=width, force_terminal=False, color_system=None, highlight=False) if False else Console(file=buf, width=width)
                try:
                    cli_run._print_report(report)
                finally:
                    cli_run.console = saved
                out.append({"name": f"{scenario}/{run.name}", "width": width, "expected": buf.getvalue()})
    return out


def _autostart_cases() -> dict:
    from unittest import mock
    from versevision import autostart
    with mock.patch.object(Path, "home", return_value=Path("/home/u")):
        return _autostart_cases_inner(autostart)


def _autostart_cases_inner(autostart) -> dict:
    out = {"args": [], "plist": [], "systemd": [], "windows": []}
    arg_sets = [["/usr/local/bin/versevision", "serve", "--host", "127.0.0.1", "--port", "8777", "--no-open"],
                ["/Users/me/Applications/Verse Vision/versevision", "serve", "--host", "127.0.0.1", "--port", "9000", "--no-open"],
                ["C:\\Program Files\\VerseVision\\versevision.exe", "serve", "--host", "localhost", "--port", "8777", "--no-open"],
                ["/opt/we\"ird/versevision", "serve", "--host", "::1", "--port", "1", "--no-open"],
                ["/home/ünï/bin/versevision", "serve", "--host", "127.0.0.1", "--port", "65535", "--no-open"]]
    for args in arg_sets:
        for log in ("/Users/me/Library/Logs/versevision/serve.log", "/home/u/.local/state/versevision/log/serve.log"):
            out["plist"].append({"args": args, "log": log, "expected": autostart.launch_agent_plist(args, log).decode("utf-8")})
        out["systemd"].append({"args": args, "expected": autostart.systemd_unit(args)})
        out["windows"].append({"args": args, "expected": autostart.windows_task_command(args)})
    return out


def _dirs_cases() -> list[dict]:
    from platformdirs.macos import MacOS
    from platformdirs.unix import Unix
    from platformdirs.windows import Windows
    out = []
    for label, cls, env in (("linux", Unix, {"home": "/home/u"}), ("macos", MacOS, {"home": "/Users/u"}),
                            ("windows", Windows, {"home": "C:\\Users\\u", "local": "C:\\Users\\u\\AppData\\Local"})):
        if label == "windows":
            continue                      # platformdirs.windows needs the registry/env of a real Windows machine: left unverified
        saved_home = os.environ.get("HOME")
        saved = {k: os.environ.pop(k, None) for k in ("XDG_DATA_HOME", "XDG_CONFIG_HOME", "XDG_STATE_HOME", "XDG_CACHE_HOME")}
        os.environ["HOME"] = env["home"]
        try:
            import platformdirs
            if label == "macos":
                dirs = MacOS("versevision")
                row = {"config": dirs.user_config_dir, "data": dirs.user_data_dir, "logs": dirs.user_log_dir, "cache": dirs.user_cache_dir}
            else:
                dirs = Unix("versevision")
                row = {"config": dirs.user_config_dir, "data": dirs.user_data_dir, "logs": dirs.user_log_dir, "cache": dirs.user_cache_dir}
        finally:
            os.environ["HOME"] = saved_home or ""
            for k, v in saved.items():
                if v is not None:
                    os.environ[k] = v
        out.append({"os": label, "home": env["home"], **row})
    return out


def cli_oracle(out: Path, repo: Path) -> int:
    target = out / "cli"
    if target.exists():
        shutil.rmtree(target)
    target.mkdir(parents=True)
    cases = _cases()
    with tempfile.TemporaryDirectory(prefix="vv-cli-oracle-") as tmp:
        tmp_path = Path(tmp)
        def work(item):
            index, case = item
            return _answer(case, repo, tmp_path / f"c{index}")
        with ThreadPoolExecutor(max_workers=8) as pool:
            answered = list(pool.map(work, enumerate(cases)))
    answered.sort(key=lambda c: c["name"])
    with (target / "cases.jsonl").open("w", encoding="utf-8", newline="\n") as stream:
        for case in answered:
            stream.write(json.dumps(case, ensure_ascii=False, sort_keys=True) + "\n")
    (target / "render.jsonl").write_text("".join(json.dumps(r, ensure_ascii=False, sort_keys=True) + "\n" for r in _render_cases(repo)), encoding="utf-8")
    (target / "autostart.json").write_text(json.dumps(_autostart_cases(), ensure_ascii=False, indent=1, sort_keys=True) + "\n", encoding="utf-8")
    (target / "dirs.json").write_text(json.dumps(_dirs_cases(), ensure_ascii=False, indent=1, sort_keys=True) + "\n", encoding="utf-8")
    return len(answered)


if __name__ == "__main__":
    sys.path.insert(0, str(PY_ROOT / "src"))
    out_dir = Path(sys.argv[1]) if len(sys.argv) > 1 else PY_ROOT.parent / "oracle"
    print(f"cli: {cli_oracle(out_dir, PY_ROOT.parent)} cases")
