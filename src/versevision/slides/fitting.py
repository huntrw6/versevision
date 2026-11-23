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
    tolerance: float = 0.15,
) -> list[str]:
    """Split verse text into pieces of nearly equal length, each within ``max_lines`` lines, breaking at punctuation where that costs little.

    The number of slides is the fewest that fit. The cut points are then chosen together (not one after another) so every slide
    carries about the same amount of text; a break after a full stop, semicolon or comma is preferred when it moves the cut only
    a little."""
    flat = " ".join(text.split())
    if not flat:
        return [""]
    if len(wrap_body_lines(flat, line_width_units)) <= max_lines:
        return [flat]
    words = flat.split(" ")
    count = len(words)
    prefix = [0.0]
    for word in words:
        prefix.append(prefix[-1] + text_width_units(word) + text_width_units(" "))

    def width(a: int, b: int) -> float:
        return prefix[b] - prefix[a] - text_width_units(" ")

    from functools import lru_cache

    @lru_cache(maxsize=None)
    def fits(a: int, b: int) -> bool:
        return len(wrap_body_lines(" ".join(words[a:b]), line_width_units)) <= max_lines

    pieces_needed = max(2, math.ceil(len(wrap_body_lines(flat, line_width_units)) / max_lines))
    for tolerance in (tolerance, max(tolerance, 0.35), 10.0):          # no piece further than this from the average length; relaxed only if no such cut exists
        found = _even_cuts(words, prefix, pieces_needed, fits, prefer_punctuation, tolerance)
        if found:
            return found
    return [flat]


def _even_cuts(words, prefix, pieces_needed, fits, prefer_punctuation, tolerance) -> list[str] | None:
    count = len(words)
    space = text_width_units(" ")

    def width(a: int, b: int) -> float:
        return prefix[b] - prefix[a] - space

    for k in range(pieces_needed, min(count, pieces_needed + 1) + 1):
        mean = prefix[count] / k
        inf = float("inf")
        best = [[inf] * (count + 1) for _ in range(k + 1)]
        back = [[0] * (count + 1) for _ in range(k + 1)]
        best[0][0] = 0.0
        for j in range(1, k + 1):
            for b in range(j, count + 1):
                for a in range(j - 1, b):
                    if best[j - 1][a] == inf or not fits(a, b) or abs(width(a, b) - mean) > tolerance * mean:
                        continue
                    bonus = 0.0
                    if prefer_punctuation and b < count:
                        bonus = 0.3 * mean * mean * (_break_priority(" ".join(words[:b]) + " ", len(" ".join(words[:b])) + 1) - 1) / 3
                    cost = best[j - 1][a] + (width(a, b) - mean) ** 2 - bonus
                    if cost < best[j][b]:
                        best[j][b], back[j][b] = cost, a
        if best[k][count] < inf:
            cuts, b = [], count
            for j in range(k, 0, -1):
                a = back[j][b]
                cuts.append((a, b)); b = a
            cuts.reverse()
            return [" ".join(words[a:b]) for a, b in cuts]
    return None


_WEAK_END = {"a", "an", "the", "of", "to", "in", "on", "at", "it", "is", "as", "by", "or", "and", "for", "but", "—", "–", "-", "&"}


def balance_lines(text: str, line_width_units: float, *, upper: bool = True, glue_first: int = 0, max_lines: int = 4) -> str:
    """Break text into the fewest lines that fit, then spread the words so the lines are as even in width as possible.

    Greedy wrapping leaves a short last line ("RIGHTEOUSNESS." alone). Here the number of lines is the greedy count, but the break
    points are chosen to minimise how far each line is from the average width. ``upper`` measures the text as capitals (the look
    shows points in capitals, which are wider). ``glue_first`` keeps that many leading words together (a bullet or "KEY #2)" with
    the word after it). ``max_lines`` is how many lines the text box holds at full size: ProPresenter shrinks longer text to fit, so
    a point that needs more lines than that gets wider lines (the shrunk letters are narrower). Lines already in the text (a newline)
    are kept and each is balanced on its own."""
    return "\n".join(_balance_one(line, line_width_units, upper, glue_first if i == 0 else 0, max_lines) for i, line in enumerate(text.split("\n")))


def _balance_one(line: str, limit: float, upper: bool, glue_first: int, max_lines: int = 4) -> str:
    words = line.split()
    if len(words) < 2:
        return line.strip()
    shown = [w.upper() if upper else w for w in words]
    space = text_width_units(" ")
    widths = [text_width_units(w) for w in shown]
    total = sum(widths) + space * (len(words) - 1)
    if total <= limit:
        return " ".join(words)
    units: list[tuple[int, int]] = []          # words that must stay together count as one unit
    start = 0
    if glue_first > 1 and len(words) > glue_first:
        units.append((0, glue_first)); start = glue_first
    units += [(i, i + 1) for i in range(start, len(words))]
    uw = [sum(widths[a:b]) + space * (b - a - 1) for a, b in units]

    def greedy(cap: float) -> int:
        count, current = 1, uw[0]
        for width in uw[1:]:
            if current + space + width <= cap:
                current += space + width
            else:
                count, current = count + 1, width
        return count

    lines_needed = greedy(limit)
    if lines_needed == 1:
        return " ".join(words)
    if lines_needed > max_lines:
        for n_lines in range(max_lines + 1, lines_needed + 1):
            cap = limit * n_lines / max_lines * 0.95          # shrunk to fit n_lines in the box height
            if greedy(cap) <= n_lines:
                lines_needed, limit = n_lines, cap
                break
    n = len(units)
    target = total / lines_needed
    prefix = [0.0]
    for width in uw:
        prefix.append(prefix[-1] + width + space)

    def span(a: int, b: int) -> float:
        return prefix[b] - prefix[a] - space

    inf = float("inf")
    best = [[inf] * (n + 1) for _ in range(lines_needed + 1)]
    back = [[0] * (n + 1) for _ in range(lines_needed + 1)]
    best[0][0] = 0.0
    for k in range(1, lines_needed + 1):
        for b in range(k, n + 1):
            for a in range(k - 1, b):
                if best[k - 1][a] == inf:
                    continue
                width = span(a, b)
                over = max(0.0, width - limit)
                tail = words[units[b - 1][1] - 1].strip(".,;:!?\"'”’)").lower()
                dangling = 25.0 if (b < n and tail in _WEAK_END) else 0.0          # a line should not end on "it", "a", "to", a dash...
                cost = best[k - 1][a] + (width - target) ** 2 + 1000 * over * over + dangling
                if cost < best[k][b]:
                    best[k][b], back[k][b] = cost, a
    cuts, b = [], n
    for k in range(lines_needed, 0, -1):
        a = back[k][b]
        cuts.append((a, b)); b = a
    cuts.reverse()
    return "\n".join(" ".join(words[units[a][0]:units[b - 1][1]]) for a, b in cuts)
