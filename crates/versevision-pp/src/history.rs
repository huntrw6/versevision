//! `history.py`: what each import added, so a rerun can replace it and a mistake can be undone. The file is `imports.json` of the data
//! folder, written the way Python writes it (`json.dump(..., indent=2)`, ASCII escapes, a temporary file then a rename), so the Python app
//! reads what this writes and the other way round.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{json, Value};

use crate::clock::{pydantic_datetime, Clock};
use crate::error::PpError;
use crate::errors::{VvError, VvResult};
use crate::files::LockGuard;
use crate::pyjson;

/// One import (`ImportRecord`). Field order is the order of the Python model, which is the order of the file.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportRecord {
    pub import_id: String,
    /// What pydantic writes for the time: `2026-11-24T05:12:33.123456Z`.
    pub created: String,
    pub source_name: String,
    pub source_sha256: String,
    pub target_uuid: String,
    pub target_name: String,
    pub node: String,
    pub cue_uuids: Vec<String>,
    pub slide_count: i64,
    pub verse_count: i64,
    pub point_count: i64,
    pub backup_path: Option<String>,
    /// `active | replaced | undone | rolled_back`
    pub status: String,
    pub replaces: Option<String>,
    /// `new` (own presentation) | `append` | `replace` | `swap`
    pub mode: String,
    pub library: Option<String>,
    pub file_path: Option<String>,
    pub file_sha256: String,
    pub file_cue_count: i64,
    pub copied_from: Option<String>,
    /// `[{"playlist": id, "item": id, "name": ..., "presentation": ...}]` added for this file
    pub playlist_items: Vec<Value>,
}

impl ImportRecord {
    /// A record with Python's defaults; `created` is the given time.
    pub fn new(
        import_id: &str,
        created_micros: i64,
        source_name: &str,
        source_sha256: &str,
        target_uuid: &str,
        target_name: &str,
    ) -> ImportRecord {
        ImportRecord {
            import_id: import_id.to_string(),
            created: pydantic_datetime(created_micros),
            source_name: source_name.to_string(),
            source_sha256: source_sha256.to_string(),
            target_uuid: target_uuid.to_string(),
            target_name: target_name.to_string(),
            node: "local".to_string(),
            cue_uuids: Vec::new(),
            slide_count: 0,
            verse_count: 0,
            point_count: 0,
            backup_path: None,
            status: "active".to_string(),
            replaces: None,
            mode: "append".to_string(),
            library: None,
            file_path: None,
            file_sha256: String::new(),
            file_cue_count: 0,
            copied_from: None,
            playlist_items: Vec::new(),
        }
    }

    /// `model_dump(mode="json")`.
    pub fn to_value(&self) -> Value {
        let opt = |v: &Option<String>| v.as_ref().map(|s| json!(s)).unwrap_or(Value::Null);
        json!({
            "import_id": self.import_id,
            "created": self.created,
            "source_name": self.source_name,
            "source_sha256": self.source_sha256,
            "target_uuid": self.target_uuid,
            "target_name": self.target_name,
            "node": self.node,
            "cue_uuids": self.cue_uuids,
            "slide_count": self.slide_count,
            "verse_count": self.verse_count,
            "point_count": self.point_count,
            "backup_path": opt(&self.backup_path),
            "status": self.status,
            "replaces": opt(&self.replaces),
            "mode": self.mode,
            "library": opt(&self.library),
            "file_path": opt(&self.file_path),
            "file_sha256": self.file_sha256,
            "file_cue_count": self.file_cue_count,
            "copied_from": opt(&self.copied_from),
            "playlist_items": self.playlist_items,
        })
    }

