use crate::{
    grammar::{
        Grammar,
        ir::{Scope, State},
        recovery::{ErrorRecoveryStrategy, RecoverySpecs},
    },
    parsec::{
        tree::{ParsecError, RedNode, Tag, TreeAlloc},
        words::Matcher,
    },
};
use std::collections::HashSet;

#[derive(Debug, Clone, Copy)]
pub struct ParserConfig {
    pub recover: bool,
}

impl ParserConfig {
    pub fn new() -> Self {
        Self { recover: false }
    }

    pub fn recovering() -> Self {
        Self { recover: true }
    }
}

pub struct Parser<'a> {
    pub(crate) text: &'a str,
    pub(crate) grammar: &'a Grammar,
    pub(crate) alloc: TreeAlloc,
    in_flight: HashSet<ParseKey>,
    config: ParserConfig,
    specs: Option<RecoverySpecs>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct ParseKey {
    state_id: usize,
    pos: usize,
}

struct ParseResult {
    pos: usize,
    ok: bool,
    cost: usize,
    has_error: bool,
}

impl ParseResult {
    fn ok(pos: usize) -> Self {
        Self {
            pos,
            ok: true,
            cost: 0,
            has_error: false,
        }
    }
    fn failed(pos: usize) -> Self {
        Self {
            pos,
            ok: false,
            cost: 0,
            has_error: false,
        }
    }
    fn recovered(pos: usize) -> Self {
        Self {
            pos,
            ok: true,
            cost: 1,
            has_error: true,
        }
    }
    fn with_error(mut self) -> Self {
        self.has_error = true;
        self
    }
}

impl<'a> Parser<'a> {
    pub fn new(text: &'a str, grammar: &'a Grammar) -> Self {
        Self::new_with_config(text, grammar, ParserConfig::new())
    }

    pub fn new_with_config(text: &'a str, grammar: &'a Grammar, config: ParserConfig) -> Self {
        let alloc = TreeAlloc::new();
        let specs = if config.recover {
            let strategy = ErrorRecoveryStrategy::from_grammar(grammar);
            Some(RecoverySpecs::from_text_with_strategy(text, strategy))
        } else {
            None
        };
        Self {
            text,
            grammar,
            alloc,
            in_flight: HashSet::new(),
            config,
            specs,
        }
    }

    pub fn parse_text(&mut self) -> RedNode {
        let start_state = self.grammar.analysis.start_state;

        let mut root = RedNode::new_root(&self.alloc, self.text);
        let root_green = root.green;

        // Treat root as if it's in a sequence to enable top-level recovery
        self.parse(root_green, start_state, 0, None, true);

        if self.alloc.get_node(root_green).children.len() == 1 {
            if let Some(&child) = self.alloc.get_node(root_green).children.first() {
                root.green = child;
            }
        } else if self.alloc.get_node(root_green).children.len() > 1 {
            // Find the valid rule node among debris
            let start_rule_ix = self.grammar.analysis.states[start_state].ref_ix();
            let valid_child = self
                .alloc
                .get_node(root_green)
                .children
                .iter()
                .find(|&&child| match &self.alloc.get_node(child).tag {
                    Tag::Rule { rule_ix } => *rule_ix == start_rule_ix,
                    _ => false,
                });

            if let Some(&child) = valid_child {
                root.green = child;
            }
        }

        root
    }

    fn parse(
        &mut self,
        node_id: usize,
        state_id: usize,
        pos: usize,
        last_rule_ix: Option<usize>,
        parent_is_sequence: bool,
    ) -> ParseResult {
        let key = ParseKey { state_id, pos };
        if self.in_flight.contains(&key) {
            return ParseResult::failed(pos);
        }

        self.in_flight.insert(key);
        let res = self.parse_inner(node_id, state_id, pos, last_rule_ix, parent_is_sequence);
        self.in_flight.remove(&key);
        res
    }

