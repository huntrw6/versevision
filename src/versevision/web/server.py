"""Local web page for VerseVision: drop a notes file, see what was added and what was flagged, then import.

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

from versevision.config import AppConfig, load_config
from versevision.documents import extract_text
from versevision.errors import DocumentError, VerseVisionError
from versevision.history import HistoryStore
from versevision.node.transport import make_transport
from versevision import runlog
from versevision.workflow.pipeline import apply_import, list_presentations, plan_for_transport, publish_new, publish_swap, undo_import
from versevision.workflow.report import build_report

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
        self.mode = "new"
        self.title = ""
        self.title_override: str | None = None
        self.library: str | None = None


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
        except VerseVisionError as exc:
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


def start_plan(state: AppState, name: str, data: bytes, default_version: str | None, target: str | None,
               mode: str = "new", title: str | None = None, library: str | None = None) -> Job:
    job = Job("plan")
    job.name = name
    job.mode, job.title_override, job.library = mode, title, library
    job.run_id = ""

    def work(j: Job) -> None:
        run = runlog.RunLog.start("web", source=name, mode=mode, title=title, library=library, target=target,
                                  node=state.config.node.name or "this computer",
                                  default_version=default_version or state.config.bible.default_version)
        j.run_id = run.id
        token = runlog.activate(run)
        outcome, error = "failed", None
        try:
            _plan_work(j)
            outcome = "reviewed, not imported yet"
        except Exception as exc:
            error = str(exc)
            run.failed(exc)
            raise
        finally:
            run.finish(outcome, j.result, None, error)
            runlog.deactivate(token)

    def _plan_work(j: Job) -> None:
        run = runlog.current()
        suffix = Path(name).suffix.casefold()
        if suffix not in ALLOWED:
            raise DocumentError(f"Unsupported file type {suffix or '(none)'}. Use PDF, Word (.docx), text, Markdown or HTML.")
        with tempfile.TemporaryDirectory(prefix="versevision-up-") as folder:
            path = Path(folder) / ("upload" + suffix)
            path.write_bytes(data)
            j.message = "Reading the document"
            j.text = extract_text(path)
            run.event("document_read", message=f"{name} ({len(data)} bytes)", file=name, bytes=len(data), type=suffix)
        run.save_source(j.text, name)
        if mode not in {"new", "replace", "append"}:
            raise DocumentError("Unknown mode")
        if not state.config.presentation_template:
            raise DocumentError("No template is set up yet. Run: versevision template setup")
        j.message = "Finding verses and fetching Scripture"
        transport = state.transport()
        j.message = "Checking ProPresenter"
        tgt = None
        if mode != "new":
            if not target:
                raise DocumentError("Choose which presentation to change")
            plan, tgt = asyncio.run(plan_for_transport(
                j.text, state.config, transport, default_version=default_version,
                target_uuid=target, target_name=None, library=None,
            ))
        else:
            plan, _ = asyncio.run(plan_for_transport(
                j.text, state.config, transport, default_version=default_version,
                target_uuid=None, target_name=None, library=None,
            ))
        j.plan, j.target = plan, tgt
        from versevision.notes.title import document_title, safe_presentation_name
        j.title = safe_presentation_name(title) if title else document_title(j.text, name)
        report = build_report(plan, source_name=name)
        report["mode"] = mode
        report["title"] = j.title
        report["library"] = library or state.config.output.library
        report["target"] = {"name": tgt[2], "library": tgt[0], "uuid": tgt[1]} if tgt else None
        report["node"] = transport.description
        report["replaces"] = None
        j.result = report
        report["run_id"] = j.run_id
        run.save_report(report)
        j.message = "Ready to review"

    _spawn(state, job, work)
    return job


def start_apply(state: AppState, plan_job_id: str, accept_flags: bool) -> Job:
    source = state.jobs.get(plan_job_id)
    if not source or source.kind != "plan" or not source.plan or (source.mode != "new" and not source.target):
        raise VerseVisionError("That review is no longer available; drop the file again")
    if source.result and source.result["flagged"] and not accept_flags:
        raise VerseVisionError("Some lines are flagged; confirm you want to import the rest")
    with state.lock:
        if state.busy:
            raise VerseVisionError("Another import is already running")
        state.busy = True
    job = Job("apply")

    def work(j: Job) -> None:
        run = runlog.RunLog.reopen(getattr(source, "run_id", "")) or runlog.RunLog.start("web", source=source.name, mode=source.mode)
        token = runlog.activate(run)
        run.event("apply_start", message=f"{source.mode}: importing after review", accept_flags=accept_flags)
        outcome, error, result = "failed", None, None
        try:
            result = _apply_work(j, run)
            outcome = "ok"
        except Exception as exc:
            error = str(exc)
            run.failed(exc)
            raise
        finally:
            run.finish(outcome, source.result, result, error)
            runlog.deactivate(token)

    def _apply_work(j: Job, run) -> dict:
        def progress(message: str) -> None:
            run.event("progress", message=message)
            j.message = message
        assert source.plan is not None
        if source.mode == "new":
            applied = publish_new(
                source.text, source.name, source.title, source.plan, state.config, state.transport(),
                library=source.library, history=state.history, progress=progress,
            )
        else:
            assert source.target is not None
            applied = publish_new(
                source.text, source.name, source.title, source.plan, state.config, state.transport(),
                history=state.history, progress=progress, base=source.target, clear_generated=(source.mode == "replace"),
            )
        j.result = applied.model_dump()
        j.message = "Done"
        return j.result

    _spawn(state, job, work)
    return job


def start_undo(state: AppState, import_id: str) -> Job:
    with state.lock:
        if state.busy:
            raise VerseVisionError("Another import is already running")
        state.busy = True
    job = Job("apply")

    def work(j: Job) -> None:
        run = runlog.RunLog.start("undo", source=f"undo of import {import_id}", mode="undo")
        token = runlog.activate(run)
        outcome, error, applied = "failed", None, None
        try:
            def progress(m: str) -> None:
                run.event("progress", message=m)
                j.message = m
            result = undo_import(import_id, state.config, state.transport(), history=state.history, progress=progress)
            j.result = applied = result.model_dump()
            outcome = "ok"
            j.message = "Done"
        except Exception as exc:
            error = str(exc)
            run.failed(exc)
            raise
        finally:
            run.finish(outcome, None, applied, error)
            runlog.deactivate(token)

    _spawn(state, job, work)
    return job


def make_handler(state: AppState):
    page = resources.files("versevision.web").joinpath("index.html").read_bytes()

    class Handler(BaseHTTPRequestHandler):
        server_version = "VerseVision"

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
            supplied = self.headers.get("X-VerseVision-Token") or (query.get("token") or [""])[0]
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
                raise VerseVisionError("That file is too large (limit 25 MB)")
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
                    "default_version": cfg.bible.default_version, "version": __import__("versevision").__version__,
                    "template_ready": bool(cfg.presentation_template), "library": cfg.output.library,
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
            elif path == "/api/runs":
                self._json(200, runlog.list_runs(30))
            elif path.startswith("/api/run/"):
                run_id = runlog.resolve(path.rsplit("/", 1)[-1])
                summary = (runlog.runs_dir() / run_id / "summary.txt") if run_id else None
                if not run_id or not summary or not summary.exists():
                    self._json(404, {"error": "No such run"})
                else:
                    self._json(200, {"id": run_id, "summary": summary.read_text(encoding="utf-8"), "folder": str(runlog.runs_dir() / run_id)})
            elif path == "/api/presentations":
                try:
                    items = list_presentations(state.transport())
                    libraries = sorted({item["library"] for item in items} | {state.config.output.library})
                    self._json(200, {"presentations": items, "libraries": libraries, "default_library": state.config.output.library})
                except Exception as exc:
                    self._json(200, {"presentations": [], "libraries": [state.config.output.library], "default_library": state.config.output.library, "error": str(exc)})
            elif path == "/api/autostart":
                from versevision import autostart
                try:
                    self._json(200, autostart.status().model_dump())
                except Exception as exc:
                    self._json(200, {"supported": False, "enabled": False, "detail": str(exc)})
            elif path == "/api/history":
                self._json(200, [
                    {"id": r.import_id, "when": r.created.isoformat(), "source": r.source_name, "target": r.target_name,
                     "slides": r.slide_count, "status": r.status, "mode": r.mode}
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
                    job = start_plan(
                        state, name, self._body(), version, (query.get("target") or [None])[0] or None,
                        mode=(query.get("mode") or ["new"])[0], title=(query.get("title") or [None])[0] or None,
                        library=(query.get("library") or [None])[0] or None,
                    )
                    self._json(202, {"job": job.id})
                elif parsed.path == "/api/apply":
                    body = json.loads(self._body() or b"{}")
                    job = start_apply(state, body.get("job", ""), bool(body.get("accept_flags")))
                    self._json(202, {"job": job.id})
                elif parsed.path == "/api/autostart":
                    from versevision import autostart
                    if self.server.server_address[0] not in {"127.0.0.1", "::1"}:
                        raise VerseVisionError("Start-at-login can only be changed from the page running on this computer (127.0.0.1)")
                    body = json.loads(self._body() or b"{}")
                    if body.get("enabled"):
                        port = int(self.server.server_address[1])
                        self._json(200, autostart.enable("127.0.0.1", port).model_dump())
                    else:
                        # Turning it off may stop the very service that is answering this request, so
                        # reply first (with the new state) and remove the login item a moment later.
                        current = autostart.status().model_dump()
                        current.update(enabled=False, port=None, host=None)
                        self._json(200, current)
                        threading.Timer(0.5, autostart.disable).start()
                elif parsed.path == "/api/undo":
                    body = json.loads(self._body() or b"{}")
                    job = start_undo(state, body.get("id", ""))
                    self._json(202, {"job": job.id})
                else:
                    self._json(404, {"error": "Not found"})
            except (VerseVisionError, DocumentError) as exc:
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
