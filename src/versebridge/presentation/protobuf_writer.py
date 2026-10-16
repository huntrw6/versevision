from __future__ import annotations

from copy import deepcopy
import hashlib
import os
from pathlib import Path
import re
import sys
import tempfile
import uuid

from google.protobuf.message import Message
from filelock import FileLock

from versebridge.config import PresentationTemplateConfig
from versebridge.slides.models import SlidePlan
from .backup import create_backup
from .base import PlanWriteResult, PresentationInfo, VerificationResult, WriteResult
from .compatibility import CompatibilityPolicy


_GENERATED = Path(__file__).with_name("generated")
if str(_GENERATED) not in sys.path:
    sys.path.insert(0, str(_GENERATED))

import presentation_pb2  # type: ignore[import-not-found]  # generated binding


_NIL_UUID = "00000000-0000-0000-0000-000000000000"


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def _escape_rtf(text: str) -> bytes:
    pieces: list[str] = []
    for character in text:
        if character in "\\{}":
            pieces.append("\\" + character)
        elif character == "\n":
            pieces.append("\\line ")
        elif ord(character) > 127:
            value = ord(character)
            if value > 32767:
                value -= 65536
            pieces.append(f"\\u{value}?")
        else:
            pieces.append(character)
    return "".join(pieces).encode("ascii")


_HEADER_GROUPS = re.compile(rb"\{\\(?:fonttbl|colortbl|\*\\expandedcolortbl|stylesheet|\*\\listtable)[^{}]*(?:\{[^{}]*\}[^{}]*)*\}", re.DOTALL)
_CONTROL = re.compile(rb"\\(?P<word>[A-Za-z]+)(?P<num>-?\d+)? ?|\\\*|[\r\n]+")


def _text_start(rtf: bytes) -> int:
    """Index where visible text begins: after the RTF header and the last formatting control word."""
    markers = list(re.finditer(rb"\\cb\d+\s?", rtf))
    if markers:                                   # Windows-style output ends its header with \cbN
        return markers[-1].end()
    position = 0
    for group in _HEADER_GROUPS.finditer(rtf):
        position = group.end()
    if not position:
        raise ValueError("template RTF has no recognizable header")
    while position < len(rtf):
        match = _CONTROL.match(rtf, position)
        if not match:
            break
        word = match.group("word")
        if word == b"u" and match.group("num"):    # \u8217? is text, not formatting
            break
        position = match.end()
    return position


def _replace_rtf_text(rtf: bytes, text: str) -> bytes:
    if not rtf.startswith(b"{\\rtf") or not rtf.rstrip().endswith(b"}"):
        raise ValueError("template text element does not contain supported RTF")
    trimmed = rtf.rstrip()
    prefix = trimmed[:_text_start(trimmed)]
    if not prefix.endswith((b" ", b"\n", b"\r", b"\t")):
        prefix += b" "
    return prefix + _escape_rtf(text) + b"}"


def _refresh_uuids(message: Message) -> set[str]:
    generated: set[str] = set()
    for field, value in message.ListFields():
        if field.message_type and field.message_type.full_name == "rv.data.UUID":
            items = value if field.is_repeated else [value]
            for item in items:
                if item.string and item.string != _NIL_UUID:
                    item.string = str(uuid.uuid4())
                    generated.add(item.string)
        elif field.message_type:
            items = value if field.is_repeated else [value]
            for item in items:
                generated.update(_refresh_uuids(item))
    return generated


def _all_uuids(message: Message) -> list[str]:
    found: list[str] = []
    for field, value in message.ListFields():
        if field.message_type and field.message_type.full_name == "rv.data.UUID":
            items = value if field.is_repeated else [value]
            found.extend(item.string for item in items if item.string and item.string != _NIL_UUID)
        elif field.message_type:
            items = value if field.is_repeated else [value]
            for item in items:
                found.extend(_all_uuids(item))
    return found


