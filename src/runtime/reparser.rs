use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use crate::{
    parsec::{
        Parser,
        msg::ParserMessages,
        tree::{ParsecError, RedNode, Tag, TreeAllocRef, TreeAllocRefExt},
    },
    runtime::{
        metrics::EditMetrics,
        strategy::{EditKind, StrategyCandidate, StrategyContext, pick_candidate},
    },
    semantic::{Command, SemanticId},
    utils::Span,
};

#[derive(Debug, Clone)]
pub(crate) struct EditResult {
    pub messages: ParserMessages,
    pub reparsed_tree: Rc<RedNode>,
    pub newly_computed_nodes: Vec<Span>,
    pub newly_computed_tokens: Vec<Span>,
    pub semantic_commands: Vec<Command>,
}

pub struct Reparser {
    pub current: Rc<RedNode>,
    alloc: TreeAllocRef,
    config: ReparserConfig,
    next_semantic_id: SemanticId,
    semantic_roots: Vec<SemanticId>,
    semantic_nodes: HashMap<SemanticId, PersistSemanticNode>,
    rule_green_to_semantic: HashMap<usize, SemanticId>,
    token_label_to_id: HashMap<String, SemanticId>,
    token_refcount: HashMap<SemanticId, usize>,
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
    pub fn new(root: RedNode, alloc: TreeAllocRef) -> Self {
        Self {
            current: Rc::new(root),
            alloc,
            config: ReparserConfig::default(),
            next_semantic_id: 0,
            semantic_roots: Vec::new(),
            semantic_nodes: HashMap::new(),
            rule_green_to_semantic: HashMap::new(),
            token_label_to_id: HashMap::new(),
            token_refcount: HashMap::new(),
        }
    }

    pub fn with_config(mut self, config: ReparserConfig) -> Self {
        self.config = config;
        self
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

    pub fn handle_edit(
        &mut self,
        parser: &mut Parser,
        span: Span,
        new_len: usize,
        source_text: &str,
        mut metrics: Option<&mut EditMetrics>,
    ) -> EditResult {
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
            return EditResult {
                messages: parser.messages.clone(),
                reparsed_tree: self.current.clone(),
                newly_computed_nodes: Vec::new(),
                newly_computed_tokens: Vec::new(),
                semantic_commands: Vec::new(),
            };
        }

        let old_messages = parser.messages.clone();
        let old_text = parser.text().to_string();
        parser.messages.clear();
        parser.newly_computed_nodes.clear();
        parser.newly_computed_tokens.clear();
        parser.set_text(source_text);
        let focus_span = Self::focus_span_for_edit(source_text, span, new_len);

        if !old_messages.is_empty() {
            let result = self.full_reparse(parser, source_text, focus_span);
            if let Some(m) = &mut metrics {
                if let Some(start) = total_start {
                    m.total_duration_us = start.elapsed().as_micros();
                }
                m.used_incremental_path = false;
            }
            return result;
        }

        let (focus_node, mut steps, level) = self.get_context(span);
        let delta = new_len as isize - span.len() as isize;

        let specs = parser.recovery_specs().cloned();
        let strategy = parser.recovery_strategy().cloned();

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
            parser,
        );

        if let Some(m) = &mut metrics {
            m.candidates_collected = zippers.len();
            if let Some(start) = zipper_start {
                m.zipper_collection_us = start.elapsed().as_micros();
            }
        }

        if zippers.is_empty() {
            // Fallback: If focus node based search failed (rare), try from root
            self.ascend_to_root();
            zippers =
                collect_affected_zippers(self.current.clone(), search_span, &self.alloc, parser);

            if let Some(m) = &mut metrics {
                m.candidates_collected = zippers.len();
            }
        }

        if zippers.is_empty() {
            let result = self.full_reparse(parser, source_text, focus_span);
            if let Some(m) = &mut metrics {
                if let Some(start) = total_start {
                    m.total_duration_us = start.elapsed().as_micros();
                }
                m.used_incremental_path = false;
            }
            return result;
        }

        let reuse_before = parser.reuse_stats();

