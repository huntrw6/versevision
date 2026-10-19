from __future__ import annotations

import asyncio
import shutil
from pathlib import Path

import pytest

from fakes import FakeNode
from versevision.config import AppConfig, PresentationTemplateConfig
from versevision.errors import VerificationRollbackError
from versevision.history import HistoryStore
from versevision.node.transport import sha256_file
from versevision.presentation.generated import presentation_pb2
from versevision.presentation.template import inspect_point_candidates, inspect_template_candidates
from versevision.workflow import pipeline
from versevision.workflow.pipeline import apply_import, plan_for_transport, publish_new, publish_swap, undo_import

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
    from versevision.bibles.models import BibleVerse, ResolvedPassage

    async def fake_fetch(self, reference):
        verses = [BibleVerse(book_id=reference.book_id, chapter=s.start.chapter, verse=v, text=f"Text of verse {v}.", version=reference.version or "NIV")
                  for s in reference.spans for v in range(s.start.verse, (s.end or s.start).verse + 1)]
        return ResolvedPassage(reference=reference, resolved_version=reference.version or "NIV", verses=verses)

    monkeypatch.setattr("versevision.bibles.registry.ProviderRegistry.fetch", fake_fetch)
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
    from versevision.workflow.report import build_report
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
from versevision.notes.title import document_title, safe_presentation_name, unique_name
from versevision.presentation.protobuf_writer import ProtobufPresentationWriter


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
    from versevision.errors import PresentationWriteError
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
    from versevision.slides.models import PlannedSlide, SlidePlan
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
    t = config.presentation_template
    assert kept_original == {blank.uuid.string, t.cue_uuid, t.point_cue_uuid}   # blank + the template slides themselves are kept
    assert len(after.cues) == len(kept_original) + len(plan.slide_plan.slides)


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


def test_template_slides_in_the_same_file_are_never_treated_as_generated(env):
    """--replace on the presentation that also holds the template must keep the template slides."""
    from versevision.presentation.protobuf_writer import ProtobufPresentationWriter

    tmp, pres, node, config, history = env
    message = ProtobufPresentationWriter()._read(pres)
    victims = {u.casefold() for u in ProtobufPresentationWriter.generated_cue_uuids(message, config.presentation_template)}
    for cue in (config.presentation_template.cue_uuid, config.presentation_template.point_cue_uuid):
        assert cue.casefold() not in victims


# ----------------------------------------------------------------------------- copies: no restart, original untouched
def _sha(path):
    import hashlib
    return hashlib.sha256(path.read_bytes()).hexdigest()


def _cues(path):
    m = presentation_pb2.Presentation()
    m.ParseFromString(path.read_bytes())
    return m


def test_replace_makes_a_numbered_copy_and_never_closes_the_app(env):
    from versevision.notes.title import numbered_name
    tmp, pres, node, config, history = env
    plan, target = _plan(node, config)
    before = _sha(pres)
    result = publish_new("n", "n.txt", "T", plan, config, node, history=history, base=target, clear_generated=True)
    assert "quit" not in node.log and "launch" not in node.log          # ProPresenter never restarted
    assert node.running
    assert _sha(pres) == before                                          # the original is byte-for-byte untouched
    copy = pres.parent / "Sample 2.pro"
    assert copy.exists() and result.presentation_name == "Sample 2" and result.mode == "replace"
    m = _cues(copy)
    assert m.name == "Sample 2"
    assert len(m.cues) == result.cue_count_after
    original = _cues(pres)
    assert not ({c.uuid.string for c in m.cues} & {c.uuid.string for c in original.cues})   # all ids are new
    assert m.uuid.string != original.uuid.string


def test_append_copy_keeps_every_original_slide_and_adds_to_the_end(env):
    tmp, pres, node, config, history = env
    plan, target = _plan(node, config)
    original = _cues(pres)
    result = publish_new("n", "n.txt", "T", plan, config, node, history=history, base=target, clear_generated=False)
    m = _cues(pres.parent / "Sample 2.pro")
    assert result.mode == "append"
    assert len(m.cues) == len(original.cues) + len(plan.slide_plan.slides)
    assert [c.actions[0].label.text for c in m.cues[:len(original.cues)]] == [c.actions[0].label.text for c in original.cues]


