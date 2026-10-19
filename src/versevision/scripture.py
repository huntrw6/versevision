"""A local copy of Scripture text, used to recognise a verse by its words when the speaker never says the reference.

Two public-domain English Bibles are used (KJV and the Berean Standard Bible). The text a congregation reads from
is usually different (ESV, NIV...), but the wording overlaps enough that a careful fuzzy match still finds the verse.
The matcher is deliberately conservative: it needs a long run of words, and it refuses when two different verses
match about equally well (parallel passages), because showing the wrong verse is worse than showing none.
"""
from __future__ import annotations

import json
import re
from collections import defaultdict
from dataclasses import dataclass
from pathlib import Path

from rapidfuzz import fuzz
from rapidfuzz.distance import LCSseq

from .references.books import BOOKS

SOURCES = {
    "KJV": "https://raw.githubusercontent.com/thiagobodruk/bible/master/json/en_kjv.json",
    "BSB": "https://raw.githubusercontent.com/scrollmapper/bible_databases/master/formats/json/BSB.json",
}

_NON_WORD = re.compile(r"[^a-z0-9' ]+")
_SPACE = re.compile(r"\s+")


def normalize(text: str) -> str:
    text = text.lower().replace("\u2019", "'").replace("\u2018", "'").replace("-", " ").replace("\u2014", " ")
    text = re.sub(r"\b(\d+)\b", " ", text) if False else text
    return _SPACE.sub(" ", _NON_WORD.sub(" ", text)).strip()


@dataclass(frozen=True)
class Verse:
    book_id: str
    book: str
    chapter: int
    verse: int
    text: str

    @property
    def reference(self) -> str:
        return f"{self.book} {self.chapter}:{self.verse}"


@dataclass
class ContentMatch:
    verse: Verse
    score: float
    words: int                 # how many words of the speech lined up with the verse
    version: str


def default_dir() -> Path:
    from platformdirs import user_data_path
    return user_data_path("versevision") / "scripture"


def download(directory: Path | None = None, *, log=print) -> Path:
    """Fetch the two Bibles once. Plain downloads of public-domain text; nothing is sent anywhere."""
    import httpx
    directory = directory or default_dir()
    directory.mkdir(parents=True, exist_ok=True)
    for name, url in SOURCES.items():
        target = directory / f"{name}.json"
        if target.exists() and target.stat().st_size > 1_000_000:
            continue
        log(f"Downloading {name} ...")
        resp = httpx.get(url, timeout=120, follow_redirects=True)
        resp.raise_for_status()
        target.write_bytes(resp.content)
    return directory


def _load_file(path: Path, version: str) -> list[Verse]:
    data = json.loads(path.read_text(encoding="utf-8-sig"))
    out: list[Verse] = []
    if isinstance(data, dict):                       # {"books":[{"name","chapters":[{"verses":[{"verse","text"}]}]}]}
        books = data["books"]
        for book, raw in zip(BOOKS, books):
            for c_no, chapter in enumerate(raw["chapters"], 1):
                for v in chapter["verses"]:
                    out.append(Verse(book.id, book.name, c_no, int(v["verse"]), str(v["text"]).strip()))
    else:                                            # [{"abbrev","name","chapters":[[verse text,...],...]}]
        for book, raw in zip(BOOKS, data):
            for c_no, chapter in enumerate(raw["chapters"], 1):
                for v_no, text in enumerate(chapter, 1):
                    out.append(Verse(book.id, book.name, c_no, v_no, str(text).strip()))
    return out