        let mut ctx = StrategyContext {
            parser,
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

        let best = pick_candidate(&mut ctx, edit_span, kind);

        let reuse_after = parser.reuse_stats();
        if let Some(m) = &mut metrics {
            m.parse_rule_calls = reuse_after.lookups.saturating_sub(reuse_before.lookups);
            m.parse_rule_cache_hits = reuse_after.hits.saturating_sub(reuse_before.hits);
        }

        if let Some(candidate) = best {
            let result = self.apply_candidate(
                parser,
                candidate,
                &old_messages,
                delta,
                focus_span,
                &old_text,
                source_text,
                metrics.as_deref_mut(),
            );
            if let Some(m) = &mut metrics {
                if let Some(start) = total_start {
                    m.total_duration_us = start.elapsed().as_micros();
                }
                m.used_incremental_path = true;
            }
            return result;
        }

        // If no candidate found among collected zippers, fall back to full reparse
        let result = self.full_reparse(parser, source_text, focus_span);
        if let Some(m) = &mut metrics {
            if let Some(start) = total_start {
                m.total_duration_us = start.elapsed().as_micros();
            }
            m.used_incremental_path = false;
        }
        result
    }

    fn apply_candidate(
        &mut self,
        parser: &mut Parser,
        candidate: StrategyCandidate,
        old_messages: &ParserMessages,
        delta: isize,
        focus_span: Span,
        old_source_text: &str,
        new_source_text: &str,
        mut metrics: Option<&mut EditMetrics>,
    ) -> EditResult {
        let (_updated_node, root) = candidate.zipper.replace_green(&self.alloc, candidate.green);
        self.current = root;

        // Merge messages: keep old messages outside the replaced range (shifted if needed)
        // and add new messages from the candidate.
        let replaced_start = candidate.zipper.offset;
        let replaced_end = replaced_start + candidate.zipper.old_width;
        let mut new_messages = Vec::with_capacity(old_messages.len() + candidate.messages.len());

        for msg in old_messages {
            if msg.span.end <= replaced_start {
                new_messages.push(msg.clone());
            } else if msg.span.start >= replaced_end {
                let mut shifted = msg.clone();
                let start = (msg.span.start as isize + delta).max(0) as usize;
                let end = (msg.span.end as isize + delta).max(0) as usize;
                shifted.span = Span::new(start, end);
                new_messages.push(shifted);
            }
        }
        new_messages.extend(candidate.messages.iter().cloned());
        new_messages.sort_by_key(|m| m.span.start);

        parser.messages = new_messages;
        self.normalize_root(parser);
        let reparsed_tree = self.focus_reparsed_tree(parser, focus_span);

        let new_width = self.alloc.get_node(candidate.green).width;
        let changed = Self::changed_window(
            old_source_text,
            new_source_text,
            candidate.zipper.offset,
            candidate.zipper.old_width,
            new_width,
        );
        let newly_computed_nodes =
            Self::filter_spans_by_window(candidate.newly_computed_nodes, changed);
        let newly_computed_tokens =
            Self::filter_spans_by_window(candidate.newly_computed_tokens, changed);

        let semantic_start = if metrics.is_some() {
            Some(std::time::Instant::now())
        } else {
            None
        };

        let semantic_commands = self.generate_commands_incremental(
            parser,
            candidate.zipper.node.green,
            candidate.green,
            candidate.zipper.offset,
            &newly_computed_nodes,
        );

        if let Some(m) = &mut metrics {
            m.semantic_commands_emitted = semantic_commands.len();
            if let Some(start) = semantic_start {
                m.semantic_diff_us = start.elapsed().as_micros();
            }
        }

        EditResult {
            messages: parser.messages.clone(),
            reparsed_tree,
            newly_computed_nodes,
            newly_computed_tokens,
            semantic_commands,
        }
    }

    fn full_reparse(
        &mut self,
        parser: &mut Parser,
        source_text: &str,
        focus_span: Span,
    ) -> EditResult {
        let result = parser.parse_text(source_text);
        self.current = Rc::new(result.root);
        let reparsed_tree = self.focus_reparsed_tree(parser, focus_span);

        let newly_computed_nodes = parser.newly_computed_nodes();
        let newly_computed_tokens = parser.newly_computed_tokens();

        let current_tree = self.current.clone();
        let semantic_commands =
            self.generate_commands(parser, &current_tree, &newly_computed_nodes);

        EditResult {
            messages: result.messages,
            reparsed_tree,
            newly_computed_nodes,
            newly_computed_tokens,
            semantic_commands,
        }
    }

    fn ascend_to_root(&mut self) {
        while let Some(parent) = &self.current.parent {
            self.current = Rc::clone(parent);
        }
    }

