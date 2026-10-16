from __future__ import annotations

import asyncio
import shutil
from pathlib import Path

import pytest

from fakes import FakeNode
from versebridge.config import AppConfig, PresentationTemplateConfig
from versebridge.errors import VerificationRollbackError
from versebridge.history import HistoryStore
from versebridge.node.transport import sha256_file
from versebridge.presentation.generated import presentation_pb2
from versebridge.presentation.template import inspect_point_candidates, inspect_template_candidates
from versebridge.workflow import pipeline
from versebridge.workflow.pipeline import apply_import, plan_for_transport, undo_import

FIXTURE = Path(__file__).parent / "fixtures" / "sample" / "Sample.pro"
NOTES = "Title\n“For God so loved the world.”\nJohn 3:16 NIV\n1) First point\nPsalm 23:1 NIV\n2) Second point\n"


def _config(path: Path) -> AppConfig:
    verse = inspect_template_candidates(path)[0]
    point = inspect_point_candidates(path)[0]
    template = PresentationTemplateConfig(
        presentation_uuid="P1", presentation_name="Sample", cue_uuid=verse.cue_uuid,
        style_fingerprint=verse.style_fingerprint, point_cue_uuid=point[1], point_style_fingerprint=point[2],
    )
    return AppConfig(presentation_template=template)


@pytest.fixture
def env(tmp_path, monkeypatch):
    pres = tmp_path / "Sample.pro"
    shutil.copy(FIXTURE, pres)
    node = FakeNode(tmp_path / "bk", pres)
    # offline: stand in for the Bible providers with fixed text
    from versebridge.bibles.models import BibleVerse, ResolvedPassage

    async def fake_fetch(self, reference):
        verses = [BibleVerse(book_id=reference.book_id, chapter=s.start.chapter, verse=v, text=f"Text of verse {v}.", version=reference.version or "NIV")
                  for s in reference.spans for v in range(s.start.verse, (s.end or s.start).verse + 1)]
        return ResolvedPassage(reference=reference, resolved_version=reference.version or "NIV", verses=verses)

    monkeypatch.setattr("versebridge.bibles.registry.ProviderRegistry.fetch", fake_fetch)
    return tmp_path, pres, node, _config(pres), HistoryStore(tmp_path / "imports.json")


def _plan(node, config):
    return asyncio.run(plan_for_transport(NOTES, config, node, default_version="NIV", target_uuid=None, target_name="Sample", library=None))


def _fake_target(node):
    return ("Verses", "P1", "Sample", str(node.file), {})


def test_import_then_reimport_replaces_then_undo_restores_original(env):
    tmp, pres, node, config, history = env
    original = presentation_pb2.Presentation()
    original.ParseFromString(pres.read_bytes())
    plan, target = _plan(node, config)
    assert len(plan.slide_plan.slides) == 4 and not plan.failed
    first = apply_import(NOTES, "n.txt", plan, target, config, node, history=history)
    assert first.cue_count_after == first.cue_count_before + 4 and node.log == ["quit", "install", "launch"]
    second = apply_import(NOTES, "n.txt", plan, target, config, node, history=history)
    assert second.removed_slides == 4 and second.cue_count_after == first.cue_count_after
    assert [r.status for r in history.load()] == ["replaced", "active"]
    undo_import(second.import_id, config, node, history=history)
    restored = presentation_pb2.Presentation()
    restored.ParseFromString(pres.read_bytes())
    assert restored.SerializeToString(deterministic=True) == original.SerializeToString(deterministic=True)


def test_failed_verification_rolls_back_the_file(env, monkeypatch):
    tmp, pres, node, config, history = env
    node.show_wrong_text = True
    before = sha256_file(pres)
    real = pipeline.verify_via_api
    monkeypatch.setattr(pipeline, "verify_via_api", lambda *a, **k: real(*a, timeout=1, **k))
    plan, target = _plan(node, config)
    with pytest.raises(VerificationRollbackError):
        apply_import(NOTES, "n.txt", plan, target, config, node, history=history)
    assert sha256_file(pres) == before and node.running and not history.load()


