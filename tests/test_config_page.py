"""The Config page: every customisation is listed with its limits, can be changed and saved, and applies to the next import."""
from __future__ import annotations

import json
import threading
import urllib.error
import urllib.request

import pytest

from versevision.config import AppConfig, load_config
from versevision.errors import ConfigurationError
from versevision.web import configpage
from versevision.web import server as web


@pytest.fixture(autouse=True)
def own_config(tmp_path, monkeypatch):
    monkeypatch.setenv("VERSEVISION_CONFIG", str(tmp_path / "config.toml"))
    return tmp_path / "config.toml"


def _fields():
    return {f["key"]: f for s in configpage.describe(AppConfig())["sections"] for f in s["fields"]}


def test_the_line_limits_are_on_the_page_with_their_help_and_bounds():
    fields = _fields()
    body, ref, points = fields["slides.max_body_lines"], fields["slides.max_reference_lines"], fields["points.max_lines_per_slide"]
    assert body["kind"] == "int" and body["value"] == 5 and body["min"] == 1 and "verse" in body["help"].lower()
    assert ref["value"] == 1 and points["value"] == 0 and points["min"] == 0 and "continues on the next slide" in points["help"]
    assert fields["notes.line_breaks"]["kind"] == "choice" and fields["notes.line_breaks"]["choices"] == ["slide", "paragraph"]
    assert fields["points.strip_trailing_period"]["kind"] == "bool" and fields["notes.extra_cue_words"]["kind"] == "list"
    assert fields["points.bullet_char"]["label"] == "Bullet mark"


def test_settings_that_are_not_for_this_page_are_left_off():
    keys = set(_fields())
    assert not keys & {"slides.max_characters", "points.theme", "verses.theme", "propresenter.host", "node.command"}
    assert {"slides.max_body_lines", "points.max_lines_per_slide", "points.fix_label_typos", "verses.quote_match", "bible.default_version"} <= keys


def test_changes_are_saved_and_the_next_load_sees_them(own_config):
    new = configpage.apply(AppConfig(), {"slides.max_body_lines": 4, "points.max_lines_per_slide": 2, "points.strip_trailing_period": False,
                                         "notes.extra_cue_words": ["pause", " sing "], "bible.default_version": "NKJV"})
    assert (new.slides.max_body_lines, new.points.max_lines_per_slide, new.points.strip_trailing_period) == (4, 2, False)
    saved = load_config()
    assert saved.slides.max_body_lines == 4 and saved.notes.extra_cue_words == ["pause", "sing"] and saved.bible.default_version == "NKJV"
    assert configpage.apply(saved, {"bible.default_version": ""}).bible.default_version is None


def test_a_bad_value_or_a_setting_not_on_the_page_is_refused_and_nothing_is_saved(own_config):
    with pytest.raises(ConfigurationError, match="slides.max_body_lines"):
        configpage.apply(AppConfig(), {"slides.max_body_lines": 99})
    with pytest.raises(ConfigurationError, match="cannot be changed here"):
        configpage.apply(AppConfig(), {"propresenter.host": "evil.example"})
    assert not own_config.exists()


def test_the_page_reads_and_saves_over_http_and_the_running_server_uses_the_new_limits(own_config):
    server, state = web.serve(AppConfig(), "127.0.0.1", 0)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        base = f"http://127.0.0.1:{server.server_address[1]}"
        got = json.load(urllib.request.urlopen(base + "/api/config", timeout=5))
        assert [s["id"] for s in got["sections"]][:2] == ["verse_slides", "points"]
        req = urllib.request.Request(base + "/api/config", method="POST", data=json.dumps({"values": {"points.max_lines_per_slide": 2}}).encode(),
                                     headers={"Content-Type": "application/json"})
        after = json.load(urllib.request.urlopen(req, timeout=5))
        assert {f["key"]: f for s in after["sections"] for f in s["fields"]}["points.max_lines_per_slide"]["value"] == 2
        assert state.config.points.max_lines_per_slide == 2 and load_config().points.max_lines_per_slide == 2
        bad = urllib.request.Request(base + "/api/config", method="POST", data=json.dumps({"values": {"slides.max_body_lines": 0}}).encode(),
                                     headers={"Content-Type": "application/json"})
        with pytest.raises(urllib.error.HTTPError) as err:
            urllib.request.urlopen(bad, timeout=5)
        assert err.value.code in (400, 422) and "slides.max_body_lines" in json.loads(err.value.read())["error"]
    finally:
        server.shutdown()


def test_the_toolbar_has_a_config_button_with_its_own_icon():
    server, _ = web.serve(None, "127.0.0.1", 0)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        base = f"http://127.0.0.1:{server.server_address[1]}"
        page = urllib.request.urlopen(base + "/", timeout=5).read().decode()
        assert 'id="tab-config"' in page and ">Config<" in page and 'id="viewConfig"' in page
        assert urllib.request.urlopen(base + "/ico-config.svg", timeout=5).read().startswith(b"<svg")
    finally:
        server.shutdown()


def test_every_option_has_a_one_line_label_and_help_so_rows_stay_the_same_height():
    for key, field in _fields().items():
        assert 0 < len(field["help"]) <= 70, (key, len(field["help"]))
        assert 0 < len(field["label"]) <= 52, (key, len(field["label"]))
        assert "\n" not in field["help"]
