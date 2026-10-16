from __future__ import annotations

import asyncio
import json
from pathlib import Path
import sys
from typing import Annotated, Optional

import typer
from rich.console import Console
from rich.table import Table
from platformdirs import user_log_path

from versebridge import __version__
from versebridge.config import AppConfig, config_path, load_config, save_config
from versebridge.bibles import ProviderRegistry, ScriptureCache
from versebridge.documents import extract_text
from versebridge.errors import (
    ConfigurationError, DocumentError, ExitCode, PresentationWriteError,
    VerificationRollbackError, VerseBridgeError,
)
from versebridge.logging import AuditEntry, append_audit_entry
from versebridge.presentation import ProtobufPresentationWriter
from versebridge.presentation.backup import restore_backup
from versebridge.propresenter import ProPresenterApiClient
from versebridge.propresenter.selection import select_unique_presentation
from versebridge.presentation.template import (
    create_template_definition, inspect_point_candidates, inspect_template_candidates, validate_template,
)
from versebridge.propresenter.process import is_propresenter_running
from versebridge.references import parse_references
from versebridge.versions.registry import known_versions
from versebridge.workflow import dry_run_import, plan_notes, verify_import_via_api
from versebridge.workflow.result import ImportResult

app = typer.Typer(help="Import Bible references into explicitly selected ProPresenter presentations.", no_args_is_help=True)
config_app = typer.Typer(help="Manage VerseBridge configuration.")
propresenter_app = typer.Typer(help="Inspect the configured ProPresenter instance.")
versions_app = typer.Typer(help="Inspect known Bible translations.")
providers_app = typer.Typer(help="Inspect configured Bible providers.")
cache_app = typer.Typer(help="Inspect or clear the Scripture cache.")
template_app = typer.Typer(help="Select and validate a styled ProPresenter Scripture slide.")
app.add_typer(config_app, name="config")
app.add_typer(propresenter_app, name="propresenter")
app.add_typer(versions_app, name="versions")
app.add_typer(providers_app, name="providers")
app.add_typer(cache_app, name="cache")
app.add_typer(template_app, name="template")
console = Console()


@app.callback()
def main() -> None:
    """VerseBridge command line application."""


def _read_source(file: Path | None, text: str | None) -> str:
    if (file is None) == (text is None):
        raise typer.BadParameter("provide exactly one FILE or --text")
    try:
        return text if text is not None else sys.stdin.read() if str(file) == "-" else extract_text(file)
    except (OSError, DocumentError) as exc:
        raise DocumentError(str(exc)) from exc


def _emit_model(model, output: str) -> None:
    if output.casefold() == "json":
        typer.echo(json.dumps(model.model_dump(mode="json"), indent=2, ensure_ascii=False))
    else:
        console.print(model)


def _fail(exc: Exception) -> None:
    if isinstance(exc, VerseBridgeError):
        console.print(f"[red]{exc}[/red]")
        raise typer.Exit(int(exc.exit_code)) from exc
    console.print(f"[red]{exc}[/red]")
    raise typer.Exit(int(ExitCode.GENERAL_FAILURE)) from exc


@config_app.command("init")
def config_init(
    force: Annotated[bool, typer.Option("--force", help="Replace an existing configuration")] = False,
) -> None:
    try:
        selected = save_config(AppConfig(), overwrite=force)
        console.print(f"Configuration written: {selected}")
    except Exception as exc:
        _fail(exc)


@config_app.command("show")
def config_show(output: Annotated[str, typer.Option("--output")] = "terminal") -> None:
    try:
        config = load_config()
        if output.casefold() == "json":
            typer.echo(config.model_dump_json(indent=2))
        else:
            console.print(f"Configuration: {config_path()}")
            console.print(config.model_dump())
    except Exception as exc:
        _fail(exc)


