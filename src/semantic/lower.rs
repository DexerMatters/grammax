use super::Command;
use crate::grammar::Grammar;
use crate::parsec::tree::{GreenId, Tag, TreeAllocRef, TreeAllocRefExt};
use crate::utils::Span;
use std::any::Any;
use std::collections::HashMap;

pub trait Lower: Clone + Send + 'static {
    fn lower(ctx: &LowerContext) -> Self;
}

pub struct LowerContext<'a> {
    green_id: GreenId,
    alloc: &'a TreeAllocRef,
    source: &'a str,
    offset: usize,
    grammar: &'a Grammar,
    nodes: &'a HashMap<GreenId, Box<dyn Any + Send>>,
}

impl<'a> LowerContext<'a> {
    pub fn new(
        green_id: GreenId,
        alloc: &'a TreeAllocRef,
        source: &'a str,
        offset: usize,
        grammar: &'a Grammar,
        nodes: &'a HashMap<GreenId, Box<dyn Any + Send>>,
    ) -> Self {
        Self {
            green_id,
            alloc,
            source,
            offset,
            grammar,
            nodes,
        }
    }

    pub fn rule_name(&self) -> &str {
        let node = self.alloc.get_node(self.green_id);
        match &node.tag {
            Tag::Rule { rule_ix } => self.grammar.name(*rule_ix),
            Tag::Token { rule_ix } => self.grammar.name(*rule_ix),
            _ => "",
        }
    }

    pub fn text(&self) -> &str {
        let node = self.alloc.get_node(self.green_id);
        let end = self.offset + node.width;
        &self.source[self.offset..end]
    }

    pub fn child<T: Lower>(&self, index: usize) -> T {
        let node = self.alloc.get_node(self.green_id);
        if index >= node.children.len() {
            panic!(
                "child index {} out of bounds (len: {})",
                index,
                node.children.len()
            );
        }

        let child_id = node.children[index];

        if let Some(boxed) = self.nodes.get(&child_id) {
            if let Some(value) = boxed.downcast_ref::<T>() {
                return value.clone();
            }
        }

        let mut child_offset = self.offset;
        for &prev_child_id in &node.children[..index] {
            let prev_child = self.alloc.get_node(prev_child_id);
            child_offset += prev_child.width;
        }

        let ctx = LowerContext::new(
            child_id,
            self.alloc,
            self.source,
            child_offset,
            self.grammar,
            self.nodes,
        );

        T::lower(&ctx)
    }

    pub fn children<T: Lower>(&self) -> Vec<T> {
        let node = self.alloc.get_node(self.green_id);
        let mut result = Vec::new();
        let mut child_offset = self.offset;

        for &child_id in &node.children {
            if let Some(boxed) = self.nodes.get(&child_id) {
                if let Some(value) = boxed.downcast_ref::<T>() {
                    result.push(value.clone());
                    let child = self.alloc.get_node(child_id);
                    child_offset += child.width;
                    continue;
                }
            }

            let ctx = LowerContext::new(
                child_id,
                self.alloc,
                self.source,
                child_offset,
                self.grammar,
                self.nodes,
            );

            result.push(T::lower(&ctx));

            let child = self.alloc.get_node(child_id);
            child_offset += child.width;
        }

        result
    }

    pub fn green_id(&self) -> GreenId {
        self.green_id
    }

    pub fn span(&self) -> Span {
        let node = self.alloc.get_node(self.green_id);
        Span::new(self.offset, self.offset + node.width)
    }
}

pub struct SemanticTree<T: Lower> {
    nodes: HashMap<GreenId, Box<dyn Any + Send>>,
    _phantom: std::marker::PhantomData<T>,
}

impl<T: Lower> SemanticTree<T> {
    pub fn new() -> Self {
        Self {
            nodes: HashMap::new(),
            _phantom: std::marker::PhantomData,
        }
    }

    pub fn apply_commands(
        &mut self,
        commands: &[Command],
        alloc: &TreeAllocRef,
        source: &str,
        grammar: &Grammar,
    ) {
        for cmd in commands {
            match cmd {
                Command::Insert { green_id, span } | Command::Update { green_id, span } => {
                    let ctx = LowerContext::new(
                        *green_id,
                        alloc,
                        source,
                        span.start,
                        grammar,
                        &self.nodes,
                    );
                    let value = T::lower(&ctx);
                    self.nodes.insert(*green_id, Box::new(value));
                }
                Command::Delete { green_id, .. } => {
                    self.nodes.remove(green_id);
                }
                _ => {}
            }
        }
    }

    pub fn root(&self, green_id: GreenId) -> Option<&T> {
        self.nodes.get(&green_id)?.downcast_ref::<T>()
    }
}

impl<T: Lower> Default for SemanticTree<T> {
    fn default() -> Self {
        Self::new()
    }
}