    fn focus_reparsed_tree(&self, parser: &Parser, focus_span: Span) -> Rc<RedNode> {
        let zippers =
            collect_affected_zippers(self.current.clone(), focus_span, &self.alloc, parser);
        if zippers.is_empty() {
            return self.current.clone();
        }

        let containing = zippers
            .iter()
            .filter(|z| {
                let end = z.offset + z.old_width;
                z.offset <= focus_span.start && end >= focus_span.end
            })
            .min_by_key(|z| (z.old_width, std::cmp::Reverse(z.level)))
            .map(|z| z.node.clone());

        if let Some(node) = containing {
            return node;
        }

        zippers
            .iter()
            .min_by_key(|z| (z.old_width, std::cmp::Reverse(z.level)))
            .map(|z| z.node.clone())
            .unwrap_or_else(|| self.current.clone())
    }

    fn focus_span_for_edit(source_text: &str, span: Span, new_len: usize) -> Span {
        if span.len() > 0 {
            return span;
        }
        if new_len == 0 {
            return span;
        }

        let original_start = span.start.min(source_text.len());
        let original_end = (span.start + new_len).min(source_text.len());
        let mut start = original_start;
        let mut end = original_end;
        if start >= end {
            return Span::new(start, end);
        }

        // Phase 1: trim insertion boundary trivia and list separators.
        start = Self::trim_leading_boundary(source_text, start, end);
        end = Self::trim_trailing_boundary(source_text, start, end);

        if start >= end {
            return Span::new(original_start, original_end);
        }

        // Keep inner text untouched; only trim outside boundary trivia.
        let trimmed = &source_text[start..end];
        let trailing_ws = trimmed
            .char_indices()
            .rev()
            .find(|(_, c)| !c.is_whitespace())
            .map(|(i, c)| i + c.len_utf8())
            .unwrap_or(0);
        end = start + trailing_ws;

        if start >= end {
            Span::new(original_start, original_end)
        } else {
            Span::new(start, end)
        }
    }

    fn trim_leading_boundary(source_text: &str, mut start: usize, end: usize) -> usize {
        loop {
            let slice = &source_text[start..end];
            let ws = slice
                .char_indices()
                .find(|(_, c)| !c.is_whitespace())
                .map(|(i, _)| i)
                .unwrap_or(slice.len());
            start += ws;

            if start >= end {
                return start;
            }

            let first = source_text[start..end].chars().next();
            if first.is_some_and(Self::is_list_separator) {
                start += first.unwrap().len_utf8();
                continue;
            }
            return start;
        }
    }

    fn trim_trailing_boundary(source_text: &str, start: usize, mut end: usize) -> usize {
        loop {
            let slice = &source_text[start..end];
            let trailing_ws = slice
                .char_indices()
                .rev()
                .find(|(_, c)| !c.is_whitespace())
                .map(|(i, c)| i + c.len_utf8())
                .unwrap_or(0);
            end = start + trailing_ws;

            if end <= start {
                return end;
            }

            let last = source_text[start..end]
                .char_indices()
                .last()
                .map(|(_, c)| c);
            if last.is_some_and(Self::is_list_separator) {
                end -= last.unwrap().len_utf8();
                continue;
            }
            return end;
        }
    }

