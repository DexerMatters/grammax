use std::rc::Rc;
use std::sync::Arc;

use crate::{
    grammar::recovery::{ErrorRecoveryStrategy, RecoverySpecs},
    parsec::{
        Parser,
        msg::ParserMessages,
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
    /// Minimum zipper level (depth) required to consider a candidate.
    /// Higher values force narrower (deeper) reparses.
    pub min_level: usize,
}

impl Default for ReparserConfig {
    fn default() -> Self {
        Self {
            enforce_region_end: true,
            enforce_sync_bound: true,
            min_level: 0,
        }
    }
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
    level: std::cmp::Reverse<usize>,
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
        self.ascend_to_root();

        let start_state = parser.grammar.analysis.start_state;
        let start_rule_ix = parser.grammar.analysis.states[start_state].ref_ix();
        let delta = new_len as isize - span.len() as isize;

        let specs = parser.recovery_specs().cloned();
        let strategy = parser.recovery_strategy().cloned();

        let search_span = if span.len() == 0 {
            let root_width = self.alloc.get_node(self.current.green).width;
            if root_width == 0 {
                span
            } else {
                let start = span.start.saturating_sub(1);
                let end = (span.end + 1).min(root_width);
                Span::new(start, end.max(start + 1))
            }
        } else {
            span
        };

        let zippers = collect_affected_zippers(self.current.clone(), search_span, &self.alloc);
        if zippers.is_empty() {
            return self.full_reparse(parser);
        }

        let best = self.pick_candidate(
            parser,
            span,
            delta,
            start_rule_ix,
            specs.as_ref(),
            strategy.as_ref(),
            &zippers,
            true,
        );

        let best = best.or_else(|| {
            self.pick_candidate(
                parser,
                span,
                delta,
                start_rule_ix,
                specs.as_ref(),
                strategy.as_ref(),
                &zippers,
                false,
            )
        });

        if let Some((_score, green, messages, zipper)) = best {
            let (updated_node, root) = zipper.replace_green(&self.alloc, green);
            self.current = root;
            parser.messages = (*messages).clone();
            self.normalize_root(parser);
            return ReparseResult {
                messages: parser.messages.clone(),
                reparsed_tree: updated_node,
            };
        }

        self.full_reparse(parser)
    }

    fn pick_candidate(
        &self,
        parser: &mut Parser,
        span: Span,
        delta: isize,
        start_rule_ix: usize,
        specs: Option<&RecoverySpecs>,
        strategy: Option<&ErrorRecoveryStrategy>,
        zippers: &[Zipper],
        hard_gates: bool,
    ) -> Option<(CandidateScore, usize, Arc<ParserMessages>, Zipper)> {
        let mut best: Option<(CandidateScore, usize, Arc<ParserMessages>, Zipper)> = None;

        for zipper in zippers {
            if zipper.level < self.config.min_level {
                continue;
            }
            parser.messages.clear();

            if let Some(new_green) = parser.parse_rule(zipper.rule_ix, zipper.offset) {
                let new_node = self.alloc.get_node(new_green);
                let new_width = new_node.width;
                let expected_width = (zipper.old_width as isize + delta) as usize;

                // We enforce that the new width matches the expected width (old_width + delta).
                // If it doesn't, it means the node expanded/shrunk in a way that affects
                // the alignment of subsequent siblings, which we cannot handle in a local (zipper) reparse.
                // We must bubble up to the parent to handle the structural change.
                if new_width != expected_width {
                    continue;
                }

                let check_end = zipper.offset + new_width;

                let mut region_mismatch = 0u8;
                if let Some(specs) = specs {
                    if let Some(region_end) = specs.region_end_at(span.start) {
                        if check_end > region_end {
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
                        if sync_point >= zipper.offset && check_end > sync_point {
                            if hard_gates && self.config.enforce_sync_bound {
                                continue;
                            }
                            sync_cross = 1;
                        }
                    }
                }

                let (mut errors_inside, mut errors_outside) =
                    Self::count_errors(&parser.messages, span);

                if self.config.min_level > 0 {
                    errors_inside = 0;
                    errors_outside = 0;
                }

                let mut recovery_state_mismatch = 0u8;
                if errors_inside + errors_outside > 0 {
                    if let (Some(strategy), Some(state_id)) = (
                        strategy,
                        parser.grammar.analysis.state_id_for_rule(zipper.rule_ix),
                    ) {
                        if !strategy.can_recover_at(state_id) {
                            recovery_state_mismatch = 1;
                        }
                    }
                }

                let score = CandidateScore {
                    start_rule_mismatch: if self.config.min_level > 0 {
                        0
                    } else {
                        (zipper.rule_ix != start_rule_ix) as u8
                    },
                    region_mismatch,
                    sync_cross,
                    recovery_state_mismatch,
                    errors_outside,
                    errors_inside,
                    level: std::cmp::Reverse(zipper.level),
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
                        zipper.clone(),
                    ));
                }

                if self.config.min_level == 0
                    && parser.messages.is_empty()
                    && zipper.rule_ix == start_rule_ix
                    && region_mismatch == 0
                    && sync_cross == 0
                {
                    return best;
                }
            }
        }

        best
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

        let start_state = parser.grammar.analysis.start_state;
        let start_rule_ix = parser.grammar.analysis.states[start_state].ref_ix();

        if let Tag::Rule { rule_ix } = &root.tag {
            if *rule_ix != start_rule_ix {
                if let Some(&child) =
                    children
                        .iter()
                        .find(|&&child| match &self.alloc.get_node(child).tag {
                            Tag::Rule { rule_ix } => *rule_ix == start_rule_ix,
                            _ => false,
                        })
                {
                    Rc::make_mut(&mut self.current).green = child;
                    return;
                }
            }
        }

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
}

