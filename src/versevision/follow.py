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
from .references.spoken import _version_at, find_chapter_only, find_spoken_references, verse_splits
from .scripture import ScriptureIndex, normalize


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
    range_end: tuple[int, int] | None = None     # (chapter, verse) when the speaker named a range: the whole range can be loaded at once
    version: str = ""                            # a Bible version to switch to first (e.g. "NIV"); empty = leave the selector alone
    version_source: str = ""                     # "said" or "wording"
    late_version: bool = False                   # the version was said after the verse was already shown: show the same verse again in it
    tentative: bool = False                      # chosen as the earliest of several equally good matches

    @property
    def reference(self) -> str:
        return f"{self.book} {self.chapter}:{self.verse}" if self.kind == "show" else ""


@dataclass
class Pending:
    """"Isaiah 42" was heard with no verse. After a pause it may turn out to be Isaiah 40:2 (or 4:2) that was run together, so the
    candidates wait for the words that follow to say which one (if any) is being read."""
    book_id: str
    book: str
    number: int
    first_seen: float
    last_changed: float
    candidates: list[tuple[int, int]]
    heard: str = ""
    resolved: bool = False


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
    version_said: bool = False                   # a version has been named for this verse (with it, or just after it)
    reading_share: float = 0.0                   # how much of this verse the speaker has been heard reading (0-1)
    assumed: bool = False                        # this verse was put up on the assumption that the speaker carried straight on
    assumed_at: float = 0.0
    assumed_from: tuple[str, int, int] | None = None   # the verse that was on screen before the assumption
    confirmed: bool = False
    read_started: float = 0.0                    # when the speaker began reading this verse (first voice after it came up)
    version_shown: str = ""


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
    last: Shown | None = None            # where the speaker was, kept after the screen clears so reading on from there is still understood
    context_keep: float = 300.0          # seconds the "where we are in the chapter" context is kept
    look_ahead: int = 6                  # how many verses ahead of the last one are checked
    tail_words: int = 10                 # the latest words of speech compared with those verses
    assume: bool = True                  # put the next verse up as soon as the speaker carries on after finishing the one on screen
    assume_progress: float = 0.8         # how much of the verse on screen must have been heard before an assumption is made
    assume_window: float = 8.0           # ... and speech must resume within this many seconds of its ending (or of the last read words)
    verify_words: int = 9                # words after the assumed verse starts before the guess is checked
    verify_timeout: float = 5.0          # seconds after which an unchecked guess is judged on whatever was heard
    chapter_pause: float = 1.2           # a chapter-only mention must stay unchanged this long (a pause) before it is re-read as chapter + verse
    chapter_window: float = 12.0         # ... and the verse must start being read within this long
    pending: list = field(default_factory=list)
    _last_text: str = ""
    wps: float = 2.7                     # the speaker's reading pace in words per second of speech; learned as verses are read
    min_read_seconds: float = 2.5        # a verse must have been on screen this long before the next one can be guessed
    est_progress: float = 0.85           # an assumption needs this much of the verse to be accounted for by speaking time
    min_transcript_share: float = 0.55    # ... while the transcript must already show it is this verse (it lags the voice by ~1.5 s)
    _segments: list = field(default_factory=list)   # [start, end] of each stretch of speech heard; end is None while it is going on
    _voice_at: float = -1e9              # when speech was last heard to start
    _quiet_since: float = -1e9
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
            txt = item.get("text", "")
            if txt == "speech start":
                if not self._segments or self._segments[-1][1] is not None:
                    self._segments.append([t, None])
                self._segments = self._segments[-60:]
                return self._voice_started(t) + self.tick(t)
            if txt == "speech end":
                self._quiet_since = t
                if self._segments and self._segments[-1][1] is None:
                    self._segments[-1][1] = t
                return self.tick(t) + self._judge_assumption(t, self._last_text, speech_ended=True)
            return self.tick(t) + self._judge_assumption(t, self._last_text)
        item = self.continuer.process(item)
        text = item.get("text", "")
        self._last_text = text
        final = kind == "final"
        actions: list[Action] = []
        referenced = False
        for det in self.detector.feed(t, text, final=final):
            act = self._from_detection(det, t)
            if act:
                actions.append(act)
                referenced = True
        actions += self._tick_detector(t)
        self._note_chapter_only(t, text)
        # the speaker is reading what is on screen: keep it up, and note how far through it they are
        if self.current and self.index:
            if self.index.reading_score(text, self.current.book_id, self.current.chapter, self.current.verse) >= 78:
                self.current.last_activity = t
            if not self.current.assumed:
                share, _ = self.index.progress(text, self.current.book_id, self.current.chapter, self.current.verse)
                self.current.reading_share = max(self.current.reading_share, share)
        judged = self._judge_assumption(t, text)
        if judged:
            actions += judged
            referenced = True if any(a.kind == "show" for a in judged) else referenced
        # While a guess is waiting to be checked, only the check (or a spoken reference) may change the screen: the transcript
        # is still catching up with the voice, so reading it as "the speaker moved on" would just undo the guess.
        guessing = bool(self.current and self.current.assumed and not self.current.confirmed)
        if not referenced and not guessing:
            via = self._resolve_chapter_only(t, text)
            if via:
                actions.append(via)
                referenced = True
        if not referenced and not guessing:
            assumed = self._assume_from_words(t, text)
            if assumed:
                actions.append(assumed)
                referenced = True
        if not referenced and not guessing:
            seq = self._sequential(t, text)
            if seq:
                actions.append(seq)
        if not referenced and not guessing and not any(a.kind == "show" for a in actions):
            late = self._late_version(t, text)
            if late:
                actions.append(late)
        if not referenced and not guessing and not any(a.late_version for a in actions):
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
            self.last = self.current
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
        act = self._show(t, ref.book_id, ref.book_display_name, chapter, verse, range_end=end, reason="the speaker said the reference", heard=det.heard)
        if act and ref.version:
            act.version, act.version_source = ref.version, "said"
            if self.current:
                self.current.version_said, self.current.version_shown = True, ref.version
        return act

    def _late_version(self, t: float, text: str) -> Action | None:
        """"Romans 8:28" ... pause ... "in the New Living Translation": Whisper often splits these. A version named within a few
        seconds of a verse being shown, when none was named with it, applies to that verse."""
        cur = self.current
        if not cur or cur.version_said or t - cur.shown_at > 8:
            return None
        words = normalize(text).split()
        for i in range(max(0, len(words) - 14), len(words)):
            hit = _version_at(words, i)
            if hit and (i == 0 or words[i - 1] not in {"a", "an"}) and (hit[0] != "AMP" or words[i:i + 2] == ["amplified", "classic"] or True):
                if hit[0] == cur.version_shown:
                    return None
                cur.version_said = True
                act = Action("show", t, reason=f"the speaker named the version ({hit[0]}) after the verse", book_id=cur.book_id, book=cur.book,
                             chapter=cur.chapter, verse=cur.verse, heard=" ".join(words[-8:]), version=hit[0], version_source="said", late_version=True,
                             range_end=None)
                return act
        return None

    # ------------------------------------------------------------------ 1a. "Isaiah 42": a chapter, or "Isaiah 40:2" run together?
    def _note_chapter_only(self, t: float, text: str) -> None:
        """Keep track of book+number mentions that have no verse, and forget those that got one or went stale."""
        if not self.index:
            return
        mentions = find_chapter_only(text)
        full_books = {r.reference.book_id for r in find_spoken_references(text)}
        for m in mentions:
            if m.book_id in full_books:
                continue                                    # a full reference to this book is in the same stretch of speech
            key = (m.book_id, m.number)
            existing = next((p for p in self.pending if (p.book_id, p.number) == key and not p.resolved), None)
            if existing:
                existing.heard = m.heard
                continue
            if t - self.shown_at.get(("chapter", m.book_id, m.number), -1e9) < self.cooldown:
                continue
            cands = [(c, v) for (c, v) in verse_splits(m.number, 150) if self.index.has_verse(m.book_id, c, v)]
            if self.index.has_verse(m.book_id, m.number, 1) and (m.number, 1) not in cands:
                cands.append((m.number, 1))                     # or it really was the chapter, and the reading starts at its first verse
            if not cands:
                continue
            self.pending.append(Pending(m.book_id, m.book, m.number, t, t, cands, m.heard))
            self.pending = self.pending[-4:]
        # a full reference to the same book resolves it the normal way
        for p in self.pending:
            if p.book_id in full_books:
                p.resolved = True
        self.pending = [p for p in self.pending if not p.resolved and t - p.first_seen <= self.chapter_window + self.chapter_pause + 6]

    def _resolve_chapter_only(self, t: float, text: str) -> Action | None:
        """After the pause, see which candidate verse the speaker is now reading (its opening words), and put that one up."""
        if not self.pending or not self.index:
            return None
        words = normalize(text).split()
        for p in list(self.pending):
            if t - p.first_seen < self.chapter_pause or t - p.first_seen > self.chapter_window + self.chapter_pause:
                continue
            marker = normalize(p.heard).split()
            pos = -1
            for k in range(len(words) - len(marker), -1, -1):
                if words[k:k + len(marker)] == marker:
                    pos = k + len(marker)
                    break
            after = words[pos:] if pos >= 0 else words[-14:]
            if len(after) < 4:
                continue
            best = None
            for rank, (c, v) in enumerate(p.candidates):
                score = self.index.opening_matches(" ".join(after[:14]), p.book_id, c, v, words=5)
                if score >= 84 and (best is None or score > best[0] + 4):
                    best = (score, rank, c, v)
            if not best:
                continue
            _, rank, c, v = best
            p.resolved = True
            self.shown_at[("chapter", p.book_id, p.number)] = t
            heard = f'"{p.heard}" then "{" ".join(after[:7])}"'
            return self._show(t, p.book_id, p.book, c, v, reason=f"heard only a chapter ({p.number}) but what followed was {p.book} {c}:{v}", heard=heard)
        return None

    # ------------------------------------------------------------------ 1b. confident but assumptive: the next verse straight away
    def _next_in_line(self, ctx: "Shown") -> tuple[int, int] | None:
        """The verse that comes next after ``ctx`` in its chapter (None at the end of the chapter or of a named range)."""
        if not self.index:
            return None
        v = ctx.verse + 1
        if not self.index.has_verse(ctx.book_id, ctx.chapter, v):
            return None
        if ctx.range_end and ctx.chapter == ctx.range_end[0] and v > ctx.range_end[1]:
            return None
        return ctx.chapter, v

    def _speaking_since(self, since: float, now: float) -> float:
        total = 0.0
        for s, e in self._segments:
            e = now if e is None else e
            total += max(0.0, min(e, now) - max(s, since))
        return total

    def _estimated_share(self, cur: "Shown", t: float) -> float:
        """How much of the verse the speaker has probably said, from how long they have been talking and at what pace, once the
        transcript confirms this is the verse. (The transcript itself is 1-2 s behind the voice.)"""
        if not self.index or not cur.read_started or cur.reading_share < self.min_transcript_share:
            return cur.reading_share
        words = max((len(x.split()) for x in self.index.verse_texts(cur.book_id, cur.chapter, cur.verse)), default=0)
        if words < 4:
            return cur.reading_share
        spoken = self._speaking_since(cur.read_started, t) * self.wps
        # The transcript is about 2 s behind the voice, so the speaker can be at most that far ahead of what it shows: the timing
        # estimate may add the words of those 2 s to the transcript's share, but no more (a long pause or an aside is not reading).
        ahead = min(1.0, cur.reading_share + (self.wps * 2.2) / words)
        return max(cur.reading_share, min(spoken / words, ahead))

    def _learn_pace(self, cur: "Shown", t: float) -> None:
        """A verse that was read through and confirmed teaches the pace (words per second of speech)."""
        if not self.index or not cur.read_started:
            return
        words = max((len(x.split()) for x in self.index.verse_texts(cur.book_id, cur.chapter, cur.verse)), default=0)
        secs = self._speaking_since(cur.read_started, t)
        if words >= 8 and secs >= 1.5 and cur.reading_share >= 0.9:
            self.wps = max(1.6, min(4.2, 0.6 * self.wps + 0.4 * (words / secs)))

    def _can_assume(self, t: float) -> "tuple[Shown, tuple[int, int]] | None":
        cur = self.current
        if not (self.assume and cur and self.index) or cur.assumed:
            return None                                   # one guess at a time: a guess is never built on a guess
        if self._estimated_share(cur, t) < self.est_progress or t - cur.last_activity > self.assume_window + 6:
            return None
        if t - cur.shown_at < self.min_read_seconds:
            return None                                   # the verse only just came up: it cannot have been read through yet
        nxt = self._next_in_line(cur)
        return (cur, nxt) if nxt else None

    def _assume(self, t: float, cur: "Shown", nxt: tuple[int, int], why: str, heard: str = "") -> Action | None:
        prev = (cur.book_id, cur.chapter, cur.verse)
        self._learn_pace(cur, t)
        act = self._show(t, cur.book_id, cur.book, nxt[0], nxt[1], range_end=cur.range_end, reason=why, heard=heard, followed=cur.followed + 1)
        if act and self.current:
            self.current.assumed, self.current.assumed_at, self.current.assumed_from = True, t, prev
            self.current.reading_share = 0.0
            act.tentative = True
        return act

    def _voice_started(self, t: float) -> list[Action]:
        """Speech started again. If the verse on screen has been read to its end, this is almost certainly the next verse: put it
        up now (before a single word has been transcribed) and let the words that follow confirm or correct it."""
        self._voice_at = t
        if self.current and not self.current.read_started:
            self.current.read_started = t            # the first voice after a verse comes up is the speaker reading it
        got = self._can_assume(t)
        if not got:
            return []
        cur, nxt = got
        act = self._assume(t, cur, nxt, f"assumed: the speaker carried on after reading {cur.book} {cur.chapter}:{cur.verse}")
        return [act] if act else []

    def _assume_from_words(self, t: float, text: str) -> Action | None:
        """The same assumption, made from the first words heard (when no voice-onset signal arrived, e.g. the Apple engine)."""
        got = self._can_assume(t)
        if not got or not self.index:
            return None
        cur, nxt = got
        share, end = self.index.progress(text, cur.book_id, cur.chapter, cur.verse)
        if end < 0:
            return None                                   # the end of the verse has not been heard, so these words may still belong to it
        after = normalize(text).split()[end:]
        if not after:
            return None
        return self._assume(t, cur, nxt, f"assumed: the speaker carried on after reading {cur.book} {cur.chapter}:{cur.verse}", heard=" ".join(after[:8]))

    def _judge_assumption(self, t: float, text: str, speech_ended: bool = False) -> list[Action]:
        """Check a guess against the words that followed it. Confirmed: keep it. Contradicted: go back to the verse before, or
        clear the screen if the speaker has moved on to something that is not Scripture."""
        cur = self.current
        if not (cur and cur.assumed and not cur.confirmed and self.index and cur.assumed_from):
            return []
        words = normalize(text).split()
        book_id, chapter, verse = cur.book_id, cur.chapter, cur.verse
        # words spoken since the guess: everything after the end of the previous verse
        pb, pc, pv = cur.assumed_from
        _, end = self.index.progress(text, pb, pc, pv)
        after = words[end:] if end >= 0 else words[-self.verify_words:]
        opening = self.index.opening_matches(" ".join(after), book_id, chapter, verse, words=min(5, max(2, len(after)))) if after else 0.0
        # How well the latest speech fits the guessed verse as a whole. The first words of a sentence are often garbled while the rest
        # comes out fine ("The Bible says, For the most glorious kinds" ... "For you know that the testing of your faith"), so a
        # good fit anywhere in the latest words confirms the guess just as the opening does.
        fit = self.index.reading_score(" ".join(words[-16:]), book_id, chapter, verse) if len(words) >= 5 else 0.0
        if len(after) >= 2 and opening >= 80 or (len(after) >= self.verify_words and opening >= 62) or fit >= 72:
            cur.confirmed = True
            prev_obj = self.last if self.last is not cur else None
            cur.last_activity = t
            return []
        waited = t - cur.assumed_at
        if fit >= 55 and not speech_ended:
            return []                                     # it still looks like this verse: never withdraw on a partial fit
        if speech_ended and not after and waited >= 0.3 and waited <= 0.9:
            # The "start of speech" was only a blip (a breath, a dip inside the last word): nothing new was said before it went quiet again.
            cur.assumed = False
            self.current = None
            act = self._show(t, pb, cur.book, pc, pv, range_end=cur.range_end, reason="the guess at the next verse was wrong: nothing new was said", heard="")
            if act:
                self.current.reading_share = 1.0
            return [act] if act else []
        if waited >= 3.0 and (len(after) >= self.verify_words or (waited >= self.verify_timeout and len(after) >= 3)):
            # Not the next verse. Go back to the verse before: it was read to its end, so it is the right thing to leave up (the hold
            # timer clears it later). Anything the speaker says next is read from there.
            cur.assumed = False
            self.shown_at.pop((book_id, chapter, verse), None)
            self.current = None
            act = self._show(t, pb, cur.book, pc, pv, range_end=cur.range_end, reason="the guess at the next verse was wrong: back to the verse before", heard=" ".join(after[:8]))
            if act:
                self.current.reading_share = 1.0
            return [act] if act else []
        if waited >= 12:                                  # nothing more heard for a long time: let the normal hold timer decide
            cur.confirmed = True
        return []

    # ------------------------------------------------------------------ 2a. the speaker is reading on through the chapter
    def _sequential(self, t: float, text: str) -> Action | None:
        """Where in the chapter is the speaker NOW? Compare the latest words with the verses around the last one shown (the one
        before it up to ``look_ahead`` after it) instead of searching the whole Bible. That is quick, tolerant of a few misheard
        words, copes with a skipped verse, and keeps working after the screen has cleared."""
        ctx = self.current or (self.last if self.last and t - self.last.last_activity <= self.context_keep else None)
        if not ctx or not self.index:
            return None
        words = normalize(text).split()
        if len(words) < 4:
            return None
        idx = self.index
        cands: list[tuple[int, int]] = []
        c, v = ctx.chapter, max(1, ctx.verse - 1)
        for _ in range(self.look_ahead + 2):
            if not idx.has_verse(ctx.book_id, c, v):
                if idx.has_verse(ctx.book_id, c + 1, 1):
                    c, v = c + 1, 1
                else:
                    break
            cands.append((c, v))
            v += 1
        best = None
        for c, v in cands:
            step = (c - ctx.chapter) * 1000 + (v - ctx.verse)
            # Several window sizes: just after the speaker starts a new verse the last 10 words are half old verse, half new, so a
            # short window (the last 5-6 words) is what first fits the new one; a long window is what makes a match convincing.
            fits = []
            for size in (5, 7, self.tail_words):
                tail = words[-size:]
                if len(tail) < min(size, 5):
                    continue
                for vn in idx.verse_texts(ctx.book_id, c, v):
                    m_, cov_, last_, first_ = idx.tail_fit(tail, vn.split())
                    if first_ > 7 and m_ < 8:
                        continue          # a few words matching the MIDDLE or END of a verse is a coincidence ("of the Lord Jesus Christ"); beginning to read a verse means its opening
                    if last_ and m_ >= min(5, len(tail)):
                        fits.append((m_, cov_, size))
            if not fits:
                continue                                                  # the speaker's last words must be in this verse
            matched, cover, _ = max(fits, key=lambda x: (x[1], x[0]))
            if step == 0:
                if matched >= 4 and cover >= 0.6:
                    ctx.last_activity = t                                   # still in the verse that is up
                continue
            if ctx.range_end and (c, v) > ctx.range_end:
                continue                                                      # the speaker named a range: stop at its end
            if step == 1:
                need_matched, need_cover = 5, 0.7
            elif step < 0:
                continue                                                      # never go backwards on a transcript: a late sentence looks like a re-read
            else:
                need_matched, need_cover = 7, 0.8                             # skipped ahead, or reading on past a named range
            if matched < min(need_matched, len(tail)) or cover < need_cover:
                continue
            score = cover + 0.02 * matched
            if best is None or score > best[0] + 0.05 or (abs(score - best[0]) <= 0.05 and step > best[4]):
                best = (score, c, v, matched, step, cover)
        if not best:
            return None
        _, c, v, matched, step, cover = best
        if c != ctx.chapter:
            return None                                                   # crossing into the next chapter needs a spoken reference or a clear read
        return self._show(t, ctx.book_id, ctx.book, c, v, range_end=ctx.range_end,
                          reason=f"the speaker began reading verse {v} (in sequence: {matched} words, {cover:.0%})", heard=" ".join(words[-8:]), followed=ctx.followed + 1)

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
            after, ended = self.index.words_after_ending(text, cur.book_id, cur.chapter, cur.verse)
            if not ended and self.index.reading_score(text, cur.book_id, cur.chapter, cur.verse) >= 78:
                break                          # still reading the current verse (its ending is not yet heard): do not run ahead
            score = self.index.opening_matches(after, cur.book_id, cur.chapter, v, words=self.next_words)
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
        # Look at the longest stretch of speech first: more words is more evidence, and it is what can tell parallel
        # passages apart. Shorter windows only matter when a long one is diluted by unrelated talk before the verse.
        for n in (40, 26, 18, 12):
            hit = self.index.match(" ".join(words[-n:]), prefer=prefer)
            if hit is not None:
                found = hit
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
        note = " , tied with " + ", ".join(f"{a.book} {a.chapter}:{a.verse}" for a in found.alternatives) + "; earliest chosen" if found.tied else ""
        act = self._show(t, v.book_id, v.book, v.chapter, v.verse, reason=f"heard the verse itself ({found.version} {found.score:.2f}){note}", heard=" ".join(words[-14:]))
        if act and found.tied:
            act.tentative = True
        if act and found.version_clear and not found.tied:
            act.version, act.version_source = found.version, "wording"
            if self.current:
                self.current.version_said, self.current.version_shown = True, found.version
        return act

    # ------------------------------------------------------------------ the one place that decides to show
    def _show(self, t: float, book_id: str, book: str, chapter: int, verse: int, *, reason: str, heard: str = "",
              range_end: tuple[int, int] | None = None, followed: int = 0) -> Action | None:
        key = (book_id, chapter, verse)
        last = self.shown_at.get(key)
        if last is not None and t - last < self.cooldown and not reason.startswith(("the speaker began reading", "the guess", "assumed")):
            return None                          # the same verse was just shown
        self.shown_at[key] = t
        self.current = Shown(book_id, book, chapter, verse, t, t, range_end=range_end, followed=followed)
        self.last = self.current
        if self.current:
            if reason.startswith(("the speaker began reading", "heard the verse itself", "heard only a chapter")) and self.index:
                # the words that made us show it were the verse's opening: the speaker is already part-way into it
                share, _ = self.index.progress(heard, book_id, chapter, verse)
                self.current.reading_share = max(self.current.reading_share, share)
            if reason.startswith("assumed"):
                self.current.read_started = t
            elif reason.startswith(("the speaker began reading", "heard the verse itself")):
                self.current.read_started = max(0.0, t - 1.5)          # the words are heard ~1.5 s after they are spoken
            else:
                self.current.read_started = 0.0
        self._cand, self._cand_count = None, 0
        return Action("show", t, reason=reason, book_id=book_id, book=book, chapter=chapter, verse=verse, heard=heard,
                      range_end=range_end if (range_end and range_end[0] == chapter and not followed) else None)