class ScriptureIndex:
    """Searchable verses from one or more Bibles."""

    def __init__(self, bibles: dict[str, list[Verse]] | None = None):
        self.bibles = bibles or {}
        self.entries: list[tuple[str, Verse, str]] = []          # (version, verse, normalized text)
        self.grams: dict[tuple[str, str, str], list[int]] = defaultdict(list)
        self.by_ref: dict[tuple[str, int, int], dict[str, str]] = defaultdict(dict)
        self.chapter_sizes: dict[tuple[str, int], int] = {}
        for version, verses in self.bibles.items():
            for verse in verses:
                norm = normalize(verse.text)
                idx = len(self.entries)
                self.entries.append((version, verse, norm))
                words = norm.split()
                for i in range(len(words) - 2):
                    self.grams[(words[i], words[i + 1], words[i + 2])].append(idx)
                self.by_ref[(verse.book_id, verse.chapter, verse.verse)][version] = norm
                key = (verse.book_id, verse.chapter)
                self.chapter_sizes[key] = max(self.chapter_sizes.get(key, 0), verse.verse)
        # a trigram found in very many verses ("and he said unto") says nothing, so ignore it when voting
        self.common = {g for g, ids in self.grams.items() if len(ids) > 60}
        # rare words say more than common ones: weight each word by how few verses use it
        import math
        doc_freq: dict[str, int] = defaultdict(int)
        for _, _, norm in self.entries:
            for w in set(norm.split()):
                doc_freq[w] += 1
        total = max(1, len(self.entries))
        self.idf = {w: math.log(total / (1 + c)) for w, c in doc_freq.items()}
        self.idf_default = math.log(total)

    @classmethod
    def load(cls, directory: Path | None = None) -> "ScriptureIndex":
        directory = directory or default_dir()
        bibles = {}
        for name in SOURCES:
            path = directory / f"{name}.json"
            if path.exists():
                bibles[name] = _load_file(path, name)
        return cls(bibles)

    def __bool__(self) -> bool:
        return bool(self.entries)

    # ------------------------------------------------------------------ lookups used by the follower
    def verse_texts(self, book_id: str, chapter: int, verse: int) -> list[str]:
        """Normalized wording of one verse in every loaded Bible (empty if it does not exist)."""
        return list(self.by_ref.get((book_id, chapter, verse), {}).values())

    def has_verse(self, book_id: str, chapter: int, verse: int) -> bool:
        return (book_id, chapter, verse) in self.by_ref

    def verse_obj(self, book_id: str, chapter: int, verse: int) -> Verse | None:
        for version, v, _ in self.entries:
            if v.book_id == book_id and v.chapter == chapter and v.verse == verse:
                return v
        return None

    # ------------------------------------------------------------------ recognise a verse from its words
    def _idf(self, word: str) -> float:
        return self.idf.get(word, self.idf_default)

    def _align(self, speech: list[str], verse: list[str]) -> tuple[int, float, float, int]:
        """Line the speech up with a verse in order. Returns (matched words, weight of matched / weight of speech,
        weight of matched / weight of verse, span of the verse the matches cover). Rare words weigh more than "the"."""
        ops = LCSseq.editops(speech, verse)
        gone_s = {op.src_pos for op in ops if op.tag == "delete"}
        gone_v = {op.dest_pos for op in ops if op.tag == "insert"}
        matched_s = [i for i in range(len(speech)) if i not in gone_s]
        matched_v = [j for j in range(len(verse)) if j not in gone_v]
        if not matched_s:
            return 0, 0.0, 0.0, 0
        w_matched = sum(self._idf(speech[i]) for i in matched_s)
        w_speech = sum(self._idf(w) for w in speech) or 1.0
        w_verse = sum(self._idf(w) for w in verse) or 1.0
        span = (matched_v[-1] - matched_v[0] + 1) if matched_v else 0
        return len(matched_s), w_matched / w_speech, w_matched / w_verse, span

    def match(self, speech: str, *, min_words: int = 8, min_cover: float = 0.62, margin: float = 0.08,
              prefer: tuple[str, int] | None = None) -> ContentMatch | None:
        """Find the one verse whose wording best matches ``speech``. None if there is none, or it is ambiguous.

        A verse counts when at least ``min_words`` of the speech's words line up with it in order, and those words are
        at least ``min_cover`` of the speech by weight (so a verse read aloud, or the start of one, qualifies, while a
        sentence that merely shares a phrase does not).
        """
        words = normalize(speech).split()
        if len(words) < min_words or not self.entries:
            return None
        votes: dict[int, int] = defaultdict(int)
        for i in range(len(words) - 2):
            gram = (words[i], words[i + 1], words[i + 2])
            if gram in self.common:
                continue
            for idx in self.grams.get(gram, ()):
                votes[idx] += 1
        if not votes:
            return None
        best_per_verse: dict[tuple[str, int, int], tuple[float, str, Verse, int]] = {}
        for idx, count in sorted(votes.items(), key=lambda kv: -kv[1])[:40]:
            version, verse, vnorm = self.entries[idx]
            vwords = vnorm.split()
            if len(vwords) < 7:
                continue                                   # very short verses ("Jesus wept.") are too easy to hit by chance
            matched, cover_speech, cover_verse, span = self._align(words, vwords)
            if matched < min_words or span > matched * 1.7 + 3:
                continue                                   # matches scattered across the verse are not a quotation
            # reading the verse (or its start) qualifies; so does a verse that sits whole inside a longer stretch of speech
            if cover_speech < min_cover and cover_verse < 0.8:
                continue
            score = (cover_speech + cover_verse) / 2 + matched / 100.0
            key = (verse.book_id, verse.chapter, verse.verse)
            if key not in best_per_verse or score > best_per_verse[key][0]:
                best_per_verse[key] = (score, version, verse, matched)
        if not best_per_verse:
            return None
        ranked = sorted(best_per_verse.values(), key=lambda x: -x[0])
        score, version, verse, matched = ranked[0]
        if len(ranked) > 1 and ranked[1][0] >= score - margin:
            if prefer is not None:
                tied = [r for r in ranked if r[0] >= score - margin]
                near = [r for r in tied if (r[2].book_id, r[2].chapter) == prefer]
                if len(near) == 1:
                    return ContentMatch(near[0][2], near[0][0], near[0][3], near[0][1])
            return None
        return ContentMatch(verse, score, matched, version)

    def opening_matches(self, speech_tail: str, book_id: str, chapter: int, verse: int, *, words: int = 6, min_score: float = 84.0) -> float:
        """How well the end of ``speech_tail`` matches the OPENING words of a given verse (0-100). Used to notice that
        the speaker has begun reading the next verse."""
        tail = " ".join(normalize(speech_tail).split()[-(words + 8):])
        best = 0.0
        for text in self.verse_texts(book_id, chapter, verse):
            opening = " ".join(text.split()[:words])
            if len(opening.split()) < 3:
                continue
            best = max(best, fuzz.partial_ratio(opening, tail))
        return best

    def reading_score(self, speech_tail: str, book_id: str, chapter: int, verse: int, *, last_words: int = 40) -> float:
        """0-100: how much of this verse's wording is in the most recent speech. High means the speaker is reading it."""
        tail = " ".join(normalize(speech_tail).split()[-last_words:])
        best = 0.0
        for text in self.verse_texts(book_id, chapter, verse):
            if tail and text:
                best = max(best, fuzz.partial_ratio(tail, text) if len(tail) <= len(text) else fuzz.partial_ratio(text, tail))
        return best
