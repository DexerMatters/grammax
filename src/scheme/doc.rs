#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct URI {
    pub scheme: internment::Intern<String>,
    pub authority: Option<internment::Intern<String>>,
    pub path: internment::Intern<String>,
}

impl URI {
    pub fn new(scheme: impl AsRef<str>, path: impl AsRef<str>) -> Self {
        URI {
            scheme: internment::Intern::from_ref(scheme.as_ref()),
            authority: None,
            path: internment::Intern::from_ref(path.as_ref()),
        }
    }

    pub fn new_with_authority(
        scheme: impl AsRef<str>,
        authority: impl AsRef<str>,
        path: impl AsRef<str>,
    ) -> Self {
        URI {
            scheme: internment::Intern::from_ref(scheme.as_ref()),
            authority: Some(internment::Intern::from_ref(authority.as_ref())),
            path: internment::Intern::from_ref(path.as_ref()),
        }
    }

    pub fn valid(&self) -> bool {
        if self.scheme.is_empty() || self.path.is_empty() {
            return false;
        }
        // File scheme requires the path exists on the filesystem
        if self.scheme.as_ref() == "file" {
            return std::path::Path::new(self.path.as_ref()).exists();
        } else if self.scheme.as_ref() == "http" || self.scheme.as_ref() == "https" {
            // For http/https, we can do a simple check for authority and path
            return self.authority.is_some() && !self.path.as_ref().is_empty();
        }
        false
    }

    pub fn each_subdirectory(&self, f: impl Fn(URI)) {
        if self.scheme.as_ref() != "file" {
            return;
        }
        let path = std::path::Path::new(self.path.as_ref());
        if !path.is_dir() {
            return;
        }
        if let Ok(entries) = std::fs::read_dir(path) {
            for entry in entries.flatten() {
                f(URI {
                    scheme: self.scheme,
                    authority: self.authority,
                    path: internment::Intern::from_ref(entry.path().to_string_lossy().as_ref()),
                });
            }
        }
    }

    pub fn each_subdirectory_recursive(&self, f: impl Fn(URI)) {
        if self.scheme.as_ref() != "file" {
            return;
        }
        let path = std::path::Path::new(self.path.as_ref());
        if !path.is_dir() {
            return;
        }
        if let Ok(entries) = std::fs::read_dir(path) {
            for entry in entries.flatten() {
                let sub_uri = URI {
                    scheme: self.scheme,
                    authority: self.authority,
                    path: internment::Intern::from_ref(entry.path().to_string_lossy().as_ref()),
                };
                f(sub_uri);
                sub_uri.each_subdirectory_recursive(&f);
            }
        }
    }

    pub fn fetch_text(&self) -> Result<String, std::io::Error> {
        if self.scheme.as_ref() != "file" {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Only file scheme is supported for fetching text",
            ));
        }
        std::fs::read_to_string(self.path.as_ref())
    }

    pub fn fetch_binary(&self) -> Result<Vec<u8>, std::io::Error> {
        if self.scheme.as_ref() != "file" {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Only file scheme is supported for fetching binary data",
            ));
        }
        std::fs::read(self.path.as_ref())
    }
}

impl Default for URI {
    fn default() -> Self {
        URI {
            scheme: internment::Intern::from_ref("file"),
            authority: None,
            path: internment::Intern::from_ref("undefined"),
        }
    }
}

impl fmt::Display for URI {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(authority) = &self.authority {
            write!(f, "{}://{}/{}", self.scheme, authority, self.path)
        } else {
            write!(f, "{}://{}", self.scheme, self.path)
        }
    }
}

impl From<&str> for URI {
    fn from(s: &str) -> Self {
        let parts: Vec<&str> = s.splitn(2, "://").collect();
        if parts.len() == 2 {
            let scheme = parts[0];
            let rest = parts[1];
            // Try to split authority from path
            if let Some(slash_pos) = rest.find('/') {
                let authority = &rest[..slash_pos];
                let path = &rest[slash_pos + 1..];
                URI::new_with_authority(scheme, authority, path)
            } else {
                // No path separator, entire rest is authority
                URI::new_with_authority(scheme, rest, "")
            }
        } else {
            URI::new("file", s)
        }
    }
}

