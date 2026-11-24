"""The oracle of the ProPresenter file writer and the theme reader (S6a): what the Python app writes for a slide plan, for the Rust port to repeat byte for byte.

Written by ``export_oracle.py`` into ``oracle/pro/`` (never by hand). Randomness is made injectable: the exporter replaces
``versevision.presentation.protobuf_writer.new_uuid`` with a seeded source (the n-th id of a case is the version-4 uuid made from the first 16 bytes of
``sha256("<case>:<n>")``) and the clock of the module with a fixed one, so a run can be reproduced exactly.

Layout::

    oracle/pro/files/*.pro         presentations the cases start from (built here from python/tests/fixtures/sample/Sample.pro and
                                   python/tests/fixtures/propresenter19/Verses.original.pro; a case may also name a fixture: "fixture:sample/Sample.pro")
    oracle/pro/themes/...          a copy of ProPresenter's Themes folder (the three real theme files in python/tests/fixtures/themes plus made-up ones)
    oracle/pro/themes.json         what themes.py makes of that folder: slides, picker rows, find_slide answers and errors
    oracle/pro/candidates.json     inspect_template_candidates / inspect_point_candidates of every file above
    oracle/pro/cases/<case>/       options.toml, plan.json, expected.json and expected.pro (an error case has no expected.pro)

``options.toml`` keys: ``mode`` (append | replace | derive | create | template_file | synthetic), ``base`` (the presentation written to or copied),
``template`` (the presentation that holds the template slides; absent: the template is inside ``base``), ``name``, ``now`` (Unix seconds),
``uuid_seed``, ``clear_generated``, ``keep_uuid``, ``remove`` (cue ids for append), ``body``/``label`` (synthetic), ``[definition]`` (the saved
``presentation_template`` config) and ``[looks]`` (``themes`` folder, ``verse``/``point`` theme and slide). ``expected.json`` has ``result`` (the
``PlanWriteResult`` without paths) or ``error`` (``"<ExceptionName>: <message>"``) and ``validate`` (``validate_template`` on the template file).
"""
from __future__ import annotations

import hashlib
import json
import shutil
import sys
import tempfile
import uuid
from datetime import datetime
from pathlib import Path
from typing import Any

import tomli_w

NOW = 1784419571
_SRC = Path(__file__).resolve().parents[1] / "src" / "versevision"
sys.path.insert(0, str(_SRC / "presentation" / "generated"))

import cue_pb2  # noqa: E402
import presentation_pb2  # noqa: E402
import template_pb2  # noqa: E402

from versevision import themes  # noqa: E402
from versevision.config import PresentationTemplateConfig  # noqa: E402
from versevision.errors import VerseVisionError  # noqa: E402
from versevision.presentation import protobuf_writer  # noqa: E402
from versevision.presentation.protobuf_writer import ProtobufPresentationWriter  # noqa: E402
from versevision.presentation.template import (  # noqa: E402
    create_template_definition, inspect_point_candidates, inspect_template_candidates, validate_template,
)
from versevision.slides.models import SlidePlan  # noqa: E402


def dumps(obj: Any) -> str:
    return json.dumps(obj, indent=2, sort_keys=True, ensure_ascii=False) + "\n"


# ------------------------------------------------------------------------------------------------------------ determinism
def seeded_uuids(seed: str):
    counter = [0]

    def new_uuid() -> uuid.UUID:
        digest = hashlib.sha256(f"{seed}:{counter[0]}".encode()).digest()[:16]
        counter[0] += 1
        return uuid.UUID(bytes=digest, version=4)

    return new_uuid


class _FrozenDatetime(datetime):
    @classmethod
    def now(cls, tz=None):
        return datetime.fromtimestamp(NOW, tz)


class frozen:
    """Inside the block the writer's ids are seeded and its clock stands still."""

    def __init__(self, seed: str):
        self.seed = seed

    def __enter__(self):
        self._saved = (protobuf_writer.new_uuid, protobuf_writer.datetime)
        protobuf_writer.new_uuid = seeded_uuids(self.seed)
        protobuf_writer.datetime = _FrozenDatetime
        return self

    def __exit__(self, *exc):
        protobuf_writer.new_uuid, protobuf_writer.datetime = self._saved


# ------------------------------------------------------------------------------------------------------------ building the starting files
def _read(path: Path):
    message = presentation_pb2.Presentation()
    message.ParseFromString(path.read_bytes())
    return message


class _Ids:
    """Made-up but fixed ids for the files the exporter builds (not the ids of the writer under test)."""

    def __init__(self, seed: str, upper: bool = False):
        self.seed, self.n, self.upper = seed, 0, upper

    def __call__(self) -> str:
        digest = hashlib.sha256(f"file:{self.seed}:{self.n}".encode()).digest()[:16]
        self.n += 1
        text = str(uuid.UUID(bytes=digest, version=4))
        return text.upper() if self.upper else text


