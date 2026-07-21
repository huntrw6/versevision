from pathlib import Path
import shutil

from versebridge.presentation import ProtobufPresentationWriter
from versebridge.presentation.backup import restore_backup


FIXTURE = Path(__file__).parents[1] / "fixtures" / "propresenter19" / "Verses.original.pro"


def test_append_verify_and_restore(tmp_path):
    target = tmp_path / "Verses.pro"
    shutil.copy2(FIXTURE, target)
    original = target.read_bytes()
    writer = ProtobufPresentationWriter()

    result = writer.append_synthetic_slide(target, "Synthetic verse-like text", "VerseBridge Test")

    assert result.cue_count_before == 1
    assert result.cue_count_after == 2
    assert writer.verify(target, result).valid
    assert result.backup_path.read_bytes() == original

    restore_backup(target, result.backup_path)
    assert target.read_bytes() == original


def test_unknown_application_version_is_rejected_without_changes(tmp_path):
    target = tmp_path / "Verses.pro"
    shutil.copy2(FIXTURE, target)
    original = target.read_bytes()
    writer = ProtobufPresentationWriter()
    message = writer._read(target)
    message.application_info.application_version.major_version = 99
    target.write_bytes(message.SerializeToString(deterministic=True))
    unsupported = target.read_bytes()

    try:
        writer.append_synthetic_slide(target, "text", "label")
    except ValueError as exc:
        assert "Unsupported ProPresenter version" in str(exc)
    else:
        raise AssertionError("unsupported document was accepted")

    assert target.read_bytes() == unsupported
    assert not list(tmp_path.glob("*.versebridge-backup-*"))
