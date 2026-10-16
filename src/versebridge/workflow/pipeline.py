"""One import pipeline used by the command line and the web page.

plan  -> read notes, classify, fetch Scripture, report successes and problems (writes nothing)
apply -> close the app if needed, take a backup, write atomically, reopen, verify through the
         ProPresenter API, roll back on failure, and record the import so it can be replaced or undone.
"""
from __future__ import annotations

import asyncio
import hashlib
import tempfile
import time
import uuid
from pathlib import Path
from typing import Callable

from pydantic import BaseModel, Field

from versebridge import __version__
from versebridge.config import AppConfig
from versebridge.errors import (
    ConfigurationError, PresentationWriteError, VerificationRollbackError, VerseBridgeError,
)
from versebridge.history import HistoryStore, ImportRecord
from versebridge.logging import AuditEntry, append_audit_entry
from versebridge.node.transport import Transport, make_transport, sha256_file
from versebridge.presentation.protobuf_writer import ProtobufPresentationWriter
from versebridge.presentation.template import validate_template
from versebridge.workflow.notes_workflow import NotesImportResult, plan_notes

Progress = Callable[[str], None]


def _noop(_: str) -> None:
    pass


class ApplyResult(BaseModel):
    import_id: str
    target_name: str
    node: str
    slide_count: int
    cue_count_before: int
    cue_count_after: int
    removed_slides: int = 0
    backup_path: str | None = None
    verification: str = ""
    verified: bool = False
    reopened: bool = False
    steps: list[str] = Field(default_factory=list)


def source_fingerprint(text: str) -> str:
    return hashlib.sha256(text.encode("utf-8")).hexdigest()


def _target_from_disk(transport: Transport, *, uuid_: str | None, name: str | None, library: str | None):
    """Find the target by its file when ProPresenter is closed (the API is unavailable)."""
    import tempfile
    from versebridge.presentation.generated import presentation_pb2
    wanted = name
    if not wanted:
        record = next((r for r in HistoryStore().load() if uuid_ and r.target_uuid.casefold() == uuid_.casefold()), None)
        wanted = record.target_name if record else None
    if not wanted:
        raise ConfigurationError("ProPresenter is not running, so the presentation cannot be looked up by id; give its name")
    paths = transport.find_presentation(wanted, library)
    if not paths:
        raise ConfigurationError(f"ProPresenter is not running and no file named {wanted}.pro was found in its library folder")
    if len(paths) > 1:
        raise ConfigurationError("Presentation target is ambiguous: " + ", ".join(paths))
    local = Path(tempfile.mkdtemp(prefix="versebridge-")) / Path(paths[0]).name
    transport.fetch_file(paths[0], local)
    message = presentation_pb2.Presentation()
    message.ParseFromString(local.read_bytes())
    return Path(paths[0]).parent.name, message.uuid.string, message.name or wanted, paths[0], {"from_disk": True}


def _target_from_api(transport: Transport, *, uuid_: str | None, name: str | None, library: str | None):
    """Find the target presentation and its file path: through the ProPresenter API, or from disk when it is closed."""
    try:
        return _target_from_api_only(transport, uuid_=uuid_, name=name, library=library)
    except ConfigurationError:
        raise
    except Exception as api_error:
        if transport.is_running():
            raise
        try:
            return _target_from_disk(transport, uuid_=uuid_, name=name, library=library)
        except ConfigurationError as disk_error:
            raise ConfigurationError(f"{disk_error} (the ProPresenter API was also unavailable: {api_error})") from disk_error


def _target_from_api_only(transport: Transport, *, uuid_: str | None, name: str | None, library: str | None):
    libraries = transport.api_get("/v1/libraries")
    found = []
    for lib in libraries:
        if library and lib["name"].casefold() != library.casefold():
            continue
        contents = transport.api_get(f"/v1/library/{lib['uuid']}")
        for item in contents.get("items", []):
            if (uuid_ and item["uuid"].casefold() == uuid_.casefold()) or (name and item["name"].casefold() == name.casefold()):
                found.append((lib["name"], item))
    if not found:
        raise ConfigurationError(f"Presentation {name or uuid_!r} was not found in ProPresenter")
    if len(found) > 1:
        raise ConfigurationError("Presentation target is ambiguous: " + ", ".join(f"{lib}/{it['name']}" for lib, it in found))
    library_name, item = found[0]
    detail = transport.api_get(f"/v1/presentation/{item['uuid']}")["presentation"]
    return library_name, item["uuid"], item["name"], detail["presentation_path"], detail


