//! `presentation/backup.py`: the copy VerseVision keeps beside a presentation before it changes it, and putting it back.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::{PpError, Result};

/// `datetime.now(timezone.utc).strftime("%Y%m%d-%H%M%S")` for a Unix time (seconds, UTC).
pub fn stamp(epoch_seconds: i64) -> String {
    let days = epoch_seconds.div_euclid(86_400);
    let rest = epoch_seconds.rem_euclid(86_400);
    // civil-from-days (Howard Hinnant)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };
    format!(
        "{year:04}{month:02}{day:02}-{:02}{:02}{:02}",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

/// The name of a backup: `<file>.versevision-backup-<stamp>`, then `-1`, `-2`, ... while the name is taken.
pub fn backup_name(file_name: &str, stamp: &str, counter: u32) -> String {
    if counter == 0 {
        format!("{file_name}.versevision-backup-{stamp}")
    } else {
        format!("{file_name}.versevision-backup-{stamp}-{counter}")
    }
}

pub fn now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// `create_backup(path)`: copy the file (with its modification time) beside it. `now` is the Unix time in seconds.
pub fn create_backup(path: &Path, now: i64) -> Result<PathBuf> {
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| PpError::value("presentation path has no file name"))?;
    let stamp = stamp(now);
    let mut counter = 0;
    let mut backup = path.with_file_name(backup_name(file_name, &stamp, counter));
    while backup.exists() {
        counter += 1;
        backup = path.with_file_name(backup_name(file_name, &stamp, counter));
    }
    fs::copy(path, &backup).map_err(PpError::io)?;
    if let Ok(modified) = fs::metadata(path).and_then(|m| m.modified()) {
        if let Ok(file) = fs::OpenOptions::new().write(true).open(&backup) {
            let _ = file.set_modified(modified); // `copy2` keeps the time; a file system that cannot is not a reason to fail
        }
    }
    Ok(backup)
}

/// `restore_backup(path, backup)`: put the backup back, atomically (a temporary file beside the target, then a rename).
pub fn restore_backup(path: &Path, backup: &Path) -> Result<()> {
    let path = fs::canonicalize(path).or_else(|_| {
        // the target may be gone; resolve its folder instead
        let parent = path.parent().unwrap_or(Path::new("."));
        fs::canonicalize(if parent.as_os_str().is_empty() {
            Path::new(".")
        } else {
            parent
        })
        .map(|p| p.join(path.file_name().unwrap_or_default()))
    });
    let path = path.map_err(PpError::io)?;
    let backup = fs::canonicalize(backup).map_err(PpError::io)?;
    if backup.parent() != path.parent() {
        return Err(PpError::value(
            "backup must be in the same directory as the presentation",
        ));
    }
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("presentation");
    let temp = path.with_file_name(format!(
        ".{name}.versevision-restore-{}.tmp",
        uuid::Uuid::new_v4().simple()
    ));
    let result = (|| -> Result<()> {
        fs::copy(&backup, &temp).map_err(PpError::io)?;
        fs::File::open(&temp)
            .and_then(|f| f.sync_all())
            .map_err(PpError::io)?;
        fs::rename(&temp, &path).map_err(PpError::io)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamps_are_utc_and_zero_padded() {
        assert_eq!(stamp(0), "19700101-000000");
        assert_eq!(stamp(1_784_419_571), "20260719-000611");
        assert_eq!(stamp(951_782_400), "20000229-000000"); // a leap day
        assert_eq!(stamp(1_700_000_000), "20231114-221320");
    }

    #[test]
    fn names_count_up_while_taken() {
        assert_eq!(
            backup_name("Verses.pro", "20260719-000611", 0),
            "Verses.pro.versevision-backup-20260719-000611"
        );
        assert_eq!(
            backup_name("Verses.pro", "20260719-000611", 2),
            "Verses.pro.versevision-backup-20260719-000611-2"
        );
    }

    #[test]
    fn a_backup_is_a_copy_next_to_the_file_and_never_overwrites() {
        let dir = std::env::temp_dir().join(format!("vv-backup-{}", uuid::Uuid::new_v4().simple()));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("Verses.pro");
        fs::write(&file, b"one").unwrap();
        let first = create_backup(&file, 1_784_419_571).unwrap();
        let second = create_backup(&file, 1_784_419_571).unwrap();
        assert_eq!(
            first.file_name().unwrap(),
            "Verses.pro.versevision-backup-20260719-000611"
        );
        assert_eq!(
            second.file_name().unwrap(),
            "Verses.pro.versevision-backup-20260719-000611-1"
        );
        fs::write(&file, b"two").unwrap();
        restore_backup(&file, &first).unwrap();
        assert_eq!(fs::read(&file).unwrap(), b"one");
        let elsewhere =
            std::env::temp_dir().join(format!("vv-other-{}", uuid::Uuid::new_v4().simple()));
        fs::write(&elsewhere, b"x").unwrap();
        assert!(restore_backup(&file, &elsewhere).is_err());
        fs::remove_dir_all(&dir).unwrap();
        fs::remove_file(&elsewhere).unwrap();
    }
}