class ProtobufPresentationWriter:
    def __init__(self, policy: CompatibilityPolicy | None = None):
        self.policy = policy or CompatibilityPolicy()

    @staticmethod
    def _read(path: Path):
        message = presentation_pb2.Presentation()
        message.ParseFromString(path.read_bytes())
        return message

    def inspect(self, path: Path) -> PresentationInfo:
        message = self._read(path)
        app = message.application_info.application_version
        document = message.application_info.platform_version
        return PresentationInfo(
            path=path,
            name=message.name,
            uuid=message.uuid.string,
            cue_count=len(message.cues),
            group_count=len(message.cue_groups),
            application_version=f"{app.major_version}.{app.minor_version}.{app.patch_version} ({app.build})",
            document_version=f"{document.major_version}/{document.build}",
        )

    @staticmethod
    def _cue_and_group(message, cue_uuid: str):
        cue = next((item for item in message.cues if item.uuid.string.casefold() == cue_uuid.casefold()), None)
        if cue is None:
            raise ValueError("configured template cue no longer exists")
        actions = [action for action in cue.actions if action.WhichOneof("ActionTypeData") == "slide"]
        if len(actions) != 1:
            raise ValueError("template cue must contain exactly one slide action")
        group = next((
            item for item in message.cue_groups
            if any(identifier.string == cue.uuid.string for identifier in item.cue_identifiers)
        ), None)
        if group is None:
            raise ValueError("template cue is not assigned to a cue group")
        return cue, group, actions[0]

    @staticmethod
    def _template_parts(message, definition: PresentationTemplateConfig):
        cue, group, action = ProtobufPresentationWriter._cue_and_group(message, definition.cue_uuid)
        elements = action.slide.presentation.base_slide.elements
        references = [item for item in elements if item.element.name == definition.reference_object and item.element.HasField("text")]
        bodies = [item for item in elements if item.element.name == definition.body_object and item.element.HasField("text")]
        if len(references) != 1 or len(bodies) != 1:
            raise ValueError("template must contain exactly one configured reference and verse object")
        return cue, group

    def _render_plan(
        self, original, slide_plan: SlidePlan, definition: PresentationTemplateConfig, template_message=None
    ) -> list[str]:
        source = template_message or original
        verse_cue = verse_group = point_cue = point_group = None
        if any(item.kind == "verse" for item in slide_plan.slides):
            verse_cue, verse_group = self._template_parts(source, definition)
        if any(item.kind == "point" for item in slide_plan.slides):
            if not definition.point_cue_uuid:
                raise ValueError("no sermon-point template is configured; run versebridge template select-point")
            point_cue, point_group, point_action = self._cue_and_group(source, definition.point_cue_uuid)
            if len([e for e in point_action.slide.presentation.base_slide.elements
                    if e.element.name == definition.point_object and e.element.HasField("text")]) != 1:
                raise ValueError("sermon-point template must contain exactly one configured point text object")
        existing_uuids = set(_all_uuids(original))
        created: list[str] = []
        grouped: dict[int, list[str]] = {}
        group_labels: dict[int, str] = {}
        group_templates: dict[int, object] = {}
        for planned in slide_plan.slides:
            is_point = planned.kind == "point"
            cue = deepcopy(point_cue if is_point else verse_cue)
            new_uuids = _refresh_uuids(cue)
            if existing_uuids & new_uuids:
                raise ValueError("new cue UUID collides with the presentation")
            existing_uuids.update(new_uuids)
            cue_uuid = cue.uuid.string
            actions = [action for action in cue.actions if action.WhichOneof("ActionTypeData") == "slide"]
            action = actions[0]
            elements = action.slide.presentation.base_slide.elements
            if is_point:
                label = planned.body_text.replace("\n", " ")[:60]
                action.label.text = label
                body = next(item for item in elements if item.element.name == definition.point_object)
                body.element.text.rtf_data = _replace_rtf_text(body.element.text.rtf_data, planned.body_text)
            else:
                label = f"{planned.reference_text} ({planned.version_text})" if planned.version_text else planned.reference_text
                action.label.text = label
                reference = next(item for item in elements if item.element.name == definition.reference_object)
                body = next(item for item in elements if item.element.name == definition.body_object)
                reference.element.text.rtf_data = _replace_rtf_text(reference.element.text.rtf_data, label)
                body.element.text.rtf_data = _replace_rtf_text(body.element.text.rtf_data, planned.body_text)
            original.cues.append(cue)
            created.append(cue_uuid)
            grouped.setdefault(planned.source_passage_index, []).append(cue_uuid)
            group_labels.setdefault(planned.source_passage_index, label)
            group_templates.setdefault(planned.source_passage_index, point_group if is_point else verse_group)

        for passage_index, cue_uuids in grouped.items():
            group = deepcopy(group_templates[passage_index])
            _refresh_uuids(group.group)
            label = group_labels[passage_index]
            group.group.name = label
            group.group.application_group_name = label
            del group.cue_identifiers[:]
            for cue_uuid in cue_uuids:
                group.cue_identifiers.add().string = cue_uuid
            original.cue_groups.append(group)
        return created

    def append_plan(
        self, path: Path, slide_plan: SlidePlan, definition: PresentationTemplateConfig,
        *, template_path: Path | None = None,
    ) -> PlanWriteResult:
        if not slide_plan.slides:
            raise ValueError("slide plan is empty")
        path = path.resolve(strict=True)
        lock = FileLock(str(path) + ".versebridge.lock", timeout=10)
        with lock:
            original_hash = _sha256(path)
            original = self._read(path)
            self.policy.validate(original)
            template_message = original
            if template_path and template_path.resolve() != path:
                template_message = self._read(template_path.resolve(strict=True))
                self.policy.validate(template_message)
            before_cues, before_groups = len(original.cues), len(original.cue_groups)
            cue_uuids = self._render_plan(original, slide_plan, definition, template_message)
            backup = create_backup(path)
            temp_path: Path | None = None
            try:
                with tempfile.NamedTemporaryFile(
                    mode="wb", prefix=f".{path.name}.versebridge-", suffix=".tmp", dir=path.parent, delete=False
                ) as stream:
                    temp_path = Path(stream.name)
                    stream.write(original.SerializeToString(deterministic=True))
                    stream.flush()
                    os.fsync(stream.fileno())
                reparsed = self._read(temp_path)
                if len(reparsed.cues) != before_cues + len(cue_uuids):
                    raise ValueError("temporary presentation cue count differs")
                if len(reparsed.cue_groups) != before_groups + len({item.source_passage_index for item in slide_plan.slides}):
                    raise ValueError("temporary presentation group count differs")
                appended = {item.uuid.string for item in reparsed.cues[-len(cue_uuids):]}
                if appended != set(cue_uuids):
                    raise ValueError("temporary presentation is missing generated cue UUIDs")
                if _sha256(path) != original_hash:
                    raise ValueError("presentation changed while VerseBridge was preparing the import; no files changed")
                os.replace(temp_path, path)
                temp_path = None
            except Exception:
                if temp_path and temp_path.exists():
                    temp_path.unlink()
                raise
            result = PlanWriteResult(
                path=path, backup_path=backup, cue_uuids=cue_uuids,
                cue_count_before=before_cues, cue_count_after=before_cues + len(cue_uuids),
                group_count_before=before_groups,
                group_count_after=before_groups + len({item.source_passage_index for item in slide_plan.slides}),
                output_sha256=_sha256(path), original_sha256=original_hash,
            )
            if _sha256(path) != result.output_sha256:
                os.replace(backup, path)
                raise ValueError("verification failed; rollback completed")
            return result

    def append_synthetic_slide(self, path: Path, body_text: str, label: str) -> WriteResult:
        path = path.resolve(strict=True)
        original = self._read(path)
        self.policy.validate(original)
        if not original.cues or not original.cue_groups:
            raise ValueError("presentation has no cue and group to use as a template")

        before = len(original.cues)
        existing_uuids = set(_all_uuids(original))
        cue = deepcopy(original.cues[0])
        new_uuids = _refresh_uuids(cue)
        if existing_uuids & new_uuids:
            raise ValueError("new cue UUID collides with the presentation")
        cue_uuid = cue.uuid.string
        cue.name = label

        slide_actions = [action for action in cue.actions if action.WhichOneof("ActionTypeData") == "slide"]
        if len(slide_actions) != 1:
            raise ValueError("template cue must contain exactly one slide action")
        elements = slide_actions[0].slide.presentation.base_slide.elements
        text_elements = [element.element.text for element in elements if element.element.HasField("text")]
        if len(text_elements) != 1:
            raise ValueError("feasibility template must contain exactly one text element")
        text_elements[0].rtf_data = _replace_rtf_text(text_elements[0].rtf_data, body_text)

        group = deepcopy(original.cue_groups[0])
        group.group.uuid.string = str(uuid.uuid4())
        if group.group.application_group_identifier.string != _NIL_UUID:
            group.group.application_group_identifier.string = str(uuid.uuid4())
        group.group.name = label
        group.group.application_group_name = label
        del group.cue_identifiers[:]
        identifier = group.cue_identifiers.add()
        identifier.string = cue_uuid

        original.cue_groups.append(group)
        original.cues.append(cue)
        backup = create_backup(path)
        temp_path: Path | None = None
        try:
            with tempfile.NamedTemporaryFile(
                mode="wb", prefix=f".{path.name}.versebridge-", suffix=".tmp", dir=path.parent, delete=False
            ) as stream:
                temp_path = Path(stream.name)
                stream.write(original.SerializeToString(deterministic=True))
                stream.flush()
                os.fsync(stream.fileno())
            reparsed = self._read(temp_path)
            if len(reparsed.cues) != before + 1 or reparsed.cues[-1].uuid.string != cue_uuid:
                raise ValueError("temporary presentation failed structural verification")
            os.replace(temp_path, path)
            temp_path = None
        except Exception:
            if temp_path and temp_path.exists():
                temp_path.unlink()
            raise

        result = WriteResult(
            path=path,
            backup_path=backup,
            cue_uuid=cue_uuid,
            cue_count_before=before,
            cue_count_after=before + 1,
            output_sha256=_sha256(path),
        )
        verification = self.verify(path, result)
        if not verification.valid:
            os.replace(backup, path)
            raise ValueError(f"verification failed; rollback completed: {verification.message}")
        return result

    def verify(self, path: Path, expected: WriteResult) -> VerificationResult:
        try:
            message = self._read(path)
            self.policy.validate(message)
            if len(message.cues) != expected.cue_count_after:
                return VerificationResult(valid=False, message="cue count differs")
            if message.cues[-1].uuid.string != expected.cue_uuid:
                return VerificationResult(valid=False, message="appended cue UUID differs")
            uuids = _all_uuids(message)
            duplicates = {value for value in uuids if uuids.count(value) > 1}
            # Cue identifiers intentionally reference cue UUIDs; other repeats are unsafe.
            permitted = {item.string for group in message.cue_groups for item in group.cue_identifiers}
            if duplicates - permitted:
                return VerificationResult(valid=False, message="unexpected duplicate object UUID")
            if _sha256(path) != expected.output_sha256:
                return VerificationResult(valid=False, message="output hash changed after write")
            return VerificationResult(valid=True, message="presentation reparsed and expected cue is present")
        except Exception as exc:
            return VerificationResult(valid=False, message=str(exc))