async def plan_for_transport(
    text: str, config: AppConfig, transport: Transport, *, default_version: str | None,
    target_uuid: str | None, target_name: str | None, library: str | None, no_fetch: bool = False,
) -> tuple[NotesImportResult, tuple | None]:
    result = await plan_notes(text, config, default_version=default_version, no_fetch=no_fetch)
    target = None
    if target_uuid or target_name:
        target = await asyncio.to_thread(_target_from_api, transport, uuid_=target_uuid, name=target_name, library=library)
    return result, target


def verify_via_api(transport: Transport, presentation_uuid: str, plan, expected_slides: int, timeout: float = 25.0) -> tuple[bool, str]:
    def norm(value: str) -> str:
        return " ".join(value.split()).casefold()

    deadline = time.monotonic() + timeout
    problem = "ProPresenter did not report the presentation"
    while time.monotonic() < deadline:
        try:
            detail = transport.api_get(f"/v1/presentation/{presentation_uuid}")["presentation"]
            slides = [slide.get("text", "") for group in detail["groups"] for slide in group["slides"]]
            if len(slides) < expected_slides:
                problem = f"API reports {len(slides)} slides; expected at least {expected_slides}"
            else:
                visible = [norm(item) for item in slides]
                missing = [s.body_text for s in plan.slides if not any(norm(s.body_text.split("\n")[0]) in v for v in visible)]
                if not missing:
                    return True, f"ProPresenter reports all {len(plan.slides)} generated slides"
                problem = f"{len(missing)} generated slide(s) not reported by ProPresenter"
        except Exception as exc:  # the app may still be loading
            problem = str(exc)
        time.sleep(1.5)
    return False, problem


def apply_import(
    text: str, source_name: str, plan: NotesImportResult, target: tuple, config: AppConfig, transport: Transport,
    *, replace: bool = True, close_app: bool = True, reopen: bool = True, history: HistoryStore | None = None,
    progress: Progress = _noop,
) -> ApplyResult:
    library_name, presentation_uuid, presentation_name, path, _detail = target
    history = history or HistoryStore()
    definition = config.presentation_template
    if not definition:
        raise ConfigurationError("No template is configured; run versebridge template select")
    if not plan.slide_plan.slides:
        raise ConfigurationError("There are no slides to import")
    node_name = config.node.name or "local"
    steps: list[str] = []

    previous = history.latest_active(target_uuid=presentation_uuid, source_name=source_name, node=node_name) if replace else None
    remove = previous.cue_uuids if previous else []
    if previous:
        steps.append(f"replacing earlier import {previous.import_id} ({len(previous.cue_uuids)} slides)")

    was_running = transport.is_running()
    reopen_after = reopen and was_running
    if was_running:
        if not close_app:
            raise ConfigurationError("ProPresenter is running; close it completely first (or allow VerseBridge to close it)")
        progress("Closing ProPresenter")
        transport.quit_app()
        deadline = time.monotonic() + config.node.quit_wait_seconds
        while time.monotonic() < deadline and transport.is_running():
            time.sleep(1.0)
        if transport.is_running():
            raise PresentationWriteError(
                "ProPresenter is waiting for someone at that computer to confirm quitting or saving "
                "(look for an 'Are you sure you want to quit?' or 'Save changes?' prompt). "
                "Answer it, then run the import again. No files changed"
            )
        steps.append("closed ProPresenter")

    workdir = Path(tempfile.mkdtemp(prefix="versebridge-"))
    local_copy = workdir / Path(path).name
    backup = None
    installed = False
    try:
        progress("Reading the presentation")
        original_sha = transport.fetch_file(path, local_copy)
        validate_template(local_copy, definition)
        progress("Backing up")
        backup = transport.backup_file(path)
        steps.append(f"backup: {backup}")
        progress("Writing slides")
        written = ProtobufPresentationWriter().append_plan(
            local_copy, plan.slide_plan, definition, remove_cue_uuids=remove or None,
        )
        transport.install_file(local_copy, path, original_sha)
        installed = True
        steps.append("installed updated presentation")
    except Exception:
        if installed and backup:
            try:
                transport.restore_backup(backup, path)
            except Exception:
                pass
        if was_running and reopen_after:
            try:
                transport.launch_app()
            except Exception:
                pass
        raise

    verified, message, reopened = False, "Written and checked in the file; ProPresenter was closed, so the slides appear the next time you open it", False
    if reopen_after:
        progress("Reopening ProPresenter")
        transport.launch_app()
        reopened = True
        deadline = time.monotonic() + config.node.launch_wait_seconds
        while time.monotonic() < deadline:
            try:
                transport.api_get("/version")
                break
            except Exception:
                time.sleep(2.0)
        progress("Verifying slides in ProPresenter")
        verified, message = verify_via_api(transport, presentation_uuid, plan.slide_plan, written.cue_count_after)
        if not verified:
            progress("Verification failed; rolling back")
            transport.quit_app()
            time.sleep(config.node.quit_wait_seconds / 2)
            if transport.is_running():
                raise VerificationRollbackError(
                    f"Verification failed ({message}) and ProPresenter would not close. Close it and restore {backup} over {path}"
                )
            transport.restore_backup(backup, path)
            transport.launch_app()
            raise VerificationRollbackError(f"Verification failed ({message}). The original was restored from {backup}")
    else:
        verified = True

    import_id = uuid.uuid4().hex[:10]
    if previous:
        history.set_status(previous.import_id, "replaced")
    history.add(ImportRecord(
        import_id=import_id, source_name=source_name, source_sha256=source_fingerprint(text),
        target_uuid=presentation_uuid, target_name=presentation_name, node=node_name,
        cue_uuids=written.cue_uuids, slide_count=len(written.cue_uuids),
        verse_count=plan.verse_count, point_count=plan.point_count, backup_path=backup,
        replaces=previous.import_id if previous else None,
    ))
    from platformdirs import user_log_path
    append_audit_entry(user_log_path("versebridge") / "audit.jsonl", AuditEntry(
        application_version=__version__, propresenter_version="19.0.1", target_uuid=presentation_uuid,
        presentation_name=presentation_name, reference_count=plan.verse_count, slide_count=len(written.cue_uuids),
        versions=sorted({s.version_text for s in plan.slide_plan.slides if s.version_text}),
        backup_path=Path(backup) if backup and transport.__class__.__name__ == "LocalTransport" else None,
        verification_result=message,
    ))
    return ApplyResult(
        import_id=import_id, target_name=presentation_name, node=node_name,
        slide_count=len(written.cue_uuids), cue_count_before=written.cue_count_before,
        cue_count_after=written.cue_count_after, removed_slides=written.removed_cue_count,
        backup_path=backup, verification=message, verified=verified, reopened=reopened, steps=steps,
    )


