from .models import BibleVersion


_NAMES = {
    "NLT": "New Living Translation", "NIV": "New International Version",
    "ESV": "English Standard Version", "KJV": "King James Version",
    "NKJV": "New King James Version", "NASB": "New American Standard Bible",
    "CSB": "Christian Standard Bible", "NRSV": "New Revised Standard Version",
    "RSV": "Revised Standard Version", "AMP": "Amplified Bible",
    "MSG": "The Message", "NET": "New English Translation", "WEB": "World English Bible",
    "ASV": "American Standard Version",
}

VERSIONS = {value.casefold(): value for value in _NAMES}
VERSION_MODELS = {key: BibleVersion(abbreviation=key, name=name) for key, name in _NAMES.items()}


def canonical_version(value: str) -> str | None:
    return VERSIONS.get(value.casefold())


def known_versions() -> list[BibleVersion]:
    return sorted(VERSION_MODELS.values(), key=lambda item: item.abbreviation)
