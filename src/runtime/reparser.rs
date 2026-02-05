use std::rc::Rc;
use std::sync::Arc;

use crate::{
    parsec::{
        msg::ParserMessages,
        parser::Parser,
        tree::{ParsecError, RedNode, Tag, TreeAllocRef, TreeAllocRefExt},
    },
    utils::Span,
};

#[derive(Debug, Clone)]
pub struct ReparseResult {
    pub messages: ParserMessages,
    pub reparsed_tree: Rc<RedNode>,
}

pub struct Reparser {
    pub current: Rc<RedNode>,
    alloc: TreeAllocRef,
}

impl Reparser {
    pub fn new(root: RedNode, alloc: TreeAllocRef) -> Self {
        Self {
            current: Rc::new(root),
            alloc,
        }
    }

    pub fn handle_edit(
        &mut self,
        parser: &mut Parser,
        span: Span,
        new_len: usize,
    ) -> ReparseResult {
        parser.messages.clear();
        self.navigate_to(span);

        let delta = new_len as isize - span.len() as isize;
        let mut fallback: Option<(usize, Arc<ParserMessages>, usize)> = None;

        loop {
            // Avoid cloning tag - check directly via reference
            let current_node = self.alloc.get_node(self.current.green);
            if let Tag::Rule { rule_ix } = &current_node.tag {
                let rule_ix = *rule_ix;
                let start = self.current.offset;
                let old_width = current_node.width;
                drop(current_node);

                if let Some(new_green) = parser.parse_rule(rule_ix, start) {
                    let new_width = self.alloc.get_node(new_green).width;
                    let expected_width = (old_width as isize + delta) as usize;

                    if new_width == expected_width {
                        if parser.messages.is_empty() {
                            return self.finalize_update(parser, new_green);
                        }

                        let err_count = parser.messages.len();
                        let should_replace = match &fallback {
                            None => true,
                            Some((_, _, best_errs)) => err_count < *best_errs,
                        };

                        if should_replace {
                            fallback =
                                Some((new_green, Arc::new(parser.messages.clone()), err_count));
                        }
                    }
                }
            } else {
                drop(current_node);
            }

            // Use Rc::clone explicitly to show we're cloning the pointer, not data
            match &self.current.parent {
                Some(parent) => {
                    self.current = Rc::clone(parent);
                }
                None => break,
            }
        }

        if let Some((green, messages, _)) = fallback {
            parser.messages = (*messages).clone();
            return self.finalize_update(parser, green);
        }

        let current_node = self.alloc.get_node(self.current.green);
        if current_node.tag.is_error() {
            let start_state = parser.grammar.analysis.start_state;
            let start_rule_ix = parser.grammar.analysis.states[start_state].ref_ix();
            let start = self.current.offset;
            drop(current_node);
            if let Some(new_green) = parser.parse_rule(start_rule_ix, start) {
                return self.finalize_update(parser, new_green);
            }
        }

        ReparseResult {
            messages: parser.messages.clone(),
            reparsed_tree: self.current.clone(),
        }
    }

    fn finalize_update(&mut self, parser: &mut Parser, new_green: usize) -> ReparseResult {
        let current_mut = Rc::make_mut(&mut self.current);

        // Find which child index this node is in its parent (if it has one)
        let child_idx = if let Some(ref parent) = current_mut.parent {
            let parent_green = self.alloc.get_node(parent.green);
            let relative_offset = current_mut.offset - parent.offset;
            let mut offset = 0;
            let mut idx = None;
            for (i, &c) in parent_green.children.iter().enumerate() {
                if offset == relative_offset {
                    idx = Some(i);
                    break;
                }
                offset += self.alloc.get_node(c).width;
            }
            idx
        } else {
            None
        };

        current_mut.green = new_green;
        Self::fix_tree(current_mut, &self.alloc, child_idx);
        let reparsed_tree = self.current.clone();
        self.ascend_to_root();
        self.normalize_root(parser);
        ReparseResult {
            messages: parser.messages.clone(),
            reparsed_tree,
        }
    }

