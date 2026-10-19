"""Decide what to put on screen while listening to a sermon.

Three things can put a verse up:

  1. The speaker says a reference ("John chapter three verse sixteen").
  2. The speaker begins reading the NEXT verse of the passage that is on screen: the screen follows along, so a
     reference to a range ("Psalm 23, one through three") or a single verse that the speaker keeps reading both work.
  3. The speaker reads Scripture without saying where it is from: the words are matched against the Bible text.

The class here is pure logic: it takes transcript lines and returns actions ("show this verse", "clear the screen").
Doing the action (ProPresenter, the log) is the caller's job, so all of this is testable without a Mac.
"""
from __future__ import annotations

from dataclasses import dataclass, field

from .listen import Continuer, Detection, Detector, clock
from .references.models import PassageReference
from .scripture import ScriptureIndex


@dataclass
class Action:
    kind: str                    # "show" | "clear"
    t: float
    reason: str = ""
    book_id: str = ""
    book: str = ""
    chapter: int = 0
    verse: int = 0
    heard: str = ""

    @property
    def reference(self) -> str:
        return f"{self.book} {self.chapter}:{self.verse}" if self.kind == "show" else ""


@dataclass
class Shown:
    book_id: str
    book: str
    chapter: int
    verse: int
    shown_at: float
    last_activity: float
    range_end: tuple[int, int] | None = None     # (chapter, verse) when the speaker named a range
    followed: int = 0                            # how many next-verse steps we have taken from it


