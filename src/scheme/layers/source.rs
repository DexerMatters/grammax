use crate::scheme::{Command, IR, Transaction};
use crate::utils::Span;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceTextError {
    /// The staged id was never created in this transaction.
    UnknownStagingId(usize),
    /// The span references a byte range that falls outside the current text.
    InvalidSpan { span: Span, text_len: usize },
    /// An Insert target must have `start == end`.
    NotAnInsertionPoint { span: Span },
}

#[derive(Debug, Clone, Default)]
pub struct SourceText {
    pub text: String,
    staging: Vec<Option<String>>,
}

impl SourceText {
    pub fn new() -> Self {
        Self::default()
    }

    /// Construct from an existing string (e.g. initial file contents).
    pub fn from_string(text: String) -> Self {
        Self {
            text,
            staging: Vec::new(),
        }
    }

    // ── helpers ──────────────────────────────────────────────────────────────

    fn ensure_staged(&self, id: usize) -> Result<&str, SourceTextError> {
        self.staging
            .get(id)
            .and_then(|s| s.as_deref())
            .ok_or(SourceTextError::UnknownStagingId(id))
    }

    fn validate_span(&self, span: Span) -> Result<(), SourceTextError> {
        if span.start <= span.end && span.end <= self.text.len() {
            Ok(())
        } else {
            Err(SourceTextError::InvalidSpan {
                span,
                text_len: self.text.len(),
            })
        }
    }

    fn clamp_span(&self, span: Span) -> Span {
        let len = self.text.len();
        let start = span.start.min(len);
        let end = span.end.min(len);
        Span::new(start.min(end), end)
    }
}

// ── IR impl ──────────────────────────────────────────────────────────────────

impl IR for SourceText {
    type Ix = Span;
    /// A text fragment (either stored or staged).
    type Value = String;
    type Error = SourceTextError;

    /// Query a substring (the `Value` at `index`).
    fn query(&self, index: Span) -> Result<String, Self::Error> {
        let span = self.clamp_span(index);
        self.validate_span(span)?;
        Ok(self.text[span.start..span.end].to_owned())
    }

    /// Clears staging table then applies the transaction directly.
    fn apply_transaction(&mut self, transaction: Transaction<Self>) -> Result<(), Self::Error> {
        self.staging.clear();
        for command in transaction.iter() {
            match command {
                Command::Create { id, value } => {
                    if *id >= self.staging.len() {
                        self.staging.resize(*id + 1, None);
                    }
                    self.staging[*id] = Some(value.clone());
                }
                Command::Insert { index, id } => {
                    if index.start != index.end {
                        return Err(SourceTextError::NotAnInsertionPoint { span: *index });
                    }
                    let at = index.start.min(self.text.len());
                    let fragment = self.ensure_staged(*id)?.to_owned();
                    self.text.insert_str(at, &fragment);
                }
                Command::Delete { index } => {
                    let span = self.clamp_span(*index);
                    self.validate_span(span)?;
                    self.text.drain(span.start..span.end);
                }
                Command::Replace { index, id } => {
                    let span = self.clamp_span(*index);
                    self.validate_span(span)?;
                    let fragment = self.ensure_staged(*id)?.to_owned();
                    self.text.replace_range(span.start..span.end, &fragment);
                }
                Command::SetRoot { .. } => {} // text has no root concept
            }
        }
        Ok(())
    }
}
