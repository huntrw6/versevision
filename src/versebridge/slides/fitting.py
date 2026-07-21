from __future__ import annotations

import math
import re


_NARROW = set(".,:;!'`|ijlI1()[]")
_WIDE = set("MW@%QOwm")


def text_width_units(value: str) -> float:
    total = 0.0
    for character in value:
        if character.isspace():
            total += 0.52
        elif character in _NARROW:
            total += 0.58
        elif character in _WIDE:
            total += 1.28
        else:
            total += 1.0
    return total


def wrap_body_lines(text: str, line_width_units: float) -> list[str]:
    lines: list[str] = []
    for paragraph in text.splitlines() or [""]:
        words = paragraph.split()
        if not words:
            lines.append("")
            continue
        current = words[0]
        for word in words[1:]:
            candidate = f"{current} {word}"
            if text_width_units(candidate) <= line_width_units:
                current = candidate
            else:
                lines.append(current)
                current = word
        lines.append(current)
    return lines


def _break_priority(text: str, position: int) -> int:
    before = text[:position].rstrip()
    if not before:
        return 0
    if before[-1] in ".?!":
        return 4
    if before[-1] in ";:":
        return 3
    if before[-1] in ",—–-":
        return 2
    return 1


def _candidate_breaks(text: str) -> list[int]:
    return [match.end() for match in re.finditer(r"(?<=[.!?;:,—–-])\s+|\s+", text)]


def split_verse_text(
    text: str,
    *,
    max_lines: int = 5,
    line_width_units: float = 48.0,
    prefer_punctuation: bool = True,
) -> list[str]:
    """Split verse text into balanced, naturally broken, line-limited chunks."""
    remaining = " ".join(text.split())
    if not remaining:
        return [""]
    pieces: list[str] = []
    while len(wrap_body_lines(remaining, line_width_units)) > max_lines:
        line_count = len(wrap_body_lines(remaining, line_width_units))
        slides_left = max(2, math.ceil(line_count / max_lines))
        target = len(remaining) / slides_left
        fitting = [
            position for position in _candidate_breaks(remaining)
            if len(wrap_body_lines(remaining[:position].strip(), line_width_units)) <= max_lines
        ]
        if not fitting:
            # Only an unusually long unbroken token can reach this path.
            position = max(1, int(target))
        else:
            nearby = [position for position in fitting if abs(position - target) <= max(12, target * 0.4)]
            pool = nearby or fitting
            if prefer_punctuation:
                position = max(
                    pool,
                    key=lambda value: (_break_priority(remaining, value), -abs(value - target)),
                )
            else:
                position = min(pool, key=lambda value: abs(value - target))
        pieces.append(remaining[:position].strip())
        remaining = remaining[position:].strip()
    pieces.append(remaining)
    return pieces