def _renew_ids(message, ids: _Ids) -> None:
    for field, value in message.ListFields():
        if field.message_type is None:
            continue
        for item in (value if field.is_repeated else [value]):
            if field.message_type.full_name == "rv.data.UUID":
                if item.string and item.string != "00000000-0000-0000-0000-000000000000":
                    item.string = ids()
            else:
                _renew_ids(item, ids)


def _slide_action(cue):
    return next(a for a in cue.actions if a.WhichOneof("ActionTypeData") == "slide")


def _elements(cue):
    return _slide_action(cue).slide.presentation.base_slide.elements


def _set(cue, name: str, text: str) -> None:
    for item in _elements(cue):
        if item.element.name == name and item.element.HasField("text"):
            protobuf_writer._set_text(item.element.text, text)


def _clone_cue(source, ids: _Ids, *, name: str | None = None, texts: dict[str, str] | None = None):
    cue = cue_pb2.Cue()
    cue.CopyFrom(source)
    _renew_ids(cue, ids)
    if name is not None:
        cue.name = name
    for key, text in (texts or {}).items():
        _set(cue, key, text)
    return cue


def _add_group(presentation, template_group, cue, label: str, ids: _Ids):
    group = presentation_pb2.Presentation.CueGroup()
    group.CopyFrom(template_group)
    _renew_ids(group.group, ids)
    group.group.name = label
    group.group.application_group_name = label
    del group.cue_identifiers[:]
    group.cue_identifiers.add().string = cue.uuid.string
    presentation.cues.append(cue)
    presentation.cue_groups.append(group)
    return group


