use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};
use std::hash::Hash;
use std::ops;

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

    pub fn start_line_col(&self, text: &str) -> (usize, usize) {
        let index = LineIndex::new(text);
        let line_col = index.byte_to_line_col_with_text(self.start, text);
        (line_col.line, line_col.col)
    }

    pub fn end_line_col(&self, text: &str) -> (usize, usize) {
        let index = LineIndex::new(text);
        let line_col = index.byte_to_line_col_with_text(self.end, text);
        (line_col.line, line_col.col)
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

#[derive(Debug, Clone)]
pub(crate) struct LineIndex {
    line_starts: Vec<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LineCol {
    pub line: usize,
    pub col: usize,
}

impl LineIndex {
    pub fn new(text: &str) -> Self {
        let mut line_starts = vec![0];
        let mut line_utf16_offsets = vec![0];
        let mut utf16_offset = 0;

        let mut chars = text.char_indices().peekable();
        while let Some((_byte_pos, ch)) = chars.next() {
            let ch_utf16_len = ch.len_utf16();
            utf16_offset += ch_utf16_len;

            if ch == '\n' {
                // Next line starts after this \n
                if let Some(&(next_byte_pos, _)) = chars.peek() {
                    line_starts.push(next_byte_pos);
                    line_utf16_offsets.push(utf16_offset);
                }
            }
        }

        LineIndex { line_starts }
    }

    pub fn byte_to_line_col_with_text(&self, byte_pos: usize, text: &str) -> LineCol {
        // Binary search for the line containing this byte offset
        let line_idx = self
            .line_starts
            .binary_search(&byte_pos)
            .unwrap_or_else(|next_idx| next_idx.saturating_sub(1));

        let line = line_idx + 1; // Convert to 1-indexed
        let line_start_byte = self.line_starts[line_idx];

        // Find the end of this line
        let line_end_byte = self
            .line_starts
            .get(line_idx + 1)
            .copied()
            .unwrap_or(text.len());

        // Get the line text
        let line_text = &text[line_start_byte..line_end_byte];

        // Compute UTF-16 column: count UTF-16 code units from line start to the position within the line
        let offset_in_line = byte_pos.saturating_sub(line_start_byte);
        let mut col = 0;

        for (byte_offset, ch) in line_text.char_indices() {
            if byte_offset >= offset_in_line {
                break;
            }
            col += ch.len_utf16();
        }

        LineCol { line, col }
    }
}
