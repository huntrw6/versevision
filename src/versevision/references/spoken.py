"""Turn what a speech recogniser hears into written Bible references.

Speech comes out as "John chapter three verse sixteen" or "first Corinthians thirteen four" or
"turn with me to Romans eight twenty eight". The reference parser wants "John 3:16". This module rewrites
the spoken form into the written form and then hands it to the normal parser, so every rule about books,
versions and spans stays in one place.

It is deliberately strict: a number word only becomes part of a reference when it follows a book name, so
"chapter three" or "ten minutes" on their own never trigger anything.
"""
from __future__ import annotations

import re
from dataclasses import dataclass

from .books import BOOKS, recognize_book
from .models import PassageReference
from .parser import parse_references

_UNITS = {"zero": 0, "oh": 0, "o": 0, "one": 1, "two": 2, "three": 3, "four": 4, "five": 5, "six": 6, "seven": 7,
          "eight": 8, "nine": 9, "ten": 10, "eleven": 11, "twelve": 12, "thirteen": 13, "fourteen": 14, "fifteen": 15,
          "sixteen": 16, "seventeen": 17, "eighteen": 18, "nineteen": 19}
_TENS = {"twenty": 20, "thirty": 30, "forty": 40, "fourty": 40, "fifty": 50, "sixty": 60, "seventy": 70, "eighty": 80, "ninety": 90}
_ORDINALS = {"first": "1", "second": "2", "third": "3", "1st": "1", "2nd": "2", "3rd": "3"}
_NUMBER_WORDS = set(_UNITS) | set(_TENS) | {"hundred", "and"}
_FILLER = {"chapter", "chapters", "verse", "verses", "versus", "vs", "colon", "dash", "through", "to", "thru"}


def _read_number(words: list[str], i: int) -> tuple[int, int] | None:
    """Read a spoken number starting at words[i]. Returns (value, next_index) or None.

    Handles "sixteen", "twenty eight", "one hundred nineteen", "one twenty" (=120 only inside psalm range rules
    is left to the caller), and plain digits.
    """
    if i >= len(words):
        return None
    w = words[i]
    if w.isdigit():
        return int(w), i + 1
    if w in _TENS:
        value, j = _TENS[w], i + 1
        if j < len(words) and words[j] in _UNITS and 1 <= _UNITS[words[j]] <= 9:
            return value + _UNITS[words[j]], j + 1
        return value, j
    if w == "one" and i + 2 < len(words) and words[i + 1] in {"oh", "o", "zero"} and words[i + 2] in _UNITS and 1 <= _UNITS[words[i + 2]] <= 9:
        return 100 + _UNITS[words[i + 2]], i + 3          # "one oh five" = 105
    if w in _UNITS:
        value, j = _UNITS[w], i + 1
        if j < len(words) and words[j] == "hundred":
            value *= 100
            j += 1
            if j < len(words) and words[j] == "and":
                j += 1
            rest = _read_number(words, j)
            if rest and rest[0] < 100:
                return value + rest[0], rest[1]
            return value, j
        return value, j
    return None


def _book_at(words: list[str], i: int) -> tuple[str, int] | None:
    """If a book name starts at words[i] return (canonical display name, next_index). Tries 3, 2 then 1 word."""
    ordinal = ""
    j = i
    if j < len(words) and words[j] in _ORDINALS:
        ordinal = _ORDINALS[words[j]]
        j += 1
    elif j < len(words) and words[j] in {"1", "2", "3"}:
        ordinal = words[j]
        j += 1
    elif j < len(words) and words[j] in {"i", "ii", "iii"}:
        ordinal = str(len(words[j]))
        j += 1
    for size in (3, 2, 1):
        chunk = words[j:j + size]
        if len(chunk) != size:
            continue
        name = " ".join(chunk)
        if name in {"song of solomon", "song of songs"}:
            return "Song of Solomon", j + size
        if size == 1 and name in _ORDINALS or size == 1 and name in _NUMBER_WORDS:
            continue
        candidate = f"{ordinal} {name}".strip()
        hit = recognize_book(candidate, 0)
        # the whole candidate must be the book name (not just a prefix of it)
        if hit is not None and hit[1] >= len(candidate) and (not ordinal or hit[0].name[:1].isdigit()):
            return hit[0].name, j + size
    return None


