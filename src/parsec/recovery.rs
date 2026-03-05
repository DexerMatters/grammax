use crate::grammar::analysis::{Action, EOF_TOKEN, GrammarStateAnalysis};
use crate::grammar::bridge::BridgeSpec;
use crate::grammar::ir::Production;
use crate::parsec::words::MatcherRef;
use rustc_hash::FxHashMap;
use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::time::{Duration, Instant};

const UNKNOWN_TOKEN: usize = usize::MAX - 1;
const MAX_NULLABLE_SHIFT_LEN: usize = 4;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct RecoveryCacheKey {
    stack_hash: u64,
    stack_state: usize,
    pos: usize,
    string_opened: bool,
}

pub type RecoveryCache = FxHashMap<RecoveryCacheKey, Vec<Vec<RepairOp>>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RepairOp {
    Insert(usize),
    Delete,
    Shift,
}

#[derive(Debug, Clone)]
pub struct RecoveryConfig {
    pub nshifts: usize, // N_shifts from paper: success when we shift this many tokens
    pub ntry: usize,    // N_try from paper: how far to simulate for ranking
    pub timeout: Duration, // Timeout from paper (0.5s recommended)
}

impl Default for RecoveryConfig {
    fn default() -> Self {
        Self {
            nshifts: 3,
            ntry: 250,
            timeout: Duration::from_millis(500),
        }
    }
}

// Parent-pointer tree for parsing stack
#[derive(Clone)]
struct StackNode {
    state: usize,
    depth: usize,
    hash: u64,
    parent: Option<Rc<StackNode>>,
}

impl StackNode {
    fn new_root(state: usize) -> Rc<Self> {
        let hash = hash_stack(None, state);
        Rc::new(Self {
            state,
            depth: 1,
            hash,
            parent: None,
        })
    }

    fn push(parent: &Rc<Self>, state: usize) -> Rc<Self> {
        let hash = hash_stack(Some(parent.hash), state);
        Rc::new(Self {
            state,
            depth: parent.depth + 1,
            hash,
            parent: Some(Rc::clone(parent)),
        })
    }

    fn pop_n(node: &Rc<Self>, n: usize) -> Option<Rc<Self>> {
        let mut cur = Rc::clone(node);
        for _ in 0..n {
            let next = cur.parent.as_ref()?.clone();
            cur = next;
        }
        Some(cur)
    }
}

// Graph-structured stack for repair sequences (for merging)
#[derive(Clone)]
struct RepairMerge {
    op: RepairOp,
    parent: Option<Rc<RepairMerge>>,
    merged: Vec<Rc<RepairMerge>>, // Merged alternative repair sequences
}

impl RepairMerge {
    fn new(op: RepairOp, parent: Option<Rc<RepairMerge>>) -> Rc<Self> {
        Rc::new(Self {
            op,
            parent,
            merged: Vec::new(),
        })
    }

    // Count trailing shifts for compatibility check
    fn count_trailing_shifts(&self) -> usize {
        let mut count = 0;
        let mut cur = Some(self);
        while let Some(node) = cur {
            match node.op {
                RepairOp::Shift => count += 1,
                _ => break,
            }
            cur = node.parent.as_ref().map(|rc| rc.as_ref());
        }
        count
    }

    // Check if ends with delete
    fn ends_with_delete(&self) -> bool {
        matches!(self.op, RepairOp::Delete)
    }

    // Expand into all repair sequences
    fn expand_all(&self) -> Vec<Vec<RepairOp>> {
        fn collect_repair(node: &Rc<RepairMerge>, ops: &mut Vec<RepairOp>) {
            if let Some(parent) = &node.parent {
                collect_repair(parent, ops);
            }
            ops.push(node.op);
        }

        fn expand_recursive(node: &Rc<RepairMerge>) -> Vec<Vec<RepairOp>> {
            if node.merged.is_empty() {
                let mut ops = Vec::new();
                collect_repair(node, &mut ops);
                vec![ops]
            } else {
                let mut result = Vec::new();
                let mut primary_ops = Vec::new();
                collect_repair(node, &mut primary_ops);
                result.push(primary_ops);
                for merged in &node.merged {
                    result.extend(expand_recursive(merged));
                }
                result
            }
        }

        expand_recursive(&Rc::new(self.clone()))
    }
}

