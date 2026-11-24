//! The scripture cache (`bibles/cache.py`, `ScriptureCache`): verses kept by provider, provider version id, book, chapter and
//! verse, in a folder the host app names.
//!
//! Python keeps them in one SQLite file. Here each verse is a small JSON file (`<folder>/<hash of provider + version id>/<book>.<chapter>.<verse>.json`)
//! written to a temporary name and renamed into place, so a reader never sees half a file and any number of threads or
//! processes can read and write at once; the last writer of a verse wins, as with `INSERT OR REPLACE`. A file that does not
//! parse, or whose text does not match its hash, is a miss. Nothing in this module logs.
//!
//! The registry only ever calls [`VerseCache::put`] for a provider that says its text may be stored; the ESV provider says no, so
//! ESV text never reaches this folder (a test checks the folder stays empty).

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use versevision_core::bibles::{BibleVerse, VerseCache};

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn sha256_hex(text: &str) -> String {
    hex(&Sha256::digest(text.as_bytes()))
}

/// What one file holds: the columns of Python's `verses` table.
#[derive(Debug, Serialize, Deserialize)]
struct Entry {
    provider: String,
    provider_version_id: String,
    book_id: String,
    chapter: u64,
    verse: u64,
    version: String,
    text: String,
    paragraph_start: bool,
    poetry_indent: i64,
    copyright: Option<String>,
    attribution: Option<String>,
    retrieved_at: String,
    content_hash: String,
}

#[derive(Debug)]
pub struct ScriptureCache {
    dir: PathBuf,
}

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// `datetime.now(timezone.utc).isoformat()`-like text (seconds, `Z`) without a date library.
fn now_iso() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let (days, rest) = ((secs / 86_400) as i64, secs % 86_400);
    // civil_from_days (Howard Hinnant)
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
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

impl ScriptureCache {
    /// A cache in `dir` (made on the first write).
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn provider_dir(&self, provider: &str, provider_version_id: &str) -> PathBuf {
        let key = sha256_hex(&format!("{provider}\0{provider_version_id}"));
        self.dir.join(&key[..32])
    }

    fn file(
        &self,
        provider: &str,
        provider_version_id: &str,
        book_id: &str,
        chapter: u64,
        verse: u64,
    ) -> PathBuf {
        // a book id is three letters/digits; anything else is hashed so it can never be a path
        let book = if book_id.len() <= 8 && book_id.chars().all(|c| c.is_ascii_alphanumeric()) {
            book_id.to_string()
        } else {
            sha256_hex(book_id)[..16].to_string()
        };
        self.provider_dir(provider, provider_version_id)
            .join(format!("{book}.{chapter}.{verse}.json"))
    }

    /// `ScriptureCache.count`
    pub fn count(&self) -> usize {
        self.entry_files().len()
    }

    /// `ScriptureCache.clear`: forget everything; returns how many verses were held.
    pub fn clear(&self) -> usize {
        let files = self.entry_files();
        let mut removed = 0;
        for file in &files {
            if fs::remove_file(file).is_ok() {
                removed += 1;
            }
        }
        if let Ok(dirs) = fs::read_dir(&self.dir) {
            for dir in dirs.flatten() {
                let _ = fs::remove_dir(dir.path()); // only when empty
            }
        }
        removed
    }

    fn entry_files(&self) -> Vec<PathBuf> {
        let mut files = Vec::new();
        let Ok(dirs) = fs::read_dir(&self.dir) else {
            return files;
        };
        for dir in dirs.flatten() {
            let Ok(inner) = fs::read_dir(dir.path()) else {
                continue;
            };
            for file in inner.flatten() {
                let path = file.path();
                if path.extension().is_some_and(|e| e == "json") {
                    files.push(path);
                }
            }
        }
        files
    }
}

impl VerseCache for ScriptureCache {
    fn get(
        &self,
        provider: &str,
        provider_version_id: &str,
        book_id: &str,
        chapter: u64,
        verse: u64,
    ) -> Option<BibleVerse> {
        let bytes =
            fs::read(self.file(provider, provider_version_id, book_id, chapter, verse)).ok()?;
        let entry: Entry = serde_json::from_slice(&bytes).ok()?;
        let same_key = entry.provider == provider
            && entry.provider_version_id == provider_version_id
            && entry.book_id == book_id
            && entry.chapter == chapter
            && entry.verse == verse;
        if !same_key || entry.content_hash != sha256_hex(&entry.text) {
            return None;
        }
        Some(BibleVerse {
            book_id: entry.book_id,
            chapter: entry.chapter,
            verse: entry.verse,
            text: entry.text,
            version: entry.version,
            paragraph_start: entry.paragraph_start,
            poetry_indent: entry.poetry_indent,
            copyright: entry.copyright,
            attribution: entry.attribution,
        })
    }

    fn put(&self, provider: &str, provider_version_id: &str, value: &BibleVerse) {
        let entry = Entry {
            provider: provider.to_string(),
            provider_version_id: provider_version_id.to_string(),
            book_id: value.book_id.clone(),
            chapter: value.chapter,
            verse: value.verse,
            version: value.version.clone(),
            text: value.text.clone(),
            paragraph_start: value.paragraph_start,
            poetry_indent: value.poetry_indent,
            copyright: value.copyright.clone(),
            attribution: value.attribution.clone(),
            retrieved_at: now_iso(),
            content_hash: sha256_hex(&value.text),
        };
        let Ok(body) = serde_json::to_vec(&entry) else {
            return;
        };
        let target = self.file(
            provider,
            provider_version_id,
            &value.book_id,
            value.chapter,
            value.verse,
        );
        let Some(dir) = target.parent() else { return };
        if fs::create_dir_all(dir).is_err() {
            return;
        }
        let temp = dir.join(format!(
            ".{}-{}.tmp",
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let written = fs::File::create(&temp).and_then(|mut file| {
            file.write_all(&body)?;
            file.sync_all()
        });
        if written.is_ok() && fs::rename(&temp, &target).is_ok() {
            return;
        }
        let _ = fs::remove_file(&temp);
    }
}
