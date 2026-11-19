"""ProPresenter themes as the look of the imported slides.

ProPresenter keeps every theme it has loaded in ``Documents/ProPresenter/Themes`` (one ``Theme`` file per theme, each holding one or more named
theme slides). A theme slide is a normal slide: text boxes with their position, font, colour and fill, and a background. VerseVision reads those
files and, when you choose a theme for verses or points, builds each slide from that theme slide instead of from the plain template. The slide
structure (labels, groups, the way Replace finds its own slides, undo) is unchanged; only the look comes from the theme.

Which text box is which: a point uses the theme's largest text box. A verse uses the largest box for the verse text and, when the theme slide has
a second text box, the smallest of the others for the reference; a one-box theme gets the reference as the last line of the same box.
"""
from __future__ import annotations

import re
import shutil
import sys
import tarfile
import tempfile
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from versevision.errors import VerseVisionError

_GENERATED = Path(__file__).with_name("presentation") / "generated"
if str(_GENERATED) not in sys.path:
    sys.path.insert(0, str(_GENERATED))

import template_pb2  # type: ignore[import-not-found]  # noqa: E402

MARKER = "VerseVision"          # Cue.name of every slide built from a theme, so Replace can tell them apart from the operator's own slides
EXTRA = "Theme Extra"           # a text box of the theme that is neither verse, reference nor point (its sample text is cleared)


class ThemeError(VerseVisionError):
    pass


@dataclass
class TextBox:
    name: str
    x: float
    y: float
    w: float
    h: float
    font: str = ""
    size: float = 0.0

    @property
    def area(self) -> float:
        return self.w * self.h


@dataclass
class ThemeSlide:
    group: str
    theme: str
    name: str
    number: int                       # 1-based within its theme
    path: str
    boxes: list[TextBox] = field(default_factory=list)
    other_objects: int = 0            # shapes, images, video backgrounds
    size: tuple[int, int] = (1920, 1080)
    base: Any = field(default=None, repr=False, compare=False)       # the rv.data.Slide message

    @property
    def theme_label(self) -> str:
        return f"{self.group}/{self.theme}" if self.group else self.theme

    @property
    def label(self) -> str:
        return f"{self.theme_label} · {self.name or 'Slide ' + str(self.number)}"

    @property
    def usable(self) -> bool:
        return bool(self.boxes)


@dataclass
class Looks:
    verse: ThemeSlide | None = None
    point: ThemeSlide | None = None

    def describe(self) -> dict[str, str]:
        return {"verse": self.verse.label if self.verse else "", "point": self.point.label if self.point else ""}


def _font_of(element) -> tuple[str, float]:
    rtf = element.text.rtf_data
    size = 0.0
    name = ""
    match = re.search(rb"\\fs(\d+)", rtf)
    if match:
        size = int(match.group(1)) / 2
    table = re.search(rb"\{\\fonttbl\s*\{\\f\d+[^;]*?\s([^;{}\\]+);", rtf)
    if table:
        name = table.group(1).decode("latin1").strip()
    try:
        attrs = element.text.attributes
        name = attrs.font.name or name
        size = attrs.font.size or size
    except Exception:
        pass
    return name, size


def read_theme_file(path: Path, group: str, theme: str) -> list[ThemeSlide]:
    document = template_pb2.Template.Document()
    document.ParseFromString(path.read_bytes())
    slides: list[ThemeSlide] = []
    for index, item in enumerate(document.slides, 1):
        base = item.base_slide
        boxes, others = [], 0
        for wrapped in base.elements:
            el = wrapped.element
            if el.HasField("text"):
                font, size = _font_of(el)
                boxes.append(TextBox(el.name, el.bounds.origin.x, el.bounds.origin.y, el.bounds.size.width, el.bounds.size.height, font, size))
            else:
                others += 1
        slides.append(ThemeSlide(group, theme, item.name, index, str(path), boxes, others, (int(base.size.width) or 1920, int(base.size.height) or 1080), base))
    return slides