// Configuration in the search space
#[derive(Clone)]
struct Config {
    stack: Rc<StackNode>,             // Parsing stack as parent-pointer tree
    pos: usize,                       // Position in input
    cost: usize,                      // Cost of repair sequence
    shifts: usize,                    // Number of trailing shifts
    string_opened: bool,              // Lexer context: inside quote-delimited string
    repairs: Option<Rc<RepairMerge>>, // Repair sequence as graph-structured stack
}

impl Config {
    // Check if compatible for merging (Section 5.2 of paper)
    fn compatible_for_merge(&self, other: &Self) -> bool {
        // Must have identical parsing stacks
        if self.stack.hash != other.stack.hash || self.stack.state != other.stack.state {
            return false;
        }
        // Must have identical remaining input
        if self.pos != other.pos {
            return false;
        }
        if self.string_opened != other.string_opened {
            return false;
        }
        // Must have compatible repair sequences
        match (&self.repairs, &other.repairs) {
            (None, None) => true,
            (Some(r1), Some(r2)) => {
                // Must end in same number of shifts
                if r1.count_trailing_shifts() != r2.count_trailing_shifts() {
                    return false;
                }
                // If one ends in delete, both must end in delete
                if r1.ends_with_delete() != r2.ends_with_delete() {
                    return false;
                }
                true
            }
            _ => false,
        }
    }

    // Merge with another compatible configuration
    fn merge_with(&self, other: &Self) -> Option<Self> {
        if !self.compatible_for_merge(other) {
            return None;
        }

        // Merge repair sequences using graph-structured stack
        let merged_repairs = match (&self.repairs, &other.repairs) {
            (Some(r1), Some(r2)) => {
                let mut new_r1 = (**r1).clone();
                new_r1.merged.push(Rc::clone(r2));
                Some(Rc::new(new_r1))
            }
            (r, None) | (None, r) => r.clone(),
        };

        Some(Config {
            stack: Rc::clone(&self.stack),
            pos: self.pos,
            cost: self.cost,
            shifts: self.shifts,
            string_opened: self.string_opened,
            repairs: merged_repairs,
        })
    }
}

// Hash key for detecting compatible configurations
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct ConfigKey {
    stack_hash: u64,
    stack_state: usize,
    pos: usize,
    string_opened: bool,
}

