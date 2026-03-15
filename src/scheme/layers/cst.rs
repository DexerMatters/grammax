use rustc_hash::{FxHashMap, FxHashSet};
use serde::{Serialize, Serializer};
use std::fmt;

use crate::{
    parsec::{
        msg::{ErrorMessage, ParserMessage, ParserMessages},
        tree::{ParsecError, Tag, TreeAllocRef, TreeAllocRefExt},
        view::Viewer,
    },
    scheme::{self, IR},
    utils::Span,
};

#[derive(
    Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Default, Serialize, serde::Deserialize,
)]
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

    pub fn is_prefix_of(&self, other: &Self) -> bool {
        self.0.len() <= other.0.len() && self.0.iter().zip(other.0.iter()).all(|(a, b)| a == b)
    }

    pub fn overlaps_subtree(&self, other: &Self) -> bool {
        self.is_prefix_of(other) || other.is_prefix_of(self)
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

#[derive(Clone)]
pub enum ParseTreeValue {
    Node(ParseNodeValue),
    GreenId(usize),
    Messages(ParserMessages),
    Allocator(TreeAllocRef),
}

impl fmt::Debug for ParseTreeValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Node(n) => write!(f, "Node({n:?})"),
            Self::GreenId(id) => write!(f, "GreenId({id})"),
            Self::Messages(m) => write!(f, "Messages({m:?})"),
            Self::Allocator(_) => write!(f, "Allocator(<opaque>)"),
        }
    }
}

impl Serialize for ParseTreeValue {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Node(n) => n.serialize(s),
            Self::GreenId(id) => id.serialize(s),
            Self::Messages(m) => m.serialize(s),
            // TreeAllocRef is Rc-based and not serialisable; emit null at the
            // HTTP boundary (it is only used internally by the CLI/tests).
            Self::Allocator(_) => s.serialize_none(),
        }
    }
}

