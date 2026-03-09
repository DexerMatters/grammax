use std::vec;

use crate::{
    grammar::Grammar,
    parsec::{
        display::format_ast,
        tree::{GreenId, ParsecError, RedNode, Tag, TreeAllocRef, TreeAllocRefExt},
    },
    utils::LineIndex,
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
    pub(crate) parent: Option<GreenId>,
    pub(crate) sibling_index: usize,
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
            parent: None,
            sibling_index: 0,
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

    pub fn span_bytes(&self) -> (usize, usize) {
        (self.offset(), self.offset() + self.width())
    }

    pub fn start_line_col(&self) -> (usize, usize) {
        let index = LineIndex::new(self.props.source);
        let line_col = index.byte_to_line_col_with_text(self.offset(), self.props.source);
        (line_col.line, line_col.col)
    }

    pub fn end_line_col(&self) -> (usize, usize) {
        let index = LineIndex::new(self.props.source);
        let end_offset = self.offset() + self.width();
        let line_col = index.byte_to_line_col_with_text(end_offset, self.props.source);
        (line_col.line, line_col.col)
    }

    pub fn line_col_range(&self) -> ((usize, usize), (usize, usize)) {
        (self.start_line_col(), self.end_line_col())
    }

    pub fn is_error(&self) -> bool {
        matches!(self.props.alloc.get_node(self.current()).tag, Tag::Error(_))
    }

    pub fn into(self) -> Option<View<'a>> {
        self.into_each().next()
    }

    pub fn next(self) -> Option<View<'a>> {
        let parent_id = self.parent?;
        let parent = self.props.alloc.get_node(parent_id);
        let next_index = self.sibling_index + 1;
        if next_index >= parent.children.len() {
            return None;
        }

        Some(View {
            props: self.props,
            node: parent.children[next_index],
            offset: self.offset() + self.width(),
            parent: Some(parent_id),
            sibling_index: next_index,
        })
    }

    pub fn next_field(self, field_name: &str) -> Option<View<'a>> {
        let mut cursor = if self.sibling_index == 0 {
            Some(self)
        } else {
            self.next()
        };

        while let Some(current) = cursor {
            let node = current.props.alloc.get_node(current.current());
            if matches!(&node.tag, Tag::Field { name, .. } if *name == field_name) {
                return Some(current);
            }
            cursor = current.next();
        }

        None
    }

    fn rule_ix(&self) -> Option<usize> {
        self.props.alloc.get_node(self.current()).tag.rule_ix()
    }

    fn raw_rule_name(&self) -> Option<&'static str> {
        self.rule_ix()
            .and_then(|ix| self.props.grammar.table.rules.get(ix))
            .map(|rule| rule.name)
    }

    fn is_generated_rule_name(rule_name: &str) -> bool {
        rule_name.contains('@')
    }

    fn is_generated_rule(&self) -> bool {
        self.raw_rule_name()
            .map(Self::is_generated_rule_name)
            .unwrap_or(false)
    }

    fn raw_children(&self) -> Vec<View<'a>> {
        let node = self.props.alloc.get_node(self.current());
        let mut children = Vec::new();
        let mut offset = self.offset();
        for (ix, child) in node.children.iter().enumerate() {
            let child_view = View {
                props: self.props,
                node: *child,
                offset,
                parent: Some(self.current()),
                sibling_index: ix,
            };
            offset += self.props.alloc.get_node(*child).width;
            children.push(child_view);
        }
        children
    }

    fn collect_visible_children(&self, candidate: View<'a>, out: &mut Vec<View<'a>>) {
        if candidate.is_generated_rule() {
            let raw_children = candidate.raw_children();
            if raw_children.is_empty() {
                if !candidate.text().trim().is_empty() {
                    out.push(candidate);
                }
                return;
            }

            for child in raw_children {
                self.collect_visible_children(child, out);
            }
            return;
        }

        out.push(candidate);
    }

    pub fn rule_name(&self) -> Option<&'static str> {
        let raw_name = self.raw_rule_name()?;
        if raw_name.starts_with('@') {
            return None;
        }
        raw_name.split('@').next()
    }

    pub fn next_rule(self, rule_name: &str) -> Option<View<'a>> {
        let mut cursor = if self.sibling_index == 0 {
            Some(self)
        } else {
            self.next()
        };

        while let Some(current) = cursor {
            if current.rule_name() == Some(rule_name) {
                return Some(current);
            }
            cursor = current.next();
        }

        None
    }

    pub fn into_each(&self) -> vec::IntoIter<View<'a>> {
        let mut children = Vec::new();
        for child in self.raw_children() {
            self.collect_visible_children(child, &mut children);
        }
        children.into_iter()
    }

    pub fn into_each_field(&self, field_name: &str) -> vec::IntoIter<View<'a>> {
        let node = self.props.alloc.get_node(self.current());
        let mut children = Vec::new();
        let mut offset = self.offset();
        for (ix, child) in node.children.iter().enumerate() {
            let child_node = self.props.alloc.get_node(*child);
            if matches!(&child_node.tag, Tag::Field { name, .. } if *name == field_name) {
                children.push(View {
                    props: self.props,
                    node: *child,
                    offset,
                    parent: Some(self.current()),
                    sibling_index: ix,
                });
            }
            offset += child_node.width;
        }
        children.into_iter()
    }

    pub fn into_each_rule(&self, rule_name: &str) -> vec::IntoIter<View<'a>> {
        let node = self.props.alloc.get_node(self.current());
        let mut children = Vec::new();
        let mut offset = self.offset();
        for (ix, child) in node.children.iter().enumerate() {
            let child_node = self.props.alloc.get_node(*child);
            if child_node.tag.rule_ix().and_then(|ix| {
                self.props
                    .grammar
                    .table
                    .rules
                    .get(ix)
                    .map(|rule| rule.name == rule_name)
            }) == Some(true)
            {
                children.push(View {
                    props: self.props,
                    node: *child,
                    offset,
                    parent: Some(self.current()),
                    sibling_index: ix,
                });
            }
            offset += child_node.width;
        }
        children.into_iter()
    }

    pub fn error_kind(&self) -> Option<ParsecError> {
        match &self.props.alloc.get_node(self.current()).tag {
            Tag::Error(e) => Some(e.clone()),
            _ => None,
        }
    }

    pub fn display(&self) -> String {
        format_ast(
            self.props.grammar,
            &RedNode {
                parent: None,
                green: self.current(),
                offset: self.offset(),
            },
            &self.props.alloc,
            self.props.source,
        )
    }
}
