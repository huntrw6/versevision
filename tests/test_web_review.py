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
    from versevision.workflow.notes_workflow import apply_review_choices
    trimmed = type("P", (), {"slide_plan": apply_review_choices(job.plan, {warn["id"]})})
    assert len(trimmed.slide_plan.slides) == kept and len(job.plan.slide_plan.slides) == job.result["summary"]["slides"]


def test_the_page_can_choose_a_look_from_the_loaded_themes(state):
    from test_themes import THEMES
    st, node = state
    node.themes = THEMES
    rows = web.list_looks(st)
    assert {r["label"] for r in rows} == {"Scripture main screens · Slide 1", "Sermon Points · Theme Slide", "The biblke · Slide 1"}
    two_box = next(r for r in rows if r["label"].startswith("The biblke"))
    assert two_box["reference_box"] is False and two_box["key"] == "The biblke::1"       # its two boxes are stacked on one band
    job = wait(st, web.start_plan(st, "n.txt", NOTES.encode(), "NIV", None, verse_look=two_box["key"], point_look="Sermon Points::1"))
    assert job.result["looks"] == {"verse": "The biblke · Slide 1", "point": "Sermon Points · Theme Slide"}
    redo = wait(st, web.start_replan(st, job.id, {}))
    assert redo.result["looks"] == job.result["looks"]          # a recheck keeps the look


def test_picker_rows_carry_group_theme_and_slide_and_thumbnail_is_none_without_propresenter(state):
    from test_themes import THEMES
    st, node = state
    node.themes = THEMES
    row = next(r for r in web.list_looks(st) if r["theme"] == "The biblke")
    assert (row["group"], row["slide"], row["number"]) == ("", "Slide 1", 1)
    st.config.propresenter.port = 9                        # nothing listens there
    assert web.theme_thumbnail(st, "The biblke", 1) is None


def test_the_thumbnail_route_answers_over_http(state):
    import threading, urllib.error, urllib.request
    st, node = state
    st.config.propresenter.port = 9
    server, _ = web.serve(st.config, "127.0.0.1", 0)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        url = f"http://127.0.0.1:{server.server_address[1]}/api/themethumb?theme=Nope&slide=1"
        try:
            urllib.request.urlopen(url, timeout=5)
            raise AssertionError("expected 404")
        except urllib.error.HTTPError as exc:
            assert exc.code == 404                      # a clean 404, not a dropped connection
    finally:
        server.shutdown()


def test_playlists_list_their_presentations_under_their_section_headers(state):
    st, node = state
    lists = web.list_playlist_presentations(st.transport())
    assert [p["name"] for p in lists] == ["Sunday"]
    entry = lists[0]["entries"][0]
    assert entry["section"] == "Sermon" and entry["name"] == "Sample" and entry["library"] == "Verses"
    assert entry["uuid"] == "P1"


def test_the_icon_is_served_without_a_token_and_only_the_action_has_a_description():
    import threading, urllib.request
    server, _ = web.serve(None, "127.0.0.1", 0)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        base = f"http://127.0.0.1:{server.server_address[1]}"
        icon = urllib.request.urlopen(base + "/icon.svg", timeout=5)
        assert icon.headers["Content-Type"] == "image/svg+xml" and b"<svg" in icon.read()
        page = urllib.request.urlopen(base + "/", timeout=5).read().decode()
        assert 'rel="icon"' in page and page.count('class="help"') == 1 and 'id="modeHelp"' in page
    finally:
        server.shutdown()


def test_the_page_is_an_installable_app(tmp_path):
    import json, threading, urllib.request
    server, _ = web.serve(None, "127.0.0.1", 0)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        base = f"http://127.0.0.1:{server.server_address[1]}"
        get = lambda p: urllib.request.urlopen(base + p, timeout=5)
        manifest = json.loads(get("/manifest.webmanifest").read())
        assert manifest["display"] == "standalone" and manifest["start_url"] == "/" and manifest["name"] == "VerseVision"
        for icon in manifest["icons"]:                         # every icon the manifest names is served, with the right type
            response = get(icon["src"])
            assert response.headers["Content-Type"] == icon["type"] and len(response.read()) > 200
        assert {i["sizes"] for i in manifest["icons"]} >= {"192x192", "512x512"} and any(i["purpose"] == "maskable" for i in manifest["icons"])
        assert b"serviceWorker" not in get("/sw.js").read() and get("/sw.js").headers["Content-Type"] == "text/javascript"
        page = get("/").read().decode()
        assert 'rel="manifest"' in page and 'rel="apple-touch-icon"' in page and 'serviceWorker.register("/sw.js"' in page
    finally:
        server.shutdown()


def test_the_sidebar_uses_propresenters_icons_for_libraries_playlists_and_presentations():
    import threading, urllib.request
    server, _ = web.serve(None, "127.0.0.1", 0)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        base = f"http://127.0.0.1:{server.server_address[1]}"
        for name in ("ico-library", "ico-playlist", "ico-presentation"):
            body = urllib.request.urlopen(f"{base}/{name}.svg", timeout=5).read()
            assert body.startswith(b"<svg") and b"<rect" in body
        page = urllib.request.urlopen(base + "/", timeout=5).read().decode()
        assert "pi lib" in page and "pi pl" in page and "pi pres" in page
    finally:
        server.shutdown()



def test_each_title_in_the_main_panel_has_its_own_pixel_icon():
    import threading, urllib.request
    server, _ = web.serve(None, "127.0.0.1", 0)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        base = f"http://127.0.0.1:{server.server_address[1]}"
        for name in ("new", "replace", "append", "history"):
            body = urllib.request.urlopen(f"{base}/ico-{name}.svg", timeout=5).read()
            assert body.startswith(b"<svg") and b'viewBox="0 0 8 8"' in body
        page = urllib.request.urlopen(base + "/", timeout=5).read().decode()
        assert 'id="ttlIcon"' in page and "titleIcon(m)" in page and "titleIcon('history')" in page
    finally:
        server.shutdown()


def test_a_left_out_line_ticked_in_the_review_becomes_a_slide_when_importing(state):
    st, node = state
    job = wait(st, web.start_plan(st, "n.txt", NOTES.encode(), "NIV", None))
    title = next(d for d in job.result["decisions"] if d["kind"] == "left_out" and d["title"] == "Title")
    assert title["addable"] and title["add_text"] == "• Title" and title["slides"] == []
    before = job.result["summary"]["slides"]
    done = wait(st, web.start_apply(st, job.id, True, set(), {title["id"]}), limit=60)
    assert done.result["slide_count"] == before + 1
    from versevision import runlog
    assert any(r["stage"] == "review_included" and r["added"][0]["text"] == "• Title" for r in runlog.read_trace(job.run_id))
