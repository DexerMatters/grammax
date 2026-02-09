use crate::grammar::analysis::{Action, EOF_TOKEN, GrammarStateAnalysis};
use crate::grammar::ir::Production;
use crate::parsec::words::MatcherRef;
use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::rc::Rc;

const UNKNOWN_TOKEN: usize = usize::MAX - 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RepairOp {
    Insert(usize),
    Delete,
    Shift,
}

#[derive(Debug, Clone)]
pub struct RecoveryConfig {
    pub max_cost: usize,
    pub max_shifts: usize,
    pub max_try: usize,
    pub max_configs: usize,
}

impl Default for RecoveryConfig {
    fn default() -> Self {
        Self {
            max_cost: 6,
            max_shifts: 3,
            max_try: 250,
            max_configs: 20_000,
        }
    }
}

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

#[derive(Clone)]
struct RepairNode {
    op: RepairOp,
    parent: Option<Rc<RepairNode>>,
}

#[derive(Clone)]
struct Config {
    stack: Rc<StackNode>,
    pos: usize,
    cost: usize,
    shifts: usize,
    ops: Option<Rc<RepairNode>>,
    last_op: Option<RepairOp>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct ConfigKey {
    stack_hash: u64,
    stack_depth: usize,
    stack_state: usize,
    pos: usize,
    shifts: usize,
}

pub fn recover(
    analysis: &GrammarStateAnalysis,
    productions: &[Production],
    terminals: &[MatcherRef],
    text: &str,
    pos: usize,
    stack: &[usize],
    config: &RecoveryConfig,
) -> Option<Vec<RepairOp>> {
    if stack.is_empty() {
        return None;
    }

    let mut root = StackNode::new_root(stack[0]);
    for &state in &stack[1..] {
        root = StackNode::push(&root, state);
    }

    let mut buckets: Vec<VecDeque<Config>> = vec![VecDeque::new(); config.max_cost + 1];
    let start = Config {
        stack: root,
        pos,
        cost: 0,
        shifts: 0,
        ops: None,
        last_op: None,
    };
    buckets[0].push_back(start);

    let mut visited: HashMap<ConfigKey, usize> = HashMap::new();
    let mut success: Vec<Config> = Vec::new();
    let mut found_cost: Option<usize> = None;
    let mut processed = 0usize;

    for cost in 0..=config.max_cost {
        while let Some(cfg) = buckets[cost].pop_front() {
            processed += 1;
            if processed > config.max_configs {
                return None;
            }

            if found_cost.is_some() {
                if Some(cost) != found_cost {
                    continue;
                }
            }

            let key = ConfigKey {
                stack_hash: cfg.stack.hash,
                stack_depth: cfg.stack.depth,
                stack_state: cfg.stack.state,
                pos: cfg.pos,
                shifts: cfg.shifts,
            };
            if let Some(prev_cost) = visited.get(&key) {
                if *prev_cost <= cfg.cost {
                    continue;
                }
            }
            visited.insert(key, cfg.cost);

            if cfg.shifts >= config.max_shifts || can_accept(analysis, productions, &cfg.stack) {
                if found_cost.is_none() {
                    found_cost = Some(cost);
                }
                success.push(cfg);
                continue;
            }

            // Shift (consume next token)
            if let Some(next) = shift_config(analysis, productions, terminals, text, &cfg) {
                if next.cost <= config.max_cost {
                    buckets[next.cost].push_back(next);
                }
            }

            // Insert (try expected terminals only)
            if let Some(expected) = analysis.states.get(cfg.stack.state) {
                for term_ix in expected.actions.keys().copied() {
                    if term_ix == EOF_TOKEN || term_ix == UNKNOWN_TOKEN {
                        continue;
                    }
                    if let Some(next) = insert_config(analysis, productions, &cfg, term_ix) {
                        if next.cost <= config.max_cost {
                            buckets[next.cost].push_back(next);
                        }
                    }
                }
            }

            // Delete (skip next token)
            if let Some(next) = delete_config(terminals, text, &cfg) {
                if next.cost <= config.max_cost {
                    buckets[next.cost].push_back(next);
                }
            }
        }
    }

    let best = pick_best(
        analysis,
        productions,
        terminals,
        text,
        success,
        config.max_try,
    )?;
    Some(collect_ops(best.ops))
}

fn collect_ops(node: Option<Rc<RepairNode>>) -> Vec<RepairOp> {
    let mut ops = Vec::new();
    let mut cur = node;
    while let Some(n) = cur {
        ops.push(n.op);
        cur = n.parent.clone();
    }
    ops.reverse();
    ops
}

fn pick_best(
    analysis: &GrammarStateAnalysis,
    productions: &[Production],
    terminals: &[MatcherRef],
    text: &str,
    candidates: Vec<Config>,
    max_try: usize,
) -> Option<Config> {
    let mut best: Option<(usize, Config)> = None;

    for cfg in candidates {
        let score = simulate_progress(
            analysis,
            productions,
            terminals,
            text,
            &cfg.stack,
            cfg.pos,
            max_try,
        );
        match &best {
            None => best = Some((score, cfg)),
            Some((best_score, _)) if score > *best_score => best = Some((score, cfg)),
            _ => {}
        }
    }

    best.map(|(_, cfg)| cfg)
}

fn simulate_progress(
    analysis: &GrammarStateAnalysis,
    productions: &[Production],
    terminals: &[MatcherRef],
    text: &str,
    stack: &Rc<StackNode>,
    mut pos: usize,
    max_try: usize,
) -> usize {
    let mut cur = Rc::clone(stack);
    let mut consumed = 0usize;

    while consumed < max_try {
        let (term, len) = lex_at(terminals, text, pos);
        let next = match simulate_shift(analysis, productions, &cur, term) {
            Some(s) => s,
            None => break,
        };
        if term == EOF_TOKEN {
            break;
        }
        cur = next;
        pos += len;
        consumed += 1;
    }

    consumed
}

fn shift_config(
    analysis: &GrammarStateAnalysis,
    productions: &[Production],
    terminals: &[MatcherRef],
    text: &str,
    cfg: &Config,
) -> Option<Config> {
    let (term, len) = lex_at(terminals, text, cfg.pos);
    let new_stack = simulate_shift(analysis, productions, &cfg.stack, term)?;
    let ops = Some(Rc::new(RepairNode {
        op: RepairOp::Shift,
        parent: cfg.ops.clone(),
    }));
    Some(Config {
        stack: new_stack,
        pos: cfg.pos + len,
        cost: cfg.cost,
        shifts: cfg.shifts + 1,
        ops,
        last_op: Some(RepairOp::Shift),
    })
}

fn insert_config(
    analysis: &GrammarStateAnalysis,
    productions: &[Production],
    cfg: &Config,
    term_ix: usize,
) -> Option<Config> {
    let new_stack = simulate_shift(analysis, productions, &cfg.stack, term_ix)?;
    let ops = Some(Rc::new(RepairNode {
        op: RepairOp::Insert(term_ix),
        parent: cfg.ops.clone(),
    }));
    Some(Config {
        stack: new_stack,
        pos: cfg.pos,
        cost: cfg.cost + 1,
        shifts: cfg.shifts,
        ops,
        last_op: Some(RepairOp::Insert(term_ix)),
    })
}

fn delete_config(terminals: &[MatcherRef], text: &str, cfg: &Config) -> Option<Config> {
    // We allow Insert -> Delete to support token replacement (Insert correct one, Delete bad one)

    let (term, len) = lex_at(terminals, text, cfg.pos);
    if term == EOF_TOKEN || len == 0 {
        return None;
    }
    let ops = Some(Rc::new(RepairNode {
        op: RepairOp::Delete,
        parent: cfg.ops.clone(),
    }));
    Some(Config {
        stack: Rc::clone(&cfg.stack),
        pos: cfg.pos + len,
        cost: cfg.cost + 1,
        shifts: cfg.shifts,
        ops,
        last_op: Some(RepairOp::Delete),
    })
}

fn can_accept(
    analysis: &GrammarStateAnalysis,
    productions: &[Production],
    stack: &Rc<StackNode>,
) -> bool {
    let mut cur = Rc::clone(stack);
    let mut seen = HashSet::new();
    loop {
        let state = cur.state;
        if !seen.insert(state) {
            return false;
        }
        let action = analysis.states[state].actions.get(&EOF_TOKEN).cloned();
        match action {
            Some(Action::Accept) => return true,
            Some(Action::Reduce(prod_ix)) => {
                let prod = &productions[prod_ix];
                let next = match reduce_stack(analysis, productions, &cur, prod) {
                    Some(s) => s,
                    None => return false,
                };
                cur = next;
            }
            _ => return false,
        }
    }
}

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
            return None;
        }
        let state = cur.state;
        if !seen.insert(state) {
            return None;
        }
        let action = analysis.states[state].actions.get(&lookahead).cloned();
        match action {
            Some(Action::Shift(next_state)) => return Some(StackNode::push(&cur, next_state)),
            Some(Action::Reduce(prod_ix)) => {
                let prod = &productions[prod_ix];
                cur = reduce_stack(analysis, productions, &cur, prod)?;
            }
            Some(Action::Accept) => return Some(cur),
            None => return None,
        }
    }
}

