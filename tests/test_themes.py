"""ProPresenter themes as the look of verse and point slides. The fixtures are real theme files from the church's ProPresenter."""
from __future__ import annotations

import asyncio
from pathlib import Path

import pytest

from test_pipeline import NOTES, _config, env  # noqa: F401
from versevision import themes
from versevision.config import with_options
from versevision.presentation.generated import presentation_pb2
from versevision.workflow import pipeline
from versevision.workflow.pipeline import plan_for_transport

THEMES = Path(__file__).parent / "fixtures" / "themes"


def cues_of(path: Path):
    m = presentation_pb2.Presentation()
    m.ParseFromString(path.read_bytes())
    return m


def texts(cue) -> dict[str, str]:
    import re

    def _plain_rtf(data: bytes) -> str:
        tail = re.split(rb"expandedcolortbl[^}]*\}", data)[-1].decode("latin1")
        return tail.replace("\\line ", "\n").strip("}\n\r ")
    out = {}
    for a in cue.actions:
        if a.WhichOneof("ActionTypeData") == "slide":
            for e in a.slide.presentation.base_slide.elements:
                if e.element.HasField("text"):
                    out[e.element.name] = _plain_rtf(e.element.text.rtf_data)
    return out


def test_the_theme_files_are_read_with_their_text_boxes():
    slides = themes.read_theme_tree(THEMES)
    by_label = {s.label: s for s in slides}
    assert set(by_label) == {"Scripture main screens · Slide 1", "Sermon Points · Theme Slide", "The biblke · Slide 1"}
    assert len(by_label["The biblke · Slide 1"].boxes) == 2 and len(by_label["Scripture main screens · Slide 1"].boxes) == 1
    box = by_label["Scripture main screens · Slide 1"].boxes[0]
    assert (round(box.w), round(box.h)) == (1620, 880)


def test_choosing_a_theme_and_slide():
    slides = themes.read_theme_tree(THEMES)
    assert themes.find_slide(slides, "sermon points").name == "Theme Slide"
    assert themes.find_slide(slides, "Sermon Points", "1").number == 1
    assert themes.find_slide(slides, "Sermon Points", "theme slide").number == 1
    with pytest.raises(themes.ThemeError, match="Loaded themes"):
        themes.find_slide(slides, "Nope")
    with pytest.raises(themes.ThemeError, match="has no slide"):
        themes.find_slide(slides, "Sermon Points", "9")


def test_unset_themes_change_nothing(env):
    tmp, pres, node, config, history = env
    node.themes = THEMES
    plan, _ = asyncio.run(plan_for_transport(NOTES, config, node, default_version="NIV", target_uuid=None, target_name=None, library=None))
    assert plan.slide_plan.looks is None


def _themes_with_split(tmp: Path) -> Path:
    """The real themes plus "Split": The biblke with its small box moved clear of the big one (a true two-box layout)."""
    import shutil
    root = tmp / "themes"
    if not root.exists():
        shutil.copytree(THEMES, root)
        from versevision.presentation.generated import template_pb2
        doc = template_pb2.Template.Document()
        doc.ParseFromString((THEMES / "The biblke" / "Theme").read_bytes())
        box = min((w for w in doc.slides[0].base_slide.elements if w.element.HasField("text")), key=lambda w: w.element.bounds.size.height)
        box.element.bounds.origin.y = 800
        (root / "Split").mkdir()
        (root / "Split" / "Theme").write_bytes(doc.SerializeToString())
    return root


def build(env, **sets):
    tmp, pres, node, config, history = env
    node.themes = _themes_with_split(tmp)
    config = with_options(config, sets=[f"{k.replace('__', '.')}={v}" for k, v in sets.items()])
    plan, _ = asyncio.run(plan_for_transport(NOTES, config, node, default_version="NIV", target_uuid=None, target_name=None, library=None))
    return config, plan, node, history


def written(env, name, **sets):
    tmp, pres, node, config, history = env
    config, plan, node, history = build(env, **sets)
    result = pipeline.publish_new(NOTES, "n.txt", name, plan, config, node, history=history, verify_timeout=3)
    return config, plan, node, history, result, cues_of(pres.parent / f"{result.presentation_name}.pro")


def test_a_two_box_theme_gives_verse_and_reference_separate_boxes_and_the_point_theme_gives_points(env):
    config, plan, node, history, result, cues = written(env, "Themed", verses__theme="Split", points__theme="Sermon Points")
    assert plan.slide_plan.looks.describe() == {"verse": "Split · Slide 1", "point": "Sermon Points · Theme Slide"}
    themed = [c for c in cues.cues if c.name == themes.MARKER]
    assert len(themed) == 4
    verse = next(texts(c) for c in themed if "Verse" in texts(c))
    assert set(verse) == {"Reference", "Verse"} and "John 3:16 (NIV)" in verse["Reference"] and "Text of verse 16" in verse["Verse"]
    point = next(texts(c) for c in themed if "Sermon Point" in texts(c))
    assert set(point) == {"Sermon Point"} and "1) First point" in point["Sermon Point"]


