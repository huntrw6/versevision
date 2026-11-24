"""The oracle of the web app's data routes (S7a): what the Python server answers for the routes that only read or write data, for the
Rust router (``crates/versevision-web/tests/oracle_web.rs``) to answer the same.

Written by ``export_oracle.py`` into ``oracle/web/`` (never by hand)::

    oracle/web/state/            the inputs both sides start from: config.toml, imports.json (the history), logs/runs/<id>/...
    oracle/web/routes.json       [{name, method, path, body?, status, json}] in the order they were asked, on one server (so a config
                                 saved by one step is what the next read shows); the scratch folder is written ``<ROOT>``
    oracle/web/config_apply.jsonl  ``configpage.apply`` for a set of values: the error message or the values every option has after it

The routes that need ProPresenter (``check``, ``presentations``, ``themes``) are answered by a small fake transport (below) that the Rust
test has a twin of. Nothing here is random or timed: every id, time and name is written out.
"""
from __future__ import annotations

import json
import os
import shutil
import sys
import tempfile
import threading
import urllib.error
import urllib.request
from datetime import datetime, timezone
from pathlib import Path

ROOT_TOKEN = "<ROOT>"

CONFIG_EXTRA = """
[profiles.short.points]
max_lines = 3

[profiles.long.slides]
max_body_lines = 8
"""

HISTORY = [
    dict(import_id="imp0000001", created=datetime(2026, 11, 1, 10, 0, 0, tzinfo=timezone.utc), source_name="sermon-1.docx", source_sha256="a" * 64,
         target_uuid="P1", target_name="Sample", node="local", slide_count=7, verse_count=3, point_count=4, status="active", mode="new", library="Verses"),
    dict(import_id="imp0000002", created=datetime(2026, 11, 8, 9, 30, 15, 123456, tzinfo=timezone.utc), source_name="sermon-2.pdf", source_sha256="b" * 64,
         target_uuid="P1", target_name="Sample", node="local", slide_count=12, verse_count=5, point_count=7, status="replaced", mode="replace", library="Verses"),
    dict(import_id="imp0000003", created=datetime(2026, 11, 15, 8, 0, 0, tzinfo=timezone.utc), source_name="sermon-3.txt", source_sha256="c" * 64,
         target_uuid="P2", target_name="Weekly", node="local", slide_count=3, status="undone", mode="append"),
]

RUNS = {
    "20261101-100000-aaaaaa": [
        {"step": "run_start", "level": "info", "source": "sermon-1.docx", "mode": "new", "started": "2026-11-01T10:00:00-08:00"},
        {"step": "run_end", "level": "info", "outcome": "ok"},
    ],
    "20261108-093015-bbbbbb": [
        {"step": "run_start", "level": "info", "source": "sermon-2.pdf", "mode": "replace", "started": "2026-11-08T09:30:15-08:00"},
        {"step": "run_end", "level": "info", "outcome": "failed"},
    ],
    "20261115-080000-cccccc": [
        {"step": "run_start", "level": "info", "source": "sermon-3.txt", "kind": "web", "started": "2026-11-15T08:00:00-08:00"},
    ],
}

PRESENTATIONS = {"/v1/libraries": [{"uuid": "L1", "name": "Verses"}, {"uuid": "L2", "name": "Songs"}],
                 "/v1/library/L1": {"items": [{"uuid": "P1", "name": "Sample"}, {"uuid": "P2", "name": "Weekly"}]},
                 "/v1/library/L2": {"items": [{"uuid": "S1", "name": "Amazing Grace"}]}}