#[derive(Debug, Clone)]
pub struct ZipperStep {
    pub parent: Rc<RedNode>,
    pub child_idx: usize,
}

#[derive(Debug, Clone)]
pub struct Zipper {
    pub node: Rc<RedNode>,
    pub rule_ix: usize,
    pub offset: usize,
    pub old_width: usize,
    pub level: usize,
    steps: Vec<ZipperStep>,
}

impl Zipper {
    pub fn replace_green(
        &self,
        alloc: &TreeAllocRef,
        new_green: usize,
    ) -> (Rc<RedNode>, Rc<RedNode>) {
        let mut updated_node = self.node.clone();
        Rc::make_mut(&mut updated_node).green = new_green;

        let mut current = updated_node.clone();
        for step in self.steps.iter().rev() {
            let mut parent = step.parent.clone();
            let parent_green = alloc.get_node(parent.green);
            let mut children = parent_green.children.clone();
            children[step.child_idx] = current.green;

            let new_width: usize = children.iter().map(|&c| alloc.get_node(c).width).sum();
            let new_parent_green = alloc.alloc(parent_green.tag.clone(), children, new_width);

            Rc::make_mut(&mut parent).green = new_parent_green;
            current = parent;
        }

        (updated_node, current)
    }
}

pub fn collect_affected_zippers(
    root: Rc<RedNode>,
    span: Span,
    alloc: &TreeAllocRef,
) -> Vec<Zipper> {
    let mut results = Vec::new();
    collect_from(root, span, alloc, &mut Vec::new(), 0, &mut results);
    results
}

fn collect_from(
    node: Rc<RedNode>,
    span: Span,
    alloc: &TreeAllocRef,
    steps: &mut Vec<ZipperStep>,
    level: usize,
    out: &mut Vec<Zipper>,
) {
    let green = alloc.get_node(node.green);
    if let Tag::Rule { rule_ix } = &green.tag {
        out.push(Zipper {
            node: node.clone(),
            rule_ix: *rule_ix,
            offset: node.offset,
            old_width: green.width,
            level,
            steps: steps.clone(),
        });
    }

    let mut offset = node.offset;
    let mut overlaps = Vec::new();

    for (idx, &child_id) in green.children.iter().enumerate() {
        let child = alloc.get_node(child_id);
        let child_start = offset;
        let child_end = offset + child.width;
        offset = child_end;

        if span.end <= child_start {
            break;
        }
        if span.start >= child_end {
            continue;
        }

        overlaps.push((idx, child_id, child_start, child_end));
    }

    if overlaps.len() == 1 {
        let (child_idx, child_id, child_start, child_end) = overlaps[0];
        if span.start >= child_start && span.end <= child_end {
            steps.push(ZipperStep {
                parent: node.clone(),
                child_idx,
            });

            let child_node = Rc::new(RedNode {
                parent: Some(node.clone()),
                offset: child_start,
                green: child_id,
            });
            collect_from(child_node, span, alloc, steps, level + 1, out);
            steps.pop();
            return;
        }
    }
}
