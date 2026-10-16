"""Automation-friendly commands: run, undo, history, template setup, node check.

Exit codes: 0 success; 10 finished but some lines were flagged; other codes as in `versebridge --help`.
"""
from __future__ import annotations

import asyncio
import json
from pathlib import Path
from typing import Annotated, Optional

import typer
from rich.console import Console
from rich.table import Table

from versebridge.config import AppConfig, NodeConfig, load_config, save_config
from versebridge.documents import extract_text
from versebridge.errors import ConfigurationError, DocumentError, ExitCode, VerseBridgeError
from versebridge.history import HistoryStore
from versebridge.node.transport import make_transport
from versebridge.presentation.template import (
    create_template_definition, inspect_point_candidates, inspect_template_candidates,
)
from versebridge.workflow.pipeline import _target_from_api, apply_import, plan_for_transport, undo_import
from versebridge.workflow.report import build_report

console = Console(stderr=True)


def _config_with_node(config: AppConfig, node: str | None, host: str | None, port: int | None) -> AppConfig:
    update = {}
    if node:
        base = config.node if config.node.type == "hermes-remote" else NodeConfig(type="hermes-remote")
        update["node"] = base.model_copy(update={"type": "hermes-remote", "name": node})
    if host or port:
        update["propresenter"] = config.propresenter.model_copy(update={
            "host": host or config.propresenter.host, "port": port or config.propresenter.port,
        })
    return config.model_copy(update=update) if update else config


def _read(file: Path | None, text: str | None) -> tuple[str, str]:
    if (file is None) == (text is None):
        raise typer.BadParameter("provide exactly one FILE or --text")
    if text is not None:
        return text, "text"
    try:
        return extract_text(file), file.name  # type: ignore[arg-type]
    except (OSError, DocumentError) as exc:
        raise DocumentError(str(exc)) from exc


def _fail(exc: Exception, as_json: bool) -> None:
    code = int(exc.exit_code) if isinstance(exc, VerseBridgeError) else int(ExitCode.GENERAL_FAILURE)
    if as_json:
        typer.echo(json.dumps({"ok": False, "error": str(exc), "exit_code": code}, indent=2))
    else:
        console.print(f"[red]{exc}[/red]")
    raise typer.Exit(code) from exc


def _print_report(report: dict) -> None:
    s = report["summary"]
    table = Table("#", "Type", "Slide")
    for slide in report["slides"]:
        table.add_row(str(slide["n"]), slide["kind"], f"{slide['title']}" + ("" if slide["kind"] == "point" else f": {slide['text'][:50]}"))
    console.print(table)
    console.print(f"{s['slides']} slides ({s['verse_slides']} verse, {s['point_slides']} point)")
    for title, key, style in (("FLAGGED", "flagged", "red"), ("Adjustments", "adjustments", "cyan"), ("Left out", "left_out", "dim")):
        rows = report[key]
        if rows:
            console.print(f"[{style}]{title}: {len(rows)}[/{style}]")
            for row in rows:
                console.print(f"  [{style}]line {row['line']}: {row['text'][:60]!r} - {row['detail']}[/{style}]")


