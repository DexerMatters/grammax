use rustc_hash::FxHashMap;

use crate::{
    grammar::Grammar,
    parsec::{Parser, ParserConfig},
    scheme::{
        self, DocumentSpan, LayerObserver, ObserveError, Span, URI,
        layers::{ParseNodeValue, ParseTreeIR, ParseTreeQuery, SourceText, source::SourceFault},
    },
};

use super::{
    delta,
    reparser::{Reparser, ReparserConfig},
};

pub struct ParserPass {
    reparser: Reparser,
}

impl ParserPass {
    /// Create a pass for `grammar` with default parser/reparser settings.
    pub fn new(grammar: &'static Grammar) -> Self {
        let parser = Parser::new(grammar);
        let reparser = Reparser::from_parser(parser);
        Self { reparser }
    }

    /// Create with custom parser and reparser configuration.
    pub fn with_config(
        grammar: &'static Grammar,
        parser_config: ParserConfig,
        reparser_config: ReparserConfig,
    ) -> Self {
        let mut parser = Parser::new(grammar);
        parser.set_config(parser_config);
        let mut reparser = Reparser::from_parser(parser);
        reparser.set_config(reparser_config);
        Self { reparser }
    }
}

impl scheme::Pass<SourceText, ParseTreeIR> for ParserPass {
    fn push(
        &mut self,
        upstream: &LayerObserver<SourceText>,
        downstream: &ParseTreeIR,
        txn: &[scheme::LayerCommand<SourceText>],
    ) -> Vec<scheme::LayerCommand<ParseTreeIR>> {
        // Every SourceText transaction must carry at least one edit command that
        // identifies which document changed. No URI -> nothing to do.
        let Some(uri) = find_uri_in_txn(txn) else {
            return Vec::new();
        };

        let new_text_atom = match full_source_text(upstream, &uri) {
            Ok(atom) => atom,
            Err(err) if err.is_resolvable() => return Vec::new(),
            Err(err) => {
                eprintln!("[ParserPass::transform] Permanent error for uri {uri:?}: {err:?}");
                return Vec::new();
            }
        };
        let new_text = new_text_atom.as_ref().as_str();

        // For single-edit transactions, attempt incremental re-parse.
        if let Some((edit_uri, span, new_len)) = extract_edit(txn) {
            debug_assert_eq!(edit_uri, uri);
            if downstream.roots.contains_key(&uri) {
                let result = self
                    .reparser
                    .handle_edit(&uri, span, new_len, new_text, None);
                if let Ok(edit_result) = result {
                    let cmds =
                        prepend_messages_command(&uri, &self.reparser.parser.messages, edit_result);
                    return cmds;
                }
                // Incremental re-parse failed; fall through to full re-parse.
            }
        }

        full_parse_transaction(self, &uri, new_text)
    }

    fn resolve(
        &mut self,
        upstream: &LayerObserver<SourceText>,
        _downstream: &ParseTreeIR,
        index: ParseTreeQuery,
    ) -> scheme::ResolveOutcome<ParseTreeIR> {
        let uri = match index {
            ParseTreeQuery::Path(path) => path.0,
            ParseTreeQuery::Message(uri) => uri,
            ParseTreeQuery::Allocator => return scheme::ResolveOutcome::Impossible,
        };

        let source = match full_source_text(upstream, &uri) {
            Ok(source) => source,
            Err(err) if err.is_resolvable() => return scheme::ResolveOutcome::Blocked,
            Err(_) => return scheme::ResolveOutcome::Impossible,
        };

        let txn = full_parse_transaction(self, &uri, source.as_ref().as_str());
        if txn.is_empty() {
            scheme::ResolveOutcome::Impossible
        } else {
            scheme::ResolveOutcome::Done(std::sync::Arc::new(txn))
        }
    }
}

fn full_source_text(
    upstream: &LayerObserver<SourceText>,
    uri: &URI,
) -> Result<crate::scheme::SourceAtom, ObserveError<SourceFault>> {
    upstream.query(DocumentSpan {
        uri: *uri,
        span: Span::new(0, usize::MAX),
    })
}

fn full_parse_transaction(
    pass: &mut ParserPass,
    uri: &URI,
    new_text: &str,
) -> Vec<scheme::LayerCommand<ParseTreeIR>> {
    let crate::parsec::Result { root, .. } = pass.reparser.parser.parse_text(new_text);
    pass.reparser.current = std::sync::Arc::new(root.clone());
    let tree_cmds = delta::generate_commands_for_full_tree(
        &pass.reparser.parser.alloc,
        uri,
        root.green,
        new_text,
    );
    prepend_messages_command(uri, &pass.reparser.parser.messages, tree_cmds)
}

/// Extract the single edit (URI, Span, new_len) from a SourceText transaction.
/// Returns `None` for multi-edit batches or transactions with no edit commands.
fn extract_edit(txn: &[scheme::LayerCommand<SourceText>]) -> Option<(URI, Span, usize)> {
    let mut staged_len: FxHashMap<usize, usize> = FxHashMap::default();
    let mut edit_count = 0usize;
    let mut result: Option<(URI, Span, usize)> = None;

    for cmd in txn {
        match cmd {
            scheme::Command::Create { id, value } => {
                staged_len.insert(*id, value.len());
            }
            scheme::Command::Delete { index: ds } => {
                edit_count += 1;
                result = Some((ds.uri, ds.span, 0));
            }
            scheme::Command::Insert { index: ds, id } => {
                let new_len = staged_len.get(id).copied().unwrap_or(0);
                edit_count += 1;
                result = Some((ds.uri, ds.span, new_len));
            }
            scheme::Command::Replace { index: ds, id } => {
                let new_len = staged_len.get(id).copied().unwrap_or(0);
                edit_count += 1;
                result = Some((ds.uri, ds.span, new_len));
            }
        }
    }

    if edit_count == 1 { result } else { None }
}

/// Extract just the URI from any edit command in the transaction (for multi-edit batches).
fn find_uri_in_txn(txn: &[scheme::LayerCommand<SourceText>]) -> Option<URI> {
    for cmd in txn {
        match cmd {
            scheme::Command::Delete { index: ds } => return Some(ds.uri),
            scheme::Command::Insert { index: ds, .. } => return Some(ds.uri),
            scheme::Command::Replace { index: ds, .. } => return Some(ds.uri),
            scheme::Command::Create { .. } => {}
        }
    }
    None
}

/// Prepend a `ParseNodeValue::Messages` Create command (id=0) carrying the
/// parser-level message list so the CST layer can include errors that are not
/// encoded as error nodes in the green tree (e.g. panic-mode skip errors
/// dropped by `finalize_root`).  Tree-node IDs start at 1, so id=0 is safe.
fn prepend_messages_command(
    uri: &URI,
    messages: &crate::parsec::msg::ParserMessages,
    rest: Vec<scheme::LayerCommand<ParseTreeIR>>,
) -> Vec<scheme::LayerCommand<ParseTreeIR>> {
    if messages.is_empty() {
        return rest;
    }
    let mut cmds = Vec::with_capacity(rest.len() + 1);
    cmds.push(scheme::Command::Create {
        id: 0,
        value: ParseNodeValue::Messages {
            uri: *uri,
            messages: messages.clone(),
        },
    });
    cmds.extend(rest);
    cmds
}
