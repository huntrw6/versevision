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
    mode: str = "new"
    presentation_name: str = ""
    restart_needed: bool = False
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


def list_presentations(transport: Transport) -> list[dict]:
    """Every presentation ProPresenter knows about, grouped by library, for pickers."""
    out: list[dict] = []
    for lib in transport.api_get("/v1/libraries"):
        contents = transport.api_get(f"/v1/library/{lib['uuid']}")
        for item in contents.get("items", []):
            out.append({"uuid": item["uuid"], "name": item["name"], "library": lib["name"], "library_uuid": lib["uuid"]})
    return out


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
    *, replace: bool = True, clear_generated: bool = False, close_app: bool = True, reopen: bool = True,
    history: HistoryStore | None = None, progress: Progress = _noop,
) -> ApplyResult:
    """Change an EXISTING presentation in place. This needs ProPresenter closed (it ignores edits to an open file).

    replace=True removes the slides an earlier import of the same file added.
    clear_generated=True removes every verse and sermon-point slide in the presentation first
    (blank, background, title and media slides are kept).
    """
    library_name, presentation_uuid, presentation_name, path, _detail = target
    history = history or HistoryStore()
    definition = config.presentation_template
    if not definition:
        raise ConfigurationError("No template is configured; run versebridge template select")
    if not plan.slide_plan.slides:
        raise ConfigurationError("There are no slides to import")
    node_name = config.node.name or "local"
    steps: list[str] = []

    previous = history.latest_active(target_uuid=presentation_uuid, source_name=source_name, node=node_name) if (replace and not clear_generated) else None
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
        writer = ProtobufPresentationWriter()
        if clear_generated:
            from versebridge.presentation.generated import presentation_pb2
            current = presentation_pb2.Presentation()
            current.ParseFromString(local_copy.read_bytes())
            remove = writer.generated_cue_uuids(current, definition)
            steps.append(f"clearing {len(remove)} verse and sermon-point slides (other slides are kept)")
        written = writer.append_plan(local_copy, plan.slide_plan, definition, remove_cue_uuids=remove or None)
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
        replaces=previous.import_id if previous else None, mode="replace" if clear_generated else "append",
        library=library_name,
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
        mode="replace" if clear_generated else "append", presentation_name=presentation_name, restart_needed=reopen_after is False and not was_running,
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
    if record.mode == "new":
        return _undo_new(record, config, transport, history, progress)
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