    fn parse_child(
        &mut self,
        work_node: usize,
        child_state: usize,
        pos: usize,
        rule_ix: usize,
        parent_is_sequence: bool,
    ) -> ParseResult {
        self.parse(
            work_node,
            child_state,
            pos,
            Some(rule_ix),
            parent_is_sequence,
        )
    }

    fn make_work_node(
        &mut self,
        node_id: usize,
        rule_ix: usize,
        should_create_node: bool,
    ) -> usize {
        if should_create_node {
            let tag = Tag::new_rule(rule_ix);
            self.alloc.alloc(tag, vec![], 0)
        } else {
            node_id
        }
    }

    fn parse_inner(
        &mut self,
        node_id: usize,
        state_id: usize,
        pos: usize,
        last_rule_ix: Option<usize>,
        parent_is_sequence: bool,
    ) -> ParseResult {
        let _ = last_rule_ix;
        let state = self.grammar.analysis.states[state_id].clone();
        let current_rule_ix = state.ref_ix();

        // Only create a new AST node when the rule changes and the rule is named
        let is_trivial_rule = self.grammar.name(current_rule_ix).starts_with('@');
        let should_create_node = !is_trivial_rule;

        let result = match state {
            State::Tok(rule_ix, matcher) => {
                self.parse_token(node_id, rule_ix, matcher.as_ref(), pos, should_create_node)
            }

            State::Seq(rule_ix, children) => self.parse_sequence(
                node_id,
                rule_ix,
                children,
                pos,
                should_create_node,
                parent_is_sequence,
            ),

            State::Alt(rule_ix, children, has_epsilon) => self.parse_alternative(
                node_id,
                rule_ix,
                children,
                has_epsilon,
                pos,
                should_create_node,
                parent_is_sequence,
            ),

            State::Field(rule_ix, name, child) => {
                self.parse_field(node_id, rule_ix, name, child, pos)
            }

            State::LeftRec(rule_ix, base, tail, tail_fields) => self.parse_left_rec(
                node_id,
                rule_ix,
                base,
                tail,
                tail_fields,
                pos,
                should_create_node,
            ),
        };

        if !result.ok && self.config.recover && last_rule_ix.is_none() && should_create_node {
            if let Some(recovered_pos) = self.attempt_recovery(node_id, pos) {
                let retry = self.parse_inner(
                    node_id,
                    state_id,
                    recovered_pos,
                    last_rule_ix,
                    parent_is_sequence,
                );
                if retry.ok {
                    return retry;
                }
                return ParseResult::recovered(recovered_pos);
            }
        }

        result
    }

    fn parse_token(
        &mut self,
        node_id: usize,
        rule_ix: usize,
        matcher: &dyn Matcher,
        pos: usize,
        _should_create_node: bool,
    ) -> ParseResult {
        let mut current_pos = pos;
        let matched = if current_pos <= self.text.len() {
            matcher.matches(self.text, &mut current_pos)
        } else {
            None
        };
        if let Some(width) = matched {
            if width == 0 {
                return ParseResult::ok(current_pos);
            }
            let tag = Tag::new_token(rule_ix);
            let token_id = self.alloc.alloc_token(tag, width);
            self.alloc.get_node_mut(node_id).children.push(token_id);
            ParseResult::ok(current_pos)
        } else {
            ParseResult::failed(pos)
        }
    }

