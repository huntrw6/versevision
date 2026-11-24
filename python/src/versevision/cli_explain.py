"""`versevision explain` and `versevision replay`: read back how a document became slides."""
from __future__ import annotations

import json
from typing import Annotated, Optional

import typer

from versevision import explain as explain_mod


def explain_cmd(
    run_id: Annotated[Optional[str], typer.Argument(help="Run id (or the end of it); default: the latest run")] = None,
    line: Annotated[Optional[int], typer.Option("--line", "-l", help="Only the paragraph that holds this line of the document")] = None,
    tier: Annotated[Optional[str], typer.Option("--tier", help="Only error, warning or good paragraphs")] = None,
    output: Annotated[str, typer.Option("--output", help="terminal or json")] = "terminal",
) -> None:
    """Follow every line of the document to the slides it became: the rule that decided it, how versions were chosen, how verses were split."""
    try:
        if output.casefold() == "json":
            typer.echo(json.dumps(explain_mod.explain_data(run_id or "latest", line=line, tier=tier), indent=2, ensure_ascii=False, default=str))
        else:
            typer.echo(explain_mod.render_explain(run_id or "latest", line=line, tier=tier))
    except FileNotFoundError as exc:
        typer.echo(str(exc), err=True)
        raise typer.Exit(1)


def replay_cmd(
    run_id: Annotated[Optional[str], typer.Argument(help="Run id; default: the latest run")] = None,
    recent: Annotated[int, typer.Option("--recent", "-n", help="Replay this many of the latest runs instead")] = 0,
    todays_options: Annotated[bool, typer.Option("--todays-options", help="Use today's options, not the ones saved with the run")] = False,
) -> None:
    """Parse a past run's document again with today's rules and list every line whose decision changed (after a rules upgrade)."""
    from versevision import runlog
    ids = [r["id"] for r in runlog.list_runs(recent)] if recent else [run_id or "latest"]
    failed = False
    for rid in ids:
        try:
            typer.echo(explain_mod.render_replay(explain_mod.replay_data(rid, use_saved_options=not todays_options)))
        except FileNotFoundError as exc:
            typer.echo(f"{rid}: {exc}")
            failed = True
    if failed and not recent:
        raise typer.Exit(1)