def read_theme_tree(root: Path) -> list[ThemeSlide]:
    """Every theme slide under a copy of ProPresenter's Themes folder (Group/Theme/Theme, or Theme/Theme)."""
    found: list[ThemeSlide] = []
    for file in sorted(root.rglob("Theme")):
        if not file.is_file():
            continue
        parts = file.relative_to(root).parts[:-1]
        if not parts:
            continue
        group, theme = ("/".join(parts[:-1]), parts[-1])
        try:
            found.extend(read_theme_file(file, group, theme))
        except Exception:                         # a theme file this version cannot read is simply not offered
            continue
    return found


def fetch_themes(transport, workdir: Path | None = None) -> list[ThemeSlide]:
    """Copy the node's Themes folder (just the Theme files) and read it."""
    own = workdir is None
    folder = Path(tempfile.mkdtemp(prefix="versevision-themes-")) if own else workdir
    try:
        transport.fetch_themes(folder)
        return read_theme_tree(folder)
    finally:
        if own:
            shutil.rmtree(folder, ignore_errors=True)


def unpack_themes(archive: Path, dest: Path) -> None:
    with tarfile.open(archive) as tar:
        for member in tar.getmembers():
            target = (dest / member.name).resolve()
            if member.isfile() and str(target).startswith(str(dest.resolve())) and member.name.endswith("Theme"):
                target.parent.mkdir(parents=True, exist_ok=True)
                with tar.extractfile(member) as source:      # type: ignore[union-attr]
                    target.write_bytes(source.read())


def find_slide(slides: list[ThemeSlide], theme: str, slide: str = "") -> ThemeSlide:
    """The theme slide named by config. theme is "Name" or "Group/Name" (any case); slide is a name, a 1-based number, or empty (the first)."""
    wanted = theme.strip().casefold()
    pool = [s for s in slides if wanted in (s.theme.casefold(), s.theme_label.casefold())]
    if not pool:
        names = sorted({s.theme_label for s in slides})
        raise ThemeError(f"No ProPresenter theme named '{theme}'. Loaded themes: {', '.join(names) or 'none found'}")
    labels = {s.theme_label for s in pool}
    if len(labels) > 1:
        raise ThemeError(f"'{theme}' matches several themes ({', '.join(sorted(labels))}); use Group/Name")
    spec = slide.strip()
    if not spec:
        usable = [s for s in pool if s.usable]
        if not usable:
            raise ThemeError(f"Theme '{pool[0].theme_label}' has no slide with a text box")
        return usable[0]
    chosen = next((s for s in pool if s.number == int(spec)), None) if spec.isdigit() else next((s for s in pool if s.name.casefold() == spec.casefold()), None)
    if chosen is None:
        raise ThemeError(f"Theme '{pool[0].theme_label}' has no slide '{spec}'. Its slides: " + ", ".join(f"{s.number}={s.name or '(unnamed)'}" for s in pool))
    if not chosen.usable:
        raise ThemeError(f"Slide '{chosen.label}' has no text box, so it cannot show a verse or point")
    return chosen


def resolve_looks(config, transport) -> Looks | None:
    """The theme slides chosen in the config (verses.theme / points.theme), or None when neither is set."""
    verse_theme, point_theme = config.verses.theme.strip(), config.points.theme.strip()
    if not (verse_theme or point_theme):
        return None
    slides = fetch_themes(transport)
    return Looks(
        verse=find_slide(slides, verse_theme, config.verses.theme_slide) if verse_theme else None,
        point=find_slide(slides, point_theme, config.points.theme_slide) if point_theme else None,
    )


def apply_look(slide_action, look: ThemeSlide, kind: str, *, reference: str, body: str, point: str) -> bool:
    """Replace the look of one generated slide with the theme slide's. Returns True when the slide has a separate reference box."""
    base = slide_action.slide.presentation.base_slide
    base.CopyFrom(look.base)
    texts = [e for e in base.elements if e.element.HasField("text")]
    if not texts:
        raise ThemeError(f"Theme slide '{look.label}' has no text box")
    area = lambda e: e.element.bounds.size.width * e.element.bounds.size.height      # noqa: E731
    ranked = sorted(texts, key=area, reverse=True)
    main, others = ranked[0], ranked[1:]
    if kind == "point":
        main.element.name = point
        reference_box = None
    else:
        main.element.name = body
        reference_box = min(others, key=area) if others else None
        if reference_box is not None:
            reference_box.element.name = reference
    for extra in others:
        if extra is not reference_box:
            extra.element.name = EXTRA
    return reference_box is not None
