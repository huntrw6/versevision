"""The Config page: every customisation that matters, read from and written to the saved configuration.

The options, their choices, limits and one-line explanations all come from ``versevision.config`` itself (the field types and the
comment next to each field), so a new option shows up on the page as soon as it is added there.
"""
from __future__ import annotations

import ast
import functools
import inspect
import io
import tokenize
import typing
from typing import Any, Literal, get_args, get_origin

from pydantic import ValidationError
from pydantic.fields import FieldInfo

from versevision import config as config_module
from versevision.config import AppConfig, config_path, save_config
from versevision.errors import ConfigurationError

# (section id, title, what it is, [(model on AppConfig, [only these options] or None for all)])
SECTIONS: list[tuple[str, str, str, list[tuple[str, list[str] | None]]]] = [
    ("verse_slides", "Verse slides", "How a verse is cut and laid out on its slides.", [("slides", None)]),
    ("points", "Sermon points", "What counts as a point and how its slides are written.", [("points", None)]),
    ("verses", "Verse versions", "Which Bible version each verse gets.", [("verses", None)]),
    ("notes", "Reading the notes", "How lines of the document are read: titles, cues, wrapped lines, duplicates.", [("notes", None)]),
    ("general", "General", "Defaults for every import.", [("bible", ["default_version"]), ("output", ["library"]), ("presentation", ["backup_count"])]),
]
HIDDEN = {"slides.max_characters", "slides.max_verses_per_slide", "slides.preserve_poetry_lines", "slides.reference_format", "slides.break_on_new_verse",
          "verses.theme", "verses.theme_slide", "points.theme", "points.theme_slide", "output.swap_keeps_id"}     # not used, or set with the look picker
LABELS = {
    "slides.max_body_lines": "Most lines of verse text on a slide",
    "slides.max_reference_lines": "Most lines for the reference",
    "slides.line_width_units": "Verse line width",
    "slides.reference_width_units": "Reference line width",
    "slides.include_verse_numbers": "Show verse numbers in the text",
    "slides.reference_label_mode": "Reference shows",
    "slides.repeat_reference_on_continuation": "Repeat the reference on every slide of a long verse",
    "slides.prefer_punctuation_breaks": "Prefer to cut a long verse at punctuation",
    "slides.even_split_tolerance": "Uneven split allowed (mid-phrase)",
    "slides.even_split_punctuation_tolerance": "Uneven split allowed (at punctuation)",
    "points.max_lines_per_slide": "Most lines on a sermon point slide (0 = no limit)",
    "points.max_lines": "Lines a point box holds at full size",
    "points.line_width_units": "Point line width",
    "points.line_tolerance": "Uneven lines allowed",
    "points.line_tolerance_at_punctuation": "Uneven lines allowed (at punctuation)",
    "points.strip_trailing_period": "Remove the period at the end of a point",
    "points.fix_label_typos": "Correct a header written unlike the others (KEY#1 vs KEY #1)",
    "points.balance_lines": "Break long points into evenly wide lines",
    "points.bullet_char": "Bullet mark",
    "points.shown_in_capitals": "Points are shown in capitals",
    "notes.line_breaks": "Each line break is",
    "bible.default_version": "Default Bible version",
    "output.library": "Library for new presentations",
    "presentation.backup_count": "Backups to keep",
}


@functools.lru_cache(maxsize=1)
def _help() -> dict[tuple[str, str], str]:
    """The comment on each config field (and the string under it, if any), keyed by (class name, field name)."""
    try:
        source = inspect.getsource(config_module)
        comments = {tok.start[0]: tok.string.lstrip("#").strip() for tok in tokenize.generate_tokens(io.StringIO(source).readline) if tok.type == tokenize.COMMENT}
        found: dict[tuple[str, str], str] = {}
        for cls in ast.parse(source).body:
            if not isinstance(cls, ast.ClassDef):
                continue
            last = None
            for stmt in cls.body:
                if isinstance(stmt, ast.AnnAssign) and isinstance(stmt.target, ast.Name):
                    last = (cls.name, stmt.target.id)
                    found[last] = comments.get(stmt.end_lineno) or comments.get(stmt.lineno) or ""
                elif isinstance(stmt, ast.Expr) and isinstance(getattr(stmt, "value", None), ast.Constant) and isinstance(stmt.value.value, str) and last:
                    found[last] = (found[last] + " " + " ".join(stmt.value.value.split())).strip()
                    last = None
        return found
    except Exception:
        return {}