    /// `ImportRecord.model_validate(item)`: `None` where pydantic would raise (a missing or mistyped field). Fields a newer app added
    /// are ignored, as pydantic ignores them.
    pub fn from_value(item: &Value, default_created: &str) -> Option<ImportRecord> {
        let map = item.as_object()?;
        let required = |key: &str| -> Option<String> { map.get(key)?.as_str().map(str::to_string) };
        let string = |key: &str, default: &str| -> Option<String> {
            match map.get(key) {
                None => Some(default.to_string()),
                Some(v) => v.as_str().map(str::to_string),
            }
        };
        let optional = |key: &str| -> Option<Option<String>> {
            match map.get(key) {
                None | Some(Value::Null) => Some(None),
                Some(v) => v.as_str().map(|s| Some(s.to_string())),
            }
        };
        let int = |key: &str| -> Option<i64> {
            match map.get(key) {
                None => Some(0),
                Some(Value::Number(n)) => n
                    .as_i64()
                    .or_else(|| n.as_f64().filter(|f| f.fract() == 0.0).map(|f| f as i64)),
                Some(Value::String(s)) => s.trim().parse().ok(),
                _ => None,
            }
        };
        let strings = |key: &str| -> Option<Vec<String>> {
            match map.get(key) {
                None => Some(Vec::new()),
                Some(Value::Array(a)) => a.iter().map(|v| v.as_str().map(str::to_string)).collect(),
                _ => None,
            }
        };
        let created = match map.get("created") {
            None => None,
            Some(Value::String(s)) => Some(normalize_datetime(s)?),
            Some(_) => return None,
        };
        let playlist_items = match map.get("playlist_items") {
            None => Vec::new(),
            Some(Value::Array(a)) => {
                if !a.iter().all(Value::is_object) {
                    return None;
                }
                a.clone()
            }
            _ => return None,
        };
        Some(ImportRecord {
            import_id: required("import_id")?,
            created: created.unwrap_or_else(|| default_created.to_string()),
            source_name: required("source_name")?,
            source_sha256: required("source_sha256")?,
            target_uuid: required("target_uuid")?,
            target_name: required("target_name")?,
            node: string("node", "local")?,
            cue_uuids: strings("cue_uuids")?,
            slide_count: int("slide_count")?,
            verse_count: int("verse_count")?,
            point_count: int("point_count")?,
            backup_path: optional("backup_path")?,
            status: string("status", "active")?,
            replaces: optional("replaces")?,
            mode: string("mode", "append")?,
            library: optional("library")?,
            file_path: optional("file_path")?,
            file_sha256: string("file_sha256", "")?,
            file_cue_count: int("file_cue_count")?,
            copied_from: optional("copied_from")?,
            playlist_items,
        })
    }

    /// `created.strftime("%Y-%m-%d %H:%M")`.
    pub fn created_display(&self) -> String {
        let c: Vec<char> = self.created.chars().collect();
        if c.len() >= 16 {
            format!(
                "{} {}",
                c[..10].iter().collect::<String>(),
                c[11..16].iter().collect::<String>()
            )
        } else {
            self.created.clone()
        }
    }
}