def build_sample_files(repo: Path, out: Path) -> dict[str, Path]:
    """The presentations the cases start from. Every one is a valid ProPresenter 19.0.1 file (document version 15 or 10)."""
    files = out / "files"
    files.mkdir(parents=True, exist_ok=True)
    sample = repo / "python" / "tests" / "fixtures" / "sample" / "Sample.pro"
    windows = repo / "python" / "tests" / "fixtures" / "propresenter19" / "Verses.original.pro"
    made: dict[str, Path] = {}

    def save(name: str, message, extra: bytes = b"") -> None:
        path = files / f"{name}.pro"
        path.write_bytes(message.SerializeToString(deterministic=True) + extra)
        made[name] = path

    base = _read(sample)
    verse_cue = next(c for c in base.cues if any(e.element.name == "Verse" for e in _elements(c)))
    point_cue = next(c for c in base.cues if any(e.element.name == "Sermon Point" for e in _elements(c)))
    verse_group = next(g for g in base.cue_groups if any(i.string == verse_cue.uuid.string for i in g.cue_identifiers))
    point_group = next(g for g in base.cue_groups if any(i.string == point_cue.uuid.string for i in g.cue_identifiers))

    # a presentation that holds only the two template slides (what "create_template_file" makes), and the Sample as it is
    save("templates", base)

    # a service presentation: the operator's own slides (blank, title with a shape, an action that is not a slide), earlier generated verse and
    # point slides (plain and themed), one arrangement that lists every group
    ids = _Ids("service", upper=True)
    service = presentation_pb2.Presentation()
    service.CopyFrom(base)
    service.name = "Sunday"
    _renew_ids(service.uuid, ids)
    # keep the template slides in the file (the "template inside the target" setup of older installs)
    blank = _clone_cue(verse_cue, ids, name="Blank")
    del _slide_action(blank).slide.presentation.base_slide.elements[:]
    title = _clone_cue(verse_cue, ids, name="Title", texts={"Reference": "Sunday Service", "Verse": "Welcome"})
    for wrapper in _elements(title):
        if wrapper.element.name == "Reference":
            wrapper.element.name = "Title"
        else:
            wrapper.element.name = "Subtitle"
    shape = _slide_action(title).slide.presentation.base_slide.elements.add()
    shape.element.CopyFrom(_elements(title)[0].element)
    shape.element.ClearField("text")
    shape.element.name = "Shape"
    shape.element.uuid.string = ids()
    media = _clone_cue(verse_cue, ids, name="Media")
    media.actions[0].ClearField("slide")                      # an action with no slide: not something VerseVision made
    old_verse = _clone_cue(verse_cue, ids, texts={"Reference": "John 3:16 (NIV)", "Verse": "Old verse text"})
    old_point = _clone_cue(point_cue, ids, texts={"Sermon Point": "1) Old point"})
    old_themed = _clone_cue(verse_cue, ids, name=themes.MARKER, texts={"Reference": "Romans 8:28 (NIV)", "Verse": "Old themed verse"})
    old_extra = _slide_action(old_themed).slide.presentation.base_slide.elements.add()
    old_extra.element.CopyFrom(_elements(old_themed)[0].element)
    old_extra.element.name = themes.EXTRA
    old_extra.element.uuid.string = ids()
    two_blocks = _clone_cue(verse_cue, ids, texts={"Reference": "Psalm 23:1", "Verse": "Two"})
    extra_text = _slide_action(two_blocks).slide.presentation.base_slide.elements.add()
    extra_text.element.CopyFrom(_elements(two_blocks)[1].element)
    extra_text.element.name = "Notes"                         # a third text object: not a plain generated verse
    extra_text.element.uuid.string = ids()
    for cue, group_of, label in ((blank, verse_group, "Blank"), (title, verse_group, "Title"), (media, verse_group, "Media"),
                                 (old_verse, verse_group, "John 3:16 (NIV)"), (old_point, point_group, "1) Old point"),
                                 (old_themed, verse_group, "Romans 8:28 (NIV)"), (two_blocks, verse_group, "Psalm 23:1")):
        _add_group(service, group_of, cue, label, ids)
    arrangement = service.arrangements.add()
    arrangement.uuid.string = ids()
    arrangement.name = "normal"
    for group in service.cue_groups:
        arrangement.group_identifiers.add().string = group.group.uuid.string
    service.selected_arrangement.string = arrangement.uuid.string
    save("service", service)

    # a service where the group of an old generated slide is shared with a slide that stays (a group only loses the removed cue)
    shared = presentation_pb2.Presentation()
    shared.CopyFrom(service)
    first_generated = next(c for c in shared.cues if c.uuid.string == old_verse.uuid.string)
    group_with_two = next(g for g in shared.cue_groups if any(i.string == first_generated.uuid.string for i in g.cue_identifiers))
    group_with_two.cue_identifiers.add().string = next(c for c in shared.cues if c.name == "Title").uuid.string
    save("service_shared_group", shared)

    # no template inside: the other presentations (target for the "template in another file" setup)
    plain = presentation_pb2.Presentation()
    plain.CopyFrom(service)
    removed = {verse_cue.uuid.string, point_cue.uuid.string}
    plain.name = "Plain"
    kept_cues = [c for c in plain.cues if c.uuid.string not in removed]
    del plain.cues[:]
    plain.cues.extend(kept_cues)
    kept_groups = [g for g in plain.cue_groups if not (len(g.cue_identifiers) == 1 and g.cue_identifiers[0].string in removed)]
    dropped = {g.group.uuid.string for g in plain.cue_groups} - {g.group.uuid.string for g in kept_groups}
    del plain.cue_groups[:]
    plain.cue_groups.extend(kept_groups)
    for arr in plain.arrangements:
        keep = [i.string for i in arr.group_identifiers if i.string not in dropped]
        del arr.group_identifiers[:]
        for item in keep:
            arr.group_identifiers.add().string = item
    save("plain", plain)

    # an empty presentation (no cues): everything is appended
    empty = presentation_pb2.Presentation()
    empty.CopyFrom(service)
    del empty.cues[:]
    del empty.cue_groups[:]
    del empty.arrangements[:]
    empty.ClearField("selected_arrangement")
    empty.name = "Empty"
    save("empty", empty)

    # float -0.0 in the style (upb writes it) and fields the schema does not know, at the top and inside a cue
    quirk = presentation_pb2.Presentation()
    quirk.CopyFrom(base)
    for cue in quirk.cues:
        for wrapper in _elements(cue):
            wrapper.element.bounds.origin.x = -0.0
            wrapper.element.shadow.offset = -0.0
    first = quirk.cues[0]
    raw = first.SerializeToString() + b"\x98\x06\x07\xa2\x06\x03abc"           # fields 99 (varint) and 100 (bytes)
    first.ParseFromString(raw)
    save("quirks", quirk, extra=b"\xa8\x06\x2a")                             # field 101 at the top

    # a Windows-style file (document version 10): the single sample text object of Verses.original.pro becomes Reference + Verse + Sermon Point
    win = _read(windows)
    ids = _Ids("windows")
    template_cue = win.cues[0]
    template_group = win.cue_groups[0]
    verse_win = _clone_cue(template_cue, ids)
    reference_el = _elements(verse_win)[0]
    reference_el.element.name = "Reference"
    body_el = _slide_action(verse_win).slide.presentation.base_slide.elements.add()
    body_el.CopyFrom(reference_el)
    body_el.element.name = "Verse"
    body_el.element.uuid.string = ids()
    body_el.element.bounds.origin.y = 500
    body_el.element.bounds.size.height = 400
    point_win = _clone_cue(template_cue, ids)
    _elements(point_win)[0].element.name = "Sermon Point"
    windows_file = presentation_pb2.Presentation()
    windows_file.CopyFrom(win)
    del windows_file.cues[:]
    del windows_file.cue_groups[:]
    windows_file.name = "WindowsVerses"
    _add_group(windows_file, template_group, verse_win, "Verse template", ids)
    _add_group(windows_file, template_group, point_win, "Point template", ids)
    save("windows", windows_file)

    # a presentation from another ProPresenter (the compatibility check refuses it)
    wrong = presentation_pb2.Presentation()
    wrong.CopyFrom(base)
    wrong.application_info.application_version.minor_version = 1
    save("wrong_version", wrong)
    wrong_doc = presentation_pb2.Presentation()
    wrong_doc.CopyFrom(base)
    wrong_doc.application_info.platform_version.build = "12345"
    save("wrong_document", wrong_doc)

    # no point template: the config says there is one but the cue is gone / the template cue is not in a group / has two slide actions
    nogroup = presentation_pb2.Presentation()
    nogroup.CopyFrom(base)
    del nogroup.cue_groups[:]
    save("no_group", nogroup)
    twoact = presentation_pb2.Presentation()
    twoact.CopyFrom(base)
    extra_action = twoact.cues[0].actions.add()
    extra_action.CopyFrom(twoact.cues[0].actions[0])
    save("two_actions", twoact)
    nopoint = presentation_pb2.Presentation()
    nopoint.CopyFrom(base)
    kept = [c for c in nopoint.cues if c.uuid.string != point_cue.uuid.string]
    del nopoint.cues[:]
    nopoint.cues.extend(kept)
    save("no_point_cue", nopoint)
    twopoint = presentation_pb2.Presentation()
    twopoint.CopyFrom(base)
    for cue in twopoint.cues:
        if cue.uuid.string == point_cue.uuid.string:
            dup = _slide_action(cue).slide.presentation.base_slide.elements.add()
            dup.CopyFrom(_elements(cue)[0])
    save("two_point_texts", twopoint)
    badrtf = presentation_pb2.Presentation()
    badrtf.CopyFrom(base)
    for cue in badrtf.cues:
        for wrapper in _elements(cue):
            if wrapper.element.name == "Verse":
                wrapper.element.text.rtf_data = b"plain text, not rtf"
    save("bad_rtf", badrtf)
    return made


