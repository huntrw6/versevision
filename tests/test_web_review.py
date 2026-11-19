"""The web review: plan -> grouped decisions -> fix a line and recheck -> switch a slide off -> import."""
from __future__ import annotations

import asyncio
import time
from pathlib import Path

import pytest

from test_pipeline import _config, env  # noqa: F401  (the same offline fixtures the pipeline tests use)
from versevision.web import server as web

NOTES = ("Title\n1) Faith\nJohn 3:16 NIV\nRomans 8:28\nOpen with prayer\nJohn 3:99 NIV\n")


@pytest.fixture
def state(env):
    tmp, pres, node, config, history = env
    st = web.AppState(config, None, history)
    st.transport = lambda: node        # the fake Mac
    return st, node


def wait(st, job, limit=15):
    end = time.time() + limit
    while job.state == "running" and time.time() < end:
        time.sleep(0.02)
    assert job.state == "done", job.error
    return job


def test_the_plan_is_graded_and_every_line_can_be_edited_and_rechecked(state):
    st, node = state
    job = wait(st, web.start_plan(st, "n.txt", NOTES.encode(), "NIV", None))
    r = job.result
    assert r["summary"]["errors"] == 1 and r["summary"]["warnings"] >= 1 and r["summary"]["good"] >= 2
    error = r["review"]["error"][0]
    assert "John 3:99" in error["title"] and r["lines"][str(error["line"])].startswith("John 3:99")
    # fix the bad verse; the whole document is checked again
    redo = wait(st, web.start_replan(st, job.id, {str(error["line"]): "John 3:17 NIV"}))
    assert redo.result["summary"]["errors"] == 0
    assert any("John 3:17" in d["title"] for d in redo.result["review"]["good"])


def test_a_decision_can_be_switched_off_before_importing(state):
    st, node = state
    job = wait(st, web.start_plan(st, "n.txt", NOTES.encode(), "NIV", None))
    warn = next(d for d in job.result["review"]["warning"] if d["slides"])
    kept = job.result["summary"]["slides"] - len(warn["slides"])
    trimmed = web._without_skipped(job.plan, job.result, {warn["id"]})
    assert len(trimmed.slide_plan.slides) == kept and len(job.plan.slide_plan.slides) == job.result["summary"]["slides"]
