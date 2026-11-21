"""Every import leaves a trace: how each line of the document became (or did not become) a slide."""
from __future__ import annotations

import asyncio
import json

import pytest

from test_pipeline import _config, env  # noqa: F401
from versevision import explain, runlog
from versevision.bibles.models import BibleVerse, ResolvedPassage
from versevision.workflow.pipeline import plan_for_transport

LONG = "word " * 90          # a verse long enough to need cutting into several slides

DOC = "\n".join([
    "Sunday Sermon 11/8/26",                              # 1  title/date -> left out
    "1) Faith waits on the Lord and trusts Him completely every single day",     # 2  numbered point
    "Faith without doubt",                                 # 3  a short plain line: a bullet with a warning
    "Psalm 62:5 NASB1995",                                 # 4  explicit version
    "James 1:6",                                           # 5  inherits NASB1995
    "Show the slide of the church",                        # 6  stage cue
    "John 3:99 NIV",                                       # 7  not a real verse
    "Romans 8:1",                                          # 8  still inherits NASB1995 from line 4
    "2) Second point",                                     # 9  numbered point
    "Luke 2:1 NKJV",                                       # 10 long verse (fake provider returns long text for Luke)
])


@pytest.fixture
def run(tmp_path, monkeypatch):
    monkeypatch.setenv("VERSEVISION_LOGS", str(tmp_path / "logs"))
    r = runlog.RunLog.start("test", source="sermon.txt", mode="new")
    r.save_source(DOC, "sermon.txt")
    token = runlog.activate(r)
    yield r
    runlog.deactivate(token)


def make_plan(env, monkeypatch):
    tmp, pres, node, config, history = env

    async def fetch(self, reference):
        text = LONG if reference.book_id.lower().startswith("luk") or reference.book_display_name.startswith("Luke") else f"Text of verse {{v}}."
        verses = [BibleVerse(book_id=reference.book_id, chapter=s.start.chapter, verse=v, text=text.format(v=v) if "{v}" in text else text,
                             version=reference.version or "NIV")
                  for s in reference.spans for v in range(s.start.verse, (s.end or s.start).verse + 1)]
        return ResolvedPassage(reference=reference, resolved_version=reference.version or "NIV", verses=verses)
    monkeypatch.setattr("versevision.bibles.registry.ProviderRegistry.fetch", fetch)
    plan, _ = asyncio.run(plan_for_transport(DOC, config, node, default_version="NKJV", target_uuid=None, target_name=None, library=None))
    return plan


def test_the_trace_explains_each_line_and_every_source_line_is_covered(env, monkeypatch, run):
    make_plan(env, monkeypatch)
    data = explain.explain_data(run.id)
    by_start = {entry["line"]: entry for entry in data["lines"]}
    assert data["uncovered_source_lines"] == []
    assert by_start[2]["end_line"] == 2 and by_start[2]["paragraph"]["rule"].startswith("numbered/lettered marker")
    assert by_start[3]["item"]["outcome"][0]["role"] == "list_item" and by_start[3]["worst_tier"] == "warning"
    assert by_start[1]["item"]["outcome"][0]["kind"] == "skipped" and "date" in by_start[1]["paragraph"]["rule"]
    assert by_start[6]["paragraph"]["rule"].startswith("starts with a stage/cue word") and by_start[6]["item"]["settings"] == {"stage_cues": "skip"}
    assert by_start[7]["paragraph"]["label"] in ("badref", "other") and by_start[7]["worst_tier"] == "error"
    # versions: written, inherited from the nearest line above, never the default while an explicit one exists
    assert by_start[4]["versions"][0]["how"] == "written on the verse"
    assert "nearest version above (line 4)" in by_start[5]["versions"][0]["how"] and by_start[5]["versions"][0]["version"] == "NASB1995"
    assert "line 4" in by_start[8]["versions"][0]["how"]
    # fetch and split
    assert by_start[5]["fetches"][0]["verses"] == 1 and by_start[5]["splits"][0]["slides"] == 1
    long_split = by_start[10]["splits"][0]
    assert long_split["slides"] > 1 and long_split["verses"][0]["wrapped_lines"] > long_split["settings"]["max_body_lines"]
    # decisions link the line to the slides it produced
    assert by_start[10]["decisions"][0]["slides"][0] >= 1 and len(by_start[10]["decisions"][0]["slides"]) == long_split["slides"]
    plan = data["plan"][-1]
    assert plan["slide_count"] == len(plan["slides"]) and plan["slides"][0]["text"].startswith("1) Faith waits")
    assert (runlog.runs_dir() / run.id / "options.json").exists()