/// Read an ISO 8601 date-time the way pydantic does and write it back the way pydantic does (`Z` for UTC, a fraction only when it is not
/// zero). `None` when it is not a date-time.
pub fn normalize_datetime(text: &str) -> Option<String> {
    let t = text.trim();
    let b = t.as_bytes();
    let num = |from: usize, len: usize| -> Option<i64> {
        let s = t.get(from..from + len)?;
        s.chars()
            .all(|c| c.is_ascii_digit())
            .then(|| s.parse().ok())?
    };
    let (year, month, day) = (num(0, 4)?, num(5, 2)?, num(8, 2)?);
    if b.get(4) != Some(&b'-')
        || b.get(7) != Some(&b'-')
        || !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
    {
        return None;
    }
    let mut out = format!("{year:04}-{month:02}-{day:02}");
    let rest = &t[10..];
    if rest.is_empty() {
        return Some(format!("{out}T00:00:00"));
    }
    let rest = rest.strip_prefix('T').or_else(|| rest.strip_prefix(' '))?;
    let (hour, minute) = (num(11, 2)?, num(14, 2)?);
    if hour > 23 || minute > 59 || t.as_bytes().get(13) != Some(&b':') {
        return None;
    }
    let mut pos = 16;
    let mut second = 0;
    if t.as_bytes().get(pos) == Some(&b':') {
        second = num(pos + 1, 2)?;
        pos += 3;
    }
    let mut micro = 0i64;
    if t.as_bytes().get(pos) == Some(&b'.') {
        let digits: String = t[pos + 1..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        if digits.is_empty() {
            return None;
        }
        let padded = format!("{:0<6}", &digits[..digits.len().min(6)]);
        micro = padded.parse().ok()?;
        pos += 1 + digits.len();
    }
    let _ = rest;
    let tz = &t[pos..];
    let tz_text = match tz {
        "" => String::new(),
        "Z" | "z" | "+00:00" | "-00:00" | "+0000" | "+00" => "Z".to_string(),
        other => {
            let sign = other.chars().next()?;
            if sign != '+' && sign != '-' {
                return None;
            }
            let digits: String = other[1..].chars().filter(char::is_ascii_digit).collect();
            if digits.len() != 4 && digits.len() != 2 {
                return None;
            }
            let (h, m) = (
                &digits[..2],
                if digits.len() == 4 {
                    &digits[2..]
                } else {
                    "00"
                },
            );
            format!("{sign}{h}:{m}")
        }
    };
    out.push_str(&format!("T{hour:02}:{minute:02}:{second:02}"));
    if micro != 0 {
        out.push_str(&format!(".{micro:06}"));
    }
    out.push_str(&tz_text);
    Some(out)
}

/// The history file (`HistoryStore`).
#[derive(Clone)]
pub struct HistoryStore {
    pub path: PathBuf,
    clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for HistoryStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HistoryStore")
            .field("path", &self.path)
            .finish()
    }
}

fn lock_error(error: PpError) -> VvError {
    VvError::from(error)
}

impl HistoryStore {
    pub fn new(path: impl Into<PathBuf>, clock: Arc<dyn Clock>) -> HistoryStore {
        HistoryStore {
            path: path.into(),
            clock,
        }
    }

    fn lock(&self) -> VvResult<LockGuard> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let name = format!("{}.lock", self.path.display());
        LockGuard::acquire_at(Path::new(&name)).map_err(lock_error)
    }

    /// `load()`: every record, or none when the file is missing, unreadable or not a list of valid records.
    pub fn load(&self) -> Vec<ImportRecord> {
        let Ok(bytes) = fs::read(&self.path) else {
            return Vec::new();
        };
        let Ok(text) = String::from_utf8(bytes) else {
            return Vec::new();
        };
        let Ok(Value::Array(items)) = serde_json::from_str::<Value>(&text) else {
            return Vec::new();
        };
        let now = pydantic_datetime(self.clock.now_micros());
        items
            .iter()
            .map(|item| ImportRecord::from_value(item, &now))
            .collect::<Option<Vec<_>>>()
            .unwrap_or_default()
    }

    fn save(&self, records: &[ImportRecord]) -> VvResult<()> {
        let dir = self.path.parent().unwrap_or(Path::new("."));
        let value = Value::Array(records.iter().map(ImportRecord::to_value).collect());
        let text = pyjson::dumps(&value, Some(2), true);
        let mut temp = tempfile::Builder::new()
            .prefix(".imports-")
            .suffix(".tmp")
            .tempfile_in(dir)?;
        temp.write_all(text.as_bytes())?;
        temp.flush()?;
        temp.persist(&self.path)
            .map_err(|e| VvError::from(e.error))?;
        Ok(())
    }

    pub fn add(&self, record: ImportRecord) -> VvResult<()> {
        let _lock = self.lock()?;
        let mut records = self.load();
        records.push(record);
        self.save(&records)
    }

    pub fn set_status(&self, import_id: &str, status: &str) -> VvResult<()> {
        let _lock = self.lock()?;
        let mut records = self.load();
        for item in records.iter_mut().filter(|r| r.import_id == import_id) {
            item.status = status.to_string();
        }
        self.save(&records)
    }

    pub fn get(&self, import_id: &str) -> Option<ImportRecord> {
        self.load().into_iter().find(|r| r.import_id == import_id)
    }

    /// The newest active record for the presentation (and, when given, the source and the node).
    pub fn latest_active(
        &self,
        target_uuid: &str,
        source_name: Option<&str>,
        node: Option<&str>,
    ) -> Option<ImportRecord> {
        let folded = versevision_core::references::pytext::casefold(target_uuid);
        self.load().into_iter().rfind(|r| {
            r.status == "active"
                && versevision_core::references::pytext::casefold(&r.target_uuid) == folded
                && source_name.is_none_or(|s| r.source_name == s)
                && node.is_none_or(|n| r.node == n)
        })
    }

    /// The time stamped on a new record.
    pub fn now_micros(&self) -> i64 {
        self.clock.now_micros()
    }
}

