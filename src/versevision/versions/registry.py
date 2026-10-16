from __future__ import annotations

from .models import BibleVersion

_NAMES = {
    "NLT": "New Living Translation", "NIV": "New International Version",
    "ESV": "English Standard Version", "KJV": "King James Version",
    "NKJV": "New King James Version", "NASB": "New American Standard Bible",
    "NASB1995": "New American Standard Bible 1995", "NASB2020": "New American Standard Bible 2020",
    "CSB": "Christian Standard Bible", "NRSV": "New Revised Standard Version",
    "RSV": "Revised Standard Version", "AMP": "Amplified Bible",
    "MSG": "The Message", "NET": "New English Translation", "WEB": "World English Bible",
    "ASV": "American Standard Version", "TPT": "The Passion Translation",
    "NIRV": "New International Reader's Version", "BSB": "Berean Standard Bible",
    "LSV": "Literal Standard Version", "YLT": "Young's Literal Translation",
}

# Real translations that are not necessarily available from a configured
# provider. A label in this set is NEVER spell-corrected into something else;
# if no provider has it, the verse is reported as skipped instead.
REAL_VERSIONS = frozenset(_NAMES) | frozenset({
    "HCSB", "NCV", "GNT", "GNB", "CEV", "ISV", "LSB", "MEV", "NABRE", "NAB", "NJB", "JB",
    "DARBY", "DRA", "GNV", "GW", "ICB", "NIRV", "NIVUK", "NLV", "NOG", "NRSVCE", "ERV",
    "EHV", "EXB", "GWT", "TLB", "TLV", "VOICE", "WYC", "TNIV", "NCB", "CJB", "OJB",
    "TOJB", "RV", "RWV", "WMB", "WEBBE", "FBV", "CPDV", "JUB", "ACV", "BRG", "AKJV",
    "KJ21", "TCENT", "T4T", "PEV", "EASY", "LXX", "NABRE", "UKJV", "NMB", "PHILLIPS",
    "NEB", "REB", "NJPS", "TANAKH", "CEB", "CEV", "ETRV", "NIV84", "NIV2011", "NIV11",
})

# Spelling variants / exact editions that map to one canonical label.
ALIASES = {
    "NASB": "NASB2020",       # plain NASB means the 2020 edition
    "NASB95": "NASB1995", "NASB1995": "NASB1995", "NASB20": "NASB2020", "NASB2020": "NASB2020",
    "NIV11": "NIV", "NIV2011": "NIV",
}

# Operator overrides: label typed in documents -> version actually meant.
# Applied before the real-version protection, always with a warning.
DEFAULT_USER_ALIASES = {"YLT": "NLT"}

VERSIONS = {value.casefold(): value for value in _NAMES}
VERSION_MODELS = {key: BibleVersion(abbreviation=key, name=name) for key, name in _NAMES.items()}


def canonical_version(value: str) -> str | None:
    key = value.casefold()
    for alias, target in ALIASES.items():
        if alias.casefold() == key:
            return target
    return VERSIONS.get(key)


def is_real_version(value: str) -> bool:
    return value.upper() in REAL_VERSIONS or canonical_version(value) is not None


def _edit_distance_one(a: str, b: str) -> bool:
    """True when strings differ by one insert/delete/substitute/adjacent swap."""
    if a == b:
        return True
    if abs(len(a) - len(b)) > 1:
        return False
    if len(a) == len(b):
        diffs = [i for i in range(len(a)) if a[i] != b[i]]
        if len(diffs) == 1:
            return True
        return len(diffs) == 2 and diffs[1] == diffs[0] + 1 and a[diffs[0]] == b[diffs[1]] and a[diffs[1]] == b[diffs[0]]
    short, long_ = (a, b) if len(a) < len(b) else (b, a)
    return any(long_[:i] + long_[i + 1:] == short for i in range(len(long_)))


def correct_version(value: str, available: set[str] | None = None) -> str | None:
    """Spell-correct an unrecognized version label.

    Returns None when the label is a real version (kept as typed, never altered)
    or when there is no single unambiguous near match.
    """
    token = value.upper()
    if is_real_version(token):
        return None
    pool = {item.upper() for item in (available or set())} | {key for key in _NAMES}
    matches = {canonical_version(item) or item for item in pool if len(item) >= 3 and _edit_distance_one(token, item)}
    return next(iter(matches)) if len(matches) == 1 else None


def known_versions() -> list[BibleVersion]:
    return sorted(VERSION_MODELS.values(), key=lambda item: item.abbreviation)
