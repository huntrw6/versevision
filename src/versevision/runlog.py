"""One folder per run, so any import can be audited afterwards.

    <log dir>/runs/20261018-091502-3fa9c1/
        summary.txt   plain-language account of what happened (read this first)
        events.jsonl  every step with a timestamp: document read, each Bible lookup, each ProPresenter action, errors
        source.txt    the exact text that was read from the document
        report.json   the full plan: every slide, every flagged / adjusted / skipped line

Nothing secret is written: API keys, tokens and request URLs are never logged.
"""
from __future__ import annotations

from contextvars import ContextVar
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import shutil
import threading
import time
import traceback
from typing import Any, Callable

from platformdirs import user_log_path

KEEP_RUNS = 200
_SECRET_WORDS = ("api_key", "apikey", "authorization", "password", "secret", "token", "x-api-key")
_current: ContextVar["RunLog | None"] = ContextVar("versevision_run", default=None)


def runs_dir() -> Path:
    override = os.getenv("VERSEVISION_LOGS")
    return (Path(override).expanduser() if override else user_log_path("versevision")) / "runs"


def _clean(value: Any, limit: int = 400) -> Any:
    if isinstance(value, dict):
        return {k: ("[REDACTED]" if any(w in str(k).casefold() for w in _SECRET_WORDS) else _clean(v, limit)) for k, v in value.items()}
    if isinstance(value, (list, tuple)):
        return [_clean(v, limit) for v in value[:300]]
    if isinstance(value, Path):
        return str(value)
    if isinstance(value, str) and len(value) > limit:
        return value[:limit] + f"... [{len(value) - limit} more characters]"
    return value


class RunLog:
    def __init__(self, folder: Path, run_id: str, kind: str):
        self.folder, self.id, self.kind = folder, run_id, kind
        self.started = time.monotonic()
        self._lock = threading.Lock()
        self.meta: dict[str, Any] = {}

    # ------------------------------------------------------------------ create / reopen
    @classmethod
    def start(cls, kind: str, **meta: Any) -> "RunLog":
        stamp = datetime.now().strftime("%Y%m%d-%H%M%S")
        run_id = f"{stamp}-{hashlib.sha1(os.urandom(8)).hexdigest()[:6]}"
        folder = runs_dir() / run_id
        folder.mkdir(parents=True, exist_ok=True)
        try:
            os.chmod(folder, 0o700)
        except OSError:
            pass
        run = cls(folder, run_id, kind)
        from versevision import __version__
        run.meta = {"run_id": run_id, "kind": kind, "application_version": __version__,
                    "started": datetime.now(timezone.utc).isoformat(timespec="seconds"), **_clean(meta)}
        run.event("run_start", **run.meta)
        _prune()
        return run

    @classmethod
    def reopen(cls, run_id: str) -> "RunLog | None":
        folder = runs_dir() / run_id
        if not folder.is_dir():
            return None
        run = cls(folder, run_id, "run")
        run.event("run_resumed")
        return run

    # ------------------------------------------------------------------ writing
    def event(self, step: str, level: str = "info", **data: Any) -> None:
        line = {"t": datetime.now(timezone.utc).isoformat(timespec="milliseconds"),
                "elapsed": round(time.monotonic() - self.started, 2), "step": step, "level": level, **_clean(data)}
        try:
            with self._lock, (self.folder / "events.jsonl").open("a", encoding="utf-8") as stream:
                stream.write(json.dumps(line, ensure_ascii=False, default=str) + "\n")
        except OSError:
            pass                     # logging must never break an import

    def save_source(self, text: str, name: str) -> None:
        try:
            (self.folder / "source.txt").write_text(text, encoding="utf-8")
        except OSError:
            return
        self.event("source_saved", file=name, characters=len(text), lines=text.count("\n") + 1,
                   sha256=hashlib.sha256(text.encode("utf-8")).hexdigest())

    def save_report(self, report: dict) -> None:
        try:
            (self.folder / "report.json").write_text(json.dumps(report, indent=2, ensure_ascii=False, default=str), encoding="utf-8")
        except OSError:
            pass

    def failed(self, exc: BaseException) -> None:
        self.event("run_failed", level="error", error=str(exc), error_type=type(exc).__name__,
                   traceback="".join(traceback.format_exception(type(exc), exc, exc.__traceback__))[-3000:])

    def finish(self, outcome: str, report: dict | None = None, applied: dict | None = None, error: str | None = None) -> None:
        bad = outcome == "failed" or bool(error)
        self.event("run_end", level="error" if bad else "warn" if outcome.startswith("stopped") else "info", outcome=outcome, error=error)
        try:
            (self.folder / "summary.txt").write_text(render_summary(self, outcome, report, applied, error), encoding="utf-8")
        except Exception as exc:  # pragma: no cover - summary is best effort
            self.event("summary_failed", level="warn", error=str(exc))


