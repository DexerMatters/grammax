use crate::{
    grammar::Grammar,
    parsec::tree::{GreenId, ParsecError, Tag, TreeAllocRef, TreeAllocRefExt},
};

struct ViewProps<'a> {
    grammar: &'static Grammar,
    alloc: TreeAllocRef,
    source: &'a str,
}

/// Copyable cursor for navigating parse trees
#[derive(Clone, Copy)]
pub struct View<'a> {
    props: &'a ViewProps<'a>,
    pub(crate) node: GreenId,
    pub(crate) offset: usize,
}

impl<'a> View<'a> {
    pub fn new(
        grammar: &'static Grammar,
        alloc: TreeAllocRef,
        source: &'a str,
        node: GreenId,
        offset: usize,
    ) -> Self {
        let props = Box::leak(Box::new(ViewProps {
            grammar,
            alloc,
            source,
        }));
        View {
            props,
            node,
            offset,
        }
    }

    pub fn current(&self) -> GreenId {
        self.node
    }

    pub fn offset(&self) -> usize {
        self.offset
    }

    pub fn width(&self) -> usize {
        self.props.alloc.get_node(self.current()).width
    }

    pub fn text(&self) -> &'a str {
        &self.props.source[self.offset()..self.offset() + self.width()]
    }

    pub fn is_error(&self) -> bool {
        matches!(self.props.alloc.get_node(self.current()).tag, Tag::Error(_))
    }

    /// Move into the first child (non-backward, chainable navigation)
    pub fn into(self) -> Option<View<'a>> {
        let node = self.props.alloc.get_node(self.current());

        if node.children.is_empty() {
            return None;
        }

        Some(View {
            props: self.props,
            node: node.children[0],
            offset: self.offset(),
        })
    }

    /// Move to the next sibling (chainable navigation)
    pub fn next(self) -> Option<View<'a>> {
        let current_width = self.props.alloc.get_node(self.current()).width;

        Some(View {
            props: self.props,
            node: self.node,
            offset: self.offset() + current_width,
        })
    }

    /// Move to the next field matching the given name (chainable)
    pub fn next_field(self, field_name: &str) -> Option<View<'a>> {
        let mut current = self;

        loop {
            let node = current.props.alloc.get_node(current.current());

            // Check if this node has the matching field name
            if let Tag::Field { name, .. } = &node.tag {
                if *name == field_name {
                    return Some(current);
                }
            }

            // Move to next sibling
            let next_offset = current.offset() + node.width;
            // If we've moved beyond reasonable bounds, stop
            if next_offset >= current.props.source.len() {
                return None;
            }

            current = View {
                props: current.props,
                node: current.node,
                offset: next_offset,
            };

            if current.offset() >= current.props.source.len() {
                return None;
            }
        }
    }

    /// Get the rule index if this is a rule or field node
    fn rule_ix(&self) -> Option<usize> {
        let node = self.props.alloc.get_node(self.current());
        match &node.tag {
            Tag::Rule { rule_ix, .. } => Some(*rule_ix),
            Tag::Token { rule_ix } => Some(*rule_ix),
            Tag::Field { rule_ix, .. } => Some(*rule_ix),
            Tag::Error(_) => None,
        }
    }

    /// Move to the next node matching the given rule name (chainable)
    pub fn next_rule(self, rule_name: &str) -> Option<View<'a>> {
        let mut current = self;

        loop {
            let node = current.props.alloc.get_node(current.current());

            // Check if this node has the matching rule name
            if let Some(rule_ix) = current.rule_ix() {
                if rule_ix < current.props.grammar.table.rules.len() {
                    if current.props.grammar.table.rules[rule_ix].name == rule_name {
                        return Some(current);
                    }
                }
            }

            // Move to next sibling
            let next_offset = current.offset() + node.width;
            // If we've moved beyond reasonable bounds, stop
            if next_offset >= current.props.source.len() {
                return None;
            }

            current = View {
                props: current.props,
                node: current.node,
                offset: next_offset,
            };

            if current.offset() >= current.props.source.len() {
                return None;
            }
        }
    }

    /// Get all children as a vector
    pub fn all_children(&self) -> Vec<View<'a>> {
        let node = self.props.alloc.get_node(self.current());
        let mut children = Vec::new();
        let mut offset = self.offset();
        for child in &node.children {
            children.push(View {
                props: self.props,
                node: *child,
                offset,
            });
            offset += self.props.alloc.get_node(*child).width;
        }
        children
    }

    /// Get error information if this is an error node
    pub fn error_kind(&self) -> Option<ParsecError> {
        match &self.props.alloc.get_node(self.current()).tag {
            Tag::Error(e) => Some(e.clone()),
            _ => None,
        }
    }
}