# values for configpage.apply: (name, values)
APPLY_CASES = [
    ("lines", {"slides.max_body_lines": 4, "points.max_lines_per_slide": 2}),
    ("bool and list", {"points.strip_trailing_period": False, "notes.extra_cue_words": ["pause", " sing ", ""]}),
    ("list from text", {"notes.extra_cue_words": "one\n two \n\n"}),
    ("default version", {"bible.default_version": "NKJV"}),
    ("default version cleared", {"bible.default_version": ""}),
    ("number as text", {"slides.max_body_lines": "6", "slides.even_split_tolerance": "0.3"}),
    ("truthy text", {"points.fix_label_typos": "no"}),
    ("choice", {"notes.line_breaks": "paragraph", "points.numbering": "renumber"}),
    ("too many lines", {"slides.max_body_lines": 99}),
    ("zero lines", {"slides.max_body_lines": 0}),
    ("not a choice", {"notes.line_breaks": "sideways"}),
    ("tolerance too big", {"slides.even_split_tolerance": 1.5}),
    ("not editable", {"propresenter.host": "evil.example"}),
    ("hidden", {"slides.max_characters": 10}),
    ("unknown", {"nothing.here": 1}),
    ("not a number", {"slides.max_body_lines": "many"}),
    ("float for int", {"slides.max_body_lines": 3.9}),
    ("first error wins", {"slides.max_body_lines": 99, "points.max_lines": 99}),
    ("library", {"output.library": "Sermons"}),
    ("backups", {"presentation.backup_count": 100}),
    ("too many backups", {"presentation.backup_count": 101}),
]


class FakeTransport:
    """The ProPresenter the data routes ask: its version, its libraries and the theme files of the repository's fixtures."""
    description = "fake ProPresenter"

    def __init__(self, themes: Path):
        self.themes = themes

    def api_get(self, route):
        if route == "/version":
            return {"host_description": "ProPresenter 19.0", "api_version": "v1"}
        if route in PRESENTATIONS:
            return PRESENTATIONS[route]
        raise KeyError(route)

    def is_running(self):
        return True

    def fetch_themes(self, dest: Path):
        shutil.copytree(self.themes, dest, dirs_exist_ok=True)


def _write_state(state: Path, repo: Path) -> None:
    from versevision.history import HistoryStore, ImportRecord
    template = (repo / "oracle" / "pipeline" / "scenarios" / "new" / "expected" / "config.toml").read_text(encoding="utf-8")
    template = template.replace("<ROOT>", "/library").replace("port = <PORT>", "port = 50001")
    template = template.replace('[node]\ntype = "local"', '[node]\nname = "Sanctuary Mac"\ntype = "local"')
    (state / "config.toml").write_text(template.rstrip("\n") + "\n" + CONFIG_EXTRA, encoding="utf-8")
    store = HistoryStore(state / "imports.json")
    for row in HISTORY:
        store.add(ImportRecord(**row))
    for name, events in RUNS.items():
        folder = state / "logs" / "runs" / name
        folder.mkdir(parents=True, exist_ok=True)
        (folder / "events.jsonl").write_text("".join(json.dumps(e, sort_keys=True) + "\n" for e in events), encoding="utf-8")
    (state / "imports.json.lock").unlink(missing_ok=True)
    (state / "logs" / "runs" / "20261101-100000-aaaaaa" / "summary.txt").write_text("VerseVision run 20261101-100000-aaaaaa\nOutcome: ok\n", encoding="utf-8")


def _call(base: str, method: str, path: str, body=None):
    data = None if body is None else json.dumps(body).encode()
    request = urllib.request.Request(base + path, method=method, data=data, headers={"Content-Type": "application/json"} if data else {})
    try:
        with urllib.request.urlopen(request, timeout=10) as response:
            return response.status, response.read()
    except urllib.error.HTTPError as exc:
        return exc.code, exc.read()


def _normalise(value, root: str):
    text = json.dumps(value, ensure_ascii=False)
    return json.loads(text.replace(root, ROOT_TOKEN))


