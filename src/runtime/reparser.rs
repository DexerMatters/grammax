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
    config: ReparserConfig,
}

#[derive(Debug, Clone, Copy)]
pub struct ReparserConfig {
    pub enforce_region_end: bool,
    pub enforce_sync_bound: bool,
}

impl Default for ReparserConfig {
    fn default() -> Self {
        Self {
            enforce_region_end: true,
            enforce_sync_bound: true,
        }
    }
}

#[derive(Debug, Clone)]
struct ReparseCandidate {
    node: Rc<RedNode>,
    rule_ix: usize,
    offset: usize,
    old_width: usize,
    level: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct CandidateScore {
    // Prefer candidates whose rule matches the grammar start rule.
    start_rule_mismatch: u8,
    // Hard gates (or penalties when soft).
    region_mismatch: u8,
    sync_cross: u8,
    recovery_state_mismatch: u8,
    // Prefer fewer errors outside the edit range first.
    errors_outside: usize,
    errors_inside: usize,
    // Prefer the smallest matching ancestor among otherwise-equal candidates.
    level: usize,
}

impl Reparser {
    pub fn new(root: RedNode, alloc: TreeAllocRef) -> Self {
        Self {
            current: Rc::new(root),
            alloc,
            config: ReparserConfig::default(),
        }
    }

    pub fn with_config(mut self, config: ReparserConfig) -> Self {
        self.config = config;
        self
    }

    pub fn handle_edit(
        &mut self,
        parser: &mut Parser,
        span: Span,
        new_len: usize,
    ) -> ReparseResult {
        parser.messages.clear();
        self.navigate_to(span);

        let start_state = parser.grammar.analysis.start_state;
        let start_rule_ix = parser.grammar.analysis.states[start_state].ref_ix();
        let delta = new_len as isize - span.len() as isize;

        let specs = parser.recovery_specs().cloned();
        let strategy = parser.recovery_strategy().cloned();

        let best = self.pick_candidate(
            parser,
            span,
            delta,
            start_rule_ix,
            specs.as_ref(),
            strategy.as_ref(),
            true,
        );

        let best = match best {
            Some(best) => Some(best),
            None => self.pick_candidate(
                parser,
                span,
                delta,
                start_rule_ix,
                specs.as_ref(),
                strategy.as_ref(),
                false,
            ),
        };

        if let Some((_score, green, messages, node)) = best {
            self.current = node;
            parser.messages = (*messages).clone();
            return self.finalize_update(parser, green);
        }

        let current_node = self.alloc.get_node(self.current.green);
        if current_node.tag.is_error() {
            let start = self.current.offset;
            drop(current_node);
            parser.messages.clear();
            if let Some(new_green) = parser.parse_rule(start_rule_ix, start) {
                return self.finalize_update(parser, new_green);
            }
            return self.full_reparse(parser);
        }

        drop(current_node);
        self.full_reparse(parser)
    }

    fn pick_candidate(
        &self,
        parser: &mut Parser,
        span: Span,
        delta: isize,
        start_rule_ix: usize,
        specs: Option<&crate::grammar::recovery::RecoverySpecs>,
        strategy: Option<&crate::grammar::recovery::ErrorRecoveryStrategy>,
        hard_gates: bool,
    ) -> Option<(CandidateScore, usize, Arc<ParserMessages>, Rc<RedNode>)> {
        let candidates = self.collect_candidates();
        let mut best: Option<(CandidateScore, usize, Arc<ParserMessages>, Rc<RedNode>)> = None;

        for candidate in candidates {
            parser.messages.clear();

            if let Some(new_green) = parser.parse_rule(candidate.rule_ix, candidate.offset) {
                let new_width = self.alloc.get_node(new_green).width;
                let expected_width = (candidate.old_width as isize + delta) as usize;

                if expected_width != new_width {
                    continue;
                }

                let expected_end = candidate.offset + expected_width;
                let mut region_mismatch = 0u8;
                if let Some(specs) = specs {
                    if let Some(region_end) = specs.region_end_at(span.start) {
                        if expected_end > region_end {
                            if hard_gates && self.config.enforce_region_end {
                                continue;
                            }
                            region_mismatch = 1;
                        }
                    }
                }

                let mut sync_cross = 0u8;
                if let Some(strategy) = strategy {
                    if let Some(sync_point) = strategy.find_sync_point(parser.text(), span.end) {
                        if sync_point >= candidate.offset && expected_end > sync_point {
                            if hard_gates && self.config.enforce_sync_bound {
                                continue;
                            }
                            sync_cross = 1;
                        }
                    }
                }

                let (errors_inside, errors_outside) = Self::count_errors(&parser.messages, span);

                let mut recovery_state_mismatch = 0u8;
                if errors_inside + errors_outside > 0 {
                    if let (Some(strategy), Some(state_id)) = (
                        strategy,
                        parser.grammar.analysis.state_id_for_rule(candidate.rule_ix),
                    ) {
                        if !strategy.can_recover_at(state_id) {
                            recovery_state_mismatch = 1;
                        }
                    }
                }

                let score = CandidateScore {
                    start_rule_mismatch: (candidate.rule_ix != start_rule_ix) as u8,
                    region_mismatch,
                    sync_cross,
                    recovery_state_mismatch,
                    errors_outside,
                    errors_inside,
                    level: candidate.level,
                };

                let should_replace = match &best {
                    None => true,
                    Some((best_score, _, _, _)) => score < *best_score,
                };

                if should_replace {
                    best = Some((
                        score,
                        new_green,
                        Arc::new(parser.messages.clone()),
                        candidate.node,
                    ));
                }

                if parser.messages.is_empty()
                    && candidate.rule_ix == start_rule_ix
                    && region_mismatch == 0
                    && sync_cross == 0
                {
                    return best;
                }
            }
        }

        best
    }

    fn collect_candidates(&self) -> Vec<ReparseCandidate> {
        let mut out = Vec::new();
        let mut level = 0usize;
        let mut node = self.current.clone();

        loop {
            let green_id = node.green;
            let offset = node.offset;
            let green = self.alloc.get_node(green_id);

            if let Tag::Rule { rule_ix } = &green.tag {
                out.push(ReparseCandidate {
                    node: node.clone(),
                    rule_ix: *rule_ix,
                    offset,
                    old_width: green.width,
                    level,
                });
            }

            match &node.parent {
                Some(parent) => {
                    node = Rc::clone(parent);
                    level += 1;
                }
                None => break,
            }
        }

        out
    }

    fn count_errors(messages: &ParserMessages, edit_span: Span) -> (usize, usize) {
        let mut inside = 0usize;
        let mut outside = 0usize;

        for msg in messages {
            let span = msg.span;
            let overlaps = span.start < edit_span.end && span.end > edit_span.start;
            if overlaps {
                inside += 1;
            } else {
                outside += 1;
            }
        }

        (inside, outside)
    }

    fn full_reparse(&mut self, parser: &mut Parser) -> ReparseResult {
        let text = parser.text.clone();
        let result = parser.parse_text(&text);
        self.current = Rc::new(result.root);
        ReparseResult {
            messages: result.messages,
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
