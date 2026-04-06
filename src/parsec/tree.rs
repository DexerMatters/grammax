use rustc_hash::FxHashMap;
use serde::Serialize;
use std::cell::UnsafeCell;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use crate::grammar::Grammar;
use crate::parsec::ParserConfig;

/// Error types that can occur during parsing, used for error reporting and recovery.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ParsecError {
    Incomplete,
    UnexpectedToken { expected: Vec<usize> },
    MissingToken { expected: Vec<usize> },
    Placeholder,
    LRError, // Added for LR parser
}

/// A type alias for the identifier of a green node in the syntax tree. This is used to reference nodes in the tree allocator.
pub(crate) type GreenId = usize;

/// Tags indicating the type of a syntax tree node, such as whether it's a rule, token, field, or an error.
///
/// `Tag::Rule` carries two rule indices:
/// - `rule_ix`        — the "display" rule used for semantics/display (e.g. `expr`).
/// - `reparse_rule_ix`— the original rule used to produce this node (e.g. `expr@drop_1`).
///                      Used by the incremental reparser to pick the correct grammar entry
///                      point when re-parsing a subtree, so drop constraints are respected.
///                      Excluded from PartialEq/Hash so tree equivalence is based on structure.
#[derive(Debug, Clone, Serialize)]
pub(crate) enum Tag {
    Rule {
        rule_ix: usize,
        #[serde(skip)]
        reparse_rule_ix: usize,
    },
    Token {
        rule_ix: usize,
    }, // Usually terminal_idx
    Field {
        rule_ix: usize,
        name: &'static str,
    },
    Error(ParsecError),
}

impl PartialEq for Tag {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            // Intentionally ignore reparse_rule_ix — tree equivalence is based on display rule.
            (Tag::Rule { rule_ix: a, .. }, Tag::Rule { rule_ix: b, .. }) => a == b,
            (Tag::Token { rule_ix: a }, Tag::Token { rule_ix: b }) => a == b,
            (
                Tag::Field {
                    rule_ix: a,
                    name: na,
                },
                Tag::Field {
                    rule_ix: b,
                    name: nb,
                },
            ) => a == b && na == nb,
            (Tag::Error(a), Tag::Error(b)) => a == b,
            _ => false,
        }
    }
}

impl Eq for Tag {}

impl std::hash::Hash for Tag {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        match self {
            // Intentionally skip reparse_rule_ix to match PartialEq.
            Tag::Rule { rule_ix, .. } => {
                0u8.hash(state);
                rule_ix.hash(state);
            }
            Tag::Token { rule_ix } => {
                1u8.hash(state);
                rule_ix.hash(state);
            }
            Tag::Field { rule_ix, name } => {
                2u8.hash(state);
                rule_ix.hash(state);
                name.hash(state);
            }
            Tag::Error(e) => {
                3u8.hash(state);
                e.hash(state);
            }
        }
    }
}

impl Tag {
    pub fn new_rule(rule_ix: usize) -> Self {
        Tag::Rule {
            rule_ix,
            reparse_rule_ix: rule_ix,
        }
    }
    pub fn new_token(rule_ix: usize) -> Self {
        Tag::Token { rule_ix }
    }
    pub fn new_error(err: ParsecError) -> Self {
        Tag::Error(err)
    }
}

/// Red nodes are the nodes in the "red" syntax tree,
/// which includes parent references and offsets for easier traversal and error reporting.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct RedNode {
    pub parent: Option<Rc<RedNode>>,
    pub offset: usize,
    pub green: GreenId,
}

impl RedNode {
    /// Creates a root red node with the given green node ID.
    /// Its parent is None and offset is 0.
    pub fn root(green: GreenId) -> Self {
        Self {
            parent: None,
            offset: 0,
            green,
        }
    }
}

/// Green nodes are the nodes in the "green" syntax tree, which are immutable and can be shared.
/// They contain the tag, width, and children references, and are interned for memory efficiency.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct GreenNode {
    pub tag: Tag,
    pub width: usize,
    pub children: Vec<GreenId>,
}

/// The tree allocator is responsible for managing the allocation and deduplication of green nodes.
pub struct TreeAlloc {
    nodes: Vec<GreenNode>,
    dedup: FxHashMap<u64, Vec<usize>>,
}

/// A shared reference to the tree allocator.
pub type TreeAllocRef = Rc<UnsafeCell<TreeAlloc>>;

/// A trait that extends the functionality of the tree allocator reference, providing methods for creating and managing green nodes.
pub(crate) trait TreeAllocRefExt {
    /// Creates a new tree allocator reference.
    fn create() -> Self;
    /// Creates an immutable snapshot clone safe to move away from the mutating owner thread.
    fn snapshot(&self) -> Self;
    /// Retrieves a cloned green node snapshot by its ID.
    fn get_node(&self, id: GreenId) -> GreenNode;
    /// Clones a green node snapshot by its ID, avoiding long-lived `RefCell` borrows.
    fn node(&self, id: GreenId) -> GreenNode;
    /// Reads a green node width without exposing the underlying borrow.
    fn width_of(&self, id: GreenId) -> usize;
    fn alloc_token(&self, tag: Tag, width: usize) -> GreenId;
    fn alloc(&self, tag: Tag, children: Vec<GreenId>, width: usize) -> GreenId;
}