    fn is_list_separator(c: char) -> bool {
        matches!(c, ',' | ';')
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

        let start_rule_ix = parser.grammar.table.start_rule;

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

    fn changed_window(
        old_text: &str,
        new_text: &str,
        start: usize,
        old_width: usize,
        new_width: usize,
    ) -> Span {
        let old_end = (start + old_width).min(old_text.len());
        let new_end = (start + new_width).min(new_text.len());
        if start > old_end || start > new_end {
            return Span::new(start, new_end);
        }

        let old_slice = &old_text[start..old_end];
        let new_slice = &new_text[start..new_end];
        let old_bytes = old_slice.as_bytes();
        let new_bytes = new_slice.as_bytes();

        let mut prefix = 0usize;
        while prefix < old_bytes.len()
            && prefix < new_bytes.len()
            && old_bytes[prefix] == new_bytes[prefix]
        {
            prefix += 1;
        }

        let mut suffix = 0usize;
        while suffix < old_bytes.len().saturating_sub(prefix)
            && suffix < new_bytes.len().saturating_sub(prefix)
            && old_bytes[old_bytes.len() - 1 - suffix] == new_bytes[new_bytes.len() - 1 - suffix]
        {
            suffix += 1;
        }

        let changed_start = start + prefix;
        let changed_end = start + new_bytes.len().saturating_sub(suffix);
        Span::new(changed_start.min(new_end), changed_end.min(new_end))
    }

    fn filter_spans_by_window(spans: Vec<Span>, changed: Span) -> Vec<Span> {
        if changed.is_empty() {
            return Vec::new();
        }
        spans
            .into_iter()
            .filter(|span| span.start < changed.end && span.end > changed.start)
            .collect()
    }

    fn generate_commands(
        &mut self,
        parser: &Parser,
        reparsed_tree: &RedNode,
        newly_computed_nodes: &[Span],
    ) -> Vec<Command> {
        let _ = newly_computed_nodes;
        let (temp_nodes, temp_roots) = self.build_temp_semantic_tree(parser, reparsed_tree);
        if temp_nodes.is_empty() || temp_roots.is_empty() {
            self.semantic_nodes.clear();
            self.semantic_roots.clear();
            return Vec::new();
        }

        let old_nodes = self.semantic_nodes.clone();
        let old_roots = self.semantic_roots.clone();

        let mut commands = Vec::new();
        let mut new_nodes = HashMap::<SemanticId, PersistSemanticNode>::new();
        let mut used_old = HashSet::<SemanticId>::new();
        let mut replaced_old = HashSet::<SemanticId>::new();
        let mut token_reuse = self.token_label_to_id.clone();

        let new_roots = self.diff_child_list(
            None,
            &old_roots,
            &temp_roots,
            &old_nodes,
            &temp_nodes,
            &mut new_nodes,
            &mut used_old,
            &mut replaced_old,
            &mut token_reuse,
            &mut commands,
        );

        for old_id in old_nodes.keys().copied() {
            if !used_old.contains(&old_id)
                && !replaced_old.contains(&old_id)
                && !new_nodes.contains_key(&old_id)
            {
                commands.push(Command::Delete(old_id));
            }
        }

        self.semantic_nodes = new_nodes;
        self.semantic_roots = new_roots;
        self.rebuild_rule_green_index();
        self.rebuild_token_tables();

        commands
    }

    fn generate_commands_incremental(
        &mut self,
        parser: &Parser,
        old_rule_green: usize,
        new_rule_green: usize,
        new_offset: usize,
        newly_computed_nodes: &[Span],
    ) -> Vec<Command> {
        if self.semantic_roots.is_empty() {
            let current_tree = self.current.clone();
            return self.generate_commands(parser, &current_tree, newly_computed_nodes);
        }

        let Some(old_root_id) = self.rule_green_to_semantic.get(&old_rule_green).copied() else {
            let current_tree = self.current.clone();
            return self.generate_commands(parser, &current_tree, newly_computed_nodes);
        };

        let new_subtree =
            RedNode::root_with_span(new_rule_green, Span::new(new_offset, new_offset));
        let (temp_nodes, temp_roots) = self.build_temp_semantic_tree(parser, &new_subtree);
        if temp_roots.is_empty() {
            let current_tree = self.current.clone();
            return self.generate_commands(parser, &current_tree, newly_computed_nodes);
        }

        let old_parent = self.semantic_nodes.get(&old_root_id).and_then(|n| n.parent);
        let mut commands = Vec::new();
        let mut token_reuse = self.token_label_to_id.clone();
        let mut created = HashMap::<SemanticId, PersistSemanticNode>::new();

        let replacement_root = self.create_subtree(
            temp_roots[0],
            &temp_nodes,
            &mut created,
            old_parent,
            &mut token_reuse,
            &mut commands,
        );
        commands.push(Command::Replace(old_root_id, replacement_root));

        if let Some(parent_id) = old_parent {
            if let Some(parent_node) = self.semantic_nodes.get_mut(&parent_id) {
                if let Some(pos) = parent_node
                    .children
                    .iter()
                    .position(|&id| id == old_root_id)
                {
                    parent_node.children[pos] = replacement_root;
                }
            }
        } else {
            if let Some(pos) = self.semantic_roots.iter().position(|&id| id == old_root_id) {
                self.semantic_roots[pos] = replacement_root;
            }
        }

        for (id, mut node) in created {
            if node.parent == Some(old_root_id) {
                node.parent = old_parent;
            }
            self.semantic_nodes.insert(id, node);
        }

        self.remove_rule_green_subtree(old_root_id);
        self.add_rule_green_subtree(replacement_root);
        self.token_label_to_id = token_reuse;

        commands
    }

    fn rebuild_rule_green_index(&mut self) {
        self.rule_green_to_semantic.clear();
        let roots = self.semantic_roots.clone();
        for root in roots {
            self.add_rule_green_subtree(root);
        }
    }

    fn add_rule_green_subtree(&mut self, root_id: SemanticId) {
        let mut stack = vec![root_id];
        while let Some(node_id) = stack.pop() {
            if let Some(node) = self.semantic_nodes.get(&node_id) {
                if let Some(green) = node.rule_green {
                    self.rule_green_to_semantic.insert(green, node_id);
                }
                for &child in &node.children {
                    stack.push(child);
                }
            }
        }
    }

    fn remove_rule_green_subtree(&mut self, root_id: SemanticId) {
        let mut stack = vec![root_id];
        while let Some(node_id) = stack.pop() {
            if let Some(node) = self.semantic_nodes.get(&node_id) {
                if let Some(green) = node.rule_green {
                    self.rule_green_to_semantic.remove(&green);
                }
                for &child in &node.children {
                    stack.push(child);
                }
            }
        }
    }

    fn build_temp_semantic_tree(
        &self,
        parser: &Parser,
        root: &RedNode,
    ) -> (Vec<TempSemanticNode>, Vec<usize>) {
        let mut nodes = Vec::<TempSemanticNode>::new();
        let roots = self.collect_temp_semantic_nodes(parser, root, &mut nodes);
        (nodes, roots)
    }

    fn collect_temp_semantic_nodes(
        &self,
        parser: &Parser,
        node: &RedNode,
        out_nodes: &mut Vec<TempSemanticNode>,
    ) -> Vec<usize> {
        let green = self.alloc.get_node(node.green);

        let mut child_offset = node.offset;
        let mut semantic_children = Vec::<usize>::new();
        for &child_id in &green.children {
            let child_node = RedNode {
                parent: Some(Rc::new(node.clone())),
                offset: child_offset,
                green: child_id,
            };
            semantic_children.extend(self.collect_temp_semantic_nodes(
                parser,
                &child_node,
                out_nodes,
            ));
            child_offset += self.alloc.get_node(child_id).width;
        }

        let mut include = false;
        let mut kind = SemanticNodeKind::Rule;
        let mut label = String::new();

        match &green.tag {
            Tag::Rule { rule_ix } => {
                let is_root = node.parent.is_none();
                let rule_child_count = green
                    .children
                    .iter()
                    .filter(|&&child_id| {
                        matches!(self.alloc.get_node(child_id).tag, Tag::Rule { .. })
                    })
                    .count();
                let is_simple_wrapper = !is_root && rule_child_count == 1;
                if !is_simple_wrapper {
                    include = true;
                    kind = SemanticNodeKind::Rule;
                    label = parser.grammar.name(*rule_ix).to_string();
                }
            }
            Tag::Token { .. } => {
                if green.width > 0 {
                    include = true;
                    kind = SemanticNodeKind::Token;
                    let token_end = (node.offset + green.width).min(parser.text().len());
                    label = parser.text()[node.offset..token_end].to_string();
                }
            }
            _ => {}
        }

        if !include {
            return semantic_children;
        }

        let node_ix = out_nodes.len();
        out_nodes.push(TempSemanticNode {
            kind,
            label,
            rule_green: matches!(kind, SemanticNodeKind::Rule).then_some(node.green),
            children: semantic_children,
        });
        vec![node_ix]
    }

    fn diff_child_list(
        &mut self,
        parent_id: Option<SemanticId>,
        old_children: &[SemanticId],
        new_children: &[usize],
        old_nodes: &HashMap<SemanticId, PersistSemanticNode>,
        temp_nodes: &[TempSemanticNode],
        new_nodes: &mut HashMap<SemanticId, PersistSemanticNode>,
        used_old: &mut HashSet<SemanticId>,
        replaced_old: &mut HashSet<SemanticId>,
        token_reuse: &mut HashMap<String, SemanticId>,
        commands: &mut Vec<Command>,
    ) -> Vec<SemanticId> {
        let mut out = Vec::<SemanticId>::new();
        let mut i = 0;
        let mut j = 0;

        while i < old_children.len() && j < new_children.len() {
            let old_id = old_children[i];
            let new_ix = new_children[j];

            let old_sig = old_nodes.get(&old_id).map(|n| (&n.kind, &n.label));
            let new_sig = temp_nodes.get(new_ix).map(|n| (&n.kind, &n.label));

            if old_sig.is_some() && old_sig == new_sig {
                let reused_id = self.diff_node_reuse(
                    old_id,
                    new_ix,
                    old_nodes,
                    temp_nodes,
                    new_nodes,
                    used_old,
                    replaced_old,
                    token_reuse,
                    commands,
                );
                out.push(reused_id);
                i += 1;
                j += 1;
                continue;
            }

            let can_skip_old = i + 1 < old_children.len()
                && old_nodes
                    .get(&old_children[i + 1])
                    .map(|n| (&n.kind, &n.label))
                    == new_sig;

            if can_skip_old {
                i += 1;
                continue;
            }

            let can_insert_new = j + 1 < new_children.len()
                && old_sig
                    == temp_nodes
                        .get(new_children[j + 1])
                        .map(|n| (&n.kind, &n.label));

            if can_insert_new {
                let inserted_id = self.create_subtree(
                    new_ix,
                    temp_nodes,
                    new_nodes,
                    parent_id,
                    token_reuse,
                    commands,
                );
                if let Some(parent) = parent_id {
                    commands.push(Command::Insert(parent, inserted_id));
                }
                out.push(inserted_id);
                j += 1;
                continue;
            }

            let replaced_id = self.create_subtree(
                new_ix,
                temp_nodes,
                new_nodes,
                parent_id,
                token_reuse,
                commands,
            );
            commands.push(Command::Replace(old_id, replaced_id));
            self.mark_replaced_subtree(old_id, old_nodes, replaced_old);
            used_old.insert(old_id);
            out.push(replaced_id);
            i += 1;
            j += 1;
        }

        while j < new_children.len() {
            let new_ix = new_children[j];
            let inserted_id = self.create_subtree(
                new_ix,
                temp_nodes,
                new_nodes,
                parent_id,
                token_reuse,
                commands,
            );
            if let Some(parent) = parent_id {
                commands.push(Command::Insert(parent, inserted_id));
            }
            out.push(inserted_id);
            j += 1;
        }

        out
    }

    fn diff_node_reuse(
        &mut self,
        old_id: SemanticId,
        new_ix: usize,
        old_nodes: &HashMap<SemanticId, PersistSemanticNode>,
        temp_nodes: &[TempSemanticNode],
        new_nodes: &mut HashMap<SemanticId, PersistSemanticNode>,
        used_old: &mut HashSet<SemanticId>,
        replaced_old: &mut HashSet<SemanticId>,
        token_reuse: &mut HashMap<String, SemanticId>,
        commands: &mut Vec<Command>,
    ) -> SemanticId {
        used_old.insert(old_id);

        let old_children = old_nodes
            .get(&old_id)
            .map(|n| n.children.clone())
            .unwrap_or_default();
        let new_children_ix = temp_nodes
            .get(new_ix)
            .map(|n| n.children.clone())
            .unwrap_or_default();

        let new_children = self.diff_child_list(
            Some(old_id),
            &old_children,
            &new_children_ix,
            old_nodes,
            temp_nodes,
            new_nodes,
            used_old,
            replaced_old,
            token_reuse,
            commands,
        );

        if let Some(new_node) = temp_nodes.get(new_ix) {
            new_nodes.insert(
                old_id,
                PersistSemanticNode {
                    kind: new_node.kind,
                    label: new_node.label.clone(),
                    rule_green: new_node.rule_green,
                    parent: old_nodes.get(&old_id).and_then(|n| n.parent),
                    children: new_children,
                },
            );
        }

        old_id
    }

    fn create_subtree(
        &mut self,
        root_ix: usize,
        temp_nodes: &[TempSemanticNode],
        new_nodes: &mut HashMap<SemanticId, PersistSemanticNode>,
        parent: Option<SemanticId>,
        token_reuse: &mut HashMap<String, SemanticId>,
        commands: &mut Vec<Command>,
    ) -> SemanticId {
        let node = &temp_nodes[root_ix];

        if matches!(node.kind, SemanticNodeKind::Token) {
            if let Some(existing_id) = token_reuse.get(&node.label).copied() {
                if !new_nodes.contains_key(&existing_id) {
                    if let Some(existing_node) = self.semantic_nodes.get(&existing_id).cloned() {
                        new_nodes.insert(existing_id, existing_node);
                    } else {
                        new_nodes.insert(
                            existing_id,
                            PersistSemanticNode {
                                kind: SemanticNodeKind::Token,
                                label: node.label.clone(),
                                rule_green: None,
                                parent,
                                children: Vec::new(),
                            },
                        );
                    }
                }
                return existing_id;
            }
        }

        let id = self.next_semantic_id;
        self.next_semantic_id += 1;

        match node.kind {
            SemanticNodeKind::Rule => commands.push(Command::Create(id, node.label.clone())),
            SemanticNodeKind::Token => {
                commands.push(Command::CreateToken(id, node.label.clone()));
                token_reuse.insert(node.label.clone(), id);
            }
        }

        let mut child_ids = Vec::new();
        for &child_ix in &node.children {
            let child_id = self.create_subtree(
                child_ix,
                temp_nodes,
                new_nodes,
                Some(id),
                token_reuse,
                commands,
            );
            commands.push(Command::Insert(id, child_id));
            child_ids.push(child_id);
        }

        new_nodes.insert(
            id,
            PersistSemanticNode {
                kind: node.kind,
                label: node.label.clone(),
                rule_green: node.rule_green,
                parent,
                children: child_ids,
            },
        );

        id
    }

    fn mark_replaced_subtree(
        &self,
        root_id: SemanticId,
        old_nodes: &HashMap<SemanticId, PersistSemanticNode>,
        replaced_old: &mut HashSet<SemanticId>,
    ) {
        let mut stack = vec![root_id];
        while let Some(node_id) = stack.pop() {
            if !replaced_old.insert(node_id) {
                continue;
            }
            if let Some(node) = old_nodes.get(&node_id) {
                for &child in &node.children {
                    stack.push(child);
                }
            }
        }
    }

    fn rebuild_token_tables(&mut self) {
        let mut label_to_id = HashMap::<String, SemanticId>::new();
        let mut refcount = HashMap::<SemanticId, usize>::new();

        for &root_id in &self.semantic_roots {
            self.count_token_occurrences(root_id, &mut label_to_id, &mut refcount);
        }

        self.token_label_to_id = label_to_id;
        self.token_refcount = refcount;
    }

    fn count_token_occurrences(
        &self,
        node_id: SemanticId,
        label_to_id: &mut HashMap<String, SemanticId>,
        refcount: &mut HashMap<SemanticId, usize>,
    ) {
        let Some(node) = self.semantic_nodes.get(&node_id) else {
            return;
        };

        match node.kind {
            SemanticNodeKind::Token => {
                label_to_id.entry(node.label.clone()).or_insert(node_id);
                *refcount.entry(node_id).or_insert(0) += 1;
            }
            SemanticNodeKind::Rule => {
                for &child in &node.children {
                    self.count_token_occurrences(child, label_to_id, refcount);
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SemanticNodeKind {
    Rule,
    Token,
}

#[derive(Debug, Clone)]
struct TempSemanticNode {
    kind: SemanticNodeKind,
    label: String,
    rule_green: Option<usize>,
    children: Vec<usize>,
}

#[derive(Debug, Clone)]
struct PersistSemanticNode {
    kind: SemanticNodeKind,
    label: String,
    rule_green: Option<usize>,
    parent: Option<SemanticId>,
    children: Vec<SemanticId>,
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
    pub steps: Vec<ZipperStep>,
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
            let (parent_tag, mut children) = {
                let parent_green = alloc.get_node(parent.green);
                (parent_green.tag.clone(), parent_green.children.clone())
            };
            children[step.child_idx] = current.green;

            let new_width: usize = children.iter().map(|&c| alloc.get_node(c).width).sum();
            let new_parent_green = alloc.alloc(parent_tag, children, new_width);

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
    let green = alloc.get_node(node.green);
    let _rule_ix = match &green.tag {
        Tag::Rule { rule_ix } => *rule_ix,
        _ => usize::MAX,
    };

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

    let is_insertion = span.len() == 0;

    let mut has_separator_children = false;
    let mut separator_index = vec![false; green.children.len()];

    for (idx, &child_id) in green.children.iter().enumerate() {
        let child = alloc.get_node(child_id);
        if matches!(&child.tag, Tag::Token { .. }) {
            if child.width <= 2 {
                has_separator_children = true;
                separator_index[idx] = true;
            }
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
                        let next_child_end = next_child_start + alloc.get_node(next_child_id).width;
                        overlaps.push((idx + 2, next_child_id, next_child_start, next_child_end));
                    }
                }
            }
        }
    }

    if overlaps.len() == 1 || (is_insertion && overlaps.len() > 1) {
        let (child_idx, child_id, child_start, child_end) = if overlaps.len() == 1 {
            overlaps[0]
        } else {
            let mut preferred = None;

            for candidate in &overlaps {
                if alloc.get_node(candidate.1).width == 0 {
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
                        if matches!(&next_sep.tag, Tag::Token { .. }) {
                            if idx + 2 < green.children.len() {
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
            }

            if preferred.is_none() {
                // Prefer rules (like @sep) over tokens (like "{") when insertion is at a boundary
                for candidate in &overlaps {
                    let (_, child_id, cstart, _) = *candidate;
                    let child = alloc.get_node(child_id);
                    let is_rule = matches!(&child.tag, Tag::Rule { .. });

                    // Prefer: insertion starts at child start (for list rules)
                    if span.start == cstart && is_rule && child.width > 0 {
                        if preferred.is_none() {
                            preferred = Some(*candidate);
                        }
                    }
                }
            }

            if preferred.is_none() {
                for candidate in &overlaps {
                    let (_, _, _, cend) = *candidate;
                    let child = alloc.get_node(candidate.1);
                    if span.start == cend && child.width > 0 {
                        // Prefer rules over tokens, so we can recurse into structure
                        let is_rule = matches!(&child.tag, Tag::Rule { .. });
                        let already_preferred = preferred.is_some();
                        let current_is_token =
                            matches!(&alloc.get_node(candidate.1).tag, Tag::Token { .. });

                        if !already_preferred || (is_rule && current_is_token) {
                            preferred = Some(*candidate);
                        }
                    }
                }
            }

            preferred.unwrap_or_else(|| *overlaps.last().unwrap())
        };

        let can_descend = if is_insertion {
            span.start >= child_start && span.start <= child_end
        } else {
            span.start >= child_start && span.end <= child_end
        };

        if can_descend {
            let child = alloc.get_node(child_id);
            let should_stop_at_separator = is_insertion
                && has_separator_children
                && child_idx > 0
                && separator_index[child_idx - 1];

            steps.push(ZipperStep {
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
                    rule_ix: child_rule_ix,
                } = &child.tag
                {
                    out.push(Zipper {
                        node: child_node.clone(),
                        rule_ix: *child_rule_ix,
                        offset: child_start,
                        old_width: child.width,
                        level: level + 1,
                        steps: steps.clone(),
                    });
                }
                // Continue recursing even after stopping at separator to find deeper tails
                collect_from(child_node, span, alloc, steps, level + 1, out, parser);
            } else {
                collect_from(child_node, span, alloc, steps, level + 1, out, parser);
            }
            steps.pop();

            // For sep-based list rules, also explore following children to find @sep_tail
            if let Tag::Rule { rule_ix } = &green.tag {
                let rule_name = parser.grammar.name(*rule_ix);
                if is_insertion && (rule_name == "@sep" || rule_name.ends_with("@sep")) {
                    // After processing the selected child, also check remaining children for @sep_tail
                    for (remaining_idx, &remaining_id) in green.children.iter().enumerate() {
                        if remaining_idx <= child_idx {
                            continue; // Skip children we already processed
                        }

                        let remaining_child = alloc.get_node(remaining_id);
                        if let Tag::Rule {
                            rule_ix: remaining_rule,
                        } = &remaining_child.tag
                        {
                            let remaining_name = parser.grammar.name(*remaining_rule);
                            if remaining_name == "@sep_tail" || remaining_name.ends_with("_tail") {
                                // Found a tail rule, recurse into it
                                let mut remaining_start = node.offset;
                                for &prior_id in green.children.iter().take(remaining_idx) {
                                    remaining_start += alloc.get_node(prior_id).width;
                                }
                                steps.push(ZipperStep {
                                    parent: node.clone(),
                                    child_idx: remaining_idx,
                                });

                                let tail_node = Rc::new(RedNode {
                                    parent: Some(node.clone()),
                                    offset: remaining_start,
                                    green: remaining_id,
                                });

                                collect_from(tail_node, span, alloc, steps, level + 1, out, parser);
                                steps.pop();
                            }
                        }
                    }
                }
            }

            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Reparser;
    use crate::utils::Span;

    #[test]
    fn insertion_focus_trims_leading_separator() {
        let source = r#"{"name": "Dexer", "age": 30}"#;
        let inserted = r#", "age": 30"#;
        let start = source.find(inserted).expect("inserted text must exist");

        let span = Reparser::focus_span_for_edit(source, Span::new(start, start), inserted.len());
        assert_eq!(&source[span.start..span.end], r#""age": 30"#);
    }

    #[test]
    fn insertion_focus_trims_trailing_separator() {
        let source = r#"{"name": "Dexer", "age": 30, "city": "LA"}"#;
        let inserted = r#""age": 30, "#;
        let start = source.find(inserted).expect("inserted text must exist");

        let span = Reparser::focus_span_for_edit(source, Span::new(start, start), inserted.len());
        assert_eq!(&source[span.start..span.end], r#""age": 30"#);
    }

    #[test]
    fn insertion_focus_falls_back_when_only_separator_inserted() {
        let source = "{, }";
        let inserted = ", ";
        let start = source.find(inserted).expect("inserted text must exist");

        let span = Reparser::focus_span_for_edit(source, Span::new(start, start), inserted.len());
        assert_eq!(&source[span.start..span.end], inserted);
    }
}
