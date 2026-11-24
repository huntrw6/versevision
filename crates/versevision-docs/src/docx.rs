//! `.docx`: `"\n".join(paragraph.text for paragraph in docx.Document(path).paragraphs)`.
//!
//! A port of what `docx.Document(path)` does with a `pathlib.Path` (python-docx 1.x): the OPC package is read through its
//! relationships (`docx/opc/pkgreader.py`), every reachable part with an XML part class is parsed (`docx/opc/part.py::XmlPart.load`),
//! the main document part is found by the `officeDocument` relationship, and `Document.paragraphs` are the direct `w:p` children of
//! `w:body` (tables, headers, footers, text boxes, `w:sdt` content are not read). `Paragraph.text` joins the `w:r` / `w:hyperlink`
//! children; `Run.text` joins `w:br | w:cr | w:noBreakHyphen | w:ptab | w:t | w:tab`.
//!
//! Python's exceptions are reproduced as `str(exc)`:
//! * `BadZipFile`: `File is not a zip file` (a `Path` is never `PackageNotFoundError`, python-docx hands it to `ZipFile`);
//! * `KeyError`: `str()` of a `KeyError` is the `repr` of its message, quotes included;
//! * lxml `XMLSyntaxError`: libxml2's message plus `, line L, column C (<string>, line L)`. The parser below reproduces libxml2's
//!   message and cursor position for the common well-formedness errors (see `xml::Parser`); rarer ones fall back to a generic message.
//!
//! Normalisation (the temp directory cannot be reproduced): the one message that contains the file path (`file '<path>' is not a Word
//! file, content type is '<type>'`) uses the file name `upload.docx`, which is what the web layer calls the saved upload.
//!
//! Not reproduced: zlib messages for corrupt deflate data, non-UTF-8 XML encodings, DTD entity declarations, encrypted or bzip2/lzma
//! members, namespace-less `lxml` class names in `AttributeError` for roots other than `w:document`.

use std::collections::HashMap;
use std::io::{Cursor, Read};

use crate::DocumentError;

const NS_WML: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const NS_CT: &str = "http://schemas.openxmlformats.org/package/2006/content-types";
const NS_PR: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
const CT_WML_DOCUMENT_MAIN: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml";
const RT_OFFICE_DOCUMENT: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument";
const RT_IMAGE: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/image";
/// the content types python-docx gives an `XmlPart` subclass (parsed when the package is loaded)
const XML_PART_CONTENT_TYPES: &[&str] = &[
    "application/vnd.openxmlformats-package.core-properties+xml",
    "application/vnd.openxmlformats-officedocument.wordprocessingml.comments+xml",
    CT_WML_DOCUMENT_MAIN,
    "application/vnd.openxmlformats-officedocument.wordprocessingml.footer+xml",
    "application/vnd.openxmlformats-officedocument.wordprocessingml.header+xml",
    "application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml",
    "application/vnd.openxmlformats-officedocument.wordprocessingml.settings+xml",
    "application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml",
];

/// `str(exc)` of the Python exception that was raised.
type PyResult<T> = Result<T, String>;

pub fn extract_docx(bytes: &[u8]) -> Result<String, DocumentError> {
    extract(bytes).map_err(DocumentError::Unexpected)
}

// ---------------------------------------------------------------------------------------------------------------------
// Python string helpers
// ---------------------------------------------------------------------------------------------------------------------