@app.command("parse")
def parse_command(
    file: Annotated[Optional[Path], typer.Argument(help="Document path, or - for stdin")] = None,
    text: Annotated[Optional[str], typer.Option("--text", help="Parse text supplied directly")] = None,
    output: Annotated[str, typer.Option("--output", help="terminal or json")] = "terminal",
) -> None:
    """Find typed Bible references without retrieving Scripture text."""
    try:
        result = parse_references(_read_source(file, text))
        if output.casefold() == "json":
            typer.echo(result.model_dump_json(indent=2))
        elif output.casefold() == "terminal":
            table = Table("#", "Source", "Normalized", "Version", "Status")
            for index, ref in enumerate(result.references, 1):
                table.add_row(str(index), ref.source_text, ref.normalized_text, ref.version or "—", "Valid")
            for issue in result.issues:
                table.add_row("—", issue.source_text, "—", "—", f"Invalid: {issue.message}")
            console.print(table)
        else:
            raise typer.BadParameter("--output must be terminal or json")
        if result.issues:
            raise typer.Exit(int(ExitCode.PARSING_OR_VALIDATION))
    except typer.Exit:
        raise
    except Exception as exc:
        _fail(exc)


@app.command("doctor")
def doctor(
    host: Annotated[Optional[str], typer.Option("--host")] = None,
    port: Annotated[Optional[int], typer.Option("--port")] = None,
    output: Annotated[str, typer.Option("--output")] = "terminal",
) -> None:
    """Check configuration, process state, and Network API connectivity."""
    async def run():
        config = load_config()
        pp = config.propresenter
        async with ProPresenterApiClient(host or pp.host, port or pp.port, pp.timeout_seconds) as client:
            version = await client.version()
            libraries = await client.libraries()
            return version, libraries
    try:
        version, libraries = asyncio.run(run())
        payload = {
            "healthy": True,
            "process_running": is_propresenter_running(),
            "version": version.model_dump(mode="json"),
            "library_count": len(libraries),
            "files_changed": False,
        }
        if output.casefold() == "json":
            typer.echo(json.dumps(payload, indent=2))
        else:
            console.print(f"[green]Connected[/green]: {version.host_description}, API {version.api_version}")
            console.print(f"Libraries: {len(libraries)} · ProPresenter process running: {payload['process_running']}")
    except Exception as exc:
        _fail(exc)


@versions_app.command("list")
def versions_list(output: Annotated[str, typer.Option("--output")] = "terminal") -> None:
    versions = known_versions()
    if output.casefold() == "json":
        typer.echo(json.dumps([item.model_dump(mode="json") for item in versions], indent=2))
    else:
        table = Table("Abbreviation", "Name", "Provider")
        for version in versions:
            table.add_row(version.abbreviation, version.name, version.provider or "Not configured")
        console.print(table)


@providers_app.command("test")
def providers_test(output: Annotated[str, typer.Option("--output")] = "terminal") -> None:
    async def run():
        registry = ProviderRegistry(load_config().bible)
        try:
            return await registry.health_checks()
        finally:
            await registry.aclose()
    try:
        results = asyncio.run(run())
        if output.casefold() == "json":
            typer.echo(json.dumps([item.model_dump(mode="json") for item in results], indent=2))
        else:
            table = Table("Provider", "Healthy", "Versions", "Message")
            for item in results:
                table.add_row(item.provider or "—", "Yes" if item.healthy else "No", ", ".join(item.versions), item.message)
            console.print(table)
        if any(not item.healthy for item in results):
            raise typer.Exit(int(ExitCode.BIBLE_PROVIDER))
    except typer.Exit:
        raise
    except Exception as exc:
        _fail(exc)


@cache_app.command("inspect")
def cache_inspect(output: Annotated[str, typer.Option("--output")] = "terminal") -> None:
    with ScriptureCache() as cache:
        payload = {"path": str(cache.path), "verse_count": cache.count()}
    typer.echo(json.dumps(payload, indent=2) if output.casefold() == "json" else f"{payload['verse_count']} cached verse(s) at {payload['path']}")


@cache_app.command("clear")
def cache_clear() -> None:
    with ScriptureCache() as cache:
        count = cache.clear()
    console.print(f"Cleared {count} cached verse(s).")


async def _resolve_template_target(config: AppConfig, target_uuid: str | None, target_name: str | None, library: str | None):
    pp = config.propresenter
    async with ProPresenterApiClient(pp.host, pp.port, pp.timeout_seconds) as client:
        selected = select_unique_presentation(
            await client.presentations(), uuid=target_uuid, name=target_name, library=library
        )
        metadata = await client.presentation(selected.uuid)
        return selected, metadata


