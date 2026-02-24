use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use rustc_hash::{FxHashMap, FxHashSet};

use crate::grammar::Grammar;
use crate::grammar::analysis::{Action, EOF_TOKEN, GrammarStateAnalysis};
use crate::grammar::ir::{NormalizedNode, Production, RuleInfo, Symbol};
use crate::grammar::recovery::{ErrorRecoveryStrategy, RecoverySpecs};
use crate::parsec::msg::{ParserMessage, ParserMessages};
use crate::parsec::recovery::{RecoveryCache, RecoveryConfig, RepairOp, recover};
use crate::parsec::tree::{GreenId, ParsecError, RedNode, Tag, TreeAllocRef, TreeAllocRefExt};
use crate::utils::{LruCache, Span};

const UNKNOWN_TOKEN: usize = usize::MAX - 1;
const DEFAULT_REUSE_CAPACITY: usize = 4096;

#[derive(Debug, Clone)]
pub struct ParserConfig {
    pub simple_ast: bool,
    pub recovery: RecoveryConfig,
}

impl ParserConfig {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Default for ParserConfig {
    fn default() -> Self {
        Self {
            simple_ast: true,
            recovery: RecoveryConfig::default(),
        }
    }
}

#[derive(Default)]
pub struct ParserListener {
    // Callbacks
}

pub struct Result {
    pub root: RedNode,
    pub messages: ParserMessages,
    pub semantic_commands: Vec<crate::semantic::Command>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct IncrementalReuseStats {
    pub lookups: usize,
    pub hits: usize,
    pub inserts: usize,
}

#[derive(Debug, Clone, Copy)]
struct StackEntry {
    node: GreenId,
    binds_state: bool,
}

#[derive(Debug, Clone, Default)]
struct RecoveryProfile {
    sync_tokens: FxHashSet<usize>,
    opening_tokens: FxHashSet<usize>,
    closing_tokens: FxHashSet<usize>,
    closing_to_opening: FxHashMap<usize, FxHashSet<usize>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ParseRuleCacheKey {
    rule_ix: usize,
    expected_width: usize,
    slice_hash: u64,
    simple_ast: bool,
}

#[derive(Debug, Clone)]
struct ParseRuleCacheEntry {
    slice: String,
    green: Option<GreenId>,
    relative_messages: ParserMessages,
}

pub struct Parser {
    pub grammar: Grammar,
    pub alloc: TreeAllocRef,
    pub messages: ParserMessages,
    pub newly_computed_nodes: Vec<Span>,
    pub newly_computed_tokens: Vec<Span>,

    config: ParserConfig,
    listener: Option<ParserListener>,

    text: String,
    pos: usize,

    // For incremental/reparsing (stubs)
    inc_insert_pos: Option<usize>,
    string_opened: bool,
    rule_analyses: FxHashMap<usize, Arc<GrammarStateAnalysis>>,
    recovery_profile: RecoveryProfile,
    reuse_enabled: bool,
    reuse_cache_failures: bool,
    reuse_cache: LruCache<ParseRuleCacheKey, ParseRuleCacheEntry>,
    reuse_stats: IncrementalReuseStats,
    recovery_cache: RecoveryCache,
}

impl Parser {
    pub fn new(grammar: Grammar) -> Self {
        let recovery_profile = Self::build_recovery_profile(&grammar);
        Self {
            grammar,
            alloc: TreeAllocRef::create(),
            messages: vec![],
            newly_computed_nodes: vec![],
            newly_computed_tokens: vec![],
            config: ParserConfig::default(),
            listener: None,
            text: String::new(),
            pos: 0,
            inc_insert_pos: None,
            string_opened: false,
            rule_analyses: FxHashMap::default(),
            recovery_profile,
            reuse_enabled: true,
            reuse_cache_failures: true,
            reuse_cache: LruCache::new(DEFAULT_REUSE_CAPACITY),
            reuse_stats: IncrementalReuseStats::default(),
            recovery_cache: RecoveryCache::default(),
        }
    }

    pub fn with_config(mut self, config: ParserConfig) -> Self {
        self.config = config;
        self
    }