def _bounds(info: FieldInfo) -> dict[str, float]:
    out: dict[str, float] = {}
    for meta in info.metadata:
        for key in ("ge", "gt", "le", "lt"):
            if hasattr(meta, key) and getattr(meta, key) is not None:
                out[{"ge": "min", "gt": "min", "le": "max", "lt": "max"}[key]] = getattr(meta, key)
    return out


def _kind(info: FieldInfo) -> tuple[str, list[str]]:
    ann = info.annotation
    origin = get_origin(ann)
    if origin is typing.Union or str(origin) == "<class 'types.UnionType'>":
        args = [a for a in get_args(ann) if a is not type(None)]
        ann = args[0] if len(args) == 1 else ann
        origin = get_origin(ann)
    if origin is Literal:
        return "choice", [str(a) for a in get_args(ann)]
    if ann is bool:
        return "bool", []
    if ann is int:
        return "int", []
    if ann is float:
        return "float", []
    if ann is str:
        return "text", []
    if origin is list:
        return "list", []
    return "", []


def _humanize(name: str) -> str:
    return name.replace("_", " ").capitalize()


def _editable() -> dict[str, tuple[str, str, FieldInfo, str, list[str]]]:
    """key -> (section id, model attr, field info, kind, choices) for every option the page can change."""
    out = {}
    for sid, _title, _help_text, parts in SECTIONS:
        for attr, only in parts:
            model = getattr(AppConfig(), attr)
            for name, info in type(model).model_fields.items():
                key = f"{attr}.{name}"
                kind, choices = _kind(info)
                if kind and key not in HIDDEN and (only is None or name in only):
                    out[key] = (sid, attr, info, kind, choices)
    return out


def describe(config: AppConfig) -> dict[str, Any]:
    helps = _help()
    defaults = AppConfig()
    sections = []
    editable = _editable()
    for sid, title, text, parts in SECTIONS:
        fields = []
        for key, (fsid, attr, info, kind, choices) in editable.items():
            if fsid != sid:
                continue
            name = key.split(".", 1)[1]
            model, default_model = getattr(config, attr), getattr(defaults, attr)
            fields.append({
                "key": key, "label": LABELS.get(key) or _humanize(name), "kind": kind, "choices": choices,
                "value": getattr(model, name), "default": getattr(default_model, name),
                "help": helps.get((type(model).__name__, name), ""), **_bounds(info),
            })
        sections.append({"id": sid, "title": title, "help": text, "fields": fields})
    return {"sections": sections, "path": str(config_path())}


def apply(config: AppConfig, values: dict[str, Any]) -> AppConfig:
    """The saved configuration with these option values changed. Unknown or non-editable options are refused."""
    editable = _editable()
    data = config.model_dump(mode="json")
    for key, value in values.items():
        if key not in editable:
            raise ConfigurationError(f"{key} cannot be changed here")
        _sid, attr, _info, kind, choices = editable[key]
        name = key.split(".", 1)[1]
        if kind == "list":
            value = [str(v).strip() for v in (value if isinstance(value, list) else str(value).splitlines()) if str(v).strip()]
        elif kind in ("text", "choice"):
            value = None if (value is None or str(value).strip() == "") and key == "bible.default_version" else str(value)
        elif kind == "bool":
            value = bool(value)
        elif kind == "int":
            value = int(value)
        elif kind == "float":
            value = float(value)
        data[attr][name] = value
    try:
        new = AppConfig.model_validate(data)
    except ValidationError as exc:
        err = exc.errors()[0]
        where = ".".join(str(p) for p in err["loc"])
        raise ConfigurationError(f"{where}: {err['msg']}") from exc
    save_config(new, overwrite=True)
    return new
