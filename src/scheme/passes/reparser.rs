use std::{rc::Rc, sync::Arc};

use rustc_hash::{FxHashMap, FxHashSet};

use crate::{
    parsec::{
        Parser,
        msg::ParserMessages,
        tree::{ParsecError, RedNode, Tag, TreeAllocRef, TreeAllocRefExt},
        view::{NodeView, Viewer},
    },
    scheme::{
        Span, URI,
        layers::{ParseTreeIR, cst::NodePath},
        passes::{
            delta,
            metrics::EditMetrics,
            strategy::{
                CandidateScore, EditKind, StrategyCandidate, StrategyContext, count_errors,
                pick_candidate,
            },
        },
    },
};

pub type Command = crate::scheme::LayerCommand<ParseTreeIR>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReparseError {
    NoIncrementalCandidate {
        span: Span,
        delta: isize,
        candidates_collected: usize,
    },
    OutOfBounds {
        span: Span,
        text_len: usize,
    },
    InvalidSpan {
        span: Span,
    },
}

pub struct Reparser {
    pub(crate) current: Rc<RedNode>,
    pub(crate) parser: Parser,
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

impl Reparser {
    pub(crate) fn init(root: RedNode, alloc: TreeAllocRef, parser: Parser) -> Self {
        Self {
            current: Rc::new(root),
            alloc,
            config: ReparserConfig::default(),
            parser,
        }
    }

    pub(crate) fn set_config(&mut self, config: ReparserConfig) {
        self.config = config;
    }

    pub fn from_parser(mut parser: Parser) -> Self {
        let alloc = parser.alloc.clone();
        let crate::parsec::Result { root, .. } = parser.parse_text("");
        Self::init(root, alloc, parser)
    }

    pub fn current_view(&self) -> NodeView {
        NodeView::from_specs(
            self.parser.grammar,
            self.alloc.clone(),
            self.parser.text(),
            self.current.green,
            self.current.offset,
        )
    }

    pub fn current_viewer(&self) -> Viewer {
        Viewer::new(self.parser.grammar, self.alloc.clone(), self.parser.text())
    }

    pub fn current_messages(&self) -> &ParserMessages {
        &self.parser.messages
    }

    pub fn current_text(&self) -> &str {
        self.parser.text()
    }

    pub fn replace(
        &mut self,
        start: usize,
        end: usize,
        text: &str,
    ) -> Result<Vec<Command>, ReparseError> {
        let old_text = self.parser.text();
        if start > end {
            return Err(ReparseError::InvalidSpan {
                span: Span::new(start, end),
            });
        }
        if end > old_text.len() {
            return Err(ReparseError::OutOfBounds {
                span: Span::new(start, end),
                text_len: old_text.len(),
            });
        }
        let mut new_source = String::with_capacity(old_text.len() - (end - start) + text.len());
        new_source.push_str(&old_text[..start]);
        new_source.push_str(text);
        new_source.push_str(&old_text[end..]);

        let span = Span::new(start, end);
        let new_len = text.len();

        self.handle_edit(&URI::default(), span, new_len, &new_source, None)
    }

    pub fn insert(&mut self, offset: usize, text: &str) -> Result<Vec<Command>, ReparseError> {
        self.replace(offset, offset, text)
    }

    pub fn delete(&mut self, start: usize, end: usize) -> Result<Vec<Command>, ReparseError> {
        self.replace(start, end, "")
    }

    fn get_context(&mut self, span: Span) -> (Rc<RedNode>, Vec<ZipperStep>, usize) {
        // 1. Ascend to enclose span
        while let Some(parent) = &self.current.parent {
            let width = self.alloc.get_node(self.current.green).width;
            if self.current.offset <= span.start && self.current.offset + width >= span.end {
                break;
            }
            self.current = Rc::clone(parent);
        }

        let focus = self.current.clone();
        let mut steps = Vec::new();
        let mut level = 0;
        let mut temp = focus.clone();

        while let Some(parent) = &temp.parent {
            let parent_id = parent.green;
            let parent_node = self.alloc.get_node(parent_id);
            let mut offset = parent.offset;
            let mut found_idx = None;

            for (idx, &child_id) in parent_node.children.iter().enumerate() {
                if offset == temp.offset && child_id == temp.green {
                    found_idx = Some(idx);
                    break;
                }
                offset += self.alloc.get_node(child_id).width;
            }

            steps.push(ZipperStep {
                parent: Rc::clone(parent),
                child_idx: found_idx.unwrap_or(0),
            });
            temp = Rc::clone(parent);
            level += 1;
        }

        steps.reverse();
        (focus, steps, level)
    }

