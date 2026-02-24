use rustc_hash::FxHashMap;
use std::cell::{self, RefCell};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use crate::utils::Span;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ParsecError {
    Incomplete,
    UnexpectedToken,
    MissingToken,
    Placeholder,
    LRError, // Added for LR parser
}

pub type GreenId = usize;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Tag {
    Rule { rule_ix: usize },
    Token { rule_ix: usize }, // Usually terminal_idx
    Field { rule_ix: usize, name: &'static str },
    Error(Vec<ParsecError>),
    Root,
}

impl Tag {
    pub fn new_rule(rule_ix: usize) -> Self {
        Tag::Rule { rule_ix }
    }
    pub fn new_token(rule_ix: usize) -> Self {
        Tag::Token { rule_ix }
    }
    pub fn new_field(rule_ix: usize, name: &'static str) -> Self {
        Tag::Field { rule_ix, name }
    }
    pub fn new_error(err: ParsecError) -> Self {
        Tag::Error(vec![err])
    }
    pub fn is_error(&self) -> bool {
        matches!(self, Tag::Error(_))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RedNode {
    pub parent: Option<Rc<RedNode>>,
    pub offset: usize,
    pub green: GreenId,
}

impl RedNode {
    pub fn new_root(alloc: &TreeAllocRef, text: &str) -> Self {
        Self {
            parent: None,
            offset: 0,
            green: alloc.new_placeholder(text.len()),
        }
    }

    pub fn root(green: GreenId) -> Self {
        Self {
            parent: None,
            offset: 0,
            green,
        }
    }

    pub fn root_with_span(green: GreenId, span: Span) -> Self {
        Self {
            parent: None,
            offset: span.start,
            green,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GreenNode {
    pub tag: Tag,
    pub width: usize,
    pub children: Vec<GreenId>,
}

pub struct TreeAlloc {
    nodes: Vec<GreenNode>,
    dedup: FxHashMap<u64, Vec<usize>>,
}

pub type TreeAllocRef = Rc<RefCell<TreeAlloc>>;

pub trait TreeAllocRefExt {
    fn create() -> Self;
    fn get_node(&self, id: GreenId) -> cell::Ref<'_, GreenNode>;
    fn alloc_token(&self, tag: Tag, width: usize) -> GreenId;
    fn alloc(&self, tag: Tag, children: Vec<GreenId>, width: usize) -> GreenId;
    fn new_placeholder(&self, width: usize) -> GreenId;
}

impl TreeAllocRefExt for TreeAllocRef {
    fn create() -> Self {
        Rc::new(RefCell::new(TreeAlloc {
            nodes: Vec::new(),
            dedup: FxHashMap::default(),
        }))
    }

    fn get_node(&self, id: GreenId) -> cell::Ref<'_, GreenNode> {
        cell::Ref::map(self.borrow(), |alloc| &alloc.nodes[id])
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
                let borrowed = self.borrow();
                if let Some(indices) = borrowed.dedup.get(&hash) {
                    for &idx in indices {
                        if borrowed.nodes[idx] == node {
                            return idx;
                        }
                    }
                }
            }

            let mut borrowed = self.borrow_mut();
            let idx = borrowed.nodes.len();
            borrowed.nodes.push(node);
            borrowed.dedup.entry(hash).or_default().push(idx);
            idx
        } else {
            let mut borrowed = self.borrow_mut();
            let idx = borrowed.nodes.len();
            borrowed.nodes.push(node);
            idx
        }
    }

    fn new_placeholder(&self, width: usize) -> GreenId {
        self.alloc(Tag::Error(vec![ParsecError::Placeholder]), vec![], width)
    }
}
