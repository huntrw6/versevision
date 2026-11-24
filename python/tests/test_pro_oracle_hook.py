"""The id hook of the protobuf writer (S6a): `new_uuid` is the only source of new ids, a seeded one makes the output reproducible, and the
default is `uuid.uuid4`, so nothing changes for the app."""
from __future__ import annotations

import sys
import uuid
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "scripts"))

import oracle_pro  # noqa: E402
from versevision.config import PresentationTemplateConfig  # noqa: E402
from versevision.presentation import protobuf_writer  # noqa: E402
from versevision.slides.models import SlidePlan  # noqa: E402

SAMPLE = Path(__file__).parent / "fixtures" / "sample" / "Sample.pro"


def test_the_default_source_is_uuid4():
    value = protobuf_writer.new_uuid()
    assert isinstance(value, uuid.UUID) and value.version == 4
    assert protobuf_writer.new_uuid() != value


def _write(tmp_path: Path, seed: str) -> bytes:
    target = tmp_path / f"{seed}.pro"
    target.write_bytes(SAMPLE.read_bytes())
    definition = PresentationTemplateConfig.model_validate(oracle_pro._definition(SAMPLE))
    plan = SlidePlan.model_validate(oracle_pro.basic_plan())
    with oracle_pro.frozen(seed):
        protobuf_writer.ProtobufPresentationWriter().append_plan(target, plan, definition)
    return target.read_bytes()


def test_a_seeded_source_makes_the_file_reproducible_and_a_free_one_does_not(tmp_path):
    for folder in ("one", "two"):
        (tmp_path / folder).mkdir()
    first, again, other = _write(tmp_path / "one", "a"), _write(tmp_path / "two", "a"), _write(tmp_path / "two", "b")
    assert first == again and first != other
    target = tmp_path / "free.pro"
    target.write_bytes(SAMPLE.read_bytes())
    definition = PresentationTemplateConfig.model_validate(oracle_pro._definition(SAMPLE))
    plan = SlidePlan.model_validate(oracle_pro.basic_plan())
    protobuf_writer.ProtobufPresentationWriter().append_plan(target, plan, definition)
    assert target.read_bytes() != first
