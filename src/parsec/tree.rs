use std::cell::{self, RefCell};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::ops;
use std::rc::Rc;

use dashmap::DashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ParsecError {
    Incomplete,
    UnexpectedToken,
    MissingToken,
    Placeholder,
}

type GreenId = usize;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Tag {
    Rule { rule_ix: usize },
    Token { rule_ix: usize },
    Field { rule_ix: usize, name: &'static str },
    Error(Vec<ParsecError>),
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
    pub(crate) fn new_root(alloc: TreeAllocRef, text: &str) -> Self {
        Self {
            parent: None,
            offset: 0,
            green: alloc.new_placeholder(text.len()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GreenNode {
    pub tag: Tag,
    pub width: usize,
    pub children: Vec<GreenId>,
}

#[derive(Debug)]
pub(crate) struct TreeAlloc {
    nodes: boxcar::Vec<GreenNode>,
    dedup: DashMap<u64, Vec<usize>>,
}

pub(crate) type TreeAllocRef = Rc<RefCell<TreeAlloc>>;

pub(crate) trait TreeAllocRefExt {
    fn get_node(&self, id: GreenId) -> cell::Ref<'_, GreenNode>;
    fn get_node_mut(&self, id: GreenId) -> cell::RefMut<'_, GreenNode>;
    fn alloc_token(&self, tag: Tag, width: usize) -> GreenId;
    fn alloc(&self, tag: Tag, children: Vec<GreenId>, width: usize) -> GreenId;
    fn new_placeholder(&self, width: usize) -> GreenId;
}

impl TreeAlloc {
    pub fn new() -> Self {
        Self {
            nodes: boxcar::Vec::new(),
            dedup: DashMap::new(),
        }
    }
}

impl TreeAllocRefExt for TreeAllocRef {
    fn get_node(&self, id: GreenId) -> std::cell::Ref<'_, GreenNode> {
        cell::Ref::map(self.borrow(), |alloc| alloc.nodes.get(id).unwrap())
    }

    fn get_node_mut(&self, id: GreenId) -> cell::RefMut<'_, GreenNode> {
        cell::RefMut::map(self.borrow_mut(), |alloc| alloc.nodes.get_mut(id).unwrap())
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

        let should_dedup = matches!(node.tag, Tag::Token { .. } | Tag::Error(_));
        let borrowed = self.borrow();
        if should_dedup {
            let mut hasher = DefaultHasher::new();
            node.hash(&mut hasher);
            let hash = hasher.finish();

            if let Some(indices) = borrowed.dedup.get(&hash) {
                for &idx in indices.iter() {
                    if borrowed.nodes[idx] == node {
                        return idx;
                    }
                }
            }

            let idx = borrowed.nodes.count();
            borrowed.nodes.push(node);
            borrowed.dedup.entry(hash).or_default().push(idx);
            idx
        } else {
            let idx = borrowed.nodes.count();
            borrowed.nodes.push(node);
            idx
        }
    }

    fn new_placeholder(&self, width: usize) -> GreenId {
        self.alloc(Tag::Error(vec![ParsecError::Placeholder]), vec![], width)
    }
}
