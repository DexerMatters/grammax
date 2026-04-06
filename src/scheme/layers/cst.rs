use rustc_hash::{FxHashMap, FxHashSet};
use serde::{Serialize, Serializer};
use std::fmt;

use crate::{
    parsec::{
        msg::{ErrorMessage, ParserMessage, ParserMessages},
        tree::{ParsecError, Tag, TreeAllocRef, TreeAllocRefExt},
        view::{NodeView, Viewer},
    },
    scheme::{self, IR, LazyResult, Span, URI},
};

/// Internal path within a single document's parse tree.
/// Each element is a child index at that level of the tree.
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, Default, PartialOrd, Ord, Serialize, serde::Deserialize,
)]
pub struct NodePath(pub Vec<usize>);

impl NodePath {
    /// Root path (empty).
    pub fn root() -> Self {
        NodePath(vec![])
    }

    /// Return the `index`-th direct child of this path.
    pub fn child(&self, index: usize) -> Self {
        let mut v = self.0.clone();
        v.push(index);
        NodePath(v)
    }

    /// Parent path, or `None` if this is the root.
    pub fn parent(&self) -> Option<Self> {
        if self.0.is_empty() {
            None
        } else {
            let mut v = self.0.clone();
            v.pop();
            Some(NodePath(v))
        }
    }

    pub fn is_prefix_of(&self, other: &NodePath) -> bool {
        self.0.len() <= other.0.len() && self.0.iter().zip(other.0.iter()).all(|(a, b)| a == b)
    }

    pub fn is_direct_child_of(&self, parent: &NodePath) -> bool {
        self.0.len() == parent.0.len() + 1 && parent.is_prefix_of(self)
    }
}

impl From<Vec<usize>> for NodePath {
    fn from(path: Vec<usize>) -> Self {
        Self(path)
    }
}

/// Full document-addressed path: URI identifying the document plus the
/// in-tree `NodePath`.  Used at the CST IR API boundary (transactions and
/// queries that must specify which document to target).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Serialize, serde::Deserialize)]
pub struct DocumentNodePath(pub URI, pub Vec<usize>);

impl PartialOrd for DocumentNodePath {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for DocumentNodePath {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0
            .scheme
            .cmp(&other.0.scheme)
            .then_with(|| self.0.path.cmp(&other.0.path))
            .then_with(|| self.1.cmp(&other.1))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ParseTreeQuery {
    Path(DocumentNodePath),
    Message(URI),
    Allocator,
}

impl Default for ParseTreeQuery {
    fn default() -> Self {
        Self::Path(DocumentNodePath::default())
    }
}

impl DocumentNodePath {
    pub fn root_default() -> Self {
        Self(URI::default(), Vec::new())
    }

    pub fn root(uri: impl Into<URI>) -> Self {
        Self(uri.into(), Vec::new())
    }

    /// Return the `index`-th direct child of this path.
    pub fn child(&self, index: usize) -> Self {
        let mut path = self.1.clone();
        path.push(index);
        Self(self.0, path)
    }

    pub fn is_prefix_of(&self, other: &Self) -> bool {
        self.0 == other.0
            && self.1.len() <= other.1.len()
            && self.1.iter().zip(other.1.iter()).all(|(a, b)| a == b)
    }

    pub fn is_direct_child_of(&self, parent: &Self) -> bool {
        self.1.len() == parent.1.len() + 1 && parent.is_prefix_of(self)
    }

    pub fn overlaps_subtree(&self, other: &Self) -> bool {
        self.is_prefix_of(other) || other.is_prefix_of(self)
    }

    pub fn parent(&self) -> Option<Self> {
        if self.1.is_empty() {
            None
        } else {
            let mut path = self.1.clone();
            path.pop();
            Some(Self(self.0, path))
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
        uri: URI,
        messages: ParserMessages,
    },
}

/// Permanent domain errors for `ParseTreeIR`. Absence (unknown URI or invalid
/// path) is represented as `LazyResult::Absent`, not as a variant here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseTreeFault {
    MissingGrammar,
}

#[derive(Clone)]
pub enum ParseTreeValue {
    Node(ParseNodeValue),
    View(NodeView),
    Messages(ParserMessages),
    Allocator(TreeAllocRef),
}

impl fmt::Debug for ParseTreeValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Node(n) => write!(f, "Node({n:?})"),
            Self::View(v) => write!(f, "NodeView(green={}, path={:?})", v.green(), v.path()),
            Self::Messages(m) => write!(f, "Messages({m:?})"),
            Self::Allocator(_) => write!(f, "Allocator(<opaque>)"),
        }
    }
}

