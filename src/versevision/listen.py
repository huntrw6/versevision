"""Listen for spoken Bible references and (optionally) put them on screen.

The audio side is a small macOS program (vvlisten) that writes what it hears as JSON lines. This module reads
those lines, finds references in the speech, and decides when to act. It never shows anything unless told to
(`show` is a callback), so the default is a safe dry run that only writes a log.
"""
from __future__ import annotations

import json
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Callable, Iterable, Iterator

from .references.models import PassageReference
from .references.spoken import find_spoken_references


CONTINUE_WITHIN = 3.0          # seconds: a sentence starting this soon after the last one may continue it


@dataclass
class Detection:
    t: float                    # seconds into the session
    reference: str              # "John 3:16"
    heard: str                  # words it came from
    after_ms: float | None = None   # time from the last partial change to detection
    action: str = "logged"      # logged | shown | skipped-repeat | skipped-unconfirmed
    ref: PassageReference | None = None     # what the parser made of it (book, chapter, verses, any range)


@dataclass
class Detector:
    """Turns a stream of partial/final transcripts into one detection per spoken reference.

    Speech recognition keeps revising its guess, so the same reference shows up in many partials, and a
    half-heard "John three" can later become "John three sixteen". We act on a reference only when it has
    been stable for `settle` seconds (or the sentence is final), and never repeat one inside `cooldown` seconds.
    """
    settle: float = 0.8              # a reference must be unchanged this long...
    quiet: float = 1.6               # ...and then either more words follow it, or the speech has been quiet this long
    cooldown: float = 30.0
    min_confirm: float = 0.0
    seen: dict[str, float] = field(default_factory=dict)
    pending: dict[str, tuple[float, str]] = field(default_factory=dict)
    followed: set[str] = field(default_factory=set)      # references that have more words after them now
    _text_changed_at: float = 0.0
    handled: set[str] = field(default_factory=set)       # references already acted on within this utterance
    _last_text: str = ""
    parsed: dict[str, PassageReference] = field(default_factory=dict)

    def tick(self, t: float) -> list[Detection]:
        """Call regularly with the current time. Acts on references that have stopped changing, even if the
        recogniser sends no further text (it often goes quiet without sending a final result)."""
        out: list[Detection] = []
        for key, (first, heard) in list(self.pending.items()):
            if key in self.handled or t - first < self.settle:
                continue
            if key not in self.followed and t - self._text_changed_at < self.quiet:
                continue                     # "Romans 820" may still become "Romans 828": wait for more words or a pause
            self.handled.add(key)
            del self.pending[key]
            last = self.seen.get(key)
            if last is not None and t - last < self.cooldown:
                out.append(Detection(t, key, heard, action="skipped-repeat", ref=self.parsed.get(key)))
            else:
                self.seen[key] = t
                out.append(Detection(t, key, heard, ref=self.parsed.get(key)))
        return out

    def feed(self, t: float, text: str, final: bool = False) -> list[Detection]:
        out: list[Detection] = []
        found_refs = find_spoken_references(text)
        refs = {r.text: r.heard for r in found_refs}
        for r in found_refs:
            self.parsed[r.text] = r.reference
        # a new sentence has begun when the text got much shorter or no longer starts the same way
        if self._last_text:
            old_head = self._last_text.lower().split()[:3]
            new_head = text.lower().split()[:3]
            if len(text) + 10 < len(self._last_text) or (old_head and new_head and old_head[0] != new_head[0]):
                self.handled.clear()
        if text != self._last_text:
            self._text_changed_at = t
        self._last_text = text
        norm_words = text.lower().replace("-", " ").split()
        # forget pending references that disappeared from the latest guess (the recogniser changed its mind)
        for key in list(self.pending):
            if key not in refs:
                del self.pending[key]
                self.followed.discard(key)
        for key, heard in refs.items():
            if key in self.handled:
                continue
            first = self.pending.setdefault(key, (t, heard))[0]
            heard_words = heard.split()
            if heard_words and norm_words[-len(heard_words):] != heard_words:
                self.followed.add(key)                           # the speaker kept talking after the reference
            if final or (t - first >= self.settle and (key in self.followed or t - self._text_changed_at >= self.quiet)):
                self.handled.add(key)
                del self.pending[key]
                last = self.seen.get(key)
                if last is not None and t - last < self.cooldown:
                    out.append(Detection(t, key, heard, action="skipped-repeat", ref=self.parsed.get(key)))
                else:
                    self.seen[key] = t
                    out.append(Detection(t, key, heard, ref=self.parsed.get(key)))
        if final:
            self.handled.clear()
            self.pending.clear()
            self.followed.clear()
        return out