    fn ascend_to_root(&mut self) {
        while let Some(parent) = &self.current.parent {
            self.current = Rc::clone(parent);
        }
    }
    fn normalize_root(&mut self, parser: &Parser) {
        if self.current.parent.is_some() {
            return;
        }

        let root_green = self.current.green;
        let root = self.alloc.get_node(root_green);

        // Check for placeholder root without cloning
        let is_placeholder_root = matches!(
            &root.tag,
            Tag::Error(errors)
                if errors.iter().any(|e| matches!(e, ParsecError::Placeholder))
        );

        let children = root.children.clone();

        if children.len() == 1 {
            let child = children[0];
            let child_node = self.alloc.get_node(child);

            // Check for duplicate rule without cloning tags
            let is_duplicate_rule = match (&root.tag, &child_node.tag) {
                (Tag::Rule { rule_ix: a }, Tag::Rule { rule_ix: b }) => a == b,
                _ => false,
            };

            if is_placeholder_root || is_duplicate_rule {
                Rc::make_mut(&mut self.current).green = child;
            }
            return;
        }

        if !is_placeholder_root {
            return;
        }

        if children.len() > 1 {
            let start_state = parser.grammar.analysis.start_state;
            let start_rule_ix = parser.grammar.analysis.states[start_state].ref_ix();
            let valid_child =
                children
                    .iter()
                    .find(|&&child| match &self.alloc.get_node(child).tag {
                        Tag::Rule { rule_ix } => *rule_ix == start_rule_ix,
                        _ => false,
                    });

            if let Some(&child) = valid_child {
                Rc::make_mut(&mut self.current).green = child;
            }
        }
    }

    fn fix_tree(node: &mut RedNode, alloc: &TreeAllocRef, child_idx: Option<usize>) {
        if let Some(ref mut parent_arc) = node.parent {
            let parent_green_id = parent_arc.green;
            let parent_green = alloc.get_node(parent_green_id);

            // Use provided child_idx if available, otherwise search (fallback)
            let idx = child_idx.or_else(|| {
                let relative_offset = node.offset - parent_arc.offset;
                let mut offset = 0;
                for (i, &c) in parent_green.children.iter().enumerate() {
                    if offset == relative_offset {
                        return Some(i);
                    }
                    offset += alloc.get_node(c).width;
                }
                None
            });

            if let Some(idx) = idx {
                let mut children = parent_green.children.clone();
                children[idx] = node.green;

                let new_width: usize = children.iter().map(|&c| alloc.get_node(c).width).sum();
                let new_parent_green = alloc.alloc(parent_green.tag.clone(), children, new_width);

                let parent_mut = Rc::make_mut(parent_arc);
                parent_mut.green = new_parent_green;

                Self::fix_tree(parent_mut, alloc, None);
            }
        }
    }

    pub fn navigate_to(&mut self, span: Span) {
        // Step 1: Ascend until we find a node that fully contains the span
        loop {
            let (start, end) = {
                let node = self.alloc.get_node(self.current.green);
                (self.current.offset, self.current.offset + node.width)
            };

            if span.start >= start && span.end <= end {
                break;
            }

            match &self.current.parent {
                Some(parent) => {
                    self.current = Rc::clone(parent);
                }
                None => return,
            }
        }

        // Step 2: Descend to the smallest child that contains the span
        'descend: loop {
            let current_green = self.alloc.get_node(self.current.green);

            let mut offset = self.current.offset;
            for &child_id in &current_green.children {
                let child = self.alloc.get_node(child_id);
                let width = child.width;
                let end = offset + width;

                // Case 1: Span is completely to the right of this child
                if span.start >= end {
                    offset = end;
                    continue;
                }

                // Case 2: Span is fully contained in this child
                if span.end <= end {
                    drop(child);
                    drop(current_green);

                    let parent = self.current.clone();

                    self.current = Rc::new(RedNode {
                        parent: Some(parent),
                        offset,
                        green: child_id,
                    });
                    continue 'descend;
                }

                // Case 3: Span overlaps/splits (starts here, ends later) -> Stop
                return;
            }
            break;
        }
    }
}