// Main CPCT+ recovery function (Section 5 of paper)
// Returns complete set of top-ranked minimum cost repair sequences
pub fn recover(
    analysis: &GrammarStateAnalysis,
    productions: &[Production],
    terminals: &[MatcherRef],
    text: &str,
    pos: usize,
    stack: &[usize],
    string_opened: bool,
    config: &RecoveryConfig,
    cache: Option<&mut RecoveryCache>,
) -> Vec<Vec<RepairOp>> {
    if stack.is_empty() {
        return Vec::new();
    }

    let stack_hash = hash_stack_slice(stack);
    let cache_key = RecoveryCacheKey {
        stack_hash,
        stack_state: *stack.last().unwrap(),
        pos,
        string_opened,
    };
    if let Some(cache) = cache.as_ref() {
        if let Some(hit) = cache.get(&cache_key) {
            return hit.clone();
        }
    }

    let start_time = Instant::now();

    // Convert stack list to parent-pointer tree
    let mut root = StackNode::new_root(stack[0]);
    for &state in &stack[1..] {
        root = StackNode::push(&root, state);
    }

    // Cost buckets for Dijkstra-like search (Section 4.3)
    let mut buckets: Vec<VecDeque<Config>> = vec![VecDeque::new(); 100];
    let initial_cfg = Config {
        stack: root,
        pos,
        cost: 0,
        shifts: 0,
        string_opened,
        repairs: None,
    };
    buckets[0].push_back(initial_cfg);

    // Use HashMap for compatible configuration merging (Section 5.2)
    // Key is (stack_hash, stack_state, pos) for fast lookup of compatible configs
    let mut visited: HashMap<ConfigKey, Vec<Config>> = HashMap::new();
    let mut successful: Vec<Config> = Vec::new();
    let mut min_cost: Option<usize> = None;

    let token_stream = TokenStream::new(terminals, text);
    let insert_candidates = build_insert_candidates(analysis, terminals);

    // Breadth-first search by cost
    for cost in 0..buckets.len() {
        // Check timeout
        if start_time.elapsed() > config.timeout {
            break;
        }

        // If we found successful configs at previous cost, we're done with this cost level
        if let Some(found_cost) = min_cost {
            if cost > found_cost {
                break;
            }
        }

        while let Some(cfg) = buckets[cost].pop_front() {
            // Timeout check
            if start_time.elapsed() > config.timeout {
                break;
            }

            // Try to merge with compatible configurations (Section 5.2)
            let key = ConfigKey {
                stack_hash: cfg.stack.hash,
                stack_state: cfg.stack.state,
                pos: cfg.pos,
                string_opened: cfg.string_opened,
            };

            if let Some(existing_configs) = visited.get_mut(&key) {
                let mut merged = false;
                for existing in existing_configs.iter_mut() {
                    if let Some(merged_cfg) = existing.merge_with(&cfg) {
                        *existing = merged_cfg;
                        merged = true;
                        break;
                    }
                }
                if merged {
                    continue; // Successfully merged, skip this config
                }
                // Not compatible with any existing, add as new alternative
                existing_configs.push(cfg.clone());
            } else {
                visited.insert(key, vec![cfg.clone()]);
            }

            // Check success conditions (Section 5.1)
            // Success: either N_shifts shifts, or accept with no meaningful remaining input.
            let no_remaining_input = cfg.pos >= text.len() || text[cfg.pos..].trim().is_empty();
            if cfg.shifts >= config.nshifts
                || (no_remaining_input && can_accept(analysis, productions, &cfg.stack))
            {
                if min_cost.is_none() {
                    min_cost = Some(cost);
                }
                successful.push(cfg);
                continue;
            }

            // Generate neighbors using →CR rules (Figure 5)

            // CR Shift (up to 1 shift at a time, per CR Shift 3)
            if let Some(next) = cr_shift(analysis, productions, terminals, &token_stream, &cfg) {
                if next.cost < buckets.len() {
                    buckets[next.cost].push_back(next);
                }
            }

            // CR Insert
            for next in cr_insert(analysis, productions, &insert_candidates, &cfg) {
                if next.cost < buckets.len() {
                    buckets[next.cost].push_back(next);
                }
            }

            // CR Delete (never follows insert to avoid duplicates)
            if !matches!(
                cfg.repairs.as_ref().map(|r| r.op),
                Some(RepairOp::Insert(_))
            ) {
                if let Some(next) = cr_delete(&token_stream, &cfg) {
                    if next.cost < buckets.len() {
                        buckets[next.cost].push_back(next);
                    }
                }
            }
        }
    }

    if successful.is_empty() {
        return Vec::new();
    }

    // Rank repair sequences (Section 5.3)
    let result = rank_and_select(
        analysis,
        productions,
        terminals,
        &token_stream,
        successful,
        config.ntry,
    );

    if let Some(cache) = cache {
        cache.insert(cache_key, result.clone());
    }

    result
}
// CR Shift: Shift at most 1 token (CR Shift 3 from Figure 7)
fn cr_shift(
    analysis: &GrammarStateAnalysis,
    productions: &[Production],
    terminals: &[MatcherRef],
    tokens: &TokenStream,
    cfg: &Config,
) -> Option<Config> {
    let (term, len) = lex_at_stream(tokens, cfg.pos);

    // Can't shift EOF
    if term == EOF_TOKEN {
        return None;
    }

    // Try to shift this terminal
    let new_stack = simulate_shift(analysis, productions, &cfg.stack, term)?;

    // Create new repair sequence with a Shift operation
    let new_repairs = RepairMerge::new(RepairOp::Shift, cfg.repairs.clone());
    let next_string_opened = if is_quote_terminal(terminals, term) {
        !cfg.string_opened
    } else {
        cfg.string_opened
    };

    Some(Config {
        stack: new_stack,
        pos: cfg.pos + len,
        cost: cfg.cost, // Shifts cost 0
        shifts: cfg.shifts + 1,
        string_opened: next_string_opened,
        repairs: Some(new_repairs),
    })
}