def test_the_readable_explanation_names_the_rules_and_the_final_slides(env, monkeypatch, run):
    make_plan(env, monkeypatch)
    text = explain.render_explain(run.id)
    for expected in ("HOW THE DOCUMENT BECAME SLIDES", "line 2", "rule: numbered/lettered marker", "version: NASB1995  - none written; used the nearest version above (line 4)",
                     "needed cutting", "FINAL SLIDES", "[ERROR]"):
        assert expected in text, expected
    assert "Source lines the parser never looked at" not in text
    only = explain.render_explain(run.id, line=7)
    assert "John 3:99" in only and "Faith waits" not in only
    assert "ERROR" in explain.render_explain(run.id, tier="error") and "[ok   ]" not in explain.render_explain(run.id, tier="error")


def test_the_trace_never_contains_secrets(env, monkeypatch, run, tmp_path):
    monkeypatch.setenv("API_BIBLE_API_KEY", "SECRET-KEY-VALUE-123")
    make_plan(env, monkeypatch)
    blob = "".join(p.read_text() for p in (runlog.runs_dir() / run.id).iterdir() if p.suffix in {".jsonl", ".json", ".txt"})
    assert "SECRET-KEY-VALUE-123" not in blob


def test_replay_reports_nothing_changed_with_the_same_rules_and_names_a_changed_line_when_rules_differ(env, monkeypatch, run):
    make_plan(env, monkeypatch)
    same = explain.replay_data(run.id)
    assert same["differences"] == [] and same["decisions_now"] == same["decisions_then"]
    # an older run whose log says line 6 (a stage cue) was once treated differently: replay must name that line
    trace = runlog.runs_dir() / run.id / "trace.jsonl"
    rows = [json.loads(x) for x in trace.read_text().splitlines()]
    for row in rows:
        if row["stage"] == "decision" and row["line"] == 6:
            row["tier"], row["kind"] = "warning", "left_out"
    trace.write_text("\n".join(json.dumps(r) for r in rows) + "\n")
    changed = explain.replay_data(run.id)["differences"]
    assert [d["line"] for d in changed] == [6] and "tier warning -> good" in changed[0]["change"]
    assert "line    6" in explain.render_replay({"run": run.id, "options": "saved with the run", "decisions_then": 1, "decisions_now": 1, "differences": changed})


def test_review_edits_and_exclusions_are_logged(run):
    run.trace("review_edit", parent_run="abc", changes=[{"line": 7, "before": "John 3:99 NIV", "after": "John 3:16 NIV"}])
    run.finish("ok", None, {"presentation_name": "X", "slide_count": 3, "cue_count_after": 9, "verified": True})
    rows = runlog.read_trace(run.id)
    assert [r["stage"] for r in rows][-2:] == ["review_edit", "output"] and rows[-1]["slide_count"] == 3


WRAPPED = "\n".join([
    "The Lord is my shepherd, I shall not want. He makes me lie down in green pastures; He leads me beside",     # full-width lines, as a PDF breaks them
    "quiet waters and restores my soul. He guides me in the paths of righteousness for His name's sake and I",
    "will fear no evil, for You are with me and Your rod and Your staff, they comfort me all of my days and",
    "I will dwell in the house of the Lord forever and ever amen, because goodness and mercy shall follow me",
    "",
    "1) The shepherd leads us beside still waters and gives rest to everyone who follows Him in faith every day",
    "and trusts the way He leads even through the valley of the shadow of death without any fear at all today",
    "2) Second point",
    "Psalm 23:1 NKJV",
    "Psalm 23:4",
    "Psalm 23:6",
    "3) Third point",
])


def test_pdf_style_wrapped_lines_are_joined_and_the_log_says_why():
    from versevision.notes import classify_document
    from versevision.config import NotesConfig, PointsConfig
    token = None
    import tempfile, os
    with tempfile.TemporaryDirectory() as tmp:
        os.environ["VERSEVISION_LOGS"] = tmp
        r = runlog.RunLog.start("test", source="x")
        r.save_source(WRAPPED, "x")
        token = runlog.activate(r)
        try:
            classify_document(WRAPPED, {}, NotesConfig(), PointsConfig())
            data = explain.explain_data(r.id)
        finally:
            runlog.deactivate(token)
            os.environ.pop("VERSEVISION_LOGS")
    assert data["parse_start"][0]["hard_wrapped"] is True
    wrapped_point = next(e for e in data["lines"] if e["line"] == 6)
    assert wrapped_point["end_line"] == 7 and wrapped_point["joins"][0]["line"] == 7
    assert "does not start a new item" in wrapped_point["joins"][0]["why"] and wrapped_point["paragraph"]["raw_lines"][1].startswith("and trusts")
    assert data["uncovered_source_lines"] == []
