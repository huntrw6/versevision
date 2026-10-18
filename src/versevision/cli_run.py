"""Automation-friendly commands: run, undo, history, template setup, node check.

Exit codes: 0 success; 10 finished but some lines were flagged; other codes as in `versevision --help`.
"""
from __future__ import annotations

import asyncio
import json
import os
import time
from pathlib import Path
from typing import Annotated, Optional

import typer
from rich.console import Console
from rich.table import Table

from versevision.config import AppConfig, NodeConfig, load_config, save_config
from versevision.documents import extract_text
from versevision.errors import ConfigurationError, DocumentError, ExitCode, VerseVisionError
from versevision.history import HistoryStore
from versevision.node.transport import make_transport
from versevision.presentation.template import (
    create_template_definition, inspect_point_candidates, inspect_template_candidates,
)
from versevision.presentation.protobuf_writer import ProtobufPresentationWriter
from versevision.workflow.pipeline import _target_from_api, _template_target, apply_import, list_presentations, plan_for_transport, publish_new, publish_swap, undo_import
from versevision.workflow.report import build_report

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
    code = int(exc.exit_code) if isinstance(exc, VerseVisionError) else int(ExitCode.GENERAL_FAILURE)
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
    @app.command("presentations")
    def presentations_command(
        node: Annotated[Optional[str], typer.Option("--node")] = None,
        library: Annotated[Optional[str], typer.Option("--library")] = None,
        all_libraries: Annotated[bool, typer.Option("--all", help="Include every library (songs too)")] = False,
        output: Annotated[str, typer.Option("--output")] = "terminal",
    ) -> None:
        """List presentations in the output library (default: Verses). Use a name with `run -p NAME --replace/--append`."""
        as_json = output.casefold() == "json"
        try:
            from versevision.workflow.pipeline import list_presentations
            config = _config_with_node(load_config(), node, None, None)
            items = list_presentations(make_transport(config.propresenter, config.node))
            if library:
                items = [i for i in items if i["library"].casefold() == library.casefold()]
            elif not all_libraries and not as_json:
                items = [i for i in items if i["library"].casefold() == config.output.library.casefold()]
            if as_json:
                typer.echo(json.dumps(items, indent=2, ensure_ascii=False))
                return
            table = Table("Library", "Presentation", "UUID")
            for item in items:
                table.add_row(item["library"], item["name"], item["uuid"])
            Console().print(table)
        except Exception as exc:
            _fail(exc, as_json)

    @app.command("run")
    def run_command(
        file: Annotated[Optional[Path], typer.Argument(help="Notes file (.pdf .docx .txt .md .html)")] = None,
        text: Annotated[Optional[str], typer.Option("--text")] = None,
        presentation: Annotated[Optional[str], typer.Option("--presentation", "-p", help="Existing presentation to change instead of making a new one (needs --replace or --append)")] = None,
        presentation_uuid: Annotated[Optional[str], typer.Option("--presentation-uuid")] = None,
        title: Annotated[Optional[str], typer.Option("--title", help="Name for the new presentation (default: the document's title)")] = None,
        library: Annotated[Optional[str], typer.Option("--library", help="Library for the new presentation (default: Verses)")] = None,
        replace: Annotated[bool, typer.Option("--replace", help="Copy the chosen presentation without its verse and point slides (blank/background/media stay) and add the new ones")] = False,
        append: Annotated[bool, typer.Option("--append", help="Copy the chosen presentation and add the slides to the end")] = False,
        node: Annotated[Optional[str], typer.Option("--node", help="Remote computer running ProPresenter")] = None,
        default_version: Annotated[Optional[str], typer.Option("--default-version")] = None,
        dry_run: Annotated[bool, typer.Option("--dry-run", help="Plan and report only; change nothing")] = False,
        yes: Annotated[bool, typer.Option("--yes", help="Import even when some lines are flagged")] = False,
        no_playlist: Annotated[bool, typer.Option("--no-playlist", help="Do not add the new file to the playlists that hold the original")] = False,
        as_copy: Annotated[bool, typer.Option("--as-copy", help="Keep the original and save the result as a numbered copy (Name 2) instead of replacing it")] = False,
        in_place: Annotated[bool, typer.Option("--in-place", help="Edit the original file instead of saving a copy (closes ProPresenter)")] = False,
        no_close: Annotated[bool, typer.Option("--no-close", help="With --in-place: never close ProPresenter automatically")] = False,
        no_reopen: Annotated[bool, typer.Option("--no-reopen")] = False,
        output: Annotated[str, typer.Option("--output")] = "terminal",
        target_presentation: Annotated[Optional[str], typer.Option("--target-presentation", "-t", hidden=True)] = None,
        target_uuid: Annotated[Optional[str], typer.Option("--target-uuid", hidden=True)] = None,
    ) -> None:
        """Turn sermon notes into ProPresenter slides (verses and sermon points).

        By default this creates a NEW presentation named after the document. With -p NAME and --replace
        (swap its verse and point slides) or --append (add to the end), the presentation is replaced under its
        own name: the original is backed up, renamed, replaced by the new version, and removed once ProPresenter
        shows the new one. Playlists keep pointing at it. ProPresenter stays open the whole time.
        --as-copy keeps the original and saves "NAME 2"; --in-place edits the file itself (needs a restart).
        """
        as_json = output.casefold() == "json"
        from versevision import runlog
        run, token, st = None, None, {"outcome": "failed", "error": None, "report": None, "applied": None}
        try:
            config = _config_with_node(load_config(), node, None, None)
            source, source_name = _read(file, text)
            mode_name = "replace" if replace else "append" if append else "new"
            run = runlog.RunLog.start(
                "run", source=source_name, mode=mode_name + (" (dry run)" if dry_run else ""), title=title, library=library,
                target=presentation or target_presentation or presentation_uuid or target_uuid, node=config.node.name or "this computer",
                default_version=default_version or config.bible.default_version,
                flags=", ".join(f for f, on in (("--yes", yes), ("--no-playlist", no_playlist), ("--in-place", in_place)) if on) or None,
            )
            token = runlog.activate(run)
            run.save_source(source, source_name)
            chosen = presentation or target_presentation
            chosen_uuid = presentation_uuid or target_uuid
            editing = bool(chosen or chosen_uuid)
            if replace and append:
                raise ConfigurationError("choose either --replace or --append, not both")
            if (replace or append) and not editing:
                raise ConfigurationError("--replace and --append change an existing presentation: name it with -p NAME (see: versevision presentations)")
            if editing and not (replace or append):
                raise ConfigurationError("-p changes an existing presentation: add --replace (swap its verse and point slides) or --append. Leave -p out to make a new presentation")
            transport = make_transport(config.propresenter, config.node)
            plan, target = asyncio.run(plan_for_transport(
                source, config, transport, default_version=default_version,
                target_uuid=chosen_uuid, target_name=chosen, library=library if editing else None,
            ))
            from versevision.notes.title import document_title
            new_title = title or document_title(source, source_name)
            report = build_report(plan, source_name=source_name)
            report["mode"] = "replace" if replace else "append" if append else "new"
            report["title"] = new_title
            report["target"] = {"name": target[2], "uuid": target[1], "library": target[0]} if target else None
            report["library"] = library or config.output.library
            report["node"] = transport.description
            flags = bool(report["flagged"])
            st["report"] = report
            run.save_report(report)
            if dry_run:
                st["outcome"] = "dry run"
                report.update(ok=True, dry_run=True, exit_code=int(ExitCode.COMPLETED_WITH_FLAGS if flags else 0))
                typer.echo(json.dumps(report, indent=2, ensure_ascii=False)) if as_json else _print_report(report)
                raise typer.Exit(report["exit_code"])
            if flags and not yes:
                st["outcome"] = "stopped: flagged lines (rerun with --yes to import the rest)"
                report.update(ok=False, error="Some lines are flagged. Fix them, or rerun with --yes to import the rest.", exit_code=int(ExitCode.PARSING_OR_VALIDATION))
                typer.echo(json.dumps(report, indent=2, ensure_ascii=False)) if as_json else _print_report(report)
                raise typer.Exit(int(ExitCode.PARSING_OR_VALIDATION))
            progress = runlog.wrap_progress((lambda m: console.print(f"[cyan]{m}[/cyan]")) if not as_json else (lambda m: None))
            if editing and in_place:
                applied = apply_import(
                    source, source_name, plan, target, config, transport,
                    replace=True, clear_generated=replace, close_app=not no_close, reopen=not no_reopen, progress=progress,
                )
            elif editing and as_copy:
                applied = publish_new(
                    source, source_name, new_title, plan, config, transport, progress=progress,
                    base=target, clear_generated=replace, add_to_playlists=not no_playlist,
                )
            elif editing:
                applied = publish_swap(source, source_name, plan, config, transport, target, clear_generated=replace, progress=progress)
            else:
                applied = publish_new(source, source_name, new_title, plan, config, transport, library=library, progress=progress)
            report.update(ok=True, applied=applied.model_dump(), exit_code=int(ExitCode.COMPLETED_WITH_FLAGS if flags else 0))
            st["outcome"], st["applied"] = "ok", applied.model_dump()
            report["run_id"] = run.id
            if as_json:
                typer.echo(json.dumps(report, indent=2, ensure_ascii=False))
            else:
                _print_report(report)
                what = "new presentation" if applied.mode == "new" else ("new copy" if as_copy else "presentation")
                console.print(f"[green]Imported {applied.slide_count} slides into {what} '{applied.presentation_name or applied.target_name}'[/green] (import {applied.import_id})")
                console.print(applied.verification)
                console.print(f"[dim]Log: {run.folder}[/dim]")
            raise typer.Exit(report["exit_code"])
        except typer.Exit:
            raise
        except Exception as exc:
            st["error"] = str(exc)
            if run:
                run.failed(exc)
            _fail(exc, as_json)
        finally:
            if run:
                run.finish(st["outcome"], st["report"], st["applied"], st["error"])
            if token is not None:
                runlog.deactivate(token)

    @app.command("undo")
    def undo_command(
        import_id: Annotated[str, typer.Argument(help="Import id from `versevision history`")],
        node: Annotated[Optional[str], typer.Option("--node")] = None,
        no_close: Annotated[bool, typer.Option("--no-close")] = False,
        output: Annotated[str, typer.Option("--output")] = "terminal",
    ) -> None:
        """Remove the slides a previous import added (and nothing else)."""
        as_json = output.casefold() == "json"
        from versevision import runlog
        run = runlog.RunLog.start("undo", source=f"undo of import {import_id}", mode="undo")
        token, st = runlog.activate(run), {"outcome": "failed", "error": None, "applied": None}
        try:
            config = _config_with_node(load_config(), node, None, None)
            result = undo_import(import_id, config, make_transport(config.propresenter, config.node), close_app=not no_close,
                                 progress=runlog.wrap_progress((lambda m: console.print(f"[cyan]{m}[/cyan]")) if not as_json else (lambda m: None)))
            st["outcome"], st["applied"] = "ok", result.model_dump()
            if as_json:
                typer.echo(json.dumps({"ok": True, **result.model_dump()}, indent=2))
            else:
                console.print(f"[green]Removed {result.removed_slides} slides from {result.target_name}[/green]")
        except Exception as exc:
            st["error"] = str(exc)
            run.failed(exc)
            _fail(exc, as_json)
        finally:
            run.finish(st["outcome"], None, st["applied"], st["error"])
            runlog.deactivate(token)

    @app.command("serve")
    def serve_command(
        host: Annotated[str, typer.Option("--host", help="Address to listen on; the default keeps it on this computer")] = "127.0.0.1",
        port: Annotated[int, typer.Option("--port")] = 8777,
        node: Annotated[Optional[str], typer.Option("--node")] = None,
        open_browser: Annotated[bool, typer.Option("--open/--no-open")] = False,
    ) -> None:
        """Start the local web page (drag and drop notes, review, import)."""
        from versevision.web.server import serve
        config = _config_with_node(load_config(), node, None, None)
        server, state = serve(config, host=host, port=port)
        link = f"http://{host if host not in {'0.0.0.0', '::'} else '127.0.0.1'}:{port}/" + (f"?token={state.token}" if state.token else "")
        console.print(f"[green]VerseVision is running[/green]  {link}")
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

    log_app = typer.Typer(help="Read the record of what VerseVision did (one folder per run).", no_args_is_help=False)
    app.add_typer(log_app, name="log")

    def _show_run(run_id: str | None, events: bool, problems: bool, path: bool) -> None:
        from versevision import runlog
        found = runlog.resolve(run_id or "latest")
        if not found:
            console.print("[red]No such run.[/red] Try: versevision log list")
            raise typer.Exit(1)
        folder = runlog.runs_dir() / found
        if path:
            typer.echo(str(folder))
            return
        if events or problems:
            for e in runlog.read_events(found):
                if problems and e.get("level") not in ("warn", "error"):
                    continue
                extra = {k: v for k, v in e.items() if k not in ("t", "elapsed", "step", "level")}
                typer.echo(f"[{e['elapsed']:>7}s] {e['level'].upper():<5} {e['step']:<16} {json.dumps(extra, ensure_ascii=False, default=str)[:300]}")
            return
        summary = folder / "summary.txt"
        typer.echo(summary.read_text(encoding="utf-8") if summary.exists() else "No summary yet (the run may still be going, or it was cut off). Showing the steps:\n" +
                   "\n".join(f"[{e['elapsed']}s] {e['step']} {e.get('message', '')}" for e in runlog.read_events(found)))

    @log_app.callback(invoke_without_command=True)
    def log_main(ctx: typer.Context) -> None:
        """With no subcommand, shows the latest run's summary."""
        if ctx.invoked_subcommand is None:
            _show_run(None, False, False, False)

    @log_app.command("show")
    def log_show(
        run_id: Annotated[Optional[str], typer.Argument(help="Run id (or the end of it); default: the latest")] = None,
        events: Annotated[bool, typer.Option("--events", help="Every logged step instead of the summary")] = False,
        problems: Annotated[bool, typer.Option("--problems", help="Only warnings and errors")] = False,
        path: Annotated[bool, typer.Option("--path", help="Print the folder path only")] = False,
    ) -> None:
        """Show one run: the readable summary, or --events / --problems."""
        _show_run(run_id, events, problems, path)

    @log_app.command("list")
    def log_list(count: Annotated[int, typer.Option("--count", "-n")] = 20, output: Annotated[str, typer.Option("--output")] = "terminal") -> None:
        """List recent runs, newest first."""
        from versevision import runlog
        rows = runlog.list_runs(count)
        if output.casefold() == "json":
            typer.echo(json.dumps(rows, indent=2))
            return
        table = Table("Run", "Result", "Action", "Document")
        for r in rows:
            table.add_row(r["id"], r["outcome"], r["mode"], r["source"][:50])
        console.print(table)
        console.print(f"[dim]{runlog.runs_dir()}[/dim]")

    autostart_app = typer.Typer(help="Start the web page automatically when you log in.")
    app.add_typer(autostart_app, name="autostart")

    @autostart_app.command("status")
    def autostart_status(output: Annotated[str, typer.Option("--output")] = "terminal") -> None:
        from versevision import autostart
        state = autostart.status()
        if output.casefold() == "json":
            typer.echo(state.model_dump_json(indent=2))
        else:
            console.print(("[green]On[/green]" if state.enabled else "Off") + f" ({state.platform})" + (f", http://{state.host}:{state.port}/" if state.enabled and state.port else ""))

    @autostart_app.command("enable")
    def autostart_enable(
        host: Annotated[str, typer.Option("--host")] = "127.0.0.1",
        port: Annotated[int, typer.Option("--port")] = 8777,
    ) -> None:
        """Turn on: the web page starts when you log in and restarts if it stops."""
        try:
            from versevision import autostart
            state = autostart.enable(host, port)
            console.print(f"[green]On[/green]: VerseVision will start when you log in at http://{host}:{port}/")
            if state.file:
                console.print(f"Service file: {state.file}")
        except Exception as exc:
            _fail(exc, False)

    @autostart_app.command("disable")
    def autostart_disable() -> None:
        """Turn off and remove the login item."""
        try:
            from versevision import autostart
            autostart.disable()
            console.print("Off: VerseVision will no longer start at login.")
        except Exception as exc:
            _fail(exc, False)

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
            from versevision.presentation.template import validate_template
            config = _config_with_node(load_config(), node, None, None)
            definition = config.presentation_template
            if not definition:
                raise ConfigurationError("no template is configured; run: versevision template setup")
            transport = make_transport(config.propresenter, config.node)
            lib, uuid_, name, path, _ = _template_target(transport, definition)
            import tempfile
            local = Path(tempfile.mkdtemp(prefix="versevision-")) / Path(path).name
            transport.fetch_file(path, local)
            candidate = validate_template(local, definition)
            payload = {"ok": True, "presentation": name, "verse_slide": candidate.slide_number, "has_point_template": bool(definition.point_cue_uuid)}
            typer.echo(json.dumps(payload, indent=2)) if as_json else console.print(f"[green]Templates are valid[/green] in {name} (verse slide {candidate.slide_number})")
        except Exception as exc:
            _fail(exc, as_json)

    @template_app.command("new-file")
    def template_new_file(
        source_file: Annotated[Optional[str], typer.Option("--source-file", help="A presentation FILE that still holds the template slides (a backup works)")] = None,
        source: Annotated[Optional[str], typer.Option("--source", help="Name of a presentation in ProPresenter that holds them")] = None,
        name: Annotated[str, typer.Option("--name")] = "VerseVision Template",
        library: Annotated[Optional[str], typer.Option("--library", help="Library for the template file (default: the output library)")] = None,
        node: Annotated[Optional[str], typer.Option("--node")] = None,
    ) -> None:
        """Move the template slides into their own presentation, so a sermon edit can never delete them."""
        try:
            from versevision import runlog
            config = _config_with_node(load_config(), node, None, None)
            definition = config.presentation_template
            if not definition:
                raise ConfigurationError("No template is configured; run: versevision template setup")
            transport = make_transport(config.propresenter, config.node)
            lib = library or config.output.library
            import tempfile
            work = Path(tempfile.mkdtemp(prefix="versevision-"))
            if source_file:
                src_path = source_file
            else:
                src_path = _target_from_api(transport, uuid_=None, name=source or definition.presentation_name, library=None)[3]
            local = work / "source.pro"
            transport.fetch_file(os.path.expandvars(src_path) if "$" in src_path else src_path, local)
            out = work / f"{name}.pro"
            ProtobufPresentationWriter().create_template_file(local, out, definition, name=name)
            folder = transport.library_folder(lib)
            if transport.find_presentation(name, lib):
                raise ConfigurationError(f"a presentation named {name!r} already exists in {lib}; nothing was changed")
            transport.create_file(out, f"{folder}/{name}.pro")
            console.print(f"[cyan]Created {name} in {lib}; waiting for ProPresenter to list it[/cyan]")
            listed = None
            for _ in range(20):
                try:
                    listed = next((i for i in list_presentations(transport) if i["name"] == name and i["library"].casefold() == lib.casefold()), None)
                except Exception:
                    listed = None
                if listed:
                    break
                time.sleep(1.5)
            if not listed:
                raise ConfigurationError("The file was created but ProPresenter has not listed it yet. Run this command again after it appears (it will keep the file).")
            updated = config.model_copy(update={"presentation_template": definition.model_copy(update={
                "presentation_uuid": listed["uuid"], "presentation_name": name, "library_name": lib})})
            save_config(updated, overwrite=True)
            console.print(f"[green]Done.[/green] The templates now live in {lib} / {name}. Settings updated.")
        except Exception as exc:
            _fail(exc, False)

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
            local = Path(tempfile.mkdtemp(prefix="versevision-")) / Path(path).name
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