@dataclass
class SpokenReference:
    text: str                 # the written form we built, e.g. "John 3:16"
    heard: str                # the words it came from
    reference: PassageReference


_CLEAN = re.compile(r"[^a-z0-9: ]+")


_HOMOPHONES = {"for": "four", "fore": "four", "to": "two", "too": "two", "won": "one", "ate": "eight", "tree": "three"}


_CONTINUERS = {"and", "then", "also", "next", "or", "versus", "compare", "see", "plus", "says", "said", "tells", "teaches", "reads", "is", "was",
               "through", "thru", "to", "dash"}


def _reference_ends_here(words: list[str], k: int) -> bool:
    """True when what follows the sound-alike looks like the end of a reference, not the start of a sentence.

    The reference is over if the sentence ends there, or the next word joins references/introduces a comment
    ("and", "then", "says"), or another book name starts. "John 3 for God so loved" fails all three, so is left alone.
    """
    if k >= len(words):
        return True
    if words[k] in _CONTINUERS:
        return True
    return _book_at(words, k) is not None


def _fix_homophones(words: list[str]) -> list[str]:
    """"Habakkuk 2 for" is "Habakkuk 2:4". Only a word that comes right after book + chapter number is changed,
    so ordinary sentences ("pray for us", "to the church") are never touched."""
    out = list(words)
    i = 0
    while i < len(out):
        hit = _book_at(out, i)
        if not hit:
            i += 1
            continue
        _, j = hit
        while j < len(out) and out[j] in {"chapter", "chapters"}:
            j += 1
        chap = _read_number(out, j)
        if chap:
            k = chap[1]
            while k < len(out) and out[k] in {"verse", "verses"}:
                k += 1
            if k < len(out) and out[k] in _HOMOPHONES and _reference_ends_here(out, k + 1):
                out[k] = _HOMOPHONES[out[k]]
        i = max(i + 1, j)
    return out


def _words(text: str) -> list[str]:
    text = text.lower()
    # Whisper writes verses as "2.4" or "13.4" or "Isaiah 40-31". Turn those into chapter:verse first.
    text = re.sub(r"(?<![\d.])(\d{1,3})\.(\d{1,3})(?![\d.]\d)", r"\1:\2", text)
    # Right after a book name, "2-4" / "8-28" / "40-31" is chapter-verse (a speaker says "Habakkuk two four"; a speaker who
    # means chapters says "chapters two through four"). Elsewhere a dash between numbers is a range of verses.
    text = re.sub(r"([a-z]\.?\s+)(\d{1,3})\s*[-–—]\s*(\d{1,3})\b(?!\s*:)", r"\1\2:\3", text)
    text = re.sub(r"(\d)\s*[-–—]\s*(\d)", r"\1 through \2", text)
    text = text.replace("-", " ").replace("–", " ").replace("—", " ")
    text = re.sub(r"(\d)\s*:\s*(\d)", r"\1:\2", text)
    return _fix_homophones(_CLEAN.sub(" ", text).split())


# Versions as people say them after a reference: "in the NIV", "N I V", "New Living Translation", "King James".
_SPOKEN_VERSIONS = [
    (("new", "international", "version"), "NIV"), (("english", "standard", "version"), "ESV"), (("new", "living", "translation"), "NLT"),
    (("new", "king", "james"), "NKJV"), (("king", "james"), "KJV"), (("new", "american", "standard", "bible"), "NASB1995"), (("new", "american", "standard"), "NASB1995"),
    (("amplified", "classic"), "AMPC"), (("amplified",), "AMP"),
    (("n", "i", "v"), "NIV"), (("e", "s", "v"), "ESV"), (("n", "l", "t"), "NLT"), (("n", "k", "j", "v"), "NKJV"), (("k", "j", "v"), "KJV"),
    (("n", "a", "s", "b"), "NASB1995"), (("niv",), "NIV"), (("esv",), "ESV"), (("nlt",), "NLT"), (("nkjv",), "NKJV"), (("kjv",), "KJV"),
    (("nasb",), "NASB1995"), (("nasb95",), "NASB1995"), (("nasb1995",), "NASB1995"), (("ampc",), "AMPC"),
]
_VERSION_LEAD = {"in", "the", "from", "of", "version", "translation", "reading", "using", "its", "say", "says"}


