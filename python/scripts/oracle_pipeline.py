"""The oracle of the ProPresenter side (S6b): what the Python app asks ProPresenter, writes, records and reads back for a whole import, for the
Rust port to repeat request for request.

Written by ``export_oracle.py`` into ``oracle/pipeline/`` (never by hand). Everything that is random or timed is made repeatable:

* **Time** is virtual. ``time.sleep`` only moves a clock (``VClock``); ``monotonic``, ``datetime.now`` and the run log's ``elapsed`` follow it, so a
  90 second playlist convergence takes no time and every request carries the virtual second it was made at.
* **Ids** are seeded: the n-th id is the version-4 uuid made from ``sha256("<seed>:<n>")`` (the writer's ``new_uuid`` and the pipeline's
  ``uuid4`` draw from one sequence); ``os.urandom`` of the run log is made the same way.
* **ProPresenter** is a scripted fake HTTP server (``FakePP``, below; ``crates/versevision-pp/tests/common`` has the Rust twin that follows the
  same rules, which ``oracle/pipeline/README.md`` writes down). It lists the files of the library folders, hands out ids on a schedule (a new file
  lands with an id that is re-issued some seconds later), answers 404 for an id it has not loaded, and takes playlists with a 204.
* **Bible text** is made up (``synthetic``), recorded into ``bible.json``, and served back by both sides.

Layout::

    oracle/pipeline/scenarios/<name>/scenario.json    the input (see ``README.md``)
    oracle/pipeline/scenarios/<name>/expected/        what the Python app did:
        result.json     the steps' answers, every HTTP request (virtual time, method, path, body, status), every file operation, the final files
        history.json    imports.json byte for byte (root made ``<ROOT>``)
        audit.jsonl     the audit log
        runs/<n>/...    every run folder's files (events.jsonl, trace.jsonl, options.json, report.json, source.txt, summary.txt)
        config.toml     the config the run used;   bible.json   the verse text served
    oracle/pipeline/selection.jsonl                   WRatio / extract answers of rapidfuzz (the near-miss hints)
"""
from __future__ import annotations

import hashlib
import json
import os
import shutil
import sys
import tempfile
import threading
import time as _real_time
import uuid as _real_uuid
from datetime import datetime, timedelta, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from types import SimpleNamespace
from urllib.parse import unquote

REPO = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(Path(__file__).resolve().parent))
FIXTURES = REPO / "python" / "tests" / "fixtures"

import tomli_w  # noqa: E402

from versevision import runlog  # noqa: E402
from versevision.config import AppConfig, PresentationTemplateConfig, save_config  # noqa: E402
from versevision.history import HistoryStore  # noqa: E402
from versevision.presentation.template import inspect_point_candidates, inspect_template_candidates  # noqa: E402

BASE_EPOCH = 1784419571
PREFIX = "versevision-"


def dumps(obj) -> str:
    return json.dumps(obj, indent=2, sort_keys=True, ensure_ascii=False) + "\n"


# ------------------------------------------------------------------------------------------------------------ virtual time and ids
class VClock:
    """Moves only when somebody sleeps."""

    def __init__(self, seed: str):
        self.elapsed = 0.0
        self.mono0 = 1000.0
        self.seed = seed
        self.rand = 0

    def monotonic(self) -> float:
        return self.mono0 + self.elapsed

    def sleep(self, seconds: float) -> None:
        if seconds > 0:
            self.elapsed += seconds

    def micros(self) -> int:
        return BASE_EPOCH * 1_000_000 + round(self.elapsed * 1_000_000)

    def utc(self) -> datetime:
        return datetime(1970, 1, 1, tzinfo=timezone.utc) + timedelta(microseconds=self.micros())

    def urandom(self, n: int) -> bytes:
        digest = hashlib.sha256(f"{self.seed}:{self.rand}".encode()).digest()
        self.rand += 1
        return (digest * 2)[:n]


class VTime(SimpleNamespace):
    """Stands in for the ``time`` module of the modules under test."""

    def __init__(self, clock: VClock):
        super().__init__(monotonic=clock.monotonic, sleep=clock.sleep, time=lambda: BASE_EPOCH + clock.elapsed)


def vdatetime(clock: VClock):
    class VDatetime(datetime):
        @classmethod
        def now(cls, tz=None):
            moment = clock.utc()
            return moment if tz is not None else moment.replace(tzinfo=None)

    return VDatetime


class VOS:
    """``os`` with the run log's randomness made repeatable."""

    def __init__(self, clock: VClock):
        self._clock = clock

    def __getattr__(self, name):
        return getattr(os, name)

    def urandom(self, n: int) -> bytes:
        return self._clock.urandom(n)


def seeded_uuids(seed: str):
    counter = [0]

    def new_uuid() -> _real_uuid.UUID:
        digest = hashlib.sha256(f"{seed}:{counter[0]}".encode()).digest()[:16]
        counter[0] += 1
        return _real_uuid.UUID(bytes=digest, version=4)

    return new_uuid


