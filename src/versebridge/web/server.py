"""Local web page for VerseBridge: drop a notes file, see what was added and what was flagged, then import.

Standard library only. Binds to this computer (127.0.0.1) unless told otherwise; when bound to a
network address it requires the access token printed at startup.
"""
from __future__ import annotations

import asyncio
import json
import secrets
import tempfile
import threading
import traceback
import uuid
from typing import Any
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from importlib import resources
from pathlib import Path
from urllib.parse import parse_qs, urlparse

from versebridge.config import AppConfig, load_config
from versebridge.documents import extract_text
from versebridge.errors import DocumentError, VerseBridgeError
from versebridge.history import HistoryStore
from versebridge.node.transport import make_transport
from versebridge.workflow.pipeline import apply_import, plan_for_transport, undo_import
from versebridge.workflow.report import build_report

MAX_UPLOAD = 25 * 1024 * 1024
ALLOWED = {".pdf", ".docx", ".txt", ".md", ".html", ".htm"}


class Job:
    def __init__(self, kind: str) -> None:
        self.id = uuid.uuid4().hex[:12]
        self.kind = kind
        self.state = "running"          # running | done | error
        self.message = "Starting"
        self.result: dict | None = None
        self.error: str | None = None
        self.exit_code = 0
        # kept for apply
        self.text = ""
        self.name = ""
        self.plan: Any = None
        self.target: Any = None


class AppState:
    def __init__(self, config: AppConfig, token: str | None, history: HistoryStore | None = None) -> None:
        self.config = config
        self.token = token
        self.history = history or HistoryStore()
        self.jobs: dict[str, Job] = {}
        self.lock = threading.Lock()
        self.busy = False               # one write at a time

    def transport(self):
        return make_transport(self.config.propresenter, self.config.node)

    def target_name(self) -> str | None:
        return self.config.presentation_template.presentation_name if self.config.presentation_template else None


def _spawn(state: AppState, job: Job, fn) -> None:
    def runner() -> None:
        try:
            fn(job)
            job.state = "done"
        except VerseBridgeError as exc:
            job.state, job.error, job.exit_code = "error", str(exc), int(exc.exit_code)
        except Exception as exc:  # unexpected: keep details in the server log only
            traceback.print_exc()
            job.state, job.error, job.exit_code = "error", f"Unexpected problem: {exc}", 1
        finally:
            if job.kind == "apply":
                with state.lock:
                    state.busy = False

    with state.lock:
        state.jobs[job.id] = job
        if len(state.jobs) > 40:
            for key in list(state.jobs)[:10]:
                state.jobs.pop(key, None)
    threading.Thread(target=runner, daemon=True).start()


def start_plan(state: AppState, name: str, data: bytes, default_version: str | None, target: str | None) -> Job:
    job = Job("plan")
    job.name = name

    def work(j: Job) -> None:
        suffix = Path(name).suffix.casefold()
        if suffix not in ALLOWED:
            raise DocumentError(f"Unsupported file type {suffix or '(none)'}. Use PDF, Word (.docx), text, Markdown or HTML.")
        with tempfile.TemporaryDirectory(prefix="versebridge-up-") as folder:
            path = Path(folder) / ("upload" + suffix)
            path.write_bytes(data)
            j.message = "Reading the document"
            j.text = extract_text(path)
        wanted = target or state.target_name()
        if not wanted:
            raise DocumentError("No target presentation is set. Run: versebridge template setup")
        j.message = "Finding verses and fetching Scripture"
        transport = state.transport()
        j.message = "Checking ProPresenter"
        plan, tgt = asyncio.run(plan_for_transport(
            j.text, state.config, transport, default_version=default_version,
            target_uuid=None, target_name=wanted, library=None,
        ))
        j.plan, j.target = plan, tgt
        report = build_report(plan, source_name=name)
        report["target"] = {"name": tgt[2], "library": tgt[0]} if tgt else None
        report["node"] = transport.description
        previous = state.history.latest_active(target_uuid=tgt[1], source_name=name, node=state.config.node.name or "local") if tgt else None
        report["replaces"] = {"import_id": previous.import_id, "slides": len(previous.cue_uuids)} if previous else None
        j.result = report
        j.message = "Ready to review"

    _spawn(state, job, work)
    return job


def start_apply(state: AppState, plan_job_id: str, accept_flags: bool) -> Job:
    source = state.jobs.get(plan_job_id)
    if not source or source.kind != "plan" or not source.plan or not source.target:
        raise VerseBridgeError("That review is no longer available; drop the file again")
    if source.result and source.result["flagged"] and not accept_flags:
        raise VerseBridgeError("Some lines are flagged; confirm you want to import the rest")
    with state.lock:
        if state.busy:
            raise VerseBridgeError("Another import is already running")
        state.busy = True
    job = Job("apply")

    def work(j: Job) -> None:
        def progress(message: str) -> None:
            j.message = message
        assert source.plan is not None and source.target is not None
        applied = apply_import(
            source.text, source.name, source.plan, source.target, state.config, state.transport(),
            replace=True, close_app=True, reopen=True, history=state.history, progress=progress,
        )
        j.result = applied.model_dump()
        j.message = "Done"

    _spawn(state, job, work)
    return job