def _version_at(words: list[str], n: int) -> tuple[str, int] | None:
    """A version named in the first few words after a reference. Returns (abbreviation, next index)."""
    k = n
    while k < len(words) and k < n + 3 and words[k] in _VERSION_LEAD:
        k += 1
    for phrase, abbr in _SPOKEN_VERSIONS:
        if tuple(words[k:k + len(phrase)]) == phrase:
            return abbr, k + len(phrase)
    return None


def _emit(found: list, words: list[str], start: int, written: str, n: int) -> int:
    ver = None if re.search(r"\b[A-Z]{2,6}\d{0,4}\s*$", written) else _version_at(words, n)    # one already written next to it ("John 3:16 NIV")
    if ver:
        written, n = f"{written} {ver[0]}", ver[1]
    found.append((written, " ".join(words[start:n])))
    return n


def spoken_to_written(text: str) -> list[tuple[str, str]]:
    """Find book + chapter (+ verse [through verse]) groups in speech. Returns [(written, heard_words)]."""
    words = _words(text)
    found: list[tuple[str, str]] = []
    i = 0
    while i < len(words):
        hit = _book_at(words, i)
        if not hit:
            i += 1
            continue
        book, j = hit
        start = i
        # optional filler: "chapter"
        while j < len(words) and words[j] in {"chapter", "chapters"}:
            j += 1
        # digits already written by the recogniser, e.g. "John 3:16"
        if j < len(words) and re.fullmatch(r"\d+:\d+", words[j]):
            written = f"{book} {words[j]}"
            j += 1
            tail = _range_tail(words, j, words[j - 1].split(":")[0])
            if tail:
                written, j = written + tail[0], tail[1]
            j = _emit(found, words, start, written, j)
            i = j
            continue
        if j < len(words) and words[j].isdigit() and len(words[j]) >= 3 and (j + 1 >= len(words) or not words[j + 1].isdigit()):
            split = _split_run_together(book, words[j])
            if split:
                written = f"{book} {split[0]}:{split[1]}"
                tail = _range_tail(words, j + 1, str(split[0]))
                n = j + 1
                if tail:
                    written, n = written + tail[0], tail[1]
                n = _emit(found, words, start, written, n)
                i = n
                continue
        chap = None
        if book == "Psalms" and j < len(words) and words[j] == "one":      # "one nineteen" = Psalm 119, "one fifty" = 150
            rest = _read_number(words, j + 1)
            if rest and 10 <= rest[0] <= 50:
                chap = (100 + rest[0], rest[1])
        chap = chap or _read_number(words, j)
        if not chap:
            i += 1
            continue
        chapter, k = chap
        # chapter then verse: "chapter three verse sixteen" or "three sixteen" or "three colon sixteen"
        m = k
        had_verse_word = False
        while m < len(words) and words[m] in {"verse", "verses", "versus", "vs", "colon"}:
            m += 1
            had_verse_word = True
        verse = _read_number(words, m)
        if verse and (had_verse_word or _plausible_bare(book, chapter, verse[0])):
            v, n = verse
            written = f"{book} {chapter}:{v}"
            tail = _range_tail(words, n, str(chapter))
            if tail:
                written, n = written + tail[0], tail[1]
            n = _emit(found, words, start, written, n)
            i = n
            continue
        if had_verse_word or (k < len(words) and words[k - 1] == "chapter"):
            i = k
            continue
        # book + a single number with no verse ("Jude 24", or just "Psalm 23"): only accept for one-chapter books
        if book in {"Obadiah", "Philemon", "2 John", "3 John", "Jude"}:
            found.append((f"{book} 1:{chapter}", " ".join(words[start:k])))
            i = k
            continue
        i = k
    return found


def _split_run_together(book: str, digits: str) -> tuple[int, int] | None:
    """The recogniser often writes "eight twenty eight" as 828. Split it into chapter and verse using the real
    number of chapters in the book. Returns None unless exactly one split is sensible, so ambiguity never guesses."""
    chapters = next((b.chapters for b in BOOKS.values() if b.name == book), None) if isinstance(BOOKS, dict) else next((b.chapters for b in BOOKS if b.name == book), None)
    if not chapters or not digits.isdigit() or len(digits) < 3 or len(digits) > 6:
        return None
    options = []
    for cut in range(1, len(digits)):
        chap, verse = digits[:cut], digits[cut:]
        if chap.startswith("0") or verse.startswith("0"):
            continue
        if 1 <= int(chap) <= chapters and 1 <= int(verse) <= 176:
            options.append((int(chap), int(verse)))
    return options[0] if len(options) == 1 else None