impl TreeAllocRefExt for TreeAllocRef {
    fn create() -> Self {
        Rc::new(UnsafeCell::new(TreeAlloc {
            nodes: Vec::new(),
            dedup: FxHashMap::default(),
        }))
    }

    fn snapshot(&self) -> Self {
        // SAFETY: TreeAllocRef is single-threaded (`Rc`). We clone owned data
        // out of the current allocator and return a detached copy.
        let borrowed = unsafe { &*self.get() };
        Rc::new(UnsafeCell::new(TreeAlloc {
            nodes: borrowed.nodes.clone(),
            dedup: borrowed.dedup.clone(),
        }))
    }

    fn get_node(&self, id: GreenId) -> GreenNode {
        // SAFETY: TreeAllocRef is single-threaded (`Rc`) and tree reads return owned clones,
        // so no references to inner storage escape across mutation boundaries.
        unsafe { (&*self.get()).nodes[id].clone() }
    }

    fn node(&self, id: GreenId) -> GreenNode {
        self.get_node(id)
    }

    fn width_of(&self, id: GreenId) -> usize {
        // SAFETY: see `get_node`.
        unsafe { (&*self.get()).nodes[id].width }
    }

    fn alloc_token(&self, tag: Tag, width: usize) -> GreenId {
        self.alloc(tag, vec![], width)
    }

    fn alloc(&self, tag: Tag, children: Vec<GreenId>, width: usize) -> GreenId {
        let node = GreenNode {
            tag,
            children,
            width,
        };

        // Deduplicate only errors. Tokens carry user text via source spans, so they
        // should remain distinct per allocation.
        let should_dedup = matches!(node.tag, Tag::Error(_));
        if should_dedup {
            let mut hasher = DefaultHasher::new();
            node.hash(&mut hasher);
            let hash = hasher.finish();

            {
                // SAFETY: see `get_node`.
                let borrowed = unsafe { &*self.get() };
                if let Some(indices) = borrowed.dedup.get(&hash) {
                    for &idx in indices {
                        if borrowed.nodes[idx] == node {
                            return idx;
                        }
                    }
                }
            }

            // SAFETY: see `get_node`.
            let borrowed = unsafe { &mut *self.get() };
            let idx = borrowed.nodes.len();
            borrowed.nodes.push(node);
            borrowed.dedup.entry(hash).or_default().push(idx);
            idx
        } else {
            // SAFETY: see `get_node`.
            let borrowed = unsafe { &mut *self.get() };
            let idx = borrowed.nodes.len();
            borrowed.nodes.push(node);
            idx
        }
    }
}

pub(crate) struct TreeBuilder<'a> {
    grammar: &'a Grammar,
    alloc: &'a TreeAllocRef,
    config: &'a ParserConfig,
}

impl<'a> TreeBuilder<'a> {
    pub fn new(grammar: &'a Grammar, alloc: &'a TreeAllocRef, config: &'a ParserConfig) -> Self {
        Self {
            grammar,
            alloc,
            config,
        }
    }

    pub fn build_node(&self, rule_ix: usize, children: Vec<GreenId>) -> GreenId {
        let reparse_rule_ix = rule_ix; // Original rule, possibly with @drop_ suffix.
        let mut effective_rule_ix = rule_ix; // Display/semantic rule, @drop_ stripped.
        let name = self.grammar.name(rule_ix);

        if let Some(pos) = name.find("@drop_") {
            let target_name = &name[..pos];
            if let Some(ix) = self
                .grammar
                .table
                .rules
                .iter()
                .position(|rule| rule.name == target_name)
            {
                effective_rule_ix = ix;
            }
        }

        let filtered_children: Vec<GreenId> = children
            .into_iter()
            .filter(|child_id| !self.is_silent_token(*child_id))
            .collect();

        if self.config.simple_ast {
            let mut flat_children = Vec::with_capacity(filtered_children.len());
            for &child_id in &filtered_children {
                let child_node = self.alloc.get_node(child_id);
                let should_flatten = if let Tag::Rule {
                    rule_ix: child_ix, ..
                } = child_node.tag
                {
                    let child_name = self.grammar.name(child_ix);
                    child_name.contains('@')
                } else {
                    false
                };

                if should_flatten {
                    flat_children.extend_from_slice(&child_node.children);
                } else {
                    flat_children.push(child_id);
                }
            }

            let width: usize = flat_children
                .iter()
                .map(|id| self.alloc.get_node(*id).width)
                .sum();
            self.alloc.alloc(
                Tag::Rule {
                    rule_ix: effective_rule_ix,
                    reparse_rule_ix,
                },
                flat_children,
                width,
            )
        } else {
            let width: usize = filtered_children
                .iter()
                .map(|id| self.alloc.get_node(*id).width)
                .sum();
            self.alloc.alloc(
                Tag::Rule {
                    rule_ix: effective_rule_ix,
                    reparse_rule_ix,
                },
                filtered_children,
                width,
            )
        }
    }

    fn is_silent_token(&self, id: GreenId) -> bool {
        let node = self.alloc.get_node(id);
        matches!(node.tag, Tag::Token { .. }) && node.children.is_empty() && node.width == 0
    }
}