@dataclass
class Follower:
    index: ScriptureIndex | None = None
    hold: float = 30.0                  # seconds a verse stays up after the speaker last touched it
    cooldown: float = 30.0              # do not show the same verse again within this many seconds
    follow: bool = True                 # move on to the next verse when the speaker starts reading it
    content: bool = True                # recognise a verse from its words when the reference was never said
    follow_window: float = 60.0         # stop waiting for the next verse this long after the last sign of reading
    next_threshold: float = 86.0        # how well the opening of the next verse must match (0-100)
    next_words: int = 5                 # how many opening words are compared
    detector: Detector = field(default_factory=Detector)
    continuer: Continuer = field(default_factory=Continuer)
    current: Shown | None = None
    shown_at: dict[tuple[str, int, int], float] = field(default_factory=dict)
    _cand: tuple[str, int, int] | None = None
    _cand_count: int = 0
    log: list[str] = field(default_factory=list)

    # ------------------------------------------------------------------ input
    def feed(self, item: dict) -> list[Action]:
        kind = item.get("kind")
        if kind not in {"partial", "final", "status"}:
            return []
        t = clock(item)
        if kind == "status":
            return self.tick(t)
        item = self.continuer.process(item)
        text = item.get("text", "")
        final = kind == "final"
        actions: list[Action] = []
        referenced = False
        for det in self.detector.feed(t, text, final=final):
            act = self._from_detection(det, t)
            if act:
                actions.append(act)
                referenced = True
        actions += self._tick_detector(t)
        # the speaker is reading what is on screen: keep it up
        if self.current and self.index:
            if self.index.reading_score(text, self.current.book_id, self.current.chapter, self.current.verse) >= 78:
                self.current.last_activity = t
        if not referenced:
            nxt = self._next_verse(t, text) if self.follow else None
            if nxt:
                actions.append(nxt)
            elif self.content:
                heard = self._from_content(t, text, final)
                if heard:
                    actions.append(heard)
        actions += self.tick(t, _detect=False)
        return actions

    def tick(self, t: float, *, _detect: bool = True) -> list[Action]:
        """Call regularly with the current time: settles references that have stopped changing and clears expired verses."""
        out: list[Action] = []
        if _detect:
            out += self._tick_detector(t)
        if self.current and t - self.current.last_activity >= self.hold:
            out.append(Action("clear", t, reason=f"{self.hold:.0f} s since the speaker last touched {self.current.book} {self.current.chapter}:{self.current.verse}"))
            self.current = None
            self._cand, self._cand_count = None, 0
        return out

    # ------------------------------------------------------------------ 1. a reference was said
    def _tick_detector(self, t: float) -> list[Action]:
        out = []
        for det in self.detector.tick(t):
            act = self._from_detection(det, t)
            if act:
                out.append(act)
        return out

    def _from_detection(self, det: Detection, t: float) -> Action | None:
        if det.action != "logged" or det.ref is None or not det.ref.spans:
            return None
        ref: PassageReference = det.ref
        span = ref.spans[0]
        chapter, verse = span.start.chapter, span.start.verse
        end = (span.end.chapter, span.end.verse) if span.end else None
        if self.current and (self.current.book_id, self.current.chapter, self.current.verse) == (ref.book_id, chapter, verse):
            self.current.last_activity = t
            if end and not self.current.range_end:
                self.current.range_end = end
            return None
        return self._show(t, ref.book_id, ref.book_display_name, chapter, verse, range_end=end, reason="the speaker said the reference", heard=det.heard)

    # ------------------------------------------------------------------ 2. the speaker moves on to the next verse
    def _next_verse(self, t: float, text: str) -> Action | None:
        cur = self.current
        if not cur or not self.index:
            return None
        if t - cur.last_activity > self.follow_window:
            return None
        best: tuple[float, int] | None = None
        for step in (1, 2, 3):
            v = cur.verse + step
            if cur.range_end and cur.chapter == cur.range_end[0] and v > cur.range_end[1]:
                break
            if not self.index.has_verse(cur.book_id, cur.chapter, v):
                break
            score = self.index.opening_matches(text, cur.book_id, cur.chapter, v, words=self.next_words)
            if score >= self.next_threshold:
                best = (score, v)            # the furthest verse that has begun: if two openings are in the text, the speaker is on the later one
        if not best:
            return None
        return self._show(t, cur.book_id, cur.book, cur.chapter, best[1], range_end=cur.range_end,
                          reason=f"the speaker began reading verse {best[1]} (matched {best[0]:.0f}%)", heard=text[-80:], followed=cur.followed + 1)

    # ------------------------------------------------------------------ 3. a verse read without saying where it is
    def _from_content(self, t: float, text: str, final: bool) -> Action | None:
        if not self.index:
            return None
        words = text.split()
        prefer = (self.current.book_id, self.current.chapter) if self.current else None
        found = None
        for n in (12, 18, 26, 40):
            window = " ".join(words[-n:])
            found = self.index.match(window, prefer=prefer)
            if found:
                break
        if not found:
            self._cand, self._cand_count = None, 0
            return None
        v = found.verse
        key = (v.book_id, v.chapter, v.verse)
        if self.current and key == (self.current.book_id, self.current.chapter, self.current.verse):
            self.current.last_activity = t
            return None
        if t - self.shown_at.get(key, -1e9) < self.cooldown:
            return None
        if key == self._cand:
            self._cand_count += 1
        else:
            self._cand, self._cand_count = key, 1
        if self._cand_count < 2 and not final:
            return None                          # one clean read of the same verse twice in a row, or the sentence has ended
        self._cand, self._cand_count = None, 0
        return self._show(t, v.book_id, v.book, v.chapter, v.verse, reason=f"heard the verse itself ({found.version} {found.score:.2f})", heard=" ".join(words[-14:]))

    # ------------------------------------------------------------------ the one place that decides to show
    def _show(self, t: float, book_id: str, book: str, chapter: int, verse: int, *, reason: str, heard: str = "",
              range_end: tuple[int, int] | None = None, followed: int = 0) -> Action | None:
        key = (book_id, chapter, verse)
        last = self.shown_at.get(key)
        if last is not None and t - last < self.cooldown and not reason.startswith("the speaker began reading"):
            return None                          # the same verse was just shown
        self.shown_at[key] = t
        self.current = Shown(book_id, book, chapter, verse, t, t, range_end=range_end, followed=followed)
        self._cand, self._cand_count = None, 0
        return Action("show", t, reason=reason, book_id=book_id, book=book, chapter=chapter, verse=verse, heard=heard)
