use std::collections::hash_map::DefaultHasher;
use std::collections::{HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use crate::grammar::Grammar;
use crate::grammar::analysis::{Action, EOF_TOKEN, GrammarStateAnalysis};
use crate::grammar::ir::Symbol;
use crate::grammar::recovery::{ErrorRecoveryStrategy, RecoverySpecs};
use crate::parsec::display::format_ast;
use crate::parsec::msg::{ParserMessage, ParserMessages};
use crate::parsec::recovery::{
    OpenScopeToken, RecoveryCache, RecoveryConfig, RepairOp, ScopeStop, recover, scope_recover,
};
use crate::parsec::tree::{GreenId, ParsecError, RedNode, Tag, TreeAllocRef, TreeAllocRefExt};
use crate::parsec::view::View;
use crate::runtime;
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

pub struct Result<'a> {
    alloc: TreeAllocRef,
    grammar: &'static Grammar,
    source: &'a str,
    pub root: RedNode,
    pub messages: ParserMessages,
    pub semantic_commands: Vec<runtime::Command>,
}

impl<'a> Result<'a> {
    pub fn format_messages(&self) -> String {
        crate::parsec::display::format_messages_with_source(
            &self.grammar,
            &self.messages,
            self.source,
        )
    }

    pub fn format_ast(&self) -> String {
        format_ast(&self.grammar, &self.root, &self.alloc, self.source)
    }