# ---------------------------------------------------------------------- module-level helpers (no-ops without a run)
def current() -> RunLog | None:
    return _current.get()


def activate(run: RunLog | None):
    return _current.set(run)


def deactivate(token) -> None:
    _current.reset(token)


def event(step: str, level: str = "info", **data: Any) -> None:
    run = _current.get()
    if run:
        run.event(step, level, **data)


def wrap_progress(progress: Callable[[str], None]) -> Callable[[str], None]:
    def logged(message: str) -> None:
        event("progress", message=message)
        progress(message)
    return logged


def _prune() -> None:
    try:
        folders = sorted((p for p in runs_dir().iterdir() if p.is_dir()), key=lambda p: p.name)
        for old in folders[:-KEEP_RUNS]:
            shutil.rmtree(old, ignore_errors=True)
    except OSError:
        pass


# ---------------------------------------------------------------------- reading
def list_runs(limit: int = 30) -> list[dict]:
    out: list[dict] = []
    try:
        folders = sorted((p for p in runs_dir().iterdir() if p.is_dir()), key=lambda p: p.name, reverse=True)[:limit]
    except OSError:
        return out
    for folder in folders:
        info: dict[str, Any] = {"id": folder.name, "path": str(folder), "outcome": "unfinished", "source": "", "mode": ""}
        events = read_events(folder.name)
        for e in events:
            if e["step"] == "run_start":
                info.update(source=e.get("source", ""), mode=e.get("mode", e.get("kind", "")), started=e.get("started", ""))
            if e["step"] == "run_end":
                info["outcome"] = e.get("outcome", "?")
        out.append(info)
    return out


def read_events(run_id: str) -> list[dict]:
    path = runs_dir() / run_id / "events.jsonl"
    rows: list[dict] = []
    try:
        for line in path.read_text(encoding="utf-8").splitlines():
            try:
                rows.append(json.loads(line))
            except ValueError:
                continue
    except OSError:
        pass
    return rows


def resolve(run_id: str) -> str | None:
    if run_id in ("latest", ""):
        runs = list_runs(1)
        return runs[0]["id"] if runs else None
    if (runs_dir() / run_id).is_dir():
        return run_id
    matches = [p.name for p in runs_dir().iterdir() if p.is_dir() and p.name.endswith(run_id)] if runs_dir().is_dir() else []
    return matches[0] if len(matches) == 1 else None