    /// Parse n-ary sequence (all children in sequence)
    fn parse_sequence(
        &mut self,
        node_id: usize,
        rule_ix: usize,
        children: Vec<usize>,
        pos: usize,
        should_create_node: bool,
        parent_is_sequence: bool,
    ) -> ParseResult {
        let work_node = self.make_work_node(node_id, rule_ix, should_create_node);
        let saved_children_len = self.alloc.get_node(work_node).children.len();

        let mut current_pos = pos;
        let mut has_error = false;
        let allow_recovery =
            self.config.recover && parent_is_sequence && self.sequence_is_structural(&children);

        let rule_name = self.grammar.name(rule_ix);
        let is_sep_tail = rule_name.starts_with("@sep_tail");

        let mut idx = 0;
        while idx < children.len() {
            let child_state = children[idx];
            let res = self.parse_child(work_node, child_state, current_pos, rule_ix, true);

            if res.ok {
                current_pos = res.pos;
                if res.has_error {
                    has_error = true;
                }
                idx += 1;
                continue;
            }

            // --- Fine-grained Recovery: Insertion ---
            // Only enable insertion if we are in a safe context:
            // - Structural sequence (e.g., block with delimiters), OR
            // - Committed sequence (already matched at least 2 tokens)
            let can_use_insertion = allow_recovery || ((current_pos > pos) && idx > 1);
            if self.config.recover && can_use_insertion {
                let can_insert = self.is_literal(child_state);
                if can_insert {
                    let mut should_insert = false;
                    // Lookahead: Try to parse the next child at current_pos
                    if idx + 1 < children.len() {
                        let next_child = children[idx + 1];
                        // Speculative parse (cost check omitted for now, assuming 1)
                        let saved = self.alloc.get_node(work_node).children.len();
                        let next_res =
                            self.parse_child(work_node, next_child, current_pos, rule_ix, true);
                        // Revert the node (discard speculative result)
                        self.alloc.get_node_mut(work_node).children.truncate(saved);

                        if next_res.ok {
                            should_insert = true;
                        }
                    } else {
                        // End of sequence: insert if committed
                        if idx > 0 {
                            should_insert = true;
                        }
                    }

                    if should_insert {
                        let error_node = self.alloc.alloc(
                            Tag::new_error(ParsecError::MissingToken),
                            vec![],
                            0, // 0 width for insertion
                        );
                        self.alloc.get_node_mut(work_node).children.push(error_node);
                        has_error = true;
                        idx += 1; // Skip this child (it is "inserted")
                        continue;
                    }
                }
            }
            // ----------------------------------------

            let mut recovered = None;
            let committed = (current_pos > pos) && self.config.recover && idx > 0;

            // Recover if structural (safe at start) or committed (safe to finish)
            if allow_recovery || committed {
                recovered = self.attempt_recovery(work_node, current_pos);
            }
            if recovered.is_none()
                && self.config.recover
                && is_sep_tail
                && !self.at_list_end(current_pos)
            {
                recovered = self.attempt_recovery(work_node, current_pos);
            }

            // Fallback: Panic mode (skip one character) if committed or structural
            if recovered.is_none() && (committed || allow_recovery) {
                if let Some(c) = self.text[current_pos..].chars().next() {
                    let w = c.len_utf8();
                    let error_node =
                        self.alloc
                            .alloc(Tag::new_error(ParsecError::UnexpectedToken), vec![], w);
                    self.alloc.get_node_mut(work_node).children.push(error_node);
                    recovered = Some(current_pos + w);
                }
            }

            if let Some(recovered_pos) = recovered {
                if recovered_pos > current_pos {
                    // Try to match the current child again at the recovered position
                    let retry_res =
                        self.parse_child(work_node, child_state, recovered_pos, rule_ix, true);
                    if retry_res.ok {
                        current_pos = retry_res.pos;
                        if retry_res.has_error {
                            has_error = true;
                        }
                    } else {
                        current_pos = recovered_pos;
                        has_error = true;
                    }
                    idx += 1;
                    continue;
                }
            }

            // Rollback on failure
            self.alloc
                .get_node_mut(work_node)
                .children
                .truncate(saved_children_len);
            return ParseResult::failed(pos);
        }

        if should_create_node {
            self.finalize_node(work_node, node_id);
        }

        // Determine if result has errors based on children
        if !has_error {
            has_error = self
                .alloc
                .get_node(work_node)
                .children
                .iter()
                .any(|&child_id| {
                    let tag = &self.alloc.get_node(child_id).tag;
                    tag.is_error() // Assume Tag::is_error() exists and checks for Tag::Error
                });
        }

        let mut res = ParseResult::ok(current_pos);
        if has_error {
            res = res.with_error();
        }
        res
    }