impl Serialize for ParseTreeValue {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Node(n) => n.serialize(s),
            // NodeView is not serialisable at the HTTP boundary.
            Self::View(_) => s.serialize_none(),
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
    pub(crate) alloc: TreeAllocRef,
    grammar: Option<&'static crate::grammar::Grammar>,
    /// Per-URI root green node IDs.
    pub roots: FxHashMap<URI, usize>,
    pub staging: FxHashMap<usize, ParseNodeValue>,
    created: FxHashMap<usize, usize>,
    /// Green node → field name (shared across all documents; green IDs are globally unique).
    fields: FxHashMap<usize, String>,
    token_text: FxHashMap<usize, String>,
    pub forwarded_messages: FxHashMap<URI, ParserMessages>,
    messages_cache: FxHashMap<URI, ParserMessages>,
}

// SAFETY: ParseTreeIR is always owned by a single worker thread when used in
// the concurrent runtime pipeline. It is moved across thread boundaries but
// never shared concurrently.
unsafe impl Send for ParseTreeIR {}

impl fmt::Debug for ParseTreeIR {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ParseTreeIR")
            .field("roots_count", &self.roots.len())
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
            grammar: None,
            roots: FxHashMap::default(),
            staging: FxHashMap::default(),
            created: FxHashMap::default(),
            fields: FxHashMap::default(),
            token_text: FxHashMap::default(),
            forwarded_messages: FxHashMap::default(),
            messages_cache: FxHashMap::default(),
        }
    }

    pub fn with_grammar(grammar: &'static crate::grammar::Grammar) -> Self {
        let mut ir = Self::new();
        ir.grammar = Some(grammar);
        ir
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

    pub fn offset_at_path(&self, path: &DocumentNodePath) -> Option<usize> {
        let mut current = *self.roots.get(&path.0)?;
        let mut offset = 0;
        for &ix in &path.1 {
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
        uri: &URI,
        parent_path: &[usize],
        edit: impl FnOnce(&mut Vec<usize>),
    ) {
        let Some(mut current) = self.roots.get(uri).copied() else {
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
            self.roots.insert(*uri, rebuilt);
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

        self.roots.insert(*uri, rebuilt);
    }

    fn apply_child_edits(&mut self, uri: &URI, parent_path: &[usize], edits: &[PendingChildEdit]) {
        if edits.is_empty() {
            return;
        }

        self.rebuild_parent_with_edit(uri, parent_path, |children| {
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

    pub fn green_at_path(&self, path: &DocumentNodePath) -> Option<usize> {
        let mut current = *self.roots.get(&path.0)?;
        for &ix in &path.1 {
            let node = self.alloc.node(current);
            if ix >= node.children.len() {
                return None;
            }
            current = node.children[ix];
        }
        Some(current)
    }

    /// Look up a green node by URI and in-tree `NodePath`.
    pub fn green_at_node_path(&self, path: &DocumentNodePath) -> Option<usize> {
        let mut current = *self.roots.get(&path.0)?;
        for &ix in &path.1 {
            let node = self.alloc.node(current);
            if ix >= node.children.len() {
                return None;
            }
            current = node.children[ix];
        }
        Some(current)
    }

    /// Return the byte offset (from document start) of the node at `path`
    /// within the document identified by `uri`.
    pub fn offset_at_node_path(&self, path: &DocumentNodePath) -> Option<usize> {
        let mut current = *self.roots.get(&path.0)?;
        let mut offset = 0;
        for &ix in &path.1 {
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
                        ErrorMessage::InternalError {
                            message: format!("{err:?}"),
                        }
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

    /// Returns messages for a specific URI. Returns `None` if the URI is unknown.
    pub fn parser_messages(&self, uri: &URI) -> Option<ParserMessages> {
        self.messages_cache.get(uri).cloned()
    }

    fn compute_parser_messages(&self) -> FxHashMap<URI, ParserMessages> {
        let mut result: FxHashMap<URI, ParserMessages> = FxHashMap::default();

        // Seed with forwarded messages per URI.
        for (uri, fwd) in &self.forwarded_messages {
            result.entry(*uri).or_default().extend(fwd.iter().cloned());
        }

        // Collect error nodes from each document's green tree.
        for (uri, &root) in &self.roots {
            let messages = result.entry(*uri).or_default();
            self.collect_messages_from_green(root, 0, messages);
            messages.sort_by_key(|m| (m.span.start, m.span.end));
            messages.dedup_by(|a, b| a.span == b.span && a.message == b.message);
        }

        result
    }

    fn live_green_ids(&self) -> FxHashSet<usize> {
        let mut live = FxHashSet::default();
        let mut stack: Vec<usize> = self.roots.values().copied().collect();
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
    type Fault = ParseTreeFault;

    fn query(&self, index: ParseTreeQuery) -> LazyResult<ParseTreeValue, ParseTreeFault> {
        use LazyResult::*;
        match index {
            ParseTreeQuery::Message(uri) => {
                if !self.roots.contains_key(&uri) && !self.messages_cache.contains_key(&uri) {
                    return Absent;
                }
                Present(ParseTreeValue::Messages(
                    self.messages_cache.get(&uri).cloned().unwrap_or_default(),
                ))
            }
            // Return a detached snapshot so query consumers never race with
            // concurrent allocator mutation in the pipeline thread.
            ParseTreeQuery::Allocator => Present(ParseTreeValue::Allocator(self.alloc.snapshot())),
            ParseTreeQuery::Path(path) => {
                if !self.roots.contains_key(&path.0) {
                    return Absent;
                }
                let Some(green) = self.green_at_path(&path) else {
                    return Absent;
                };
                let Some(grammar) = self.grammar else {
                    return Fault(ParseTreeFault::MissingGrammar);
                };
                let Some(offset) = self.offset_at_path(&path) else {
                    return Absent;
                };
                let alloc_snapshot = self.alloc.snapshot();
                Present(ParseTreeValue::View(
                    Viewer::new(grammar, alloc_snapshot, String::new())
                        .with_token_texts(self.token_text.clone())
                        .node(green, offset)
                        .with_path(path.1.clone().into()),
                ))
            }
        }
    }

    fn apply_transaction(
        &mut self,
        transaction: scheme::Transaction<Self>,
    ) -> Result<(), ParseTreeFault> {
        // Flush a batch of child edits for the current (URI, parent_path) pair.
        let flush_pending =
            |this: &mut Self,
             pending_parent: &mut Option<(URI, Vec<usize>)>,
             pending_edits: &mut Vec<PendingChildEdit>| {
                if let Some((uri, parent_path)) = pending_parent.take() {
                    this.apply_child_edits(&uri, &parent_path, pending_edits);
                    pending_edits.clear();
                }
            };

        self.staging.clear();
        self.created.clear();
        self.forwarded_messages.clear();
        let mut pending_parent: Option<(URI, Vec<usize>)> = None;
        let mut pending_edits: Vec<PendingChildEdit> = Vec::new();

        for command in transaction.iter() {
            match command {
                scheme::Command::Create { id, value } => {
                    // Extract the inner ParseNodeValue; skip non-node values.
                    let value = match value {
                        ParseTreeValue::Node(n) => n,
                        _ => continue,
                    };

                    // Messages variant carries forwarded parser-level errors for a URI.
                    if let ParseNodeValue::Messages { uri, messages } = value {
                        self.forwarded_messages.insert(*uri, messages.clone());
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

                    let uri = &index.0;
                    let path = &index.1;

                    if path.is_empty() {
                        // Setting/initialising the root for this URI.
                        flush_pending(self, &mut pending_parent, &mut pending_edits);
                        self.roots.insert(*uri, green);
                        continue;
                    }

                    let parent_path = &path[..path.len() - 1];
                    let at = path[path.len() - 1];

                    let matches = pending_parent
                        .as_ref()
                        .is_some_and(|(u, p)| u == uri && p.as_slice() == parent_path);
                    if !matches {
                        flush_pending(self, &mut pending_parent, &mut pending_edits);
                        pending_parent = Some((*uri, parent_path.to_vec()));
                    }
                    pending_edits.push(PendingChildEdit::Insert { at, green });
                }
                scheme::Command::Delete { index } => {
                    let ParseTreeQuery::Path(index) = index else {
                        continue;
                    };

                    let uri = &index.0;
                    let path = &index.1;

                    if path.is_empty() {
                        flush_pending(self, &mut pending_parent, &mut pending_edits);
                        self.roots.remove(uri);
                        continue;
                    }

                    let parent_path = &path[..path.len() - 1];
                    let at = path[path.len() - 1];

                    let matches = pending_parent
                        .as_ref()
                        .is_some_and(|(u, p)| u == uri && p.as_slice() == parent_path);
                    if !matches {
                        flush_pending(self, &mut pending_parent, &mut pending_edits);
                        pending_parent = Some((*uri, parent_path.to_vec()));
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

                    let uri = &index.0;
                    let path = &index.1;

                    if path.is_empty() {
                        // Replace root — init if not yet present, matches source.rs behaviour.
                        flush_pending(self, &mut pending_parent, &mut pending_edits);
                        self.roots.insert(*uri, green);
                        continue;
                    }

                    let parent_path = &path[..path.len() - 1];
                    let at = path[path.len() - 1];

                    let matches = pending_parent
                        .as_ref()
                        .is_some_and(|(u, p)| u == uri && p.as_slice() == parent_path);
                    if !matches {
                        flush_pending(self, &mut pending_parent, &mut pending_edits);
                        pending_parent = Some((*uri, parent_path.to_vec()));
                    }
                    pending_edits.push(PendingChildEdit::Replace { at, green });
                }
            }
        }
        flush_pending(self, &mut pending_parent, &mut pending_edits);

        // Evict stale metadata for green nodes no longer in any tree.
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
