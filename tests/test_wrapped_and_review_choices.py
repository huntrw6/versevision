"""PDF notes wrap long lines; the tail words must rejoin their sentence. Anything the importer leaves out can be added in the review."""
from __future__ import annotations

import asyncio
from pathlib import Path

import httpx
import pytest

from test_pipeline import env  # noqa: F401
from versevision import errors
from versevision.config import AppConfig, NotesConfig, PointsConfig
from versevision.node.transport import LocalTransport
from versevision.notes import classify_document
from versevision.workflow import pipeline
from versevision.workflow.notes_workflow import apply_review_choices, plan_notes

NOTES = (Path(__file__).parent / "fixtures" / "whitefield_notes.txt").read_text(encoding="utf-8")


def points(doc, **kwargs):
    return {it.line: it.text for it in doc.items if it.kind == "point"}


def test_the_tail_words_of_a_wrapped_sentence_rejoin_their_line():
    got = points(classify_document(NOTES, {}, NotesConfig(), PointsConfig()))
    # an all-capitals point whose last word (LIFE) wrapped onto the next line, in the notes and again in the recap
    assert got[39].endswith("IT MUST PRODUCE A CHANGED LIFE") and got[103].endswith("CHANGED LIFE")
    assert 40 not in got and 104 not in got
    assert got[60].endswith("committed to righteousness.")
    assert 61 not in got


def test_a_sentence_that_has_ended_never_swallows_the_next_line():
    got = points(classify_document(NOTES, {}, NotesConfig(), PointsConfig()))
    assert got[82].endswith("even a nation.") and "CONCLUSION" not in got[82]
    assert got[83] == "CONCLUSION: WHAT AMERICA AND THE WORLD NEEDS MOST IS ANOTHER SPIRITUAL AWAKENING"
    assert 84 not in got            # its last word AWAKENING was the wrapped tail, not a line of its own


def test_a_heading_a_quotation_and_numbered_keys_stay_apart():
    got = points(classify_document(NOTES, {}, NotesConfig(), PointsConfig()))
    assert got[24] == "PREACHING MUST CONFRONT SIN AND POINT PEOPLE TO THE SAVIOR"      # the quotation under it is a verse, not part of it ("KEY #2)" is its label)
    assert [got[n] for n in (100, 101, 102)] == ["SPEAK THE BORN AGAIN MESSAGE", "PREACHING MUST CONFRONT SIN AND POINT PEOPLE TO THE SAVIOR",
                                                 "DON’T BE ‘THE ALMOST CHRISTIAN’"]


def _page(tail: str) -> str:
    full = "Grace is the free and undeserved favor of God toward people who could never earn it by their own effort. "
    return "1) Opening point\n\n" + "\n\n".join([full.strip()] * 9 + [f"The shepherd leads us beside still waters and gives us rest in every season of life and\n{tail}"] + [full.strip()] * 3)


def test_the_wrap_rules_have_options():
    doc = _page("Every Single Day")
    joined = points(classify_document(doc, {}, NotesConfig(), PointsConfig()))
    assert any(t.endswith("season of life and Every Single Day") for t in joined.values())          # a 3-word tail after a line that has not ended
    tight = points(classify_document(doc, {}, NotesConfig(orphan_max_words=1), PointsConfig()))
    assert any(t.endswith("life and") for t in tight.values()) and any(t == "Every Single Day" for t in tight.values())
    off = points(classify_document(doc, {}, NotesConfig(wrapped_lines="separate"), PointsConfig()))
    assert any(t.endswith("life and") for t in off.values())
    # the same words after a line that HAS ended its sentence are a new line, whatever the setting
    ended = points(classify_document(_page("Every Single Day").replace("season of life and", "season of life."), {}, NotesConfig(), PointsConfig()))
    assert any(t == "Every Single Day" for t in ended.values())


def test_a_title_or_conclusion_that_was_left_out_can_be_added_in_the_review():
    plan = asyncio.run(plan_notes(NOTES, AppConfig(), default_version="NKJV", no_fetch=True))
    left = {d.line: d for d in plan.decisions if d.kind == "left_out"}
    title = left[1]
    assert title.title.startswith("JPL 11/22/26") and title.add_text.startswith("• JPL 11/22/26") and title.tier == "good"
    assert left[3].add_text == "• Show Photo of George Whitefield"      # a production cue is just as addable
    before = [s.body_text for s in plan.slide_plan.slides]
    chosen = apply_review_choices(plan, include={title.id, left[3].id}).slides
    texts = [s.body_text for s in chosen]
    assert len(texts) == len(before) + 2
    # each goes where it stood in the notes: before the first slide that follows it
    added = [t for t in texts if t.startswith("• JPL 11/22/26") or t == "• Show Photo of George Whitefield"]
    assert len(added) == 2 and texts.index(added[0]) == title.first_slide
    assert [t for t in texts if t not in added] == before      # nothing else moved or changed
    # switching a decision off and adding another work together
    first_point = next(d for d in plan.decisions if d.kind == "point")
    both = apply_review_choices(plan, skip={first_point.id}, include={title.id}).slides
    assert len(both) == len(before) - 1 + 1 and any(s.body_text.startswith("• JPL 11/22/26") for s in both)
    # a decision that already makes a slide, or a line that is not left out, cannot be "added"
    assert apply_review_choices(plan, include={first_point.id}).slides == plan.slide_plan.slides


