//! Pass 1→2: the incremental parser pass.
//!
//! [`ParserPass`] implements [`scheme::Pass<SourceText, RedGreenTreeIR>`].
//! Given a [`SourceText`] transaction (the text edit), it runs the
//! incremental parser/reparser and produces parser commands as a
//! [`Transaction<RedGreenTreeIR>`]
//! describing the parse-tree delta.
//!
//! The pass internally holds a [`Parser`] and a [`Reparser`] so that
//! re-parses are incremental: only the affected parse region is re-computed.

use crate::{
    grammar::Grammar,
    parsec::{Parser, ParserConfig},
    scheme::{
        self,
        layers::{RedGreenTreeIR, SourceText},
    },
    utils::Span,
};

use super::{
    delta,
    reparser::{Reparser, ReparserConfig},
};

// ── ParserPass ─────────────────────────────────────────────────────────────────

/// Pass from Layer 1 (SourceText) → Layer 2 (RedGreenTreeIR).
///
/// Wraps an incremental [`Reparser`] that keeps the current parse tree and
/// re-parses only the changed region on each transaction.
///
/// # How it wires into the Pipeline
///
/// ```text
/// ──Transaction<SourceText>──▶  [ParserPass thread]  ──Transaction<RedGreenTreeIR>──▶  [IncrementalLowerer thread]
/// ```
pub struct ParserPass {
    parser: Parser,
    reparser: Reparser,
    /// Full current text, kept in sync with the upstream SourceText IR.
    text: String,
}

impl ParserPass {
    /// Create a pass for `grammar` with default parser/reparser settings.
    pub fn new(grammar: &'static Grammar) -> Self {
        let mut parser = Parser::new(grammar);
        let crate::parsec::Result { root, .. } = parser.parse_text("");
        let alloc = parser.alloc.clone();
        let reparser = Reparser::new(root, alloc);
        Self {
            parser,
            reparser,
            text: String::new(),
        }
    }

    /// Create with custom parser and reparser configuration.
    pub fn with_config(
        grammar: &'static Grammar,
        parser_config: ParserConfig,
        reparser_config: ReparserConfig,
    ) -> Self {
        let mut parser = Parser::new(grammar).with_config(parser_config);
        let crate::parsec::Result { root, .. } = parser.parse_text("");
        let alloc = parser.alloc.clone();
        let reparser = Reparser::new(root, alloc).with_config(reparser_config);
        Self {
            parser,
            reparser,
            text: String::new(),
        }
    }
}

// ── scheme::Pass impl ─────────────────────────────────────────────────────────

impl scheme::Pass<SourceText, RedGreenTreeIR> for ParserPass {
    type Error = std::convert::Infallible;

    /// Transform a source-text transaction into a parse-tree transaction.
    ///
    /// After `upstream` has already applied the transaction, `upstream.text`
    /// is the *new* full source text. The edit coordinates (span + new_len)
    /// are reconstructed from the command sequence so the reparser can do an
    /// incremental re-parse.
    fn transform(
        &mut self,
        upstream: &SourceText,
        txn: scheme::Transaction<SourceText>,
    ) -> Result<scheme::Transaction<RedGreenTreeIR>, Self::Error> {
        let new_text = &upstream.text;

        // Extract the edit (span, new_len) from the transaction.
        let edit = extract_edit(&txn);

        if let Some((span, new_len)) = edit {
            // Attempt an incremental re-parse.
            let result = self
                .reparser
                .handle_edit(&mut self.parser, span, new_len, new_text, None);
            match result {
                Ok(edit_result) => {
                    self.text = new_text.clone();
                    return Ok(edit_result.semantic_commands);
                }
                Err(_) => {
                    // Incremental re-parse failed; fall through to full re-parse.
                }
            }
        }

        // Full re-parse (first edit, or incremental failure).
        let crate::parsec::Result { root, .. } = self.parser.parse_text(new_text);
        self.reparser.current = std::rc::Rc::new(root.clone());
        let commands = delta::generate_commands_for_full_tree(
            &self.parser.alloc,
            root.green,
            new_text,
        );
        self.text = new_text.clone();
        Ok(commands)
    }
}

// ── helpers ───────────────────────────────────────────────────────────────────

/// Reconstruct the (changed_span, new_len) pair from a SourceText transaction.
///
/// Returns `None` if the transaction has no structural edit (e.g. SetRoot only).
fn extract_edit(txn: &[scheme::Command<SourceText>]) -> Option<(Span, usize)> {
    // Collect staged string lengths indexed by Create id.
    let mut staged_len: Vec<usize> = Vec::new();

    for cmd in txn {
        match cmd {
            scheme::Command::Create { id, value } => {
                if *id >= staged_len.len() {
                    staged_len.resize(*id + 1, 0);
                }
                staged_len[*id] = value.len();
            }
            scheme::Command::Delete { index: span } => {
                return Some((*span, 0));
            }
            scheme::Command::Insert { index: span, id } => {
                let new_len = staged_len.get(*id).copied().unwrap_or(0);
                return Some((*span, new_len));
            }
            scheme::Command::Replace { index: span, id } => {
                let new_len = staged_len.get(*id).copied().unwrap_or(0);
                return Some((*span, new_len));
            }
            scheme::Command::SetRoot { .. } => {}
        }
    }
    None
}
