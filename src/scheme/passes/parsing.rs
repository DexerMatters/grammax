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
        let mut parser = Parser::new(grammar).with_config(parser_config);
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
        txn: scheme::Transaction<SourceText>,
    ) -> Result<scheme::Transaction<ParseTreeIR>, Self::Error> {
        let new_text = &upstream.text;

        let edit = extract_edit(&txn);

        if let Some((span, new_len)) = edit {
            // A span that starts at 0 and covers the entire current text is a
            // "replace-all" (e.g. the CLI submitting a full buffer, or the very
            // first character typed into an empty document).  No incremental
            // candidate can help here — always use the full-reparse path so the
            // CST layer receives a correct root-setting Insert command.
            let old_len = self.parser.text().len();
            let is_replace_all = span.start == 0 && span.end >= old_len;

            if !is_replace_all {
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
