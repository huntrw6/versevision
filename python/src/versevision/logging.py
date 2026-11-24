from __future__ import annotations

from datetime import datetime, timezone
import json
import logging
from pathlib import Path

from pydantic import BaseModel, Field


_SECRET_KEYS = ("api_key", "authorization", "password", "secret", "token")


def redact_fields(values: dict) -> dict:
    """Return log-safe metadata without mutating the caller's dictionary."""
    return {
        key: "[REDACTED]" if any(secret in key.casefold() for secret in _SECRET_KEYS) else value
        for key, value in values.items()
    }


def configure_logging(verbose: bool = False) -> None:
    logging.basicConfig(
        level=logging.DEBUG if verbose else logging.INFO,
        format="%(asctime)s %(levelname)s %(name)s %(message)s",
    )


class AuditEntry(BaseModel):
    timestamp: datetime = Field(default_factory=lambda: datetime.now(timezone.utc))
    application_version: str
    propresenter_version: str | None = None
    target_uuid: str | None = None
    presentation_name: str | None = None
    reference_count: int = Field(ge=0)
    slide_count: int = Field(ge=0)
    versions: list[str] = Field(default_factory=list)
    backup_path: Path | None = None
    verification_result: str


def append_audit_entry(path: Path, entry: AuditEntry) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("a", encoding="utf-8") as stream:
        stream.write(json.dumps(entry.model_dump(mode="json"), ensure_ascii=False) + "\n")