def test_write_refused_when_file_changed_underneath(env):
    tmp, pres, node, config, history = env
    plan, target = _plan(node, config)
    real_fetch = node.fetch_file
    changed = {}

    def fetch_then_edit(remote, local):
        sha = real_fetch(remote, local)
        pres.write_bytes(pres.read_bytes() + b"\x00")      # someone edits the file after it was read
        changed["sha"] = sha256_file(pres)
        return sha

    node.fetch_file = fetch_then_edit
    with pytest.raises(AssertionError):
        apply_import(NOTES, "n.txt", plan, target, config, node, history=history)
    assert sha256_file(pres) == changed["sha"]            # the concurrent edit was not overwritten
    assert not history.load()


def test_flagged_lines_are_reported_and_never_become_slides(env):
    tmp, pres, node, config, history = env
    plan, _ = asyncio.run(plan_for_transport("Psalm 23\nRomans 8:28 CSB\nJohn 3:16 NIV", config, node, default_version="NIV",
                                              target_uuid=None, target_name="Sample", library=None))
    assert [e.text for e in plan.review] == ["Psalm 23"]
    assert len(plan.slide_plan.slides) == 2          # CSB has no provider in this offline fixture, but fetch is faked


def test_report_marks_flagged_and_clean(env):
    from versebridge.workflow.report import build_report
    tmp, pres, node, config, history = env
    plan, _ = _plan(node, config)
    assert build_report(plan)["clean"] is True
    flagged, _ = asyncio.run(plan_for_transport("Psalm 23\nJohn 3:16 NIV", config, node, default_version="NIV", target_uuid=None, target_name="Sample", library=None))
    report = build_report(flagged)
    assert report["clean"] is False and report["flagged"][0]["status"] == "needs_review"


def test_target_is_found_from_disk_when_propresenter_is_closed(env):
    tmp, pres, node, config, history = env
    node.running = False

    def offline(route):
        raise ConnectionError("API down")

    node.api_get = offline
    lib, uuid_, name, path, _ = pipeline._target_from_api(node, uuid_=None, name="Sample", library=None)
    assert name and path == str(pres) and uuid_
    plan, _ = asyncio.run(plan_for_transport(NOTES, config, node, default_version="NIV", target_uuid=None, target_name="Sample", library=None))
    result = apply_import(NOTES, "n.txt", plan, (lib, uuid_, name, path, {}), config, node, history=history)
    assert result.slide_count == 4 and not result.reopened and node.log == ["install"]


# ----------------------------------------------------------------------------- new presentation (the default)
from versebridge.notes.title import document_title, safe_presentation_name, unique_name
from versebridge.presentation.protobuf_writer import ProtobufPresentationWriter


def test_publish_new_creates_separate_presentation_without_touching_the_original(env):
    tmp, pres, node, config, history = env
    before = sha256_file(pres)
    plan, _ = _plan(node, config)
    result = pipeline.publish_new(NOTES, "n.txt", "9/27/26 Heroes: Martin Luther", plan, config, node, history=history, verify_timeout=3)
    assert result.mode == "new" and result.presentation_name == "9-27-26 Heroes - Martin Luther" and not result.restart_needed
    assert sha256_file(pres) == before                         # existing presentation never touched
    assert node.log == ["create"]                              # no quit, no launch, no install
    assert (pres.parent / "9-27-26 Heroes - Martin Luther.pro").exists()


def test_publish_new_twice_never_overwrites_and_numbers_the_second(env):
    tmp, pres, node, config, history = env
    plan, _ = _plan(node, config)
    a = pipeline.publish_new(NOTES, "n.txt", "Message", plan, config, node, history=history, verify_timeout=3)
    b = pipeline.publish_new(NOTES, "n.txt", "Message", plan, config, node, history=history, verify_timeout=3)
    assert (a.presentation_name, b.presentation_name) == ("Message", "Message (2)")


