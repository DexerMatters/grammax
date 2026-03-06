use rustc_hash::FxHashMap;
use serde::Serialize;
use std::fmt;

use crate::{
    parsec::tree::{ParsecError, Tag, TreeAllocRef, TreeAllocRefExt},
    scheme::{self, IR},
};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Serialize)]
pub struct NodePath(pub Vec<usize>);

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
}

impl ParseNodeValue {
    pub fn field(&self) -> &str {
        match self {
            ParseNodeValue::Token { field, .. }
            | ParseNodeValue::Error { field, .. }
            | ParseNodeValue::Node { field, .. } => field,
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
}

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
}

impl IR for ParseTreeIR {
    type Ix = NodePath;
    type Value = ParseNodeValue;
    type Error = std::convert::Infallible;

    fn query(&self, index: NodePath) -> Result<ParseNodeValue, Self::Error> {
        let green = self
            .green_at_path(&index)
            .expect("ParseTreeIR::query: invalid path or missing root");
        Ok(self.value_of_green(green))
    }

    fn apply_transaction(
        &mut self,
        transaction: scheme::Transaction<Self>,
    ) -> Result<(), Self::Error> {
        self.staging.clear();
        self.created.clear();
        for command in transaction {
            match command {
                scheme::Command::Create { id, value } => {
                    if id >= self.staging.len() {
                        self.staging.resize(id + 1, None);
                    }
                    self.staging[id] = Some(value.clone());

                    if id >= self.created.len() {
                        self.created.resize(id + 1, None);
                    }
                    self.created[id] = self.alloc_from_value(&value);
                }
                scheme::Command::Insert { index, id } => {
                    let Some(green) = self.created.get(id).and_then(|v| *v) else {
                        continue;
                    };

                    if index.0.is_empty() {
                        self.root = Some(green);
                        continue;
                    }

                    let parent_path = &index.0[..index.0.len() - 1];
                    let at = index.0[index.0.len() - 1];
                    self.rebuild_parent_with_edit(parent_path, |children| {
                        let pos = at.min(children.len());
                        children.insert(pos, green);
                    });
                }
                scheme::Command::Delete { index } => {
                    if index.0.is_empty() {
                        self.root = None;
                        continue;
                    }

                    let parent_path = &index.0[..index.0.len() - 1];
                    let at = index.0[index.0.len() - 1];
                    self.rebuild_parent_with_edit(parent_path, |children| {
                        if at < children.len() {
                            children.remove(at);
                        }
                    });
                }
                scheme::Command::Replace { index, id } => {
                    let Some(green) = self.created.get(id).and_then(|v| *v) else {
                        continue;
                    };

                    if index.0.is_empty() {
                        self.root = Some(green);
                        continue;
                    }

                    let parent_path = &index.0[..index.0.len() - 1];
                    let at = index.0[index.0.len() - 1];
                    self.rebuild_parent_with_edit(parent_path, |children| {
                        if at < children.len() {
                            children[at] = green;
                        }
                    });
                }
                scheme::Command::SetRoot { id } => {
                    self.root = id.and_then(|ix| self.created.get(ix).and_then(|v| *v));
                }
            }
        }
        Ok(())
    }
}

// ── Command type alias ────────────────────────────────────────────────────────

/// A concrete parse-tree command — `scheme::Command` specialised for the parse-tree IR.
pub type Command = scheme::Command<ParseTreeIR>;

/// Alias for Layer 2 in the terraced model terminology.
pub type RedGreenTreeIR = ParseTreeIR;

/// Alias emphasizing that Layer 2 transactions are parser commands.
pub type ParserCommand = Command;
