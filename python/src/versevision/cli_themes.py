"""`versevision themes`: the ProPresenter themes loaded on the Mac, and which ones give the verse and point slides their look."""
from __future__ import annotations

import json
from typing import Annotated, Optional

import typer
from rich.console import Console
from rich.table import Table

from versevision import themes as themes_mod
from versevision.cli_run import _config_with_node
from versevision.config import load_config, save_config, with_options
from versevision.errors import VerseVisionError
from versevision.node.transport import make_transport

console = Console()
themes_app = typer.Typer(help="Use the themes loaded in ProPresenter as the look of verse and point slides.", no_args_is_help=False)


def _slides(node: Optional[str]):
    config = _config_with_node(load_config(), node, None, None)
    return config, themes_mod.fetch_themes(make_transport(config.propresenter, config.node))


@themes_app.callback(invoke_without_command=True)
def list_themes(
    ctx: typer.Context,
    node: Annotated[Optional[str], typer.Option("--node", help="Remote computer running ProPresenter")] = None,
    output: Annotated[str, typer.Option("--output")] = "terminal",
) -> None:
    """List every theme slide loaded in ProPresenter and say which are in use."""
    if ctx.invoked_subcommand:
        return
    try:
        config, slides = _slides(node)
        mark = {"verse": config.verses.theme.strip().casefold(), "point": config.points.theme.strip().casefold()}
        if output.casefold() == "json":
            typer.echo(json.dumps([{"theme": s.theme_label, "slide": s.name, "number": s.number, "text_boxes": len(s.boxes), "other_objects": s.other_objects,
                                    "largest_box": [round(max(s.boxes, key=lambda b: b.area).w), round(max(s.boxes, key=lambda b: b.area).h)] if s.boxes else None}
                                   for s in slides], indent=2))
            return
        table = Table("Theme", "Slide", "Text boxes", "Main box", "Font", "In use for")
        for s in slides:
            main = max(s.boxes, key=lambda b: b.area) if s.boxes else None
            used = [k for k, v in mark.items() if v and v in (s.theme.casefold(), s.theme_label.casefold())]
            table.add_row(s.theme_label, f"{s.number}: {s.name or '(unnamed)'}", str(len(s.boxes)) if s.boxes else "[dim]none[/dim]",
                          f"{round(main.w)}x{round(main.h)}" if main else "", f"{main.font} {main.size:g}".strip() if main else "", ", ".join(used))
        console.print(table)
        console.print(f"Verses: {config.verses.theme or 'the VerseVision template'}   Points: {config.points.theme or 'the VerseVision template'}\n"
                      "Choose:  versevision themes use --verses \"The biblke\" --points \"Sermon Points\"     Go back:  versevision themes use --clear")
    except Exception as exc:
        console.print(f"[red]{exc}[/red]")
        raise typer.Exit(int(exc.exit_code) if isinstance(exc, VerseVisionError) else 1)


@themes_app.command("use")
def use(
    verses: Annotated[Optional[str], typer.Option("--verses", help="Theme for verse slides (Name or Group/Name)")] = None,
    verse_slide: Annotated[str, typer.Option("--verse-slide", help="Which slide of it: name or number (default: the first with a text box)")] = "",
    points: Annotated[Optional[str], typer.Option("--points", help="Theme for sermon-point slides")] = None,
    point_slide: Annotated[str, typer.Option("--point-slide")] = "",
    clear: Annotated[bool, typer.Option("--clear", help="Go back to the VerseVision template slides")] = False,
    node: Annotated[Optional[str], typer.Option("--node")] = None,
) -> None:
    """Save the theme choice. It is checked against the themes loaded on the Mac before it is saved."""
    try:
        config = load_config()
        if clear:
            sets = ["verses.theme=", "verses.theme_slide=", "points.theme=", "points.theme_slide="]
        else:
            sets = ([f"verses.theme={verses}", f"verses.theme_slide={verse_slide}"] if verses is not None else []) + \
                   ([f"points.theme={points}", f"points.theme_slide={point_slide}"] if points is not None else [])
        if not sets:
            raise VerseVisionError("Choose --verses and/or --points (or --clear)")
        updated = with_options(config, sets=sets)
        if not clear:
            _, slides = _slides(node)
            chosen = {}
            if verses:
                chosen["verses"] = themes_mod.find_slide(slides, verses, verse_slide)
            if points:
                chosen["points"] = themes_mod.find_slide(slides, points, point_slide)
            for what, s in chosen.items():
                console.print(f"{what}: {s.label}  ({len(s.boxes)} text box{'es' if len(s.boxes) != 1 else ''})")
        save_config(updated, overwrite=True)
        console.print("Saved." if not clear else "Back to the VerseVision template slides.")
    except Exception as exc:
        console.print(f"[red]{exc}[/red]")
        raise typer.Exit(int(exc.exit_code) if isinstance(exc, VerseVisionError) else 1)


def playlists_cmd(
    node: Annotated[Optional[str], typer.Option("--node", help="Remote computer running ProPresenter")] = None,
    output: Annotated[str, typer.Option("--output")] = "terminal",
) -> None:
    """List the ProPresenter playlists and the presentations in each (the same list the web page offers)."""
    from versevision.workflow.pipeline import list_playlist_presentations
    try:
        config = _config_with_node(load_config(), node, None, None)
        playlists = list_playlist_presentations(make_transport(config.propresenter, config.node))
        if output.casefold() == "json":
            typer.echo(json.dumps(playlists, indent=2))
            return
        for pl in playlists:
            console.print(f"[bold]{pl['name']}[/bold]  ({len(pl['entries'])})")
            section = None
            for entry in pl["entries"]:
                if entry["section"] != section:
                    section = entry["section"]
                    console.print(f"  [dim]{section or '-'}[/dim]")
                console.print(f"    {entry['name']}  [dim]{entry['library'] or 'not in a library'}[/dim]")
    except Exception as exc:
        console.print(f"[red]{exc}[/red]")
        raise typer.Exit(int(exc.exit_code) if isinstance(exc, VerseVisionError) else 1)