def test_a_one_box_theme_puts_the_reference_on_the_last_line_and_points_keep_the_plain_template(env):
    config, plan, node, history, result, cues = written(env, "OneBox", verses__theme="Scripture main screens")
    themed = [c for c in cues.cues if c.name == themes.MARKER]
    plain = [c for c in cues.cues if c.name != themes.MARKER and "Sermon Point" in texts(c)]
    assert len(themed) == 2 and plain
    only = texts(themed[0])
    assert list(only) == ["Verse"] and only["Verse"].rstrip().splitlines()[-1].endswith("John 3:16 (NIV)") and "Text of verse 16" in only["Verse"]


def test_replace_recognises_themed_slides_but_not_the_operators_own(env):
    from versevision.presentation.protobuf_writer import ProtobufPresentationWriter
    config, plan, node, history, result, cues = written(env, "Themed2", verses__theme="The biblke", points__theme="Sermon Points")
    found = ProtobufPresentationWriter.generated_cue_uuids(cues, config.presentation_template)
    themed = {c.uuid.string for c in cues.cues if c.name == themes.MARKER}
    assert themed <= set(found) and len(themed) == 4
    # a theme with a shape or background image on it: the marker is what lets Replace claim the slide
    sample = presentation_pb2.Presentation()
    sample.CopyFrom(cues)
    for cue in sample.cues:
        if cue.name == themes.MARKER:
            for a in cue.actions:
                if a.WhichOneof("ActionTypeData") == "slide":
                    a.slide.presentation.base_slide.elements.add().element.name = "Background shape"
    assert themed <= set(ProtobufPresentationWriter.generated_cue_uuids(sample, config.presentation_template))
    for cue in sample.cues:
        if cue.name == themes.MARKER:
            cue.name = ""
    assert not (themed & set(ProtobufPresentationWriter.generated_cue_uuids(sample, config.presentation_template)))


def test_the_node_must_have_the_theme_the_config_asks_for(env):
    tmp, pres, node, config, history = env
    node.themes = THEMES
    bad = with_options(config, sets=["verses.theme=Does Not Exist"])
    with pytest.raises(themes.ThemeError, match="Loaded themes: .*Sermon Points"):
        asyncio.run(plan_for_transport(NOTES, bad, node, default_version="NIV", target_uuid=None, target_name=None, library=None))


def test_boxes_stacked_on_each_other_are_one_box_so_the_reference_is_the_last_line(env):
    """The biblke's two boxes sit on the same black band: a separate reference box would print over the verse."""
    slides = themes.read_theme_tree(THEMES)
    biblke = next(x for x in slides if x.theme == "The biblke")
    assert len(biblke.boxes) == 2 and biblke.has_reference_box is False
    config, plan, node, history, result, cues = written(env, "Stacked", verses__theme="The biblke")
    verse = next(texts(c) for c in cues.cues if c.name == themes.MARKER and "Verse" in texts(c))
    assert set(verse) - {themes.EXTRA} == {"Verse"} and verse["Verse"].rstrip().splitlines()[-1].endswith("John 3:16 (NIV)")


def test_a_theme_box_with_an_empty_rtf_gets_its_font_size_and_colour_written_into_the_text(env):
    """The biblke's boxes have no run formatting in their rtf; without it the verse rendered invisible on the iMac."""
    config, plan, node, history, result, cues = written(env, "EmptyRtf", verses__theme="Split")
    verse = next(c for c in cues.cues if c.name == themes.MARKER and "Verse" in texts(c))
    rtf = {e.element.name: e.element.text.rtf_data for a in verse.actions if a.WhichOneof("ActionTypeData") == "slide"
           for e in a.slide.presentation.base_slide.elements if e.element.HasField("text")}
    for name in ("Verse", "Reference"):
        assert b"\\f0" in rtf[name] and b"\\cf2" in rtf[name] and b"\\red255\\green255\\blue255" in rtf[name], rtf[name][:200]
    assert b"HelveticaNeue" in rtf["Verse"] and b"\\fs84" in rtf["Verse"] and b"BanglaMN-Bold" in rtf["Reference"] and b"\\fs92" in rtf["Reference"]
    assert b"\\qc" in rtf["Verse"]