    pub fn view(self) -> View<'a> {
        View::new(self.grammar, self.alloc, self.source, self.root.green, 0)
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct IncrementalReuseStats {
    pub lookups: usize,
    pub hits: usize,
    pub inserts: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct StackEntry {
    node: GreenId,
    binds_state: bool,
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
    slice: Box<str>,
    green: Option<GreenId>,
    relative_messages: ParserMessages,
}

pub struct Parser {
    pub grammar: &'static Grammar,
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
    reuse_enabled: bool,
    reuse_cache_failures: bool,
    reuse_cache: LruCache<ParseRuleCacheKey, ParseRuleCacheEntry>,
    reuse_stats: IncrementalReuseStats,
    recovery_cache: RecoveryCache,
    recovery_specs_cache: Option<RecoverySpecs>,
}

impl Parser {
    pub fn new(grammar: impl Into<&'static Grammar>) -> Self {
        let grammar = grammar.into();
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
            reuse_enabled: true,
            reuse_cache_failures: true,
            reuse_cache: LruCache::new(DEFAULT_REUSE_CAPACITY),
            reuse_stats: IncrementalReuseStats::default(),
            recovery_cache: RecoveryCache::default(),
            recovery_specs_cache: None,
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

    pub(crate) fn recovery_specs(&mut self) -> Option<&RecoverySpecs> {
        if self.recovery_specs_cache.is_none() {
            let strategy = self.build_recovery_strategy();
            self.recovery_specs_cache =
                Some(RecoverySpecs::from_text_with_strategy(&self.text, strategy));
        }
        self.recovery_specs_cache.as_ref()
    }

    pub(crate) fn recovery_strategy(&mut self) -> Option<&ErrorRecoveryStrategy> {
        self.recovery_specs().map(|specs| &specs.strategy)
    }

    pub(crate) fn set_insert_pos(&mut self, pos: Option<usize>) {
        self.inc_insert_pos = pos;
    }

    pub(crate) fn set_text(&mut self, text: &str) {
        self.text = text.to_string();
        self.recovery_specs_cache = None;
    }

    pub(crate) fn clear_reuse_cache(&mut self) {
        self.reuse_cache.clear();
    }

    pub(crate) fn reuse_stats(&self) -> IncrementalReuseStats {
        self.reuse_stats
    }

    pub fn parse_text<'a>(&'a mut self, text: &'a str) -> Result<'a> {
        self.text = text.to_string();
        self.recovery_specs_cache = None;
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
        // Track open delimiters encountered during parsing for scope recovery
        // (Nilsson-Nyman 2009 §4).
        let mut open_scope_stack: Vec<OpenScopeToken> = Vec::new();
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
                        let token_start = self.pos;
                        self.consume(token_len);
                        state_stack.push(next_state);
                        node_stack.push(StackEntry {
                            node: token_node,
                            binds_state: true,
                        });
                        // Track open/close bracket tokens for scope recovery.
                        let bridge_specs = &self.grammar.bridge_specs;
                        if bridge_specs.iter().any(|b| b.open == term_idx) {
                            open_scope_stack.push(OpenScopeToken {
                                term_idx,
                                start: token_start,
                            });
                        } else if let Some(pos) = open_scope_stack.iter().rposition(|t| {
                            bridge_specs
                                .iter()
                                .any(|b| b.open == t.term_idx && b.close == term_idx)
                        }) {
                            open_scope_stack.truncate(pos);
                        }
                        #[cfg(test)]
                        if std::env::var("TRACE_PARSE").is_ok() {
                            let name = self
                                .grammar
                                .table
                                .terminals
                                .get(term_idx)
                                .map(|m| m.display())
                                .unwrap_or_default();
                            eprintln!(
                                "[parse_text] Shift term={term_idx}({name}) len={token_len} → state={next_state}, nd={}",
                                node_stack.len()
                            );
                        }
                    }
                    Action::Reduce(prod_idx) => {
                        #[cfg(test)]
                        if std::env::var("TRACE_PARSE").is_ok() {
                            let prod = &self.grammar.table.productions[prod_idx];
                            eprintln!(
                                "[parse_text] Reduce prod={prod_idx}(lhs={},rhs={}), nd={}",
                                prod.lhs,
                                prod.rhs.len(),
                                node_stack.len()
                            );
                        }
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
                                Tag::new_error(ParsecError::UnexpectedToken {
                                    expected: Vec::new(),
                                }),
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
                // Scope recovery (Nilsson-Nyman 2009 §4): before falling back to
                // CPCT+, try to skip to the nearest matching close delimiter.
                //
                // However, do NOT attempt scope recovery when we are in a
                // "reduce-only" LR state (a state whose action table contains
                // only Reduce actions and no Shift actions).  In such states the
                // parser has already fully matched a construct and simply needs
                // the right lookahead token to trigger the reduction.  Scope
                // recovery in this situation tends to skip over valid content
                // (e.g. the next key string), burying the skipped text inside
                // the just-matched construct.  CPCT+ can instead insert the
                // missing FOLLOW token at cost 1, which triggers the correct
                // reduction chain and allows the subsequent valid input to be
                // parsed normally.
                let has_shift_in_current_state = {
                    let cur = *state_stack.last().unwrap();
                    self.grammar.analysis.states[cur]
                        .actions
                        .values()
                        .any(|a| matches!(a, Action::Shift(_)))
                };
                let scope_result = if has_shift_in_current_state {
                    scope_recover(
                        &self.grammar.bridge_specs,
                        &self.grammar.recovery_delimiters,
                        &self.grammar.table.terminals,
                        &self.text,
                        self.pos,
                        &open_scope_stack,
                    )
                } else {
                    None
                };
                if let Some(sr) = scope_result {
                    let skip_len = sr.skip_to - self.pos;
                    if skip_len > 0 {
                        // Use actual expected terminals (from current state) so the
                        // UnexpectedToken node carries meaningful diagnostic information.
                        let expected = self.expected_ids(*state_stack.last().unwrap());
                        self.push_unexpected_trimmed(self.pos, sr.skip_to, expected.clone());
                        let error_node = self.alloc.alloc(
                            Tag::new_error(ParsecError::UnexpectedToken { expected }),
                            vec![],
                            skip_len,
                        );
                        node_stack.push(StackEntry {
                            node: error_node,
                            binds_state: false,
                        });
                        self.consume(skip_len);
                        if matches!(sr.stop, ScopeStop::Close) {
                            // On close-stop, drop inner opens up to the matched scope.
                            if let Some(stack_pos) = open_scope_stack
                                .iter()
                                .rposition(|t| t.term_idx == sr.bridge.open)
                            {
                                open_scope_stack.truncate(stack_pos);
                            }
                        }
                        continue;
                    }
                }

                if let Some(boundary_term) = self.current_recovery_boundary(&open_scope_stack) {
                    if self.try_insert_missing_before_boundary(
                        &self.grammar.analysis,
                        boundary_term,
                        &mut state_stack,
                        &mut node_stack,
                    ) {
                        continue;
                    }
                }

                // Panic-mode fast skip: if the lexer produced a completely unrecognised
                // token (UNKNOWN_TOKEN), nothing in the grammar can consume it and
                // CPCT+ would just burn its full timeout budget looking for "Delete"
                // repairs.  Instead, scan forward until we reach a character that
                // matches at least one expected terminal (or EOF), emit a single
                // UnexpectedToken error for the whole skipped region, and continue
                // parsing.  This turns a per-character O(timeout) cost into O(n).
                if term_idx == UNKNOWN_TOKEN {
                    let skip_start = self.pos;
                    let expected = self.expected_ids(*state_stack.last().unwrap());
                    // Advance at least one char, then keep going while no expected
                    // terminal can lex at the current position.
                    let first_char_len = self.text[self.pos..]
                        .chars()
                        .next()
                        .map(|c| c.len_utf8())
                        .unwrap_or(1);
                    self.pos += first_char_len.min(self.text.len() - self.pos);
                    while self.pos < self.text.len() {
                        let any_matches = expected.iter().any(|&tid| {
                            let mut p = self.pos;
                            self.grammar
                                .table
                                .terminals
                                .get(tid)
                                .and_then(|m| m.matches(&self.text, &mut p))
                                .is_some()
                        });
                        if any_matches {
                            break;
                        }
                        let ch_len = self.text[self.pos..]
                            .chars()
                            .next()
                            .map(|c| c.len_utf8())
                            .unwrap_or(1);
                        self.pos += ch_len;
                    }
                    let skip_end = self.pos;
                    if skip_end > skip_start {
                        self.push_unexpected_trimmed(skip_start, skip_end, expected.clone());
                        let error_node = self.alloc.alloc(
                            Tag::new_error(ParsecError::UnexpectedToken { expected }),
                            vec![],
                            skip_end - skip_start,
                        );
                        node_stack.push(StackEntry {
                            node: error_node,
                            binds_state: false,
                        });
                    }
                    // If we consumed all remaining input during the skip, nothing
                    // further can be parsed — break out immediately to avoid a
                    // secondary CPCT+ invocation at EOF position.
                    if self.pos >= self.text.len() {
                        break;
                    }
                    continue;
                }

                // Short-circuit: if we are already at EOF (or only trailing whitespace remains),
                // do NOT invoke CPCT+ — it would burn its full 500ms timeout searching for
                // insertions with no remaining input to shift.  Instead, break out and let
                // `force_accept` insert missing tokens by following the LR automaton from the
                // current state, which is fast and semantically equivalent.
                if self.pos >= self.text.len() || self.text[self.pos..].trim().is_empty() {
                    break;
                }

                // CPCT+ error recovery: find minimum cost repair sequences and apply the first.
                let repairs = recover(
                    &self.grammar.analysis,
                    &self.grammar.table.productions,
                    &self.grammar.table.terminals,
                    &self.grammar.bracketed_terminals,
                    &self.text,
                    self.pos,
                    &state_stack,
                    self.string_opened,
                    &self.config.recovery,
                    Some(&mut self.recovery_cache),
                );

                if repairs.is_empty() {
                    self.messages.push(ParserMessage::new_unexpected(
                        Span::new(self.pos, self.pos),
                        self.expected_ids(*state_stack.last().unwrap()),
                    ));
                    break;
                }

                let ops = &repairs[0];
                if ops.is_empty() {
                    self.messages.push(ParserMessage::new_unexpected(
                        Span::new(self.pos, self.pos),
                        self.expected_ids(*state_stack.last().unwrap()),
                    ));
                    break;
                }

                let old_pos = self.pos;
                let old_state_stack = state_stack.clone();
                let old_node_stack = node_stack.clone();

                if !self.apply_repair_ops(ops, &mut state_stack, &mut node_stack) {
                    self.messages.push(ParserMessage::new_unexpected(
                        Span::new(self.pos, self.pos),
                        self.expected_ids(*state_stack.last().unwrap()),
                    ));
                    break;
                }

                if self.pos == old_pos
                    && state_stack == old_state_stack
                    && node_stack == old_node_stack
                {
                    self.messages.push(ParserMessage::new_unexpected(
                        Span::new(self.pos, self.pos),
                        self.expected_ids(*state_stack.last().unwrap()),
                    ));
                    break;
                }
            }
        }

        // Force-accept phase: if the main loop exited before reaching the
        // Accept action, we can still drive the parser to EOF by processing
        // EOF-lookahead actions and inserting MissingToken nodes.
        //
        // However, only do this when we are already at trailing EOF/whitespace.
        // If non-whitespace input remains, synthetic insertion can keep growing
        // zero-width structures without consuming input (especially on
        // left-recursive grammars), yielding pathological MissingToken chains.
        if self.pos >= self.text.len() || self.text[self.pos..].trim().is_empty() {
            self.force_accept(&mut state_stack, &mut node_stack);
        }

        let root_green = self.finalize_root(&mut node_stack);
        self.prime_reuse_from_tree(root_green, 0);

        Result {
            source: &self.text,
            grammar: self.grammar,
            alloc: self.alloc.clone(),
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
        self.recovery_cache.clear();

        let cache_key = self.build_parse_rule_cache_key(
            rule_ix,
            expected_width,
            &self.text[pos..pos + expected_width],
        );
        if let Some(cached) = self.lookup_parse_rule_cache(&cache_key, pos, expected_width) {
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
                return self.finalize_parse_rule_failure(
                    cache_key,
                    pos,
                    old_pos,
                    old_string_opened,
                    expected_width,
                );
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
                        return self.finalize_parse_rule_failure(
                            cache_key,
                            pos,
                            old_pos,
                            old_string_opened,
                            expected_width,
                        );
                    }
                }
                Action::Accept => break,
            }
        }