def _with_pp_overrides(config: AppConfig, host: str | None, port: int | None) -> AppConfig:
    if not host and not port:
        return config
    return config.model_copy(update={"propresenter": config.propresenter.model_copy(update={
        "host": host or config.propresenter.host,
        "port": port or config.propresenter.port,
    })})


@template_app.command("inspect")
def template_inspect(
    target_uuid: Annotated[Optional[str], typer.Option("--target-uuid")] = None,
    target_presentation: Annotated[Optional[str], typer.Option("--target-presentation")] = None,
    library: Annotated[Optional[str], typer.Option("--library")] = None,
    host: Annotated[Optional[str], typer.Option("--host")] = None,
    port: Annotated[Optional[int], typer.Option("--port")] = None,
    output: Annotated[str, typer.Option("--output")] = "terminal",
) -> None:
    try:
        config = _with_pp_overrides(load_config(), host, port)
        selected, metadata = asyncio.run(_resolve_template_target(config, target_uuid, target_presentation, library))
        candidates = inspect_template_candidates(metadata.presentation_path)
        if output.casefold() == "json":
            typer.echo(json.dumps([item.model_dump(mode="json") for item in candidates], indent=2))
            return
        table = Table("Slide", "Reference", "Verse preview", "Cue UUID")
        for item in candidates:
            table.add_row(str(item.slide_number), item.reference_preview, item.body_preview, item.cue_uuid)
        console.print(f"{selected.library_name} / {selected.name}")
        console.print(table)
    except Exception as exc:
        _fail(exc)


@template_app.command("select")
def template_select(
    target_uuid: Annotated[Optional[str], typer.Option("--target-uuid")] = None,
    target_presentation: Annotated[Optional[str], typer.Option("--target-presentation")] = None,
    library: Annotated[Optional[str], typer.Option("--library")] = None,
    cue_uuid: Annotated[Optional[str], typer.Option("--cue-uuid")] = None,
    host: Annotated[Optional[str], typer.Option("--host")] = None,
    port: Annotated[Optional[int], typer.Option("--port")] = None,
) -> None:
    try:
        config = _with_pp_overrides(load_config(), host, port)
        selected, metadata = asyncio.run(_resolve_template_target(config, target_uuid, target_presentation, library))
        candidates = inspect_template_candidates(metadata.presentation_path)
        if not candidates:
            raise typer.BadParameter("no slides contain exactly one Reference and one Verse text object")
        chosen = next((item for item in candidates if cue_uuid and item.cue_uuid.casefold() == cue_uuid.casefold()), None)
        if cue_uuid and not chosen:
            raise typer.BadParameter("--cue-uuid is not one of the viable template cues")
        if not chosen:
            table = Table("Choice", "Slide", "Reference", "Verse preview")
            for choice, item in enumerate(candidates, 1):
                table.add_row(str(choice), str(item.slide_number), item.reference_preview, item.body_preview)
            console.print(table)
            choice = typer.prompt("Choose template", type=int)
            if choice < 1 or choice > len(candidates):
                raise typer.BadParameter("template choice is out of range")
            chosen = candidates[choice - 1]
        definition = create_template_definition(
            metadata.presentation_path, presentation_uuid=selected.uuid, presentation_name=selected.name,
            library_name=selected.library_name, cue_uuid=chosen.cue_uuid,
        )
        updated = config.model_copy(update={
            "presentation_template": definition,
            "slides": config.slides.model_copy(update={"include_verse_numbers": False}),
        })
        save_config(updated, overwrite=True)
        console.print(f"Template saved from slide {chosen.slide_number}: {chosen.reference_preview}")
    except typer.BadParameter:
        raise
    except Exception as exc:
        _fail(exc)