def publish_new(
    text: str, source_name: str, title: str, plan: NotesImportResult, config: AppConfig, transport: Transport,
    *, library: str | None = None, history: HistoryStore | None = None, progress: Progress = _noop,
    verify_timeout: float = 25.0,
) -> ApplyResult:
    """Create a NEW presentation named after the document. ProPresenter can stay open: it shows new files at once."""
    from versebridge.notes.title import safe_presentation_name, unique_name

    history = history or HistoryStore()
    definition = config.presentation_template
    if not definition:
        raise ConfigurationError("No template is configured; run versebridge template setup")
    if not plan.slide_plan.slides:
        raise ConfigurationError("There are no slides to import")
    lib_name = library or config.output.library
    folder = transport.library_folder(lib_name)
    node_name = config.node.name or "local"
    name = safe_presentation_name(title)

    # existing names: from the library listing when ProPresenter answers, else from disk
    existing: set[str] = set()
    try:
        existing = {item["name"] for item in list_presentations(transport) if item["library"].casefold() == lib_name.casefold()}
    except Exception:
        pass
    name = unique_name(name, existing)
    # also guard against a file that exists on disk but is not in the listing
    for _ in range(50):
        if not transport.find_presentation(name, lib_name):
            break
        name = unique_name(name, existing | {name})
    steps = [f"new presentation: {name}"]

    try:
        template_target = _target_from_api(transport, uuid_=definition.presentation_uuid, name=None, library=None)
    except ConfigurationError:
        # the saved id can go stale (the presentation was recreated); fall back to its name
        template_target = _target_from_api(transport, uuid_=None, name=definition.presentation_name, library=definition.library_name)
    workdir = Path(tempfile.mkdtemp(prefix="versebridge-"))
    template_local = workdir / "template.pro"
    progress("Reading the template")
    transport.fetch_file(template_target[3], template_local)
    validate_template(template_local, definition)
    out_local = workdir / f"{name}.pro"
    progress("Building the presentation")
    written = ProtobufPresentationWriter().create_presentation(
        template_local, out_local, plan.slide_plan, definition, name=name,
    )
    remote_path = f"{folder}/{name}.pro"
    progress("Adding it to ProPresenter")
    transport.create_file(out_local, remote_path)
    steps.append(f"placed {remote_path}")

    verified, message, new_uuid = False, "Placed in the library folder", ""
    was_running = transport.is_running()
    if was_running:
        progress("Checking ProPresenter shows it")
        deadline = time.monotonic() + verify_timeout
        problem = "ProPresenter has not listed the new presentation yet"
        while time.monotonic() < deadline:
            try:
                match = next((i for i in list_presentations(transport) if i["name"] == name and i["library"].casefold() == lib_name.casefold()), None)
                if match:
                    new_uuid = match["uuid"]
                    ok, message = verify_via_api(transport, new_uuid, plan.slide_plan, len(plan.slide_plan.slides), timeout=max(5.0, deadline - time.monotonic()))
                    if ok:
                        verified = True
                        break
                    problem = message
            except Exception as exc:
                problem = str(exc)
            time.sleep(1.5)
        if not verified:
            raise VerificationRollbackError(
                f"ProPresenter did not show the new presentation ({problem}). The file {remote_path} was left in place; "
                "reopen ProPresenter to see it, or remove the file."
            )
    else:
        verified, message = True, "Placed in the library folder; ProPresenter will list it the next time it opens"

    import_id = uuid.uuid4().hex[:10]
    history.add(ImportRecord(
        import_id=import_id, source_name=source_name, source_sha256=source_fingerprint(text),
        target_uuid=new_uuid or written.cue_uuids[0], target_name=name, node=node_name,
        cue_uuids=written.cue_uuids, slide_count=len(written.cue_uuids), verse_count=plan.verse_count,
        point_count=plan.point_count, backup_path=None, mode="new", library=lib_name, file_path=remote_path,
    ))
    from platformdirs import user_log_path
    append_audit_entry(user_log_path("versebridge") / "audit.jsonl", AuditEntry(
        application_version=__version__, propresenter_version="19.0.1", target_uuid=new_uuid or None,
        presentation_name=name, reference_count=plan.verse_count, slide_count=len(written.cue_uuids),
        versions=sorted({s.version_text for s in plan.slide_plan.slides if s.version_text}),
        verification_result=message,
    ))
    return ApplyResult(
        mode="new", presentation_name=name, restart_needed=False, import_id=import_id, target_name=name, node=node_name,
        slide_count=len(written.cue_uuids), cue_count_before=0, cue_count_after=len(written.cue_uuids),
        verification=message, verified=verified, steps=steps,
    )


def _undo_new(record: ImportRecord, config: AppConfig, transport: Transport, history: HistoryStore, progress: Progress) -> ApplyResult:
    """Undo a "new presentation" import: delete that file. Only a file VerseBridge created and that is unchanged."""
    if not record.file_path:
        raise ConfigurationError("This import has no recorded file; remove the presentation in ProPresenter instead")
    progress("Removing the presentation")
    removed = transport.delete_created_file(record.file_path, expected_cues=len(record.cue_uuids))
    history.set_status(record.import_id, "undone")
    return ApplyResult(
        mode="new", presentation_name=record.target_name, import_id=record.import_id, target_name=record.target_name,
        node=config.node.name or "local", slide_count=0, cue_count_before=len(record.cue_uuids), cue_count_after=0,
        removed_slides=len(record.cue_uuids), verification=removed, verified=True,
    )