# ------------------------------------------------------------------------------------------------------------ the scripted ProPresenter
class Entry:
    def __init__(self, library: str, name: str, ids: list, listed_from: float, blank_until: float, loaded_from):
        self.library, self.name = library, name
        self.ids = ids                      # [(valid from, id)]
        self.listed_from, self.blank_until, self.loaded_from = listed_from, blank_until, loaded_from

    def current(self, t: float) -> str:
        found = ""
        for start, ident in self.ids:
            if start <= t:
                found = ident
        return found


class FakePP:
    """ProPresenter's network API as far as VerseVision uses it, scripted. The rules (all times are virtual seconds):

    * a file lands (create / rename target) -> an entry for its name in the library of its folder. It is listed from ``listed_after`` seconds on
      (blank id for the first ``blank_for``), with a fresh id (``<prefix><n>``); it gets another fresh id at each of ``reissue_at``.
    * ``GET /v1/presentation/<id>`` answers 200 for the entry's CURRENT id once it is loaded (``loaded_after`` seconds after landing; ``null`` = never),
      otherwise 404. The slides are read from the file on disk.
    * a renamed file is a new entry under its new name (``rename_landing``); a deleted file's entry is gone at once.
    * ``PUT /v1/playlist/<id>`` stores the items and answers ``put_status`` (204); with ``put_needs_loaded`` a PUT that names an id which is not
      a loaded current id answers 400. ``put_new_item_ids`` gives every item a new id on each save.
    """

    def __init__(self, scenario: dict, root: Path, clock: VClock):
        self.root, self.clock = root, clock
        self.cfg = scenario.get("server", {})
        self.libraries = [{"uuid": f"L{i + 1}", "name": name, "index": i} for i, name in enumerate(scenario["libraries"])]
        self.entries: dict[tuple[str, str], Entry] = {}
        self.next_id = 0
        self.next_item = 0
        self.log: list[dict] = []
        self.lock = threading.Lock()
        self.playlist = json.loads(json.dumps(scenario["playlist"])) if scenario.get("playlist") else None
        self.t0 = 0.0
        for item in scenario["files"]:
            self.entries[(item["library"], item["name"])] = Entry(item["library"], item["name"], [(-1e9, item["id"])], -1e9, -1e9, -1e9)

    # -- events from the file side
    def landing(self, name: str, kind: str) -> dict:
        spec = dict(self.cfg.get("landing", {}))
        if kind == "rename":
            spec.update(self.cfg.get("rename_landing", {}))
        spec.update(self.cfg.get("by_name", {}).get(name, {}))
        return spec

    def on_landed(self, library: str, name: str, kind: str) -> None:
        t = self.clock.elapsed
        spec = self.landing(name, kind)
        self.next_id += 1
        first = f"{spec.get('prefix', 'N')}{self.next_id:04d}"
        ids = [(t + spec.get("listed_after", 0.0), first)]
        for when in spec.get("reissue_at", []):
            self.next_id += 1
            ids.append((t + when, f"{spec.get('prefix', 'N')}{self.next_id:04d}"))
        loaded = spec.get("loaded_after", 0.0)
        self.entries[(library, name)] = Entry(
            library, name, ids, t + spec.get("listed_after", 0.0), t + spec.get("listed_after", 0.0) + spec.get("blank_for", 0.0),
            None if loaded is None else t + loaded)

    def on_removed(self, library: str, name: str) -> None:
        self.entries.pop((library, name), None)

    # -- answers
    def _library_by_uuid(self, uuid: str):
        return next((l for l in self.libraries if l["uuid"] == uuid), None)

    def _listed(self, t: float):
        return [e for e in self.entries.values() if e.listed_from <= t]

    def _slides(self, path: Path) -> list[dict]:
        from versevision.presentation.generated import presentation_pb2
        from versevision.presentation.template import _plain_rtf
        message = presentation_pb2.Presentation()
        message.ParseFromString(path.read_bytes())
        out = []
        for cue in message.cues:
            for action in cue.actions:
                if action.WhichOneof("ActionTypeData") == "slide":
                    parts = [_plain_rtf(e.element.text.rtf_data) for e in action.slide.presentation.base_slide.elements if e.element.HasField("text")]
                    out.append({"enabled": True, "notes": "", "text": " ".join(parts), "label": cue.name})
        return out

    def get(self, path: str):
        t = self.clock.elapsed
        if path == "/version":
            return 200, {"name": "ProPresenter", "platform": "macOS", "os_version": "15.7.9", "host_description": "fake",
                         "api_version": self.cfg.get("api_version", "v1")}
        if path == "/v1/libraries":
            return 200, self.libraries
        if path.startswith("/v1/library/"):
            lib = self._library_by_uuid(unquote(path.rsplit("/", 1)[-1]))
            if lib is None:
                return 404, {"error": "no such library"}
            items = sorted((e for e in self._listed(t) if e.library == lib["name"]), key=lambda e: e.name)
            return 200, {"update_type": "", "items": [
                {"uuid": "" if t < e.blank_until else e.current(t), "name": e.name, "index": i} for i, e in enumerate(items)]}
        if path == "/v1/presentation/active":
            return 404, {"error": "no active presentation"}
        if path.startswith("/v1/presentation/"):
            ident = unquote(path.rsplit("/", 1)[-1])
            for e in self._listed(t):
                if e.current(t) == ident and e.loaded_from is not None and t >= e.loaded_from and t >= e.blank_until \
                        and ident not in self.cfg.get("presentation_404", []):
                    file = self.root / "Libraries" / e.library / f"{e.name}.pro"
                    if not file.is_file():
                        break
                    slides = self._slides(file)
                    limit = self.cfg.get("show_slides")
                    if limit is not None:
                        slides = slides[:limit]
                    return 200, {"presentation": {"id": {"uuid": ident, "name": e.name, "index": 0}, "groups": [{"name": "", "slides": slides}],
                                                  "has_timeline": False, "presentation_path": str(file), "destination": "presentation",
                                                  "has_timeline_items": False}}
            return 404, {"error": "not found"}
        if path == "/v1/playlists":
            if not self.playlist:
                return 200, []
            return 200, [{"id": {"uuid": self.playlist["id"], "name": self.playlist["name"], "index": 0}, "field_type": "playlist", "children": []}]
        if path.startswith("/v1/playlist/") and self.playlist and unquote(path.rsplit("/", 1)[-1]) == self.playlist["id"]:
            return 200, {"id": {"uuid": self.playlist["id"], "name": self.playlist["name"], "index": 0}, "items": self.playlist["items"]}
        return 404, {"error": "unknown route"}

    def put(self, path: str, body):
        t = self.clock.elapsed
        if not (path.startswith("/v1/playlist/") and self.playlist and unquote(path.rsplit("/", 1)[-1]) == self.playlist["id"]):
            return 404, None
        status = self.cfg.get("put_status", 204)
        if status != 204:
            return status, {"error": "refused"}
        if self.cfg.get("put_needs_loaded"):
            for item in body:
                ident = (item.get("presentation_info") or {}).get("presentation_uuid")
                if ident and not any(e.current(t) == ident and e.loaded_from is not None and t >= e.loaded_from for e in self._listed(t)):
                    return 400, {"error": "that presentation is not loaded"}
        if self.cfg.get("put_new_item_ids"):
            for item in body:
                self.next_item += 1
                item["id"]["uuid"] = f"ITEM-{self.next_item:04d}"
        self.playlist["items"] = body
        return 204, None