# ---------------------------------------------------------------------- the readable summary
def render_summary(run: RunLog, outcome: str, report: dict | None, applied: dict | None, error: str | None) -> str:
    events = read_events(run.id)
    start = next((e for e in events if e["step"] == "run_start"), {})
    lines: list[str] = []
    add = lines.append
    add(f"VerseVision run {run.id}")
    add("=" * 60)
    add(f"Result:        {'SUCCESS' if outcome == 'ok' else 'FAILED' if outcome == 'failed' else outcome.upper()}")
    if error:
        add(f"Error:         {error}")
    add(f"Started:       {start.get('started', '')}")
    add(f"Version:       VerseVision {start.get('application_version', '')}")
    add(f"Document:      {start.get('source', '')}")
    add(f"Action:        {start.get('mode', run.kind)}")
    for key, label in (("title", "Name used"), ("library", "Library"), ("target", "Chosen presentation"), ("node", "ProPresenter on"),
                       ("default_version", "Default version"), ("flags", "Options")):
        if start.get(key):
            add(f"{label + ':':<14} {start[key]}")
    src = next((e for e in events if e["step"] == "source_saved"), None)
    if src:
        add(f"Source text:   {src['characters']} characters, {src['lines']} lines (sha256 {src['sha256'][:12]}), kept in source.txt")
    if applied and start.get("mode") == "undo":
        add("")
        add("What was undone")
        add("-" * 60)
        add(f"Removed:       {applied.get('presentation_name') or applied.get('target_name')} ({applied.get('removed_slides')} slides)")
        add(f"Result:        {applied.get('verification', '')}")
    elif applied:
        add("")
        add("What was written")
        add("-" * 60)
        add(f"New file:      {applied.get('presentation_name') or applied.get('target_name')}")
        add(f"Slides:        {applied.get('slide_count')} added; the file has {applied.get('cue_count_after')} slides"
            + (f" (copied from one with {applied.get('cue_count_before')}, {applied.get('removed_slides')} removed)" if applied.get('mode') in ('replace', 'append') else ""))
        add(f"Verified:      {'yes' if applied.get('verified') else 'NO'} - {applied.get('verification', '')}")
        for step in applied.get("steps", []):
            add(f"  - {step}")
    if report:
        s = report["summary"]
        add("")
        add("What was understood")
        add("-" * 60)
        add(f"{s['slides']} slides: {s['verse_slides']} verse, {s['point_slides']} sermon point. "
            f"{s['flagged']} flagged ({s['not_added']} not added, {s['needs_review']} need review). "
            f"{s['adjustments']} adjustments. {s['left_out']} lines left out on purpose.")
        add("")
        for slide in report["slides"]:
            add(f"  {slide['n']:>3}. {slide['kind']:<5} {slide['title'][:46]:<46} {slide['text'].replace(chr(10), ' ')[:60]}")
        for title, key in (("FLAGGED (not imported or needs a look)", "flagged"), ("ADJUSTMENTS made automatically", "adjustments"), ("LEFT OUT on purpose", "left_out")):
            rows = report.get(key) or []
            if rows:
                add("")
                add(f"{title}: {len(rows)}")
                for r in rows:
                    add(f"  line {r['line']:>4}  {r['text'][:70]!r}  -> {r['detail']}")
    fetches = [e for e in events if e["step"] == "bible_fetch"]
    if fetches:
        add("")
        add("Bible lookups")
        add("-" * 60)
        for e in fetches:
            tail = f"ERROR {e.get('error')}" if e.get("error") else f"{e.get('verses')} verses"
            add(f"  {e.get('reference', ''):<28} {e.get('version', ''):<8} via {e.get('provider', '-'):<10} {e.get('source', ''):<8} {e.get('ms', 0):>5} ms  {tail}")
    problems = [e for e in events if e.get("level") in ("warn", "error") and e["step"] not in ("run_end",)]
    if problems:
        add("")
        add("Warnings and errors")
        add("-" * 60)
        for e in problems:
            add(f"  [{e['elapsed']:>6}s] {e['step']}: {e.get('message') or e.get('error') or ''}")
    add("")
    add("Timeline")
    add("-" * 60)
    for e in events:
        if e["step"] in ("progress", "node_action", "playlist", "verify", "file_placed", "run_failed", "plan_ready"):
            detail = e.get("message") or e.get("action") or e.get("error") or ""
            add(f"  [{e['elapsed']:>6}s] {e['step']:<12} {detail}")
    add("")
    add(f"Files in this folder: summary.txt, events.jsonl (every step), source.txt, report.json")
    return "\n".join(lines) + "\n"