# ------------------------------------------------------------------------------------------------------------ themes folder
def build_theme_folder(repo: Path, out: Path) -> Path:
    root = out / "themes"
    if root.exists():
        shutil.rmtree(root)
    source = repo / "python" / "tests" / "fixtures" / "themes"
    shutil.copytree(source, root)

    def doc_of(name: str):
        doc = template_pb2.Template.Document()
        doc.ParseFromString((source / name / "Theme").read_bytes())
        return doc

    def put(parts: tuple[str, ...], data: bytes) -> None:
        target = root.joinpath(*parts, "Theme")
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(data)

    # the real "The biblke" with its small box moved clear of the big one: a true two-box layout (as tests/test_themes.py does)
    split = doc_of("The biblke")
    box = min((w for w in split.slides[0].base_slide.elements if w.element.HasField("text")), key=lambda w: w.element.bounds.size.height)
    box.element.bounds.origin.y = 800
    put(("Split",), split.SerializeToString())
    # three slides: named, unnamed and named, with a different number of boxes
    multi = doc_of("The biblke")
    first = multi.slides[0]
    first.name = "Verse A"
    second = multi.slides.add()
    second.CopyFrom(first)
    second.name = ""
    third = multi.slides.add()
    third.CopyFrom(doc_of("Sermon Points").slides[0])
    third.name = "Points"
    put(("Multi",), multi.SerializeToString())
    # a theme with a slide that has no text box
    empty = doc_of("The biblke")
    for item in empty.slides:
        kept = [w for w in item.base_slide.elements if not w.element.HasField("text")]
        del item.base_slide.elements[:]
        item.base_slide.elements.extend(kept)
        item.name = "Pictures"
    put(("Pictures",), empty.SerializeToString())
    # grouped themes, one name used in two groups (ambiguous without the group), a file nobody can read, a Theme at the top (ignored)
    put(("Church", "Hymn"), (source / "Sermon Points" / "Theme").read_bytes())
    put(("Church", "Choir"), (source / "The biblke" / "Theme").read_bytes())
    put(("Youth", "Hymn"), (source / "The biblke" / "Theme").read_bytes())
    put(("Church", "Deep", "Nested"), (source / "Scripture main screens" / "Theme").read_bytes())
    put(("Broken",), b"\xff\xff\xff not a theme")
    (root / "Theme").write_bytes((source / "Sermon Points" / "Theme").read_bytes())
    # unusual boxes: an RTF with a font table and size but no attribute font, one with neither
    odd = doc_of("The biblke")
    for wrapper in odd.slides[0].base_slide.elements:
        if wrapper.element.HasField("text"):
            wrapper.element.text.attributes.font.ClearField("name")
            wrapper.element.text.attributes.font.ClearField("size")
    put(("Odd fonts",), odd.SerializeToString())
    return root


def _slide_dict(slide, root: Path) -> dict:
    return {
        "group": slide.group, "theme": slide.theme, "name": slide.name, "number": slide.number,
        "path": Path(slide.path).relative_to(root).as_posix(),
        "boxes": [{"name": b.name, "x": b.x, "y": b.y, "w": b.w, "h": b.h, "font": b.font, "size": b.size} for b in slide.boxes],
        "other_objects": slide.other_objects, "size": list(slide.size),
        "theme_label": slide.theme_label, "label": slide.label, "has_reference_box": slide.has_reference_box, "usable": slide.usable,
    }