    pub(crate) fn handle_edit(
        &mut self,
        uri: &URI,
        span: Span,
        new_len: usize,
        source_text: &str,
        mut metrics: Option<&mut EditMetrics>,
    ) -> Result<Vec<Command>, ReparseError> {
        let total_start = if metrics.is_some() {
            Some(std::time::Instant::now())
        } else {
            None
        };

        if span.len() == 0 && new_len == 0 {
            if let Some(m) = &mut metrics {
                if let Some(start) = total_start {
                    m.total_duration_us = start.elapsed().as_micros();
                }
                m.used_incremental_path = false;
            }
            return Ok(Vec::new());
        }

        let old_messages = self.parser.messages.clone();
        let previous_source_text = self.parser.text().to_string();
        self.parser.messages.clear();
        self.parser.set_text(source_text);
        let (focus_node, mut steps, level) = self.get_context(span);
        let delta = new_len as isize - span.len() as isize;

        let specs = self.parser.recovery_specs().cloned();
        let strategy = self.parser.recovery_strategy().cloned();

        let search_span = span;
        let edit_span = if span.len() == 0 {
            Span::new(span.start, span.start + new_len)
        } else {
            span
        };

        let zipper_start = if metrics.is_some() {
            Some(std::time::Instant::now())
        } else {
            None
        };

        let mut zippers = Vec::new();
        collect_from(
            focus_node,
            search_span,
            &self.alloc,
            &mut steps,
            level,
            &mut zippers,
            &self.parser,
        );
        dedupe_zippers(&mut zippers);

        if let Some(m) = &mut metrics {
            m.candidates_collected = zippers.len();
            if let Some(start) = zipper_start {
                m.zipper_collection_us = start.elapsed().as_micros();
            }
        }

        if zippers.is_empty() {
            // Fallback: If focus node based seRch failed (rare), try from root
            self.ascend_to_root();
            zippers = collect_affected_zippers(
                self.current.clone(),
                search_span,
                &self.alloc,
                &self.parser,
            );
            dedupe_zippers(&mut zippers);

            if let Some(m) = &mut metrics {
                m.candidates_collected = zippers.len();
            }
        }

        if zippers.is_empty() {
            if let Some(root_candidate) = self.try_root_rule_candidate(source_text) {
                let result = self.apply_candidate(
                    root_candidate,
                    &old_messages,
                    delta,
                    &previous_source_text,
                    source_text,
                    uri,
                    metrics.as_deref_mut(),
                );
                if let Some(m) = &mut metrics {
                    m.used_incremental_path = true;
                    m.fell_back_to_full_diff = false;
                    if m.message.is_empty() {
                        m.message =
                            "used root-level incremental candidate after zipper collection found none"
                                .to_string();
                    }
                    if let Some(start) = total_start {
                        m.total_duration_us = start.elapsed().as_micros();
                    }
                }
                return Ok(result);
            }
            if let Some(m) = &mut metrics {
                if let Some(start) = total_start {
                    m.total_duration_us = start.elapsed().as_micros();
                }
                m.used_incremental_path = false;
            }
            return Err(ReparseError::NoIncrementalCandidate {
                span,
                delta,
                candidates_collected: 0,
            });
        }

        let reuse_before = self.parser.reuse_stats();

        let mut ctx = StrategyContext {
            parser: &mut self.parser,
            span,
            delta,
            specs: specs.as_ref(),
            recovery_strategy: strategy.as_ref(),
            zippers: &zippers,
            config: self.config,
            metrics: metrics.as_deref_mut(),
        };

        let kind = if delta > 0 {
            EditKind::Insertion
        } else if delta < 0 {
            EditKind::Deletion
        } else {
            EditKind::Update
        };

        let mut root_candidate_cache: Option<Option<StrategyCandidate>> = None;
        let mut best = pick_candidate(&mut ctx, edit_span, kind);
        if best.is_none() && (self.config.enforce_sync_bound || self.config.enforce_region_end) {
            ctx.config = ReparserConfig {
                enforce_sync_bound: false,
                enforce_region_end: false,
                min_level: self.config.min_level,
            };
            if let Some(m) = ctx.metrics.as_deref_mut() {
                m.message = "strict candidate filters rejected all zippers; retried with relaxed incremental bounds".to_string();
            }
            best = pick_candidate(&mut ctx, edit_span, kind);
        }

        if best.is_none() {
            self.ascend_to_root();
            let root_span = Span::new(0, source_text.len());
            let root_zippers = collect_affected_zippers(
                self.current.clone(),
                root_span,
                &self.alloc,
                &self.parser,
            );
            let mut root_zippers = root_zippers;
            dedupe_zippers(&mut root_zippers);

            if !root_zippers.is_empty() {
                let mut root_ctx = StrategyContext {
                    parser: &mut self.parser,
                    span,
                    delta,
                    specs: specs.as_ref(),
                    recovery_strategy: strategy.as_ref(),
                    zippers: &root_zippers,
                    config: ReparserConfig {
                        enforce_sync_bound: false,
                        enforce_region_end: false,
                        min_level: 0,
                    },
                    metrics: metrics.as_deref_mut(),
                };

                best = pick_candidate(&mut root_ctx, edit_span, kind);

                if best.is_some() {
                    if let Some(m) = metrics.as_deref_mut() {
                        m.message =
                            "primary incremental candidates failed; using relaxed root-level incremental candidate"
                                .to_string();
                    }
                }
            }
        }

        if best.is_none() {
            best = get_cached_root_candidate(&mut root_candidate_cache, self, source_text);
            if best.is_some() {
                if let Some(m) = metrics.as_deref_mut() {
                    m.message =
                        "primary incremental candidates failed; using direct start-rule incremental parse"
                            .to_string();
                }
            }
        }

        // If the best zipper candidate still has internal errors or parse messages,
        // attempt a full root-rule reparse and prefer it when it produces a cleaner result.
        if let Some(ref candidate) = best {
            let candidate_is_errorful =
                !candidate.score.is_error_free() || !candidate.messages.is_empty();
            if candidate_is_errorful {
                if let Some(root_candidate) =
                    get_cached_root_candidate(&mut root_candidate_cache, self, source_text)
                {
                    // Always prefer root when the zipper green was an Incomplete node (parse_rule
                    // gave up entirely), or when root has fewer errors outside the edit span.
                    //
                    // Using errors_outside rather than total message count is critical:
                    // sub-region recovery may bundle several characters into a single large error
                    // token (1 message) while the root parse emits two precise, in-span messages.
                    // Message count would incorrectly favour the bundled result; comparing how
                    // far errors spill outside the edit boundary is the reliable signal.
                    let zipper_green_is_incomplete = matches!(
                        self.alloc.get_node(candidate.green).tag,
                        crate::parsec::tree::Tag::Error(
                            crate::parsec::tree::ParsecError::Incomplete
                        )
                    );
                    let (root_inside, root_outside) =
                        count_errors(&root_candidate.messages, edit_span);
                    let root_is_cleaner = zipper_green_is_incomplete
                        || root_candidate.messages.is_empty()
                        // Candidate spills more errors outside the edit span than root does.
                        // The `root_outside == 0` guard used previously was too strict: when a
                        // prior edit left a residual error just outside the new edit_span,
                        // root_outside becomes 1, blocking this branch even when the candidate
                        // has 2+ structural errors outside (e.g. missing `}` + missing EOF).
                        // Comparing relative spillover fixes both cases:
                        //   • candidate pollutes more than root   → prefer root
                        //   • same spillover, root has fewer inside → prefer root
                        || candidate.score.errors_outside > root_outside
                        || (candidate.score.errors_outside == root_outside
                            && root_inside < candidate.score.errors_inside);
                    if root_is_cleaner {
                        if let Some(m) = metrics.as_deref_mut() {
                            m.message =
                                "incremental candidate had errors; replaced with cleaner root reparse"
                                    .to_string();
                        }
                        best = Some(root_candidate);
                    }
                }
            }
        }

        let reuse_after = self.parser.reuse_stats();
        if let Some(m) = &mut metrics {
            m.parse_rule_calls = reuse_after.lookups.saturating_sub(reuse_before.lookups);
            m.parse_rule_cache_hits = reuse_after.hits.saturating_sub(reuse_before.hits);
        }

        if let Some(candidate) = best {
            let result = self.apply_candidate(
                candidate,
                &old_messages,
                delta,
                &previous_source_text,
                source_text,
                uri,
                metrics.as_deref_mut(),
            );
            if let Some(m) = &mut metrics {
                if let Some(start) = total_start {
                    m.total_duration_us = start.elapsed().as_micros();
                }
                m.used_incremental_path = true;
            }
            return Ok(result);
        }

        if let Some(m) = &mut metrics {
            if let Some(start) = total_start {
                m.total_duration_us = start.elapsed().as_micros();
            }
            m.used_incremental_path = false;
        }
        Err(ReparseError::NoIncrementalCandidate {
            span,
            delta,
            candidates_collected: zippers.len(),
        })
    }