/// `repr(str)`
fn py_repr(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::with_capacity(s.len() + 2);
    out.push(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if c == ' ' || (c.is_ascii() && !c.is_ascii_control()) => out.push(c),
            c if !is_printable(c) => {
                let n = u32::from(c);
                if n <= 0xff {
                    out.push_str(&format!("\\x{n:02x}"));
                } else if n <= 0xffff {
                    out.push_str(&format!("\\u{n:04x}"));
                } else {
                    out.push_str(&format!("\\U{n:08x}"));
                }
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// Approximation of `str.isprintable()` for one character.
fn is_printable(c: char) -> bool {
    !(c.is_control()
        || (c.is_whitespace() && c != ' ')
        || matches!(u32::from(c), 0xad | 0x600..=0x605 | 0x61c | 0x6dd | 0x70f | 0x180e | 0x200b..=0x200f | 0x2028..=0x202e | 0x2060..=0x2064 | 0x2066..=0x206f | 0xfeff | 0xfff9..=0xfffb | 0xe000..=0xf8ff))
}

/// `str(KeyError(message))`
fn key_error(message: &str) -> String {
    py_repr(message)
}

// ---------------------------------------------------------------------------------------------------------------------
// posixpath / PackURI
// ---------------------------------------------------------------------------------------------------------------------

/// `posixpath.join(a, b)`
fn posix_join(a: &str, b: &str) -> String {
    if b.starts_with('/') {
        b.to_string()
    } else if a.is_empty() || a.ends_with('/') {
        format!("{a}{b}")
    } else {
        format!("{a}/{b}")
    }
}

/// `posixpath.normpath` of an absolute path (what `posixpath.abspath` does to a path that starts with `/`)
fn posix_abspath(path: &str) -> String {
    let initial = if path.starts_with("//") && !path.starts_with("///") {
        "//"
    } else {
        "/"
    };
    let mut parts: Vec<&str> = Vec::new();
    for comp in path.split('/') {
        if comp.is_empty() || comp == "." {
            continue;
        }
        if comp != ".." || (initial.is_empty() && parts.is_empty()) || parts.last() == Some(&"..") {
            parts.push(comp);
        } else if !parts.is_empty() {
            parts.pop();
        }
    }
    format!("{initial}{}", parts.join("/"))
}

/// `PackURI.from_rel_ref(base_uri, relative_ref)`
fn from_rel_ref(base_uri: &str, relative_ref: &str) -> String {
    posix_abspath(&posix_join(base_uri, relative_ref))
}

/// `posixpath.split(path)`
fn posix_split(path: &str) -> (String, String) {
    let i = path.rfind('/').map_or(0, |i| i + 1);
    let (head, tail) = (&path[..i], &path[i..]);
    let head = if !head.is_empty() && head.chars().any(|c| c != '/') {
        head.trim_end_matches('/')
    } else {
        head
    };
    (head.to_string(), tail.to_string())
}

/// `PackURI.ext`
fn pack_ext(uri: &str) -> String {
    let (_, filename) = posix_split(uri);
    match filename.rfind('.') {
        // posixpath.splitext: leading dots of the file name do not start an extension
        Some(i) if filename[..i].chars().any(|c| c != '.') => filename[i + 1..].to_string(),
        _ => String::new(),
    }
}

/// `PackURI.rels_uri`
fn rels_uri(uri: &str) -> String {
    let (base, filename) = posix_split(uri);
    posix_join(&posix_join(&base, "_rels"), &format!("{filename}.rels"))
}

// ---------------------------------------------------------------------------------------------------------------------
// the zip file (`zipfile.ZipFile`)
// ---------------------------------------------------------------------------------------------------------------------

struct Zip<'a> {
    archive: zip::ZipArchive<Cursor<&'a [u8]>>,
}

impl Zip<'_> {
    /// `ZipFile.read(name)`: `KeyError` when the member is missing.
    fn read(&mut self, name: &str) -> PyResult<Vec<u8>> {
        let mut member = match self.archive.by_name(name) {
            Ok(member) => member,
            Err(zip::result::ZipError::FileNotFound) => {
                return Err(key_error(&format!(
                    "There is no item named {} in the archive",
                    py_repr(name)
                )))
            }
            Err(_) => return Err("Bad magic number for file header".to_string()),
        };
        let mut data = Vec::new();
        match member.read_to_end(&mut data) {
            Ok(_) => Ok(data),
            Err(error)
                if error.to_string().contains("checksum") || error.to_string().contains("CRC") =>
            {
                Err(format!("Bad CRC-32 for file {}", py_repr(name)))
            }
            Err(_) => {
                Err("Error -3 while decompressing data: invalid stored block lengths".to_string())
            }
        }
    }

    /// `rels_xml_for`: `None` when the member does not exist (only `KeyError` is caught by python-docx).
    fn rels_xml_for(&mut self, source_uri: &str) -> PyResult<Option<Vec<u8>>> {
        let member = &rels_uri(source_uri)[1..];
        if self.archive.by_name(member).is_err() {
            return Ok(None);
        }
        self.read(member).map(Some)
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// package reader (docx/opc/pkgreader.py)
// ---------------------------------------------------------------------------------------------------------------------

#[derive(Clone)]
struct SRel {
    base_uri: String,
    rid: Option<String>,
    reltype: Option<String>,
    target_ref: Option<String>,
    external: bool,
}

struct SPart {
    partname: String,
    content_type: Option<String>,
    reltype: Option<String>,
    blob: Vec<u8>,
}

struct ContentTypes {
    overrides: HashMap<String, Option<String>>,
    defaults: HashMap<String, Option<String>>,
}

impl ContentTypes {
    /// `_ContentTypeMap.__getitem__`
    fn get(&self, partname: &str) -> PyResult<Option<String>> {
        if let Some(ct) = self.overrides.get(&partname.to_lowercase()) {
            return Ok(ct.clone());
        }
        if let Some(ct) = self.defaults.get(&pack_ext(partname).to_lowercase()) {
            return Ok(ct.clone());
        }
        Err(key_error(&format!(
            "no content type for partname '{partname}' in [Content_Types].xml"
        )))
    }
}

fn content_types_from_xml(xml: &[u8]) -> PyResult<ContentTypes> {
    let root = xml::parse(xml)?;
    if !root.is(NS_CT, "Types") {
        return Err("'lxml.etree._Element' object has no attribute 'overrides'".into());
    }
    let mut map = ContentTypes {
        overrides: HashMap::new(),
        defaults: HashMap::new(),
    };
    for child in root.children_named(NS_CT, "Override") {
        let partname = child
            .attr("PartName")
            .ok_or("'NoneType' object has no attribute 'lower'")?;
        map.overrides.insert(
            partname.to_lowercase(),
            child.attr("ContentType").map(str::to_string),
        );
    }
    for child in root.children_named(NS_CT, "Default") {
        let extension = child
            .attr("Extension")
            .ok_or("'NoneType' object has no attribute 'lower'")?;
        map.defaults.insert(
            extension.to_lowercase(),
            child.attr("ContentType").map(str::to_string),
        );
    }
    Ok(map)
}

/// `_SerializedRelationships.load_from_xml`
fn srels_from_xml(base_uri: &str, xml: Option<&[u8]>) -> PyResult<Vec<SRel>> {
    let Some(xml) = xml else {
        return Ok(Vec::new());
    };
    let root = xml::parse(xml)?;
    if !root.is(NS_PR, "Relationships") {
        return Err("'lxml.etree._Element' object has no attribute 'Relationship_lst'".into());
    }
    Ok(root
        .children_named(NS_PR, "Relationship")
        .map(|rel| SRel {
            base_uri: base_uri.to_string(),
            rid: rel.attr("Id").map(str::to_string),
            reltype: rel.attr("Type").map(str::to_string),
            target_ref: rel.attr("Target").map(str::to_string),
            external: rel.attr("TargetMode").unwrap_or("Internal") == "External",
        })
        .collect())
}

/// `PackageReader._walk_phys_parts` + `_load_serialized_parts`
fn walk_phys_parts(
    zip: &mut Zip<'_>,
    content_types: &ContentTypes,
    srels: &[SRel],
    visited: &mut Vec<String>,
    out: &mut Vec<SPart>,
) -> PyResult<()> {
    for srel in srels {
        if srel.external {
            continue;
        }
        let Some(target) = &srel.target_ref else {
            return Err(
                "join() argument must be str, bytes, or os.PathLike object, not 'NoneType'".into(),
            );
        };
        let partname = from_rel_ref(&srel.base_uri, target);
        if visited.contains(&partname) {
            continue;
        }
        visited.push(partname.clone());
        let (part_base, _) = posix_split(&partname);
        let rels_xml = zip.rels_xml_for(&partname)?;
        let part_srels = srels_from_xml(&part_base, rels_xml.as_deref())?;
        let blob = zip.read(&partname[1..])?;
        let content_type = content_types.get(&partname)?;
        out.push(SPart {
            partname: partname.clone(),
            content_type,
            reltype: srel.reltype.clone(),
            blob,
        });
        walk_phys_parts(zip, content_types, &part_srels, visited, out)?;
    }
    Ok(())
}

fn extract(bytes: &[u8]) -> PyResult<String> {
    let archive = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|_| "File is not a zip file".to_string())?;
    let mut zip = Zip { archive };
    // PackageReader.from_file
    let content_types = content_types_from_xml(&zip.read("[Content_Types].xml")?)?;
    let pkg_rels_xml = zip.rels_xml_for("/")?;
    let pkg_srels = srels_from_xml("/", pkg_rels_xml.as_deref())?;
    let mut sparts = Vec::new();
    walk_phys_parts(
        &mut zip,
        &content_types,
        &pkg_srels,
        &mut Vec::new(),
        &mut sparts,
    )?;

    // Unmarshaller._unmarshal_parts: parts with an XmlPart class are parsed
    let mut documents: HashMap<String, xml::Element> = HashMap::new();
    for part in &sparts {
        let is_image = part.reltype.as_deref() == Some(RT_IMAGE);
        if !is_image
            && part
                .content_type
                .as_deref()
                .is_some_and(|ct| XML_PART_CONTENT_TYPES.contains(&ct))
        {
            documents.insert(part.partname.clone(), xml::parse(&part.blob)?);
        }
    }

    // Package.main_document_part: Relationships keyed by rId (a repeated rId replaces the earlier relationship in place)
    let mut rels: Vec<&SRel> = Vec::new();
    for srel in &pkg_srels {
        match rels.iter().position(|r| r.rid == srel.rid) {
            Some(i) => rels[i] = srel,
            None => rels.push(srel),
        }
    }
    let matching: Vec<&&SRel> = rels
        .iter()
        .filter(|r| r.reltype.as_deref() == Some(RT_OFFICE_DOCUMENT))
        .collect();
    if matching.is_empty() {
        return Err(key_error(&format!(
            "no relationship of type '{RT_OFFICE_DOCUMENT}' in collection"
        )));
    }
    if matching.len() > 1 {
        return Err(format!(
            "multiple relationships of type '{RT_OFFICE_DOCUMENT}' in collection"
        ));
    }
    let rel = matching[0];
    if rel.external {
        return Err(
            "target_part property on _Relationship is undefined when target mode is External"
                .into(),
        );
    }
    let partname = from_rel_ref("/", rel.target_ref.as_deref().unwrap_or_default());
    let part = sparts
        .iter()
        .find(|p| p.partname == partname)
        .ok_or("part missing")?;
    if part.content_type.as_deref() != Some(CT_WML_DOCUMENT_MAIN) {
        // the path of the temporary upload is normalised to its file name
        return Err(format!(
            "file 'upload.docx' is not a Word file, content type is '{}'",
            part.content_type.as_deref().unwrap_or("None")
        ));
    }
    let root = documents.get(&partname).ok_or("part not parsed")?;
    if !root.is(NS_WML, "document") {
        return Err("'lxml.etree._Element' object has no attribute 'body'".into());
    }
    let Some(body) = root.children_named(NS_WML, "body").next() else {
        return Err("'NoneType' object has no attribute 'p_lst'".into());
    };
    Ok(body
        .children_named(NS_WML, "p")
        .map(paragraph_text)
        .collect::<Vec<_>>()
        .join("\n"))
}

// ---------------------------------------------------------------------------------------------------------------------
// text (docx/oxml/text/paragraph.py, run.py, hyperlink.py)
// ---------------------------------------------------------------------------------------------------------------------

/// `CT_P.text`: `"".join(e.text for e in xpath("w:r | w:hyperlink"))`
fn paragraph_text(p: &xml::Element) -> String {
    let mut out = String::new();
    for child in &p.children {
        if child.is(NS_WML, "r") {
            out.push_str(&run_text(child));
        } else if child.is(NS_WML, "hyperlink") {
            // CT_Hyperlink.text: the w:r children
            for r in child.children_named(NS_WML, "r") {
                out.push_str(&run_text(r));
            }
        }
    }
    out
}

/// `CT_R.text`: `str(e)` of `w:br | w:cr | w:noBreakHyphen | w:ptab | w:t | w:tab`, in document order
fn run_text(r: &xml::Element) -> String {
    let mut out = String::new();
    for e in r
        .children
        .iter()
        .filter(|e| e.ns.as_deref() == Some(NS_WML))
    {
        match e.name.as_str() {
            "br" => {
                // CT_Br.__str__: only a text-wrapping break (the default) is a newline; page and column breaks are ""
                if e.attr_ns(NS_WML, "type").unwrap_or("textWrapping") == "textWrapping" {
                    out.push('\n');
                }
            }
            "cr" => out.push('\n'),
            "noBreakHyphen" => out.push('-'),
            "ptab" | "tab" => out.push('\t'),
            "t" => out.push_str(&e.text),
            _ => {}
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------------------------------
// XML (lxml with `remove_blank_text=True, resolve_entities=False`, i.e. libxml2)
// ---------------------------------------------------------------------------------------------------------------------

mod xml {
    //! A small namespace-aware XML parser that behaves like libxml2 for the documents python-docx reads and reports libxml2's first
    //! error the way lxml formats it: `<message>, line L, column C (<string>, line L)`.
    //!
    //! `remove_blank_text` matters for `w:t`: libxml2 (`areBlanks`) drops a whitespace-only text chunk that is the first content of an
    //! element and is followed by `<` (but not `</`) unless `xml:space="preserve"` is in scope.

    const NS_XML: &str = "http://www.w3.org/XML/1998/namespace";

    #[derive(Debug, Default)]
    pub struct Element {
        pub ns: Option<String>,
        pub name: String,
        /// `(namespace, local name, value)`
        attrs: Vec<(Option<String>, String, String)>,
        /// lxml `.text`: the text before the first child node (element, comment or processing instruction)
        pub text: String,
        /// the element children
        pub children: Vec<Element>,
    }

    impl Element {
        pub fn is(&self, ns: &str, name: &str) -> bool {
            self.ns.as_deref() == Some(ns) && self.name == name
        }

        pub fn children_named<'a>(
            &'a self,
            ns: &'a str,
            name: &'a str,
        ) -> impl Iterator<Item = &'a Element> {
            self.children.iter().filter(move |c| c.is(ns, name))
        }

        /// `element.get("Name")`: an attribute without a namespace
        pub fn attr(&self, name: &str) -> Option<&str> {
            self.attrs
                .iter()
                .find(|(ns, n, _)| ns.is_none() && n == name)
                .map(|(_, _, v)| v.as_str())
        }

        pub fn attr_ns(&self, ns: &str, name: &str) -> Option<&str> {
            self.attrs
                .iter()
                .find(|(a, n, _)| a.as_deref() == Some(ns) && n == name)
                .map(|(_, _, v)| v.as_str())
        }
    }

    pub fn parse(data: &[u8]) -> Result<Element, String> {
        let newlines = data
            .iter()
            .enumerate()
            .filter(|(_, &b)| b == b'\n')
            .map(|(i, _)| i)
            .collect();
        let mut parser = Parser {
            data,
            pos: 0,
            ns_scopes: Vec::new(),
            space: vec![false],
            depth: 0,
            newlines,
            bad_at: std::cell::Cell::new(None),
        };
        parser.document().map_err(|e| e.render(data))
    }

    struct XmlError {
        message: String,
        /// byte offset of the cursor when libxml2 raised the error
        pos: usize,
    }

    impl XmlError {
        fn render(&self, data: &[u8]) -> String {
            let upto = &data[..self.pos.min(data.len())];
            let line = upto.iter().filter(|&&b| b == b'\n').count() + 1;
            let line_start = upto.iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
            // columns count characters
            let column = upto[line_start..]
                .iter()
                .filter(|&&b| (b & 0xc0) != 0x80)
                .count()
                + 1;
            format!(
                "{}, line {line}, column {column} (<string>, line {line})",
                self.message
            )
        }
    }

    type XResult<T> = Result<T, XmlError>;

    struct Parser<'a> {
        data: &'a [u8],
        pos: usize,
        /// namespace bindings in scope: `(prefix, uri)`, innermost last
        ns_scopes: Vec<(String, String)>,
        /// `xml:space="preserve"` in scope
        space: Vec<bool>,
        depth: usize,
        /// byte offsets of every `\n` (for line numbers)
        newlines: Vec<usize>,
        /// where a decoding scanner met bytes that are not UTF-8
        bad_at: std::cell::Cell<Option<usize>>,
    }

    fn is_blank(b: u8) -> bool {
        matches!(b, b' ' | b'\t' | b'\n' | b'\r')
    }

    fn is_name_start(c: char) -> bool {
        matches!(c, ':' | 'A'..='Z' | '_' | 'a'..='z')
            || matches!(u32::from(c), 0xc0..=0xd6 | 0xd8..=0xf6 | 0xf8..=0x2ff | 0x370..=0x37d | 0x37f..=0x1fff | 0x200c..=0x200d | 0x2070..=0x218f | 0x2c00..=0x2fef | 0x3001..=0xd7ff | 0xf900..=0xfdcf | 0xfdf0..=0xfffd | 0x10000..=0xeffff)
    }

    fn is_name_char(c: char) -> bool {
        is_name_start(c)
            || matches!(c, '-' | '.' | '0'..='9')
            || matches!(u32::from(c), 0xb7 | 0x300..=0x36f | 0x203f..=0x2040)
    }

    fn is_xml_char(c: u32) -> bool {
        matches!(c, 0x9 | 0xa | 0xd | 0x20..=0xd7ff | 0xe000..=0xfffd | 0x10000..=0x10ffff)
    }

    impl Parser<'_> {
        fn err<T>(&self, message: impl Into<String>, pos: usize) -> XResult<T> {
            // libxml2 decodes lazily: the error is "Invalid bytes" when a scanner that decodes characters (names, text) stopped on them
            if self.bad_at.get() == Some(pos) {
                return Err(self.bad_bytes(pos));
            }
            Err(XmlError {
                message: message.into(),
                pos,
            })
        }

        fn bad_bytes(&self, pos: usize) -> XmlError {
            let rest = &self.data[pos..];
            let needed = match rest[0] {
                0xc2..=0xdf => 2,
                0xe0..=0xef => 3,
                0xf0..=0xf4 => 4,
                _ => 0,
            };
            let truncated =
                needed > rest.len() && rest[1..].iter().all(|b| (0x80..0xc0).contains(b));
            let message = if truncated {
                format!("Incomplete UTF-8 sequence starting with {:02X}", rest[0])
            } else {
                "Invalid bytes in character encoding".into()
            };
            XmlError { message, pos }
        }

        fn peek(&self) -> Option<u8> {
            self.data.get(self.pos).copied()
        }

        fn starts_with(&self, s: &str) -> bool {
            self.data[self.pos..].starts_with(s.as_bytes())
        }

        fn skip_blanks(&mut self) {
            while self.peek().is_some_and(is_blank) {
                self.pos += 1;
            }
        }

        /// The character at `pos` (`None` at the end or when the bytes are not UTF-8).
        fn char_at(&self, pos: usize) -> Option<(char, usize)> {
            let rest = &self.data[pos.min(self.data.len())..];
            let decoded = (|| {
                let len = match rest.first()? {
                    0..=0x7f => 1,
                    0xc2..=0xdf => 2,
                    0xe0..=0xef => 3,
                    0xf0..=0xf4 => 4,
                    _ => return None,
                };
                let s = std::str::from_utf8(rest.get(..len)?).ok()?;
                s.chars().next().map(|c| (c, len))
            })();
            if decoded.is_none() && !rest.is_empty() {
                self.bad_at.set(Some(pos));
            }
            decoded
        }

        // [1] document ::= prolog element Misc*
        fn document(&mut self) -> XResult<Element> {
            if self.data.is_empty() {
                return self.err("Document is empty", 0);
            }
            if self.starts_with("\u{feff}") {
                self.pos += 3;
            }
            if self.starts_with("<?xml")
                && self.data.get(self.pos + 5).is_some_and(|&b| is_blank(b))
            {
                self.xml_decl()?;
            }
            self.misc()?;
            if self.starts_with("<!DOCTYPE") {
                self.doctype()?;
                self.misc()?;
            }
            if self.peek() != Some(b'<') {
                return self.err("Start tag expected, '<' not found", self.pos);
            }
            let root = self.element()?;
            self.misc()?;
            if self.pos < self.data.len() {
                return self.err("Extra content at the end of the document", self.pos);
            }
            Ok(root)
        }

        /// `xmlParseXMLDecl`
        fn xml_decl(&mut self) -> XResult<()> {
            self.pos += 5;
            self.skip_blanks();
            if !self.starts_with("version") {
                return self.err("Malformed declaration expecting version", self.pos);
            }
            self.pos += 7;
            self.decl_eq()?;
            let quote = self.decl_quote()?;
            // xmlParseVersionNum: [0-9]+ '.' [0-9]+  (anything else ends the number)
            let digits = |p: &mut Self| {
                let start = p.pos;
                while p.peek().is_some_and(|b| b.is_ascii_digit()) {
                    p.pos += 1;
                }
                p.pos > start
            };
            if digits(self) && self.peek() == Some(b'.') {
                self.pos += 1;
                digits(self);
            }
            self.decl_close(quote)?;
            let mut blank = self.peek().is_some_and(is_blank);
            self.skip_blanks();
            if self.starts_with("?>") {
                self.pos += 2;
                return Ok(());
            }
            if !blank {
                return self.err("Blank needed here", self.pos);
            }
            if self.starts_with("encoding") {
                self.pos += 8;
                self.decl_eq()?;
                let quote = self.decl_quote()?;
                if !self.peek().is_some_and(|b| b.is_ascii_alphabetic()) {
                    return self.err("Invalid XML encoding name", self.pos);
                }
                let start = self.pos;
                while self
                    .peek()
                    .is_some_and(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
                {
                    self.pos += 1;
                }
                let value = String::from_utf8_lossy(&self.data[start..self.pos]).into_owned();
                self.decl_close(quote)?;
                let known = [
                    "utf-8",
                    "utf8",
                    "us-ascii",
                    "ascii",
                    "iso-8859-1",
                    "latin1",
                    "iso-8859-15",
                    "windows-1252",
                    "cp1252",
                ];
                if !known.iter().any(|k| k.eq_ignore_ascii_case(&value)) {
                    return self.err(format!("Unsupported encoding: {value}"), self.pos);
                }
                blank = self.peek().is_some_and(is_blank);
                self.skip_blanks();
                if self.starts_with("?>") {
                    self.pos += 2;
                    return Ok(());
                }
                if !blank {
                    return self.err("Blank needed here", self.pos);
                }
            }
            if self.starts_with("standalone") {
                self.pos += 10;
                self.decl_eq()?;
                let quote = self.decl_quote()?;
                if self.starts_with("no") {
                    self.pos += 2;
                } else if self.starts_with("yes") {
                    self.pos += 3;
                } else {
                    return self.err("standalone accepts only 'yes' or 'no'", self.pos);
                }
                self.decl_close(quote)?;
                self.skip_blanks();
            }
            if self.starts_with("?>") {
                self.pos += 2;
                Ok(())
            } else if self.peek() == Some(b'>') {
                self.err("XML declaration must end with ?>", self.pos)
            } else {
                self.err("parsing XML declaration: '?>' expected", self.pos)
            }
        }

        /// `=` of a declaration pseudo-attribute
        fn decl_eq(&mut self) -> XResult<()> {
            self.skip_blanks();
            if self.peek() != Some(b'=') {
                return self.err("expected '='", self.pos);
            }
            self.pos += 1;
            self.skip_blanks();
            Ok(())
        }

        fn decl_quote(&mut self) -> XResult<u8> {
            let Some(quote @ (b'"' | b'\'')) = self.peek() else {
                return self.err("String not started expecting ' or \"", self.pos);
            };
            self.pos += 1;
            Ok(quote)
        }

        fn decl_close(&mut self, quote: u8) -> XResult<()> {
            if self.peek() != Some(quote) {
                return self.err("String not closed expecting \" or '", self.pos);
            }
            self.pos += 1;
            Ok(())
        }

        fn find(&self, needle: &str, from: usize) -> Option<usize> {
            let n = needle.as_bytes();
            self.data
                .get(from..)?
                .windows(n.len())
                .position(|w| w == n)
                .map(|i| i + from)
        }

        /// Misc ::= Comment | PI | S
        fn misc(&mut self) -> XResult<()> {
            loop {
                self.skip_blanks();
                if self.starts_with("<!--") {
                    self.comment()?;
                } else if self.starts_with("<?") {
                    self.pi()?;
                } else {
                    return Ok(());
                }
            }
        }

        fn comment(&mut self) -> XResult<()> {
            match self.find("-->", self.pos + 4) {
                Some(end) => {
                    self.check_utf8(self.pos + 4, end)?;
                    self.pos = end + 3;
                    Ok(())
                }
                None => {
                    self.check_utf8(self.pos + 4, self.data.len())?;
                    self.err("Comment not terminated", self.data.len())
                }
            }
        }

        fn pi(&mut self) -> XResult<()> {
            self.pos += 2;
            let Some(target) = self.name() else {
                return self.err("xmlParsePI : no target name", self.pos);
            };
            if target == "xml" {
                return self.err(
                    "XML declaration allowed only at the start of the document",
                    self.pos,
                );
            }
            if self.starts_with("?>") {
                self.pos += 2;
                return Ok(());
            }
            if !self.peek().is_some_and(is_blank) {
                return self.err(format!("ParsePI: PI {target} space expected"), self.pos);
            }
            self.skip_blanks();
            match self.find("?>", self.pos) {
                Some(end) => {
                    self.check_utf8(self.pos, end)?;
                    self.pos = end + 2;
                    Ok(())
                }
                None => {
                    self.check_utf8(self.pos, self.data.len())?;
                    self.err(
                        format!("ParsePI: PI {target} never end ..."),
                        self.data.len(),
                    )
                }
            }
        }

        /// The first bytes in `start..end` that are not UTF-8 are an "Invalid bytes" error.
        fn check_utf8(&self, start: usize, end: usize) -> XResult<()> {
            if let Err(e) = std::str::from_utf8(&self.data[start..end]) {
                let at = start + e.valid_up_to();
                if e.error_len().is_some() || end == self.data.len() {
                    return Err(self.bad_bytes(at));
                }
            }
            Ok(())
        }

        /// A DOCTYPE is skipped (entity declarations are not supported).
        fn doctype(&mut self) -> XResult<()> {
            let mut depth = 0usize;
            let mut quote = None;
            while let Some(b) = self.peek() {
                self.pos += 1;
                match (quote, b) {
                    (Some(q), b) if b == q => quote = None,
                    (Some(_), _) => {}
                    (None, b'"' | b'\'') => quote = Some(b),
                    (None, b'[') => depth += 1,
                    (None, b']') => depth = depth.saturating_sub(1),
                    (None, b'>') if depth == 0 => return Ok(()),
                    _ => {}
                }
            }
            self.err("DOCTYPE improperly terminated", self.data.len())
        }

        /// Parses a Name (with `:`) at the cursor; `None` when there is none.
        fn name(&mut self) -> Option<String> {
            let (first, len) = self.char_at(self.pos)?;
            if !is_name_start(first) {
                return None;
            }
            let start = self.pos;
            self.pos += len;
            while let Some((c, len)) = self.char_at(self.pos) {
                if !is_name_char(c) {
                    break;
                }
                self.pos += len;
            }
            Some(String::from_utf8_lossy(&self.data[start..self.pos]).into_owned())
        }

        /// `xmlParseNCName`: a Name without colons
        fn ncname(&mut self) -> Option<String> {
            let (first, len) = self.char_at(self.pos)?;
            if !is_name_start(first) || first == ':' {
                return None;
            }
            let start = self.pos;
            self.pos += len;
            while let Some((c, len)) = self.char_at(self.pos) {
                if !is_name_char(c) || c == ':' {
                    break;
                }
                self.pos += len;
            }
            Some(String::from_utf8_lossy(&self.data[start..self.pos]).into_owned())
        }

        /// `xmlParseNmtoken`
        fn nmtoken(&mut self) -> String {
            let start = self.pos;
            while let Some((c, len)) = self.char_at(self.pos) {
                if !is_name_char(c) {
                    break;
                }
                self.pos += len;
            }
            String::from_utf8_lossy(&self.data[start..self.pos]).into_owned()
        }

        /// `xmlParseQName`: `(prefix, local)`; `None` when there is no name. The namespace errors of libxml2 are reported as they come.
        fn qname(&mut self) -> XResult<Option<(Option<String>, String)>> {
            let Some(l) = self.ncname() else {
                if self.peek() == Some(b':') {
                    if let Some(n) = self.name() {
                        return self.err(format!("Failed to parse QName '{n}'"), self.pos);
                    }
                }
                return Ok(None);
            };
            if self.peek() != Some(b':') {
                return Ok(Some((None, l)));
            }
            self.pos += 1;
            let p = l;
            let Some(l) = self.ncname() else {
                let token = self.nmtoken();
                return self.err(format!("Failed to parse QName '{p}:{token}'"), self.pos);
            };
            if self.peek() == Some(b':') {
                self.pos += 1;
                let rest = self.name().unwrap_or_default();
                return self.err(format!("Failed to parse QName '{p}:{l}:{rest}'"), self.pos);
            }
            Ok(Some((Some(p), l)))
        }

        fn lookup_ns(&self, prefix: &str) -> Option<&str> {
            if prefix == "xml" {
                return Some(NS_XML);
            }
            self.ns_scopes
                .iter()
                .rev()
                .find(|(p, _)| p == prefix)
                .map(|(_, u)| u.as_str())
                .filter(|u| !(prefix.is_empty() && u.is_empty()))
        }

        fn line_of(&self, pos: usize) -> usize {
            self.newlines.partition_point(|&i| i < pos) + 1
        }

        /// `element ::= EmptyElemTag | STag content ETag`, the cursor is at `<`.
        fn element(&mut self) -> XResult<Element> {
            self.depth += 1;
            if self.depth > 256 {
                return self.err(
                    "Excessive depth in document: 256 use XML_PARSE_HUGE option",
                    self.pos,
                );
            }
            let start_line = self.line_of(self.pos);
            self.pos += 1;
            let Some((prefix, local_name)) = self.qname()? else {
                return self.err("StartTag: invalid element name", self.pos);
            };
            let scope_mark = self.ns_scopes.len();
            let mut raw_attrs: Vec<(String, String)> = Vec::new();
            self.skip_blanks();
            while let Some(b) = self.peek() {
                if b == b'>'
                    || (b == b'/' && self.data.get(self.pos + 1) == Some(&b'>'))
                    || !(b >= 0x20 || matches!(b, 9 | 10 | 13))
                {
                    break;
                }
                let Some((attr_prefix, attr_local)) = self.qname()? else {
                    return self.err("error parsing attribute name", self.pos);
                };
                let attr_name = match &attr_prefix {
                    Some(p) => format!("{p}:{attr_local}"),
                    None => attr_local.clone(),
                };
                self.skip_blanks();
                if self.peek() != Some(b'=') {
                    return self.err(
                        format!("Specification mandates value for attribute {attr_local}"),
                        self.pos,
                    );
                }
                self.pos += 1;
                self.skip_blanks();
                let value = self.att_value()?;
                if raw_attrs.iter().any(|(n, _)| *n == attr_name) {
                    return self.err(format!("Attribute {attr_local} redefined"), self.pos);
                }
                if attr_name == "xmlns" || attr_name.starts_with("xmlns:") {
                    if value.chars().any(|c| {
                        c.is_ascii_control()
                            || matches!(
                                c,
                                ' ' | '<' | '>' | '"' | '{' | '}' | '|' | '\\' | '^' | '`'
                            )
                    }) {
                        return self.err(
                            format!("{attr_name}: '{value}' is not a valid URI"),
                            self.pos,
                        );
                    }
                    if value.is_empty() && attr_name != "xmlns" {
                        return self.err(
                            format!("{attr_name}: Empty XML namespace is not allowed"),
                            self.pos,
                        );
                    }
                }
                raw_attrs.push((attr_name, value));
                if matches!(self.peek(), Some(b'>'))
                    || (self.peek() == Some(b'/') && self.data.get(self.pos + 1) == Some(&b'>'))
                {
                    break;
                }
                if !self.peek().is_some_and(is_blank) {
                    // raw bytes are tested here, nothing is decoded: no "Invalid bytes" error
                    return Err(XmlError {
                        message: "attributes construct error".into(),
                        pos: self.pos,
                    });
                }
                self.skip_blanks();
            }
            // namespace declarations first, then resolve the names
            for (n, v) in &raw_attrs {
                if n == "xmlns" {
                    self.ns_scopes.push((String::new(), v.clone()));
                } else if let Some(prefix) = n.strip_prefix("xmlns:") {
                    self.ns_scopes.push((prefix.to_string(), v.clone()));
                }
            }
            let mut attrs = Vec::new();
            for (n, v) in &raw_attrs {
                if n == "xmlns" || n.starts_with("xmlns:") {
                    continue;
                }
                match n.split_once(':') {
                    Some((prefix, local)) => match self.lookup_ns(prefix) {
                        Some(uri) => attrs.push((Some(uri.to_string()), local.to_string(), v.clone())),
                        None => return self.err(format!("Namespace prefix {prefix} for {local} on {local_name} is not defined"), self.pos),
                    },
                    None => attrs.push((None, n.clone(), v.clone())),
                }
            }
            let ns = match &prefix {
                Some(prefix) => match self.lookup_ns(prefix) {
                    Some(uri) => Some(uri.to_string()),
                    None => {
                        return self.err(
                            format!("Namespace prefix {prefix} on {local_name} is not defined"),
                            self.pos,
                        )
                    }
                },
                None => self.lookup_ns("").map(str::to_string),
            };
            let preserve = match attrs
                .iter()
                .find(|(a, n, _)| a.as_deref() == Some(NS_XML) && n == "space")
                .map(|(_, _, v)| v.as_str())
            {
                Some("preserve") => true,
                Some("default") => false,
                _ => *self.space.last().unwrap_or(&false),
            };
            let mut element = Element {
                ns,
                name: local_name.clone(),
                attrs,
                text: String::new(),
                children: Vec::new(),
            };
            match self.peek() {
                Some(b'/') if self.data.get(self.pos + 1) == Some(&b'>') => {
                    self.pos += 2;
                }
                Some(b'>') => {
                    self.pos += 1;
                    self.space.push(preserve);
                    self.content(
                        &mut element,
                        (prefix.as_deref(), &local_name),
                        start_line,
                        preserve,
                    )?;
                    self.space.pop();
                }
                _ => {
                    return self.err(
                        format!("Couldn't find end of Start Tag {local_name} line {start_line}"),
                        self.pos,
                    )
                }
            }
            self.ns_scopes.truncate(scope_mark);
            self.depth -= 1;
            Ok(element)
        }

        /// An attribute value (the cursor is at the opening quote).
        fn att_value(&mut self) -> XResult<String> {
            let Some(quote @ (b'"' | b'\'')) = self.peek() else {
                return self.err("AttValue: \" or ' expected", self.pos);
            };
            self.pos += 1;
            let mut out = String::new();
            loop {
                let Some(b) = self.peek() else {
                    return self.err("AttValue: ' expected", self.pos);
                };
                if b == quote {
                    self.pos += 1;
                    return Ok(out);
                }
                match b {
                    b'<' => {
                        return self.err("Unescaped '<' not allowed in attributes values", self.pos)
                    }
                    b'&' => out.push_str(&self.reference()?),
                    b'\t' | b'\n' | b'\r' => {
                        if b == b'\r' && self.data.get(self.pos + 1) == Some(&b'\n') {
                            self.pos += 1;
                        }
                        self.pos += 1;
                        out.push(' ');
                    }
                    _ => {
                        if matches!(b, 0..=8 | 11 | 12 | 14..=31) {
                            return self.err("invalid character in attribute value", self.pos);
                        }
                        out.push(self.plain_char()?);
                    }
                }
            }
        }

        /// One ordinary character (validated); the cursor moves past it.
        fn plain_char(&mut self) -> XResult<char> {
            match self.char_at(self.pos) {
                Some((c, len)) => {
                    if !is_xml_char(u32::from(c)) {
                        return self.err(
                            if c == '\0' {
                                "Invalid character: Char 0x0 out of allowed range\n".to_string()
                            } else {
                                format!("PCDATA invalid Char value {}", u32::from(c))
                            },
                            self.pos,
                        );
                    }
                    self.pos += len;
                    Ok(c)
                }
                None => self.err("Invalid bytes in character encoding", self.pos),
            }
        }

        /// A reference (the cursor is at `&`): the text it stands for.
        fn reference(&mut self) -> XResult<String> {
            self.pos += 1;
            if self.peek() == Some(b'#') {
                self.pos += 1;
                let hex = self.peek() == Some(b'x');
                if hex {
                    self.pos += 1;
                }
                let mut value: u32 = 0;
                loop {
                    let Some(b) = self.peek() else {
                        return self.err(
                            if hex {
                                "CharRef: invalid hexadecimal value"
                            } else {
                                "CharRef: invalid decimal value"
                            },
                            self.pos,
                        );
                    };
                    if b == b';' {
                        self.pos += 1;
                        break;
                    }
                    let digit = if hex {
                        (b as char).to_digit(16)
                    } else {
                        (b as char).to_digit(10)
                    };
                    let Some(digit) = digit else {
                        return self.err(
                            if hex {
                                "CharRef: invalid hexadecimal value"
                            } else {
                                "CharRef: invalid decimal value"
                            },
                            self.pos,
                        );
                    };
                    value = (value
                        .saturating_mul(if hex { 16 } else { 10 })
                        .saturating_add(digit))
                    .min(0x11_0000);
                    self.pos += 1;
                }
                if value >= 0x11_0000 {
                    return self.err(
                        "xmlParseCharRef: character reference out of bounds",
                        self.pos,
                    );
                }
                return match char::from_u32(value).filter(|c| is_xml_char(u32::from(*c))) {
                    Some(c) => Ok(c.to_string()),
                    None => self.err(
                        format!("xmlParseCharRef: invalid xmlChar value {value}"),
                        self.pos,
                    ),
                };
            }
            let Some(name) = self.name() else {
                return self.err("xmlParseEntityRef: no name", self.pos);
            };
            if self.peek() != Some(b';') {
                return self.err("EntityRef: expecting ';'", self.pos);
            }
            self.pos += 1;
            match name.as_str() {
                "lt" => Ok("<".into()),
                "gt" => Ok(">".into()),
                "amp" => Ok("&".into()),
                "quot" => Ok("\"".into()),
                "apos" => Ok("'".into()),
                _ => self.err(format!("Entity '{name}' not defined"), self.pos),
            }
        }

        /// `content` up to and including the end tag.
        fn content(
            &mut self,
            element: &mut Element,
            open: (Option<&str>, &str),
            start_line: usize,
            preserve: bool,
        ) -> XResult<()> {
            let mut lead_open = true; // no child node (element, comment, PI) yet: `.text` is still collecting
            let mut have_text = false; // a text node exists already (areBlanks: the last child is text)
            loop {
                let Some(b) = self.peek() else {
                    return self.err(
                        format!("Premature end of data in tag {} line {start_line}", open.1),
                        self.pos,
                    );
                };
                match b {
                    b'<' => {
                        if self.starts_with("</") {
                            return self.end_tag(open, start_line);
                        } else if self.starts_with("<!--") {
                            self.comment()?;
                            lead_open = false;
                        } else if self.starts_with("<![CDATA[") {
                            let Some(end) = self.find("]]>", self.pos + 9) else {
                                self.check_utf8(self.pos + 9, self.data.len())?;
                                // libxml2 prints (at most 50 characters of) what it collected: all but the last two characters
                                let rest = String::from_utf8_lossy(&self.data[self.pos + 9..])
                                    .into_owned();
                                let chars: Vec<char> = rest.chars().collect();
                                let mut shown = String::new();
                                for c in &chars[..chars.len().saturating_sub(2)] {
                                    if shown.len() + c.len_utf8() > 50 {
                                        break; // C's %.50s counts bytes
                                    }
                                    shown.push(*c);
                                }
                                return self.err(
                                    format!("CData section not finished\n{shown}"),
                                    self.data.len(),
                                );
                            };
                            self.check_utf8(self.pos + 9, end)?;
                            let text = String::from_utf8_lossy(&self.data[self.pos + 9..end])
                                .replace("\r\n", "\n")
                                .replace('\r', "\n");
                            self.pos = end + 3;
                            if lead_open {
                                element.text.push_str(&text);
                            }
                            have_text = true;
                        } else if self.starts_with("<?") {
                            self.pi()?;
                            lead_open = false;
                        } else {
                            let child = self.element()?;
                            element.children.push(child);
                            lead_open = false;
                        }
                    }
                    b'&' => {
                        let text = self.reference()?;
                        if lead_open {
                            element.text.push_str(&text);
                        }
                        have_text = true;
                    }
                    _ => {
                        // a run of character data
                        let start = self.pos;
                        let mut text = String::new();
                        while let Some(b) = self.peek() {
                            if b == b'<' || b == b'&' {
                                break;
                            }
                            if b == b']' && self.starts_with("]]>") {
                                return self.err("Sequence ']]>' not allowed in content", self.pos);
                            }
                            if b == b'\r' {
                                self.pos += if self.data.get(self.pos + 1) == Some(&b'\n') {
                                    2
                                } else {
                                    1
                                };
                                text.push('\n');
                            } else {
                                text.push(self.plain_char()?);
                            }
                        }
                        // areBlanks (remove_blank_text)
                        let blank = self.data[start..self.pos].iter().all(|&b| is_blank(b));
                        let followed_by_markup =
                            self.peek() == Some(b'<') && !self.starts_with("</");
                        let dropped =
                            blank && !preserve && !have_text && lead_open && followed_by_markup;
                        if !dropped {
                            if lead_open {
                                element.text.push_str(&text);
                            }
                            have_text = true;
                        }
                    }
                }
            }
        }

        /// `xmlParseEndTag2` (the cursor is at `</`)
        fn end_tag(&mut self, open: (Option<&str>, &str), start_line: usize) -> XResult<()> {
            self.pos += 2;
            let save = self.pos;
            let expected = match open.0 {
                Some(p) => format!("{p}:{}", open.1),
                None => open.1.to_string(),
            };
            let mut found = None;
            if self.starts_with(&expected)
                && self
                    .data
                    .get(self.pos + expected.len())
                    .is_some_and(|&b| b == b'>' || is_blank(b))
            {
                self.pos += expected.len();
            } else {
                self.pos = save;
                let parsed = if open.0.is_none() {
                    self.name()
                } else {
                    self.qname()?.map(|(_, l)| l)
                };
                found = Some(parsed.unwrap_or_else(|| "unparsable".to_string()));
            }
            self.skip_blanks();
            if self.peek() != Some(b'>') {
                return self.err("expected '>'", self.pos);
            }
            self.pos += 1;
            if let Some(found) = found {
                return self.err(
                    format!(
                        "Opening and ending tag mismatch: {} line {start_line} and {found}",
                        open.1
                    ),
                    self.pos,
                );
            }
            Ok(())
        }
    }
}