FINDS = [
    ("Sermon Points", ""), ("sermon points", ""), ("  SERMON POINTS  ", ""), ("Sermon Points", "1"), ("Sermon Points", "theme slide"),
    ("Sermon Points", "9"), ("Sermon Points", "nope"), ("The biblke", ""), ("Split", "1"), ("Scripture main screens", ""), ("Nope", ""),
    ("", ""), ("Multi", ""), ("Multi", "2"), ("Multi", "points"), ("Multi", "Verse A"), ("Multi", "4"), ("Multi", "0"), ("Pictures", ""),
    ("Pictures", "1"), ("Hymn", ""), ("Church/Hymn", ""), ("church/hymn", "1"), ("Youth/Hymn", ""), ("Nested", ""), ("Church/Deep/Nested", ""),
    ("Deep", ""), ("Choir", ""), ("Broken", ""), ("Odd fonts", ""), ("Theme", ""),
]


def themes_oracle(root: Path) -> dict:
    slides = themes.read_theme_tree(root)
    finds = []
    for theme, slide in FINDS:
        try:
            found = themes.find_slide(slides, theme, slide)
            finds.append({"theme": theme, "slide": slide, "ok": {"label": found.label, "number": found.number, "theme_label": found.theme_label}})
        except themes.ThemeError as exc:
            finds.append({"theme": theme, "slide": slide, "error": str(exc)})
    # what the picker of the web page lists (web/server.py list_looks) without the transport
    rows = [{"key": f"{s.theme_label}::{s.number}", "label": s.label, "group": s.group, "theme": s.theme, "slide": s.name or f"Slide {s.number}", "number": s.number,
             "boxes": len(s.boxes), "reference_box": s.has_reference_box} for s in slides if s.usable]
    return {"slides": [_slide_dict(s, root) for s in slides], "picker": rows, "finds": finds}


# ------------------------------------------------------------------------------------------------------------ plans
def planned(kind: str, body: str, reference: str = "", version: str = "", index: int = 0, keys: list[str] | None = None) -> dict:
    return {"kind": kind, "body_text": body, "reference_text": reference, "version_text": version, "copyright_text": None,
            "source_passage_index": index, "verse_keys": keys or []}


def plan_of(slides: list[dict]) -> dict:
    return {"slides": slides, "attribution_slides": []}


def case_plan(out: Path, name: str) -> dict:
    data = json.loads((out / "cases" / name / "expected" / "plan.json").read_text(encoding="utf-8"))
    return data["slide_plan"]


def basic_plan() -> dict:
    return plan_of([
        planned("point", "1) First point", index=0),
        planned("verse", "For God so loved the world,\nthat he gave his only begotten Son.", "John 3:16", "NIV", 1, ["JHN.3.16"]),
        planned("verse", "That whoever believes in him shall not perish\nbut have eternal life.", "John 3:16", "NIV", 1, ["JHN.3.16"]),
        planned("point", "2) Second point\nin two lines", index=2),
        planned("verse", "And we know that in all things God works for the good.", "Romans 8:28", "", 3, ["ROM.8.28"]),
    ])


def special_plan() -> dict:
    return plan_of([
        planned("verse", "It\u2019s \u201cquoted\u201d \u2014 and \u2018single\u2019 \u2013 dashes\u2026", "Psalm 23:1\u20134", "NKJV", 0, ["PSA.23.1"]),
        planned("verse", "Back\\slash {braces} and }closing{ \\u8217? literal", "John 1:1", "KJV", 1, ["JHN.1.1"]),
        planned("verse", "Caf\u00e9 na\u00efve \u00c5ngstr\u00f6m \u00e9\u00e8\u00ea; \u65e5\u672c\u8a9e; \u0420\u0443\u0441\u0441\u043a\u0438\u0439; \U0001f600 \U0001d11e; \u00a0nbsp; tab\there", "Genesis 1:1", "ESV", 2, ["GEN.1.1"]),
        planned("point", "\u2022 Bullet \u2014 with a long dash\nsecond line with \u2019 and \u201c", index=3),
        planned("verse", "Line one\nLine two\nLine three\n\nAfter a blank line\n", "Mark 1:1", "NIV", 4, ["MRK.1.1"]),
        planned("verse", "x" * 400, "Mark 1:2", "NIV", 5, ["MRK.1.2"]),            # long: the formatting range of the sample text is clipped / extended
        planned("verse", "Hi", "Mark 1:3", "NIV", 6, ["MRK.1.3"]),                 # short
    ])


def empty_body_plan() -> dict:
    return plan_of([
        planned("verse", "", "Luke 1:1", "NIV", 0, ["LUK.1.1"]),
        planned("point", "", index=1),
        planned("verse", "", "", "", 2, []),
        planned("point", "x", index=3),
    ])


