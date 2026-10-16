"""Remember what each import added so a rerun can replace it and a mistake can be undone."""
from __future__ import annotations

from datetime import datetime, timezone
import json
import os
from pathlib import Path
import tempfile

from filelock import FileLock
from platformdirs import user_data_path
from pydantic import BaseModel, Field


def history_path() -> Path:
    override = os.getenv("VERSEBRIDGE_HISTORY")
    return Path(override).expanduser() if override else user_data_path("versebridge") / "imports.json"


class ImportRecord(BaseModel):
    import_id: str
    created: datetime = Field(default_factory=lambda: datetime.now(timezone.utc))
    source_name: str
    source_sha256: str
    target_uuid: str
    target_name: str
    node: str = "local"
    cue_uuids: list[str] = Field(default_factory=list)
    slide_count: int = 0
    verse_count: int = 0
    point_count: int = 0
    backup_path: str | None = None
    status: str = "active"          # active | replaced | undone | rolled_back
    replaces: str | None = None


class HistoryStore:
    def __init__(self, path: Path | None = None):
        self.path = path or history_path()

    def _lock(self) -> FileLock:
        self.path.parent.mkdir(parents=True, exist_ok=True)
        return FileLock(str(self.path) + ".lock", timeout=10)

    def load(self) -> list[ImportRecord]:
        if not self.path.exists():
            return []
        try:
            return [ImportRecord.model_validate(item) for item in json.loads(self.path.read_text("utf-8"))]
        except (OSError, ValueError):
            return []

    def _save(self, records: list[ImportRecord]) -> None:
        with tempfile.NamedTemporaryFile("w", encoding="utf-8", dir=self.path.parent, prefix=".imports-", suffix=".tmp", delete=False) as stream:
            json.dump([item.model_dump(mode="json") for item in records], stream, indent=2)
            temp = Path(stream.name)
        os.replace(temp, self.path)

    def add(self, record: ImportRecord) -> None:
        with self._lock():
            records = self.load()
            records.append(record)
            self._save(records)

    def set_status(self, import_id: str, status: str) -> None:
        with self._lock():
            records = self.load()
            for item in records:
                if item.import_id == import_id:
                    item.status = status
            self._save(records)

    def get(self, import_id: str) -> ImportRecord | None:
        return next((item for item in self.load() if item.import_id == import_id), None)

    def latest_active(self, *, target_uuid: str, source_name: str | None = None, node: str | None = None) -> ImportRecord | None:
        matches = [
            item for item in self.load()
            if item.status == "active" and item.target_uuid.casefold() == target_uuid.casefold()
            and (source_name is None or item.source_name == source_name) and (node is None or item.node == node)
        ]
        return matches[-1] if matches else None