    /// Parse n-ary alternative (try each child until one succeeds)
    fn parse_alternative(
        &mut self,
        node_id: usize,
        rule_ix: usize,
        children: Vec<usize>,
        has_epsilon: bool,
        pos: usize,
        should_create_node: bool,
        parent_is_sequence: bool,
    ) -> ParseResult {
        let work_node = self.make_work_node(node_id, rule_ix, should_create_node);
        // if self.grammar.name(rule_ix).starts_with("@rep") {
        //     println!("Parsing @rep at pos {}, has_epsilon={}", pos, has_epsilon);
        // }

        // Try each child until one succeeds
        for child_state in children.iter().copied() {
            let saved_len = self.alloc.get_node(work_node).children.len();
            let res = self.parse_child(work_node, child_state, pos, rule_ix, false);

            if res.ok && res.pos > pos {
                // If we have an error result but also an epsilon option,
                // we should check if the error is "pure" (only error nodes).
                // If so, we reject this alternative in favor of epsilon (or subsequent alternatives)
                // to avoid consuming delimiters as error nodes (greedy error consumption).
                if res.has_error && has_epsilon {
                    let is_pure_error = !self
                        .alloc
                        .get_node(work_node)
                        .children
                        .iter()
                        .skip(saved_len)
                        .any(|&c| self.contains_valid_token(c));

                    if is_pure_error {
                        // println!("Rejecting pure error for rule {} at pos {}", self.grammar.name(rule_ix), pos);
                        // Reject this alternative
                        self.alloc
                            .get_node_mut(work_node)
                            .children
                            .truncate(saved_len);
                        continue;
                    }
                }

                if should_create_node {
                    self.finalize_node(work_node, node_id);
                }
                return res;
            }

            // Backtrack
            self.alloc
                .get_node_mut(work_node)
                .children
                .truncate(saved_len);
        }

        // Try epsilon if available
        if has_epsilon {
            if should_create_node {
                self.finalize_node(work_node, node_id);
            }
            return ParseResult::ok(pos);
        }

        // All alternatives failed
        if self.config.recover && parent_is_sequence && should_create_node {
            if let Some(recovered_pos) = self.attempt_recovery(work_node, pos) {
                self.finalize_node(work_node, node_id);
                return ParseResult::recovered(recovered_pos);
            }
        }

        ParseResult::failed(pos)
    }

    fn parse_left_rec(
        &mut self,
        node_id: usize,
        rule_ix: usize,
        base_states: Vec<usize>,
        tail_states: Vec<usize>,
        tail_fields: Vec<Option<&'static str>>,
        pos: usize,
        should_create_node: bool,
    ) -> ParseResult {
        let work_node = self.make_work_node(node_id, rule_ix, should_create_node);

        let mut current_pos = pos;
        let mut base_ok = false;
        let mut has_error = false;

        for base_state in base_states {
            let saved_children_len = self.alloc.get_node(work_node).children.len();
            let res = self.parse_child(work_node, base_state, pos, rule_ix, false);
            if res.ok {
                current_pos = res.pos;
                if res.has_error {
                    has_error = true;
                }
                base_ok = true;
                break;
            }
            self.alloc
                .get_node_mut(work_node)
                .children
                .truncate(saved_children_len);
        }

        if !base_ok {
            // If no base matched, try with epsilon base (start at current position)
            // This allows left-recursive patterns like A -> A "a" | epsilon to work
            // by trying to match tail states directly
            current_pos = pos;
        }

        if should_create_node {
            self.update_width(work_node);
        }

        let mut current_node = work_node;

        loop {
            let mut progressed = false;

            for (tail_state, tail_field) in tail_states.iter().zip(tail_fields.iter()) {
                if should_create_node {
                    let base_child = if let Some(name) = tail_field {
                        let field_node =
                            self.alloc
                                .alloc(Tag::new_field(rule_ix, *name), vec![current_node], 0);
                        self.update_width(field_node);
                        field_node
                    } else {
                        current_node
                    };
                    let tag = Tag::new_rule(rule_ix);
                    let new_node = self.alloc.alloc(tag, vec![base_child], 0);
                    let res = self.parse_child(new_node, *tail_state, current_pos, rule_ix, false);
                    if res.ok && res.pos > current_pos {
                        if res.has_error {
                            has_error = true;
                        }
                        self.update_width(new_node);
                        current_node = new_node;
                        current_pos = res.pos;
                        progressed = true;
                        break;
                    }
                } else {
                    let saved_children_len = self.alloc.get_node(current_node).children.len();
                    let res =
                        self.parse_child(current_node, *tail_state, current_pos, rule_ix, false);
                    if res.ok && res.pos > current_pos {
                        if res.has_error {
                            has_error = true;
                        }
                        current_pos = res.pos;
                        progressed = true;
                        break;
                    }
                    self.alloc
                        .get_node_mut(current_node)
                        .children
                        .truncate(saved_children_len);
                }
            }

            if !progressed {
                break;
            }
        }

        if should_create_node {
            self.finalize_node(current_node, node_id);
        }

        let mut res = ParseResult::ok(current_pos);
        if has_error {
            res = res.with_error();
        }
        res
    }

