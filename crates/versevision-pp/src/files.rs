//! The folder work around the byte-level writer: a lock, a backup, a temporary file beside the target, an atomic replace, and the new-file
//! write that never overwrites (`append_plan`, `create_presentation`, `derive_presentation` of `protobuf_writer.py`, the file half).

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use versevision_core::config::PresentationTemplateConfig;
use versevision_core::plan::SlidePlan;

use crate::backup::create_backup;
use crate::error::{PpError, Result};
use crate::themes::LoadedLooks;
use crate::writer::{sha256_hex, PlanWriteResult, PresentationWriter, UuidSource};

/// A write to a file: where it went, the backup made first, and what was done.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileWriteResult {
    pub path: PathBuf,
    pub backup_path: PathBuf,
    pub result: PlanWriteResult,
}

fn temp_beside(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("presentation");
    path.with_file_name(format!(
        ".{name}.versevision-{}.tmp",
        uuid::Uuid::new_v4().simple()
    ))
}

fn write_synced(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(PpError::io)?;
    file.write_all(bytes).map_err(PpError::io)?;
    file.flush().map_err(PpError::io)?;
    file.sync_all().map_err(PpError::io)
}

/// `FileLock(path + ".versevision.lock", timeout=10)`: held while the guard lives.
pub struct LockGuard {
    file: fs::File,
}

impl LockGuard {
    pub fn acquire(path: &Path) -> Result<LockGuard> {
        Self::acquire_at(&PathBuf::from(format!(
            "{}.versevision.lock",
            path.display()
        )))
    }

    /// Hold the lock file `lock_path` (created when missing) for up to 10 seconds.
    pub fn acquire_at(lock_path: &Path) -> Result<LockGuard> {
        let lock_path = lock_path.to_path_buf();
        let file = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&lock_path)
            .map_err(PpError::io)?;
        let started = Instant::now();
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(LockGuard { file }),
                Err(fs::TryLockError::WouldBlock)
                    if started.elapsed() < Duration::from_secs(10) =>
                {
                    std::thread::sleep(Duration::from_millis(50))
                }
                Err(fs::TryLockError::WouldBlock) => {
                    return Err(PpError::io(format!(
                        "The file lock {} could not be obtained",
                        lock_path.display()
                    )))
                }
                Err(fs::TryLockError::Error(error)) => return Err(PpError::io(error)),
            }
        }
    }
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

/// `append_plan(path, ...)`: change a presentation in place (after a backup, atomically). `template` is the file holding the template slides
/// when it is another file than the target.
#[allow(clippy::too_many_arguments)]
pub fn append_plan_in_place<U: UuidSource>(
    writer: &mut PresentationWriter<U>,
    path: &Path,
    plan: &SlidePlan,
    definition: &PresentationTemplateConfig,
    looks: Option<&LoadedLooks>,
    template: Option<&Path>,
    remove_cue_uuids: Option<&[String]>,
    now: i64,
) -> Result<FileWriteResult> {
    let path = fs::canonicalize(path).map_err(PpError::io)?;
    let _lock = LockGuard::acquire(&path)?;
    let original = fs::read(&path).map_err(PpError::io)?;
    let template_bytes = match template {
        Some(template) if fs::canonicalize(template).map_err(PpError::io)? != path => {
            Some(fs::read(template).map_err(PpError::io)?)
        }
        _ => None,
    };
    let written = writer.append_plan(
        &original,
        plan,
        definition,
        looks,
        template_bytes.as_deref(),
        remove_cue_uuids,
    )?;
    let backup = create_backup(&path, now)?;
    let temp = temp_beside(&path);
    let outcome = (|| -> Result<()> {
        write_synced(&temp, &written.bytes)?;
        if sha256_hex(&fs::read(&path).map_err(PpError::io)?) != written.result.original_sha256 {
            return Err(PpError::value(
                "presentation changed while VerseVision was preparing the import; no files changed",
            ));
        }
        fs::rename(&temp, &path).map_err(PpError::io)
    })();
    if let Err(error) = outcome {
        let _ = fs::remove_file(&temp);
        return Err(error);
    }
    if sha256_hex(&fs::read(&path).map_err(PpError::io)?) != written.result.output_sha256 {
        fs::rename(&backup, &path).map_err(PpError::io)?;
        return Err(PpError::value("verification failed; rollback completed"));
    }
    Ok(FileWriteResult {
        path,
        backup_path: backup,
        result: written.result,
    })
}