def test_second_copy_is_numbered_three(env):
    tmp, pres, node, config, history = env
    plan, target = _plan(node, config)
    publish_new("n", "n.txt", "T", plan, config, node, history=history, base=target, clear_generated=True)
    second = publish_new("n", "n.txt", "T", plan, config, node, history=history, base=target, clear_generated=True)
    assert second.presentation_name == "Sample 3"
    assert (pres.parent / "Sample 3.pro").exists()


def test_copy_is_put_in_the_playlist_directly_under_the_original_and_undo_removes_it(env):
    tmp, pres, node, config, history = env
    plan, target = _plan(node, config)
    result = publish_new("n", "n.txt", "T", plan, config, node, history=history, base=target, clear_generated=True)
    names = [i["id"]["name"] for i in node.playlist]
    assert names == ["Sermon", "Sample", "Sample 2", "Closing"]            # directly under the original, header order kept
    assert [i["id"]["index"] for i in node.playlist] == [0, 1, 2, 3]       # indexes stay in order
    undo_import(result.import_id, config, node, history=history)
    assert [i["id"]["name"] for i in node.playlist] == ["Sermon", "Sample", "Closing"]
    assert not (pres.parent / "Sample 2.pro").exists()


def test_no_playlist_flag_leaves_playlists_alone(env):
    tmp, pres, node, config, history = env
    plan, target = _plan(node, config)
    publish_new("n", "n.txt", "T", plan, config, node, history=history, base=target, clear_generated=True, add_to_playlists=False)
    assert [i["id"]["name"] for i in node.api_get("/v1/playlist/PL1")["items"]] == ["Sermon", "Sample", "Closing"]


def test_a_new_document_presentation_is_not_added_to_any_playlist(env):
    tmp, pres, node, config, history = env
    plan, _ = _plan(node, config)
    publish_new("n", "n.txt", "Fresh Notes", plan, config, node, history=history, library=None)
    assert [i["id"]["name"] for i in node.api_get("/v1/playlist/PL1")["items"]] == ["Sermon", "Sample", "Closing"]


# ----------------------------------------------------------------------------- templates in their own file
def test_template_file_holds_only_the_two_template_slides_and_a_sermon_replace_cannot_delete_them(env):
    from versevision.presentation.protobuf_writer import ProtobufPresentationWriter
    tmp, pres, node, config, history = env
    t = config.presentation_template
    out = tmp / "Templates.pro"
    ProtobufPresentationWriter().create_template_file(pres, out, t, name="Templates")
    m = _cues(out)
    assert m.name == "Templates" and {c.uuid.string.casefold() for c in m.cues} == {t.cue_uuid.casefold(), t.point_cue_uuid.casefold()}
    assert m.uuid.string != _cues(pres).uuid.string
    # a sermon presentation that does NOT hold the templates can be cleared and refilled with the templates coming from the other file
    plan, target = _plan(node, config)
    sermon = tmp / "Sermon.pro"
    shutil.copy(pres, sermon)
    ProtobufPresentationWriter().append_plan(sermon, plan.slide_plan, t, template_path=out, remove_cue_uuids=ProtobufPresentationWriter.generated_cue_uuids(_cues(sermon), t) or None)
    after = _cues(sermon)
    assert len(after.cues) >= len(plan.slide_plan.slides)


def test_template_is_found_by_name_when_propresenter_has_reissued_its_id(env):
    from versevision.workflow.pipeline import _template_target
    tmp, pres, node, config, history = env
    stale = config.presentation_template.model_copy(update={"presentation_uuid": "00000000-DEAD-BEEF-0000-000000000000"})
    assert _template_target(node, stale)[2] == "Sample"


