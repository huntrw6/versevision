from datetime import datetime, timezone
import os
from pathlib import Path
import shutil
import tempfile


def create_backup(path: Path) -> Path:
    stamp = datetime.now(timezone.utc).strftime("%Y%m%d-%H%M%S")
    backup = path.with_name(f"{path.name}.versebridge-backup-{stamp}")
    counter = 1
    while backup.exists():
        backup = path.with_name(f"{path.name}.versebridge-backup-{stamp}-{counter}")
        counter += 1
    shutil.copy2(path, backup)
    return backup


def restore_backup(path: Path, backup: Path) -> None:
    path = path.resolve()
    backup = backup.resolve(strict=True)
    if backup.parent != path.parent:
        raise ValueError("backup must be in the same directory as the presentation")
    temp_path: Path | None = None
    try:
        with tempfile.NamedTemporaryFile(
            mode="wb", prefix=f".{path.name}.versebridge-restore-", suffix=".tmp", dir=path.parent, delete=False
        ) as stream, backup.open("rb") as source:
            temp_path = Path(stream.name)
            shutil.copyfileobj(source, stream)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temp_path, path)
        temp_path = None
    finally:
        if temp_path and temp_path.exists():
            temp_path.unlink()