def many_plan(count: int = 200) -> dict:
    slides = []
    for i in range(count):
        passage = i // 2
        if i % 5 == 4:
            slides.append(planned("point", f"{i // 5 + 1}) Point number {i}\nsecond line {i}", index=passage))
        else:
            slides.append(planned("verse", f"Synthetic text of slide {i}, with a comma, and a period.", f"Book {passage % 66 + 1}:{i % 30 + 1}", "NIV", passage, [f"BK{i}"]))
    return plan_of(slides)


# ------------------------------------------------------------------------------------------------------------ the cases
def _definition(path: Path, **overrides) -> dict:
    message = _read(path)
    cand = inspect_template_candidates(path)[0]
    points = inspect_point_candidates(path)
    definition = create_template_definition(
        path, presentation_uuid=message.uuid.string, presentation_name=message.name, library_name="Verses", cue_uuid=cand.cue_uuid,
    )
    data = definition.model_dump(mode="json")
    if points:
        data["point_cue_uuid"] = points[0][1]
        data["point_style_fingerprint"] = points[0][2]
    data.update(overrides)
    return data


def _call(case: dict, work: Path, files: dict[str, Path], plan_json: dict, options: dict, looks):
    """Run one case against a throw-away copy of its files. Returns (bytes | None, result dict | None, error | None)."""
    mode = options["mode"]
    definition = PresentationTemplateConfig.model_validate(options["definition"])
    plan = SlidePlan.model_validate(plan_json)
    plan.looks = looks
    writer = ProtobufPresentationWriter()
    base_name = options.get("base")
    base_src = _resolve(files, base_name) if base_name else None
    template_src = _resolve(files, options["template"]) if options.get("template") else None
    local_base = work / "base.pro"
    if base_src:
        shutil.copyfile(base_src, local_base)
    local_template = None
    if template_src:
        local_template = work / "template.pro"
        shutil.copyfile(template_src, local_template)
    out_path = work / "out.pro"
    with frozen(options["uuid_seed"]):
        if mode == "append":
            result = writer.append_plan(local_base, plan, definition, template_path=local_template, remove_cue_uuids=options.get("remove") or None)
            produced = local_base
        elif mode == "replace":
            result = writer.replace_generated(local_base, plan, definition, template_path=local_template)
            produced = local_base
        elif mode == "derive":
            result = writer.derive_presentation(local_base, out_path, plan, definition, name=options["name"], clear_generated=options["clear_generated"],
                                                template_path=local_template, keep_uuid=options["keep_uuid"])
            produced = out_path
        elif mode == "create":
            result = writer.create_presentation(local_base, out_path, plan, definition, name=options["name"])
            produced = out_path
        elif mode == "template_file":
            result = writer.create_template_file(local_base, out_path, definition, name=options["name"])
            produced = out_path
        elif mode == "synthetic":
            written = writer.append_synthetic_slide(local_base, options["body"], options["label"])
            result = written
            produced = local_base
        else:
            raise ValueError(mode)
    dump = result.model_dump(mode="json")
    for key in ("path", "backup_path"):
        dump.pop(key, None)
    return produced.read_bytes(), dump


def _resolve(files: dict[str, Path], reference: str) -> Path:
    if reference.startswith("fixture:"):
        return Path(__file__).resolve().parents[1] / "tests" / "fixtures" / reference[len("fixture:"):]
    return files[reference.removeprefix("files/").removesuffix(".pro")]


def _validate(files: dict[str, Path], options: dict) -> str:
    definition = PresentationTemplateConfig.model_validate(options["definition"])
    target = options.get("template") or options.get("base")
    try:
        validate_template(_resolve(files, target), definition)
        return "ok"
    except Exception as exc:                   # noqa: BLE001
        return f"{type(exc).__name__}: {exc}"