    fn parse_field(
        &mut self,
        node_id: usize,
        rule_ix: usize,
        name: &'static str,
        child_state: usize,
        pos: usize,
    ) -> ParseResult {
        let field_node = self.alloc.alloc(Tag::new_field(rule_ix, name), vec![], 0);
        let res = self.parse(field_node, child_state, pos, Some(rule_ix), false);

        if !res.ok {
            return ParseResult::failed(pos);
        }

        self.finalize_node(field_node, node_id);
        res
    }

    fn sequence_is_structural(&self, children: &[usize]) -> bool {
        children
            .iter()
            .any(|&child| !matches!(self.grammar.analysis.states[child], State::Tok(_, _)))
    }

    fn attempt_recovery(&mut self, node_id: usize, pos: usize) -> Option<usize> {
        if pos >= self.text.len() {
            return None;
        }

        let current_indent = self.specs.as_ref().map(|s| s.indent_at(pos)).unwrap_or(0);

        let mut candidates = Vec::new();
        if let Some(p) = self.recover_current_structure(pos, current_indent) {
            candidates.push(p);
        }
        if let Some(p) = self.recover_previous_structure(pos, current_indent) {
            candidates.push(p);
        }
        if let Some(p) = self.recover_siblings(pos, current_indent) {
            candidates.push(p);
        }
        if let Some(p) = self.recover_parent(pos, current_indent) {
            candidates.push(p);
        }
        if let Some(p) = self.recover_sync(pos) {
            candidates.push(p);
        }
        candidates.extend(self.recover_bridge(pos));

        let recovered_pos = candidates.into_iter().filter(|&p| p > pos).min();

        if let Some(recovered_pos) = recovered_pos {
            if recovered_pos > pos {
                let error_width = recovered_pos - pos;
                let error_node = self.alloc.alloc(
                    Tag::new_error(ParsecError::UnexpectedToken),
                    vec![],
                    error_width,
                );
                self.alloc.get_node_mut(node_id).children.push(error_node);
                return Some(recovered_pos);
            }
        }

        None
    }

    fn recover_current_structure(&self, pos: usize, current_indent: usize) -> Option<usize> {
        self.specs
            .as_ref()
            .and_then(|s| s.forward_skip_to_decrease(pos, current_indent))
    }

    fn recover_previous_structure(&self, pos: usize, current_indent: usize) -> Option<usize> {
        self.specs
            .as_ref()
            .and_then(|s| s.backward_skip_to_decrease(pos, current_indent))
    }

    fn recover_siblings(&self, pos: usize, current_indent: usize) -> Option<usize> {
        let specs = self.specs.as_ref()?;
        for region in specs.regions.iter() {
            if region.indent == current_indent && region.end > pos {
                return Some(region.end);
            }
        }
        None
    }

    fn recover_parent(&self, pos: usize, current_indent: usize) -> Option<usize> {
        if current_indent == 0 {
            return None;
        }

        let specs = self.specs.as_ref()?;
        for region in specs.regions.iter() {
            if region.indent < current_indent && region.end > pos {
                return Some(region.end);
            }
        }

        specs.forward_skip_to_decrease(pos, 0)
    }

