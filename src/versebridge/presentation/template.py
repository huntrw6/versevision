from __future__ import annotations

from copy import deepcopy
import hashlib
from pathlib import Path
import re
import sys

from pydantic import BaseModel

from versebridge.config import PresentationTemplateConfig
from versebridge.errors import ConfigurationError


_GENERATED = Path(__file__).with_name("generated")
if str(_GENERATED) not in sys.path:
    sys.path.insert(0, str(_GENERATED))

import presentation_pb2  # type: ignore[import-not-found]


class TemplateCandidate(BaseModel):
    slide_number: int
    cue_uuid: str
    reference_object_uuid: str
    body_object_uuid: str
    reference_preview: str
    body_preview: str
    style_fingerprint: str


def _read(path: Path):
    message = presentation_pb2.Presentation()
    message.ParseFromString(path.read_bytes())
    return message


def _slide_elements(cue):
    actions = [action for action in cue.actions if action.WhichOneof("ActionTypeData") == "slide"]
    if len(actions) != 1:
        return []
    return list(actions[0].slide.presentation.base_slide.elements)


def _plain_rtf(value: bytes) -> str:
    text = value.decode("ascii", "replace")
    markers = list(re.finditer(r"\\cb\d+\s", text))
    if markers:
        text = text[markers[-1].end():]
    if text.endswith("}"):
        text = text[:-1]
    text = re.sub(r"\\u(-?\d+)\s*\?", lambda match: chr(int(match.group(1)) % 65536), text)
    text = re.sub(r"\\'[0-9a-fA-F]{2}", "", text)
    text = re.sub(r"\\[A-Za-z]+-?\d*\s?", "", text)
    text = text.replace("{", "").replace("}", "")
    return " ".join(text.split())


def _style_fingerprint(reference, body) -> str:
    digest = hashlib.sha256()
    for wrapper in (reference, body):
        clone = deepcopy(wrapper)
        clone.element.text.rtf_data = b""
        digest.update(clone.SerializeToString(deterministic=True))
    return digest.hexdigest()


def inspect_template_candidates(
    path: Path, *, reference_object: str = "Reference", body_object: str = "Verse"
) -> list[TemplateCandidate]:
    presentation = _read(path)
    candidates: list[TemplateCandidate] = []
    for index, cue in enumerate(presentation.cues):
        elements = _slide_elements(cue)
        references = [item for item in elements if item.element.name == reference_object and item.element.HasField("text")]
        bodies = [item for item in elements if item.element.name == body_object and item.element.HasField("text")]
        if len(references) != 1 or len(bodies) != 1:
            continue
        reference, body = references[0], bodies[0]
        candidates.append(TemplateCandidate(
            slide_number=index + 1,
            cue_uuid=cue.uuid.string,
            reference_object_uuid=reference.element.uuid.string,
            body_object_uuid=body.element.uuid.string,
            reference_preview=_plain_rtf(reference.element.text.rtf_data)[:100],
            body_preview=_plain_rtf(body.element.text.rtf_data)[:160],
            style_fingerprint=_style_fingerprint(reference, body),
        ))
    return candidates


def inspect_point_candidates(path: Path, *, point_object: str = "Sermon Point") -> list[tuple[int, str, str]]:
    """Cues with exactly one text element named ``point_object``: (slide number, cue uuid, fingerprint)."""
    found: list[tuple[int, str, str]] = []
    for index, cue in enumerate(_read(path).cues):
        elements = _slide_elements(cue)
        points = [item for item in elements if item.element.name == point_object and item.element.HasField("text")]
        if len(points) != 1 or any(item.element.HasField("text") and item is not points[0] for item in elements):
            continue
        clone = deepcopy(points[0])
        clone.element.text.rtf_data = b""
        found.append((index + 1, cue.uuid.string, hashlib.sha256(clone.SerializeToString(deterministic=True)).hexdigest()))
    return found


def create_template_definition(
    path: Path,
    *,
    presentation_uuid: str,
    presentation_name: str,
    library_name: str | None,
    cue_uuid: str,
    reference_object: str = "Reference",
    body_object: str = "Verse",
) -> PresentationTemplateConfig:
    candidate = next((item for item in inspect_template_candidates(
        path, reference_object=reference_object, body_object=body_object
    ) if item.cue_uuid.casefold() == cue_uuid.casefold()), None)
    if not candidate:
        raise ConfigurationError(f"Cue {cue_uuid} is not a valid {reference_object}/{body_object} template")
    return PresentationTemplateConfig(
        presentation_uuid=presentation_uuid,
        presentation_name=presentation_name,
        library_name=library_name,
        cue_uuid=candidate.cue_uuid,
        reference_object=reference_object,
        body_object=body_object,
        style_fingerprint=candidate.style_fingerprint,
    )


def validate_template(path: Path, definition: PresentationTemplateConfig) -> TemplateCandidate:
    candidate = next((item for item in inspect_template_candidates(
        path, reference_object=definition.reference_object, body_object=definition.body_object
    ) if item.cue_uuid.casefold() == definition.cue_uuid.casefold()), None)
    if not candidate:
        raise ConfigurationError("Configured template cue or named text objects no longer exist")
    if candidate.style_fingerprint != definition.style_fingerprint:
        raise ConfigurationError("Configured template styling or geometry changed; select the template again")
    return candidate
