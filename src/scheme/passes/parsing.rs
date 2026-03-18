use rustc_hash::FxHashMap;

use crate::{
    grammar::Grammar,
    parsec::{Parser, ParserConfig},
    scheme::{
        self,
        layers::{ParseNodeValue, ParseTreeIR, ParseTreeValue, SourceText},
    },
    utils::Span,
};

use super::{
    delta,
    reparser::{Reparser, ReparserConfig},
};

pub struct ParserPass {
    parser: Parser,
    reparser: Reparser,
}

// SAFETY: ParserPass is moved into a single worker thread in the runtime
// pipeline and not shared concurrently across threads.
unsafe impl Send for ParserPass {}

impl ParserPass {
    /// Create a pass for `grammar` with default parser/reparser settings.
    pub fn new(grammar: &'static Grammar) -> Self {
        let mut parser = Parser::new(grammar);
        let crate::parsec::Result { root, .. } = parser.parse_text("");
        let alloc = parser.alloc.clone();
        let reparser = Reparser::new(root, alloc);
        Self { parser, reparser }
    }

    /// Create with custom parser and reparser configuration.
    pub fn with_config(
        grammar: &'static Grammar,
        parser_config: ParserConfig,
        reparser_config: ReparserConfig,
    ) -> Self {
        let mut parser = Parser::new(grammar);
        parser.set_config(parser_config);
        let crate::parsec::Result { root, .. } = parser.parse_text("");
        let alloc = parser.alloc.clone();
        let reparser = Reparser::new(root, alloc).with_config(reparser_config);
        Self { parser, reparser }
    }
}

impl scheme::Pass<SourceText, ParseTreeIR> for ParserPass {
    type Error = std::convert::Infallible;

    fn transform(
        &mut self,
        upstream: &SourceText,
        downstream: &ParseTreeIR,
        txn: scheme::Transaction<SourceText>,
    ) -> Result<scheme::Transaction<ParseTreeIR>, Self::Error> {
        let new_text_owned = upstream.text();
        let new_text = new_text_owned.as_str();

        let edit = extract_edit(&txn);

        // Only attempt incremental re-parse when a CST already exists.
        // `downstream.root.is_some()` is the ground truth: if no CST root has
        // been set yet the reparser has nothing to reuse, and we must full-parse.
        if downstream.root.is_some() {
            if let Some((span, new_len)) = edit {
                let result =
                    self.reparser
                        .handle_edit(&mut self.parser, span, new_len, new_text, None);
                if let Ok(edit_result) = result {
                    let cmds = prepend_messages_command(
                        &self.parser.messages,
                        edit_result.semantic_commands,
                    );
                    return Ok(std::sync::Arc::new(cmds));
                }
                // Incremental re-parse failed; fall through to full re-parse.
            }
        }

        // Full re-parse.
        let crate::parsec::Result { root, .. } = self.parser.parse_text(new_text);
        self.reparser.current = std::rc::Rc::new(root.clone());
        let tree_cmds =
            delta::generate_commands_for_full_tree(&self.parser.alloc, root.green, new_text);
        let cmds = prepend_messages_command(&self.parser.messages, tree_cmds);

        Ok(std::sync::Arc::new(cmds))
    }
}

fn extract_edit(txn: &[scheme::Command<SourceText>]) -> Option<(Span, usize)> {
    // Collect staged string lengths for Create commands.
    let mut staged_len: FxHashMap<usize, usize> = FxHashMap::default();
    let mut edit_count = 0usize;
    let mut result = None;

    for cmd in txn {
        match cmd {
            scheme::Command::Create { id, value } => {
                staged_len.insert(*id, value.len());
            }
            scheme::Command::Delete { index: span } => {
                edit_count += 1;
                result = Some((*span, 0));
            }
            scheme::Command::Insert { index: span, id } => {
                let new_len = staged_len.get(id).copied().unwrap_or(0);
                edit_count += 1;
                result = Some((*span, new_len));
            }
            scheme::Command::Replace { index: span, id } => {
                let new_len = staged_len.get(id).copied().unwrap_or(0);
                edit_count += 1;
                result = Some((*span, new_len));
            }
        }
    }

    // Only use incremental reparse for exactly one edit; multi-edit batches
    // fall through to full reparse to avoid incorrect partial-edit hints.
    if edit_count == 1 { result } else { None }
}

/// Prepend a `ParseNodeValue::Messages` Create command (id=0) carrying the
/// parser-level message list so the CST layer can include errors that are not
/// encoded as error nodes in the green tree (e.g. panic-mode skip errors
/// dropped by `finalize_root`).  Tree-node IDs start at 1, so id=0 is safe.
fn prepend_messages_command(
    messages: &crate::parsec::msg::ParserMessages,
    rest: Vec<scheme::Command<ParseTreeIR>>,
) -> Vec<scheme::Command<ParseTreeIR>> {
    if messages.is_empty() {
        return rest;
    }
    let mut cmds = Vec::with_capacity(rest.len() + 1);
    cmds.push(scheme::Command::Create {
        id: 0,
        value: ParseTreeValue::Node(ParseNodeValue::Messages {
            messages: messages.clone(),
        }),
    });
    cmds.extend(rest);
    cmds
}