def test_style_fingerprint_ignores_the_sign_of_zero(env):
    """A template copied through another machine can come back with -0.0 instead of 0.0; that is not a style change."""
    from versevision.presentation.template import inspect_point_candidates
    tmp, pres, node, config, history = env
    point_cue = config.presentation_template.point_cue_uuid
    before = {c.casefold(): fp for _, c, fp in inspect_point_candidates(pres)}
    m = _cues(pres)
    for cue in m.cues:
        if cue.uuid.string.casefold() == point_cue.casefold():
            for a in cue.actions:
                if a.WhichOneof("ActionTypeData") == "slide":
                    for e in a.slide.presentation.base_slide.elements:
                        if e.element.name == "Sermon Point":
                            e.element.text.attributes.stroke_width = -0.0
    flipped = tmp / "flipped.pro"
    flipped.write_bytes(m.SerializeToString(deterministic=True))
    after = {c.casefold(): fp for _, c, fp in inspect_point_candidates(flipped)}
    assert before[point_cue.casefold()] == after[point_cue.casefold()]


# ----------------------------------------------------------------------------- swap: replace / append under the original name
def _folder_files(pres):
    return sorted(p.name for p in pres.parent.glob("*.pro"))


def _playlist_names(node):
    return [i["id"]["name"] for i in node.api_get("/v1/playlist/PL1")["items"]]


def test_replace_swaps_in_place_without_restart_and_the_playlist_keeps_one_entry(env):
    tmp, pres, node, config, history = env
    plan, target = _plan(node, config)
    original_slides = len(_cues(pres).cues)
    result = publish_swap("n", "n.txt", plan, config, node, target, clear_generated=True, history=history)
    assert "quit" not in node.log and "launch" not in node.log and node.running            # never restarted
    assert result.mode == "replace" and result.presentation_name == "Sample" and result.verified
    assert _folder_files(pres) == ["Sample.pro"]                                            # the old one is gone, only one file
    assert _playlist_names(node) == ["Sermon", "Sample", "Closing"]                         # still ONE entry, same place
    entry = next(i for i in node.api_get("/v1/playlist/PL1")["items"] if i["type"] == "presentation")
    assert entry["presentation_info"]["presentation_uuid"] != "P1"                          # it opens the NEW file now
    assert node.api_get("/v1/presentation/" + entry["presentation_info"]["presentation_uuid"])["presentation"]["groups"][0]["slides"]
    assert len(_cues(pres).cues) == result.cue_count_after
    backup = next(node.store.glob("Sample.pro.bak*"))                                       # the original was backed up first
    assert len(_cues(backup).cues) == original_slides


def test_append_swap_keeps_every_original_slide(env):
    tmp, pres, node, config, history = env
    plan, target = _plan(node, config)
    original = _cues(pres)
    result = publish_swap("n", "n.txt", plan, config, node, target, clear_generated=False, history=history)
    new = _cues(pres)
    assert result.mode == "append" and len(new.cues) == len(original.cues) + len(plan.slide_plan.slides)
    assert [c.actions[0].label.text for c in new.cues[:len(original.cues)]] == [c.actions[0].label.text for c in original.cues]
    assert _folder_files(pres) == ["Sample.pro"]


def test_swap_needs_propresenter_open_and_changes_nothing_if_it_is_not(env):
    tmp, pres, node, config, history = env
    plan, target = _plan(node, config)
    node.running = False
    before = _sha(pres)
    with pytest.raises(Exception, match="must be open"):
        publish_swap("n", "n.txt", plan, config, node, target, clear_generated=True, history=history)
    assert _sha(pres) == before and _folder_files(pres) == ["Sample.pro"]


def test_swap_puts_everything_back_if_propresenter_never_shows_the_new_version(env, monkeypatch):
    from versevision.workflow import pipeline
    tmp, pres, node, config, history = env
    plan, target = _plan(node, config)
    monkeypatch.setattr(pipeline, "SWAP_WAIT_SECONDS", 0.2)
    monkeypatch.setattr(pipeline, "SETTLE_SECONDS", 0.5)
    monkeypatch.setattr(pipeline, "MIN_GAP_SECONDS", 0.0)
    monkeypatch.setattr(pipeline, "verify_via_api", lambda *a, **k: (False, "not shown"))
    before = _sha(pres)
    with pytest.raises(Exception, match="did not show"):
        publish_swap("n", "n.txt", plan, config, node, target, clear_generated=True, history=history)
    assert _folder_files(pres) == ["Sample.pro"] and _sha(pres) == before                   # original back, byte for byte
    assert _playlist_names(node) == ["Sermon", "Sample", "Closing"]
    assert history.load() == []