/// `{"records": [...]}` for the JSON listing of `versevision history --output json` (a plain list in Python).
pub fn records_json(records: &[ImportRecord]) -> Value {
    Value::Array(records.iter().map(ImportRecord::to_value).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::VirtualClock;

    fn store(dir: &Path) -> HistoryStore {
        HistoryStore::new(
            dir.join("imports.json"),
            Arc::new(VirtualClock::new(1_784_419_571, "h")),
        )
    }

    #[test]
    fn a_record_survives_a_round_trip_byte_for_byte() {
        let dir = tempfile::tempdir().unwrap();
        let s = store(dir.path());
        let mut r = ImportRecord::new(
            "abc123",
            1_784_419_571_123_000,
            "Notes é.docx",
            "ff",
            "U1",
            "Verses",
        );
        r.cue_uuids = vec!["C1".into()];
        r.playlist_items = vec![json!({"playlist": "P", "item": "I"})];
        s.add(r.clone()).unwrap();
        let text = fs::read_to_string(&s.path).unwrap();
        assert!(text.contains("\"Notes \\u00e9.docx\""));
        assert!(text.contains("\"created\": \"2026-07-19T00:06:11.123000Z\""));
        assert_eq!(s.load(), vec![r]);
        s.set_status("abc123", "undone").unwrap();
        assert_eq!(s.get("abc123").unwrap().status, "undone");
        assert!(s.latest_active("u1", None, None).is_none());
    }

    #[test]
    fn one_bad_record_makes_the_history_empty_like_python() {
        let dir = tempfile::tempdir().unwrap();
        let s = store(dir.path());
        fs::write(&s.path, r#"[{"import_id": "a", "source_name": "n", "source_sha256": "x", "target_uuid": "u", "target_name": "t"}, {"import_id": 3}]"#).unwrap();
        assert!(s.load().is_empty());
        fs::write(&s.path, "not json").unwrap();
        assert!(s.load().is_empty());
    }

    #[test]
    fn datetimes_are_written_like_pydantic() {
        assert_eq!(
            normalize_datetime("2026-11-24T05:12:33.1+00:00").unwrap(),
            "2026-11-24T05:12:33.100000Z"
        );
        assert_eq!(
            normalize_datetime("2026-11-24T05:12:33Z").unwrap(),
            "2026-11-24T05:12:33Z"
        );
        assert_eq!(
            normalize_datetime("2026-11-24 05:12").unwrap(),
            "2026-11-24T05:12:00"
        );
        assert_eq!(
            normalize_datetime("2026-11-24T05:12:33+0200").unwrap(),
            "2026-11-24T05:12:33+02:00"
        );
        assert!(normalize_datetime("yesterday").is_none());
    }
}
