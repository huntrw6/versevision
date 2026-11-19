"""Text that is longer than the template's sample text must stay all caps to the very last word.

ProPresenter keeps "all caps" as a character RANGE (0 to the length of the sample text). Longer text ran past the range, so its last words came out
in lowercase. The ranges must be stretched to the new text."""
from __future__ import annotations

import shutil
from pathlib import Path

from versevision.presentation.generated import presentation_pb2
from versevision.presentation.protobuf_writer import ProtobufPresentationWriter, _set_text, _visible_length
from versevision.slides.models import PlannedSlide, SlidePlan

from test_pipeline import FIXTURE, _config

LONG_VERSE = ("Love is patient and kind; love does not envy or boast; it is not arrogant or rude. It does not insist on its own way; it is not irritable "
              "or resentful; it does not rejoice at wrongdoing, but rejoices with the truth. Love bears all things, believes all things, hopes all things, endures all things.")
LONG_POINT = "7) One Surrendered Life Can Influence Generations Far Beyond What We Ever Imagined Possible Today"


def written(tmp_path: Path):
    template = tmp_path / "Sample.pro"
    shutil.copy(FIXTURE, template)
    config = _config(template)
    plan = SlidePlan(slides=[
        PlannedSlide(kind="verse", body_text=LONG_VERSE, reference_text="1 Corinthians 13:4-7", version_text="NIV", source_passage_index=0),
        PlannedSlide(kind="verse", body_text="Short verse.", reference_text="John 3:16", version_text="NIV", source_passage_index=1),
        PlannedSlide(kind="point", body_text=LONG_POINT, reference_text="", version_text="", source_passage_index=2),
    ])
    out = tmp_path / "Out.pro"
    ProtobufPresentationWriter().create_presentation(template, out, plan, config.presentation_template, name="Out")
    message = presentation_pb2.Presentation()
    message.ParseFromString(out.read_bytes())
    return message


def text_objects(message):
    for cue in message.cues:
        for action in cue.actions:
            if action.WhichOneof("ActionTypeData") == "slide":
                for wrapped in action.slide.presentation.base_slide.elements:
                    if wrapped.element.HasField("text"):
                        yield wrapped.element.name, wrapped.element.text


def test_every_formatting_range_of_generated_text_reaches_its_last_character(tmp_path):
    message = written(tmp_path)
    generated = [(name, text) for name, text in text_objects(message) if _visible_length(text.rtf_data) not in (18, 180, 88)]
    assert len(generated) >= 5
    for name, text in generated:
        length = _visible_length(text.rtf_data)
        ranges = [(c.range.start, c.range.end) for c in text.attributes.custom_attributes]
        assert ranges, f"{name} lost its all-caps range"
        assert all(start == 0 and end == length for start, end in ranges), f"{name}: ranges {ranges} do not cover its {length} characters"


def test_the_long_verse_and_point_are_fully_covered_not_just_the_template_length(tmp_path):
    lengths = {}
    for name, text in text_objects(written(tmp_path)):
        lengths.setdefault(name, []).append((_visible_length(text.rtf_data), [c.range.end for c in text.attributes.custom_attributes]))
    assert (len(LONG_VERSE), [len(LONG_VERSE)]) in lengths["Verse"]            # was stuck at 180
    assert (len(LONG_POINT), [len(LONG_POINT)]) in lengths["Sermon Point"]       # was stuck at 88
    assert (len("1 Corinthians 13:4-7 (NIV)"), [len("1 Corinthians 13:4-7 (NIV)")]) in lengths["Reference"]    # was stuck at 18


def test_a_range_that_stopped_early_is_only_clipped_and_unicode_counts_as_one_character_each(tmp_path):
    message = written(tmp_path)
    text = next(t for name, t in text_objects(message) if name == "Verse")
    _set_text(text, "Don\u2019t \u201cquote\u201d me")
    assert _visible_length(text.rtf_data) == len("Don\u2019t \u201cquote\u201d me")
    assert [c.range.end for c in text.attributes.custom_attributes] == [len("Don\u2019t \u201cquote\u201d me")]
    text.attributes.custom_attributes[0].range.start = 0
    text.attributes.custom_attributes[0].range.end = 3                                  # only the first three characters were formatted
    _set_text(text, "Short")
    assert [(c.range.start, c.range.end) for c in text.attributes.custom_attributes] == [(0, 3)]
