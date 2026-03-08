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
