import json

from versevision import runlog


def test_a_run_leaves_a_readable_record_and_never_logs_secrets(tmp_path, monkeypatch):
    monkeypatch.setenv("VERSEVISION_LOGS", str(tmp_path))
    run = runlog.RunLog.start("run", source="notes.pdf", mode="new", api_key="SECRET-KEY-VALUE")
    token = runlog.activate(run)
    run.save_source("John 3:16 NIV\n", "notes.pdf")
    runlog.event("bible_fetch", reference="John 3:16", version="NIV", provider="youversion", source="live", verses=1, ms=120)
    runlog.event("bible_fetch", level="warn", reference="Jude 1:99", version="NIV", error="verse not found", ms=80)
    runlog.event("node_action", action="curl http://x", token="TOKEN-VALUE", exit=0, ms=5)
    report = {"summary": {"slides": 1, "verse_slides": 1, "point_slides": 0, "flagged": 1, "not_added": 1, "needs_review": 0, "adjustments": 0, "left_out": 0},
              "slides": [{"n": 1, "kind": "verse", "title": "John 3:16 (NIV)", "text": "For God so loved"}],
              "flagged": [{"line": 4, "text": "Jude 1:99", "detail": "not added: verse not found", "status": "not_added"}],
              "adjustments": [], "left_out": []}
    run.save_report(report)
    run.finish("ok", report, {"presentation_name": "Notes", "slide_count": 1, "cue_count_after": 1, "verified": True, "verification": "ok", "steps": ["placed x"]})
    runlog.deactivate(token)

    folder = tmp_path / "runs" / run.id
    assert {p.name for p in folder.iterdir()} == {"events.jsonl", "source.txt", "report.json", "summary.txt"}
    everything = "".join(p.read_text() for p in folder.iterdir())
    assert "SECRET-KEY-VALUE" not in everything and "TOKEN-VALUE" not in everything
    summary = (folder / "summary.txt").read_text()
    assert "SUCCESS" in summary and "John 3:16" in summary and "Jude 1:99" in summary and "verse not found" in summary
    assert "youversion" in summary and "FLAGGED" in summary
    assert (folder / "source.txt").read_text() == "John 3:16 NIV\n"
    steps = [json.loads(line)["step"] for line in (folder / "events.jsonl").read_text().splitlines()]
    assert steps[0] == "run_start" and steps[-1] == "run_end"


def test_a_failed_run_says_why(tmp_path, monkeypatch):
    monkeypatch.setenv("VERSEVISION_LOGS", str(tmp_path))
    run = runlog.RunLog.start("run", source="x.pdf", mode="replace")
    try:
        raise ValueError("ProPresenter did not show the new file")
    except ValueError as exc:
        run.failed(exc)
        run.finish("failed", None, None, str(exc))
    text = (run.folder / "summary.txt").read_text()
    assert "FAILED" in text and "did not show the new file" in text
    assert runlog.list_runs()[0]["outcome"] == "failed"


def test_logging_never_raises_without_a_run_or_folder(tmp_path, monkeypatch):
    runlog.event("anything", level="info", x=1)             # no active run: silently nothing
    monkeypatch.setenv("VERSEVISION_LOGS", str(tmp_path))
    run = runlog.RunLog.start("run", source="a", mode="new")
    run.folder.rmdir() if not any(run.folder.iterdir()) else None
    run.event("after_folder_gone")                          # must not raise


def test_old_runs_are_pruned(tmp_path, monkeypatch):
    monkeypatch.setenv("VERSEVISION_LOGS", str(tmp_path))
    monkeypatch.setattr(runlog, "KEEP_RUNS", 3)
    for i in range(6):
        (tmp_path / "runs" / f"2026010{i}-000000-aaaaaa").mkdir(parents=True)
    runlog.RunLog.start("run", source="a", mode="new")
    assert len(list((tmp_path / "runs").iterdir())) == 3
