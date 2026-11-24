//! `bibles/local_usfm.py`: a folder of USFM/SFM files read into memory on first use.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use regex::Regex;
use versevision_core::bibles::{BibleError, BibleProvider, BibleVerse, ProviderHealth};
use versevision_core::config::{CachePolicy, LocalUsfmProviderConfig};
use versevision_core::pystr::{self, flatten};
use versevision_core::references::VerseSpan;
use versevision_core::versions::BibleVersion;

use crate::remote::upper;

type Book = BTreeMap<(u64, u64), BibleVerse>;
type Books = BTreeMap<String, Book>;

fn re(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).expect("pattern"))
}

/// `_MARKER`: a footnote or cross-reference `\f ... \f*` / `\x ... \x*`, or any other marker with its closing `*` and the space after it.
fn marker() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    re(&R, r"(?s)\\(?:f|x)\s.*?\\(?:f|x)\*|\\[A-Za-z0-9-]+\*?\s*")
}

/// `_clean`
fn clean(value: &str) -> String {
    flatten(&marker().replace_all(value, " "))
}

pub struct LocalUsfmProvider {
    name: String,
    config: LocalUsfmProviderConfig,
    version: String,
    provider_version_id: String,
    books: Mutex<Option<Arc<Books>>>,
}

impl std::fmt::Debug for LocalUsfmProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalUsfmProvider")
            .field("name", &self.name)
            .field("version", &self.version)
            .field("path", &self.config.path)
            .finish_non_exhaustive()
    }
}

/// `str(path.resolve())`: the absolute path with links resolved when the folder exists.
fn resolved(path: &Path) -> String {
    let full = std::fs::canonicalize(path)
        .or_else(|_| std::path::absolute(path))
        .unwrap_or_else(|_| path.to_path_buf());
    let text = full.to_string_lossy().to_string();
    // Windows' canonical form starts with `\\?\`
    text.strip_prefix(r"\\?\")
        .map(str::to_string)
        .unwrap_or(text)
}

/// `Path.suffix` casefolded.
fn suffix(name: &str) -> String {
    match name.rfind('.') {
        Some(at) if at > 0 && at + 1 < name.len() => {
            versevision_core::references::casefold(&name[at..])
        }
        _ => String::new(),
    }
}

/// `(?m)^\\id\s+([A-Z0-9]{3})\b`
fn book_id_of(raw: &str) -> Option<String> {
    static R: OnceLock<Regex> = OnceLock::new();
    re(&R, r"(?m)^\\id\s+([A-Z0-9]{3})\b")
        .captures(raw)
        .map(|c| c[1].to_string())
}

/// Python's `int(digits)` for what `\d+` matched (a decimal digit of any script counts).
fn number(text: &str) -> u64 {
    text.chars().fold(0u64, |value, c| {
        let digit = versevision_core::references::decimal_value(c).unwrap_or(0);
        value.saturating_mul(10).saturating_add(u64::from(digit))
    })
}

impl LocalUsfmProvider {
    pub fn new(name: &str, config: LocalUsfmProviderConfig) -> Self {
        let version = upper(&config.version);
        let provider_version_id = resolved(Path::new(&config.path));
        Self {
            name: name.to_string(),
            config,
            version,
            provider_version_id,
            books: Mutex::new(None),
        }
    }

    fn load(&self) -> Result<Arc<Books>, BibleError> {
        if let Some(books) = self.books.lock().unwrap_or_else(|e| e.into_inner()).clone() {
            return Ok(books);
        }
        let dir = PathBuf::from(&self.config.path);
        if !dir.is_dir() {
            return Err(BibleError::Provider(format!(
                "USFM directory does not exist: {}",
                self.config.path
            )));
        }
        let mut names: Vec<String> = std::fs::read_dir(&dir)
            .map_err(|e| BibleError::Provider(format!("{}: {}", self.config.path, e.kind())))?
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| entry.file_name().into_string().ok())
            .collect();
        names.sort();
        let mut books = Books::new();
        for file_name in names {
            let path = dir.join(&file_name);
            if !path.is_file() || !matches!(suffix(&file_name).as_str(), ".usfm" | ".sfm" | ".txt")
            {
                continue;
            }
            let bytes = std::fs::read(&path)
                .map_err(|e| BibleError::Other(format!("{}: {}", path.display(), e.kind())))?;
            let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&bytes);
            let raw = std::str::from_utf8(bytes)
                .map_err(|e| BibleError::Other(crate::pyjson::utf8_message(bytes, e)))?;
            // `read_text` is a text-mode read: `\r\n` and `\r` are read as `\n`
            let raw = raw.replace("\r\n", "\n").replace('\r', "\n");
            let raw = raw.as_str();
            let Some(book_id) = book_id_of(raw) else {
                continue;
            };
            books.insert(book_id.clone(), self.parse_book(&book_id, raw));
        }
        let books = Arc::new(books);
        *self.books.lock().unwrap_or_else(|e| e.into_inner()) = Some(books.clone());
        Ok(books)
    }

    fn parse_book(&self, book_id: &str, raw: &str) -> Book {
        static CHAPTER: OnceLock<Regex> = OnceLock::new();
        static PARAGRAPH: OnceLock<Regex> = OnceLock::new();
        static POETRY: OnceLock<Regex> = OnceLock::new();
        static VERSE: OnceLock<Regex> = OnceLock::new();
        let chapter_re = re(&CHAPTER, r"^\\c\s+(\d+)");
        let paragraph_re = re(&PARAGRAPH, r"^\\p(?:\s|$)");
        let poetry_re = re(&POETRY, r"^\\q(\d*)\s*");
        let verse_re = re(&VERSE, r"^\\v\s+(\d+)(?:-[0-9]+)?\s*(.*)");

        let mut chapter = 0u64;
        let mut paragraph_start = false;
        let mut poetry_indent = 0i64;
        let mut verses = Book::new();
        let mut current: Option<(u64, u64)> = None;
        for line in pystr::splitlines(raw) {
            if let Some(found) = chapter_re.captures(line) {
                chapter = number(&found[1]);
                continue;
            }
            if paragraph_re.is_match(line) {
                paragraph_start = true;
                continue;
            }
            let mut line = line;
            if let Some(found) = poetry_re.captures(line) {
                poetry_indent = match &found[1] {
                    "" => 1,
                    digits => number(digits) as i64,
                };
                line = &line[found.get(0).map_or(0, |m| m.end())..];
            }
            let verse = verse_re.captures(line).filter(|_| chapter != 0);
            if let Some(found) = verse {
                let verse_number = number(&found[1]);
                let entry = BibleVerse {
                    book_id: book_id.to_string(),
                    chapter,
                    verse: verse_number,
                    text: clean(&found[2]),
                    version: self.version.clone(),
                    paragraph_start,
                    poetry_indent,
                    copyright: self.config.copyright.clone(),
                    attribution: self.config.attribution.clone(),
                };
                verses.insert((chapter, verse_number), entry);
                current = Some((chapter, verse_number));
                paragraph_start = false;
                poetry_indent = 0;
            } else if let Some(key) = current {
                if !line.is_empty() && !line.starts_with('\\') {
                    if let Some(entry) = verses.get_mut(&key) {
                        entry.text = clean(&format!("{} {}", entry.text, line));
                    }
                }
            }
        }
        verses
    }
}

