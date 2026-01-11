use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::ops;

use dashmap::DashMap;

type GreenId = usize;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Tag {
    Rule(usize),
    Error(GrammarError),
}

pub struct RedNode {
    pub parent: Option<Box<RedNode>>,
    pub offset: usize,
    pub green: GreenId,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GreenNode {
    pub tag: Tag,
    pub width: usize,
    pub children: Vec<GreenId>,
}

pub(crate) struct TreeAlloc {
    nodes: boxcar::Vec<GreenNode>,
    dedup: DashMap<u64, Vec<usize>>,
}

impl TreeAlloc {
    pub fn new() -> Self {
        Self {
            nodes: boxcar::Vec::new(),
            dedup: DashMap::new(),
        }
    }

    pub fn get_node(&self, id: GreenId) -> &GreenNode {
        &self.nodes[id]
    }

    pub fn get_node_mut(&mut self, id: GreenId) -> &mut GreenNode {
        self.nodes.get_mut(id).unwrap()
    }

    pub fn alloc(&self, tag: Tag, children: Vec<GreenId>, width: usize) -> GreenId {
        let node = GreenNode {
            tag,
            children,
            width,
        };

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

        let idx = self.nodes.count();
        self.nodes.push(node);
        self.dedup.entry(hash).or_default().push(idx);
        idx
    }

    pub fn new_placeholder(&self, width: usize) -> GreenId {
        self.alloc(Tag::Error(GrammarError::Placeholder), vec![], width)
    }
}

impl ops::Index<GreenId> for TreeAlloc {
    type Output = GreenNode;

    fn index(&self, index: GreenId) -> &Self::Output {
        &self.nodes[index]
    }
}

impl ops::IndexMut<GreenId> for TreeAlloc {
    fn index_mut(&mut self, index: GreenId) -> &mut Self::Output {
        self.nodes.get_mut(index).unwrap()
    }
}
