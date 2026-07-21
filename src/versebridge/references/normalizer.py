import unicodedata

_DASHES = str.maketrans({"–": "-", "—": "-", "‒": "-", "−": "-"})


def normalize_fragment(text: str) -> str:
    value = unicodedata.normalize("NFKC", text).replace("\u00a0", " ").translate(_DASHES)
    value = value.replace("＆", "&")
    return value

