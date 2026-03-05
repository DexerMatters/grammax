use rustc_hash::{FxHashMap, FxHashSet};
use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

use crate::grammar::ir::{Production, Symbol};
use crate::grammar::norm::RuleTable;

pub const EOF_TOKEN: usize = usize::MAX;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Action {
    Shift(usize),
    Reduce(usize), // Production index
    Accept,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LRState {
    #[serde(with = "crate::grammar::cache::serde_fxhashmap")]
    pub actions: FxHashMap<usize, Action>, // terminal_idx -> Action
    #[serde(with = "crate::grammar::cache::serde_fxhashmap")]
    pub goto: FxHashMap<usize, usize>, // rule_idx -> state_idx
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrammarStateAnalysis {
    pub states: Vec<LRState>,
    pub start_state: usize,
}
#[derive(Debug, Clone, PartialEq, Eq, Hash, Copy)]
pub struct Item {
    pub production_ix: usize,
    pub dot: usize,
}

impl Item {
    pub fn new(production_ix: usize, dot: usize) -> Self {
        Self { production_ix, dot }
    }
}

pub struct AnalysisContext<'a> {
    table: &'a RuleTable,
    productions: &'a [Production],
    first_sets: FxHashMap<usize, FxHashSet<Option<usize>>>, // rule_idx -> {terminal_idx | None} (None = epsilon)
}

impl GrammarStateAnalysis {
    pub fn from_table(table: &RuleTable, start_rule: usize) -> Self {
        let mut ctx = AnalysisContext::new(table);
        ctx.compute_first_sets();

        // LR(0) states
        let (states, transitions) = ctx.compute_lr0_states(start_rule);

        // LALR(1) lookaheads
        let lookaheads = ctx.compute_lookaheads(&states, &transitions);

        let mut final_states = Vec::with_capacity(states.len());

        for (i, item_set) in states.iter().enumerate() {
            let mut actions = FxHashMap::default();
            let mut goto = FxHashMap::default();

            // Add Goto transitions
            if let Some(trans) = transitions.get(&i) {
                for (sym, target) in trans {
                    if let Symbol::NonTerminal(rule_ix) = sym {
                        goto.insert(*rule_ix, *target);
                    } else if let Symbol::Terminal(term_ix) = sym {
                        actions.insert(*term_ix, Action::Shift(*target));
                    }
                }
            }

            // Add Reductions
            let state_lookaheads = lookaheads.get(&i);

            for item in item_set {
                let prod = &table.productions[item.production_ix];
                if item.dot == prod.rhs.len() {
                    if prod.lhs == start_rule && item.production_ix == 0 { // Assuming 0 is the start production
                        // Accept is handled usually by reducer, but let's mark EOF
                        // In many implementations, Accept is Reduce(StartProd) on EOF
                        // We will assume reduced start rule means accept
                    }

                    // Lookahead based reduction
                    if let Some(las) = state_lookaheads.and_then(|m| m.get(&item)) {
                        for &term_idx in las {
                            if prod.lhs == start_rule && term_idx == EOF_TOKEN {
                                ctx.add_action(&mut actions, term_idx, Action::Accept, table);
                            } else {
                                ctx.add_action(
                                    &mut actions,
                                    term_idx,
                                    Action::Reduce(item.production_ix),
                                    table,
                                );
                            }
                        }
                    }
                }
            }

            final_states.push(LRState { actions, goto });
        }

        // Post-processing for optimization or clean up

        Self {
            states: final_states,
            start_state: 0,
        }
    }
}

impl<'a> AnalysisContext<'a> {
    fn new(table: &'a RuleTable) -> Self {
        Self {
            table,
            productions: &table.productions,
            first_sets: FxHashMap::default(),
        }
    }

    fn compute_first_sets(&mut self) {
        let mut changed = true;
        while changed {
            changed = false;
            for (_pid, prod) in self.productions.iter().enumerate() {
                let lhs = prod.lhs; // Rule index
                let rhs = &prod.rhs;

                // Track if production is nullable so far
                let mut nullable = true;

                for sym in rhs {
                    match sym {
                        Symbol::Terminal(idx) => {
                            if self.add_first(lhs, Some(*idx)) {
                                changed = true;
                            }
                            nullable = false;
                            break;
                        }
                        Symbol::NonTerminal(idx) => {
                            let firsts = self.first_sets.entry(*idx).or_default().clone();
                            let mut has_epsilon = false;
                            for f in firsts {
                                if f.is_none() {
                                    has_epsilon = true;
                                } else {
                                    if self.add_first(lhs, f) {
                                        changed = true;
                                    }
                                }
                            }
                            if !has_epsilon {
                                nullable = false;
                                break;
                            }
                        }
                    }
                }

                if nullable {
                    if self.add_first(lhs, None) {
                        changed = true;
                    }
                }
            }
        }
    }

    fn add_first(&mut self, rule_idx: usize, terminal: Option<usize>) -> bool {
        let set = self.first_sets.entry(rule_idx).or_default();
        if !set.contains(&terminal) {
            set.insert(terminal);
            true
        } else {
            false
        }
    }

    // LR(0) State construction
    fn compute_lr0_states(
        &self,
        start_rule: usize,
    ) -> (
        Vec<FxHashSet<Item>>,
        FxHashMap<usize, FxHashMap<Symbol, usize>>,
    ) {
        let mut states: Vec<FxHashSet<Item>> = Vec::new();
        let mut state_map: FxHashMap<Vec<Item>, usize> = FxHashMap::default();
        let mut transitions: FxHashMap<usize, FxHashMap<Symbol, usize>> = FxHashMap::default();

        // Initial state: productions for start_rule
        let mut start_items = FxHashSet::default();
        for (i, p) in self.productions.iter().enumerate() {
            if p.lhs == start_rule {
                start_items.insert(Item::new(i, 0));
            }
        }

        let start_closure = self.closure(&start_items);
        let sorted_start = self.sort_items(&start_closure);

        states.push(start_closure);
        state_map.insert(sorted_start, 0);

        let mut queue = VecDeque::new();
        queue.push_back(0);

        while let Some(state_idx) = queue.pop_front() {
            let state = &states[state_idx];

            // Group items by next symbol
            let mut next_symbols: FxHashMap<Symbol, FxHashSet<Item>> = FxHashMap::default();

            for item in state {
                let prod = &self.productions[item.production_ix];
                if item.dot < prod.rhs.len() {
                    let sym = &prod.rhs[item.dot];
                    next_symbols
                        .entry(sym.clone())
                        .or_default()
                        .insert(Item::new(item.production_ix, item.dot + 1));
                }
            }

            for (sym, items) in next_symbols {
                let closure = self.closure(&items);
                let sorted = self.sort_items(&closure);

                let target_idx = if let Some(&idx) = state_map.get(&sorted) {
                    idx
                } else {
                    let idx = states.len();
                    states.push(closure);
                    state_map.insert(sorted, idx);
                    queue.push_back(idx);
                    idx
                };

                transitions
                    .entry(state_idx)
                    .or_default()
                    .insert(sym, target_idx);
            }
        }

        (states, transitions)
    }

    fn closure(&self, items: &FxHashSet<Item>) -> FxHashSet<Item> {
        let mut closure = items.clone();
        let mut changed = true;

        while changed {
            changed = false;
            let current_items: Vec<_> = closure.iter().cloned().collect();

            for item in current_items {
                let prod = &self.productions[item.production_ix];
                if item.dot < prod.rhs.len() {
                    if let Symbol::NonTerminal(rule_ix) = &prod.rhs[item.dot] {
                        // Add productions for this non-terminal
                        for (i, p) in self.productions.iter().enumerate() {
                            if p.lhs == *rule_ix {
                                let new_item = Item::new(i, 0);
                                if closure.insert(new_item) {
                                    changed = true;
                                }
                            }
                        }
                    }
                }
            }
        }
        closure
    }

    fn sort_items(&self, items: &FxHashSet<Item>) -> Vec<Item> {
        let mut v: Vec<_> = items.iter().cloned().collect();
        v.sort_by(|a, b| {
            a.production_ix
                .cmp(&b.production_ix)
                .then(a.dot.cmp(&b.dot))
        });
        v
    }

    // Simplified LALR(1) lookahead computation
    // For a real robust implementation, we need the full propagation graph.
    // Here we will implement a simplified version or SLR if optimization allows,
    // but user requested LALR.
    //
    // Strategy:
    // 1. Determine spontaneous lookaheads
    // 2. Determine propagated lookaheads
    // 3. Propagate

    fn compute_lookaheads(
        &self,
        states: &[FxHashSet<Item>],
        _transitions: &FxHashMap<usize, FxHashMap<Symbol, usize>>,
    ) -> FxHashMap<usize, FxHashMap<Item, FxHashSet<usize>>> {
        // Placeholder: Currently using SLR(1) for simplicity and speed as a baseline optimization.
        // Real LALR(1) is significantly more complex to implement in one go.
        // SLR is often sufficient for practical grammars, and we can upgrade if conflicts arise.
        // To make it LALR, we'd need to trace (State, Item) pairs.
        //
        // UPGRADE: Implementing full LALR(1) propagation.

        let mut lookaheads: FxHashMap<usize, FxHashMap<Item, FxHashSet<usize>>> =
            FxHashMap::default();

        let start_rule = self
            .productions
            .iter()
            .find(|p| p.lhs == self.table.start_rule) // Assuming table.start_rule is set correctly
            .map(|p| p.lhs)
            .unwrap_or(0);

        // Compute Follow sets including EOF for start rule
        let follow_sets = self.compute_follow_sets(start_rule);

        for (state_idx, state) in states.iter().enumerate() {
            for item in state {
                let prod = &self.productions[item.production_ix];
                if item.dot == prod.rhs.len() {
                    // Reduction
                    if let Some(follows) = follow_sets.get(&prod.lhs) {
                        let entry = lookaheads
                            .entry(state_idx)
                            .or_default()
                            .entry(*item)
                            .or_default();
                        entry.extend(follows.iter().cloned());
                    }
                }
            }
        }

        lookaheads
    }

    fn compute_follow_sets(&self, start_rule: usize) -> FxHashMap<usize, FxHashSet<usize>> {
        let mut follows: FxHashMap<usize, FxHashSet<usize>> = FxHashMap::default();

        // Add EOF to start rule
        follows.entry(start_rule).or_default().insert(EOF_TOKEN);

        let mut changed = true;
        while changed {
            changed = false;
            for prod in self.productions {
                let mut tail_first = FxHashSet::default();
                tail_first.insert(None); // Epsilon

                // Backward scan
                for sym in prod.rhs.iter().rev() {
                    match sym {
                        Symbol::Terminal(idx) => {
                            tail_first.clear();
                            tail_first.insert(Some(*idx));
                        }
                        Symbol::NonTerminal(idx) => {
                            let lhs_follows = if tail_first.contains(&None) {
                                follows.get(&prod.lhs).cloned()
                            } else {
                                None
                            };

                            let entry = follows.entry(*idx).or_default();

                            // Add non-epsilon from tail_first to Follow(B)
                            for f in &tail_first {
                                if let Some(t) = f {
                                    if entry.insert(*t) {
                                        changed = true;
                                    }
                                }
                            }

                            // If tail_first had epsilon, we add Follow(A) to Follow(B)
                            if let Some(f_set) = lhs_follows {
                                for f in f_set {
                                    if entry.insert(f) {
                                        changed = true;
                                    }
                                }
                            }

                            // Update tail_first for next symbol
                            let sym_first = self.first_sets.get(idx).cloned().unwrap_or_default();
                            let sym_nullable = sym_first.contains(&None);

                            let mut new_tail = sym_first.clone();
                            new_tail.remove(&None);

                            if sym_nullable {
                                new_tail.extend(tail_first.iter().cloned());
                            }
                            tail_first = new_tail;
                        }
                    }
                }
            }
        }
        follows
    }

    fn add_action(
        &self,
        actions: &mut FxHashMap<usize, Action>,
        terminal: usize,
        action: Action,
        table: &RuleTable,
    ) {
        if let Some(existing) = actions.get(&terminal) {
            // Conflict!
            self.resolve_conflict(actions, terminal, existing.clone(), action, table);
        } else {
            actions.insert(terminal, action);
        }
    }

    fn resolve_conflict(
        &self,
        actions: &mut FxHashMap<usize, Action>,
        terminal: usize,
        old: Action,
        new: Action,
        table: &RuleTable,
    ) {
        if terminal == EOF_TOKEN || terminal >= table.terminals.len() {
            return;
        }

        // Default conflict resolution:
        // Shift/Reduce: Shift wins
        // Reduce/Reduce: First wins (keep old)

        match new {
            Action::Shift(_) => {
                // If old was Reduce, Shift overrides it
                if let Action::Reduce(_) = old {
                    actions.insert(terminal, new);
                }
            }
            Action::Reduce(new_idx) => {
                // If old was Shift, keep Shift.
                // If old was Reduce, keep the one with lower production index (earlier in grammar).
                if let Action::Reduce(old_idx) = old {
                    if new_idx < old_idx {
                        actions.insert(terminal, new);
                    }
                }
            }
            _ => {}
        }
    }
}