def start_undo(state: AppState, import_id: str) -> Job:
    with state.lock:
        if state.busy:
            raise VerseBridgeError("Another import is already running")
        state.busy = True
    job = Job("apply")

    def work(j: Job) -> None:
        result = undo_import(import_id, state.config, state.transport(), history=state.history, progress=lambda m: setattr(j, "message", m))
        j.result = result.model_dump()
        j.message = "Done"

    _spawn(state, job, work)
    return job


def make_handler(state: AppState):
    page = resources.files("versebridge.web").joinpath("index.html").read_bytes()

    class Handler(BaseHTTPRequestHandler):
        server_version = "VerseBridge"

        def log_message(self, format, *args) -> None:  # keep the console quiet and free of tokens
            pass

        # --- helpers -------------------------------------------------------
        def _send(self, code: int, body: bytes, ctype: str = "application/json") -> None:
            self.send_response(code)
            self.send_header("Content-Type", ctype)
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Cache-Control", "no-store")
            self.send_header("X-Content-Type-Options", "nosniff")
            self.send_header("Referrer-Policy", "no-referrer")
            self.send_header("Content-Security-Policy", "default-src 'self'; style-src 'self' 'unsafe-inline'; script-src 'self' 'unsafe-inline'")
            self.end_headers()
            self.wfile.write(body)

        def _json(self, code: int, payload) -> None:
            self._send(code, json.dumps(payload, ensure_ascii=False).encode("utf-8"))

        def _authorized(self) -> bool:
            if not state.token:
                return True
            query = parse_qs(urlparse(self.path).query)
            supplied = self.headers.get("X-VerseBridge-Token") or (query.get("token") or [""])[0]
            return secrets.compare_digest(supplied, state.token)

        def _same_origin(self) -> bool:
            origin = self.headers.get("Origin")
            if not origin:
                return True
            host = self.headers.get("Host", "")
            return origin.split("://", 1)[-1] == host

        def _body(self) -> bytes:
            length = int(self.headers.get("Content-Length") or 0)
            if length > MAX_UPLOAD:
                raise VerseBridgeError("That file is too large (limit 25 MB)")
            return self.rfile.read(length)

        # --- routes --------------------------------------------------------
        def do_GET(self) -> None:
            path = urlparse(self.path).path
            if path == "/":
                self._send(200, page, "text/html; charset=utf-8")
                return
            if not self._authorized():
                self._json(401, {"error": "Access token required"})
                return
            if path == "/api/status":
                cfg = state.config
                self._json(200, {
                    "target": state.target_name(), "node": cfg.node.name or "this computer",
                    "default_version": cfg.bible.default_version, "version": "0.2",
                    "template_ready": bool(cfg.presentation_template),
                })
            elif path == "/api/check":
                try:
                    transport = state.transport()
                    info = transport.api_get("/version")
                    self._json(200, {"ok": True, "propresenter": info.get("host_description"), "running": transport.is_running()})
                except Exception as exc:
                    self._json(200, {"ok": False, "error": str(exc)})
            elif path.startswith("/api/jobs/"):
                job = state.jobs.get(path.rsplit("/", 1)[-1])
                if not job:
                    self._json(404, {"error": "Unknown job"})
                    return
                self._json(200, {"id": job.id, "kind": job.kind, "state": job.state, "message": job.message,
                                 "result": job.result, "error": job.error, "exit_code": job.exit_code})
            elif path == "/api/history":
                self._json(200, [
                    {"id": r.import_id, "when": r.created.isoformat(), "source": r.source_name, "target": r.target_name,
                     "slides": r.slide_count, "status": r.status}
                    for r in reversed(state.history.load())
                ][:30])
            else:
                self._json(404, {"error": "Not found"})

        def do_POST(self) -> None:
            if not self._authorized() or not self._same_origin():
                self._json(403, {"error": "Not allowed"})
                return
            parsed = urlparse(self.path)
            query = parse_qs(parsed.query)
            try:
                if parsed.path == "/api/plan":
                    name = Path(self.headers.get("X-File-Name", "notes.txt")).name
                    if Path(name).suffix.casefold() not in ALLOWED:
                        raise DocumentError(f"Unsupported file type {Path(name).suffix or '(none)'}. Use PDF, Word (.docx), text, Markdown or HTML.")
                    version = (query.get("version") or [None])[0] or None
                    job = start_plan(state, name, self._body(), version, (query.get("target") or [None])[0] or None)
                    self._json(202, {"job": job.id})
                elif parsed.path == "/api/apply":
                    body = json.loads(self._body() or b"{}")
                    job = start_apply(state, body.get("job", ""), bool(body.get("accept_flags")))
                    self._json(202, {"job": job.id})
                elif parsed.path == "/api/undo":
                    body = json.loads(self._body() or b"{}")
                    job = start_undo(state, body.get("id", ""))
                    self._json(202, {"job": job.id})
                else:
                    self._json(404, {"error": "Not found"})
            except (VerseBridgeError, DocumentError) as exc:
                self._json(400, {"error": str(exc)})
            except (ValueError, KeyError) as exc:
                self._json(400, {"error": f"Bad request: {exc}"})

    return Handler


def serve(config: AppConfig | None = None, host: str = "127.0.0.1", port: int = 8777, token: str | None = None) -> tuple[ThreadingHTTPServer, AppState]:
    config = config or load_config()
    loopback = host in {"127.0.0.1", "localhost", "::1"}
    if not loopback and not token:
        token = secrets.token_urlsafe(16)
    state = AppState(config, None if loopback else token)
    server = ThreadingHTTPServer((host, port), make_handler(state))
    return server, state
