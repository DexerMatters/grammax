use rustc_hash::FxHashMap;

use crate::scheme::{Command, DocumentSpan, IR, Span, Transaction, URI};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceTextError {
    /// The staged id was never created in this transaction.
    UnknownStagingId(usize),
    /// The span references a byte range that falls outside the current text.
    InvalidSpan {
        span: Span,
        text_len: usize,
    },
    InvalidURI {
        uri: URI,
    },
    /// An Insert target must have `start == end`.
    NotAnInsertionPoint {
        span: Span,
    },
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
pub(crate) struct GapBuf {
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
    pub(crate) sources: FxHashMap<URI, GapBuf>,
    staging: FxHashMap<usize, String>,
}

impl SourceText {
    pub fn new() -> Self {
        Self::default()
    }

    /// Construct from an existing string (e.g. initial file contents).
    pub fn from_string(text: String) -> Self {
        let mut hashmap = FxHashMap::default();
        let sources = GapBuf::from_string(text);
        hashmap.insert(URI::default(), sources);
        Self {
            sources: hashmap,
            staging: FxHashMap::default(),
        }
    }

    /// Materialise the current text as a `String`. O(n) in text length;
    /// called at most once per transaction by the downstream `ParserPass`.
    pub fn text(&self, url: &URI) -> String {
        self.sources
            .get(url)
            .map(|gap| gap.as_string())
            .unwrap_or_default()
    }

    // ── helpers ────────────────────────────────────────────────────────────

    fn ensure_staged(&self, id: usize) -> Result<&str, SourceTextError> {
        self.staging
            .get(&id)
            .map(|s| s.as_str())
            .ok_or(SourceTextError::UnknownStagingId(id))
    }

    fn gap_by_uri(&self, uri: &URI) -> Result<&GapBuf, SourceTextError> {
        self.sources
            .get(uri)
            .ok_or(SourceTextError::InvalidURI { uri: *uri })
    }

    fn gap_mut_by_uri_or_init(&mut self, uri: &URI) -> &mut GapBuf {
        self.sources
            .entry(*uri)
            .or_insert_with(|| GapBuf::default())
    }
}

// ── Helper functions for span validation and clamping ──────────────────────

fn clamp_span(span: &Span, len: usize) -> Span {
    let start = span.start.min(span.end).min(len);
    let end = span.end.max(span.start).min(len);
    Span { start, end }
}

fn validate_span(span: &Span, len: usize) -> Result<(), SourceTextError> {
    if span.start <= span.end && span.end <= len {
        Ok(())
    } else {
        Err(SourceTextError::InvalidSpan {
            span: span.clone(),
            text_len: len,
        })
    }
}

// ── IR impl ──────────────────────────────────────────────────────────────────

impl IR for SourceText {
    type Ix = DocumentSpan;
    /// A text fragment (either stored or staged).
    type Value = String;
    type Error = SourceTextError;

    /// Query a substring (the `Value` at `index`).
    fn query(&self, index: DocumentSpan) -> Result<String, Self::Error> {
        let gap = self.gap_by_uri(&index.uri)?;
        let span = clamp_span(&index.span, gap.len());
        validate_span(&span, gap.len())?;
        Ok(gap.slice(span.start, span.end))
    }

    /// Clears staging table then applies the transaction directly.
    fn apply_transaction(&mut self, transaction: Transaction<Self>) -> Result<(), Self::Error> {
        self.staging.clear();
        for command in transaction.iter() {
            match command {
                Command::Create { id, value } => {
                    self.staging.insert(*id, value.clone());
                }
                Command::Insert { index, id } => {
                    if index.span.start != index.span.end {
                        return Err(SourceTextError::NotAnInsertionPoint { span: index.span });
                    }
                    let fragment = self.ensure_staged(*id)?.to_owned();
                    let gap = self.gap_mut_by_uri_or_init(&index.uri);
                    let at = index.span.start.min(gap.len());
                    gap.insert_str(at, &fragment);
                }
                Command::Delete { index } => {
                    let gap = self.gap_mut_by_uri_or_init(&index.uri);
                    let len = gap.len();
                    let span = clamp_span(&index.span, len);
                    validate_span(&span, len)?;
                    gap.drain(span.start, span.end);
                }
                Command::Replace { index, id } => {
                    let fragment = self.ensure_staged(*id)?.to_owned();
                    let gap = self.gap_mut_by_uri_or_init(&index.uri);
                    let len = gap.len();
                    let span = clamp_span(&index.span, len);
                    validate_span(&span, len)?;
                    gap.replace_range(span.start, span.end, &fragment);
                }
            }
        }
        Ok(())
    }
}
