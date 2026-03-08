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
}

// ── IR impl ──────────────────────────────────────────────────────────────────

impl IR for SourceText {
    type Ix = Span;
    /// A text fragment (either stored or staged).
    type Value = String;
    type Error = SourceTextError;

    /// Query a substring (the `Value` at `index`).
    fn query(&self, index: Span) -> Result<String, Self::Error> {
        self.validate_span(index)?;
        Ok(self.text[index.start..index.end].to_owned())
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
                    if index.start > self.text.len() {
                        return Err(SourceTextError::InvalidSpan {
                            span: *index,
                            text_len: self.text.len(),
                        });
                    }
                    let fragment = self.ensure_staged(*id)?.to_owned();
                    self.text.insert_str(index.start, &fragment);
                }
                Command::Delete { index } => {
                    self.validate_span(*index)?;
                    self.text.drain(index.start..index.end);
                }
                Command::Replace { index, id } => {
                    self.validate_span(*index)?;
                    let fragment = self.ensure_staged(*id)?.to_owned();
                    self.text.replace_range(index.start..index.end, &fragment);
                }
                Command::SetRoot { .. } => {} // text has no root concept
            }
        }
        Ok(())
    }
}