// CR Insert: Try inserting all reachable terminals (Figure 5)
fn cr_insert(
    analysis: &GrammarStateAnalysis,
    productions: &[Production],
    insert_candidates: &[Vec<usize>],
    cfg: &Config,
) -> Vec<Config> {
    let mut result = Vec::new();

    // Get expected terminals at current state
    if let Some(candidates) = insert_candidates.get(cfg.stack.state) {
        for &term_ix in candidates {
            // Try to shift this inserted terminal
            if let Some(new_stack) = simulate_shift(analysis, productions, &cfg.stack, term_ix) {
                let new_repairs = RepairMerge::new(RepairOp::Insert(term_ix), cfg.repairs.clone());

                result.push(Config {
                    stack: new_stack,
                    pos: cfg.pos,       // Insert doesn't advance position
                    cost: cfg.cost + 1, // Inserts cost 1
                    shifts: cfg.shifts,
                    string_opened: cfg.string_opened,
                    repairs: Some(new_repairs),
                });
            }
        }
    }

    result
}

// CR Delete: Delete next token (Figure 5)
fn cr_delete(tokens: &TokenStream, cfg: &Config) -> Option<Config> {
    let (term, len) = lex_at_stream(tokens, cfg.pos);

    // Can't delete EOF or zero-length
    if term == EOF_TOKEN || len == 0 {
        return None;
    }

    let new_repairs = RepairMerge::new(RepairOp::Delete, cfg.repairs.clone());

    Some(Config {
        stack: Rc::clone(&cfg.stack),
        pos: cfg.pos + len, // Delete advances position
        cost: cfg.cost + 1, // Deletes cost 1
        shifts: cfg.shifts,
        string_opened: cfg.string_opened,
        repairs: Some(new_repairs),
    })
}

// Rank and select the best repair sequences (Section 5.3)
fn rank_and_select(
    analysis: &GrammarStateAnalysis,
    productions: &[Production],
    terminals: &[MatcherRef],
    tokens: &TokenStream,
    successful: Vec<Config>,
    ntry: usize,
) -> Vec<Vec<RepairOp>> {
    if successful.is_empty() {
        return Vec::new();
    }

    // Rank each config by how far parsing continues
    let mut ranked: Vec<(usize, Config)> = successful
        .into_iter()
        .map(|cfg| {
            let distance = simulate_progress(
                analysis,
                productions,
                terminals,
                tokens,
                cfg.string_opened,
                &cfg.stack,
                cfg.pos,
                ntry,
            );
            (distance, cfg)
        })
        .collect();

    // Find maximum distance
    let max_distance = ranked.iter().map(|(d, _)| *d).max().unwrap_or(0);

    // Keep only those that reached max distance (top-ranked)
    ranked.retain(|(d, _)| *d == max_distance);

    // Expand all repair sequences from top-ranked configs
    let mut all_repairs: Vec<Vec<RepairOp>> = Vec::new();
    for (_dist, cfg) in ranked {
        if let Some(repairs) = cfg.repairs {
            all_repairs.extend(repairs.expand_all());
        } else {
            all_repairs.push(Vec::new());
        }
    }

    // Remove trailing shifts and deduplicate
    let mut seen: HashSet<Vec<RepairOp>> = HashSet::new();
    let mut result = Vec::new();

    for mut repairs in all_repairs {
        // Remove trailing shifts
        while repairs.last() == Some(&RepairOp::Shift) {
            repairs.pop();
        }

        // Only add if unique
        if seen.insert(repairs.clone()) {
            result.push(repairs);
        }
    }

    result
}

// Simulate how far parsing continues (for ranking)
fn simulate_progress(
    analysis: &GrammarStateAnalysis,
    productions: &[Production],
    terminals: &[MatcherRef],
    tokens: &TokenStream,
    mut string_opened: bool,
    stack: &Rc<StackNode>,
    mut pos: usize,
    max_tokens: usize,
) -> usize {
    let mut cur = Rc::clone(stack);
    let mut consumed = 0usize;

    while consumed < max_tokens {
        let (term, len) = lex_at_stream(tokens, pos);

        // Stop at EOF
        if term == EOF_TOKEN {
            break;
        }

        // Try to shift this terminal
        let next = match simulate_shift(analysis, productions, &cur, term) {
            Some(s) => s,
            None => break, // Can't shift, stop here
        };

        // Avoid infinite loops with nullable terminals
        if let Some(matcher) = terminals.get(term) {
            if matcher.is_nullable() && len > MAX_NULLABLE_SHIFT_LEN {
                break;
            }
        }

        cur = next;
        pos += len;
        if is_quote_terminal(terminals, term) {
            string_opened = !string_opened;
        }
        consumed += 1;
    }

    consumed
}

