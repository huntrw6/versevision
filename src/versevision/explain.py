"""Read a run's trace back: how did the document become these slides? And re-run an old document through today's rules.

`explain_run` follows every source line to the slides it produced, naming the rule that decided it. `replay_run` parses the saved
source again with the options saved beside it (or today's), without fetching Scripture, and lists every line whose decision changed.
"""
from __future__ import annotations

import asyncio
import json
from collections import defaultdict
from pathlib import Path
from typing import Any

from versevision import runlog

_TIER_MARK = {"good": "ok", "warning": "CHECK", "error": "ERROR"}


def _clip(text: Any, width: int = 150) -> str:
    text = " ".join(str(text).split())
    return text if len(text) <= width else text[: width - 1] + "…"


def load(run_id: str) -> tuple[str, Path, list[dict]]:
    found = runlog.resolve(run_id or "latest")
    if not found:
        raise FileNotFoundError("No such run. Try: versevision log list")
    folder = runlog.runs_dir() / found
    return found, folder, runlog.read_trace(found)


def _non_default_options(folder: Path) -> list[str]:
    """Options that differ from the defaults: those are the ones that explain surprising behaviour."""
    path = folder / "options.json"
    if not path.exists():
        return []
    from versevision.config import NotesConfig, PointsConfig, SlideConfig, VersesConfig
    saved = json.loads(path.read_text(encoding="utf-8"))
    out = []
    for section, model in (("notes", NotesConfig), ("points", PointsConfig), ("slides", SlideConfig), ("verses", VersesConfig)):
        defaults = model().model_dump(mode="json")
        for key, value in (saved.get(section) or {}).items():
            if defaults.get(key) != value:
                out.append(f"{section}.{key} = {value!r}  (default {defaults.get(key)!r})")
    return out


def explain_data(run_id: str, *, line: int | None = None, tier: str | None = None) -> dict:
    """Everything the trace knows, organised by source line (the structure `--json` prints and the text view is drawn from)."""
    found, folder, rows = load(run_id)
    by_line: dict[int, dict] = defaultdict(lambda: {"joins": [], "versions": [], "fetches": [], "splits": [], "points": [], "decisions": []})
    paragraphs: list[dict] = []
    singles: dict[str, list[dict]] = defaultdict(list)
    for row in rows:
        stage = row["stage"]
        if stage == "paragraph":
            paragraphs.append(row)
            by_line[row["line"]]["paragraph"] = row
        elif stage == "line_join":
            by_line[row["into_paragraph"]]["joins"].append(row)
        elif stage == "item":
            by_line[row["line"]]["item"] = row
        elif stage == "version":
            by_line[row["line"]]["versions"].append(row)
        elif stage == "fetch_result":
            by_line[row["line"]]["fetches"].append(row)
        elif stage == "split":
            by_line[row["line"]]["splits"].append(row)
        elif stage == "point_format":
            by_line[row["line"]]["points"].append(row)
        elif stage == "decision":
            by_line[row["line"]]["decisions"].append(row)
        else:
            singles[stage].append(row)
    providers = {(r["reference"], r.get("version")): r for r in singles.get("provider", [])}
    for entry in by_line.values():
        for f in entry["fetches"]:
            f["provider"] = providers.get((f["reference"], f["version_asked"]))
    source_path = folder / "source.txt"
    source = source_path.read_text(encoding="utf-8").splitlines() if source_path.exists() else []
    covered: set[int] = set()
    for p in paragraphs:
        covered.update(range(p["line"], p["end_line"] + 1))
    uncovered = [n for n, text in enumerate(source, 1) if text.strip() and n not in covered]
    lines_out = []
    for p in paragraphs:
        entry = by_line[p["line"]]
        tiers = {d["tier"] for d in entry["decisions"]} or {"good"}
        worst = "error" if "error" in tiers else "warning" if "warning" in tiers else "good"
        if line is not None and not (p["line"] <= line <= p["end_line"]):
            continue
        if tier and worst != tier:
            continue
        lines_out.append({"line": p["line"], "end_line": p["end_line"], "worst_tier": worst, **entry})
    events = runlog.read_events(found)
    start = next((e for e in events if e["step"] == "run_start"), {})
    return {"run": found, "start": start, "folder": str(folder), "options_changed": _non_default_options(folder), "lines": lines_out,
            "uncovered_source_lines": uncovered, **{k: v for k, v in singles.items()}}