def undo_import(
    import_id: str, config: AppConfig, transport: Transport, *, history: HistoryStore | None = None,
    close_app: bool = True, reopen: bool = True, progress: Progress = _noop,
) -> ApplyResult:
    """Remove exactly the slides an earlier import added. Other edits to the presentation are kept."""
    history = history or HistoryStore()
    record = history.get(import_id)
    if not record or record.status != "active":
        raise ConfigurationError(f"No active import with id {import_id}")
    definition = config.presentation_template
    if not definition:
        raise ConfigurationError("No template is configured")
    library_name, presentation_uuid, presentation_name, path, _ = _target_from_api(
        transport, uuid_=record.target_uuid, name=None, library=None
    )
    was_running = transport.is_running()
    if was_running:
        if not close_app:
            raise ConfigurationError("ProPresenter is running; close it completely first")
        progress("Closing ProPresenter")
        transport.quit_app()
        deadline = time.monotonic() + config.node.quit_wait_seconds
        while time.monotonic() < deadline and transport.is_running():
            time.sleep(1.0)
        if transport.is_running():
            raise PresentationWriteError("ProPresenter did not close; close it yourself and retry. No files changed")
    workdir = Path(tempfile.mkdtemp(prefix="versebridge-"))
    local_copy = workdir / Path(path).name
    sha = transport.fetch_file(path, local_copy)
    backup = transport.backup_file(path)
    from versebridge.slides.models import SlidePlan
    written = ProtobufPresentationWriter().append_plan(local_copy, SlidePlan(), definition, remove_cue_uuids=record.cue_uuids)
    transport.install_file(local_copy, path, sha)
    if was_running and reopen:
        progress("Reopening ProPresenter")
        transport.launch_app()
    history.set_status(import_id, "undone")
    return ApplyResult(
        import_id=import_id, target_name=presentation_name, node=config.node.name or "local",
        slide_count=0, cue_count_before=written.cue_count_before, cue_count_after=written.cue_count_after,
        removed_slides=written.removed_cue_count, backup_path=backup, verification="Slides removed", verified=True,
        reopened=bool(was_running and reopen),
    )