def test_swap_puts_everything_back_if_the_playlist_cannot_be_updated(env):
    tmp, pres, node, config, history = env
    plan, target = _plan(node, config)
    node.fail_playlist_put = True
    before = _sha(pres)
    with pytest.raises(Exception, match="refused"):
        publish_swap("n", "n.txt", plan, config, node, target, clear_generated=True, history=history)
    assert _folder_files(pres) == ["Sample.pro"] and _sha(pres) == before
    entry = next(i for i in node.api_get("/v1/playlist/PL1")["items"] if i["type"] == "presentation")
    assert entry["presentation_info"]["presentation_uuid"] == "P1"                          # still the original


def test_swap_does_not_delete_a_renamed_original_that_changed_underneath(env):
    tmp, pres, node, config, history = env
    plan, target = _plan(node, config)
    real_rename = node.rename_file

    def rename_then_edit(old, new):
        real_rename(old, new)
        edited = presentation_pb2.Presentation()                       # someone deletes a slide after we read it
        edited.ParseFromString(Path(new).read_bytes())
        del edited.cues[-1]
        Path(new).write_bytes(edited.SerializeToString())
    node.rename_file = rename_then_edit
    with pytest.raises(Exception, match="changed since"):
        publish_swap("n", "n.txt", plan, config, node, target, clear_generated=True, history=history)
    assert "Sample.pro" in _folder_files(pres)                                              # the edited original is not lost
    assert _playlist_names(node) == ["Sermon", "Sample", "Closing"]


def test_undo_of_a_swap_brings_the_previous_version_back_under_the_same_name(env):
    tmp, pres, node, config, history = env
    plan, target = _plan(node, config)
    before = _sha(pres)
    before_slides = len(_cues(pres).cues)
    result = publish_swap("n", "n.txt", plan, config, node, target, clear_generated=True, history=history)
    assert _sha(pres) != before
    undo_import(result.import_id, config, node, history=history)
    assert len(_cues(pres).cues) == before_slides and _folder_files(pres) == ["Sample.pro"]
    assert _playlist_names(node) == ["Sermon", "Sample", "Closing"]
    assert "quit" not in node.log


def test_undo_of_a_swap_refuses_if_the_presentation_was_edited_afterwards(env):
    tmp, pres, node, config, history = env
    plan, target = _plan(node, config)
    result = publish_swap("n", "n.txt", plan, config, node, target, clear_generated=True, history=history)
    pres.write_bytes(pres.read_bytes() + b"\x00")
    with pytest.raises(Exception, match="edited after"):
        undo_import(result.import_id, config, node, history=history)
    assert history.get(result.import_id).status == "active"


def test_playlist_entry_is_found_by_name_when_its_id_was_reissued(env):
    from versevision.workflow.pipeline import _find_playlist_slots
    tmp, pres, node, config, history = env
    node.api_get("/v1/playlist/PL1")  # touch
    node.playlist[1]["presentation_info"]["presentation_uuid"] = "SOMETHING-ELSE"
    assert _find_playlist_slots(node, "P1") == []
    assert len(_find_playlist_slots(node, "P1", "Sample")) == 1


def test_swap_never_writes_the_new_file_sooner_than_the_minimum_gap_after_the_rename(env, monkeypatch):
    """ProPresenter ends up with a dead entry if a new file appears at the old name within ~2 s of a rename."""
    import time as _time
    from versevision.workflow import pipeline
    tmp, pres, node, config, history = env
    plan, target = _plan(node, config)
    monkeypatch.setattr(pipeline, "MIN_GAP_SECONDS", 1.0)
    stamps = {}
    real_rename, real_create = node.rename_file, node.create_file
    node.rename_file = lambda a, b: (stamps.setdefault("rename", _time.monotonic()), real_rename(a, b))[1]
    node.create_file = lambda a, b: (stamps.setdefault("create", _time.monotonic()), real_create(a, b))[1]
    publish_swap("n", "n.txt", plan, config, node, target, clear_generated=True, history=history)
    assert stamps["create"] - stamps["rename"] >= 0.95