    fn recover_sync(&self, pos: usize) -> Option<usize> {
        if self.specs.is_none() {
            return None;
        }

        let strategy = &self.specs.as_ref().unwrap().strategy;
        if let Some(sync_pos) = strategy.find_sync_point(self.text, pos) {
            if sync_pos > pos {
                return Some(sync_pos);
            }
            return strategy.find_sync_point(self.text, (pos + 1).min(self.text.len()));
        }
        None
    }

    fn recover_bridge(&self, pos: usize) -> Vec<usize> {
        let bridge = &self.grammar.analysis.bridge;
        let mut candidates = Vec::new();
        let mut cursor = pos;
        let max_scan = 1000;
        let end_pos = (pos + max_scan).min(self.text.len());

        while cursor < end_pos {
            let mut matched_island = false;

            // Check for Scope Open
            for scope in &bridge.scopes {
                if self.text[cursor..].starts_with(&scope.open) {
                    candidates.push(cursor);
                    // Try to skip island
                    if let Some(after_island) = self.scan_past_scope(cursor, scope) {
                        candidates.push(after_island);
                        cursor = after_island;
                    } else {
                        cursor += scope.open.len();
                    }
                    matched_island = true;
                    break;
                }
            }
            if matched_island {
                continue;
            }

            // Check for Scope Close
            for scope in &bridge.scopes {
                if self.text[cursor..].starts_with(&scope.close) {
                    candidates.push(cursor);
                    cursor += scope.close.len();
                    matched_island = true;
                    break;
                }
            }
            if matched_island {
                continue;
            }

            // Check for Reefs
            for reef in &bridge.reefs {
                if self.text[cursor..].starts_with(reef) {
                    candidates.push(cursor);
                    cursor += reef.len();
                    matched_island = true;
                    break;
                }
            }
            if matched_island {
                continue;
            }

            cursor += 1;
        }
        candidates
    }

    fn scan_past_scope(&self, start: usize, scope: &Scope) -> Option<usize> {
        let mut pos = start + scope.open.len();
        let mut depth = 1;

        while pos < self.text.len() && depth > 0 {
            if self.text[pos..].starts_with(&scope.open) {
                depth += 1;
                pos += scope.open.len();
            } else if self.text[pos..].starts_with(&scope.close) {
                depth -= 1;
                pos += scope.close.len();
            } else {
                pos += 1;
            }
        }

        if depth == 0 { Some(pos) } else { None }
    }

    fn at_list_end(&self, pos: usize) -> bool {
        let mut i = pos;
        while i < self.text.len() {
            let b = self.text.as_bytes()[i];
            if b == b' ' || b == b'\n' || b == b'\r' || b == b'\t' {
                i += 1;
                continue;
            }
            return b == b'}' || b == b']';
        }
        true
    }

    fn finalize_node(&mut self, work_node: usize, parent_node: usize) {
        // Calculate total width from children
        self.update_width(work_node);
        self.alloc
            .get_node_mut(parent_node)
            .children
            .push(work_node);
    }

    fn update_width(&mut self, node_id: usize) {
        let total_width: usize = self
            .alloc
            .get_node(node_id)
            .children
            .iter()
            .map(|&child_id| self.alloc.get_node(child_id).width)
            .sum();

        self.alloc.get_node_mut(node_id).width = total_width;
    }

    fn is_literal(&self, state_id: usize) -> bool {
        match &self.grammar.analysis.states[state_id] {
            State::Tok(_, matcher) => matcher.preview().is_some(),
            _ => false,
        }
    }

    fn contains_valid_token(&self, node_id: usize) -> bool {
        let node = self.alloc.get_node(node_id);
        match &node.tag {
            Tag::Token { .. } => true,
            Tag::Error(_) => false,
            // Traverse children for Rules and Fields
            _ => node
                .children
                .iter()
                .any(|&child| self.contains_valid_token(child)),
        }
    }
}
