from __future__ import annotations

import os


def provider_api_key(environment_name: str, bundled_name: str) -> str | None:
    """Return a runtime override first, then an optional build-time bundled key."""
    value = os.getenv(environment_name)
    if value:
        return value
    try:
        from versebridge import _bundled_credentials
    except ImportError:
        return None
    bundled = getattr(_bundled_credentials, bundled_name, None)
    return bundled if isinstance(bundled, str) and bundled else None