class Continuer:
    """A short pause can split "turn to John 3." from "verse 16". Sentences that follow each other within CONTINUE_WITHIN
    seconds are joined, so a reference split by a pause is still read whole."""

    def __init__(self, within: float = CONTINUE_WITHIN):
        self.within = within
        self.last_final = ("", -1e9)

    def process(self, item: dict) -> dict:
        if item.get("kind") not in {"partial", "final"}:
            return item
        t_now = clock(item)
        if self.last_final[0] and t_now - self.last_final[1] <= self.within:
            item = dict(item, text=(self.last_final[0].rstrip(" .") + " " + item.get("text", "")).strip())
        if item.get("kind") == "final":
            self.last_final = (item.get("text", ""), t_now)
        return item


def clock(item: dict) -> float:
    """Seconds on one shared clock. The listener stamps wall-clock seconds ("w"); older logs only have "t" (seconds into the run)."""
    w = item.get("w")
    return float(w) if w is not None else float(item.get("t", 0))


def read_lines(path: Path, *, follow: bool = True, poll: float = 0.05) -> Iterator[dict]:
    """Yield JSON objects from the listener's output file as they are written."""
    with open(path, "r", encoding="utf-8") as fh:
        while True:
            line = fh.readline()
            if not line:
                if not follow:
                    return
                time.sleep(poll)
                continue
            line = line.strip()
            if not line:
                continue
            try:
                yield json.loads(line)
            except json.JSONDecodeError:
                continue


def run(lines: Iterable[dict], *, show: Callable[[str], bool] | None = None, detector: Detector | None = None,
        log: Callable[[str], None] = print) -> list[Detection]:
    """Consume transcript lines; call ``show(reference)`` for each new reference (if given); return detections."""
    detector = detector or Detector()
    found: list[Detection] = []
    last_final = ("", -1e9)            # text and time of the previous finished sentence
    for item in lines:
        kind = item.get("kind")
        if kind in {"partial", "final"}:
            # A short pause can split "turn to John 3." from "verse 16". Treat sentences that follow each other within a few
            # seconds as one stretch of speech so a reference split by a pause is still read whole.
            t_now = float(item.get("t", 0))
            if last_final[0] and t_now - last_final[1] <= CONTINUE_WITHIN:
                item = dict(item, text=(last_final[0].rstrip(" .") + " " + item.get("text", "")).strip())
            if kind == "final":
                last_final = (item.get("text", ""), t_now)
        if kind in {"status"} or kind is None:
            # status lines carry the clock too (the listener sends one every 5 s), so quiet moments still settle
            ticks = detector.tick(float(item.get("t", 0)))
        elif kind in {"partial", "final"}:
            ticks = detector.tick(float(item.get("t", 0)))
        else:
            continue
        for d in ticks:
            if d.action == "logged" and show is not None:
                d.action = "shown" if show(d.reference) else "show-failed"
            found.append(d)
            log(f"[{d.t:8.2f}s] {d.action:15} {d.reference:22} heard: {d.heard!r}")
        if kind not in {"partial", "final"}:
            continue
        for d in detector.feed(float(item.get("t", 0)), item.get("text", ""), final=(kind == "final")):
            if d.action == "logged" and show is not None:
                ok = show(d.reference)
                d.action = "shown" if ok else "show-failed"
            found.append(d)
            log(f"[{d.t:8.2f}s] {d.action:15} {d.reference:22} heard: {d.heard!r}")
    return found