def _plausible_bare(book: str, chapter: int, verse: int) -> bool:
    """"John three sixteen": a bare second number is the verse if it is a sensible verse number."""
    return 1 <= verse <= 176 and 1 <= chapter <= 150


def _range_tail(words: list[str], j: int, chapter: str) -> tuple[str, int] | None:
    """After a verse: "through twenty", "to verse 20", "dash 20" -> ("-20", next_index)."""
    if j < len(words) and words[j] in {"through", "thru", "to", "dash"}:
        k = j + 1
        while k < len(words) and words[k] in {"verse", "verses"}:
            k += 1
        num = _read_number(words, k)
        if num:
            return f"-{num[0]}", num[1]
    return None


def find_spoken_references(text: str) -> list[SpokenReference]:
    """Spoken or already-written references found in a stretch of speech, in the order heard."""
    out: list[SpokenReference] = []
    for written, heard in spoken_to_written(text):
        parsed = parse_references(written)
        for ref in getattr(parsed, "references", None) or []:
            out.append(SpokenReference(text=ref.normalized_text, heard=heard, reference=ref))
    return out


@dataclass
class ChapterMention:
    """A book and ONE number with no verse ("Isaiah 42"): either a chapter, or a chapter and verse the recogniser ran together."""
    book_id: str
    book: str
    number: int
    heard: str                 # the words it came from
    end: int                   # index of the word just after the mention


def find_chapter_only(text: str) -> list[ChapterMention]:
    """Book + a single number that is NOT followed by a verse, in the order heard. References with a verse, ranges, "chapter N"
    said outright, and one-chapter books ("Jude 24") are not returned: they are already understood."""
    words = _words(text)
    out: list[ChapterMention] = []
    i = 0
    while i < len(words):
        hit = _book_at(words, i)
        if not hit:
            i += 1
            continue
        book, j = hit
        start = i
        said_chapter = j < len(words) and words[j] in {"chapter", "chapters"}
        if said_chapter:
            j += 1
        if j >= len(words) or ":" in words[j] or (words[j].isdigit() and len(words[j]) >= 3 and _split_run_together(book, words[j])):
            i = max(j, i + 1)
            continue
        num = _read_number(words, j)
        if not num:
            i = max(j, i + 1)
            continue
        number, k = num
        nxt = _read_number(words, k) if k < len(words) else None
        followed_by_verse = (k < len(words) and words[k] in {"verse", "verses", "versus", "vs", "colon", "through", "to", "thru", "dash"}) or nxt is not None
        if said_chapter or followed_by_verse or book in {"Obadiah", "Philemon", "2 John", "3 John", "Jude"}:
            i = max(k, i + 1)
            continue
        info = next((b for b in BOOKS if b.name == book), None)
        if info and 1 <= number <= info.chapters:
            out.append(ChapterMention(info.id, book, number, " ".join(words[start:k]), k))
        i = max(k, i + 1)
    return out


def verse_splits(number: int, chapters: int) -> list[tuple[int, int]]:
    """Ways a spoken "chapter and verse" can be heard as ONE number, most likely first.

    "Isaiah forty two" is a tens word plus a digit: chapter 40, verse 2. "Isaiah four two" is two digits: chapter 4, verse 2.
    Longer numbers ("one nineteen", 119) split anywhere. The whole number as a chapter is not returned."""
    out: list[tuple[int, int]] = []
    def add(c: int, v: int) -> None:
        if 1 <= c <= chapters and 1 <= v <= 176 and (c, v) not in out:
            out.append((c, v))
    if 21 <= number <= 99 and number % 10:
        add(number // 10 * 10, number % 10)               # forty two -> 40:2
    digits = str(number)
    for cut in range(1, len(digits)):
        chap, verse = digits[:cut], digits[cut:]
        if not chap.startswith("0") and not verse.startswith("0"):
            add(int(chap), int(verse))                     # 42 -> 4:2 ; 119 -> 1:19, 11:9
    return out