    fn try_root_rule_candidate(&mut self, source_text: &str) -> Option<StrategyCandidate> {
        self.ascend_to_root();

        let root_node = self.current.clone();
        let old_width = self.alloc.get_node(root_node.green).width;
        let start_rule = self.parser.grammar.table.start_rule;
        let expected_width = source_text.len();

        self.parser.messages.clear();
        self.parser.set_insert_pos(None);

        let mut green = self.parser.parse_rule(start_rule, 0, expected_width);
        let needs_recovery = match green {
            Some(g) if self.alloc.get_node(g).width == expected_width => {
                matches!(self.alloc.get_node(g).tag, Tag::Error(_))
                    || !self.parser.messages.is_empty()
            }
            _ => true,
        };

        if needs_recovery {
            self.parser.clear_reuse_cache();
            self.parser.messages.clear();
            self.parser.set_insert_pos(None);
            green = self.parser.parse_rule(start_rule, 0, expected_width);
        }

        let needs_full_recovery = match green {
            Some(g) if self.alloc.get_node(g).width == expected_width => {
                matches!(self.alloc.get_node(g).tag, Tag::Error(_))
                    || !self.parser.messages.is_empty()
            }
            _ => true,
        };

        let green = if needs_full_recovery {
            self.parser.messages.clear();
            self.parser.set_insert_pos(None);
            self.parser.parse_text(source_text).root.green
        } else {
            green?
        };

        Some(StrategyCandidate {
            score: CandidateScore::new(0, 0, 0),
            green,
            messages: Arc::new(self.parser.messages.clone()),
            zipper: Zipper {
                node: root_node,
                rule_ix: start_rule,
                offset: 0,
                old_width,
                level: 0,
                steps: Vec::new(),
            },
        })
    }