// SAFETY: see `SendableAlloc` above.
unsafe impl Send for ParseTreeValue {}
unsafe impl Sync for ParseTreeValue {}

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
    pub staging: FxHashMap<usize, ParseNodeValue>,
    created: FxHashMap<usize, usize>,
    fields: FxHashMap<usize, String>,
    token_text: FxHashMap<usize, String>,
    pub forwarded_messages: ParserMessages,
    /// Pre-computed message list; refreshed at the end of every transaction
    /// so repeated `query(Message)` calls are O(1) instead of O(tree size).
    messages_cache: ParserMessages,
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
            staging: FxHashMap::default(),
            created: FxHashMap::default(),
            fields: FxHashMap::default(),
            token_text: FxHashMap::default(),
            forwarded_messages: Vec::new(),
            messages_cache: Vec::new(),
        }
    }

    pub fn staged(&self, id: usize) -> Option<&ParseNodeValue> {
        self.staging.get(&id)
    }

    fn width_of(&self, green: usize) -> usize {
        self.alloc.width_of(green)
    }

    pub fn viewer(&self, grammar: &'static crate::grammar::Grammar) -> Viewer {
        Viewer::new(grammar, self.alloc.clone(), String::new())
            .with_token_texts(self.token_text.clone())
    }

    pub fn token_text_of(&self, green: usize) -> Option<&str> {
        self.token_text.get(&green).map(String::as_str)
    }

    pub fn offset_at_path(&self, path: &NodePath) -> Option<usize> {
        let mut current = self.root?;
        let mut offset = 0;
        for &ix in &path.0 {
            let node = self.alloc.node(current);
            if ix > node.children.len() {
                return None;
            }
            offset += node
                .children
                .iter()
                .take(ix)
                .map(|&child| self.alloc.width_of(child))
                .sum::<usize>();
            let next = *node.children.get(ix)?;
            current = next;
        }
        Some(offset)
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
                    child_greens.push(self.created.get(child).copied()?);
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
            let node = self.alloc.node(current);
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
            let node = self.alloc.node(current);
            if ix >= node.children.len() {
                return;
            }
            let next = node.children[ix];
            drop(node);
            spine.push((current, ix));
            current = next;
        }

        let node = self.alloc.node(current);
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
            let node = self.alloc.node(ancestor);
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
            let node = self.alloc.node(current);
            if ix >= node.children.len() {
                return None;
            }
            current = node.children[ix];
        }
        Some(current)
    }

    pub fn value_of_green(&self, green: usize) -> ParseNodeValue {
        let node = self.alloc.node(green);
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

    fn collect_messages_from_green(
        &self,
        root: usize,
        root_offset: usize,
        out: &mut ParserMessages,
    ) {
        // Iterative DFS to avoid stack overflow on deep/right-skewed trees.
        let mut stack: Vec<(usize, usize)> = vec![(root, root_offset)];
        while let Some((green, offset)) = stack.pop() {
            let node = self.alloc.node(green);
            let tag = node.tag.clone();
            let width = node.width;
            // Compute child offsets before dropping the node borrow.
            let children_with_offsets: Vec<(usize, usize)> = {
                let mut child_offset = offset;
                node.children
                    .iter()
                    .map(|&child| {
                        let w = self.alloc.width_of(child);
                        let spec = (child, child_offset);
                        child_offset = child_offset.saturating_add(w);
                        spec
                    })
                    .collect()
            };
            drop(node);

            if let Tag::Error(err) = tag {
                let message = match err {
                    ParsecError::UnexpectedToken { expected } => {
                        ErrorMessage::UnexpectedToken { expected }
                    }
                    ParsecError::MissingToken { expected } => {
                        ErrorMessage::MissingToken { expected }
                    }
                    ParsecError::Incomplete | ParsecError::Placeholder | ParsecError::LRError => {
                        ErrorMessage::Custom(0)
                    }
                };
                out.push(ParserMessage {
                    span: Span::new(offset, offset + width),
                    message,
                });
            }

            // Push in reverse so children are popped in forward order.
            for pair in children_with_offsets.into_iter().rev() {
                stack.push(pair);
            }
        }
    }

    pub fn parser_messages(&self) -> ParserMessages {
        // Pre-computed in `apply_transaction`; O(1) to return.
        self.messages_cache.clone()
    }

    fn compute_parser_messages(&self) -> ParserMessages {
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

    fn live_green_ids(&self) -> FxHashSet<usize> {
        let mut live = FxHashSet::default();
        let Some(root) = self.root else {
            return live;
        };
        let mut stack = vec![root];
        while let Some(green) = stack.pop() {
            if live.insert(green) {
                let node = self.alloc.node(green);
                for &child in &node.children {
                    stack.push(child);
                }
            }
        }
        live
    }
}

impl IR for ParseTreeIR {
    type Ix = ParseTreeQuery;
    type Value = ParseTreeValue;
    type Error = ParseTreeError;

    fn query(&self, index: ParseTreeQuery) -> Result<ParseTreeValue, Self::Error> {
        match index {
            ParseTreeQuery::Message => Ok(ParseTreeValue::Messages(self.parser_messages())),
            ParseTreeQuery::Allocator => Ok(ParseTreeValue::Allocator(self.alloc.clone())),
            ParseTreeQuery::Path(path) => {
                let green = match self.green_at_path(&path) {
                    Some(green) => green,
                    None if self.root.is_none() => return Err(ParseTreeError::MissingRoot),
                    None => return Err(ParseTreeError::InvalidPath(path)),
                };
                Ok(ParseTreeValue::GreenId(green))
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
                    // Extract the inner ParseNodeValue; skip non-node values.
                    let value = match value {
                        ParseTreeValue::Node(n) => n,
                        _ => continue,
                    };

                    // Messages variant carries forwarded parser-level errors.
                    if let ParseNodeValue::Messages { messages } = value {
                        self.forwarded_messages.clone_from(messages);
                        continue;
                    }

                    self.staging.insert(*id, value.clone());

                    if let Some(green) = self.alloc_from_value(value) {
                        self.created.insert(*id, green);
                    }
                }
                scheme::Command::Insert { index, id } => {
                    let ParseTreeQuery::Path(index) = index else {
                        continue;
                    };
                    let Some(&green) = self.created.get(id) else {
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
                    let Some(&green) = self.created.get(id) else {
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
                    self.root = (*id).and_then(|ix| self.created.get(&ix).copied());
                }
            }
        }
        flush_pending(self, &mut pending_parent, &mut pending_edits);

        // Evict stale metadata for green nodes no longer in the tree (fix 5).
        let live = self.live_green_ids();
        self.fields.retain(|id, _| live.contains(id));
        self.token_text.retain(|id, _| live.contains(id));

        // Pre-compute message cache so query(Message) is O(1) until next txn.
        self.messages_cache = self.compute_parser_messages();

        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingChildEdit {
    Insert { at: usize, green: usize },
    Delete { at: usize },
    Replace { at: usize, green: usize },
}
