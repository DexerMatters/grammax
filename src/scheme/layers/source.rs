use rustc_hash::FxHashMap;

use crate::scheme::{Command, IR, Transaction};
use crate::utils::{Range, TextIndex};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceTextError {
    /// The staged id was never created in this transaction.
    UnknownStagingId(usize),
    /// The range references a position outside the current text.
    InvalidRange { range: Range, text_len: usize },
    /// An Insert target must have `start == end`.
    NotAnInsertionPoint { range: Range },
}

// ── Gap buffer ─────────────────────────────────────────────────────────────────
//
// Layout: `buf[..gap_start]` = text-before-gap,
//         `buf[gap_start..gap_end]` = unused scratch space (the "gap"),
//         `buf[gap_end..]` = text-after-gap.
//
// Inserting/deleting near the last edit position is O(|delta|); moving the gap
// costs O(distance) but is amortised O(1) for sequential (LSP-style) edits.

#[derive(Debug, Clone)]
struct GapBuf {
    buf: Vec<u8>,
    gap_start: usize,
    gap_end: usize,
}

impl Default for GapBuf {
    fn default() -> Self {
        const INIT: usize = 256;
        Self {
            buf: vec![0u8; INIT],
            gap_start: 0,
            gap_end: INIT,
        }
    }
}

impl GapBuf {
    fn from_string(s: String) -> Self {
        const INIT: usize = 256;
        let gap = INIT.max(s.len() / 4 + INIT);
        let bytes = s.into_bytes();
        let text_len = bytes.len();
        let mut buf = vec![0u8; text_len + gap];
        // Gap at the front; text follows.
        buf[gap..].copy_from_slice(&bytes);
        Self {
            buf,
            gap_start: 0,
            gap_end: gap,
        }
    }

    /// Logical byte length of the text.
    fn len(&self) -> usize {
        self.buf.len() - (self.gap_end - self.gap_start)
    }

    /// Move the gap to logical byte position `pos`.
    fn move_gap_to(&mut self, pos: usize) {
        if pos == self.gap_start {
            return;
        }
        let gap_size = self.gap_end - self.gap_start;
        if pos < self.gap_start {
            // Shift buf[pos..gap_start] right by gap_size.
            self.buf.copy_within(pos..self.gap_start, pos + gap_size);
            self.gap_start = pos;
            self.gap_end = pos + gap_size;
        } else {
            // pos > self.gap_start:
            // Shift buf[gap_end..gap_end + (pos - gap_start)] left to buf[gap_start..].
            let move_len = pos - self.gap_start;
            self.buf
                .copy_within(self.gap_end..self.gap_end + move_len, self.gap_start);
            self.gap_start = pos;
            self.gap_end += move_len;
        }
    }

    /// Ensure the gap is at least `needed` bytes wide.
    fn ensure_gap(&mut self, needed: usize) {
        let current = self.gap_end - self.gap_start;
        if current >= needed {
            return;
        }
        let extra = (needed - current).max(256);
        let old_len = self.buf.len();
        self.buf.resize(old_len + extra, 0);
        if self.gap_end < old_len {
            self.buf
                .copy_within(self.gap_end..old_len, self.gap_end + extra);
        }
        self.gap_end += extra;
    }

    fn insert_str(&mut self, at: usize, s: &str) {
        let bytes = s.as_bytes();
        self.move_gap_to(at);
        self.ensure_gap(bytes.len());
        self.buf[self.gap_start..self.gap_start + bytes.len()].copy_from_slice(bytes);
        self.gap_start += bytes.len();
    }

    fn drain(&mut self, start: usize, end: usize) {
        if start >= end {
            return;
        }
        self.move_gap_to(start);
        // Extend the gap over the deleted region.
        self.gap_end += end - start;
    }

    fn replace_range(&mut self, start: usize, end: usize, s: &str) {
        self.drain(start, end);
        if !s.is_empty() {
            self.insert_str(start, s);
        }
    }

    /// Materialise the full text into a `String`. O(n) in text length.
    fn as_string(&self) -> String {
        let mut v = Vec::with_capacity(self.len());
        v.extend_from_slice(&self.buf[..self.gap_start]);
        v.extend_from_slice(&self.buf[self.gap_end..]);
        // SAFETY: Only valid UTF-8 is ever inserted (callers go through &str).
        unsafe { String::from_utf8_unchecked(v) }
    }