@template_app.command("select-point")
def template_select_point(
    cue_uuid: Annotated[Optional[str], typer.Option("--cue-uuid", help="Cue holding the sermon-point template slide")] = None,
    point_object: Annotated[str, typer.Option("--object", help="Name of the point text object")] = "Sermon Point",
) -> None:
    """Choose the slide used for sermon points (a slide with one text object named 'Sermon Point')."""
    try:
        config = load_config()
        definition = config.presentation_template
        if not definition:
            raise typer.BadParameter("select the verse template first: versebridge template select")
        selected, metadata = asyncio.run(_resolve_template_target(config, definition.presentation_uuid, None, definition.library_name))
        candidates = inspect_point_candidates(metadata.presentation_path, point_object=point_object)
        if not candidates:
            raise typer.BadParameter(f"no slide in {selected.name} has exactly one text object named {point_object!r}")
        chosen = next((item for item in candidates if cue_uuid and item[1].casefold() == cue_uuid.casefold()), None)
        if cue_uuid and not chosen:
            raise typer.BadParameter("--cue-uuid is not one of the viable sermon-point cues")
        if not chosen:
            if len(candidates) == 1:
                chosen = candidates[0]
            else:
                table = Table("Choice", "Slide", "Cue UUID")
                for number, item in enumerate(candidates, 1):
                    table.add_row(str(number), str(item[0]), item[1])
                console.print(table)
                choice = typer.prompt("Choose sermon-point template", type=int)
                if choice < 1 or choice > len(candidates):
                    raise typer.BadParameter("choice is out of range")
                chosen = candidates[choice - 1]
        updated = definition.model_copy(update={
            "point_cue_uuid": chosen[1], "point_object": point_object, "point_style_fingerprint": chosen[2],
        })
        save_config(config.model_copy(update={"presentation_template": updated}), overwrite=True)
        console.print(f"Sermon-point template saved from slide {chosen[0]} of {selected.name}")
    except typer.BadParameter:
        raise
    except Exception as exc:
        _fail(exc)


@template_app.command("validate")
def template_validate(output: Annotated[str, typer.Option("--output")] = "terminal") -> None:
    try:
        config = load_config()
        definition = config.presentation_template
        if not definition:
            raise typer.BadParameter("no presentation template is configured; run template select")
        selected, metadata = asyncio.run(_resolve_template_target(
            config, definition.presentation_uuid, None, definition.library_name
        ))
        candidate = validate_template(metadata.presentation_path, definition)
        payload = {
            "valid": True, "presentation_uuid": selected.uuid, "cue_uuid": candidate.cue_uuid,
            "slide_number": candidate.slide_number, "reference_object": definition.reference_object,
            "body_object": definition.body_object, "files_changed": False,
        }
        typer.echo(json.dumps(payload, indent=2) if output.casefold() == "json" else f"Template is valid: slide {candidate.slide_number} in {selected.name}")
    except typer.BadParameter:
        raise
    except Exception as exc:
        _fail(exc)


async def _pp_data(kind: str, config: AppConfig):
    pp = config.propresenter
    async with ProPresenterApiClient(pp.host, pp.port, pp.timeout_seconds) as client:
        if kind == "libraries":
            return await client.libraries()
        if kind == "presentations":
            return await client.presentations()
        return await client.playlists()


def _propresenter_list(kind: str, output: str) -> None:
    try:
        items = asyncio.run(_pp_data(kind, load_config()))
        if output.casefold() == "json":
            typer.echo(json.dumps([item.model_dump(mode="json") for item in items], indent=2))
        else:
            table = Table("Name", "UUID", "Library")
            for item in items:
                identifier = getattr(item, "id", None)
                table.add_row(
                    getattr(item, "name", None) or (identifier.name if identifier else "—"),
                    getattr(item, "uuid", None) or (identifier.uuid if identifier else "—"),
                    getattr(item, "library_name", None) or "—",
                )
            console.print(table)
    except Exception as exc:
        _fail(exc)


@propresenter_app.command("libraries")
def propresenter_libraries(output: Annotated[str, typer.Option("--output")] = "terminal") -> None:
    _propresenter_list("libraries", output)


@propresenter_app.command("presentations")
def propresenter_presentations(output: Annotated[str, typer.Option("--output")] = "terminal") -> None:
    _propresenter_list("presentations", output)


@propresenter_app.command("playlists")
def propresenter_playlists(output: Annotated[str, typer.Option("--output")] = "terminal") -> None:
    _propresenter_list("playlists", output)


class _NotesAdapter:
    """Presents a notes plan with the attributes the shared write path expects."""

    def __init__(self, plan) -> None:
        from types import SimpleNamespace
        self.target, self.slide_plan = plan.target, plan.slide_plan
        versions = {slide.version_text for slide in plan.slide_plan.slides if slide.kind == "verse" and slide.version_text}
        self.references = [SimpleNamespace(version=value) for value in sorted(versions)]
        self.references += [SimpleNamespace(version=None)] * (plan.verse_count + plan.point_count - len(self.references))