        let parsed_green = self.finalize_root(&mut node_stack);
        let width = self.alloc.get_node(parsed_green).width;
        let Some(parsed_rule_green) = self.extract_rule_node(parsed_green, rule_ix) else {
            return self.finalize_parse_rule_failure(
                cache_key,
                pos,
                old_pos,
                old_string_opened,
                expected_width,
            );
        };
        let parsed_rule_width = self.alloc.get_node(parsed_rule_green).width;
        if width != expected_width || parsed_rule_width != expected_width {
            return self.finalize_parse_rule_failure(
                cache_key,
                pos,
                old_pos,
                old_string_opened,
                expected_width,
            );
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
                self.text[pos..pos + expected_width].to_string(),
                Some(parsed_rule_green),
                self.messages.clone(),
                pos,
            );
        }
        Some(parsed_rule_green)
    }

    fn finalize_parse_rule_failure(
        &mut self,
        cache_key: ParseRuleCacheKey,
        pos: usize,
        old_pos: usize,
        old_string_opened: bool,
        expected_width: usize,
    ) -> Option<GreenId> {
        self.pos = old_pos;
        self.string_opened = old_string_opened;

        if self.messages.is_empty() {
            self.messages.push(ParserMessage::new_unexpected(
                Span::new(pos, pos + expected_width),
                Vec::new(),
            ));
        }

        let error_green = self.alloc.alloc(
            Tag::new_error(ParsecError::Incomplete),
            vec![],
            expected_width,
        );
        self.newly_computed_nodes
            .push(Span::new(pos, pos + expected_width));

        self.store_parse_rule_cache(
            cache_key,
            self.text[pos..pos + expected_width].to_string(),
            None,
            self.messages.clone(),
            pos,
        );
        Some(error_green)
    }

    fn analysis_for_rule(&self, rule_ix: usize) -> Arc<GrammarStateAnalysis> {
        if rule_ix == self.grammar.table.start_rule {
            return Arc::clone(&self.grammar.analysis);
        }
        // All analyses pre-warmed at grammar construction time — O(1) lookup.
        Arc::clone(
            self.grammar
                .rule_analyses
                .get(&rule_ix)
                .expect("rule_ix should be pre-warmed in grammar"),
        )
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
        pos: usize,
        expected_width: usize,
    ) -> Option<Option<GreenId>> {
        if !self.reuse_enabled {
            return None;
        }

        self.reuse_stats.lookups += 1;
        let slice = &self.text[pos..pos + expected_width];
        let slice_matches = self
            .reuse_cache
            .peek(cache_key)
            .is_some_and(|cached| cached.slice.as_ref() == slice);
        if !slice_matches {
            return None;
        }
        self.reuse_cache.touch_key(cache_key);
        let cached = self.reuse_cache.peek(cache_key)?;

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

        if let Some(green) = cached.green {
            if matches!(self.alloc.get_node(green).tag, Tag::Error(_)) {
                return None;
            }
        }

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
                slice: slice.into_boxed_str(),
                green,
                relative_messages,
            },
        );
        self.reuse_stats.inserts += 1;
    }

    pub fn prime_reuse_from_tree(&mut self, green: GreenId, offset: usize) {
        if !self.reuse_enabled {
            return;
        }
        if !self.messages.is_empty() {
            return;
        }
        self.prime_reuse_subtree(green, offset);
    }

    fn prime_reuse_subtree(&mut self, green: GreenId, offset: usize) {
        let mut stack = vec![(green, offset)];
        while let Some((green, offset)) = stack.pop() {
            let (tag, width, children) = {
                let node = self.alloc.get_node(green);
                (node.tag.clone(), node.width, node.children.clone())
            };

            let end = offset.saturating_add(width);
            if end > self.text.len() {
                continue;
            }

            if let Tag::Rule { rule_ix, .. } = tag {
                let slice = self.text[offset..end].to_string();
                let cache_key = self.build_parse_rule_cache_key(rule_ix, width, &slice);
                self.reuse_cache.insert(
                    cache_key,
                    ParseRuleCacheEntry {
                        slice: slice.into_boxed_str(),
                        green: Some(green),
                        relative_messages: Vec::new(),
                    },
                );
                self.reuse_stats.inserts += 1;
            }

            let mut child_offset = offset;
            for child in children.into_iter().rev() {
                let width = self.alloc.get_node(child).width;
                stack.push((child, child_offset));
                child_offset += width;
            }
        }
    }

    fn extract_rule_node(&self, green: GreenId, rule_ix: usize) -> Option<GreenId> {
        let mut stack = vec![green];
        while let Some(green) = stack.pop() {
            let node = self.alloc.get_node(green);
            match &node.tag {
                Tag::Rule {
                    rule_ix: current, ..
                } if *current == rule_ix => return Some(green),
                _ => {
                    stack.extend(node.children.iter().rev().copied());
                }
            }
        }
        None
    }

    fn build_recovery_strategy(&self) -> ErrorRecoveryStrategy {
        let mut strategy = ErrorRecoveryStrategy::new();
        strategy.sync_tokens = self
            .grammar
            .recovery_delimiters
            .iter()
            .filter_map(|&ix| self.grammar.table.terminals.get(ix).cloned())
            .collect();

        for (state_ix, state) in self.grammar.analysis.states.iter().enumerate() {
            if state
                .actions
                .keys()
                .any(|term| *term == EOF_TOKEN || self.grammar.recovery_delimiters.contains(term))
            {
                strategy.recovery_states.insert(state_ix);
            }
        }

        strategy
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
            if self.is_bracketed_terminal(idx) && !self.string_opened {
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
            let node = self.alloc.alloc(
                Tag::new_error(ParsecError::UnexpectedToken {
                    expected: expected.map(|items| items.to_vec()).unwrap_or_default(),
                }),
                vec![],
                len,
            );
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
            if self.is_bracketed_terminal(idx) && !self.string_opened {
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
                    // Allow string-body insertion only at a true parse boundary
                    // (EOF / trailing whitespace) for truncated-input completion.
                    let at_boundary =
                        self.pos >= self.text.len() || self.text[self.pos..].trim().is_empty();
                    if self.is_bracketed_terminal(term_ix) && !at_boundary {
                        return false;
                    }
                    // If the terminal actually matches at the current position
                    // with zero width (e.g. EndOfInput terminal when pos is at
                    // end of source), treat it as a real token, not a MissingToken.
                    // This prevents phantom MissingToken(EndOfInput) nodes when
                    // CPCT+ inserts the EOF terminal as a repair at true EOF.
                    let actually_present = term_ix != EOF_TOKEN && {
                        let matcher = self.grammar.table.terminals.get(term_ix);
                        let mut probe = self.pos;
                        matches!(
                            matcher.and_then(|m| m.matches(&self.text, &mut probe)),
                            Some(_) if probe == self.pos
                        )
                    };
                    let token_node = if term_ix == EOF_TOKEN || actually_present {
                        self.alloc.alloc_token(Tag::new_token(term_ix), 0)
                    } else {
                        self.messages.push(ParserMessage::new_missing(
                            Span::new(self.pos, self.pos),
                            vec![term_ix],
                        ));
                        self.alloc.alloc(
                            Tag::new_error(ParsecError::MissingToken {
                                expected: vec![term_ix],
                            }),
                            vec![],
                            0,
                        )
                    };
                    if !self.apply_terminal(term_ix, 0, token_node, false, state_stack, node_stack)
                    {
                        return false;
                    }
                }
                RepairOp::Delete => {
                    // Peek ahead: if the following op is an Insert of a keyword
                    // terminal (one with a fixed preview string like "true",
                    // "{", etc.), combine Delete+Insert into a single
                    // UnexpectedToken that is shifted as that terminal.  The
                    // existing passthrough_unexpected logic in perform_reduce then
                    // threads it through the grammar reductions, so the node
                    // becomes the value child rather than an unbound sibling next
                    // to a zero-width phantom MissingToken.
                    if i + 1 < ops.len() {
                        if let RepairOp::Insert(next_term_ix) = ops[i + 1] {
                            let is_keyword = next_term_ix != EOF_TOKEN
                                && self
                                    .grammar
                                    .table
                                    .terminals
                                    .get(next_term_ix)
                                    .and_then(|m| m.preview())
                                    .is_some();
                            if is_keyword {
                                let expected = self.expected_ids(*state_stack.last().unwrap());
                                let (_term, len, _node) = self.lex(None);
                                if len > 0 {
                                    self.push_unexpected_trimmed(
                                        self.pos,
                                        self.pos + len,
                                        expected.clone(),
                                    );
                                    let error_node = self.alloc.alloc(
                                        Tag::new_error(ParsecError::UnexpectedToken { expected }),
                                        vec![],
                                        len,
                                    );
                                    self.consume(len);
                                    if !self.apply_terminal(
                                        next_term_ix,
                                        0,
                                        error_node,
                                        false,
                                        state_stack,
                                        node_stack,
                                    ) {
                                        return false;
                                    }
                                    i += 2;
                                    continue;
                                }
                            }
                        }
                    }
                    // Normal Delete: push as an unbound error node.
                    let expected = self.expected_ids(*state_stack.last().unwrap());
                    let (_term, len, _node) = self.lex(None);
                    if len == 0 {
                        return false;
                    }
                    self.push_unexpected_trimmed(self.pos, self.pos + len, expected.clone());
                    let deleted_node = self.alloc.alloc(
                        Tag::new_error(ParsecError::UnexpectedToken { expected }),
                        vec![],
                        len,
                    );
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

    fn apply_terminal(
        &mut self,
        term: usize,
        len: usize,
        token_node: GreenId,
        consume_input: bool,
        state_stack: &mut Vec<usize>,
        node_stack: &mut Vec<StackEntry>,
    ) -> bool {
        let analysis = Arc::clone(&self.grammar.analysis);
        self.apply_terminal_with_analysis(
            term,
            len,
            token_node,
            consume_input,
            state_stack,
            node_stack,
            analysis.as_ref(),
        )
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

    /// Drive the parser to Accept by inserting zero-width MissingToken nodes.
    ///
    /// Uses a BFS over the LR state automaton to find the *shortest* sequence of
    /// terminal insertions that reaches a state where the EOF action is `Accept`.
    /// This guarantees full recovery even when multiple tokens are missing (e.g.
    /// `{"a"` needs `:`, a value node, and `}` before the grammar can close).
    fn force_accept(&mut self, state_stack: &mut Vec<usize>, node_stack: &mut Vec<StackEntry>) {
        // Safety limit: bail out if no progression after many rounds.
        let mut rounds = 0usize;
        const MAX_ROUNDS: usize = 64;

        loop {
            rounds += 1;
            if rounds > MAX_ROUNDS {
                break;
            }

            let current_state_idx = *state_stack.last().unwrap();

            // 1. EOF action → Accept: done.
            // 2. EOF action → Reduce: perform it, loop.
            match self.grammar.analysis.states[current_state_idx]
                .actions
                .get(&EOF_TOKEN)
                .cloned()
            {
                Some(Action::Accept) => break,
                Some(Action::Reduce(prod_idx)) => {
                    if !self.perform_reduce(prod_idx, state_stack, node_stack) {
                        break;
                    }
                    continue;
                }
                _ => {}
            }

            // 3. No EOF action but state has Reduce-only actions (e.g. nullable or
            //    epsilon productions from generated rules) — drain them first.
            {
                let state = &self.grammar.analysis.states[current_state_idx];
                let has_shift = state
                    .actions
                    .iter()
                    .any(|(&t, a)| t != EOF_TOKEN && matches!(a, Action::Shift(_)));
                if !has_shift {
                    let reduces: Vec<usize> = state
                        .actions
                        .values()
                        .filter_map(|a| {
                            if let Action::Reduce(p) = a {
                                Some(*p)
                            } else {
                                None
                            }
                        })
                        .collect();
                    let eps = reduces
                        .iter()
                        .copied()
                        .find(|&p| self.grammar.table.productions[p].rhs.is_empty());
                    let chosen = eps.or_else(|| {
                        reduces
                            .into_iter()
                            .min_by_key(|&p| self.grammar.table.productions[p].rhs.len())
                    });
                    if let Some(prod_idx) = chosen {
                        if !self.perform_reduce(prod_idx, state_stack, node_stack) {
                            break;
                        }
                        continue;
                    }
                }
            }

            // 4. BFS in the LR state automaton to find the shortest sequence of
            //    terminal inserts that brings the stack to a can-accept state.
            //    This replaces the one-step-lookahead heuristic and handles cases
            //    like `{"a"` where `:`, value, and `}` are all missing.
            let analysis = Arc::clone(&self.grammar.analysis);
            let productions = &self.grammar.table.productions;
            match lr_completions_to_accept(&analysis, productions, state_stack, 24) {
                Some(terms) if !terms.is_empty() => {
                    for term_ix in terms {
                        if term_ix == EOF_TOKEN {
                            break;
                        }
                        // If the terminal already matches zero-width at current pos
                        // (e.g. EndOfInput), emit a real token instead of a MissingToken.
                        let actually_present = {
                            let matcher = self.grammar.table.terminals.get(term_ix);
                            let mut probe = self.pos;
                            matches!(
                                matcher.and_then(|m| m.matches(&self.text, &mut probe)),
                                Some(_) if probe == self.pos
                            )
                        };
                        let token_node = if actually_present {
                            self.alloc.alloc_token(Tag::new_token(term_ix), 0)
                        } else {
                            self.messages.push(ParserMessage::new_missing(
                                Span::new(self.pos, self.pos),
                                vec![term_ix],
                            ));
                            self.alloc.alloc(
                                Tag::new_error(ParsecError::MissingToken {
                                    expected: vec![term_ix],
                                }),
                                vec![],
                                0,
                            )
                        };
                        if !self.apply_terminal(
                            term_ix,
                            0,
                            token_node,
                            false,
                            state_stack,
                            node_stack,
                        ) {
                            break;
                        }
                    }
                    // Loop back: re-check EOF action after applying the insertions.
                }
                Some(_) => break, // empty sequence → already at accept
                None => break,    // BFS found no path; give up
            }
        }
    }

    fn finalize_root(&self, node_stack: &mut Vec<StackEntry>) -> GreenId {
        if node_stack.is_empty() {
            return self
                .alloc
                .alloc(Tag::new_error(ParsecError::Incomplete), vec![], 0);
        }
        if node_stack.len() == 1 {
            return node_stack.pop().map(|e| e.node).unwrap_or_else(|| {
                self.alloc
                    .alloc(Tag::new_error(ParsecError::Incomplete), vec![], 0)
            });
        }

        // If there is exactly one bound (grammar) node on the stack, return it
        // directly.  The remaining unbound entries are error/recovery nodes that
        // were pushed outside the normal LR reduce chain (e.g. from scope
        // recovery or CPCT+ Delete ops).  Their text spans are already covered
        // by parser messages so they need not appear as an Incomplete wrapper.
        let bound_count = node_stack.iter().filter(|e| e.binds_state).count();
        if bound_count == 1 {
            // Find and extract the single bound node, preserving order of unbound ones.
            let bound_pos = node_stack.iter().rposition(|e| e.binds_state).unwrap();
            let root_entry = node_stack.remove(bound_pos);
            // The remaining unbound nodes are discarded (their content is in messages).
            node_stack.clear();
            return root_entry.node;
        }

        // Filter out zero-width grammar (non-error) nodes — these are phantom
        // force_accept artifacts (e.g. a `start [width:0]` built entirely from
        // MissingToken children) that carry no real text and would only clutter
        // the [Incomplete] wrapper alongside genuine error/content nodes.
        let children: Vec<GreenId> = node_stack
            .drain(..)
            .map(|e| e.node)
            .filter(|&id| {
                let n = self.alloc.get_node(id);
                let is_phantom_grammar = n.width == 0 && !matches!(n.tag, Tag::Error(_));
                !is_phantom_grammar
            })
            .collect();
        if children.is_empty() {
            return self
                .alloc
                .alloc(Tag::new_error(ParsecError::Incomplete), vec![], 0);
        }
        if children.len() == 1 {
            return children[0];
        }
        let width: usize = children
            .iter()
            .map(|id| self.alloc.get_node(*id).width)
            .sum();
        self.alloc
            .alloc(Tag::new_error(ParsecError::Incomplete), children, width)
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
            None => {
                #[cfg(test)]
                eprintln!(
                    "[perform_reduce] pop_reduce_entries({rhs_len}) returned None, state_stack={state_stack:?}, node_stack.len()={}",
                    node_stack.len()
                );
                return false;
            }
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

        // Pass through the error node directly (without creating a rule wrapper) when:
        //   (a) UnexpectedToken — already handled for all single-terminal productions.
        //   (b) MissingToken — but ONLY when the terminal is a keyword/literal (has a
        //       non-None preview, e.g. "true", "false", "null", "{", …).  Pattern
        //       terminals like `tt(NUMS)` or `tt(STRING)` have preview = None because
        //       their matched text carries semantic value (the number's digits, the
        //       string content, etc.), so we keep the rule wrapper (e.g. `primary`,
        //       `number`) to give the semantic pass something meaningful to hang
        //       on.  For keyword terminals the wrapper only names an arbitrary grammar
        //       alternative chosen by the repair engine (e.g. `boolean` chosen just
        //       because "true" happened to be the first insert candidate), so
        //       suppressing it produces a cleaner error representation.
        let passthrough_unexpected = is_single_terminal_prod && children.len() == 1 && {
            let child_tag = &self.alloc.get_node(children[0]).tag;
            matches!(child_tag, Tag::Error(ParsecError::UnexpectedToken { .. }))
                || (matches!(child_tag, Tag::Error(ParsecError::MissingToken { .. })) && {
                    // Terminal is a keyword/literal iff it has a non-None preview.
                    let prod = &self.grammar.table.productions[prod_idx];
                    matches!(prod.rhs.first(), Some(Symbol::Terminal(t))
                                if self.grammar.table.terminals
                                    .get(*t)
                                    .and_then(|m| m.preview())
                                    .is_some())
                })
        };

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
            #[cfg(test)]
            {
                let all_gotos: Vec<_> = top_state.goto.iter().collect();
                eprintln!(
                    "[perform_reduce] goto[lhs={lhs}] not found in top_state_idx={top_state_idx}, available_goto={all_gotos:?}, state_stack={state_stack:?}"
                );
            }
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

    fn current_recovery_boundary(&self, open_scope_stack: &[OpenScopeToken]) -> Option<usize> {
        let rest = self.text.get(self.pos..)?;

        let mut candidates = self.grammar.recovery_delimiters.clone();
        for open in open_scope_stack.iter().rev() {
            if let Some(close) = self
                .grammar
                .bridge_specs
                .iter()
                .find(|bridge| bridge.open == open.term_idx)
                .map(|bridge| bridge.close)
            {
                if !candidates.contains(&close) {
                    candidates.push(close);
                }
            }
        }

        candidates.into_iter().find(|&term_ix| {
            self.grammar
                .table
                .terminals
                .get(term_ix)
                .and_then(|m| m.preview())
                .is_some_and(|preview| rest.starts_with(preview))
        })
    }

    fn try_insert_missing_before_boundary(
        &mut self,
        analysis: &GrammarStateAnalysis,
        boundary_term: usize,
        state_stack: &mut Vec<usize>,
        node_stack: &mut Vec<StackEntry>,
    ) -> bool {
        let current_state_idx = *state_stack.last().unwrap();
        let mut expected = self.expected_ids_for_analysis(analysis, current_state_idx, true);
        if expected.is_empty() {
            return false;
        }

        expected.sort_by_key(|term_ix| {
            let preview = self
                .grammar
                .table
                .terminals
                .get(*term_ix)
                .and_then(|m| m.preview());
            (
                preview.is_some(),
                preview.map(str::len).unwrap_or(0),
                *term_ix,
            )
        });

        let chosen = expected.into_iter().find(|&candidate| {
            let mut sim_state_stack = state_stack.clone();
            let mut sim_node_stack = node_stack.clone();
            let missing = self.alloc.alloc(
                Tag::new_error(ParsecError::MissingToken {
                    expected: vec![candidate],
                }),
                vec![],
                0,
            );
            if !self.apply_terminal_with_analysis(
                candidate,
                0,
                missing,
                false,
                &mut sim_state_stack,
                &mut sim_node_stack,
                analysis,
            ) {
                return false;
            }

            let boundary = self.alloc.alloc_token(Tag::new_token(boundary_term), 0);
            self.apply_terminal_with_analysis(
                boundary_term,
                0,
                boundary,
                false,
                &mut sim_state_stack,
                &mut sim_node_stack,
                analysis,
            )
        });

        let Some(chosen) = chosen else {
            return false;
        };

        self.messages.push(ParserMessage::new_missing(
            Span::new(self.pos, self.pos),
            vec![chosen],
        ));
        let missing = self.alloc.alloc(
            Tag::new_error(ParsecError::MissingToken {
                expected: vec![chosen],
            }),
            vec![],
            0,
        );
        self.apply_terminal_with_analysis(
            chosen,
            0,
            missing,
            false,
            state_stack,
            node_stack,
            analysis,
        )
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

    fn is_bracketed_terminal(&self, term_ix: usize) -> bool {
        self.grammar.bracketed_terminals.contains(&term_ix)
    }
}

// ── LR completion BFS ────────────────────────────────────────────────────────

/// Find the shortest sequence of terminal insertions that drives the given LR
/// state stack to a state where the EOF-lookahead action is `Accept`.
///
/// The BFS explores only the LR state machine (no text position), so it is
/// fast even for grammars with many rules.  `max_depth` limits the number of
/// inserted tokens; grammars rarely need more than 8–10 even for deeply nested
/// partial input.
fn lr_completions_to_accept(
    analysis: &GrammarStateAnalysis,
    productions: &[crate::grammar::ir::Production],
    state_stack: &[usize],
    max_depth: usize,
) -> Option<Vec<usize>> {
    // Check if a state stack can reach Accept purely via EOF reduces.
    let can_accept_stack = |stack: &Vec<usize>| -> bool {
        if stack.is_empty() {
            return false;
        }
        let mut stack = stack.clone();
        let mut steps = 0usize;
        loop {
            steps += 1;
            if steps > 512 {
                return false;
            }
            let state = *stack.last().unwrap();
            match analysis.states[state].actions.get(&EOF_TOKEN).cloned() {
                Some(Action::Accept) => return true,
                Some(Action::Reduce(prod_ix)) => {
                    let prod = &productions[prod_ix];
                    let new_len = stack.len().saturating_sub(prod.rhs.len());
                    stack.truncate(new_len);
                    if stack.is_empty() {
                        return false;
                    }
                    let top = *stack.last().unwrap();
                    let goto = match analysis.states[top].goto.get(&prod.lhs) {
                        Some(&g) => g,
                        None => return false,
                    };
                    stack.push(goto);
                }
                _ => return false,
            }
        }
    };

    // Simulate shifting terminal `term` on `stack`, following reduce/goto
    // chains as the real parser would.  Returns the new stack or `None` if
    // no action is defined for `term` in the current top state.
    let shift_on_stack = |stack: &Vec<usize>, term: usize| -> Option<Vec<usize>> {
        if stack.is_empty() {
            return None;
        }
        let mut stack = stack.clone();
        let mut steps = 0usize;
        loop {
            steps += 1;
            if steps > 256 {
                return None;
            }
            let state = *stack.last().unwrap();
            match analysis.states[state].actions.get(&term).cloned() {
                Some(Action::Shift(next_state)) => {
                    stack.push(next_state);
                    return Some(stack);
                }
                Some(Action::Reduce(prod_ix)) => {
                    let prod = &productions[prod_ix];
                    let new_len = stack.len().saturating_sub(prod.rhs.len());
                    stack.truncate(new_len);
                    if stack.is_empty() {
                        return None;
                    }
                    let top = *stack.last().unwrap();
                    let goto = match analysis.states[top].goto.get(&prod.lhs) {
                        Some(&g) => g,
                        None => return None,
                    };
                    stack.push(goto);
                }
                Some(Action::Accept) => return Some(stack),
                None => return None,
            }
        }
    };

    // Fast-path: already at accept.
    let initial = state_stack.to_vec();
    if can_accept_stack(&initial) {
        return Some(vec![]);
    }

    // BFS: each entry is (state_stack, insertion_sequence_so_far).
    // Visited set is keyed on the state stack to avoid revisiting the same LR
    // configuration via different insertion orders.
    let mut queue: VecDeque<(Vec<usize>, Vec<usize>)> = VecDeque::new();
    queue.push_back((initial.clone(), vec![]));
    let mut visited: HashSet<Vec<usize>> = HashSet::new();
    visited.insert(initial);

    while let Some((stack, insertions)) = queue.pop_front() {
        if insertions.len() >= max_depth {
            continue;
        }

        // Collect candidates from the top state's action table (exclude EOF).
        let top_state = *stack.last().unwrap();
        let candidates: Vec<usize> = analysis.states[top_state]
            .actions
            .keys()
            .copied()
            .filter(|&t| t != EOF_TOKEN)
            .collect();

        for term in candidates {
            if let Some(new_stack) = shift_on_stack(&stack, term) {
                if can_accept_stack(&new_stack) {
                    let mut result = insertions.clone();
                    result.push(term);
                    return Some(result);
                }
                if !visited.contains(&new_stack) {
                    visited.insert(new_stack.clone());
                    let mut new_ins = insertions.clone();
                    new_ins.push(term);
                    queue.push_back((new_stack, new_ins));
                }
            }
        }
    }

    None
}