impl From<String> for URI {
    fn from(s: String) -> Self {
        URI::from(s.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DocumentRange {
    pub uri: URI,
    pub range: Range,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DocumentSpan {
    pub uri: URI,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Range {
    pub start: (u32, u32), // (line, column), 0-based
    pub end: (u32, u32),
}

impl fmt::Display for Range {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.start == self.end {
            write!(f, "{}:{}", self.start.0, self.start.1)
        } else {
            write!(
                f,
                "{}:{}-{}:{}",
                self.start.0, self.start.1, self.end.0, self.end.1
            )
        }
    }
}

impl From<ops::Range<(u32, u32)>> for Range {
    fn from(r: ops::Range<(u32, u32)>) -> Self {
        Range {
            start: r.start,
            end: r.end,
        }
    }
}

impl From<((u32, u32), (u32, u32))> for Range {
    fn from(r: ((u32, u32), (u32, u32))) -> Self {
        Range {
            start: r.0,
            end: r.1,
        }
    }
}

impl From<(u32, u32)> for Range {
    fn from(pos: (u32, u32)) -> Self {
        Range {
            start: pos,
            end: pos,
        }
    }
}

impl Range {
    pub fn new(start: (u32, u32), end: (u32, u32)) -> Self {
        Range { start, end }
    }

    /// Convert this line:col range to a byte-offset `Span` using the global line index cache.
    pub fn to_span(&self, text: &str) -> Span {
        Span {
            start: self.start.into_byte(text),
            end: self.end.into_byte(text),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub const fn new(start: usize, end: usize) -> Self {
        Span { start, end }
    }
    pub const fn new_len(offset: usize, len: usize) -> Self {
        Span {
            start: offset,
            end: offset + len,
        }
    }
    pub const fn empty() -> Self {
        Span { start: 0, end: 0 }
    }
    pub const fn len(&self) -> usize {
        if self.end >= self.start {
            self.end - self.start
        } else {
            0
        }
    }
    pub const fn is_empty(&self) -> bool {
        self.start == self.end
    }

    /// Convert this byte-offset span to a `Range` (0-based line:col) using the global line index cache.
    pub fn to_range(&self, text: &str) -> Range {
        Range {
            start: self.start.into_line_col(text),
            end: self.end.into_line_col(text),
        }
    }

    pub fn is_valid(&self) -> bool {
        self.start <= self.end
    }

    pub fn contains(&self, another: Span) -> bool {
        self.start <= another.start && self.end >= another.end
    }
}

impl ops::Add for Span {
    type Output = Span;

    fn add(self, other: Span) -> Span {
        Span {
            start: self.start,
            end: other.end,
        }
    }
}

impl From<Span> for ops::Range<usize> {
    fn from(span: Span) -> Self {
        span.start..span.end
    }
}

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::{Mutex, OnceLock};
use std::{fmt, ops};

use serde::{Deserialize, Serialize};

const MAX_LINE_INDEX_CACHE_ENTRIES: usize = 2048;
const MAX_HASH_BUCKET_ENTRIES: usize = 4;

#[derive(Clone)]
struct CachedLineIndex {
    text: String,
    line_starts: Vec<usize>,
}

fn get_line_index_cache() -> &'static Mutex<HashMap<u64, Vec<CachedLineIndex>>> {
    static CACHE: OnceLock<Mutex<HashMap<u64, Vec<CachedLineIndex>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn hash_text(text: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

fn get_or_create_line_index(text: &str) -> Vec<usize> {
    let hash = hash_text(text);
    let cache = get_line_index_cache();

    let mut map = cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(bucket) = map.get(&hash) {
        if let Some(found) = bucket.iter().find(|entry| entry.text == text) {
            return found.line_starts.clone();
        }
    }

    let index = build_line_index(text);
    if map.len() >= MAX_LINE_INDEX_CACHE_ENTRIES {
        // Keep the cache bounded under pathological document churn.
        map.clear();
    }

    let bucket = map.entry(hash).or_default();
    if bucket.len() >= MAX_HASH_BUCKET_ENTRIES {
        bucket.remove(0);
    }
    bucket.push(CachedLineIndex {
        text: text.to_string(),
        line_starts: index.clone(),
    });

    index
}

fn build_line_index(text: &str) -> Vec<usize> {
    let mut line_starts = vec![0];
    let mut chars = text.char_indices().peekable();

    while let Some((_byte_pos, ch)) = chars.next() {
        if ch == '\n' {
            if let Some(&(next_byte_pos, _)) = chars.peek() {
                line_starts.push(next_byte_pos);
            }
        }
    }

    line_starts
}

fn convert_byte_to_line_col(byte_pos: usize, line_starts: &[usize], text: &str) -> (u32, u32) {
    let line_idx = line_starts
        .binary_search(&byte_pos)
        .unwrap_or_else(|next_idx| next_idx.saturating_sub(1));

    let line_start_byte = line_starts[line_idx];
    let line_end_byte = line_starts.get(line_idx + 1).copied().unwrap_or(text.len());

    let line_text = &text[line_start_byte..line_end_byte];
    let offset_in_line = byte_pos.saturating_sub(line_start_byte);
    let mut col = 0;

    for (byte_offset, ch) in line_text.char_indices() {
        if byte_offset >= offset_in_line {
            break;
        }
        col += ch.len_utf16();
    }

    (line_idx as u32, col as u32)
}

pub trait IntoLineCol {
    fn into_line_col(&self, text: &str) -> (u32, u32);
}

impl IntoLineCol for usize {
    fn into_line_col(&self, text: &str) -> (u32, u32) {
        let line_starts = get_or_create_line_index(text);
        convert_byte_to_line_col(*self, &line_starts, text)
    }
}

pub trait IntoByte {
    fn into_byte(&self, text: &str) -> usize;
}

impl IntoByte for (u32, u32) {
    fn into_byte(&self, text: &str) -> usize {
        let line_starts = get_or_create_line_index(text);
        let (line, col) = self;
        let line = *line as usize;
        if line >= line_starts.len() {
            return text.len();
        }

        let line_start = line_starts[line];
        let line_end = line_starts.get(line + 1).copied().unwrap_or(text.len());
        let line_text = &text[line_start..line_end];

        // Convert UTF-16 column back to byte offset
        let mut byte_offset = 0;
        let mut utf16_col = 0;

        for (offset, ch) in line_text.char_indices() {
            if utf16_col >= *col {
                break;
            }
            utf16_col += ch.len_utf16() as u32;
            byte_offset = offset;
        }

        line_start + byte_offset
    }
}