def register(app: typer.Typer, template_app: typer.Typer) -> None:
    @app.command("run")
    def run_command(
        file: Annotated[Optional[Path], typer.Argument(help="Notes file (.pdf .docx .txt .md .html)")] = None,
        text: Annotated[Optional[str], typer.Option("--text")] = None,
        target_presentation: Annotated[Optional[str], typer.Option("--target-presentation", "-t")] = None,
        target_uuid: Annotated[Optional[str], typer.Option("--target-uuid")] = None,
        library: Annotated[Optional[str], typer.Option("--library")] = None,
        node: Annotated[Optional[str], typer.Option("--node", help="Remote computer running ProPresenter")] = None,
        default_version: Annotated[Optional[str], typer.Option("--default-version")] = None,
        dry_run: Annotated[bool, typer.Option("--dry-run", help="Plan and report only; change nothing")] = False,
        yes: Annotated[bool, typer.Option("--yes", help="Import even when some lines are flagged")] = False,
        no_replace: Annotated[bool, typer.Option("--no-replace", help="Append even if this file was imported before")] = False,
        no_close: Annotated[bool, typer.Option("--no-close", help="Never close ProPresenter automatically")] = False,
        no_reopen: Annotated[bool, typer.Option("--no-reopen")] = False,
        output: Annotated[str, typer.Option("--output")] = "terminal",
    ) -> None:
        """Turn sermon notes into ProPresenter slides (verses and sermon points)."""
        as_json = output.casefold() == "json"
        try:
            config = _config_with_node(load_config(), node, None, None)
            source, source_name = _read(file, text)
            if not (target_presentation or target_uuid):
                raise ConfigurationError("a target is required: --target-presentation NAME or --target-uuid UUID")
            transport = make_transport(config.propresenter, config.node)
            plan, target = asyncio.run(plan_for_transport(
                source, config, transport, default_version=default_version,
                target_uuid=target_uuid, target_name=target_presentation, library=library,
            ))
            report = build_report(plan, source_name=source_name)
            report["target"] = {"name": target[2], "uuid": target[1], "library": target[0]} if target else None
            report["node"] = transport.description
            flags = bool(report["flagged"])
            if dry_run:
                report.update(ok=True, dry_run=True, exit_code=int(ExitCode.COMPLETED_WITH_FLAGS if flags else 0))
                typer.echo(json.dumps(report, indent=2, ensure_ascii=False)) if as_json else _print_report(report)
                raise typer.Exit(report["exit_code"])
            if flags and not yes:
                report.update(ok=False, error="Some lines are flagged. Fix them, or rerun with --yes to import the rest.", exit_code=int(ExitCode.PARSING_OR_VALIDATION))
                typer.echo(json.dumps(report, indent=2, ensure_ascii=False)) if as_json else _print_report(report)
                raise typer.Exit(int(ExitCode.PARSING_OR_VALIDATION))
            applied = apply_import(
                source, source_name, plan, target, config, transport,
                replace=not no_replace, close_app=not no_close, reopen=not no_reopen,
                progress=(lambda m: console.print(f"[cyan]{m}[/cyan]")) if not as_json else (lambda m: None),
            )
            report.update(ok=True, applied=applied.model_dump(), exit_code=int(ExitCode.COMPLETED_WITH_FLAGS if flags else 0))
            if as_json:
                typer.echo(json.dumps(report, indent=2, ensure_ascii=False))
            else:
                _print_report(report)
                console.print(f"[green]Imported {applied.slide_count} slides into {applied.target_name}[/green] (import {applied.import_id})")
                console.print(applied.verification)
            raise typer.Exit(report["exit_code"])
        except typer.Exit:
            raise
        except Exception as exc:
            _fail(exc, as_json)

    @app.command("undo")
    def undo_command(
        import_id: Annotated[str, typer.Argument(help="Import id from `versebridge history`")],
        node: Annotated[Optional[str], typer.Option("--node")] = None,
        no_close: Annotated[bool, typer.Option("--no-close")] = False,
        output: Annotated[str, typer.Option("--output")] = "terminal",
    ) -> None:
        """Remove the slides a previous import added (and nothing else)."""
        as_json = output.casefold() == "json"
        try:
            config = _config_with_node(load_config(), node, None, None)
            result = undo_import(import_id, config, make_transport(config.propresenter, config.node), close_app=not no_close,
                                 progress=(lambda m: console.print(f"[cyan]{m}[/cyan]")) if not as_json else (lambda m: None))
            if as_json:
                typer.echo(json.dumps({"ok": True, **result.model_dump()}, indent=2))
            else:
                console.print(f"[green]Removed {result.removed_slides} slides from {result.target_name}[/green]")
        except Exception as exc:
            _fail(exc, as_json)

    @app.command("serve")
    def serve_command(
        host: Annotated[str, typer.Option("--host", help="Address to listen on; the default keeps it on this computer")] = "127.0.0.1",
        port: Annotated[int, typer.Option("--port")] = 8777,
        node: Annotated[Optional[str], typer.Option("--node")] = None,
        open_browser: Annotated[bool, typer.Option("--open/--no-open")] = False,
    ) -> None:
        """Start the local web page (drag and drop notes, review, import)."""
        from versebridge.web.server import serve
        config = _config_with_node(load_config(), node, None, None)
        server, state = serve(config, host=host, port=port)
        link = f"http://{host if host not in {'0.0.0.0', '::'} else '127.0.0.1'}:{port}/" + (f"?token={state.token}" if state.token else "")
        console.print(f"[green]VerseBridge is running[/green]  {link}")
        if state.token:
            console.print("This address is open to your network, so the link above includes a private access token.")
        if open_browser:
            import webbrowser
            webbrowser.open(link)
        try:
            server.serve_forever()
        except KeyboardInterrupt:
            console.print("Stopped.")
        finally:
            server.server_close()

    @app.command("history")
    def history_command(output: Annotated[str, typer.Option("--output")] = "terminal") -> None:
        """List earlier imports."""
        records = HistoryStore().load()
        if output.casefold() == "json":
            typer.echo(json.dumps([r.model_dump(mode="json") for r in records], indent=2))
            return
        table = Table("Id", "When (UTC)", "Source", "Target", "Slides", "Status")
        for r in records:
            table.add_row(r.import_id, r.created.strftime("%Y-%m-%d %H:%M"), r.source_name, r.target_name, str(r.slide_count), r.status)
        Console().print(table)

    @app.command("node")
    def node_command(
        node: Annotated[Optional[str], typer.Option("--node")] = None,
        output: Annotated[str, typer.Option("--output")] = "terminal",
    ) -> None:
        """Check the connection to the computer that runs ProPresenter."""
        as_json = output.casefold() == "json"
        try:
            config = _config_with_node(load_config(), node, None, None)
            transport = make_transport(config.propresenter, config.node)
            info = transport.api_get("/version")
            payload = {"ok": True, "node": transport.description, "propresenter": info, "app_running": transport.is_running()}
            typer.echo(json.dumps(payload, indent=2)) if as_json else console.print(
                f"[green]Connected[/green] to {transport.description}: {info.get('host_description')}, API {info.get('api_version')}"
            )
        except Exception as exc:
            _fail(exc, as_json)

    @template_app.command("check")
    def template_check(
        node: Annotated[Optional[str], typer.Option("--node")] = None,
        output: Annotated[str, typer.Option("--output")] = "terminal",
    ) -> None:
        """Check that the saved verse and sermon-point templates still exist and look the same (works through a node)."""
        as_json = output.casefold() == "json"
        try:
            from versebridge.presentation.template import validate_template
            config = _config_with_node(load_config(), node, None, None)
            definition = config.presentation_template
            if not definition:
                raise ConfigurationError("no template is configured; run: versebridge template setup")
            transport = make_transport(config.propresenter, config.node)
            lib, uuid_, name, path, _ = _target_from_api(transport, uuid_=definition.presentation_uuid, name=None, library=None)
            import tempfile
            local = Path(tempfile.mkdtemp(prefix="versebridge-")) / Path(path).name
            transport.fetch_file(path, local)
            candidate = validate_template(local, definition)
            payload = {"ok": True, "presentation": name, "verse_slide": candidate.slide_number, "has_point_template": bool(definition.point_cue_uuid)}
            typer.echo(json.dumps(payload, indent=2)) if as_json else console.print(f"[green]Templates are valid[/green] in {name} (verse slide {candidate.slide_number})")
        except Exception as exc:
            _fail(exc, as_json)

    @template_app.command("setup")
    def template_setup(
        target_presentation: Annotated[str, typer.Option("--target-presentation", "-t")] = "Verses",
        library: Annotated[Optional[str], typer.Option("--library")] = None,
        verse_slide: Annotated[Optional[int], typer.Option("--verse-slide", help="Slide number holding the verse template")] = None,
        point_slide: Annotated[Optional[int], typer.Option("--point-slide", help="Slide number holding the sermon-point template")] = None,
        node: Annotated[Optional[str], typer.Option("--node")] = None,
    ) -> None:
        """Pick the verse and sermon-point template slides (works through a remote node)."""
        try:
            config = _config_with_node(load_config(), node, None, None)
            transport = make_transport(config.propresenter, config.node)
            lib, uuid_, name, path, _ = _target_from_api(transport, uuid_=None, name=target_presentation, library=library)
            import tempfile
            local = Path(tempfile.mkdtemp(prefix="versebridge-")) / Path(path).name
            transport.fetch_file(path, local)
            verses = inspect_template_candidates(local)
            points = inspect_point_candidates(local)
            if not verses:
                raise ConfigurationError("no slide has exactly one 'Reference' and one 'Verse' text object")
            chosen = next((v for v in verses if v.slide_number == verse_slide), None) if verse_slide else verses[0]
            if verse_slide and not chosen:
                raise ConfigurationError(f"slide {verse_slide} is not a valid verse template; valid: {[v.slide_number for v in verses]}")
            definition = create_template_definition(local, presentation_uuid=uuid_, presentation_name=name, library_name=lib, cue_uuid=chosen.cue_uuid)
            if points:
                pick = next((p for p in points if p[0] == point_slide), None) if point_slide else points[0]
                if point_slide and not pick:
                    raise ConfigurationError(f"slide {point_slide} is not a valid sermon-point template; valid: {[p[0] for p in points]}")
                definition = definition.model_copy(update={"point_cue_uuid": pick[1], "point_style_fingerprint": pick[2]})
            updated = config.model_copy(update={
                "presentation_template": definition,
                "slides": config.slides.model_copy(update={"include_verse_numbers": config.slides.include_verse_numbers}),
            })
            save_config(updated, overwrite=True)
            console.print(f"[green]Saved[/green] verse template from slide {chosen.slide_number}"
                          + (f" and point template from slide {pick[0]}" if points else " (no sermon-point slide found)"))
        except Exception as exc:
            _fail(exc, False)