fn reduce_stack(
    analysis: &GrammarStateAnalysis,
    productions: &[Production],
    stack: &Rc<StackNode>,
    prod: &Production,
) -> Option<Rc<StackNode>> {
    let popped = StackNode::pop_n(stack, prod.rhs.len())?;
    let top_state = popped.state;
    let goto_state = analysis.states[top_state].goto.get(&prod.lhs)?;
    Some(StackNode::push(&popped, *goto_state))
}

fn lex_at(terminals: &[MatcherRef], text: &str, pos: usize) -> (usize, usize) {
    if pos >= text.len() {
        return (EOF_TOKEN, 0);
    }
    let rest = &text[pos..];
    let mut best_match: Option<(usize, usize)> = None;
    for (idx, matcher) in terminals.iter().enumerate() {
        let mut test_pos = 0;
        if let Some(len) = matcher.matches(rest, &mut test_pos) {
            if best_match.iter().all(|&(_, best_len)| len > best_len) {
                best_match = Some((idx, len));
            }
        }
    }
    if let Some((idx, len)) = best_match {
        (idx, len)
    } else {
        let len = rest.chars().next().map(|c| c.len_utf8()).unwrap_or(0);
        (UNKNOWN_TOKEN, len)
    }
}

fn hash_stack(parent: Option<u64>, state: usize) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    parent.hash(&mut hasher);
    state.hash(&mut hasher);
    hasher.finish()
}
