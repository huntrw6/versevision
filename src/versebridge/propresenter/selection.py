from rapidfuzz import fuzz, process

from versebridge.errors import ConfigurationError
from .models import PresentationSummary


def select_unique_presentation(
    presentations: list[PresentationSummary], *, uuid: str | None = None, name: str | None = None, library: str | None = None
) -> PresentationSummary:
    candidates = presentations
    if library:
        candidates = [item for item in candidates if (item.library_name or "").casefold() == library.casefold()]
    if uuid:
        matches = [item for item in candidates if item.uuid.casefold() == uuid.casefold()]
    elif name:
        matches = [item for item in candidates if item.name.casefold() == name.casefold()]
    else:
        raise ConfigurationError("A target UUID or presentation name is required in non-interactive mode")
    if len(matches) == 1:
        return matches[0]
    if len(matches) > 1:
        locations = ", ".join(f"{item.library_name}/{item.name} ({item.uuid})" for item in matches)
        raise ConfigurationError(f"Presentation target is ambiguous: {locations}")
    suggestions = process.extract(name or uuid or "", [item.name for item in candidates], scorer=fuzz.WRatio, limit=3)
    hint = ", ".join(value for value, score, _ in suggestions if score >= 60)
    raise ConfigurationError(f"Presentation target was not found" + (f". Possible matches: {hint}" if hint else ""))