def render_explain(run_id: str, *, line: int | None = None, tier: str | None = None, width: int = 150) -> str:
    d = explain_data(run_id, line=line, tier=tier)
    out: list[str] = []
    add = out.append
    start = d["start"]
    add(f"HOW THE DOCUMENT BECAME SLIDES  -  run {d['run']}")
    add("=" * 78)
    add(f"Document: {start.get('source', '')}    Action: {start.get('mode', '')}    VerseVision {start.get('application_version', '')}")
    for row in d.get("extract", []):
        add(f"Read as:  {row['method']}  ->  {row['characters']} characters, {row['lines']} lines ({row['blank_lines']} blank)")
    for row in d.get("parse_start", []):
        add(f"Layout:   {row['non_blank']} text lines; " + (f"HARD-WRAPPED text (PDF/email style): a line {row['full_line_width']}+ characters long is joined to the next one"
                                                              if row["hard_wrapped"] else "lines are treated as written (no hard-wrap detected)"))
    for row in d.get("look", []):
        add(f"Look:     verses = {row['verse'] or 'VerseVision template'};  points = {row['point'] or 'VerseVision template'}")
    for row in d.get("review_edit", []):
        add(f"Review:   edited after run {row['parent_run']}: " + "; ".join(f"line {c['line']}: {_clip(c['before'], 40)!r} -> {_clip(c['after'], 40)!r}" for c in row["changes"]))
    changed = d["options_changed"]
    add("Options that are not the defaults: " + ("none (house style)" if not changed else ""))
    for c in changed:
        add(f"   {c}")
    add("")
    for entry in d["lines"]:
        a, b = entry["line"], entry["end_line"]
        p = entry["paragraph"]
        where = f"line {a}" if a == b else f"lines {a}-{b}"
        add(f"[{_TIER_MARK[entry['worst_tier']]:<5}] {where}")
        for raw in p["raw_lines"]:
            add(f"        | {_clip(raw, width)}")
        if entry["joins"]:
            add("        joined: " + _clip("; ".join(f"line {j['line']} ({j['why']})" for j in entry["joins"]), width))
        add(f"        rule: {p['rule'] or p['label']}  ->  read as '{p['label']}'" + (f"  [{p['why']}]" if p.get("why") else ""))
        for ref in p.get("references", []):
            add(f"        reference: {ref['as_written']!r} -> {ref['normalized']}" + (f" (version written: {ref['version_written']})" if ref["version_written"] else "")
                + (f"  warnings: {'; '.join(ref['warnings'])}" if ref["warnings"] else ""))
        item = entry.get("item")
        if item:
            if item["settings"]:
                add("        your options that applied: " + ", ".join(f"{k}={v}" for k, v in item["settings"].items()))
            for o in item["outcome"]:
                detail = {"point": lambda o: f"POINT ({o.get('role')}) label {o.get('label')!r}", "verse": lambda o: f"VERSE {o.get('reference')}",
                          "skipped": lambda o: f"LEFT OUT ({o.get('category')}): {o.get('reason')}", "review": lambda o: f"NEEDS A DECISION: {o.get('reason')}",
                          "joined": lambda o: f"JOINED into the item at line {o.get('into_line')}: {o.get('note')}"}.get(o["kind"], lambda o: o["kind"])(o)
                add(f"        became: {detail}")
                for w in o.get("warnings", []) or []:
                    add(f"                note: {_clip(w, width)}")
        for v in entry["versions"]:
            add(f"        version: {v.get('version')}  - {v['how']}" + (f"  ({v['note']})" if v.get("note") else ""))
        for f in entry["fetches"]:
            prov = f.get("provider") or {}
            via = (f" via {prov.get('provider')} ({prov.get('source')}, {prov.get('ms')} ms)" if prov.get("provider") else f" FAILED: {prov['error']}" if prov.get("error") else "")
            add(f"        Scripture: asked {f['version_asked']}, got {f['version_returned']}{via}: {f['verses']} verse(s), {f['characters']} characters"
                + (f"; first: {_clip(f['first_verse'], 60)!r}" if f.get("first_verse") else ""))
        for s in entry["splits"]:
            multi = [v for v in s["verses"] if v["slides"] > 1]
            add(f"        slides: {s['slides']} for {len(s['verses'])} verse(s)" + (f"; {len(multi)} verse(s) needed cutting (max {s['settings']['max_body_lines']} lines per slide)" if multi else "; each fits on one slide"))
            for v in multi:
                add(f"                verse {v['verse']}: {v['wrapped_lines']} wrapped lines -> {v['slides']} slides: " + " | ".join(_clip(x, 50) for x in v["pieces"]))
        for pf in entry["points"]:
            add(f"        slide text: {_clip(pf['slide_text'], width)!r}   (numbering={pf['settings']['numbering']}, case={pf['settings']['case']})")
        for dec in entry["decisions"]:
            n = dec["slides"]
            add(f"        => {dec['kind']} [{dec['tier']}]" + (f" slides {n[0]}-{n[-1]}" if len(n) > 1 else f" slide {n[0]}" if n else " (no slide)")
                + (f"  because: {_clip('; '.join(dec['reasons']), width)}" if dec["reasons"] else ""))
        add("")
    if d["uncovered_source_lines"]:
        add(f"!! Source lines the parser never looked at: {d['uncovered_source_lines'][:30]}  (this should be empty; tell me)")
    for row in d.get("review_excluded", []):
        add("You switched these off in the review: " + "; ".join(f"line {x['line']} {_clip(x['title'], 40)!r}" for x in row["decisions"]))
    for row in d.get("output", []):
        add(f"Written to ProPresenter: {row.get('presentation_name') or row.get('target_name')}  ({row.get('slide_count')} slides added; the file has {row.get('cue_count_after')};"
            f" verified: {row.get('verified')})")
    plan = (d.get("plan") or [None])[-1]
    if plan and line is None and tier is None:
        add("")
        add(f"FINAL SLIDES ({plan['slide_count']}: {plan['verse_slides']} verse, {plan['point_slides']} point)")
        add("-" * 78)
        for s in plan["slides"]:
            who = s["reference"] or "point"
            add(f"{s['n']:>4}. {s['kind']:<5} {_clip(who, 28):<28} {_clip(s['text'], 90)!r}")
    return "\n".join(out) + "\n"


