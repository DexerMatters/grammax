use dashmap::DashSet;

use crate::{
    grammar::{
        Grammar, analysis,
        ir::{Scope, State},
        recovery::{ErrorRecoveryStrategy, RecoverySpecs},
    },
    parsec::{
        msg::{ParserMessage, ParserMessages},
        tree::{ParsecError, RedNode, Tag, TreeAlloc, TreeAllocRef, TreeAllocRefExt},
        words::Matcher,
    },
    utils::{LruCache, Span},
};
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    fmt,
    hash::{Hash, Hasher},
    ops::DerefMut,
    ptr::NonNull,
    rc::Rc,
    sync::Arc,
};

pub struct ParserConfig {
    pub recover: bool,
    pub memo_capacity: usize, // 0 = disabled, >0 = LRU cache size

    // Hooks
    pub on_start_parse: Option<Box<dyn Fn(&Parser) + Send>>,
    pub on_finish_parse: Option<Box<dyn Fn(&Parser) + Send>>,
    pub on_computation: Option<Box<dyn Fn(&Parser) + Send>>,
    pub on_memo_hit: Option<Box<dyn Fn(&Parser) + Send>>,
    pub on_node_reuse: Option<Box<dyn Fn(&Parser) + Send>>,
    pub on_recovery_attempt: Option<Box<dyn Fn(&Parser) + Send>>,
    pub on_recovery_success: Option<Box<dyn Fn(&Parser) + Send>>,
    pub on_recovery_failure: Option<Box<dyn Fn(&Parser) + Send>>,
}

impl fmt::Debug for ParserConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ParserConfig")
            .field("recover", &self.recover)
            .field("memo_capacity", &self.memo_capacity)
            .finish()
    }
}

#[derive(Debug, Clone)]
pub struct Result {
    pub root: RedNode,
    pub messages: ParserMessages,
}

impl ParserConfig {
    pub fn new() -> Self {
        Self {
            recover: false,
            memo_capacity: 0,
            on_start_parse: None,
            on_finish_parse: None,
            on_computation: None,
            on_memo_hit: None,
            on_node_reuse: None,
            on_recovery_attempt: None,
            on_recovery_success: None,
            on_recovery_failure: None,
        }
    }

    pub fn recovering() -> Self {
        Self {
            recover: true,
            memo_capacity: 0,
            ..Self::new()
        }
    }

    pub fn recovering_with_memo(capacity: usize) -> Self {
        Self {
            recover: true,
            memo_capacity: capacity,
            ..Self::new()
        }
    }

    pub fn with_memo(mut self, capacity: usize) -> Self {
        self.memo_capacity = capacity;
        self
    }

    pub fn with_on_start_parse_hook<F>(mut self, hook: F) -> Self
    where
        F: Fn(&Parser) + Send + 'static,
    {
        self.on_start_parse = Some(Box::new(hook));
        self
    }

    pub fn with_on_finish_parse_hook<F>(mut self, hook: F) -> Self
    where
        F: Fn(&Parser) + Send + 'static,
    {
        self.on_finish_parse = Some(Box::new(hook));
        self
    }

    pub fn with_on_computation_hook<F>(mut self, hook: F) -> Self
    where
        F: Fn(&Parser) + Send + 'static,
    {
        self.on_computation = Some(Box::new(hook));
        self
    }

    pub fn with_on_memo_hit_hook<F>(mut self, hook: F) -> Self
    where
        F: Fn(&Parser) + Send + 'static,
    {
        self.on_memo_hit = Some(Box::new(hook));
        self
    }

    pub fn with_on_node_reuse_hook<F>(mut self, hook: F) -> Self
    where
        F: Fn(&Parser) + Send + 'static,
    {
        self.on_node_reuse = Some(Box::new(hook));
        self
    }

    pub fn with_on_recovery_attempt_hook<F>(mut self, hook: F) -> Self
    where
        F: Fn(&Parser) + Send + 'static,
    {
        self.on_recovery_attempt = Some(Box::new(hook));
        self
    }

    pub fn with_on_recovery_success_hook<F>(mut self, hook: F) -> Self
    where
        F: Fn(&Parser) + Send + 'static,
    {
        self.on_recovery_success = Some(Box::new(hook));
        self
    }

    pub fn with_on_recovery_failure_hook<F>(mut self, hook: F) -> Self
    where
        F: Fn(&Parser) + Send + 'static,
    {
        self.on_recovery_failure = Some(Box::new(hook));
        self
    }
}