def test_undo_of_a_new_presentation_deletes_only_that_file(env):
    tmp, pres, node, config, history = env
    plan, _ = _plan(node, config)
    r = pipeline.publish_new(NOTES, "n.txt", "Message", plan, config, node, history=history, verify_timeout=3)
    created = pres.parent / "Message.pro"
    assert created.exists()
    undo_import(r.import_id, config, node, history=history)
    assert not created.exists() and pres.exists()


def test_undo_refuses_to_delete_a_presentation_that_was_edited(env):
    from versebridge.errors import PresentationWriteError
    tmp, pres, node, config, history = env
    plan, _ = _plan(node, config)
    r = pipeline.publish_new(NOTES, "n.txt", "Message", plan, config, node, history=history, verify_timeout=3)
    created = pres.parent / "Message.pro"
    ProtobufPresentationWriter().append_plan(created, plan.slide_plan, config.presentation_template, template_path=pres)   # someone added slides
    with pytest.raises(PresentationWriteError):
        undo_import(r.import_id, config, node, history=history)
    assert created.exists()


# ----------------------------------------------------------------------------- --replace keeps blank and background slides
def test_replace_removes_only_verse_and_point_slides(env):
    from versebridge.slides.models import PlannedSlide, SlidePlan
    tmp, pres, node, config, history = env
    # give the presentation some non-verse slides by cloning the template cue and stripping its text objects
    m = presentation_pb2.Presentation()
    m.ParseFromString(pres.read_bytes())
    blank = m.cues.add()
    blank.CopyFrom(m.cues[0])
    blank.uuid.string = "00000000-1111-2222-3333-444444444444"
    for action in blank.actions:
        if action.WhichOneof("ActionTypeData") == "slide":
            del action.slide.presentation.base_slide.elements[:]
            action.label.text = "Background"
    group = m.cue_groups.add()
    group.CopyFrom(m.cue_groups[0])
    group.group.uuid.string = "aaaaaaaa-1111-2222-3333-444444444444"
    del group.cue_identifiers[:]
    group.cue_identifiers.add().string = blank.uuid.string
    pres.write_bytes(m.SerializeToString(deterministic=True))
    # keep a template presentation outside the target so the template cue survives clearing
    template_copy = pres.parent / "template.pro"
    shutil.copy(pres, template_copy)
    plan, target = _plan(node, config)
    original_fetch = node.fetch_file

    def fetch(remote, local):
        sha = original_fetch(remote, local)
        return sha

    apply_import(NOTES, "n.txt", plan, target, config, node, replace=True, clear_generated=True, history=history)
    after = presentation_pb2.Presentation()
    after.ParseFromString(pres.read_bytes())
    labels = [a.label.text for c in after.cues for a in c.actions]
    assert "Background" in labels                                # kept
    kept_original = {c.uuid.string for c in after.cues} & {c.uuid.string for c in m.cues}
    assert kept_original == {blank.uuid.string}                  # the template's verse/point cues were cleared
    assert len(after.cues) == 1 + len(plan.slide_plan.slides)


# ----------------------------------------------------------------------------- titles and names
def test_document_title_rules():
    assert document_title("JPL 8/23/26 Summer in the Psalms\nJohn 3:16 NIV", "x.pdf") == "JPL 8-23-26 Summer in the Psalms"
    assert document_title("Illuminary <a@b.com>\nSunday Scripture List\nJohn 3:16", "x.pdf") == "Sunday Scripture List"
    assert document_title("John 3:16 NIV\n1) A point", "Easter_Notes.pdf") == "Easter Notes"
    assert document_title("“A quote.”\nJohn 3:16", "") == "Sermon Notes"


def test_names_are_file_safe_and_unique():
    assert safe_presentation_name('A/B: C*D?') == "A-B - CD"
    assert unique_name("Verses", {"verses"}) == "Verses (2)"
    assert unique_name("X", {"x", "x (2)"}) == "X (3)"
    assert len(safe_presentation_name("word " * 80)) <= 100