    fn apply_candidate(
        &mut self,
        candidate: StrategyCandidate,
        old_messages: &ParserMessages,
        delta: isize,
        old_source_text: &str,
        new_source_text: &str,
        uri: &URI,
        mut metrics: Option<&mut EditMetrics>,
    ) -> Vec<Command> {
        if let Some(m) = &mut metrics {
            m.used_incremental_path = true;
        }
        let replaced = candidate.zipper.replace_green(&self.alloc, candidate.green);
        self.current = replaced.root.clone();

        // Merge messages: keep old messages outside the replaced range (shifted if needed)
        // and add new messages from the candidate.
        let replaced_start = candidate.zipper.offset;
        let replaced_end = replaced_start + candidate.zipper.old_width;
        let is_point_replace = replaced_start == replaced_end;
        let mut new_messages = Vec::with_capacity(old_messages.len() + candidate.messages.len());
        let mut seen = FxHashSet::default();

        for msg in old_messages {
            let is_before = if is_point_replace {
                msg.span.end < replaced_start
            } else {
                msg.span.end <= replaced_start
            };
            let is_after = if is_point_replace {
                msg.span.start > replaced_end
            } else {
                msg.span.start >= replaced_end
            };

            if is_before {
                if seen.insert(msg.clone()) {
                    new_messages.push(msg.clone());
                }
            } else if is_after {
                let mut shifted = msg.clone();
                let start = (msg.span.start as isize + delta).max(0) as usize;
                let end = (msg.span.end as isize + delta).max(0) as usize;
                shifted.span = Span::new(start, end);
                if seen.insert(shifted.clone()) {
                    new_messages.push(shifted);
                }
            }
        }
        for msg in candidate.messages.iter().cloned() {
            if seen.insert(msg.clone()) {
                new_messages.push(msg);
            }
        }
        new_messages.sort_by_key(|m| m.span.start);

        self.parser.messages = new_messages;
        self.normalize_root();
        let semantic_start = if metrics.is_some() {
            Some(std::time::Instant::now())
        } else {
            None
        };

        // Skip zipper steps where the parent is a transparent Field wrapper,
        // because single-child Field nodes are not emitted as separate nodes in
        // the command stream (they are merged into their child's field attribute).
        let path = NodePath(
            candidate
                .zipper
                .steps
                .iter()
                .filter(|s| !matches!(self.alloc.get_node(s.parent.green).tag, Tag::Field { .. }))
                .map(|s| s.child_idx)
                .collect(),
        );
        let semantic_commands = delta::generate_commands_incremental(
            &self.alloc,
            uri,
            &path,
            candidate.zipper.node.green,
            candidate.green,
            candidate.zipper.offset,
            candidate.zipper.offset,
            old_source_text,
            new_source_text,
            self.current.parent.is_none(),
        );

        if let Some(m) = &mut metrics {
            m.semantic_commands_emitted = semantic_commands.len();
            if let Some(start) = semantic_start {
                m.semantic_diff_us = start.elapsed().as_micros();
            }
        }

        semantic_commands
    }