    pub fn with_listener(mut self, listener: ParserListener) -> Self {
        self.listener = Some(listener);
        self
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn recovery_specs(&self) -> Option<&RecoverySpecs> {
        None // TODO
    }

    pub fn recovery_strategy(&self) -> Option<&ErrorRecoveryStrategy> {
        None // TODO
    }

    pub fn newly_computed_nodes(&self) -> Vec<Span> {
        self.newly_computed_nodes.clone()
    }

    pub fn newly_computed_tokens(&self) -> Vec<Span> {
        self.newly_computed_tokens.clone()
    }

    pub fn set_insert_pos(&mut self, pos: Option<usize>) {
        self.inc_insert_pos = pos;
    }

    pub fn set_text(&mut self, text: &str) {
        self.text = text.to_string();
    }

    pub fn configure_reuse(&mut self, enabled: bool, cache_capacity: usize, cache_failures: bool) {
        self.reuse_enabled = enabled;
        self.reuse_cache_failures = cache_failures;

        let new_capacity = cache_capacity.max(1);
        if self.reuse_cache.capacity() != new_capacity {
            self.reuse_cache = LruCache::new(new_capacity);
        }
    }

    pub fn clear_reuse_cache(&mut self) {
        self.reuse_cache.clear();
    }

    pub fn reset_reuse_stats(&mut self) {
        self.reuse_stats = IncrementalReuseStats::default();
    }

    pub fn reuse_stats(&self) -> IncrementalReuseStats {
        self.reuse_stats
    }

    pub fn parse_text(&mut self, text: &str) -> Result {
        self.text = text.to_string();
        self.pos = 0;
        self.messages.clear();
        self.newly_computed_nodes.clear();
        self.newly_computed_tokens.clear();
        self.string_opened = false;
        self.recovery_cache.clear();

        // LR Parsing Loop
        let start_state = self.grammar.analysis.start_state;
        let mut state_stack = vec![start_state];
        let mut node_stack: Vec<StackEntry> = vec![];
        let mut recovery_active = false;

        loop {
            let current_state_idx = *state_stack.last().unwrap();
            let expected_ids: Vec<usize> =
                self.expected_ids_for_analysis(&self.grammar.analysis, current_state_idx, false);
            let (term_idx, token_len, token_node) = self.lex(Some(&expected_ids));

            let action = self.grammar.analysis.states[current_state_idx]
                .actions
                .get(&term_idx)
                .cloned();

            if let Some(action) = action {
                match action {
                    Action::Shift(next_state) => {
                        if recovery_active && self.recovery_profile.sync_tokens.contains(&term_idx)
                        {
                            let old_pos = self.pos;
                            let old_string_opened = self.string_opened;
                            self.pos = old_pos + token_len;
                            let (next_raw_term, _next_len, _next_node) = self.lex(None);
                            self.pos = old_pos;
                            self.string_opened = old_string_opened;

                            if self
                                .recovery_profile
                                .closing_tokens
                                .contains(&next_raw_term)
                            {
                                self.push_unexpected_trimmed(
                                    self.pos,
                                    self.pos + token_len,
                                    self.expected_ids_for_analysis(
                                        &self.grammar.analysis,
                                        current_state_idx,
                                        true,
                                    ),
                                );
                                let deleted_node = self.alloc.alloc(
                                    Tag::new_error(ParsecError::UnexpectedToken),
                                    vec![],
                                    token_len,
                                );
                                node_stack.push(StackEntry {
                                    node: deleted_node,
                                    binds_state: false,
                                });
                                self.consume(token_len);
                                continue;
                            }
                        }

                        if self.is_quote_terminal(term_idx) {
                            self.string_opened = !self.string_opened;
                        }
                        if let Tag::Error(_) = self.alloc.get_node(token_node).tag {
                            self.messages.push(ParserMessage::new_unexpected(
                                Span::new(self.pos, self.pos + token_len),
                                self.expected_ids_for_analysis(
                                    &self.grammar.analysis,
                                    current_state_idx,
                                    true,
                                ),
                            ));
                        }
                        self.consume(token_len);
                        state_stack.push(next_state);
                        node_stack.push(StackEntry {
                            node: token_node,
                            binds_state: true,
                        });
                    }
                    Action::Reduce(prod_idx) => {
                        if !self.perform_reduce(prod_idx, &mut state_stack, &mut node_stack) {
                            self.messages.push(ParserMessage::new_unexpected(
                                Span::new(self.pos, self.pos),
                                Vec::new(),
                            ));
                            break;
                        }
                    }
                    Action::Accept => {
                        // Do not silently drop trailing junk for grammars without explicit EOF.
                        if self.pos < self.text.len() && !self.text[self.pos..].trim().is_empty() {
                            let trailing_start = self.pos;
                            let trailing_end = self.text.len();
                            self.push_unexpected_trimmed(trailing_start, trailing_end, Vec::new());
                            let trailing_error = self.alloc.alloc(
                                Tag::new_error(ParsecError::UnexpectedToken),
                                vec![],
                                trailing_end - trailing_start,
                            );
                            node_stack.push(StackEntry {
                                node: trailing_error,
                                binds_state: false,
                            });
                            self.consume(trailing_end - trailing_start);
                        }
                        break;
                    }
                }
            } else {
                recovery_active = true;
                // Fast-path for unknown tokens: delete and resync locally instead of running CPCT+.
                // This keeps errors localized and avoids costly repair search on junk input.
                let (raw_term, raw_len, _raw_node) = self.lex(None);
                if raw_term == UNKNOWN_TOKEN && raw_len > 0 {
                    if !self.panic_sync(&mut state_stack, &mut node_stack) {
                        break;
                    }
                    continue;
                }

                let recovery_config = &self.config.recovery;
                let repairs = recover(
                    &self.grammar.analysis,
                    &self.grammar.table.productions,
                    &self.grammar.table.terminals,
                    &self.text,
                    self.pos,
                    &state_stack,
                    self.string_opened,
                    recovery_config,
                    Some(&mut self.recovery_cache),
                );

                // CPCT+ returns the complete set of minimum cost repair sequences
                // Apply the first one (they're all equally good locally)
                if repairs.is_empty() {
                    // No CPCT+ repair found: try delimiter/token sync fallback.
                    if !self.panic_sync(&mut state_stack, &mut node_stack) {
                        break;
                    }
                    continue;
                }

                let ops = &repairs[0]; //  Use first repair from top-ranked set
                if ops.is_empty() {
                    // Empty repair: try sync fallback before giving up.
                    if !self.panic_sync(&mut state_stack, &mut node_stack) {
                        break;
                    }
                    continue;
                }

                let old_pos = self.pos;
                let old_state_len = state_stack.len();
                let old_node_len = node_stack.len();

                if !self.apply_repair_ops(ops, &mut state_stack, &mut node_stack) {
                    // Phase 2 fallback: delimiter/token synchronisation.
                    if !self.panic_sync(&mut state_stack, &mut node_stack) {
                        break;
                    }
                    continue;
                }

                // Check we made progress
                if self.pos == old_pos
                    && state_stack.len() == old_state_len
                    && node_stack.len() == old_node_len
                {
                    // No progress from replay: try sync fallback.
                    if !self.panic_sync(&mut state_stack, &mut node_stack) {
                        break;
                    }
                }
            }
        }

        let root_green = self.finalize_root(&mut node_stack);
        self.prime_reuse_from_tree(root_green, 0);

        Result {
            root: RedNode::root(root_green),
            messages: self.messages.clone(),
            semantic_commands: Vec::new(),
        }
    }

    // Helper for Reparser
    pub fn parse_rule(
        &mut self,
        rule_ix: usize,
        pos: usize,
        expected_width: usize,
    ) -> Option<GreenId> {
        if pos > self.text.len() || pos + expected_width > self.text.len() {
            return None;
        }

        self.messages.clear();
        self.newly_computed_nodes.clear();
        self.newly_computed_tokens.clear();

        let slice = self.text[pos..pos + expected_width].to_string();
        let cache_key = self.build_parse_rule_cache_key(rule_ix, expected_width, &slice);
        if let Some(cached) = self.lookup_parse_rule_cache(&cache_key, &slice, pos) {
            return cached;
        }

        let old_pos = self.pos;
        let old_string_opened = self.string_opened;

        self.pos = pos;
        self.string_opened = false;

        let parse_end = pos + expected_width;
        let analysis = self.analysis_for_rule(rule_ix);
        let mut state_stack = vec![analysis.start_state];
        let mut node_stack: Vec<StackEntry> = vec![];
        let mut recovery_steps = 0usize;

        loop {
            let current_state_idx = *state_stack.last().unwrap();
            let expected_ids = self.expected_ids_for_analysis(&analysis, current_state_idx, false);
            let (term_idx, token_len, token_node) =
                self.lex_with_end(Some(&expected_ids), parse_end);

            let action = analysis.states[current_state_idx]
                .actions
                .get(&term_idx)
                .cloned();

            let Some(action) = action else {
                let expected = self.expected_ids_for_analysis(&analysis, current_state_idx, true);
                let (raw_term, raw_len, raw_node) = self.lex_with_end(None, parse_end);

                if raw_len > 0 {
                    if let Some(term_ix) = expected
                        .iter()
                        .copied()
                        .filter(|term_ix| {
                            !self.recovery_profile.opening_tokens.contains(term_ix)
                                && !self.recovery_profile.closing_tokens.contains(term_ix)
                                && !self.recovery_profile.sync_tokens.contains(term_ix)
                                && !self.is_quote_terminal(*term_ix)
                                && !self.is_json_string_terminal(*term_ix)
                                && *term_ix != EOF_TOKEN
                        })
                        .min()
                    {
                        let start_pos = self.pos;
                        let err_node = self.alloc.alloc(
                            Tag::new_error(ParsecError::UnexpectedToken),
                            vec![],
                            raw_len,
                        );
                        if self.apply_terminal_with_analysis(
                            term_ix,
                            raw_len,
                            err_node,
                            true,
                            &mut state_stack,
                            &mut node_stack,
                            analysis.as_ref(),
                        ) {
                            self.messages.push(ParserMessage::new_unexpected(
                                Span::new(start_pos, start_pos + raw_len),
                                expected.clone(),
                            ));
                            recovery_steps += 1;
                            if recovery_steps <= 128 {
                                continue;
                            }
                        }
                    }

                    let raw_is_beacon = self.recovery_profile.sync_tokens.contains(&raw_term)
                        || self.recovery_profile.closing_tokens.contains(&raw_term);

                    if raw_is_beacon {
                        let old_pos = self.pos;
                        let old_string_opened = self.string_opened;
                        let mut trial_states = state_stack.clone();
                        let mut trial_nodes = node_stack.clone();

                        if self.apply_terminal_with_analysis(
                            raw_term,
                            raw_len,
                            raw_node,
                            true,
                            &mut trial_states,
                            &mut trial_nodes,
                            analysis.as_ref(),
                        ) {
                            state_stack = trial_states;
                            node_stack = trial_nodes;
                            recovery_steps += 1;
                            if recovery_steps <= 128 {
                                continue;
                            }
                        }

                        self.pos = old_pos;
                        self.string_opened = old_string_opened;

                        if let Some(term_ix) = expected
                            .iter()
                            .copied()
                            .filter(|term_ix| {
                                !self.recovery_profile.opening_tokens.contains(term_ix)
                                    && !self.recovery_profile.closing_tokens.contains(term_ix)
                                    && !self.recovery_profile.sync_tokens.contains(term_ix)
                                    && !self.is_quote_terminal(*term_ix)
                                    && !self.is_json_string_terminal(*term_ix)
                                    && *term_ix != EOF_TOKEN
                            })
                            .min()
                        {
                            let missing = self.alloc.alloc(
                                Tag::new_error(ParsecError::MissingToken),
                                vec![],
                                0,
                            );
                            let before = (self.pos, state_stack.len(), node_stack.len());
                            if self.apply_terminal_with_analysis(
                                term_ix,
                                0,
                                missing,
                                false,
                                &mut state_stack,
                                &mut node_stack,
                                analysis.as_ref(),
                            ) {
                                let after = (self.pos, state_stack.len(), node_stack.len());
                                if after != before {
                                    self.messages.push(ParserMessage::new_missing(
                                        Span::new(self.pos, self.pos),
                                        vec![term_ix],
                                    ));
                                    recovery_steps += 1;
                                    if recovery_steps <= 128 {
                                        continue;
                                    }
                                }
                            }
                        }
                    }

                    self.push_unexpected_trimmed(self.pos, self.pos + raw_len, expected);
                    let deleted_node = self.alloc.alloc(
                        Tag::new_error(ParsecError::UnexpectedToken),
                        vec![],
                        raw_len,
                    );
                    node_stack.push(StackEntry {
                        node: deleted_node,
                        binds_state: false,
                    });
                    self.consume(raw_len);
                    recovery_steps += 1;
                    if recovery_steps <= 128 {
                        continue;
                    }
                } else {
                    let before = (self.pos, state_stack.len(), node_stack.len());
                    if let Some(term_ix) = expected
                        .iter()
                        .copied()
                        .find(|term_ix| *term_ix != EOF_TOKEN)
                    {
                        let missing =
                            self.alloc
                                .alloc(Tag::new_error(ParsecError::MissingToken), vec![], 0);
                        if self.apply_terminal_with_analysis(
                            term_ix,
                            0,
                            missing,
                            false,
                            &mut state_stack,
                            &mut node_stack,
                            analysis.as_ref(),
                        ) {
                            let after = (self.pos, state_stack.len(), node_stack.len());
                            if after != before {
                                self.messages.push(ParserMessage::new_missing(
                                    Span::new(self.pos, self.pos),
                                    vec![term_ix],
                                ));
                                recovery_steps += 1;
                                if recovery_steps <= 128 {
                                    continue;
                                }
                            }
                        }
                    }
                    self.messages.push(ParserMessage::new_unexpected(
                        Span::new(self.pos, self.pos),
                        expected,
                    ));
                }
                self.pos = old_pos;
                self.string_opened = old_string_opened;
                self.store_parse_rule_cache(cache_key, slice, None, self.messages.clone(), pos);
                return None;
            };

            match action {
                Action::Shift(next_state) => {
                    if self.is_quote_terminal(term_idx) {
                        self.string_opened = !self.string_opened;
                    }
                    if let Tag::Error(_) = self.alloc.get_node(token_node).tag {
                        self.messages.push(ParserMessage::new_unexpected(
                            Span::new(self.pos, self.pos + token_len),
                            self.expected_ids_for_analysis(&analysis, current_state_idx, true),
                        ));
                    }
                    self.newly_computed_tokens
                        .push(Span::new(self.pos, self.pos + token_len));
                    self.consume(token_len);
                    state_stack.push(next_state);
                    node_stack.push(StackEntry {
                        node: token_node,
                        binds_state: true,
                    });
                }
                Action::Reduce(prod_idx) => {
                    if !self.perform_reduce_with_analysis(
                        prod_idx,
                        &mut state_stack,
                        &mut node_stack,
                        &analysis,
                    ) {
                        self.messages.push(ParserMessage::new_unexpected(
                            Span::new(self.pos, self.pos),
                            Vec::new(),
                        ));
                        self.pos = old_pos;
                        self.string_opened = old_string_opened;
                        self.store_parse_rule_cache(
                            cache_key,
                            slice,
                            None,
                            self.messages.clone(),
                            pos,
                        );
                        return None;
                    }
                }
                Action::Accept => break,
            }
        }

        let parsed_green = self.finalize_root(&mut node_stack);
        let width = self.alloc.get_node(parsed_green).width;
        let Some(parsed_rule_green) = self.extract_rule_node(parsed_green, rule_ix) else {
            self.pos = old_pos;
            self.string_opened = old_string_opened;
            self.store_parse_rule_cache(cache_key, slice, None, self.messages.clone(), pos);
            return None;
        };
        let parsed_rule_width = self.alloc.get_node(parsed_rule_green).width;
        if width != expected_width || parsed_rule_width != expected_width {
            self.pos = old_pos;
            self.string_opened = old_string_opened;
            self.store_parse_rule_cache(cache_key, slice, None, self.messages.clone(), pos);
            return None;
        }

        self.newly_computed_nodes
            .push(Span::new(pos, pos + parsed_rule_width));

        self.pos = old_pos;
        self.string_opened = old_string_opened;
        if self.messages.is_empty() {
            self.prime_reuse_from_tree(parsed_rule_green, pos);
        } else {
            self.store_parse_rule_cache(
                cache_key,
                slice,
                Some(parsed_rule_green),
                self.messages.clone(),
                pos,
            );
        }
        Some(parsed_rule_green)
    }

    fn analysis_for_rule(&mut self, rule_ix: usize) -> Arc<GrammarStateAnalysis> {
        if rule_ix == self.grammar.table.start_rule {
            return Arc::clone(&self.grammar.analysis);
        }

        if let Some(analysis) = self.rule_analyses.get(&rule_ix) {
            return Arc::clone(analysis);
        }

        let mut table = self.grammar.table.clone();
        let wrapper_ix = table.rules.len();
        table.rules.push(RuleInfo {
            name: "$inc_root",
            description: "$inc_root",
            node: NormalizedNode::Reference(rule_ix),
            is_expression: false,
        });
        table.productions.push(Production {
            lhs: wrapper_ix,
            rhs: vec![Symbol::NonTerminal(rule_ix)],
            field_positions: vec![],
        });
        table.start_rule = wrapper_ix;

        let analysis = Arc::new(GrammarStateAnalysis::from_table(&table, wrapper_ix));
        self.rule_analyses.insert(rule_ix, Arc::clone(&analysis));
        analysis
    }

    fn build_parse_rule_cache_key(
        &self,
        rule_ix: usize,
        expected_width: usize,
        slice: &str,
    ) -> ParseRuleCacheKey {
        let mut hasher = DefaultHasher::new();
        slice.hash(&mut hasher);
        let slice_hash = hasher.finish();

        ParseRuleCacheKey {
            rule_ix,
            expected_width,
            slice_hash,
            simple_ast: self.config.simple_ast,
        }
    }

    fn lookup_parse_rule_cache(
        &mut self,
        cache_key: &ParseRuleCacheKey,
        slice: &str,
        pos: usize,
    ) -> Option<Option<GreenId>> {
        if !self.reuse_enabled {
            return None;
        }

        self.reuse_stats.lookups += 1;
        let cached = self.reuse_cache.get(cache_key)?;
        if cached.slice != slice {
            return None;
        }

        self.reuse_stats.hits += 1;
        self.messages = cached
            .relative_messages
            .iter()
            .cloned()
            .map(|mut msg| {
                msg.span = Span::new(msg.span.start + pos, msg.span.end + pos);
                msg
            })
            .collect();
        self.newly_computed_nodes.clear();
        self.newly_computed_tokens.clear();
        Some(cached.green)
    }

    fn store_parse_rule_cache(
        &mut self,
        cache_key: ParseRuleCacheKey,
        slice: String,
        green: Option<GreenId>,
        messages: ParserMessages,
        pos: usize,
    ) {
        if !self.reuse_enabled {
            return;
        }
        if green.is_none() && !self.reuse_cache_failures {
            return;
        }

        let relative_messages = messages
            .into_iter()
            .map(|mut msg| {
                msg.span = Span::new(
                    msg.span.start.saturating_sub(pos),
                    msg.span.end.saturating_sub(pos),
                );
                msg
            })
            .collect();

        self.reuse_cache.insert(
            cache_key,
            ParseRuleCacheEntry {
                slice,
                green,
                relative_messages,
            },
        );
        self.reuse_stats.inserts += 1;
    }

    fn prime_reuse_from_tree(&mut self, green: GreenId, offset: usize) {
        if !self.reuse_enabled {
            return;
        }
        if !self.messages.is_empty() {
            return;
        }
        self.prime_reuse_subtree(green, offset);
    }

    fn prime_reuse_subtree(&mut self, green: GreenId, offset: usize) {
        let (tag, width, children) = {
            let node = self.alloc.get_node(green);
            (node.tag.clone(), node.width, node.children.clone())
        };

        let end = offset.saturating_add(width);
        if end > self.text.len() {
            return;
        }

        if let Tag::Rule { rule_ix } = tag {
            let slice = self.text[offset..end].to_string();
            let cache_key = self.build_parse_rule_cache_key(rule_ix, width, &slice);
            self.reuse_cache.insert(
                cache_key,
                ParseRuleCacheEntry {
                    slice,
                    green: Some(green),
                    relative_messages: Vec::new(),
                },
            );
            self.reuse_stats.inserts += 1;
        }

        let mut child_offset = offset;
        for child in children {
            self.prime_reuse_subtree(child, child_offset);
            child_offset += self.alloc.get_node(child).width;
        }
    }

    fn extract_rule_node(&self, green: GreenId, rule_ix: usize) -> Option<GreenId> {
        let node = self.alloc.get_node(green);
        match &node.tag {
            Tag::Rule { rule_ix: current } if *current == rule_ix => Some(green),
            Tag::Root | Tag::Error(_) => {
                if node.children.len() == 1 {
                    let child = node.children[0];
                    let child_node = self.alloc.get_node(child);
                    if matches!(child_node.tag, Tag::Rule { rule_ix: current } if current == rule_ix)
                    {
                        return Some(child);
                    }
                }
                node.children.iter().copied().find(|child| {
                    matches!(
                        self.alloc.get_node(*child).tag,
                        Tag::Rule { rule_ix: current } if current == rule_ix
                    )
                })
            }
            _ => None,
        }
    }

    fn lex(&mut self, expected: Option<&[usize]>) -> (usize, usize, GreenId) {
        self.lex_with_end(expected, self.text.len())
    }

    fn lex_with_end(&mut self, expected: Option<&[usize]>, end: usize) -> (usize, usize, GreenId) {
        let bounded_end = end.min(self.text.len());
        if self.pos >= bounded_end {
            // Keep going: nullable terminals like EndOfInput may still be expected at boundary.
        }

        let rest = &self.text[self.pos..bounded_end];
        let at_boundary = self.pos >= bounded_end;

        let mut best_match: Option<(usize, usize)> = None;

        let mut consider = |idx: usize, matcher: &crate::parsec::words::MatcherRef| {
            if self.is_json_string_terminal(idx) && !self.string_opened {
                return;
            }
            let mut probe = self.pos;
            if matcher.matches(&self.text, &mut probe).is_some() {
                if probe < self.pos || probe > bounded_end {
                    return;
                }
                let len = probe - self.pos;
                // Allow nullable terminals only at a real parse boundary, except json string body
                // where empty content is valid before a closing quote.
                if len == 0 && !(at_boundary || rest.starts_with('"')) {
                    return;
                }
                if best_match.iter().all(|&(_, best_len)| len > best_len) {
                    best_match = Some((idx, len));
                }
            }
        };

        if let Some(expected) = expected {
            for &idx in expected.iter() {
                if idx == EOF_TOKEN {
                    continue;
                }
                if let Some(matcher) = self.grammar.table.terminals.get(idx) {
                    consider(idx, matcher);
                }
            }
        } else {
            for (idx, matcher) in self.grammar.table.terminals.iter().enumerate() {
                consider(idx, matcher);
            }
        }

        if let Some((idx, len)) = best_match {
            let node = self.alloc.alloc_token(Tag::new_token(idx), len);
            (idx, len, node)
        } else {
            if at_boundary || rest.trim().is_empty() {
                return (
                    EOF_TOKEN,
                    rest.len(),
                    self.alloc
                        .alloc_token(Tag::new_token(EOF_TOKEN), rest.len()),
                );
            }

            let len = self.unknown_span_len(self.pos, bounded_end);
            let node =
                self.alloc
                    .alloc(Tag::Error(vec![ParsecError::UnexpectedToken]), vec![], len);
            (UNKNOWN_TOKEN, len, node)
        }
    }

    fn consume(&mut self, len: usize) {
        self.pos += len;
    }

    fn unknown_span_len(&self, start: usize, end: usize) -> usize {
        let bounded_end = end.min(self.text.len());
        if start >= bounded_end {
            return 0;
        }
        let rest = &self.text[start..bounded_end];
        for (offset, _) in rest.char_indices().skip(1) {
            let abs = start + offset;
            if self.any_terminal_matches_at(abs, bounded_end) {
                return offset;
            }
        }
        rest.len()
    }

    fn any_terminal_matches_at(&self, pos: usize, end: usize) -> bool {
        let bounded_end = end.min(self.text.len());
        if pos > bounded_end {
            return false;
        }
        for (idx, matcher) in self.grammar.table.terminals.iter().enumerate() {
            if self.is_json_string_terminal(idx) && !self.string_opened {
                continue;
            }
            let mut probe = pos;
            if matcher.matches(&self.text, &mut probe).is_some() {
                if probe < pos || probe > bounded_end {
                    continue;
                }
                let len = probe - pos;
                if len == 0 && !(pos >= bounded_end || self.text[pos..bounded_end].starts_with('"'))
                {
                    continue;
                }
                return true;
            }
        }
        false
    }

    fn apply_repair_ops(
        &mut self,
        ops: &[RepairOp],
        state_stack: &mut Vec<usize>,
        node_stack: &mut Vec<StackEntry>,
    ) -> bool {
        let mut i = 0;
        while i < ops.len() {
            let op = ops[i];

            match op {
                RepairOp::Insert(term_ix) => {
                    // Reject synthetic string construction; let sync fallback handle it.
                    if self.is_quote_terminal(term_ix) || self.is_json_string_terminal(term_ix) {
                        return false;
                    }
                    self.messages.push(ParserMessage::new_missing(
                        Span::new(self.pos, self.pos),
                        vec![term_ix],
                    ));
                    let token_node =
                        self.alloc
                            .alloc(Tag::new_error(ParsecError::MissingToken), vec![], 0);
                    if !self.apply_terminal(term_ix, 0, token_node, false, state_stack, node_stack)
                    {
                        return false;
                    }
                }
                RepairOp::Delete => {
                    let expected = self.expected_ids(*state_stack.last().unwrap());
                    let (_term, len, _node) = self.lex(None);
                    if len == 0 {
                        return false;
                    }
                    self.push_unexpected_trimmed(self.pos, self.pos + len, expected);
                    let deleted_node =
                        self.alloc
                            .alloc(Tag::new_error(ParsecError::UnexpectedToken), vec![], len);
                    node_stack.push(StackEntry {
                        node: deleted_node,
                        binds_state: false,
                    });
                    self.consume(len);
                }
                RepairOp::Shift => {
                    let current_state_idx = *state_stack.last().unwrap();
                    let expected_ids: Vec<usize> = self.grammar.analysis.states[current_state_idx]
                        .actions
                        .keys()
                        .copied()
                        .collect();
                    let (term, len, node) = self.lex(Some(&expected_ids));
                    let is_error_token = {
                        let token = self.alloc.get_node(node);
                        matches!(token.tag, Tag::Error(_))
                    };
                    if is_error_token {
                        return false;
                    }
                    if !self.apply_terminal(term, len, node, true, state_stack, node_stack) {
                        return false;
                    }
                }
            }
            i += 1;
        }
        true
    }

    fn panic_sync(
        &mut self,
        state_stack: &mut Vec<usize>,
        node_stack: &mut Vec<StackEntry>,
    ) -> bool {
        let start = self.pos;
        let mut steps = 0usize;
        let mut last_expected = Vec::new();

        while steps < 64 {
            let state_idx = *state_stack.last().unwrap();
            let expected_ids = self.expected_ids(state_idx);
            last_expected = expected_ids.clone();

            let (term, len, node) = self.lex(Some(&expected_ids));
            let is_error_token = {
                let token = self.alloc.get_node(node);
                matches!(token.tag, Tag::Error(_))
            };
            let has_action = self.grammar.analysis.states[state_idx]
                .actions
                .get(&term)
                .is_some();

            if has_action && !is_error_token {
                if self.pos > start {
                    self.messages.push(ParserMessage::new_unexpected(
                        Span::new(start, self.pos),
                        last_expected.clone(),
                    ));
                }
                return self.apply_terminal(term, len, node, true, state_stack, node_stack);
            }

            let (raw_term, raw_len, raw_node) = self.lex(None);
            if raw_len == 0 {
                break;
            }

            // If we have a non-structural expected token, consume the unexpected
            // input as that token to avoid inserting a MissingToken afterwards.
            if let Some(term_ix) = expected_ids
                .iter()
                .copied()
                .filter(|term_ix| {
                    !self.recovery_profile.opening_tokens.contains(term_ix)
                        && !self.recovery_profile.closing_tokens.contains(term_ix)
                        && !self.recovery_profile.sync_tokens.contains(term_ix)
                        && !self.is_quote_terminal(*term_ix)
                        && !self.is_json_string_terminal(*term_ix)
                        && *term_ix != EOF_TOKEN
                })
                .min()
            {
                let start_pos = self.pos;
                let err_node = self.alloc.alloc(
                    Tag::new_error(ParsecError::UnexpectedToken),
                    vec![],
                    raw_len,
                );
                if self.apply_terminal(term_ix, raw_len, err_node, true, state_stack, node_stack) {
                    self.messages.push(ParserMessage::new_unexpected(
                        Span::new(start_pos, start_pos + raw_len),
                        last_expected.clone(),
                    ));
                    return true;
                }
            }

            // If we can legally shift a sync/closing token by reducing, prefer that
            // over inserting synthetic openers that can swallow surrounding structure.
            if self.recovery_profile.sync_tokens.contains(&raw_term)
                || self.recovery_profile.closing_tokens.contains(&raw_term)
            {
                let old_pos = self.pos;
                let old_string_opened = self.string_opened;
                let mut trial_states = state_stack.clone();
                let mut trial_nodes = node_stack.clone();

                if self.apply_terminal(
                    raw_term,
                    raw_len,
                    raw_node,
                    true,
                    &mut trial_states,
                    &mut trial_nodes,
                ) {
                    if self.pos > start {
                        self.messages.push(ParserMessage::new_unexpected(
                            Span::new(start, self.pos),
                            last_expected.clone(),
                        ));
                    }
                    *state_stack = trial_states;
                    *node_stack = trial_nodes;
                    return true;
                }

                self.pos = old_pos;
                self.string_opened = old_string_opened;
            }

            // When we see a sync token while expecting a value, prefer inserting a
            // non-structural expected token (e.g., null/number/boolean) so the
            // sync token can be consumed by the enclosing construct.
            if self.recovery_profile.sync_tokens.contains(&raw_term) {
                if let Some(term_ix) = expected_ids
                    .iter()
                    .copied()
                    .filter(|term_ix| {
                        !self.recovery_profile.opening_tokens.contains(term_ix)
                            && !self.recovery_profile.closing_tokens.contains(term_ix)
                            && !self.is_quote_terminal(*term_ix)
                            && !self.is_json_string_terminal(*term_ix)
                            && *term_ix != EOF_TOKEN
                    })
                    .min()
                {
                    let token_node =
                        self.alloc
                            .alloc(Tag::new_error(ParsecError::MissingToken), vec![], 0);
                    let mut trial_states = state_stack.clone();
                    let mut trial_nodes = node_stack.clone();
                    if self.apply_terminal(
                        term_ix,
                        0,
                        token_node,
                        false,
                        &mut trial_states,
                        &mut trial_nodes,
                    ) {
                        *state_stack = trial_states;
                        *node_stack = trial_nodes;
                        self.messages.push(ParserMessage::new_missing(
                            Span::new(self.pos, self.pos),
                            vec![term_ix],
                        ));
                        steps += 1;
                        continue;
                    }
                }
            }

            // Trailing separator recovery: if we see a closing token while the last
            // shifted terminal was a separator, drop the separator and retry.
            if self.recovery_profile.closing_tokens.contains(&raw_term) {
                if let Some((idx, term_ix)) =
                    node_stack
                        .iter()
                        .enumerate()
                        .rev()
                        .find_map(|(idx, entry)| {
                            if !entry.binds_state {
                                return None;
                            }
                            let node = self.alloc.get_node(entry.node);
                            if let Tag::Token { rule_ix } = node.tag {
                                if self.recovery_profile.sync_tokens.contains(&rule_ix) {
                                    return Some((idx, rule_ix));
                                }
                            }
                            None
                        })
                {
                    let _ = term_ix; // used for clarity
                    node_stack.truncate(idx);
                    state_stack.pop();
                    steps += 1;
                    continue;
                }
            }

            // Bridge-style mend: at delimiter/structural beacons, try inserting
            // expected structural tokens before deleting user input.
            if self.try_bridge_mend(raw_term, &expected_ids, state_stack, node_stack) {
                steps += 1;
                continue;
            }

            let deleted_node = self.alloc.alloc(
                Tag::new_error(ParsecError::UnexpectedToken),
                vec![],
                raw_len,
            );
            node_stack.push(StackEntry {
                node: deleted_node,
                binds_state: false,
            });
            self.consume(raw_len);
            steps += 1;
        }

        if self.pos > start {
            self.messages.push(ParserMessage::new_unexpected(
                Span::new(start, self.pos),
                last_expected,
            ));
            true
        } else {
            false
        }
    }

    fn try_bridge_mend(
        &mut self,
        raw_term: usize,
        expected_ids: &[usize],
        state_stack: &mut Vec<usize>,
        node_stack: &mut Vec<StackEntry>,
    ) -> bool {
        let raw_is_beacon = self.recovery_profile.sync_tokens.contains(&raw_term)
            || self.recovery_profile.opening_tokens.contains(&raw_term)
            || self.recovery_profile.closing_tokens.contains(&raw_term);
        if !raw_is_beacon {
            return false;
        }

        let mut candidates: Vec<(usize, usize)> = expected_ids
            .iter()
            .copied()
            .filter_map(|term_ix| {
                let priority = self.bridge_insert_priority(term_ix, raw_term)?;
                Some((priority, term_ix))
            })
            .collect();

        if candidates.is_empty() {
            return false;
        }

        candidates.sort_by_key(|(priority, term_ix)| (*priority, *term_ix));
        candidates.dedup_by_key(|(_, term_ix)| *term_ix);

        for (_priority, term_ix) in candidates {
            let silent_opener = self.recovery_profile.closing_tokens.contains(&raw_term)
                && self.recovery_profile.opening_tokens.contains(&term_ix)
                && self
                    .recovery_profile
                    .closing_to_opening
                    .get(&raw_term)
                    .is_some_and(|openers| openers.contains(&term_ix));

            let token_node = if silent_opener {
                self.alloc.alloc_token(Tag::new_token(term_ix), 0)
            } else {
                self.alloc
                    .alloc(Tag::new_error(ParsecError::MissingToken), vec![], 0)
            };
            let mut trial_states = state_stack.clone();
            let mut trial_nodes = node_stack.clone();

            if self.apply_terminal(
                term_ix,
                0,
                token_node,
                false,
                &mut trial_states,
                &mut trial_nodes,
            ) {
                *state_stack = trial_states;
                *node_stack = trial_nodes;
                if !silent_opener {
                    self.messages.push(ParserMessage::new_missing(
                        Span::new(self.pos, self.pos),
                        vec![term_ix],
                    ));
                }
                return true;
            }
        }

        false
    }

    fn apply_terminal(
        &mut self,
        term: usize,
        len: usize,
        token_node: GreenId,
        consume_input: bool,
        state_stack: &mut Vec<usize>,
        node_stack: &mut Vec<StackEntry>,
    ) -> bool {
        loop {
            let current_state_idx = *state_stack.last().unwrap();
            let action = {
                let current_state = &self.grammar.analysis.states[current_state_idx];
                current_state.actions.get(&term).cloned()
            };

            match action {
                Some(Action::Shift(next_state)) => {
                    if consume_input && self.is_quote_terminal(term) {
                        self.string_opened = !self.string_opened;
                    }
                    if consume_input {
                        self.consume(len);
                    }
                    state_stack.push(next_state);
                    node_stack.push(StackEntry {
                        node: token_node,
                        binds_state: true,
                    });
                    return true;
                }
                Some(Action::Reduce(prod_idx)) => {
                    if !self.perform_reduce(prod_idx, state_stack, node_stack) {
                        return false;
                    }
                }
                Some(Action::Accept) => return true,
                None => return false,
            }
        }
    }

    fn apply_terminal_with_analysis(
        &mut self,
        term: usize,
        len: usize,
        token_node: GreenId,
        consume_input: bool,
        state_stack: &mut Vec<usize>,
        node_stack: &mut Vec<StackEntry>,
        analysis: &GrammarStateAnalysis,
    ) -> bool {
        loop {
            let current_state_idx = *state_stack.last().unwrap();
            let action = analysis.states[current_state_idx]
                .actions
                .get(&term)
                .cloned();

            match action {
                Some(Action::Shift(next_state)) => {
                    if consume_input && self.is_quote_terminal(term) {
                        self.string_opened = !self.string_opened;
                    }
                    if consume_input {
                        self.consume(len);
                    }
                    state_stack.push(next_state);
                    node_stack.push(StackEntry {
                        node: token_node,
                        binds_state: true,
                    });
                    return true;
                }
                Some(Action::Reduce(prod_idx)) => {
                    if !self.perform_reduce_with_analysis(
                        prod_idx,
                        state_stack,
                        node_stack,
                        analysis,
                    ) {
                        return false;
                    }
                }
                Some(Action::Accept) => return true,
                None => return false,
            }
        }
    }

    fn finalize_root(&self, node_stack: &mut Vec<StackEntry>) -> GreenId {
        if node_stack.is_empty() {
            return self.alloc.new_placeholder(0);
        }
        if node_stack.len() == 1 {
            return node_stack
                .pop()
                .map(|e| e.node)
                .unwrap_or_else(|| self.alloc.new_placeholder(0));
        }

        let children: Vec<GreenId> = node_stack.drain(..).map(|e| e.node).collect();
        let width: usize = children
            .iter()
            .map(|id| self.alloc.get_node(*id).width)
            .sum();
        self.alloc.alloc(Tag::Root, children, width)
    }

    fn pop_reduce_entries(
        &self,
        rhs_len: usize,
        state_stack: &mut Vec<usize>,
        node_stack: &mut Vec<StackEntry>,
    ) -> Option<Vec<StackEntry>> {
        let mut popped = Vec::new();
        let mut bound = 0usize;

        while bound < rhs_len {
            let entry = node_stack.pop()?;
            if entry.binds_state {
                state_stack.pop();
                bound += 1;
            }
            popped.push(entry);
        }

        popped.reverse();
        Some(popped)
    }

    fn perform_reduce(
        &mut self,
        prod_idx: usize,
        state_stack: &mut Vec<usize>,
        node_stack: &mut Vec<StackEntry>,
    ) -> bool {
        let analysis = Arc::clone(&self.grammar.analysis);
        self.perform_reduce_with_analysis(prod_idx, state_stack, node_stack, analysis.as_ref())
    }

    fn perform_reduce_with_analysis(
        &mut self,
        prod_idx: usize,
        state_stack: &mut Vec<usize>,
        node_stack: &mut Vec<StackEntry>,
        analysis: &GrammarStateAnalysis,
    ) -> bool {
        let (lhs, rhs_len, field_positions, is_single_terminal_prod) = {
            let prod = &self.grammar.table.productions[prod_idx];
            (
                prod.lhs,
                prod.rhs.len(),
                prod.field_positions.clone(),
                prod.rhs.len() == 1 && matches!(prod.rhs[0], Symbol::Terminal(_)),
            )
        };

        let popped = match self.pop_reduce_entries(rhs_len, state_stack, node_stack) {
            Some(entries) => entries,
            None => return false,
        };

        let mut children: Vec<GreenId> = popped.iter().map(|entry| entry.node).collect();
        let bound_positions: Vec<usize> = popped
            .iter()
            .enumerate()
            .filter_map(|(idx, entry)| entry.binds_state.then_some(idx))
            .collect();

        for (pos, name) in field_positions {
            if let Some(&child_ix) = bound_positions.get(pos) {
                let child_id = children[child_ix];
                let width = {
                    let child_node = self.alloc.get_node(child_id);
                    child_node.width
                };
                let field_id =
                    self.alloc
                        .alloc(Tag::Field { rule_ix: lhs, name }, vec![child_id], width);
                children[child_ix] = field_id;
            }
        }

        let passthrough_unexpected = is_single_terminal_prod
            && children.len() == 1
            && matches!(
                &self.alloc.get_node(children[0]).tag,
                Tag::Error(errs) if errs.iter().any(|e| matches!(e, ParsecError::UnexpectedToken))
            );

        let new_node = if passthrough_unexpected {
            children[0]
        } else {
            let builder =
                crate::parsec::builder::TreeBuilder::new(&self.grammar, &self.alloc, &self.config);
            builder.build_node(lhs, children)
        };

        let node_width = self.alloc.get_node(new_node).width;
        let node_start = self.pos - node_width;
        self.newly_computed_nodes
            .push(Span::new(node_start, self.pos));

        node_stack.push(StackEntry {
            node: new_node,
            binds_state: true,
        });

        let top_state_idx = *state_stack.last().unwrap();
        let top_state = &analysis.states[top_state_idx];
        if let Some(goto_state) = top_state.goto.get(&lhs) {
            state_stack.push(*goto_state);
            true
        } else {
            false
        }
    }

    fn expected_ids(&self, state_idx: usize) -> Vec<usize> {
        self.expected_ids_for_analysis(&self.grammar.analysis, state_idx, true)
    }

    fn expected_ids_for_analysis(
        &self,
        analysis: &GrammarStateAnalysis,
        state_idx: usize,
        exclude_eof: bool,
    ) -> Vec<usize> {
        analysis.states[state_idx]
            .actions
            .keys()
            .copied()
            .filter(|id| !exclude_eof || *id != EOF_TOKEN)
            .collect()
    }

    fn build_recovery_profile(grammar: &Grammar) -> RecoveryProfile {
        let mut profile = RecoveryProfile::default();

        for prod in &grammar.table.productions {
            if prod.rhs.len() < 2 {
                continue;
            }

            let first_term = match prod.rhs.first() {
                Some(Symbol::Terminal(t)) => Some(*t),
                _ => None,
            };
            let last_term = match prod.rhs.last() {
                Some(Symbol::Terminal(t)) => Some(*t),
                _ => None,
            };
            let contains_nonterminal = prod.rhs.iter().any(|s| matches!(s, Symbol::NonTerminal(_)));

            // Delimiter pair extraction directly from production envelope: T ... T.
            if let (Some(open), Some(close)) = (first_term, last_term) {
                if contains_nonterminal {
                    profile.opening_tokens.insert(open);
                    profile.closing_tokens.insert(close);
                    profile
                        .closing_to_opening
                        .entry(close)
                        .or_default()
                        .insert(open);
                }
            }

            // Separator/sync extraction: terminal between nonterminals.
            for win in prod.rhs.windows(3) {
                if let [
                    Symbol::NonTerminal(_),
                    Symbol::Terminal(t),
                    Symbol::NonTerminal(_),
                ] = win
                {
                    profile.sync_tokens.insert(*t);
                }
            }

            // Prefix separator/sync extraction: terminal followed by a nonterminal.
            // This captures list separators like "," in sep-tail productions.
            for win in prod.rhs.windows(2) {
                if let [Symbol::Terminal(t), Symbol::NonTerminal(_)] = win {
                    profile.sync_tokens.insert(*t);
                }
            }

            // Statement/list boundary sync: terminal suffix after nonterminal content.
            if let Some(Symbol::Terminal(t)) = prod.rhs.last() {
                if prod.rhs[..prod.rhs.len() - 1]
                    .iter()
                    .any(|s| matches!(s, Symbol::NonTerminal(_)))
                {
                    profile.sync_tokens.insert(*t);
                }
            }
        }

        // Closers are always sync anchors.
        for close in profile.closing_tokens.iter().copied() {
            profile.sync_tokens.insert(close);
        }

        profile
    }

    fn bridge_insert_priority(&self, expected_term: usize, raw_term: usize) -> Option<usize> {
        let expected_is_closer = self
            .recovery_profile
            .closing_tokens
            .contains(&expected_term);
        let expected_is_opener = self
            .recovery_profile
            .opening_tokens
            .contains(&expected_term);
        let raw_is_sync = self.recovery_profile.sync_tokens.contains(&raw_term);
        let raw_is_closer = self.recovery_profile.closing_tokens.contains(&raw_term);

        // Construction-site insertion: prefer inserting closing delimiters near sync boundaries.
        if expected_is_closer {
            if raw_is_sync || expected_term == raw_term {
                return Some(0);
            }
            if raw_is_closer {
                return Some(1);
            }
            return Some(2);
        }

        // If parser expects an opener while seeing a closing token, prefer matched opener.
        if expected_is_opener {
            if self
                .recovery_profile
                .closing_to_opening
                .get(&raw_term)
                .is_some_and(|openers| openers.contains(&expected_term))
            {
                return Some(3);
            }
            return Some(6);
        }

        None
    }

    fn push_unexpected_trimmed(&mut self, start: usize, end: usize, expected: Vec<usize>) {
        let start = start.min(self.text.len());
        let end = end.min(self.text.len());
        if start >= end {
            return;
        }
        let slice = &self.text[start..end];
        let leading_ws = slice
            .char_indices()
            .find(|(_, c)| !c.is_whitespace())
            .map(|(i, _)| i)
            .unwrap_or(slice.len());
        let trailing_ws = slice
            .char_indices()
            .rev()
            .find(|(_, c)| !c.is_whitespace())
            .map(|(i, c)| i + c.len_utf8())
            .unwrap_or(0);
        if leading_ws >= trailing_ws {
            return;
        }
        self.messages.push(ParserMessage::new_unexpected(
            Span::new(start + leading_ws, start + trailing_ws),
            expected,
        ));
    }

    fn is_quote_terminal(&self, term_ix: usize) -> bool {
        self.grammar
            .table
            .terminals
            .get(term_ix)
            .and_then(|m| m.preview())
            .is_some_and(|preview| preview == "\"")
    }

    fn is_json_string_terminal(&self, term_ix: usize) -> bool {
        self.grammar
            .table
            .terminals
            .get(term_ix)
            .map(|m| m.display().contains("json_string"))
            .unwrap_or(false)
    }
}
