use rustc_hash::FxHashMap;
use serde::Serialize;
use std::fmt;

use crate::{
    parsec::{
        msg::{ErrorMessage, ParserMessage, ParserMessages},
        tree::{ParsecError, Tag, TreeAllocRef, TreeAllocRefExt},
    },
    runtime::Payload,
    scheme::{self, IR},
    utils::Span,
};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Serialize, serde::Deserialize)]
pub struct NodePath(pub Vec<usize>);

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ParseTreeQuery {
    Path(NodePath),
    Message,
    Allocator,
}

impl Default for ParseTreeQuery {
    fn default() -> Self {
        Self::Path(NodePath::default())
    }
}

impl NodePath {
    pub fn root() -> Self {
        Self(Vec::new())
    }

    pub fn parent(&self) -> Option<Self> {
        if self.0.is_empty() {
            None
        } else {
            let mut path = self.0.clone();
            path.pop();
            Some(Self(path))
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ParseNodeValue {
    Token {
        rule_ix: usize,
        text: String,
        field: String,
    },
    Error {
        error: ParsecError,
        text: String,
        field: String,
    },
    Node {
        rule_ix: usize,
        children: Vec<usize>,
        field: String,
    },
    Messages {
        messages: ParserMessages,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseTreeError {
    MissingRoot,
    InvalidPath(NodePath),
}

impl ParseNodeValue {
    pub fn field(&self) -> &str {
        match self {
            ParseNodeValue::Token { field, .. }
            | ParseNodeValue::Error { field, .. }
            | ParseNodeValue::Node { field, .. } => field,
            ParseNodeValue::Messages { .. } => "",
        }
    }

    pub fn is_leaf(&self) -> bool {
        matches!(
            self,
            ParseNodeValue::Token { .. } | ParseNodeValue::Error { .. }
        )
    }
}

#[derive(Clone)]
pub struct ParseTreeIR {
    /// Allocator database (IR2) from `parsec/tree.rs`.
    pub alloc: TreeAllocRef,
    /// Current root green node id inside `alloc`.
    pub root: Option<usize>,
    /// Transaction-local staging table cleared before each transaction.
    pub staging: Vec<Option<ParseNodeValue>>,
    created: Vec<Option<usize>>,
    fields: FxHashMap<usize, String>,
    token_text: FxHashMap<usize, String>,
    /// Parser-level messages forwarded directly from the pass (e.g. panic-mode
    /// recovery errors that are not encoded as error nodes in the green tree).
    /// Cleared at the start of each transaction and repopulated if a
    /// `ParseNodeValue::Messages` Create command is present.
    pub forwarded_messages: ParserMessages,
}

// SAFETY: ParseTreeIR is always owned by a single worker thread when used in
// the concurrent runtime pipeline. It is moved across thread boundaries but
// never shared concurrently.
unsafe impl Send for ParseTreeIR {}

impl fmt::Debug for ParseTreeIR {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ParseTreeIR")
            .field("root", &self.root)
            .field("staging_len", &self.staging.len())
            .field("created_len", &self.created.len())
            .finish()
    }
}

impl Default for ParseTreeIR {
    fn default() -> Self {
        Self::new()
    }
}

impl ParseTreeIR {
    pub fn new() -> Self {
        Self {
            alloc: TreeAllocRef::create(),
            root: None,
            staging: Vec::new(),
            created: Vec::new(),
            fields: FxHashMap::default(),
            token_text: FxHashMap::default(),
            forwarded_messages: Vec::new(),
        }
    }

    pub fn staged(&self, id: usize) -> Option<&ParseNodeValue> {
        self.staging.get(id)?.as_ref()
    }

    fn width_of(&self, green: usize) -> usize {
        self.alloc.get_node(green).width
    }

    fn alloc_from_value(&mut self, value: &ParseNodeValue) -> Option<usize> {
        match value {
            ParseNodeValue::Token {
                rule_ix,
                text,
                field,
            } => {
                let id = self.alloc.alloc_token(Tag::new_token(*rule_ix), text.len());
                self.fields.insert(id, field.clone());
                self.token_text.insert(id, text.clone());
                Some(id)
            }
            ParseNodeValue::Error { error, text, field } => {
                let id = self
                    .alloc
                    .alloc_token(Tag::new_error(error.clone()), text.len());
                self.fields.insert(id, field.clone());
                self.token_text.insert(id, text.clone());
                Some(id)
            }
            ParseNodeValue::Node {
                rule_ix,
                children,
                field,
            } => {
                let mut child_greens = Vec::with_capacity(children.len());
                for child in children {
                    child_greens.push(self.created.get(*child)?.as_ref().copied()?);
                }
                let width = child_greens.iter().map(|id| self.width_of(*id)).sum();
                let id = self
                    .alloc
                    .alloc(Tag::new_rule(*rule_ix), child_greens, width);
                self.fields.insert(id, field.clone());
                Some(id)
            }
            ParseNodeValue::Messages { .. } => None,
        }
    }

    fn rebuild_parent_with_edit(
        &mut self,
        parent_path: &[usize],
        edit: impl FnOnce(&mut Vec<usize>),
    ) {
        let Some(mut current) = self.root else {
            return;
        };

        if parent_path.is_empty() {
            let node = self.alloc.get_node(current);
            let mut children = node.children.clone();
            let tag = node.tag.clone();
            let old = current;
            drop(node);

            edit(&mut children);
            let width = if children.is_empty() {
                self.width_of(old)
            } else {
                children.iter().map(|id| self.width_of(*id)).sum()
            };
            let rebuilt = self.alloc.alloc(tag, children, width);
            if let Some(field) = self.fields.get(&old).cloned() {
                self.fields.insert(rebuilt, field);
            }
            self.root = Some(rebuilt);
            return;
        }

        let mut spine: Vec<(usize, usize)> = Vec::new();
        for &ix in parent_path {
            let node = self.alloc.get_node(current);
            if ix >= node.children.len() {
                return;
            }
            let next = node.children[ix];
            drop(node);
            spine.push((current, ix));
            current = next;
        }

        let node = self.alloc.get_node(current);
        let mut children = node.children.clone();
        let tag = node.tag.clone();
        let old_leaf = current;
        drop(node);

        edit(&mut children);
        let width = if children.is_empty() {
            self.width_of(old_leaf)
        } else {
            children.iter().map(|id| self.width_of(*id)).sum()
        };
        let mut rebuilt = self.alloc.alloc(tag, children, width);
        if let Some(field) = self.fields.get(&old_leaf).cloned() {
            self.fields.insert(rebuilt, field);
        }

        for (ancestor, child_ix) in spine.into_iter().rev() {
            let node = self.alloc.get_node(ancestor);
            let mut children = node.children.clone();
            let tag = node.tag.clone();
            drop(node);
            children[child_ix] = rebuilt;
            let width = children.iter().map(|id| self.width_of(*id)).sum();
            let next = self.alloc.alloc(tag, children, width);
            if let Some(field) = self.fields.get(&ancestor).cloned() {
                self.fields.insert(next, field);
            }
            rebuilt = next;
        }

        self.root = Some(rebuilt);
    }

    fn apply_child_edits(&mut self, parent_path: &[usize], edits: &[PendingChildEdit]) {
        if edits.is_empty() {
            return;
        }

        self.rebuild_parent_with_edit(parent_path, |children| {
            for edit in edits {
                match *edit {
                    PendingChildEdit::Insert { at, green } => {
                        let pos = at.min(children.len());
                        children.insert(pos, green);
                    }
                    PendingChildEdit::Delete { at } => {
                        if at < children.len() {
                            children.remove(at);
                        }
                    }
                    PendingChildEdit::Replace { at, green } => {
                        if at < children.len() {
                            children[at] = green;
                        }
                    }
                }
            }
        });
    }

    pub fn green_at_path(&self, path: &NodePath) -> Option<usize> {
        let mut current = self.root?;
        for &ix in &path.0 {
            let node = self.alloc.get_node(current);
            if ix >= node.children.len() {
                return None;
            }
            current = node.children[ix];
        }
        Some(current)
    }

    pub fn value_of_green(&self, green: usize) -> ParseNodeValue {
        let node = self.alloc.get_node(green);
        let field = self.fields.get(&green).cloned().unwrap_or_default();
        match &node.tag {
            Tag::Token { rule_ix } => ParseNodeValue::Token {
                rule_ix: *rule_ix,
                text: self.token_text.get(&green).cloned().unwrap_or_default(),
                field,
            },
            Tag::Error(err) => ParseNodeValue::Error {
                error: err.clone(),
                text: self.token_text.get(&green).cloned().unwrap_or_default(),
                field,
            },
            Tag::Rule { rule_ix, .. } => ParseNodeValue::Node {
                rule_ix: *rule_ix,
                children: node.children.clone(),
                field,
            },
            Tag::Field { rule_ix, .. } => ParseNodeValue::Node {
                rule_ix: *rule_ix,
                children: node.children.clone(),
                field,
            },
        }
    }

    fn collect_messages_from_green(&self, green: usize, offset: usize, out: &mut ParserMessages) {
        let node = self.alloc.get_node(green);
        let tag = node.tag.clone();
        let width = node.width;
        let children = node.children.clone();
        drop(node);

        if let Tag::Error(err) = tag {
            let message = match err {
                ParsecError::UnexpectedToken { expected } => {
                    ErrorMessage::UnexpectedToken { expected }
                }
                ParsecError::MissingToken { expected } => ErrorMessage::MissingToken { expected },
                ParsecError::Incomplete | ParsecError::Placeholder | ParsecError::LRError => {
                    ErrorMessage::Custom(0)
                }
            };

            out.push(ParserMessage {
                span: Span::new(offset, offset + width),
                message,
            });
        }

        let mut child_offset = offset;
        for child in children {
            let child_width = self.alloc.get_node(child).width;
            self.collect_messages_from_green(child, child_offset, out);
            child_offset = child_offset.saturating_add(child_width);
        }
    }

    pub fn parser_messages(&self) -> ParserMessages {
        let mut messages = self.forwarded_messages.clone();
        // Also collect error nodes encoded directly in the green tree (e.g.
        // MissingToken nodes from incremental recovery).  Merge with forwarded
        // messages and deduplicate by span so neither source is lost.
        if let Some(root) = self.root {
            self.collect_messages_from_green(root, 0, &mut messages);
        }
        messages.sort_by_key(|m| (m.span.start, m.span.end));
        messages.dedup_by(|a, b| a.span == b.span && a.message == b.message);
        messages
    }
}

impl IR for ParseTreeIR {
    type Ix = ParseTreeQuery;
    type Value = Payload;
    type Error = ParseTreeError;

    fn query(&self, index: ParseTreeQuery) -> Result<Payload, Self::Error> {
        match index {
            ParseTreeQuery::Message => Ok(Payload::new(self.parser_messages())),
            ParseTreeQuery::Allocator => Ok(Payload::new_any(self.alloc.clone())),
            ParseTreeQuery::Path(path) => {
                let green = match self.green_at_path(&path) {
                    Some(green) => green,
                    None if self.root.is_none() => return Err(ParseTreeError::MissingRoot),
                    None => return Err(ParseTreeError::InvalidPath(path)),
                };
                Ok(Payload::new(green))
            }
        }
    }

    fn apply_transaction(
        &mut self,
        transaction: scheme::Transaction<Self>,
    ) -> Result<(), Self::Error> {
        let flush_pending =
            |this: &mut Self,
             pending_parent: &mut Option<Vec<usize>>,
             pending_edits: &mut Vec<PendingChildEdit>| {
                if let Some(parent_path) = pending_parent.take() {
                    this.apply_child_edits(&parent_path, pending_edits);
                    pending_edits.clear();
                }
            };

        self.staging.clear();
        self.created.clear();
        self.forwarded_messages.clear();
        let mut pending_parent: Option<Vec<usize>> = None;
        let mut pending_edits: Vec<PendingChildEdit> = Vec::new();

        for command in transaction.iter() {
            match command {
                scheme::Command::Create { id, value } => {
                    let Some(value) = value.downcast_ref::<ParseNodeValue>() else {
                        continue;
                    };

                    // Messages variant carries forwarded parser-level errors.
                    if let ParseNodeValue::Messages { messages } = value {
                        self.forwarded_messages.clone_from(messages);
                        continue;
                    }

                    if *id >= self.staging.len() {
                        self.staging.resize(*id + 1, None);
                    }
                    self.staging[*id] = Some(value.clone());

                    if *id >= self.created.len() {
                        self.created.resize(*id + 1, None);
                    }
                    self.created[*id] = self.alloc_from_value(value);
                }
                scheme::Command::Insert { index, id } => {
                    let ParseTreeQuery::Path(index) = index else {
                        continue;
                    };
                    let Some(green) = self.created.get(*id).and_then(|v| *v) else {
                        continue;
                    };

                    if index.0.is_empty() {
                        flush_pending(self, &mut pending_parent, &mut pending_edits);
                        self.root = Some(green);
                        continue;
                    }

                    let parent_path = &index.0[..index.0.len() - 1];
                    let at = index.0[index.0.len() - 1];

                    if pending_parent.as_deref() != Some(parent_path) {
                        flush_pending(self, &mut pending_parent, &mut pending_edits);
                        pending_parent = Some(parent_path.to_vec());
                    }
                    pending_edits.push(PendingChildEdit::Insert { at, green });
                }
                scheme::Command::Delete { index } => {
                    let ParseTreeQuery::Path(index) = index else {
                        continue;
                    };
                    if index.0.is_empty() {
                        flush_pending(self, &mut pending_parent, &mut pending_edits);
                        self.root = None;
                        continue;
                    }

                    let parent_path = &index.0[..index.0.len() - 1];
                    let at = index.0[index.0.len() - 1];

                    if pending_parent.as_deref() != Some(parent_path) {
                        flush_pending(self, &mut pending_parent, &mut pending_edits);
                        pending_parent = Some(parent_path.to_vec());
                    }
                    pending_edits.push(PendingChildEdit::Delete { at });
                }
                scheme::Command::Replace { index, id } => {
                    let ParseTreeQuery::Path(index) = index else {
                        continue;
                    };
                    let Some(green) = self.created.get(*id).and_then(|v| *v) else {
                        continue;
                    };

                    if index.0.is_empty() {
                        flush_pending(self, &mut pending_parent, &mut pending_edits);
                        self.root = Some(green);
                        continue;
                    }

                    let parent_path = &index.0[..index.0.len() - 1];
                    let at = index.0[index.0.len() - 1];

                    if pending_parent.as_deref() != Some(parent_path) {
                        flush_pending(self, &mut pending_parent, &mut pending_edits);
                        pending_parent = Some(parent_path.to_vec());
                    }
                    pending_edits.push(PendingChildEdit::Replace { at, green });
                }
                scheme::Command::SetRoot { id } => {
                    flush_pending(self, &mut pending_parent, &mut pending_edits);
                    self.root = (*id).and_then(|ix| self.created.get(ix).and_then(|v| *v));
                }
            }
        }
        flush_pending(self, &mut pending_parent, &mut pending_edits);
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingChildEdit {
    Insert { at: usize, green: usize },
    Delete { at: usize },
    Replace { at: usize, green: usize },
}
