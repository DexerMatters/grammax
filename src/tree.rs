use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::ops;
use std::sync::Arc;

use dashmap::DashMap;

type GreenId = usize;

type RuleId = usize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub fn len(&self) -> usize {
        self.end - self.start
    }
}

impl ops::Add for Span {
    type Output = Span;

    fn add(self, other: Span) -> Span {
        Span {
            start: self.start,
            end: other.end,
        }
    }
}

pub struct RedNode {
    pub parent: Option<Box<RedNode>>,
    pub span: Span,
    pub data: GreenId,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GreenNode {
    pub rule_id: RuleId,
    pub children: Vec<GreenId>,
}

pub(crate) struct TreeAlloc {
    nodes: Vec<GreenNode>,
    dedup: DashMap<u64, Vec<usize>>,
}

pub(crate) type TreeArena = Arc<TreeAlloc>;

impl TreeAlloc {
    pub fn new() -> Self {
        Self {
            nodes: Vec::new(),
            dedup: DashMap::new(),
        }
    }

    pub fn get_node(&self, id: GreenId) -> &GreenNode {
        &self.nodes[id]
    }

    pub fn alloc(&mut self, rule_id: RuleId, children: Vec<GreenId>) -> GreenId {
        let node = GreenNode { rule_id, children };

        let mut hasher = DefaultHasher::new();
        node.hash(&mut hasher);
        let hash = hasher.finish();

        if let Some(indices) = self.dedup.get(&hash) {
            for &idx in indices.iter() {
                if self.nodes[idx] == node {
                    return idx;
                }
            }
        }

        let idx = self.nodes.len();
        self.nodes.push(node);
        self.dedup.entry(hash).or_default().push(idx);
        idx
    }
}
