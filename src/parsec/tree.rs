use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::ops;

use dashmap::DashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ParsecError {
    Incomplete,
    UnexpectedToken,
    Placeholder,
}

type GreenId = usize;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Tag {
    Rule { rule_ix: usize },
    Token { rule_ix: usize, text: String },
    Error(Vec<ParsecError>),
}

impl Tag {
    pub fn new_rule(rule_ix: usize) -> Self {
        Tag::Rule { rule_ix }
    }
    pub fn new_token(rule_ix: usize, text: String) -> Self {
        Tag::Token { rule_ix, text }
    }
    pub fn new_error(err: ParsecError) -> Self {
        Tag::Error(vec![err])
    }
    pub fn is_error(&self) -> bool {
        matches!(self, Tag::Error(_))
    }
}

pub struct RedNode {
    pub parent: Option<Box<RedNode>>,
    pub offset: usize,
    pub green: GreenId,
}

impl RedNode {
    pub(crate) fn new_root(alloc: &TreeAlloc, text: &str) -> Self {
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

    pub fn alloc_token(&self, tag: Tag, width: usize) -> GreenId {
        self.alloc(tag, vec![], width)
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
        self.alloc(Tag::Error(vec![ParsecError::Placeholder]), vec![], width)
    }

    #[allow(dead_code)]
    pub fn display(&self, id: GreenId) -> String {
        self.display_with_indent(id, 0)
    }

    #[allow(dead_code)]
    fn display_with_indent(&self, id: GreenId, indent: usize) -> String {
        let node = &self.nodes[id];
        let mut result = String::new();
        let indent_str = "  ".repeat(indent);

        // Display the tag
        match &node.tag {
            Tag::Rule { rule_ix } => {
                result.push_str(&format!(
                    "{}Rule({}) [width: {}]",
                    indent_str, rule_ix, node.width
                ));
            }
            Tag::Token { rule_ix, text } => {
                result.push_str(&format!(
                    "{}Token({}) [width: {}]",
                    indent_str, rule_ix, node.width
                ));
                if node.children.is_empty() {
                    result.push_str(&format!(" {}", text));
                }
            }
            Tag::Error(errors) => {
                result.push_str(&format!(
                    "{}Error({:?}) [width: {}]",
                    indent_str, errors, node.width
                ));
            }
        }

        // Display children
        if !node.children.is_empty() {
            result.push_str(" {\n");
            for &child_id in &node.children {
                result.push_str(&self.display_with_indent(child_id, indent + 1));
                result.push('\n');
            }
            result.push_str(&format!("{}}}", indent_str));
        }

        result
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