    /// Return a substring `[start, end)` as a `String`.
    fn slice(&self, start: usize, end: usize) -> String {
        let gap_size = self.gap_end - self.gap_start;
        let mut v = Vec::with_capacity(end - start);
        if end <= self.gap_start {
            v.extend_from_slice(&self.buf[start..end]);
        } else if start >= self.gap_start {
            v.extend_from_slice(&self.buf[start + gap_size..end + gap_size]);
        } else {
            v.extend_from_slice(&self.buf[start..self.gap_start]);
            let after_len = end - self.gap_start;
            v.extend_from_slice(&self.buf[self.gap_end..self.gap_end + after_len]);
        }
        unsafe { String::from_utf8_unchecked(v) }
    }
}

// ── SourceText ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct SourceText {
    gap: GapBuf,
    staging: FxHashMap<usize, String>,
}

impl SourceText {
    pub fn new() -> Self {
        Self::default()
    }

    /// Construct from an existing string (e.g. initial file contents).
    pub fn from_string(text: String) -> Self {
        Self {
            gap: GapBuf::from_string(text),
            staging: FxHashMap::default(),
        }
    }

    /// Materialise the current text as a `String`. O(n) in text length;
    /// called at most once per transaction by the downstream `ParserPass`.
    pub fn text(&self) -> String {
        self.gap.as_string()
    }

    // ── helpers ────────────────────────────────────────────────────────────

    fn ensure_staged(&self, id: usize) -> Result<&str, SourceTextError> {
        self.staging
            .get(&id)
            .map(|s| s.as_str())
            .ok_or(SourceTextError::UnknownStagingId(id))
    }

    fn byte_range_in(
        &self,
        range: Range,
        text: &str,
        index: &TextIndex,
    ) -> Result<std::ops::Range<usize>, SourceTextError> {
        index
            .range_to_byte_range_with_text(range, text)
            .ok_or(SourceTextError::InvalidRange {
                range,
                text_len: text.len(),
            })
    }

    fn clamp_range_in(&self, range: Range, text: &str, index: &TextIndex) -> Range {
        index.clamp_range_with_text(range, text)
    }
}

// ── IR impl ──────────────────────────────────────────────────────────────────

impl IR for SourceText {
    type Ix = Range;
    /// A text fragment (either stored or staged).
    type Value = String;
    type Error = SourceTextError;

    /// Query a substring (the `Value` at `index`).
    fn query(&self, index: Range) -> Result<String, Self::Error> {
        let text = self.text();
        let text_index = TextIndex::new(&text);
        let range = self.clamp_range_in(index, &text, &text_index);
        let byte_range = self.byte_range_in(range, &text, &text_index)?;
        Ok(self.gap.slice(byte_range.start, byte_range.end))
    }

    /// Clears staging table then applies the transaction directly.
    fn apply_transaction(&mut self, transaction: Transaction<Self>) -> Result<(), Self::Error> {
        self.staging.clear();
        let mut text = self.text();
        let mut text_index = TextIndex::new(&text);
        for command in transaction.iter() {
            match command {
                Command::Create { id, value } => {
                    self.staging.insert(*id, value.clone());
                }
                Command::Insert { index, id } => {
                    if !index.is_empty() {
                        return Err(SourceTextError::NotAnInsertionPoint { range: *index });
                    }
                    let range = self.clamp_range_in(*index, &text, &text_index);
                    let byte_range = self.byte_range_in(range, &text, &text_index)?;
                    let at = byte_range.start;
                    let fragment = self.ensure_staged(*id)?.to_owned();
                    self.gap.insert_str(at, &fragment);
                    text.insert_str(at, &fragment);
                    text_index = TextIndex::new(&text);
                }
                Command::Delete { index } => {
                    let range = self.clamp_range_in(*index, &text, &text_index);
                    let byte_range = self.byte_range_in(range, &text, &text_index)?;
                    self.gap.drain(byte_range.start, byte_range.end);
                    text.replace_range(byte_range.clone(), "");
                    text_index = TextIndex::new(&text);
                }
                Command::Replace { index, id } => {
                    let range = self.clamp_range_in(*index, &text, &text_index);
                    let byte_range = self.byte_range_in(range, &text, &text_index)?;
                    let fragment = self.ensure_staged(*id)?.to_owned();
                    self.gap
                        .replace_range(byte_range.start, byte_range.end, &fragment);
                    text.replace_range(byte_range, &fragment);
                    text_index = TextIndex::new(&text);
                }
            }
        }
        Ok(())
    }
}
