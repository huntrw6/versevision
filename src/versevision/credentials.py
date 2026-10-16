from __future__ import annotations

import os
from pathlib import Path

from platformdirs import user_config_path


def keys_file() -> Path:
    override = os.getenv("VERSEVISION_KEYS")
    return Path(override).expanduser() if override else user_config_path("versevision") / "keys.env"


def _from_keys_file(name: str) -> str | None:
    """Read NAME=value from the private keys file (so background services need no shell setup)."""
    path = keys_file()
    try:
        lines = path.read_text(encoding="utf-8").splitlines()
    except OSError:
        return None
    for line in lines:
        line = line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, _, value = line.partition("=")
        if key.strip() == name:
            value = value.strip().strip("\"'")
            return value or None
    return None


def provider_api_key(environment_name: str, bundled_name: str) -> str | None:
    """Environment first, then the keys file, then an optional build-time bundled key."""
    value = os.getenv(environment_name) or _from_keys_file(environment_name)
    if value:
        return value
    try:
        from versevision import _bundled_credentials
    except ImportError:
        return None
    bundled = getattr(_bundled_credentials, bundled_name, None)
    return bundled if isinstance(bundled, str) and bundled else None