impl BibleProvider for LocalUsfmProvider {
    async fn list_versions(&self) -> Result<Vec<BibleVersion>, BibleError> {
        Ok(vec![BibleVersion {
            abbreviation: self.version.clone(),
            name: self
                .config
                .name
                .clone()
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| self.version.clone()),
            language: "eng".to_string(),
            provider: Some(self.name.clone()),
            provider_version_id: Some(self.provider_version_id.clone()),
            copyright: self.config.copyright.clone(),
            attribution: self.config.attribution.clone(),
        }])
    }

    async fn fetch_passage(
        &self,
        version: &str,
        book_id: &str,
        spans: &[VerseSpan],
    ) -> Result<Vec<BibleVerse>, BibleError> {
        if upper(version) != self.version {
            return Err(BibleError::Provider(format!(
                "Provider {} does not supply {version}",
                self.name
            )));
        }
        let books = self.load()?;
        let Some(book) = books.get(book_id).filter(|book| !book.is_empty()) else {
            return Err(BibleError::Provider(format!(
                "USFM book {book_id} was not found in {}",
                self.config.path
            )));
        };
        let mut selected = Vec::new();
        for span in spans {
            let end = span.end.unwrap_or(span.start);
            let first = (span.start.chapter, span.start.verse);
            let last = (end.chapter, end.verse);
            let keys: Vec<(u64, u64)> = if first <= last {
                book.range(first..=last).map(|(key, _)| *key).collect()
            } else {
                Vec::new()
            };
            if keys.first() != Some(&first) || keys.last() != Some(&last) {
                return Err(BibleError::Provider(format!(
                    "USFM passage is incomplete for {book_id} {}:{}",
                    span.start.chapter, span.start.verse
                )));
            }
            selected.extend(keys.iter().map(|key| book[key].clone()));
        }
        Ok(selected)
    }

    async fn health_check(&self) -> ProviderHealth {
        let provider = Some(self.name.clone());
        match self.load() {
            Ok(books) => ProviderHealth {
                healthy: !books.is_empty(),
                message: format!("Loaded {} USFM book(s)", books.len()),
                provider,
                versions: vec![self.version.clone()],
            },
            Err(error) => ProviderHealth {
                healthy: false,
                message: error.message().to_string(),
                provider,
                versions: Vec::new(),
            },
        }
    }

    fn cache_policy(&self) -> CachePolicy {
        self.config.cache_policy
    }
}