#[derive(Debug)]
pub struct Parser {
    pub(crate) text: Arc<str>,
    pub(crate) grammar: Grammar,
    pub(crate) alloc: TreeAllocRef,
    pub(crate) messages: ParserMessages,
    in_flight: DashSet<ParseKey>,
    probe_in_flight: DashSet<ProbeKey>,
    probe_memo: Option<LruCache<ProbeKey, ParseResult>>,
    node_cache: HashMap<NodeCacheKey, CachedNode>,
    config: ParserConfig,
    specs: Option<RecoverySpecs>,
    recovery_strategy: Option<ErrorRecoveryStrategy>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct ParseKey {
    state_id: usize,
    pos: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct ProbeKey {
    state_id: usize,
    pos: usize,
    parent_is_sequence: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct NodeCacheKey {
    rule_ix: usize,
    pos: usize,
    width: usize,
    content_hash: u64,
}

#[derive(Debug, Clone, Copy)]
struct CachedNode {
    node_id: usize,
    pos: usize,
    width: usize,
}

#[derive(Debug, Clone, Copy)]
struct ParseResult {
    pos: usize,
    ok: bool,
    has_error: bool,
}

impl ParseResult {
    fn ok(pos: usize) -> Self {
        Self {
            pos,
            ok: true,
            has_error: false,
        }
    }
    fn failed(pos: usize) -> Self {
        Self {
            pos,
            ok: false,
            has_error: false,
        }
    }
    fn recovered(pos: usize) -> Self {
        Self {
            pos,
            ok: true,
            has_error: true,
        }
    }
    fn with_error(mut self) -> Self {
        self.has_error = true;
        self
    }
}

impl Parser {
    pub fn new(grammar: Grammar) -> Parser {
        Self::new_with_config_and_alloc(
            grammar,
            ParserConfig::new(),
            Rc::new(RefCell::new(TreeAlloc::new())),
        )
    }

    pub fn new_with_config(grammar: Grammar, config: ParserConfig) -> Parser {
        Self::new_with_config_and_alloc(grammar, config, Rc::new(RefCell::new(TreeAlloc::new())))
    }

    fn new_with_config_and_alloc(
        grammar: Grammar,
        config: ParserConfig,
        alloc: TreeAllocRef,
    ) -> Parser {
        let probe_memo = if config.memo_capacity > 0 {
            Some(LruCache::new(config.memo_capacity))
        } else {
            None
        };
        let recovery_strategy = if config.recover {
            Some(ErrorRecoveryStrategy::from_grammar(&grammar))
        } else {
            None
        };
        Self {
            text: Arc::from(""),
            grammar,
            alloc,
            messages: Vec::new(),
            in_flight: DashSet::new(),
            probe_in_flight: DashSet::new(),
            probe_memo,
            node_cache: HashMap::new(),
            config,
            specs: None,
            recovery_strategy,
        }
    }

    pub fn parse_text(&mut self, text: &str) -> Result {
        self.reset_for_text(text);
        let start_state = self.grammar.analysis.start_state;

        let mut root = RedNode::new_root(self.alloc.clone(), &self.text);
        let root_green = root.green;

        self.config.on_start_parse.as_ref().map(|hook| hook(self));
        // Treat root as if it's in a sequence to enable top-level recovery
        self.parse(root_green, start_state, 0, None, true);
        self.config.on_finish_parse.as_ref().map(|hook| hook(self));

        let green = self.alloc.get_node(root_green);

        if green.children.len() == 1 {
            if let Some(&child) = green.children.first() {
                root.green = child;
            }
        } else if green.children.len() > 1 {
            // Find the valid rule node among debris
            let start_rule_ix = self.grammar.analysis.states[start_state].ref_ix();
            let valid_child =
                green
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

        Result {
            root,
            messages: self.messages.clone(),
        }
    }

    pub fn parse_rule(&mut self, rule_ix: usize, pos: usize) -> Option<usize> {
        self.messages.clear();
        let state_id = self
            .grammar
            .analysis
            .states
            .iter()
            .position(|s| s.ref_ix() == rule_ix)?;

        // Check node cache first
        if let Some(cached) = self.try_reuse_node(rule_ix, pos) {
            if let Some(hook) = &self.config.on_node_reuse {
                hook(self);
            }
            return Some(cached.node_id);
        }

        let node_id = self.alloc.alloc(Tag::new_rule(rule_ix), vec![], 0);

        self.config.on_start_parse.as_ref().map(|hook| hook(self));
        let res = self.parse(node_id, state_id, pos, None, true);
        self.config.on_finish_parse.as_ref().map(|hook| hook(self));

        if res.ok {
            self.recompute_width(node_id);
            let width = self.alloc.get_node(node_id).width;

            // Cache the successfully parsed node
            if width > 0 {
                self.cache_node(rule_ix, pos, width, node_id);
            }

            let green = self.alloc.get_node(node_id);
            if green.children.len() == 1 {
                let child = green.children[0];
                let child_node = self.alloc.get_node(child);
                if let Tag::Rule { rule_ix: child_ix } = &child_node.tag {
                    if *child_ix == rule_ix {
                        return Some(child);
                    }
                }
            }
            Some(node_id)
        } else {
            None
        }
    }

    pub fn apply_edit(&mut self, text: &str, start: usize, old_len: usize, new_len: usize) {
        self.text = Arc::from(text);
        self.messages.clear();
        self.in_flight.clear();
        self.probe_in_flight.clear();

        // Update probe memo using temporal memoization strategy (shift and retain)
        if let Some(memo) = &self.probe_memo {
            memo.rebuild(|mut key, mut res| {
                let k_pos = key.pos;
                let v_pos = res.pos;

                if k_pos < start {
                    if v_pos <= start {
                        Some((key, res))
                    } else {
                        None // Invalidated: crossed edit boundary
                    }
                } else if k_pos >= start + old_len {
                    // Shift positions
                    let shift = new_len as isize - old_len as isize;
                    let new_k_pos = (k_pos as isize + shift) as usize;
                    let new_v_pos = (v_pos as isize + shift) as usize;

                    key.pos = new_k_pos;
                    res.pos = new_v_pos;
                    Some((key, res))
                } else {
                    None // Invalidated: inside edited range
                }
            });
        }

        // Update node cache: invalidate affected, shift unaffected
        let shift = new_len as isize - old_len as isize;
        let mut new_cache = HashMap::new();

        for (key, cached) in self.node_cache.drain() {
            let node_end = cached.pos + cached.width;

            if node_end <= start {
                // Before edit: keep as-is
                new_cache.insert(key, cached);
            } else if cached.pos > start + old_len {
                // After edit (strict inequality): shift position and recompute hash
                let new_pos = (cached.pos as isize + shift) as usize;
                if new_pos < self.text.len() && new_pos + cached.width <= self.text.len() {
                    let new_hash = Self::hash_text_range(&self.text, new_pos, cached.width);
                    let new_key = NodeCacheKey {
                        rule_ix: key.rule_ix,
                        pos: new_pos,
                        width: cached.width,
                        content_hash: new_hash,
                    };
                    let new_cached = CachedNode {
                        node_id: cached.node_id,
                        pos: new_pos,
                        width: cached.width,
                    };
                    new_cache.insert(new_key, new_cached);
                }
            }
            // Overlapping nodes are discarded (not inserted into new_cache)
        }

        self.node_cache = new_cache;

        self.specs = self
            .recovery_strategy
            .clone()
            .map(|strategy| RecoverySpecs::from_text_with_strategy(&self.text, strategy));
    }

    pub fn reset_for_text(&mut self, text: &str) {
        self.text = Arc::from(text);
        self.messages.clear();
        self.in_flight.clear();
        self.probe_in_flight.clear();
        if let Some(memo) = &self.probe_memo {
            memo.clear();
        }
        self.node_cache.clear();
        self.specs = self
            .recovery_strategy
            .clone()
            .map(|strategy| RecoverySpecs::from_text_with_strategy(&self.text, strategy));
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn recovery_specs(&self) -> Option<&RecoverySpecs> {
        self.specs.as_ref()
    }

    pub fn recovery_strategy(&self) -> Option<&ErrorRecoveryStrategy> {
        self.recovery_strategy.as_ref()
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

        // Check for cycles
        if self.in_flight.contains(&key) {
            return ParseResult::failed(pos);
        }

        self.in_flight.insert(key);
        let res = self.parse_inner(node_id, state_id, pos, last_rule_ix, parent_is_sequence);
        self.in_flight.remove(&key);

        res
    }

    fn probe(&mut self, state_id: usize, pos: usize, parent_is_sequence: bool) -> ParseResult {
        let key = ProbeKey {
            state_id,
            pos,
            parent_is_sequence,
        };

        // Check memo first - if hit, we skip computation
        if let Some(cached) = self.probe_memo.as_ref().and_then(|m| m.get(&key)) {
            // Memo hit - call hook
            if let Some(hook) = &self.config.on_memo_hit {
                hook(self);
            }
            return cached;
        }

        if !self.probe_in_flight.insert(key) {
            return ParseResult::failed(pos);
        }

        let res = self.probe_inner(state_id, pos, parent_is_sequence);
        self.probe_in_flight.remove(&key);

        if let Some(memo) = &self.probe_memo {
            memo.insert(key, res);
        }
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

        let analysis = self.grammar.analysis.clone();
        let state = &analysis.states[state_id];
        let current_rule_ix = state.ref_ix();

        // Only create a new AST node when the rule changes and the rule is named
        let is_trivial_rule = self.grammar.name(current_rule_ix).starts_with('@');
        let should_create_node = !is_trivial_rule;

        let result = match state {
            State::Tok(rule_ix, matcher) => {
                // Only create a wrapper node if:
                // 1. should_create_node is true (not a trivial rule)
                // 2. The token's rule is different from parent (indicates rule boundary)
                let needs_wrapper = should_create_node && Some(*rule_ix) != last_rule_ix;
                if needs_wrapper {
                    let work_node = self.make_work_node(node_id, *rule_ix, true);
                    let res = self.parse_token(work_node, *rule_ix, matcher.as_ref(), pos);
                    if res.ok {
                        self.finalize_node(work_node, node_id);
                    }
                    res
                } else {
                    self.parse_token(node_id, *rule_ix, matcher.as_ref(), pos)
                }
            }

            State::Seq(rule_ix, children) => self.parse_sequence(
                node_id,
                *rule_ix,
                children,
                pos,
                should_create_node,
                parent_is_sequence,
            ),

            State::Alt(rule_ix, children, has_epsilon) => self.parse_alternative(
                node_id,
                state_id,
                *rule_ix,
                children,
                *has_epsilon,
                pos,
                should_create_node,
                parent_is_sequence,
            ),

            State::Field(rule_ix, name, child) => {
                self.parse_field(node_id, *rule_ix, name, *child, pos)
            }

            State::LeftRec(rule_ix, base, tail, tail_fields) => self.parse_left_rec(
                node_id,
                *rule_ix,
                base,
                tail,
                tail_fields,
                pos,
                should_create_node,
            ),
        };

        if !result.ok && self.config.recover && last_rule_ix.is_none() && should_create_node {
            if let Some(recovered_pos) = self.attempt_recovery(node_id, pos, vec![state_id]) {
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
    ) -> ParseResult {
        let mut current_pos = pos;
        match matcher.matches(&self.text, &mut current_pos) {
            Some(0) => ParseResult::ok(current_pos),
            Some(width) => {
                if let Some(hook) = &self.config.on_computation {
                    hook(self);
                }
                let token_id = self.alloc.alloc_token(Tag::new_token(rule_ix), width);
                self.alloc.get_node_mut(node_id).children.push(token_id);
                ParseResult::ok(current_pos)
            }
            None => ParseResult::failed(pos),
        }
    }

    /// Parse n-ary sequence (all children in sequence)
    fn parse_sequence(
        &mut self,
        node_id: usize,
        rule_ix: usize,
        children: &Vec<usize>,
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
        let is_rep_tail = rule_name.starts_with("@rep");
        let is_list_tail = is_sep_tail || is_rep_tail;

        let mut idx = 0;
        while idx < children.len() {
            let child_state = children[idx];
            if is_list_tail && idx == 0 && self.at_list_end(current_pos) {
                self.truncate_children(work_node, saved_children_len);
                return ParseResult::failed(pos);
            }
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
                        let next_res = self.probe(next_child, current_pos, true);
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
                        if let Some(recovered_pos) =
                            self.recover_literal_ahead(work_node, current_pos, child_state)
                        {
                            current_pos = recovered_pos;
                            has_error = true;
                            continue;
                        }

                        self.push_error(
                            work_node,
                            ParsecError::MissingToken,
                            current_pos,
                            0,
                            vec![child_state],
                        );
                        has_error = true;
                        idx += 1;
                        continue;
                    }
                }
            }
            // ----------------------------------------

            let mut recovered = None;
            let committed = (current_pos > pos) && self.config.recover && idx > 0;
            let expected_ix = child_state;

            // Recover if structural (safe at start) or committed (safe to finish)
            if allow_recovery || committed {
                // Call recovery attempt hook
                if let Some(hook) = &self.config.on_recovery_attempt {
                    hook(self);
                }
                recovered = self.attempt_recovery(work_node, current_pos, vec![expected_ix]);
                // Call recovery success or failure hook
                if recovered.is_some() {
                    if let Some(hook) = &self.config.on_recovery_success {
                        hook(self);
                    }
                } else {
                    if let Some(hook) = &self.config.on_recovery_failure {
                        hook(self);
                    }
                }
            }
            if recovered.is_none()
                && self.config.recover
                && is_sep_tail
                && !self.at_list_end(current_pos)
            {
                // Call recovery attempt hook
                if let Some(hook) = &self.config.on_recovery_attempt {
                    hook(self);
                }
                recovered = self
                    .recover_sep_tail(work_node, current_pos, expected_ix)
                    .or_else(|| self.attempt_recovery(work_node, current_pos, vec![expected_ix]));
                // Call recovery success or failure hook
                if recovered.is_some() {
                    if let Some(hook) = &self.config.on_recovery_success {
                        hook(self);
                    }
                } else {
                    if let Some(hook) = &self.config.on_recovery_failure {
                        hook(self);
                    }
                }
            }

            // Fallback: Panic mode (skip one character) if committed or structural
            if recovered.is_none() && (committed || allow_recovery) {
                // Call recovery attempt hook for panic mode
                if let Some(hook) = &self.config.on_recovery_attempt {
                    hook(self);
                }
                if let Some(c) = self.text[current_pos..].chars().next() {
                    let w = c.len_utf8();
                    self.push_error(
                        work_node,
                        ParsecError::UnexpectedToken,
                        current_pos,
                        w,
                        vec![expected_ix],
                    );
                    recovered = Some(current_pos + w);
                    // Panic mode is a recovery success
                    if let Some(hook) = &self.config.on_recovery_success {
                        hook(self);
                    }
                } else {
                    // Panic mode failed
                    if let Some(hook) = &self.config.on_recovery_failure {
                        hook(self);
                    }
                }
            }

            if let Some(recovered_pos) = recovered {
                if recovered_pos > current_pos {
                    if is_sep_tail && self.at_list_end(recovered_pos) {
                        current_pos = recovered_pos;
                        has_error = true;
                        idx = children.len();
                        continue;
                    }
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

            self.truncate_children(work_node, saved_children_len);
            return ParseResult::failed(pos);
        }

        if should_create_node {
            self.finalize_node(work_node, node_id);
        }

        if !has_error {
            has_error = self
                .alloc
                .get_node(work_node)
                .children
                .iter()
                .any(|&child| self.alloc.get_node(child).tag.is_error());
        }

        let mut res = ParseResult::ok(current_pos);
        if has_error {
            res = res.with_error();
        }
        res
    }

    fn probe_inner(
        &mut self,
        state_id: usize,
        pos: usize,
        parent_is_sequence: bool,
    ) -> ParseResult {
        let analysis = self.grammar.analysis.clone();
        let state = &analysis.states[state_id];
        match state {
            State::Tok(_, matcher) => self.probe_token(matcher.as_ref(), pos),

            State::Seq(_rule_ix, children) => {
                self.probe_sequence(children, pos, parent_is_sequence)
            }

            State::Alt(_rule_ix, children, has_epsilon) => {
                self.probe_alternative(children, *has_epsilon, pos, parent_is_sequence)
            }

            State::Field(_rule_ix, _name, child) => self.probe(*child, pos, parent_is_sequence),

            State::LeftRec(_rule_ix, base, tail, _tail_fields) => {
                self.probe_left_rec(base, tail, pos)
            }
        }
    }

    fn probe_token(&self, matcher: &dyn Matcher, pos: usize) -> ParseResult {
        let mut current_pos = pos;
        let matched = if current_pos <= self.text.len() {
            matcher.matches(&self.text, &mut current_pos)
        } else {
            None
        };
        if let Some(width) = matched {
            if width == 0 {
                return ParseResult::ok(current_pos);
            }
            ParseResult::ok(current_pos)
        } else {
            ParseResult::failed(pos)
        }
    }

    fn probe_sequence(
        &mut self,
        children: &Vec<usize>,
        pos: usize,
        parent_is_sequence: bool,
    ) -> ParseResult {
        let mut current_pos = pos;
        let mut has_error = false;

        for child_state in children {
            let res = self.probe(*child_state, current_pos, parent_is_sequence);
            if !res.ok {
                return ParseResult::failed(pos);
            }
            current_pos = res.pos;
            if res.has_error {
                has_error = true;
            }
        }

        let mut res = ParseResult::ok(current_pos);
        if has_error {
            res = res.with_error();
        }
        res
    }

    fn probe_alternative(
        &mut self,
        children: &Vec<usize>,
        has_epsilon: bool,
        pos: usize,
        parent_is_sequence: bool,
    ) -> ParseResult {
        for child_state in children {
            let res = self.probe(*child_state, pos, parent_is_sequence);
            if res.ok && res.pos >= pos {
                return res;
            }
        }

        if has_epsilon {
            return ParseResult::ok(pos);
        }

        ParseResult::failed(pos)
    }

    fn probe_left_rec(
        &mut self,
        base_states: &Vec<usize>,
        tail_states: &Vec<usize>,
        pos: usize,
    ) -> ParseResult {
        let mut current_pos = pos;
        let mut base_ok = false;
        let mut has_error = false;

        for base_state in base_states {
            let res = self.probe(*base_state, pos, false);
            if res.ok {
                current_pos = res.pos;
                if res.has_error {
                    has_error = true;
                }
                base_ok = true;
                break;
            }
        }

        if !base_ok {
            current_pos = pos;
        }

        loop {
            let mut progressed = false;

            for tail_state in tail_states.iter().copied() {
                let res = self.probe(tail_state, current_pos, false);
                if res.ok && res.pos > current_pos {
                    if res.has_error {
                        has_error = true;
                    }
                    current_pos = res.pos;
                    progressed = true;
                    break;
                }
            }

            if !progressed {
                break;
            }
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
        state_id: usize,
        rule_ix: usize,
        children: &Vec<usize>,
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

            self.truncate_children(work_node, saved_len);
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
            if let Some(recovered_pos) = self.attempt_recovery(work_node, pos, vec![state_id]) {
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
        base_states: &Vec<usize>,
        tail_states: &Vec<usize>,
        tail_fields: &Vec<Option<&'static str>>,
        pos: usize,
        should_create_node: bool,
    ) -> ParseResult {
        let work_node = self.make_work_node(node_id, rule_ix, should_create_node);

        let mut current_pos = pos;
        let mut base_ok = false;
        let mut has_error = false;

        for base_state in base_states {
            let saved_children_len = self.alloc.get_node(work_node).children.len();
            let res = self.parse_child(work_node, *base_state, pos, rule_ix, false);
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
            self.recompute_width(work_node);
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
                        self.recompute_width(field_node);
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
                        self.recompute_width(new_node);
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

    fn attempt_recovery(
        &mut self,
        node_id: usize,
        pos: usize,
        expected: Vec<usize>,
    ) -> Option<usize> {
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
                self.push_error(
                    node_id,
                    ParsecError::UnexpectedToken,
                    pos,
                    recovered_pos - pos,
                    expected,
                );
                return Some(recovered_pos);
            }
        }

        None
    }

    fn recover_sep_tail(
        &mut self,
        node_id: usize,
        pos: usize,
        comma_state: usize,
    ) -> Option<usize> {
        let mut candidates: Vec<usize> = Vec::new();

        if let State::Tok(_, matcher) = &self.grammar.analysis.states[comma_state] {
            if let Some(lit) = matcher.preview() {
                if let Some(idx) = self.text[pos..].find(&lit) {
                    candidates.push(pos + idx);
                }
            }
        }

        for scope in &self.grammar.analysis.bridge.scopes {
            if scope.close.is_empty() {
                continue;
            }
            if let Some(idx) = self.text[pos..].find(&scope.close) {
                candidates.push(pos + idx);
            }
        }

        let recovered_pos = candidates.into_iter().filter(|&p| p > pos).min();
        if let Some(recovered_pos) = recovered_pos {
            self.push_error(
                node_id,
                ParsecError::UnexpectedToken,
                pos,
                recovered_pos - pos,
                vec![comma_state],
            );
            return Some(recovered_pos);
        }

        None
    }

    fn recover_literal_ahead(
        &mut self,
        node_id: usize,
        pos: usize,
        state_id: usize,
    ) -> Option<usize> {
        let State::Tok(_, matcher) = &self.grammar.analysis.states[state_id] else {
            return None;
        };
        let Some(lit) = matcher.preview() else {
            return None;
        };
        let Some(idx) = self.text[pos..].find(&lit) else {
            return None;
        };

        let recovered_pos = pos + idx;
        if recovered_pos <= pos {
            return None;
        }

        self.push_error(
            node_id,
            ParsecError::UnexpectedToken,
            pos,
            recovered_pos - pos,
            vec![state_id],
        );
        Some(recovered_pos)
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
        if let Some(sync_pos) = strategy.find_sync_point(&self.text, pos) {
            if sync_pos > pos {
                return Some(sync_pos);
            }
            return strategy.find_sync_point(&self.text, (pos + 1).min(self.text.len()));
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
        let remaining = self.text[pos..].trim_start();
        if remaining.is_empty() {
            return true;
        }

        self.grammar
            .analysis
            .bridge
            .scopes
            .iter()
            .filter(|scope| !scope.close.is_empty())
            .any(|scope| remaining.starts_with(&scope.close))
    }

    fn finalize_node(&mut self, work_node: usize, parent_node: usize) {
        if let Some(hook) = &self.config.on_computation {
            hook(self);
        }
        self.recompute_width(work_node);
        self.alloc
            .get_node_mut(parent_node)
            .children
            .push(work_node);
    }

    fn is_literal(&self, state_id: usize) -> bool {
        match &self.grammar.analysis.states[state_id] {
            State::Tok(_, matcher) => matcher.preview().is_some(),
            _ => false,
        }
    }

    fn create_error_node(&mut self, error: ParsecError, width: usize) -> usize {
        self.alloc.alloc(Tag::new_error(error), vec![], width)
    }

    fn push_error(
        &mut self,
        node_id: usize,
        error: ParsecError,
        pos: usize,
        width: usize,
        expected: Vec<usize>,
    ) {
        let error_node = self.create_error_node(error, width);
        self.alloc.get_node_mut(node_id).children.push(error_node);
        match error {
            ParsecError::UnexpectedToken => {
                let span = Span::new_len(pos, width);
                self.messages
                    .push(ParserMessage::new_unexpected(span, expected));
            }
            ParsecError::MissingToken => {
                let span = Span::new_len(pos, 0);
                self.messages
                    .push(ParserMessage::new_missing(span, expected));
            }
            _ => {}
        }
    }

    fn truncate_children(&mut self, node_id: usize, len: usize) {
        self.alloc.get_node_mut(node_id).children.truncate(len);
    }

    fn recompute_width(&mut self, node_id: usize) {
        let total: usize = self
            .alloc
            .get_node(node_id)
            .children
            .iter()
            .map(|&child| self.alloc.get_node(child).width)
            .sum();
        self.alloc.get_node_mut(node_id).width = total;
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

    fn hash_text_range(text: &str, pos: usize, width: usize) -> u64 {
        use std::collections::hash_map::DefaultHasher;

        if pos + width > text.len() {
            return 0;
        }

        let slice = &text[pos..pos + width];
        let mut hasher = DefaultHasher::new();
        slice.hash(&mut hasher);
        hasher.finish()
    }

    fn try_reuse_node(&self, rule_ix: usize, pos: usize) -> Option<CachedNode> {
        // Check all cached nodes at this position with this rule
        for (key, cached) in self.node_cache.iter() {
            if key.rule_ix == rule_ix && cached.pos == pos {
                // Verify the text content still matches
                if pos + cached.width <= self.text.len() {
                    let current_hash = Self::hash_text_range(&self.text, pos, cached.width);
                    if current_hash == key.content_hash {
                        return Some(*cached);
                    }
                }
            }
        }
        None
    }

    fn cache_node(&mut self, rule_ix: usize, pos: usize, width: usize, node_id: usize) {
        if pos + width > self.text.len() {
            return;
        }

        let content_hash = Self::hash_text_range(&self.text, pos, width);
        let key = NodeCacheKey {
            rule_ix,
            pos,
            width,
            content_hash,
        };
        let cached = CachedNode {
            node_id,
            pos,
            width,
        };
        self.node_cache.insert(key, cached);
    }
}