def pro_oracle(out: Path, repo: Path) -> int:
    """Write ``out/pro``. Returns the number of cases."""
    root = out / "pro"
    if root.exists():
        shutil.rmtree(root)
    root.mkdir(parents=True)
    files = build_sample_files(repo, root)
    theme_root = build_theme_folder(repo, root)
    (root / "themes.json").write_text(dumps(themes_oracle(theme_root)), encoding="utf-8")
    sample = "fixture:sample/Sample.pro"
    sample_path = repo / "python" / "tests" / "fixtures" / "sample" / "Sample.pro"
    windows_def = _definition(files["windows"])
    sample_def = _definition(sample_path)
    templates_def = _definition(files["templates"])
    service_def = _definition(files["service"])
    candidates = {}
    for name, path in [("sample", sample_path)] + sorted(files.items()):
        try:
            candidates[name] = {"templates": [c.model_dump(mode="json") for c in inspect_template_candidates(path)],
                                "points": [list(p) for p in inspect_point_candidates(path)]}
        except Exception as exc:               # noqa: BLE001
            candidates[name] = {"error": f"{type(exc).__name__}: {exc}"}
    (root / "candidates.json").write_text(dumps(candidates), encoding="utf-8")

    every = {"verse": "Split", "point": "Sermon Points"}
    cases: list[tuple[str, dict, dict]] = []

    def add(case: str, mode: str, plan: dict, base: str | None, *, definition: dict | None = None, template: str | None = None, looks: dict | None = None, **extra) -> None:
        options: dict[str, Any] = {"mode": mode, "uuid_seed": case, "now": NOW, "definition": definition or sample_def}
        if base:
            options["base"] = base
        if template:
            options["template"] = template
        if looks:
            options["looks"] = {"themes": "themes", **looks}
        options.update(extra)
        cases.append((case, options, plan))

    basic = basic_plan()
    whitefield = case_plan(out, "whitefield-defaults")
    # Append and Replace on the template file itself / in a service
    add("append-sample", "append", basic, sample)
    add("append-templates-file", "append", basic, "files/templates", definition=templates_def)
    add("append-empty-presentation-template-elsewhere", "append", basic, "files/empty", definition=service_def, template="files/service")
    add("append-template-in-another-file", "append", basic, "files/plain", definition=service_def, template="files/service")
    add("append-remove-given-cues", "append", basic, "files/service", definition=service_def, remove=[])      # filled in below
    add("replace-service", "replace", basic, "files/service", definition=service_def)
    add("replace-shared-group", "replace", basic, "files/service_shared_group", definition=service_def)
    add("replace-template-in-another-file", "replace", basic, "files/plain", definition=service_def, template="files/service")
    add("replace-whitefield-notes", "replace", whitefield, "files/service", definition=service_def)
    add("replace-points-only", "replace", plan_of([s for s in whitefield["slides"] if s["kind"] == "point"][:6]), "files/service", definition=service_def)
    add("append-verses-only", "append", plan_of([s for s in basic["slides"] if s["kind"] == "verse"]), "files/service", definition=service_def)
    # real plans from the planner: long verses cut into several slides, points limited to two lines
    add("append-long-verse-split", "append", case_plan(out, "long-verse-split"), "files/service", definition=service_def)
    add("append-points-two-lines", "append", case_plan(out, "whitefield-max-lines-per-slide-2"), "files/service", definition=service_def)
    add("append-kitchen-sink", "append", case_plan(out, "kitchen-sink-defaults"), sample)
    add("append-no-fetch-placeholders", "append", case_plan(out, "whitefield-no-fetch"), sample)
    # text
    add("append-special-characters", "append", special_plan(), sample)
    add("append-empty-bodies", "append", empty_body_plan(), sample)
    add("append-200-slides", "append", many_plan(200), sample)
    add("replace-200-slides", "replace", many_plan(200), "files/service", definition=service_def)
    # new files: fresh id / keep id, replace / append, create, template file, synthetic
    add("derive-replace-fresh-id", "derive", basic, "files/service", definition=service_def, name="Sunday 2", clear_generated=True, keep_uuid=False)
    add("derive-replace-keep-id", "derive", basic, "files/service", definition=service_def, name="Sunday", clear_generated=True, keep_uuid=True)
    add("derive-append-fresh-id", "derive", basic, "files/service", definition=service_def, name="Sunday 3", clear_generated=False, keep_uuid=False)
    add("derive-append-keep-id", "derive", basic, "files/service", definition=service_def, name="Sunday 4", clear_generated=False, keep_uuid=True)
    add("derive-template-in-another-file", "derive", basic, "files/plain", definition=service_def, template="files/service", name="Plain 2", clear_generated=True, keep_uuid=False)
    add("derive-whitefield", "derive", whitefield, "files/service", definition=service_def, name="Whitefield", clear_generated=True, keep_uuid=True)
    add("create-from-templates-file", "create", basic, "files/templates", definition=templates_def, name="New sermon")
    add("create-from-sample", "create", whitefield, sample, name="Whitefield notes")
    add("template-file-from-service", "template_file", plan_of([]), "files/service", definition=service_def, name="Templates only")
    add("template-file-from-sample", "template_file", plan_of([]), sample, name="Templates")
    add("synthetic-slide", "synthetic", plan_of([]), "fixture:propresenter19/Verses.original.pro", body="Synthetic feasibility text \u2019 \u2014 {x}", label="Feasibility")
    add("synthetic-slide-golden-text", "synthetic", plan_of([]), "fixture:propresenter19/Verses.original.pro", body="Hello", label="VerseBridge Test")
    add("error-synthetic-two-text-objects", "synthetic", plan_of([]), sample, body="x", label="y")
    # the Windows document version and the quirks of real files
    add("append-windows-file", "append", basic, "files/windows", definition=windows_def)
    add("create-windows-file", "create", special_plan(), "files/windows", definition=windows_def, name="Windows new")
    add("append-negative-zero-and-unknown-fields", "append", basic, "files/quirks", definition=_definition(files["quirks"]))
    add("derive-negative-zero-and-unknown-fields", "derive", basic, "files/quirks", definition=_definition(files["quirks"]), name="Quirks 2", clear_generated=False, keep_uuid=True)
    # themes (looks)
    two_box = {"verse_theme": "Split", "verse_slide": "", "point_theme": "Sermon Points", "point_slide": ""}
    add("themes-two-box-verse-and-points", "append", basic, "files/service", definition=service_def, looks=two_box)
    add("themes-one-box-verse", "append", basic, "files/service", definition=service_def, looks={"verse_theme": "Scripture main screens", "verse_slide": ""})
    add("themes-points-only", "append", basic, "files/service", definition=service_def, looks={"point_theme": "Sermon Points", "point_slide": "1"})
    add("themes-overlapping-second-box", "append", basic, "files/service", definition=service_def, looks={"verse_theme": "The biblke", "verse_slide": "", "point_theme": "Church/Hymn", "point_slide": ""})
    add("themes-replace-recognises-themed-slides", "replace", basic, "files/service", definition=service_def, looks=two_box)
    add("themes-derive-keep-id", "derive", whitefield, "files/service", definition=service_def, looks=two_box, name="Themed", clear_generated=True, keep_uuid=True)
    add("themes-create", "create", special_plan(), sample, looks={"verse_theme": "Multi", "verse_slide": "Verse A", "point_theme": "Multi", "point_slide": "Points"})
    add("themes-odd-fonts", "append", basic, "files/service", definition=service_def, looks={"verse_theme": "Odd fonts", "verse_slide": "", "point_theme": "Sermon Points", "point_slide": ""})
    add("themes-empty-bodies-and-special-characters", "append", special_plan(), "files/service", definition=service_def, looks=two_box)
    # errors
    add("error-empty-plan", "append", plan_of([]), sample)
    add("error-wrong-application-version", "append", basic, "files/wrong_version")
    add("error-wrong-document-version", "append", basic, "files/wrong_document")
    add("error-template-in-another-wrong-version", "append", basic, sample, template="files/wrong_version")
    add("error-template-cue-gone", "append", basic, "files/plain", definition=service_def)
    add("error-template-not-in-a-group", "append", basic, "files/no_group")
    add("error-template-has-two-slide-actions", "append", basic, "files/two_actions")
    add("error-no-point-template-configured", "append", basic, sample, definition={**sample_def, "point_cue_uuid": None, "point_style_fingerprint": None})
    add("error-point-cue-gone", "append", basic, "files/no_point_cue")
    add("error-point-template-two-text-objects", "append", basic, "files/two_point_texts")
    add("error-template-rtf-not-rtf", "append", basic, "files/bad_rtf")
    add("error-source-wrong-version-derive", "derive", basic, "files/wrong_version", name="X", clear_generated=True, keep_uuid=False)
    add("error-empty-plan-create", "create", plan_of([]), sample, name="X")
    add("error-template-file-slide-missing", "template_file", plan_of([]), "files/plain", definition=service_def, name="X")
    add("stale-template-fingerprint", "append", basic, sample, definition={**sample_def, "style_fingerprint": "0" * 64})

    service_generated = ProtobufPresentationWriter.generated_cue_uuids(_read(files["service"]), PresentationTemplateConfig.model_validate(service_def))
    work_root = Path(tempfile.mkdtemp(prefix="vv-pro-oracle-"))
    written = 0
    try:
        for name, options, plan_json in cases:
            if name == "append-remove-given-cues":
                options["remove"] = service_generated[:2]
            looks = None
            if "looks" in options:
                spec = options["looks"]
                slides = themes.read_theme_tree(theme_root)
                looks = themes.Looks(
                    verse=themes.find_slide(slides, spec["verse_theme"], spec.get("verse_slide", "")) if spec.get("verse_theme") else None,
                    point=themes.find_slide(slides, spec["point_theme"], spec.get("point_slide", "")) if spec.get("point_theme") else None,
                )
            options.setdefault("name", "")
            work = Path(tempfile.mkdtemp(dir=work_root))
            expected: dict[str, Any] = {"result": None, "error": None, "validate": None}
            expected_bytes: bytes | None = None
            try:
                expected_bytes, expected["result"] = _call({}, work, files, plan_json, options, looks)
            except Exception as exc:              # noqa: BLE001
                expected["error"] = f"{type(exc).__name__}: {exc}"
            if options["mode"] in {"append", "replace", "derive", "create", "template_file"} and options.get("definition"):
                expected["validate"] = _validate(files, options)
            case_dir = root / "cases" / name
            case_dir.mkdir(parents=True)
            (case_dir / "plan.json").write_text(dumps(plan_json), encoding="utf-8")
            (case_dir / "options.toml").write_text(tomli_w.dumps(_toml_safe(options)), encoding="utf-8")
            (case_dir / "expected.json").write_text(dumps(expected), encoding="utf-8")
            if expected_bytes is not None:
                (case_dir / "expected.pro").write_bytes(expected_bytes)
            written += 1
    finally:
        shutil.rmtree(work_root, ignore_errors=True)
    return written


def _toml_safe(options: dict) -> dict:
    """TOML has no null: a key whose value is None is left out (the Rust side reads it as absent)."""
    def clean(value):
        if isinstance(value, dict):
            return {k: clean(v) for k, v in value.items() if v is not None}
        return value
    return clean(options)
