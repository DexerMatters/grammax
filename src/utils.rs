use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};
use std::hash::Hash;
use std::{fmt, ops};

#[macro_export]
macro_rules! impl_listener {
    ($name: ty, $($field: ident ($($typ : ty),*)),+) => {
        impl $name {
            pub fn new() -> Self {
                Self::default()
            }
            $(
                pub fn $field(mut self, callback: impl Fn($($typ),*) + Send + 'static) -> Self {
                    self.$field = Some(Box::new(callback));
                    self
                }
            )+
        }
    };
}

#[derive(Clone, Debug, Default)]
pub struct LruCache<K: Clone + Eq + Hash, V: Clone> {
    capacity: usize,
    data: FxHashMap<K, LruEntry<K, V>>,
    head: Option<K>,
    tail: Option<K>,
}

#[derive(Clone, Debug)]
struct LruEntry<K: Clone, V: Clone> {
    value: V,
    prev: Option<K>,
    next: Option<K>,
}

impl<K: Clone + Eq + std::hash::Hash, V: Clone> LruCache<K, V> {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            data: FxHashMap::with_capacity_and_hasher(capacity, Default::default()),
            head: None,
            tail: None,
        }
    }

    fn detach(&mut self, key: &K) {
        let (prev, next) = match self.data.get(key) {
            Some(entry) => (entry.prev.clone(), entry.next.clone()),
            None => return,
        };

        if let Some(prev_key) = prev.clone() {
            if let Some(prev_entry) = self.data.get_mut(&prev_key) {
                prev_entry.next = next.clone();
            }
        } else {
            self.head = next.clone();
        }

        if let Some(next_key) = next.clone() {
            if let Some(next_entry) = self.data.get_mut(&next_key) {
                next_entry.prev = prev.clone();
            }
        } else {
            self.tail = prev.clone();
        }

        if let Some(entry) = self.data.get_mut(key) {
            entry.prev = None;
            entry.next = None;
        }
    }

    fn push_back(&mut self, key: &K) {
        let old_tail = self.tail.clone();

        if let Some(tail_key) = old_tail.clone() {
            if let Some(tail_entry) = self.data.get_mut(&tail_key) {
                tail_entry.next = Some(key.clone());
            }
        } else {
            self.head = Some(key.clone());
        }

        if let Some(entry) = self.data.get_mut(key) {
            entry.prev = old_tail;
            entry.next = None;
        }

        self.tail = Some(key.clone());
    }

    fn touch(&mut self, key: &K) {
        if self.tail.as_ref() == Some(key) {
            return;
        }
        self.detach(key);
        self.push_back(key);
    }

    pub fn peek(&self, key: &K) -> Option<&V> {
        self.data.get(key).map(|entry| &entry.value)
    }

    pub fn touch_key(&mut self, key: &K) {
        self.touch(key);
    }

    pub fn get(&mut self, key: &K) -> Option<V> {
        let value = self.data.get(key).map(|entry| entry.value.clone())?;
        self.touch(key);
        Some(value)
    }

    pub fn insert(&mut self, key: K, value: V) {
        if self.capacity == 0 {
            return;
        }

        if let Some(entry) = self.data.get_mut(&key) {
            entry.value = value;
            self.touch(&key);
            return;
        }

        if self.data.len() >= self.capacity {
            if let Some(lru_key) = self.head.clone() {
                self.detach(&lru_key);
                self.data.remove(&lru_key);
            }
        }

        self.data.insert(
            key.clone(),
            LruEntry {
                value,
                prev: None,
                next: None,
            },
        );
        self.push_back(&key);
    }

    pub fn contains(&self, key: &K) -> bool {
        self.data.contains_key(key)
    }

    pub fn clear(&mut self) {
        self.data.clear();
        self.head = None;
        self.tail = None;
    }

    pub fn rebuild<F>(&mut self, mut mapper: F)
    where
        F: FnMut(K, V) -> Option<(K, V)>,
    {
        let mut new_entries = Vec::new();

        let mut cursor = self.head.clone();
        let mut seen = 0usize;
        while let Some(key) = cursor.clone() {
            if seen > self.data.len() {
                break;
            }
            if let Some(entry) = self.data.get(&key) {
                if let Some((new_key, new_val)) = mapper(key.clone(), entry.value.clone()) {
                    new_entries.push((new_key, new_val));
                }
                cursor = entry.next.clone();
            } else {
                break;
            }
            seen += 1;
        }

        self.data.clear();
        self.head = None;
        self.tail = None;

        for (k, v) in new_entries {
            self.insert(k, v);
        }
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Range {
    pub start: (usize, usize), // (line, column), 0-based
    pub end: (usize, usize),
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

impl From<ops::Range<(usize, usize)>> for Range {
    fn from(r: ops::Range<(usize, usize)>) -> Self {
        Range {
            start: r.start,
            end: r.end,
        }
    }
}

impl From<((usize, usize), (usize, usize))> for Range {
    fn from(r: ((usize, usize), (usize, usize))) -> Self {
        Range {
            start: r.0,
            end: r.1,
        }
    }
}

impl From<(usize, usize)> for Range {
    fn from(pos: (usize, usize)) -> Self {
        Range {
            start: pos,
            end: pos,
        }
    }
}

impl Range {
    pub fn new(start: (usize, usize), end: (usize, usize)) -> Self {
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
use std::hash::Hasher;
use std::sync::{Mutex, OnceLock};

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

fn convert_byte_to_line_col(byte_pos: usize, line_starts: &[usize], text: &str) -> (usize, usize) {
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

    (line_idx, col)
}

pub trait IntoLineCol {
    fn into_line_col(&self, text: &str) -> (usize, usize);
}

impl IntoLineCol for usize {
    fn into_line_col(&self, text: &str) -> (usize, usize) {
        let line_starts = get_or_create_line_index(text);
        convert_byte_to_line_col(*self, &line_starts, text)
    }
}

pub trait IntoByte {
    fn into_byte(&self, text: &str) -> usize;
}

impl IntoByte for (usize, usize) {
    fn into_byte(&self, text: &str) -> usize {
        let line_starts = get_or_create_line_index(text);
        let (line, col) = self;

        if *line >= line_starts.len() {
            return text.len();
        }

        let line_start = line_starts[*line];
        let line_end = line_starts.get(line + 1).copied().unwrap_or(text.len());
        let line_text = &text[line_start..line_end];

        // Convert UTF-16 column back to byte offset
        let mut byte_offset = 0;
        let mut utf16_col = 0;

        for (offset, ch) in line_text.char_indices() {
            if utf16_col >= *col {
                break;
            }
            utf16_col += ch.len_utf16();
            byte_offset = offset;
        }

        line_start + byte_offset
    }
}

use std::any::Any;

// ── ForceSync wrapper ─────────────────────────────────────────────────────────
//
// Used only by `Payload::new_any` for values that are not Send/Sync (e.g.
// `TreeAllocRef = Rc<RefCell<...>>`).  The caller guarantees that the payload
// is only accessed on the thread that created it.

struct ForceSync<T>(T);
// SAFETY: Payload::new_any callers (e.g. the IR query path) ensure the payload
// is only materialised and consumed on the single IR worker thread.
unsafe impl<T> Send for ForceSync<T> {}
unsafe impl<T> Sync for ForceSync<T> {}

// ── JSON helpers ──────────────────────────────────────────────────────────────

fn json_for<T: serde::Serialize + 'static>(any: &dyn Any) -> serde_json::Value {
    match any.downcast_ref::<T>() {
        Some(v) => serde_json::to_value(v).unwrap_or(serde_json::Value::Null),
        None => serde_json::Value::Null,
    }
}

struct Inner {
    data: Box<dyn Any + Send + Sync>,
    to_json: Option<fn(&dyn Any) -> serde_json::Value>,
    type_name: &'static str,
}

pub(crate) struct Payload(Box<Inner>);

impl Payload {
    pub fn new<T: Send + Sync + 'static>(value: T) -> Self {
        Self(Box::new(Inner {
            data: Box::new(value),
            to_json: None,
            type_name: std::any::type_name::<T>(),
        }))
    }

    pub fn new_serializable<T: serde::Serialize + Send + Sync + 'static>(value: T) -> Self {
        Self(Box::new(Inner {
            data: Box::new(value),
            to_json: Some(json_for::<T>),
            type_name: std::any::type_name::<T>(),
        }))
    }
    pub fn downcast_ref<T: 'static>(&self) -> Option<&T> {
        // Direct path (stored via `new`)
        if let Some(v) = (*self.0.data).downcast_ref::<T>() {
            return Some(v);
        }
        // ForceSync path (stored via `new_any`)
        (*self.0.data).downcast_ref::<ForceSync<T>>().map(|w| &w.0)
    }

    pub fn downcast<T: 'static>(self) -> Option<T> {
        let Inner { data, .. } = *self.0;
        // Coerce Box<dyn Any + Send + Sync> → Box<dyn Any> to access downcast().
        let data: Box<dyn Any> = data;
        match data.downcast::<T>() {
            Ok(boxed) => Some(*boxed),
            Err(data) => data.downcast::<ForceSync<T>>().ok().map(|b| b.0),
        }
    }

    pub fn to_json(&self) -> serde_json::Value {
        self.0
            .to_json
            .map(|to_json| to_json(self.0.data.as_ref()))
            .unwrap_or(serde_json::Value::Null)
    }
}

impl serde::Serialize for Payload {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.to_json().serialize(serializer)
    }
}

impl std::fmt::Debug for Payload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.0.to_json.is_some() {
            write!(f, "Payload({:?})", self.to_json())
        } else {
            write!(f, "Payload(<opaque:{}>)", self.0.type_name)
        }
    }
}

// Kept for compatibility; not currently used for dynamic dispatch.
pub trait SerdeAny: serde::Serialize + Send + Sync {}
