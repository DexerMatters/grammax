use crate::grammar::Grammar;
use crate::parsec::parser::ParserConfig;
use crate::parsec::tree::{GreenId, Tag, TreeAllocRef, TreeAllocRefExt};

pub struct TreeBuilder<'a> {
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
        let name = self.grammar.name(rule_ix);
        let is_helper = name.contains('@');

        if self.config.simple_ast {
            if is_helper && name.contains("@drop_") && children.len() == 1 {
                return children[0];
            }

            let mut flat_children = Vec::with_capacity(children.len());
            for &child_id in &children {
                let child_node = self.alloc.get_node(child_id);
                let should_flatten = if let Tag::Rule { rule_ix: child_ix } = child_node.tag {
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
            self.alloc
                .alloc(Tag::new_rule(rule_ix), flat_children, width)
        } else {
            let width: usize = children
                .iter()
                .map(|id| self.alloc.get_node(*id).width)
                .sum();
            self.alloc.alloc(Tag::new_rule(rule_ix), children, width)
        }
    }
}