def test_the_review_page_shows_the_choice_for_every_left_out_line():
    plan = asyncio.run(plan_notes(NOTES, AppConfig(), default_version="NKJV", no_fetch=True))
    from versevision.workflow.report import build_report
    rows = {d["line"]: d for d in build_report(plan, source_name="x")["decisions"] if d["kind"] == "left_out"}
    assert rows[1]["addable"] and rows[3]["addable"]        # the date/speaker line and a "Show photo" cue
    assert all(d["addable"] for d in rows.values() if d["line"] > 0)


# ---------------------------------------------------------------------------------------- ProPresenter said 404
def test_a_404_from_propresenter_is_not_reported_as_a_connection_problem(monkeypatch):
    def fake_get(url, timeout=None):
        return httpx.Response(404, request=httpx.Request("GET", url))
    monkeypatch.setattr(httpx, "get", fake_get)
    from versevision.config import NodeConfig, ProPresenterConfig
    t = LocalTransport(ProPresenterConfig(), NodeConfig())
    with pytest.raises(errors.ProPresenterNotFound) as info:
        t.api_get("/v1/presentation/ABC")
    assert "Could not reach" not in str(info.value) and "running and answered" in str(info.value) and "open it once" in str(info.value)

    def refused(url, timeout=None):
        raise httpx.ConnectError("refused")
    monkeypatch.setattr(httpx, "get", refused)
    with pytest.raises(errors.ProPresenterConnectionError) as info2:
        t.api_get("/version")
    assert "Could not reach" in str(info2.value) and not isinstance(info2.value, errors.ProPresenterNotFound)


def test_a_listed_presentation_that_the_api_will_not_return_is_read_from_its_file(env):
    tmp, pres, node, config, history = env
    real = node.api_get

    def api(route):
        if route.startswith("/v1/presentation/"):
            raise errors.ProPresenterNotFound("404")
        return real(route)

    node.api_get = api
    assert node.is_running()
    lib, uuid_, name, path, detail = pipeline._target_from_api(node, uuid_="P1", name=None, library=None)
    assert (uuid_, name, path) == ("P1", node.name, str(pres)) and detail == {"from_disk": True}


def test_a_swap_is_kept_when_propresenter_lists_the_file_but_answers_404_for_it(env, monkeypatch):
    """ProPresenter can list a presentation with an id and still return 404 for that id: the intact file on disk is the proof."""
    tmp, pres, node, config, history = env
    monkeypatch.setattr(pipeline, "LOAD_GRACE_SECONDS", 0.2)
    monkeypatch.setattr(pipeline, "STABLE_SECONDS", 0.2)
    real = node.api_get

    def api(route):
        if route.startswith("/v1/presentation/"):
            raise errors.ProPresenterNotFound("ProPresenter is running and answered, but has nothing at " + route + " (404)")
        return real(route)

    node.api_get = api
    plan, target = asyncio.run(pipeline.plan_for_transport(
        "1) Faith\nJohn 3:16 NIV\n", config, node, default_version="NIV", target_uuid=None, target_name=node.name, library=None))
    assert target[3] == str(pres)                          # found through the file, not the API
    result = pipeline.publish_swap("1) Faith\nJohn 3:16 NIV\n", "n.txt", plan, config, node, target, clear_generated=True, history=history)
    assert result.verified and result.slide_count == 2


def test_a_resaved_file_with_new_ids_and_names_still_counts_as_the_written_one(env, monkeypatch, tmp_path):
    """ProPresenter re-saves an open file: slide ids, names and order change, the words do not."""
    tmp, pres, node, config, history = env
    plan, target = asyncio.run(pipeline.plan_for_transport(
        "1) Faith\nJohn 3:16 NIV\n", config, node, default_version="NIV", target_uuid=None, target_name=node.name, library=None))
    from versevision.presentation.protobuf_writer import ProtobufPresentationWriter
    from versevision.presentation.generated import presentation_pb2
    out = tmp_path / "written.pro"
    template = pipeline._template_target(node, config.presentation_template)
    tpl = tmp_path / "t.pro"; node.fetch_file(template[3], tpl)
    ProtobufPresentationWriter().derive_presentation(pres, out, plan.slide_plan, config.presentation_template, name="Sample", clear_generated=True, template_path=tpl, keep_uuid=True)
    msg = presentation_pb2.Presentation(); msg.ParseFromString(out.read_bytes())
    for cue in msg.cues:                                   # what ProPresenter does on a re-save
        cue.name = ""
        cue.uuid.string = "AAAAAAAA-0000-0000-0000-" + cue.uuid.string[-12:]
    resaved = tmp_path / "resaved.pro"; resaved.write_bytes(msg.SerializeToString())
    node.fetch_file = lambda remote, local: (local.write_bytes(resaved.read_bytes()), "different-sha")[1]
    texts = [x.body_text for x in plan.slide_plan.slides]
    assert pipeline._verify_file_on_disk(node, "x", "written-sha", out, texts)[0] is True
    assert pipeline._verify_file_on_disk(node, "x", "written-sha", out, texts + ["A slide that is not in the file"])[0] is False
    assert pipeline._verify_file_on_disk(node, "x", "written-sha", out, None)[0] is False