# ---------------------------------------------------------------------------------------------- replay
def _decisions_from_trace(rows: list[dict]) -> list[dict]:
    return [r for r in rows if r["stage"] == "decision"]


def replay_data(run_id: str, *, use_saved_options: bool = True) -> dict:
    """Parse the saved source again (no Scripture fetched) and compare every decision with what that run decided."""
    from versevision.config import NotesConfig, PointsConfig, SlideConfig, VersesConfig, load_config
    from versevision.workflow.notes_workflow import plan_notes

    found, folder, rows = load(run_id)
    source_file = folder / "source.txt"
    if not source_file.exists():
        raise FileNotFoundError(f"run {found} has no saved source.txt")
    config = load_config()
    saved = json.loads((folder / "options.json").read_text(encoding="utf-8")) if (folder / "options.json").exists() else None
    if saved and use_saved_options:
        config = config.model_copy(update={
            "notes": NotesConfig(**saved["notes"]), "points": PointsConfig(**saved["points"]), "slides": SlideConfig(**saved["slides"]),
            "verses": VersesConfig(**saved["verses"]),
        })
    default_version = (saved or {}).get("default_version_used")
    result = asyncio.run(plan_notes(source_file.read_text(encoding="utf-8"), config, default_version=default_version, no_fetch=True))
    old = _decisions_from_trace(rows)
    if not old:     # a run from before traces existed: use its report
        report = folder / "report.json"
        if report.exists():
            old = [{"line": x["line"], "kind": x["kind"], "tier": x.get("tier", "good"), "title": x["title"], "reasons": x.get("reasons", [])}
                   for x in json.loads(report.read_text(encoding="utf-8")).get("decisions", [])]

    def key(x) -> tuple:
        return (x["line"], x["kind"], x["title"])

    def network_only(x) -> bool:      # a failed Scripture fetch cannot repeat offline
        return x["tier"] == "error" and any(r.startswith("not added:") for r in x["reasons"])

    new = [{"line": x.line, "kind": x.kind, "tier": x.tier, "title": x.title, "reasons": x.reasons} for x in result.decisions]
    old_map, new_map = {key(x): x for x in old}, {key(x): x for x in new}
    diffs = []
    for k in sorted(set(old_map) | set(new_map), key=lambda t: (t[0], t[1])):
        o, n = old_map.get(k), new_map.get(k)
        if o and n:
            if o["tier"] != n["tier"] and not network_only(o):
                diffs.append({"line": k[0], "title": k[2], "change": f"tier {o['tier']} -> {n['tier']}", "before": o["reasons"], "after": n["reasons"]})
        elif o and not network_only(o):
            diffs.append({"line": k[0], "title": k[2], "change": f"was {o['kind']} [{o['tier']}]; no longer produced", "before": o["reasons"], "after": []})
        elif n:
            diffs.append({"line": k[0], "title": k[2], "change": f"now {n['kind']} [{n['tier']}]; was not there", "before": [], "after": n["reasons"]})
    return {"run": found, "options": "saved with the run" if (saved and use_saved_options) else "today's", "decisions_then": len(old), "decisions_now": len(new), "differences": diffs}


def render_replay(data: dict) -> str:
    out = [f"REPLAY of run {data['run']} with {data['options']} options (Scripture is not fetched, so only the parsing is compared)",
           f"decisions then: {data['decisions_then']}   now: {data['decisions_now']}   changed: {len(data['differences'])}"]
    for d in data["differences"]:
        out.append(f"  line {d['line']:>4}  {_clip(d['title'], 50)!r}: {d['change']}")
        for r in d["after"][:3]:
            out.append(f"            now: {_clip(r, 120)}")
    if not data["differences"]:
        out.append("  Nothing changed: today's rules decide every line the same way.")
    return "\n".join(out) + "\n"
