#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct URI {
    pub scheme: internment::Intern<String>,
    pub path: internment::Intern<String>,
}

impl URI {
    pub fn new(scheme: impl AsRef<str>, path: impl AsRef<str>) -> Self {
        URI {
            scheme: internment::Intern::from_ref(scheme.as_ref()),
            path: internment::Intern::from_ref(path.as_ref()),
        }
    }

    pub fn exists(&self) -> bool {
        fs::metadata(self.path.as_ref()).is_ok()
    }
}

impl Default for URI {
    fn default() -> Self {
        URI {
            scheme: internment::Intern::from_ref("file"),
            path: internment::Intern::from_ref("undefined"),
        }
    }
}

impl fmt::Display for URI {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}://{}", self.scheme, self.path)
    }
}

impl From<&str> for URI {
    fn from(s: &str) -> Self {
        let parts: Vec<&str> = s.splitn(2, "://").collect();
        if parts.len() == 2 {
            URI::new(parts[0], parts[1])
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
use std::{fmt, fs, ops};

use serde::{Deserialize, Serialize};

fn get_line_index_cache() -> &'static Mutex<HashMap<u64, Vec<usize>>> {
    static CACHE: OnceLock<Mutex<HashMap<u64, Vec<usize>>>> = OnceLock::new();
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

    let mut map = cache.lock().unwrap();
    if let Some(index) = map.get(&hash) {
        return index.clone();
    }

    let index = build_line_index(text);
    map.insert(hash, index.clone());
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