/// Put `bytes` at `output` as a NEW file: a temporary file beside it, then a hard link that fails instead of replacing an existing file.
pub fn write_new_file(output: &Path, bytes: &[u8]) -> Result<PathBuf> {
    let output = if output.is_absolute() {
        output.to_path_buf()
    } else {
        std::env::current_dir().map_err(PpError::io)?.join(output)
    };
    let file_name = output
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("presentation")
        .to_string();
    if output.exists() {
        return Err(PpError::value(format!(
            "{file_name} already exists; a new presentation never overwrites a file"
        )));
    }
    let temp = temp_beside(&output);
    let outcome = (|| -> Result<()> {
        write_synced(&temp, bytes)?;
        if output.exists() {
            return Err(PpError::value(format!(
                "{file_name} appeared while it was being written"
            )));
        }
        fs::hard_link(&temp, &output).map_err(PpError::io)
    })();
    let _ = fs::remove_file(&temp);
    outcome?;
    Ok(output)
}

/// `derive_presentation(source, output, ...)` on files.
#[allow(clippy::too_many_arguments)]
pub fn derive_presentation_file<U: UuidSource>(
    writer: &mut PresentationWriter<U>,
    source: &Path,
    output: &Path,
    plan: &SlidePlan,
    definition: &PresentationTemplateConfig,
    looks: Option<&LoadedLooks>,
    template: Option<&Path>,
    name: &str,
    clear_generated: bool,
    keep_uuid: bool,
    now: i64,
) -> Result<FileWriteResult> {
    let source = fs::canonicalize(source).map_err(PpError::io)?;
    if output.exists() {
        return Err(PpError::value(format!(
            "{} already exists; a new presentation never overwrites a file",
            output.file_name().and_then(|n| n.to_str()).unwrap_or("")
        )));
    }
    let source_bytes = fs::read(&source).map_err(PpError::io)?;
    let template_bytes = match template {
        Some(template) if fs::canonicalize(template).map_err(PpError::io)? != source => {
            Some(fs::read(template).map_err(PpError::io)?)
        }
        _ => None,
    };
    let written = writer.derive_presentation(
        &source_bytes,
        template_bytes.as_deref(),
        plan,
        definition,
        looks,
        name,
        clear_generated,
        keep_uuid,
        now,
    )?;
    let path = write_new_file(output, &written.bytes)?;
    Ok(FileWriteResult {
        path: path.clone(),
        backup_path: path,
        result: written.result,
    })
}

/// `create_presentation(template, output, ...)` on files.
#[allow(clippy::too_many_arguments)]
pub fn create_presentation_file<U: UuidSource>(
    writer: &mut PresentationWriter<U>,
    template: &Path,
    output: &Path,
    plan: &SlidePlan,
    definition: &PresentationTemplateConfig,
    looks: Option<&LoadedLooks>,
    name: &str,
    now: i64,
) -> Result<FileWriteResult> {
    let template = fs::canonicalize(template).map_err(PpError::io)?;
    if output.exists() {
        return Err(PpError::value(format!(
            "{} already exists; a new presentation never overwrites a file",
            output.file_name().and_then(|n| n.to_str()).unwrap_or("")
        )));
    }
    let written = writer.create_presentation(
        &fs::read(&template).map_err(PpError::io)?,
        plan,
        definition,
        looks,
        name,
        now,
    )?;
    let path = write_new_file(output, &written.bytes)?;
    Ok(FileWriteResult {
        path: path.clone(),
        backup_path: path,
        result: written.result,
    })
}