struct Token {
    term: usize,
    len: usize,
}

struct TokenStream {
    tokens: Vec<Token>,
    index_by_start: FxHashMap<usize, usize>,
    text_len: usize,
}

impl TokenStream {
    fn new(terminals: &[MatcherRef], text: &str) -> Self {
        let mut tokens = Vec::new();
        let mut index_by_start = FxHashMap::default();
        let mut pos = 0usize;
        let mut string_opened = false;

        while pos < text.len() {
            if text[pos..].trim().is_empty() {
                let len = text.len() - pos;
                let idx = tokens.len();
                tokens.push(Token {
                    term: EOF_TOKEN,
                    len,
                });
                index_by_start.insert(pos, idx);
                break;
            }

            let (term, len) = lex_at_text(terminals, text, pos, string_opened);
            if term == EOF_TOKEN && len == 0 {
                break;
            }
            let idx = tokens.len();
            tokens.push(Token { term, len });
            index_by_start.insert(pos, idx);
            pos += len;
            if is_quote_terminal(terminals, term) {
                string_opened = !string_opened;
            }
        }

        Self {
            tokens,
            index_by_start,
            text_len: text.len(),
        }
    }

    fn token_at(&self, pos: usize) -> Option<&Token> {
        self.index_by_start
            .get(&pos)
            .and_then(|idx| self.tokens.get(*idx))
    }
}

fn lex_at_stream(tokens: &TokenStream, pos: usize) -> (usize, usize) {
    if pos >= tokens.text_len {
        return (EOF_TOKEN, 0);
    }
    if let Some(tok) = tokens.token_at(pos) {
        return (tok.term, tok.len);
    }
    let mut next_start = tokens.text_len;
    for (start, _) in tokens.index_by_start.iter() {
        if *start > pos && *start < next_start {
            next_start = *start;
        }
    }
    let len = next_start.saturating_sub(pos).max(1);
    (UNKNOWN_TOKEN, len)
}