    fn ascend_to_root(&mut self) {
        while let Some(parent) = &self.current.parent {
            self.current = Rc::clone(parent);
        }
    }

    fn normalize_root(&mut self) {
        if self.current.parent.is_some() {
            return;
        }

        let root_green = self.current.green;
        let root = self.alloc.get_node(root_green);

        // Check for placeholder root without cloning
        let is_placeholder_root = matches!(&root.tag, Tag::Error(ParsecError::Placeholder));

        let children = root.children.clone();

        let start_rule_ix = self.parser.grammar.table.start_rule;

        if let Tag::Rule { rule_ix, .. } = &root.tag {
            if *rule_ix != start_rule_ix {
                if let Some(&child) =
                    children
                        .iter()
                        .find(|&&child| match &self.alloc.get_node(child).tag {
                            Tag::Rule { rule_ix, .. } => *rule_ix == start_rule_ix,
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
                (Tag::Rule { rule_ix: a, .. }, Tag::Rule { rule_ix: b, .. }) => a == b,
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
                        Tag::Rule { rule_ix, .. } => *rule_ix == start_rule_ix,
                        _ => false,
                    });

            if let Some(&child) = valid_child {
                Rc::make_mut(&mut self.current).green = child;
            }
        }
    }
}

fn dedupe_zippers(zippers: &mut Vec<Zipper>) {
    let mut best_by_key: FxHashMap<(usize, usize, usize), Zipper> = FxHashMap::default();
    for zipper in zippers.drain(..) {
        let key = (zipper.rule_ix, zipper.offset, zipper.old_width);
        match best_by_key.get(&key) {
            Some(existing) if existing.level >= zipper.level => {}
            _ => {
                best_by_key.insert(key, zipper);
            }
        }
    }
    *zippers = best_by_key.into_values().collect();
    zippers.sort_by_key(|z| (z.offset, z.old_width, z.level));
}

fn get_cached_root_candidate(
    cache: &mut Option<Option<StrategyCandidate>>,
    reparser: &mut Reparser,
    source_text: &str,
) -> Option<StrategyCandidate> {
    if cache.is_none() {
        *cache = Some(reparser.try_root_rule_candidate(source_text));
    }
    cache.as_ref().and_then(|candidate| candidate.clone())
}

#[derive(Debug, Clone)]
pub(crate) struct ZipperStep {
    pub parent: Rc<RedNode>,
    pub child_idx: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct Zipper {
    pub node: Rc<RedNode>,
    pub rule_ix: usize,
    pub offset: usize,
    pub old_width: usize,
    pub level: usize,
    pub steps: Vec<ZipperStep>,
}

impl Zipper {
    pub fn replace_green(&self, alloc: &TreeAllocRef, new_green: usize) -> ReplaceResult {
        if self.steps.is_empty() {
            let updated_root = Rc::new(RedNode {
                parent: None,
                offset: self.offset,
                green: new_green,
            });
            return ReplaceResult { root: updated_root };
        }

        let mut ancestor_greens = vec![0usize; self.steps.len()];
        let mut child_green = new_green;

        for (ix, step) in self.steps.iter().enumerate().rev() {
            let (parent_tag, mut children) = {
                let parent_green = alloc.get_node(step.parent.green);
                (parent_green.tag.clone(), parent_green.children.clone())
            };
            children[step.child_idx] = child_green;
            let new_width: usize = children.iter().map(|&c| alloc.get_node(c).width).sum();
            let new_parent_green = alloc.alloc(parent_tag, children, new_width);
            ancestor_greens[ix] = new_parent_green;
            child_green = new_parent_green;
        }

        // Only the new root RedNode is needed; the intermediate ancestor chain
        // is not used by callers, so we do not construct it.
        let root = Rc::new(RedNode {
            parent: None,
            offset: self.steps[0].parent.offset,
            green: ancestor_greens[0],
        });

        ReplaceResult { root }
    }
}

pub struct ReplaceResult {
    root: Rc<RedNode>,
}

fn collect_affected_zippers(
    root: Rc<RedNode>,
    span: Span,
    alloc: &TreeAllocRef,
    parser: &Parser,
) -> Vec<Zipper> {
    let mut results = Vec::new();
    collect_from(root, span, alloc, &mut Vec::new(), 0, &mut results, parser);

    // Special handling for insertions in sep-based lists:
    // For insertions, prefer @sep_tail over @sep when available
    // At end-of-list (insertion == sep.offset + sep.width), use the deeper @sep_tail
    // Otherwise, use shallower @sep for broader context
    if span.len() == 0 && results.len() > 1 {
        let sep_rules: Vec<usize> = (0..results.len())
            .filter(|&idx| {
                let zipper = &results[idx];
                let rule_name = parser.grammar.name(zipper.rule_ix);
                rule_name == "@sep"
                    || rule_name.ends_with("@sep")
                    || rule_name == "@sep_tail"
                    || rule_name.ends_with("_tail")
            })
            .collect();

        if !sep_rules.is_empty() {
            // Check if we're inserting at end-of-list
            let at_end_of_any_sep = sep_rules.iter().any(|&idx| {
                let zipper = &results[idx];
                let zipper_end = zipper.offset + zipper.old_width;
                span.start == zipper_end
            });

            // Select best rule
            let best_idx = if at_end_of_any_sep {
                // At end: prefer deepest @sep_tail
                sep_rules
                    .into_iter()
                    .max_by_key(|&idx| {
                        let rule_name = parser.grammar.name(results[idx].rule_ix);
                        let is_tail = rule_name == "@sep_tail" || rule_name.ends_with("_tail");
                        if is_tail {
                            (1, results[idx].level) // Tails first, then by depth
                        } else {
                            (0, 0) // Non-tails last
                        }
                    })
                    .unwrap()
            } else {
                // Not at end: prefer shallower @sep
                sep_rules
                    .into_iter()
                    .min_by_key(|&idx| results[idx].level)
                    .unwrap()
            };

            results = vec![results[best_idx].clone()];
        }
    }

    results
}

fn collect_from(
    node: Rc<RedNode>,
    span: Span,
    alloc: &TreeAllocRef,
    steps: &mut Vec<ZipperStep>,
    level: usize,
    out: &mut Vec<Zipper>,
    parser: &Parser,
) {
    let mut stack = vec![(node, steps.clone(), level)];

    while let Some((node, steps, level)) = stack.pop() {
        let green = alloc.get_node(node.green);

        if let Tag::Rule {
            reparse_rule_ix, ..
        } = &green.tag
        {
            out.push(Zipper {
                node: node.clone(),
                rule_ix: *reparse_rule_ix,
                offset: node.offset,
                old_width: green.width,
                level,
                steps: steps.clone(),
            });
        }

        let mut offset = node.offset;
        let mut overlaps = Vec::new();
        let is_insertion = span.len() == 0;

        let mut has_separator_children = false;
        let mut separator_index = vec![false; green.children.len()];

        for (idx, &child_id) in green.children.iter().enumerate() {
            let child = alloc.get_node(child_id);
            if matches!(&child.tag, Tag::Token { .. }) && child.width <= 2 {
                has_separator_children = true;
                separator_index[idx] = true;
            }
        }

        for (idx, &child_id) in green.children.iter().enumerate() {
            let child = alloc.get_node(child_id);
            let child_start = offset;
            let child_end = offset + child.width;
            offset = child_end;

            if !is_insertion {
                if span.end <= child_start || span.start >= child_end {
                    continue;
                }
                overlaps.push((idx, child_id, child_start, child_end));
                continue;
            }

            let insertion_is_inside = span.start > child_start && span.start < child_end;
            let insertion_at_start = span.start == child_start;
            let insertion_at_end = span.start == child_end;

            if insertion_is_inside || insertion_at_start || insertion_at_end {
                overlaps.push((idx, child_id, child_start, child_end));

                if has_separator_children && (insertion_at_end || insertion_is_inside) {
                    if idx + 1 < green.children.len() && separator_index[idx + 1] {
                        overlaps.push((
                            idx + 1,
                            green.children[idx + 1],
                            child_end,
                            child_end + alloc.get_node(green.children[idx + 1]).width,
                        ));
                        if idx + 2 < green.children.len() {
                            let next_child_id = green.children[idx + 2];
                            let next_child_start =
                                child_end + alloc.get_node(green.children[idx + 1]).width;
                            let next_child_end =
                                next_child_start + alloc.get_node(next_child_id).width;
                            overlaps.push((
                                idx + 2,
                                next_child_id,
                                next_child_start,
                                next_child_end,
                            ));
                        }
                    }
                }
            }
        }

        if overlaps.len() != 1 && !(is_insertion && overlaps.len() > 1) {
            continue;
        }

        let (child_idx, child_id, child_start, child_end) = if overlaps.len() == 1 {
            overlaps[0]
        } else {
            let mut preferred = None;

            for candidate in &overlaps {
                let n = alloc.get_node(candidate.1);
                if n.width == 0 && !matches!(n.tag, Tag::Error(_)) {
                    preferred = Some(*candidate);
                    break;
                }
            }

            if preferred.is_none() {
                for candidate in &overlaps {
                    let (_, _, cstart, cend) = *candidate;
                    if span.start > cstart && span.start < cend {
                        preferred = Some(*candidate);
                        break;
                    }
                }
            }

            if preferred.is_none() && is_insertion && has_separator_children {
                for candidate in &overlaps {
                    let (idx, _child_id, _cstart, cend) = *candidate;
                    let is_after_insertion_end = cend == span.start;
                    let next_is_separator =
                        idx + 1 < green.children.len() && separator_index[idx + 1];
                    if is_after_insertion_end
                        && !next_is_separator
                        && idx + 1 < green.children.len()
                    {
                        let next_idx = idx + 1;
                        let next_sep_id = green.children[next_idx];
                        let next_sep = alloc.get_node(next_sep_id);
                        if matches!(&next_sep.tag, Tag::Token { .. })
                            && idx + 2 < green.children.len()
                        {
                            let sep_width = next_sep.width;
                            let following_id = green.children[idx + 2];
                            let following_start = cend + sep_width;
                            preferred = Some((
                                idx + 2,
                                following_id,
                                following_start,
                                following_start + alloc.get_node(following_id).width,
                            ));
                            break;
                        }
                    }
                }
            }

            if preferred.is_none() {
                for candidate in &overlaps {
                    let (_, child_id, cstart, _) = *candidate;
                    let child = alloc.get_node(child_id);
                    let is_rule = matches!(&child.tag, Tag::Rule { .. });
                    if span.start == cstart && is_rule && child.width > 0 {
                        preferred = Some(*candidate);
                        break;
                    }
                }
            }

            if preferred.is_none() {
                for candidate in &overlaps {
                    let (_, _, _, cend) = *candidate;
                    let child = alloc.get_node(candidate.1);
                    if span.start == cend && child.width > 0 {
                        let is_rule = matches!(&child.tag, Tag::Rule { .. });
                        let current_preferred_is_token = preferred
                            .map(|pref| matches!(&alloc.get_node(pref.1).tag, Tag::Token { .. }))
                            .unwrap_or(true);
                        if preferred.is_none() || (is_rule && current_preferred_is_token) {
                            preferred = Some(*candidate);
                        }
                    }
                }
            }

            preferred.unwrap_or_else(|| *overlaps.last().unwrap())
        };

        let child = alloc.get_node(child_id);
        if is_insertion && child.width == 0 {
            continue;
        }

        let can_descend = if is_insertion {
            span.start >= child_start && span.start <= child_end
        } else {
            span.start >= child_start && span.end <= child_end
        };

        if !can_descend {
            continue;
        }

        let should_stop_at_separator = is_insertion
            && has_separator_children
            && child_idx > 0
            && separator_index[child_idx - 1];

        let mut child_steps = steps.clone();
        child_steps.push(ZipperStep {
            parent: node.clone(),
            child_idx,
        });

        let child_node = Rc::new(RedNode {
            parent: Some(node.clone()),
            offset: child_start,
            green: child_id,
        });

        if should_stop_at_separator {
            if let Tag::Rule {
                reparse_rule_ix: child_reparse_rule_ix,
                ..
            } = &child.tag
            {
                out.push(Zipper {
                    node: child_node.clone(),
                    rule_ix: *child_reparse_rule_ix,
                    offset: child_start,
                    old_width: child.width,
                    level: level + 1,
                    steps: child_steps.clone(),
                });
            }
        }

        if let Tag::Rule { rule_ix, .. } = &green.tag {
            let rule_name = parser.grammar.name(*rule_ix);
            if is_insertion && (rule_name == "@sep" || rule_name.ends_with("@sep")) {
                for (remaining_idx, &remaining_id) in green.children.iter().enumerate().rev() {
                    if remaining_idx <= child_idx {
                        continue;
                    }

                    let remaining_child = alloc.get_node(remaining_id);
                    if let Tag::Rule {
                        rule_ix: remaining_rule,
                        ..
                    } = &remaining_child.tag
                    {
                        let remaining_name = parser.grammar.name(*remaining_rule);
                        if remaining_name == "@sep_tail" || remaining_name.ends_with("_tail") {
                            let mut remaining_start = node.offset;
                            for &prior_id in green.children.iter().take(remaining_idx) {
                                remaining_start += alloc.get_node(prior_id).width;
                            }
                            let mut tail_steps = steps.clone();
                            tail_steps.push(ZipperStep {
                                parent: node.clone(),
                                child_idx: remaining_idx,
                            });

                            let tail_node = Rc::new(RedNode {
                                parent: Some(node.clone()),
                                offset: remaining_start,
                                green: remaining_id,
                            });

                            stack.push((tail_node, tail_steps, level + 1));
                        }
                    }
                }
            }
        }

        stack.push((child_node, child_steps, level + 1));
    }
}