def _print_notes_report(result) -> None:
    table = Table("#", "Type", "Slide text")
    for index, slide in enumerate(result.slide_plan.slides, 1):
        if slide.kind == "point":
            table.add_row(str(index), "point", slide.body_text.replace("\n", " / ")[:70])
        else:
            label = f"{slide.reference_text} {slide.version_text}".strip()
            table.add_row(str(index), "verse", f"{label}: {slide.body_text[:50]}")
    console.print(table)
    console.print(f"Slides planned: {len(result.slide_plan.slides)} ({result.verse_count} verse passages, {result.point_count} points)")
    for title, entries, style in (
        ("NOT ADDED (failed)", result.failed, "red"),
        ("NEEDS REVIEW (unclear lines)", result.review, "yellow"),
        ("Corrections / defaults applied", result.notices, "cyan"),
        ("Left out on purpose", result.skipped, "dim"),
    ):
        if entries:
            console.print(f"[{style}]{title}: {len(entries)}[/{style}]")
            for entry in entries:
                console.print(f"  [{style}]line {entry.line}: {entry.text[:60]!r} - {entry.problem}[/{style}]")


@app.command("import")
def import_command(
    file: Annotated[Optional[Path], typer.Argument(help="Document path, or - for stdin")] = None,
    text: Annotated[Optional[str], typer.Option("--text")] = None,
    default_version: Annotated[Optional[str], typer.Option("--default-version")] = None,
    target_presentation: Annotated[Optional[str], typer.Option("--target-presentation")] = None,
    target_uuid: Annotated[Optional[str], typer.Option("--target-uuid")] = None,
    library: Annotated[Optional[str], typer.Option("--library")] = None,
    host: Annotated[Optional[str], typer.Option("--host")] = None,
    port: Annotated[Optional[int], typer.Option("--port")] = None,
    dry_run: Annotated[bool, typer.Option("--dry-run")] = False,
    no_fetch: Annotated[bool, typer.Option("--no-fetch")] = False,
    non_interactive: Annotated[bool, typer.Option("--non-interactive")] = False,
    yes: Annotated[bool, typer.Option("--yes", help="Confirm the reviewed write transaction")] = False,
    notes: Annotated[bool, typer.Option("--notes", help="Treat the document as sermon notes: verses plus sermon points")] = False,
    output: Annotated[str, typer.Option("--output")] = "terminal",
) -> None:
    """Plan or execute an explicitly targeted Scripture-slide import.

    Dry runs never write. Production imports require fetched Scripture, a
    validated template, an explicit target, and confirmation.
    """
    try:
        if not dry_run and no_fetch:
            raise typer.BadParameter("production imports cannot use --no-fetch")
        if not dry_run and not (target_uuid or target_presentation):
            raise typer.BadParameter("production imports require --target-uuid or --target-presentation")
        config = _with_pp_overrides(load_config(), host, port)
        if notes:
            result = asyncio.run(plan_notes(
                _read_source(file, text), config, default_version=default_version, no_fetch=no_fetch,
                target_uuid=target_uuid, target_name=target_presentation, library=library,
            ))
            if output.casefold() == "json":
                typer.echo(result.model_dump_json(indent=2))
            else:
                _print_notes_report(result)
            if dry_run:
                console.print("Dry run: no files or playlists changed.")
                return
            if result.needs_attention and not yes:
                raise ConfigurationError(
                    f"{len(result.failed)} item(s) failed and {len(result.review)} need review; "
                    "fix them or rerun with --yes to import only the slides that were planned"
                )
            result = _NotesAdapter(result)
        else:
            result = asyncio.run(dry_run_import(
                _read_source(file, text), config, default_version=default_version,
                non_interactive=non_interactive, target_uuid=target_uuid,
                target_name=target_presentation, library=library,
                validate_target=bool(target_uuid or target_presentation),
                no_fetch=no_fetch,
            ))
        if dry_run and not notes and output.casefold() == "json":
            typer.echo(result.model_dump_json(indent=2))
            return
        if output.casefold() != "json" and not notes:
            table = Table("#", "Reference", "Version", "Status")
            for index, reference in enumerate(result.references, 1):
                status = "Default applied" if "default version applied" in reference.warnings else "Ready"
                table.add_row(str(index), reference.normalized_text, reference.version or "—", status)
            console.print(table)
            if result.target:
                console.print(f"Destination: {result.target.library_name} / {result.target.presentation_name}")
            console.print(f"Slides planned: {len(result.slide_plan.slides) if result.slide_plan else 0}")
        if dry_run:
            console.print("Dry run: no files or playlists changed.")
            return

        if not result.target or not result.slide_plan:
            raise ConfigurationError("Production import target or slide plan is missing")
        definition = config.presentation_template
        if not definition:
            raise ConfigurationError("No template is configured; run versebridge template select")
        template_selected, template_metadata = asyncio.run(_resolve_template_target(
            config, definition.presentation_uuid, None, definition.library_name
        ))
        validate_template(template_metadata.presentation_path, definition)

        running = is_propresenter_running()
        if running and non_interactive:
            raise ConfigurationError(
                "Noninteractive writes require ProPresenter to be closed so an in-memory presentation cannot overwrite the import"
            )
        if running:
            typer.prompt(
                "Close ProPresenter completely, then press Enter",
                default="", show_default=False,
            )
            if is_propresenter_running():
                raise ConfigurationError(
                    "ProPresenter is still running. Close it completely before importing; "
                    "an open process can overwrite the imported file or create duplicate .pro files"
                )
        if not yes and not typer.confirm(
            f"Append {len(result.slide_plan.slides)} slide(s) to {result.target.presentation_name} with a backup?"
        ):
            raise typer.Exit(int(ExitCode.USER_CANCELLATION))

        writer = ProtobufPresentationWriter()
        try:
            written = writer.append_plan(
                result.target.path, result.slide_plan, definition,
                template_path=template_metadata.presentation_path,
            )
        except Exception as exc:
            raise PresentationWriteError(f"Presentation write failed: {exc}. No playlist changes were made.") from exc

        verification = "Structural verification passed; ProPresenter was closed during the write"
        if running:
            typer.prompt(
                f"Reopen ProPresenter and open '{result.target.presentation_name}', then press Enter for API verification",
                default="", show_default=False,
            )
            valid, verification = asyncio.run(verify_import_via_api(
                config, result.target, written, result.slide_plan
            ))
            if not valid:
                typer.prompt(
                    f"Verification failed ({verification}). Close ProPresenter completely, then press Enter to roll back",
                    default="", show_default=False,
                )
                if is_propresenter_running():
                    raise VerificationRollbackError(
                        f"Verification failed, but rollback was not attempted because ProPresenter is still running. "
                        f"Close it and restore {written.backup_path} over {result.target.path}: {verification}"
                    )
                restore_backup(result.target.path, written.backup_path)
                raise VerificationRollbackError(
                    f"Verification failed and the original was restored from {written.backup_path}. Rollback succeeded: {verification}"
                )

        final = ImportResult(
            target=result.target,
            reference_count=len(result.references),
            slide_count=len(result.slide_plan.slides),
            versions=sorted({item.version for item in result.references if item.version}),
            backup_path=written.backup_path,
            cue_count_before=written.cue_count_before,
            cue_count_after=written.cue_count_after,
            group_count_before=written.group_count_before,
            group_count_after=written.group_count_after,
            verification_result=verification,
        )
        append_audit_entry(user_log_path("versebridge") / "audit.jsonl", AuditEntry(
            application_version=__version__,
            propresenter_version="19.0.1",
            target_uuid=result.target.uuid,
            presentation_name=result.target.presentation_name,
            reference_count=final.reference_count,
            slide_count=final.slide_count,
            versions=final.versions,
            backup_path=final.backup_path,
            verification_result=final.verification_result,
        ))
        if output.casefold() == "json":
            typer.echo(final.model_dump_json(indent=2))
        else:
            console.print(f"[green]Import verified[/green]: {final.slide_count} slide(s) appended")
            console.print(f"Backup: {final.backup_path}")
            console.print(final.verification_result)
    except typer.BadParameter:
        raise
    except Exception as exc:
        _fail(exc)


from versebridge.cli_run import register as _register_run  # noqa: E402

_register_run(app, template_app)


if __name__ == "__main__":
    app()