fn build_insert_candidates(
    analysis: &GrammarStateAnalysis,
    terminals: &[MatcherRef],
) -> Vec<Vec<usize>> {
    analysis
        .states
        .iter()
        .map(|state_info| {
            state_info
                .actions
                .keys()
                .copied()
                .filter(|term_ix| {
                    *term_ix != EOF_TOKEN
                        && *term_ix != UNKNOWN_TOKEN
                        && terminals.get(*term_ix).is_some_and(|m| m.is_consuming())
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

// Check if we can accept from this stack
fn can_accept(
    analysis: &GrammarStateAnalysis,
    productions: &[Production],
    stack: &Rc<StackNode>,
) -> bool {
    let mut cur = Rc::clone(stack);
    let mut seen = HashSet::new();
    let mut steps = 0usize;

    loop {
        steps += 1;
        if steps > 512 {
            return false;
        }

        let state = cur.state;
        if !seen.insert(cur.hash) {
            return false; // Cycle detected
        }

        let action = analysis.states[state].actions.get(&EOF_TOKEN);
        match action {
            Some(Action::Accept) => return true,
            Some(Action::Reduce(prod_ix)) => {
                let prod = &productions[*prod_ix];
                let next = match reduce_stack(analysis, &cur, prod) {
                    Some(s) => s,
                    None => return false,
                };
                cur = next;
            }
            _ => return false,
        }
    }
}

// Simulate shifting a terminal (with reductions/gotos as needed)
fn simulate_shift(
    analysis: &GrammarStateAnalysis,
    productions: &[Production],
    stack: &Rc<StackNode>,
    lookahead: usize,
) -> Option<Rc<StackNode>> {
    let mut cur = Rc::clone(stack);
    let mut steps = 0usize;
    let mut seen = HashSet::new();

    loop {
        steps += 1;
        if steps > 256 {
            return None; // Prevent infinite loops
        }

        let state = cur.state;
        if !seen.insert(cur.hash) {
            return None; // Cycle detected
        }

        let action = analysis.states[state].actions.get(&lookahead);
        match action {
            Some(&Action::Shift(next_state)) => {
                return Some(StackNode::push(&cur, next_state));
            }
            Some(&Action::Reduce(prod_ix)) => {
                let prod = &productions[prod_ix];
                cur = reduce_stack(analysis, &cur, prod)?;
            }
            Some(Action::Accept) => return Some(cur),
            None => return None,
        }
    }
}

// Reduce stack by production
fn reduce_stack(
    analysis: &GrammarStateAnalysis,
    stack: &Rc<StackNode>,
    prod: &Production,
) -> Option<Rc<StackNode>> {
    // Pop |rhs| elements
    let popped = StackNode::pop_n(stack, prod.rhs.len())?;

    // Goto
    let top_state = popped.state;
    let goto_state = analysis.states[top_state].goto.get(&prod.lhs)?;

    Some(StackNode::push(&popped, *goto_state))
}

// Lex the next terminal at a position
fn lex_at_text(
    terminals: &[MatcherRef],
    text: &str,
    pos: usize,
    string_opened: bool,
) -> (usize, usize) {
    let bounded_pos = pos.min(text.len());
    let rest = &text[bounded_pos..];
    let at_boundary = bounded_pos >= text.len();
    let mut best_match: Option<(usize, usize)> = None;

    for (idx, matcher) in terminals.iter().enumerate() {
        if !string_opened && matcher.display().contains("json_string") {
            continue;
        }
        // Inside a string, skip "opening-quote" style terminals (those with a
        // whitespace-consuming prefix, like `char_predicate* "`).  They are
        // designed for starting a string value and should not consume the
        // closing quote inside an already-open string. The exact closing-quote
        // terminal (without a whitespace prefix) will be selected instead.
        if string_opened
            && matcher.preview() == Some("\"")
            && matcher.display().contains("char_predicate")
        {
            continue;
        }
        let mut test_pos = 0;
        if let Some(len) = matcher.matches(rest, &mut test_pos) {
            // Allow zero-length matches only at boundary, except json-string
            // body handling which uses quote-start behaviour.
            if len == 0 && !(at_boundary || rest.starts_with('"')) {
                continue;
            }
            // Keep longest match
            if best_match.iter().all(|&(_, best_len)| len > best_len) {
                best_match = Some((idx, len));
            }
        }
    }

    if let Some((idx, len)) = best_match {
        (idx, len)
    } else if at_boundary || rest.trim().is_empty() {
        (EOF_TOKEN, 0)
    } else {
        // Unknown token
        let len = unknown_token_len(terminals, text, pos, string_opened);
        (UNKNOWN_TOKEN, len)
    }
}

fn is_quote_terminal(terminals: &[MatcherRef], term_ix: usize) -> bool {
    terminals
        .get(term_ix)
        .and_then(|m| m.preview())
        .is_some_and(|preview| preview == "\"")
}

// Calculate length of unknown token
fn unknown_token_len(
    terminals: &[MatcherRef],
    text: &str,
    pos: usize,
    string_opened: bool,
) -> usize {
    if pos >= text.len() {
        return 0;
    }
    let rest = &text[pos..];
    for (offset, _) in rest.char_indices().skip(1) {
        let abs = pos + offset;
        if any_terminal_matches_at(terminals, text, abs, string_opened) {
            return offset;
        }
    }
    rest.len()
}

fn any_terminal_matches_at(
    terminals: &[MatcherRef],
    text: &str,
    pos: usize,
    string_opened: bool,
) -> bool {
    if pos >= text.len() {
        return false;
    }
    let rest = &text[pos..];
    for (_idx, matcher) in terminals.iter().enumerate() {
        if !string_opened && matcher.display().contains("json_string") {
            continue;
        }
        let mut test_pos = 0;
        if matcher.matches(rest, &mut test_pos).is_some() {
            if test_pos == 0 && !rest.starts_with('"') {
                continue;
            }
            return true;
        }
    }
    false
}

// Hash function for stack nodes
fn hash_stack(parent: Option<u64>, state: usize) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    parent.hash(&mut hasher);
    state.hash(&mut hasher);
    hasher.finish()
}

fn hash_stack_slice(stack: &[usize]) -> u64 {
    let mut cur: Option<u64> = None;
    for &state in stack {
        let next = hash_stack(cur, state);
        cur = Some(next);
    }
    cur.unwrap_or_else(|| hash_stack(None, 0))
}

#[derive(Debug, Clone, Copy)]
pub struct OpenScopeToken {
    pub term_idx: usize,
    #[allow(dead_code)]
    pub start: usize,
}

#[derive(Debug, Clone)]
pub struct ScopeRecovery {
    pub bridge: BridgeSpec,
    pub stop: ScopeStop,
    pub skip_to: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeStop {
    Close,
    Delimiter(usize),
}

/// Attempt *scope recovery* by bridge parsing (Nilsson-Nyman 2009 §4).
pub fn scope_recover(
    bridge_specs: &[BridgeSpec],
    recovery_delimiters: &[usize],
    terminals: &[MatcherRef],
    text: &str,
    error_pos: usize,
    open_scope_stack: &[OpenScopeToken],
) -> Option<ScopeRecovery> {
    // Maximum bytes to scan ahead before giving up.
    const MAX_SCAN_BYTES: usize = 65536;

    // Walk the open-scope stack from top (innermost) to bottom.
    for &open_tok in open_scope_stack.iter().rev() {
        let Some(bridge) = bridge_specs.iter().find(|b| b.open == open_tok.term_idx) else {
            continue;
        };
        let bridge = bridge.clone();

        // Try to scan forward from error_pos to find the close.
        if let Some((skip_to, stop)) = scan_for_close(
            bridge_specs,
            recovery_delimiters,
            terminals,
            text,
            error_pos,
            &bridge,
            MAX_SCAN_BYTES,
            true,
        ) {
            return Some(ScopeRecovery {
                bridge,
                stop,
                skip_to,
            });
        }
        // This open has no reachable close; try the next outer scope.
    }

    None
}

/// Scan `text` forward from `start_pos` to find `bridge.close`, honouring
/// nesting of the same `(open, close)` pair.  Returns the exclusive end
/// position (i.e., `pos_after_close_token`) or `None` if not found within
/// `max_scan` bytes.
fn scan_for_close(
    bridge_specs: &[BridgeSpec],
    recovery_delimiters: &[usize],
    terminals: &[MatcherRef],
    text: &str,
    start_pos: usize,
    bridge: &BridgeSpec,
    max_scan: usize,
    stop_on_delimiter: bool,
) -> Option<(usize, ScopeStop)> {
    let open_preview = terminals[bridge.open].preview()?;
    let close_preview = terminals[bridge.close].preview()?;

    // Depth 0 means we're looking for the matching close (not nested).
    let mut depth: usize = 0;
    let mut pos = start_pos;
    let limit = (start_pos + max_scan).min(text.len());

    while pos < limit {
        // Skip whitespace quickly.
        let rest = &text[pos..];
        if rest.starts_with(open_preview) {
            depth += 1;
            pos += open_preview.len();
            continue;
        }
        if rest.starts_with(close_preview) {
            if depth == 0 {
                // Stop *before* the close delimiter so the LR parser can shift
                // it normally and use it to close the enclosing grammar rule.
                return Some((pos, ScopeStop::Close));
            }
            depth -= 1;
            pos += close_preview.len();
            continue;
        }

        if depth == 0 && stop_on_delimiter {
            for &delim_idx in recovery_delimiters {
                if delim_idx == bridge.open || delim_idx == bridge.close {
                    continue;
                }
                if let Some(delim) = terminals.get(delim_idx).and_then(|m| m.preview()) {
                    if rest.starts_with(delim) {
                        return Some((pos, ScopeStop::Delimiter(delim_idx)));
                    }
                }
            }
        }

        // Check all other bridge opens/closes to track nesting of *other*
        // pairs inside — we deliberately skip over them so we don't
        // accidentally consume a close meant for an inner scope.
        let mut advanced = false;
        for other in bridge_specs {
            if other.open == bridge.open {
                continue; // already handled above
            }
            if let Some(op) = terminals.get(other.open).and_then(|m| m.preview()) {
                if rest.starts_with(op) {
                    // Enter a nested scope of a *different* kind; skip it
                    // entirely by scanning for its matching close.
                    if let Some(after) = scan_for_close(
                        bridge_specs,
                        recovery_delimiters,
                        terminals,
                        text,
                        pos + op.len(),
                        other,
                        max_scan,
                        false,
                    ) {
                        let nested_close = terminals[other.close].preview().unwrap_or_default();
                        pos = after.0 + nested_close.len();
                        advanced = true;
                        break;
                    }
                }
            }
        }
        if advanced {
            continue;
        }

        // Advance by one byte to avoid getting stuck.
        pos += text[pos..]
            .chars()
            .next()
            .map(|c| c.len_utf8())
            .unwrap_or(1);
    }

    None
}