def serve(fake: FakePP):
    class Handler(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.0"

        def log_message(self, *args):
            return

        def _answer(self, status: int, body):
            data = b"" if body is None else json.dumps(body).encode()
            self.send_response(status)
            if data:
                self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

        def do_GET(self):
            with fake.lock:
                status, body = fake.get(self.path)
                fake.log.append({"t": round(fake.clock.elapsed, 3), "method": "GET", "path": self.path, "body": None, "status": status})
            self._answer(status, body)

        def do_PUT(self):
            raw = self.rfile.read(int(self.headers.get("Content-Length", "0")))
            parsed = json.loads(raw) if raw else None
            with fake.lock:
                status, body = fake.put(self.path, parsed)
                fake.log.append({"t": round(fake.clock.elapsed, 3), "method": "PUT", "path": self.path, "body": parsed, "status": status})
            self._answer(status, body)

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    return server


# ------------------------------------------------------------------------------------------------------------ the recording transport
def sha256_file(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


class LocalTransport:
    """Named like the real one (the pipeline asks the class name where it keeps a backup path). The real LocalTransport, recording every file operation and telling the fake ProPresenter what landed."""

    def __init__(self, real, fake: FakePP, ctx: "Ctx"):
        self.real, self.fake, self.ctx = real, fake, ctx
        self.description = real.description

    def _rel(self, value) -> str:
        text = str(value)
        root = str(self.ctx.root)
        if text.startswith(root):
            return "<ROOT>" + text[len(root):]
        return "<local>/" + Path(text).name

    def _note(self, op: str, args: list, result=None, error: BaseException | None = None) -> None:
        entry = {"t": round(self.ctx.clock.elapsed, 3), "op": op, "args": args}
        if error is not None:
            entry["error"] = f"{type(error).__name__}: {error}"
        elif result is not None:
            entry["result"] = result
        self.ctx.file_ops.append(entry)

    def _call(self, op, args, fn, shown=lambda r: r):
        try:
            result = fn()
        except BaseException as exc:
            self._note(op, args, error=exc)
            raise
        self._note(op, args, result=shown(result))
        return result

    def _hook(self, op: str, path: Path) -> None:
        for hook in self.ctx.scenario.get("hooks", []):
            if hook["after"] == op and not hook.get("done"):
                hook["done"] = True
                source = hook["overwrite_with"]
                data = (FIXTURES / source.split(":", 1)[1]).read_bytes()
                path.write_bytes(data)
                self.ctx.file_ops.append({"t": round(self.ctx.clock.elapsed, 3), "op": "hook:overwrite", "args": [self._rel(path), source]})

    def _lib_name(self, remote: str) -> tuple[str, str]:
        p = Path(remote)
        return p.parent.name, p.stem

    # passthrough
    def api_get(self, route):
        return self.real.api_get(route)

    def api_put(self, route, body):
        return self.real.api_put(route, body)

    def is_running(self):
        return self._call("is_running", [], self.real.is_running)

    def quit_app(self):
        return self._call("quit_app", [], self.real.quit_app)

    def launch_app(self):
        return self._call("launch_app", [], self.real.launch_app)

    def fetch_file(self, remote_path, local_path):
        return self._call("fetch_file", [self._rel(remote_path)], lambda: self.real.fetch_file(remote_path, local_path))

    def install_file(self, local_path, remote_path, expected_remote_sha):
        return self._call("install_file", [self._rel(remote_path)], lambda: self.real.install_file(local_path, remote_path, expected_remote_sha))

    def find_presentation(self, name, library):
        return self._call("find_presentation", [name, library], lambda: self.real.find_presentation(name, library), lambda r: [self._rel(x) for x in r])

    def library_folder(self, library):
        return self.real.library_folder(library)

    def fetch_themes(self, dest):
        return self._call("fetch_themes", [], lambda: self.real.fetch_themes(dest))

    def create_file(self, local_path, remote_path):
        def run():
            sha = self.real.create_file(local_path, remote_path)
            lib, name = self._lib_name(str(remote_path))
            self.fake.on_landed(lib, name, "create")
            self._hook("create_file", Path(remote_path))
            return sha
        return self._call("create_file", [self._rel(remote_path)], run)

    def delete_created_file(self, remote_path, expected_cues):
        def run():
            out = self.real.delete_created_file(remote_path, expected_cues)
            lib, name = self._lib_name(str(remote_path))
            self.fake.on_removed(lib, name)
            return out
        return self._call("delete_created_file", [self._rel(remote_path), expected_cues], run)

    def rename_file(self, old_path, new_path):
        def run():
            self.real.rename_file(old_path, new_path)
            lib, old = self._lib_name(str(old_path))
            _, new = self._lib_name(str(new_path))
            self.fake.on_removed(lib, old)
            self.fake.on_landed(lib, new, "rename")
            self._hook("rename_file", Path(new_path))
        return self._call("rename_file", [self._rel(old_path), self._rel(new_path)], run)

    def delete_file(self, remote_path, expected_sha):
        def run():
            out = self.real.delete_file(remote_path, expected_sha)
            lib, name = self._lib_name(str(remote_path))
            self.fake.on_removed(lib, name)
            return out
        return self._call("delete_file", [self._rel(remote_path)], run)

    def backup_file(self, remote_path):
        return self._call("backup_file", [self._rel(remote_path)], lambda: self.real.backup_file(remote_path), self._rel)

    def restore_backup(self, backup_path, remote_path):
        return self._call("restore_backup", [self._rel(backup_path), self._rel(remote_path)], lambda: self.real.restore_backup(backup_path, remote_path))


# ------------------------------------------------------------------------------------------------------------ one scenario
class Ctx:
    def __init__(self, scenario: dict, root: Path, clock: VClock):
        self.scenario, self.root, self.clock = scenario, root, clock
        self.file_ops: list[dict] = []
        self.steps: list[dict] = []
        self.bible: dict[str, str] = {}
        self.running = scenario.get("running", True)


PORT = [0]


def norm_text(text: str, root: Path) -> str:
    return text.replace(str(root), "<ROOT>").replace(f"127.0.0.1:{PORT[0]}", "127.0.0.1:<PORT>")


def norm_value(value, root: Path):
    if isinstance(value, str):
        return norm_text(value, root)
    if isinstance(value, list):
        return [norm_value(v, root) for v in value]
    if isinstance(value, dict):
        return {k: norm_value(v, root) for k, v in value.items()}
    return value


def build_library(scenario: dict, root: Path) -> None:
    for name in scenario["libraries"]:
        (root / "Libraries" / name).mkdir(parents=True, exist_ok=True)
    for item in scenario["files"]:
        kind, rel = item["source"].split(":", 1)
        source = (FIXTURES / rel) if kind == "fixture" else (REPO / "oracle" / "pro" / "files" / rel)
        shutil.copyfile(source, root / "Libraries" / item["library"] / f"{item['name']}.pro")
    if scenario.get("themes"):
        shutil.copytree(FIXTURES / scenario["themes"].split(":", 1)[1], root / "Themes")
    else:
        (root / "Themes").mkdir(parents=True, exist_ok=True)


def make_config(scenario: dict, root: Path, port: int) -> AppConfig:
    template_file = root / "Libraries" / scenario["template"]["library"] / f"{scenario['template']['name']}.pro"
    verse = inspect_template_candidates(template_file)[0]
    point = inspect_point_candidates(template_file)[0]
    definition = PresentationTemplateConfig(
        presentation_uuid=scenario["template"]["id"], presentation_name=scenario["template"]["name"], cue_uuid=verse.cue_uuid,
        style_fingerprint=verse.style_fingerprint, point_cue_uuid=point[1], point_style_fingerprint=point[2])
    data = AppConfig().model_dump(mode="json")
    data["propresenter"].update(host="127.0.0.1", port=port, timeout_seconds=5.0)
    data["node"].update(library_root=str(root / "Libraries"), theme_root=str(root / "Themes"), backup_dir=str(root / "backups"))
    for section, values in scenario.get("config", {}).items():
        data[section].update(values)
    config = AppConfig.model_validate(data)
    return config.model_copy(update={"presentation_template": definition})


def synthetic_text(version: str, book: str, chapter: int, verse: int) -> str:
    return f"Text of {version} {book} {chapter}:{verse}. " + "More words follow so that a long verse is cut into slides. " * (1 if verse % 2 else 3)


def run_scenario(path: Path, out: Path) -> dict:
    scenario = json.loads(path.read_text(encoding="utf-8"))
    scenario.setdefault("seed", path.parent.name)
    clock = VClock(scenario["seed"])
    root = Path(tempfile.mkdtemp(prefix=PREFIX + "oracle-"))
    try:
        ctx = Ctx(scenario, root, clock)
        build_library(scenario, root)
        fake = FakePP(scenario, root, clock)
        server = serve(fake)
        port = server.server_address[1]
        PORT[0] = port
        config = make_config(scenario, root, port)
        config_path = root / "config.toml"
        save_config(config, config_path)
        result = execute(ctx, fake, config, config_path)
        server.shutdown()
        return collect(ctx, fake, result, out, config_path)
    finally:
        shutil.rmtree(root, ignore_errors=True)


def execute(ctx: Ctx, fake: FakePP, config: AppConfig, config_path: Path) -> list[dict]:
    import asyncio
    import platformdirs
    from typer.testing import CliRunner
    from versevision import cli as cli_module, cli_run, history, logging as vlogging, node as _node  # noqa: F401
    from versevision.bibles import registry as registry_module
    from versevision.bibles.models import BibleVerse, ResolvedPassage
    from versevision.node import transport as transport_module
    from versevision.presentation import backup as backup_module, protobuf_writer
    from versevision.propresenter import process as process_module
    from versevision.workflow import pipeline
    from versevision.errors import BibleProviderError

    clock, root, scenario = ctx.clock, ctx.root, ctx.scenario
    draw = seeded_uuids(scenario["seed"])
    vtime, vdt, vos = VTime(clock), vdatetime(clock), VOS(clock)

    class VUuid:
        uuid4 = staticmethod(draw)

    saved = []

    def patch(obj, name, value):
        saved.append((obj, name, getattr(obj, name)))
        setattr(obj, name, value)

    patch(pipeline, "time", vtime)
    patch(pipeline, "uuid", VUuid)
    # the pauses of the product (the test suite shortens some of them), then what the scenario changes
    timing = {"MIN_GAP_SECONDS": 4.0, "QUIET_SECONDS": 6.0, "PLAYLIST_CONVERGE_SECONDS": 90.0, "LOAD_GRACE_SECONDS": 8.0,
              "STABLE_SECONDS": 4.0, "SETTLE_SECONDS": 15.0, "SWAP_WAIT_SECONDS": 30.0}
    timing.update(scenario.get("timing", {}))
    for name, value in timing.items():
        patch(pipeline, name, value)
    patch(runlog, "time", vtime)
    patch(runlog, "datetime", vdt)
    patch(runlog, "os", vos)
    patch(transport_module, "time", vtime)
    patch(history, "datetime", vdt)
    patch(vlogging, "datetime", vdt)
    patch(protobuf_writer, "new_uuid", draw)
    patch(protobuf_writer, "datetime", vdt)
    patch(backup_module, "datetime", vdt)
    patch(process_module, "is_propresenter_running", lambda: ctx.running)
    patch(platformdirs, "user_log_path", lambda *a, **k: root / "logs")

    async def fake_fetch(self, reference):
        from export_oracle import verse_keys_of
        info = {"reference": reference.normalized_text, "version": reference.version}
        verses = []
        for key, chapter, number in verse_keys_of(reference):
            text = ctx.bible.setdefault(key, synthetic_text(reference.version, reference.book_display_name, chapter, number))
            verses.append(BibleVerse(book_id=reference.book_id, chapter=chapter, verse=number, text=text, version=reference.version))
        runlog.trace("provider", provider="oracle", source="bible.json", ms=0, **info)
        return ResolvedPassage(reference=reference, resolved_version=reference.version, verses=verses)

    patch(registry_module.ProviderRegistry, "fetch", fake_fetch)
    real_make = cli_run.make_transport

    def make(pp, node):
        return LocalTransport(real_make(pp, node), fake, ctx)

    patch(cli_run, "make_transport", make)

    env = {"VERSEVISION_CONFIG": str(config_path), "VERSEVISION_LOGS": str(root / "logs"),
           "VERSEVISION_HISTORY": str(root / "data" / "imports.json")}
    os.environ.update(env)
    runner = CliRunner()
    answers: list[dict] = []
    try:
        for index, step in enumerate(scenario["steps"]):
            answers.append(do_step(ctx, fake, config, config_path, runner, cli_module, step, index, answers, make, pipeline, history))
    finally:
        for obj, name, value in reversed(saved):
            setattr(obj, name, value)
        for key in env:
            os.environ.pop(key, None)
    return answers


def _runs_sorted(root: Path) -> list[str]:
    folder = root / "logs" / "runs"
    return sorted(p.name for p in folder.iterdir() if p.is_dir()) if folder.is_dir() else []


def _json_or_text(text: str):
    try:
        return json.loads(text)
    except ValueError:
        return {"text": text}


def do_step(ctx, fake, config, config_path, runner, cli_module, step, index, answers, make, pipeline, history_module) -> dict:
    root, clock = ctx.root, ctx.clock
    op = step["op"]
    answer: dict = {"op": op}
    if op == "advance":
        clock.sleep(step["seconds"])
    elif op == "running":
        ctx.running = step["value"]
    elif op == "server":
        fake.cfg.update(step["set"])
    elif op == "write_file":
        kind, rel = step["source"].split(":", 1)
        (root / "Libraries" / step["library"] / f"{step['name']}.pro").write_bytes((FIXTURES / rel).read_bytes())
    elif op == "seed_runs":
        folder = root / "logs" / "runs"
        for n in range(step["count"]):
            (folder / f"20200101-{n // 3600:02d}{n // 60 % 60:02d}{n % 60:02d}-aaaaaa").mkdir(parents=True, exist_ok=True)
    elif op == "run":
        notes = root / step.get("file", "notes.txt")
        notes.write_text(step["text"], encoding="utf-8")
        args = ["run", str(notes), "--output", "json"]
        for flag in ("replace", "append", "dry_run", "yes", "no_playlist", "as_copy", "in_place", "no_close", "no_reopen"):
            if step.get(flag):
                args.append("--" + flag.replace("_", "-"))
        for key, option in (("presentation", "-p"), ("title", "--title"), ("library", "--library"), ("default_version", "--default-version"),
                            ("profile", "--profile"), ("presentation_uuid", "--presentation-uuid")):
            if step.get(key):
                args += [option, step[key]]
        for number in step.get("add_line", []):
            args += ["--add-line", str(number)]
        for item in step.get("set", []):
            args += ["--set", item]
        result = runner.invoke(cli_module.app, args)
        answer.update(exit_code=result.exit_code, stdout=_json_or_text(result.stdout))
    elif op == "undo":
        import_id = step.get("import_id") or answers[step["from_step"]]["stdout"]["applied"]["import_id"]
        args = ["undo", import_id, "--output", "json"] + (["--no-close"] if step.get("no_close") else [])
        result = runner.invoke(cli_module.app, args)
        answer.update(exit_code=result.exit_code, stdout=_json_or_text(result.stdout))
    elif op == "history":
        result = runner.invoke(cli_module.app, ["history", "--output", "json"])
        answer.update(exit_code=result.exit_code, stdout=_json_or_text(result.stdout))
    elif op == "log_list":
        result = runner.invoke(cli_module.app, ["log", "list", "--output", "json", "--count", str(step.get("count", 20))])
        answer.update(exit_code=result.exit_code, stdout=_json_or_text(result.stdout))
    elif op in ("explain", "replay"):
        names = _runs_sorted(root)
        rid = names[step["run_index"]] if "run_index" in step else step.get("run", "latest")
        args = [op, rid]
        if op == "explain":
            args += ["--output", "json" if step.get("json") else "terminal"]
            if step.get("line") is not None:
                args += ["--line", str(step["line"])]
            if step.get("tier"):
                args += ["--tier", step["tier"]]
        elif step.get("todays_options"):
            args.append("--todays-options")
        result = runner.invoke(cli_module.app, args)
        answer.update(exit_code=result.exit_code, stdout=_json_or_text(result.stdout) if step.get("json") else {"text": result.stdout})
    elif op == "client":
        from versevision.propresenter.api_client import ProPresenterApiClient
        import asyncio
        pp = config.propresenter

        async def go():
            out = {}
            async with ProPresenterApiClient(pp.host, pp.port, pp.timeout_seconds) as client:
                for call in step["calls"]:
                    try:
                        if call == "version":
                            value = (await client.version()).model_dump()
                        elif call == "libraries":
                            value = [x.model_dump() for x in await client.libraries()]
                        elif call == "presentations":
                            value = [x.model_dump() for x in await client.presentations()]
                        elif call.startswith("presentation:"):
                            value = json.loads((await client.presentation(call.split(":", 1)[1])).model_dump_json())
                        elif call == "playlists":
                            value = [x.model_dump(mode="json") for x in await client.playlists()]
                        elif call.startswith("playlist:"):
                            value = (await client.playlist(call.split(":", 1)[1])).model_dump(mode="json")
                        elif call.startswith("select:"):
                            from versevision.propresenter.selection import select_unique_presentation
                            name = call.split(":", 1)[1]
                            value = select_unique_presentation(await client.presentations(), name=name).model_dump()
                        else:
                            value = None
                    except Exception as exc:
                        value = {"error": f"{type(exc).__name__}: {exc}"}
                    out[call] = value
            return out

        answer["calls"] = asyncio.run(go())
    elif op == "web_plan":
        answer.update(web_plan(ctx, fake, config, step, make, pipeline))
    elif op == "web_apply":
        answer.update(web_apply(ctx, fake, config, step, make, pipeline, answers))
    else:
        raise SystemExit(f"unknown step {op}")
    answer["t"] = round(clock.elapsed, 3)
    return answer


# the web page's plan and apply (``web/server.py: _plan_work`` / ``_apply_work``), run here without the HTTP layer and the jobs
_WEB: dict = {}


def web_plan(ctx, fake, config, step, make, pipeline) -> dict:
    import asyncio
    from versevision.config import load_config
    from versevision.notes.title import document_title
    from versevision.workflow.report import build_report
    cfg = config
    transport = make(cfg.propresenter, cfg.node)
    name = step.get("name", "notes.txt")
    run = runlog.RunLog.start("web", source=name, mode=step["mode"], title=None, library=None, target=None, node=cfg.node.name or "this computer",
                              default_version=step.get("default_version") or cfg.bible.default_version)
    token = runlog.activate(run)
    try:
        run.save_source(step["text"], name)
        plan, tgt = asyncio.run(pipeline.plan_for_transport(step["text"], cfg, transport, default_version=step.get("default_version"),
                                                           target_uuid=step.get("target"), target_name=None, library=None))
        title = document_title(step["text"], name)
        report = build_report(plan, source_name=name)
        report["mode"], report["title"], report["library"] = step["mode"], title, cfg.output.library
        report["target"] = {"name": tgt[2], "library": tgt[0], "uuid": tgt[1]} if tgt else None
        report["node"] = transport.description
        report["replaces"] = None
        report["run_id"] = run.id
        run.save_report(report)
        run.finish("reviewed, not imported yet", report, None, None)
    finally:
        runlog.deactivate(token)
    _WEB.update(plan=plan, target=tgt, report=report, text=step["text"], name=name, title=title, mode=step["mode"], run_id=run.id)
    return {"run_id": run.id, "report_summary": report["summary"], "decisions": [(d["id"], d["line"], d["kind"], d["tier"]) for d in report["decisions"]]}


def web_apply(ctx, fake, config, step, make, pipeline, answers) -> dict:
    from versevision.workflow.notes_workflow import apply_review_choices
    source = SimpleNamespace(**_WEB)
    skip, include, reject = set(step.get("skip", [])), set(step.get("include", [])), set(step.get("reject_fixes", []))
    final_plan = source.plan.model_copy(update={"slide_plan": apply_review_choices(source.plan, skip, include, reject)})
    transport = make(config.propresenter, config.node)
    run = runlog.RunLog.reopen(source.run_id) or runlog.RunLog.start("web", source=source.name, mode=source.mode)
    token = runlog.activate(run)
    run.event("apply_start", message=f"{source.mode}: importing after review", accept_flags=step.get("accept_flags", True))
    outcome, error, result = "failed", None, None
    try:
        def progress(message: str) -> None:
            run.event("progress", message=message)
        if skip:
            run.event("progress", message=f"{len(skip)} decision(s) switched off in the review")
            rows = {d["id"]: d for d in source.report.get("decisions", [])}
            run.trace("review_excluded", decisions=[{"id": i, "line": rows.get(i, {}).get("line"), "kind": rows.get(i, {}).get("kind"),
                                                    "title": rows.get(i, {}).get("title"), "slides": rows.get(i, {}).get("slides")} for i in sorted(skip)],
                      note="decisions you switched off in the review: their slides were not imported")
        apply_review_choices(source.plan, skip, include, reject)
        run.trace("review_accepted", flagged_lines_accepted=step.get("accept_flags", True), slides_imported=len(final_plan.slide_plan.slides))
        if source.mode == "new":
            applied = pipeline.publish_new(source.text, source.name, source.title, final_plan, config, transport, library=None, history=None, progress=progress)
        else:
            applied = pipeline.publish_swap(source.text, source.name, final_plan, config, transport, source.target,
                                            clear_generated=(source.mode == "replace"), history=None, progress=progress)
        result = applied.model_dump()
        outcome = "ok"
    except Exception as exc:
        error = str(exc)
        run.failed(exc)
    finally:
        run.finish(outcome, source.report, result, error)
        runlog.deactivate(token)
    return {"outcome": outcome, "error": error, "applied": result}


# ------------------------------------------------------------------------------------------------------------ collecting the evidence
def tree(root: Path) -> dict:
    out = {}
    base = root / "Libraries"
    for path in sorted(base.rglob("*")):
        if path.is_file() and not path.name.endswith((".lock", ".tmp")):
            out["<ROOT>/Libraries/" + path.relative_to(base).as_posix()] = sha256_file(path)
    return out


def collect(ctx: Ctx, fake: FakePP, answers: list[dict], out: Path, config_path: Path) -> dict:
    root = ctx.root
    expected = out / "expected"
    if expected.exists():
        shutil.rmtree(expected)
    expected.mkdir(parents=True)
    result = {
        "steps": norm_value(answers, root), "requests": norm_value(fake.log, root), "file_ops": norm_value(ctx.file_ops, root),
        "files": tree(root), "running": ctx.running,
        "playlist": fake.playlist["items"] if fake.playlist else None,
    }
    (expected / "result.json").write_text(dumps(result), encoding="utf-8")
    history = root / "data" / "imports.json"
    (expected / "history.json").write_text(norm_text(history.read_text(encoding="utf-8"), root) if history.exists() else "", encoding="utf-8")
    audit = root / "logs" / "audit.jsonl"
    (expected / "audit.jsonl").write_text(norm_text(audit.read_text(encoding="utf-8"), root) if audit.exists() else "", encoding="utf-8")
    runs = root / "logs" / "runs"
    if runs.is_dir():
        for number, folder in enumerate(sorted(p for p in runs.iterdir() if p.is_dir())):
            target = expected / "runs" / f"{number:03d}"
            target.mkdir(parents=True)
            (target / "name").write_text(folder.name, encoding="utf-8")
            for file in sorted(folder.iterdir()):
                text = norm_text(file.read_text(encoding="utf-8"), root)
                if file.name == "events.jsonl":
                    text = "".join(strip_traceback(line) for line in text.splitlines(keepends=True))
                (target / file.name).write_text(text, encoding="utf-8")
    (expected / "config.toml").write_text(
        norm_text(config_path.read_text(encoding="utf-8"), root).replace(f"port = {PORT[0]}", "port = <PORT>"), encoding="utf-8")
    (expected / "bible.json").write_text(dumps(ctx.bible), encoding="utf-8")
    return result


def strip_traceback(line: str) -> str:
    """A traceback holds paths of the machine and line numbers of the source (and the run log clips it): the file keeps that there was one."""
    try:
        event = json.loads(line)
    except ValueError:
        return line
    if isinstance(event, dict) and isinstance(event.get("traceback"), str):
        event["traceback"] = "<traceback>"
        return json.dumps(event) + "\n"
    return line


def selection_cases() -> list[dict]:
    from rapidfuzz import fuzz, process
    names = ["Verses", "Sample", "Sunday Sermon", "Sunday Sermon 2", "Easter 2026", "easter 2026", "Christmas Eve", "The Lord's Prayer", "John 3", "Romans 8",
             "Sermon on the Mount", "A", "AB", "Worship Set 1", "VerseVision Template", "Notes", "Sermon Notes", "Series: Hope", "Hope", "Hebrews 11 faith"]
    queries = ["Verse", "Sermon", "sunday sermon", "Sunday Sermn", "Easter", "Mount", "zzz", "Sample.pro", "hope series", "Hebrews", "John", "Rom", "x", "Template", "The Lords Prayer",
               "Sermon Notes 2", "Christmas", "Verses 2", "V", "Worship"]
    out = []
    for query in queries:
        scores = {n: fuzz.WRatio(query, n) for n in names}
        top = process.extract(query, names, scorer=fuzz.WRatio, limit=3)
        out.append({"query": query, "choices": names, "scores": scores, "top": [[v, s, i] for v, s, i in top]})
    for a in names:
        for b in names[:6]:
            out.append({"query": a, "choices": [b], "scores": {b: fuzz.WRatio(a, b)}, "top": []})
    return out


def pipeline_oracle(out: Path, repo: Path, only: list[str] | None = None) -> int:
    """Runs every scenario of ``<repo>/oracle/pipeline/scenarios`` and writes the evidence under ``<out>/pipeline``. Returns how many ran."""
    source = repo / "oracle" / "pipeline" / "scenarios"
    target = out / "pipeline" / "scenarios"
    count = 0
    for path in sorted(source.glob("*/scenario.json")):
        name = path.parent.name
        if only and name not in only:
            continue
        folder = target / name
        folder.mkdir(parents=True, exist_ok=True)
        if folder / "scenario.json" != path:
            shutil.copyfile(path, folder / "scenario.json")
        run_scenario(folder / "scenario.json", folder)
        count += 1
    if not only:
        (out / "pipeline" / "selection.jsonl").write_text(
            "".join(json.dumps(c, sort_keys=True, ensure_ascii=False) + "\n" for c in selection_cases()), encoding="utf-8")
    return count


if __name__ == "__main__":
    print(pipeline_oracle(REPO / "oracle", REPO, sys.argv[1:] or None), "scenarios")