def web_oracle(out: Path, repo: Path) -> int:
    """Writes ``<out>/web`` and returns how many routes and apply cases it holds."""
    from versevision import config as config_module
    from versevision.config import AppConfig, load_config
    from versevision.errors import ConfigurationError
    from versevision.web import configpage
    from versevision.web import server as web

    target = out / "web"
    if target.exists():
        shutil.rmtree(target)
    state_dir = target / "state"
    state_dir.mkdir(parents=True)
    _write_state(state_dir, repo)
    themes = repo / "python" / "tests" / "fixtures" / "themes"

    scratch = Path(tempfile.mkdtemp(prefix="vv-web-oracle-"))
    saved = {k: os.environ.get(k) for k in ("VERSEVISION_CONFIG", "VERSEVISION_HISTORY", "VERSEVISION_LOGS")}
    try:
        work = scratch / "state"
        shutil.copytree(state_dir, work)
        os.environ.update(VERSEVISION_CONFIG=str(work / "config.toml"), VERSEVISION_HISTORY=str(work / "imports.json"), VERSEVISION_LOGS=str(work / "logs"))
        root = str(work)

        # configpage.apply over a set of values: the error, or what every option shows afterwards
        rows = []
        for name, values in APPLY_CASES:
            shutil.copyfile(state_dir / "config.toml", work / "config.toml")
            row = {"name": name, "values": values}
            try:
                new = configpage.apply(load_config(), dict(values))
                shown = {f["key"]: f["value"] for s in configpage.describe(new)["sections"] for f in s["fields"]}
                row.update(error=None, after=shown, saved=load_config().model_dump(mode="json") == new.model_dump(mode="json"))
            except ConfigurationError as exc:
                row.update(error=str(exc), after=None)
            except (ValueError, KeyError) as exc:             # what the server turns into "Bad request: ..."
                row.update(error=f"Bad request: {exc}", after=None)
            rows.append(row)
        shutil.copyfile(state_dir / "config.toml", work / "config.toml")
        (target / "config_apply.jsonl").write_text("".join(json.dumps(r, sort_keys=True, ensure_ascii=False) + "\n" for r in rows), encoding="utf-8")

        # the routes, on one server
        server, app_state = web.serve(load_config(), "127.0.0.1", 0)
        app_state.transport = lambda: FakeTransport(themes)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        base = f"http://127.0.0.1:{server.server_address[1]}"
        steps = [
            ("status", "GET", "/api/status", None),
            ("config", "GET", "/api/config", None),
            ("history", "GET", "/api/history", None),
            ("runs", "GET", "/api/runs", None),
            ("run found", "GET", "/api/run/20261101-100000-aaaaaa", None),
            ("run by ending", "GET", "/api/run/aaaaaa", None),
            ("run without a summary", "GET", "/api/run/20261108-093015-bbbbbb", None),
            ("run unknown", "GET", "/api/run/nope", None),
            ("job unknown", "GET", "/api/jobs/zzz", None),
            ("route unknown", "GET", "/api/nothing", None),
            ("check", "GET", "/api/check", None),
            ("presentations", "GET", "/api/presentations", None),
            ("themes", "GET", "/api/themes", None),
            ("config save", "POST", "/api/config", {"values": {"points.max_lines_per_slide": 2, "bible.default_version": "NKJV"}}),
            ("config too many lines", "POST", "/api/config", {"values": {"slides.max_body_lines": 99}}),
            ("config not editable", "POST", "/api/config", {"values": {"propresenter.host": "evil.example"}}),
            ("config empty", "POST", "/api/config", {}),
            ("status after", "GET", "/api/status", None),
            ("config after", "GET", "/api/config", None),
            ("replan without a review", "POST", "/api/replan", {"job": "zzz", "edits": {}}),
            ("apply without a review", "POST", "/api/apply", {"job": "zzz"}),
            ("plan with the wrong type", "POST", "/api/plan", None),
        ]
        out_rows = []
        for name, method, path, body in steps:
            if path == "/api/plan":                              # a file type that is not accepted
                request = urllib.request.Request(base + path, method="POST", data=b"x", headers={"X-File-Name": "notes.exe"})
                try:
                    status, raw = 200, urllib.request.urlopen(request, timeout=10).read()
                except urllib.error.HTTPError as exc:
                    status, raw = exc.code, exc.read()
            else:
                status, raw = _call(base, method, path, body)
            row = {"name": name, "method": method, "path": path, "status": status, "json": _normalise(json.loads(raw or b"null"), root)}
            if body is not None:
                row["body"] = body
            out_rows.append(row)
        server.shutdown()
        (target / "routes.json").write_text(json.dumps(out_rows, indent=1, sort_keys=True, ensure_ascii=False) + "\n", encoding="utf-8")
        return len(out_rows) + len(rows)
    finally:
        for key, value in saved.items():
            if value is None:
                os.environ.pop(key, None)
            else:
                os.environ[key] = value
        shutil.rmtree(scratch, ignore_errors=True)


if __name__ == "__main__":
    REPO = Path(__file__).resolve().parents[2]
    print(web_oracle(REPO / "oracle", REPO), "web cases")
