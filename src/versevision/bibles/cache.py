from __future__ import annotations

from datetime import datetime, timezone
import hashlib
from pathlib import Path
import sqlite3

from platformdirs import user_cache_path

from .models import BibleVerse


def default_cache_path() -> Path:
    return user_cache_path("versevision") / "scripture.sqlite3"


class ScriptureCache:
    def __init__(self, path: Path | None = None):
        self.path = path or default_cache_path()
        self.path.parent.mkdir(parents=True, exist_ok=True)
        self.connection = sqlite3.connect(self.path)
        self.connection.row_factory = sqlite3.Row
        self.connection.execute("""
            CREATE TABLE IF NOT EXISTS verses (
                provider TEXT NOT NULL,
                provider_version_id TEXT NOT NULL,
                book_id TEXT NOT NULL,
                chapter INTEGER NOT NULL,
                verse INTEGER NOT NULL,
                version TEXT NOT NULL,
                text TEXT NOT NULL,
                paragraph_start INTEGER NOT NULL,
                poetry_indent INTEGER NOT NULL,
                copyright TEXT,
                attribution TEXT,
                retrieved_at TEXT NOT NULL,
                content_hash TEXT NOT NULL,
                PRIMARY KEY (provider, provider_version_id, book_id, chapter, verse)
            )
        """)
        self.connection.commit()

    def close(self) -> None:
        self.connection.close()

    def __enter__(self):
        return self

    def __exit__(self, *_):
        self.close()

    def get(self, provider: str, provider_version_id: str, book_id: str, chapter: int, verse: int) -> BibleVerse | None:
        row = self.connection.execute(
            "SELECT * FROM verses WHERE provider=? AND provider_version_id=? AND book_id=? AND chapter=? AND verse=?",
            (provider, provider_version_id, book_id, chapter, verse),
        ).fetchone()
        if not row:
            return None
        return BibleVerse(
            book_id=row["book_id"], chapter=row["chapter"], verse=row["verse"], text=row["text"],
            version=row["version"], paragraph_start=bool(row["paragraph_start"]),
            poetry_indent=row["poetry_indent"], copyright=row["copyright"], attribution=row["attribution"],
        )

    def put(self, provider: str, provider_version_id: str, value: BibleVerse) -> None:
        digest = hashlib.sha256(value.text.encode("utf-8")).hexdigest()
        self.connection.execute("""
            INSERT OR REPLACE INTO verses VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        """, (
            provider, provider_version_id, value.book_id, value.chapter, value.verse, value.version,
            value.text, int(value.paragraph_start), value.poetry_indent, value.copyright, value.attribution,
            datetime.now(timezone.utc).isoformat(), digest,
        ))
        self.connection.commit()

    def count(self) -> int:
        return int(self.connection.execute("SELECT COUNT(*) FROM verses").fetchone()[0])

    def clear(self) -> int:
        count = self.count()
        self.connection.execute("DELETE FROM verses")
        self.connection.commit()
        return count
