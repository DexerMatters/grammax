use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};
use std::fmt;
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

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct Position {
    pub line: usize,
    pub character: usize,
}

impl Position {
    pub const fn new(line: usize, character: usize) -> Self {
        Self { line, character }
    }

    pub const fn zero() -> Self {
        Self::new(0, 0)
    }
}

impl From<(usize, usize)> for Position {
    fn from((line, character): (usize, usize)) -> Self {
        Self::new(line, character)
    }
}

impl From<(&usize, &usize)> for Position {
    fn from((line, character): (&usize, &usize)) -> Self {
        Self::new(*line, *character)
    }
}

impl fmt::Display for Position {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.line, self.character)
    }
}

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct Range {
    pub start: Position,
    pub end: Position,
}

impl Range {
    pub const fn new(start: Position, end: Position) -> Self {
        Self { start, end }
    }

    pub const fn point(position: Position) -> Self {
        Self::new(position, position)
    }

    pub const fn empty() -> Self {
        Self::point(Position::zero())
    }

    pub const fn is_empty(&self) -> bool {
        self.start.line == self.end.line && self.start.character == self.end.character
    }

    pub fn line_char(&self) -> ((usize, usize), (usize, usize)) {
        (
            (self.start.line, self.start.character),
            (self.end.line, self.end.character),
        )
    }

    pub fn from_byte_range(text: &str, start: usize, end: usize) -> Self {
        TextIndex::new(text).range_from_byte_range(text, start, end)
    }

    pub fn to_byte_range(&self, text: &str) -> Option<ops::Range<usize>> {
        TextIndex::new(text).range_to_byte_range_with_text(*self, text)
    }
}

impl<P: Into<Position>> From<(P, P)> for Range {
    fn from((start, end): (P, P)) -> Self {
        Self::new(start.into(), end.into())
    }
}

impl<P: Into<Position>> From<(P,)> for Range {
    fn from((position,): (P,)) -> Self {
        Self::point(position.into())
    }
}

impl ops::Add for Range {
    type Output = Range;

    fn add(self, other: Range) -> Range {
        Range {
            start: self.start,
            end: other.end,
        }
    }
}

impl fmt::Display for Range {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}..{}", self.start, self.end)
    }
}

pub fn advance_position_with_text(mut position: Position, text: &str) -> Position {
    for ch in text.chars() {
        if ch == '\n' {
            position.line += 1;
            position.character = 0;
        } else {
            position.character += ch.len_utf16();
        }
    }
    position
}

pub fn advance_position_by_bytes(
    text: &str,
    byte_offset: usize,
    position: Position,
    byte_len: usize,
) -> Position {
    let start = byte_offset.min(text.len());
    let end = start.saturating_add(byte_len).min(text.len());
    match text.get(start..end) {
        Some(fragment) => advance_position_with_text(position, fragment),
        None => position,
    }
}

#[derive(Debug, Clone, Default)]
pub struct TextIndex {
    line_starts: Vec<usize>,
}

impl TextIndex {
    pub fn new(text: &str) -> Self {
        let mut line_starts = vec![0];

        let mut chars = text.char_indices().peekable();
        while let Some((_byte_pos, ch)) = chars.next() {
            if ch == '\n' {
                if let Some(&(next_byte_pos, _)) = chars.peek() {
                    line_starts.push(next_byte_pos);
                }
            }
        }

        TextIndex { line_starts }
    }

    pub fn range_from_byte_range(&self, text: &str, start: usize, end: usize) -> Range {
        Range {
            start: self.byte_to_position_with_text(start, text),
            end: self.byte_to_position_with_text(end, text),
        }
    }

    pub fn byte_to_position_with_text(&self, byte_pos: usize, text: &str) -> Position {
        let clamped_byte = byte_pos.min(text.len());
        let line_idx = self
            .line_starts
            .binary_search(&clamped_byte)
            .unwrap_or_else(|next_idx| next_idx.saturating_sub(1));

        let line_start_byte = self.line_starts[line_idx];
        let line_end_byte = self
            .line_starts
            .get(line_idx + 1)
            .copied()
            .unwrap_or(text.len());
        let line_text = &text[line_start_byte..line_end_byte];
        let offset_in_line = clamped_byte.saturating_sub(line_start_byte);
        let mut character = 0;

        for (byte_offset, ch) in line_text.char_indices() {
            if byte_offset >= offset_in_line {
                break;
            }
            character += ch.len_utf16();
        }

        Position {
            line: line_idx,
            character,
        }
    }

    pub fn position_to_byte_with_text(&self, position: Position, text: &str) -> Option<usize> {
        let line_start = *self.line_starts.get(position.line)?;
        let line_end = self
            .line_starts
            .get(position.line + 1)
            .copied()
            .unwrap_or(text.len());
        let line_text = &text[line_start..line_end];

        if position.character == 0 {
            return Some(line_start);
        }

        let mut utf16_offset = 0;
        for (byte_offset, ch) in line_text.char_indices() {
            if utf16_offset == position.character {
                return Some(line_start + byte_offset);
            }
            utf16_offset += ch.len_utf16();
            if utf16_offset == position.character {
                return Some(line_start + byte_offset + ch.len_utf8());
            }
            if utf16_offset > position.character {
                return None;
            }
        }

        if utf16_offset == position.character {
            Some(line_end)
        } else {
            None
        }
    }

    pub fn clamp_position_with_text(&self, position: Position, text: &str) -> Position {
        let Some(&line_start) = self
            .line_starts
            .get(position.line.min(self.line_starts.len().saturating_sub(1)))
        else {
            return Position::zero();
        };
        let clamped_line = position.line.min(self.line_starts.len().saturating_sub(1));
        let line_end = self
            .line_starts
            .get(clamped_line + 1)
            .copied()
            .unwrap_or(text.len());
        let line_text = &text[line_start..line_end];

        let mut utf16_offset = 0;
        for ch in line_text.chars() {
            let next_offset = utf16_offset + ch.len_utf16();
            if next_offset > position.character {
                break;
            }
            utf16_offset = next_offset;
        }

        Position::new(clamped_line, utf16_offset)
    }

    pub fn clamp_range_with_text(&self, range: Range, text: &str) -> Range {
        let start = self.clamp_position_with_text(range.start, text);
        let end = self.clamp_position_with_text(range.end, text);
        if start <= end {
            Range::new(start, end)
        } else {
            Range::new(end, end)
        }
    }

    pub fn range_to_byte_range_with_text(
        &self,
        range: Range,
        text: &str,
    ) -> Option<ops::Range<usize>> {
        let start = self.position_to_byte_with_text(range.start, text)?;
        let end = self.position_to_byte_with_text(range.end, text)?;
        if start <= end { Some(start..end) } else { None }
    }
}

pub(crate) type LineIndex = TextIndex;

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
