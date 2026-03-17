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

#[derive(Debug, Clone, PartialEq, Eq, Hash, Copy)]
struct LR1Item {
    production_ix: usize,
    dot: usize,
    lookahead: usize,
}

impl LR1Item {
    fn new(production_ix: usize, dot: usize, lookahead: usize) -> Self {
        Self {
            production_ix,
            dot,
            lookahead,
        }
    }

    fn core(self) -> Item {
        Item::new(self.production_ix, self.dot)
    }
}

pub struct AnalysisContext<'a> {
    productions: &'a [Production],
    first_sets: FxHashMap<usize, FxHashSet<Option<usize>>>, // rule_idx -> {terminal_idx | None} (None = epsilon)
}

impl GrammarStateAnalysis {
    pub fn from_table(table: &RuleTable, start_rule: usize) -> Self {
        let mut ctx = AnalysisContext::new(table);
        ctx.compute_first_sets();

        let (states, transitions, lookaheads) = ctx.compute_lalr_states(start_rule);

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

    fn compute_lalr_states(
        &self,
        start_rule: usize,
    ) -> (
        Vec<FxHashSet<Item>>,
        FxHashMap<usize, FxHashMap<Symbol, usize>>,
        FxHashMap<usize, FxHashMap<Item, FxHashSet<usize>>>,
    ) {
        let mut canonical_states: Vec<FxHashSet<LR1Item>> = Vec::new();
        let mut canonical_state_map: FxHashMap<Vec<LR1Item>, usize> = FxHashMap::default();
        let mut canonical_transitions: FxHashMap<usize, FxHashMap<Symbol, usize>> =
            FxHashMap::default();

        let mut start_items = FxHashSet::default();
        for (i, p) in self.productions.iter().enumerate() {
            if p.lhs == start_rule {
                start_items.insert(LR1Item::new(i, 0, EOF_TOKEN));
            }
        }

        let start_closure = self.closure_lr1(&start_items);
        let sorted_start = self.sort_lr1_items(&start_closure);

        canonical_states.push(start_closure);
        canonical_state_map.insert(sorted_start, 0);

        let mut queue = VecDeque::new();
        queue.push_back(0);

        while let Some(state_idx) = queue.pop_front() {
            let state = &canonical_states[state_idx];

            let mut next_symbols: FxHashMap<Symbol, FxHashSet<LR1Item>> = FxHashMap::default();

            for item in state {
                let prod = &self.productions[item.production_ix];
                if item.dot < prod.rhs.len() {
                    let sym = &prod.rhs[item.dot];
                    next_symbols
                        .entry(sym.clone())
                        .or_default()
                        .insert(LR1Item::new(
                            item.production_ix,
                            item.dot + 1,
                            item.lookahead,
                        ));
                }
            }

            for (sym, items) in next_symbols {
                let closure = self.closure_lr1(&items);
                let sorted = self.sort_lr1_items(&closure);

                let target_idx = if let Some(&idx) = canonical_state_map.get(&sorted) {
                    idx
                } else {
                    let idx = canonical_states.len();
                    canonical_states.push(closure);
                    canonical_state_map.insert(sorted, idx);
                    queue.push_back(idx);
                    idx
                };

                canonical_transitions
                    .entry(state_idx)
                    .or_default()
                    .insert(sym, target_idx);
            }
        }

        let mut merged_states = Vec::new();
        let mut merged_lookaheads: FxHashMap<usize, FxHashMap<Item, FxHashSet<usize>>> =
            FxHashMap::default();
        let mut merged_transitions: FxHashMap<usize, FxHashMap<Symbol, usize>> =
            FxHashMap::default();
        let mut core_to_merged: FxHashMap<Vec<Item>, usize> = FxHashMap::default();
        let mut canonical_to_merged = vec![0; canonical_states.len()];

        for (canonical_idx, state) in canonical_states.iter().enumerate() {
            let mut core_set = FxHashSet::default();
            for item in state {
                core_set.insert(item.core());
            }

            let core_key = self.sort_items(&core_set);
            let merged_idx = if let Some(&idx) = core_to_merged.get(&core_key) {
                idx
            } else {
                let idx = merged_states.len();
                merged_states.push(core_set);
                core_to_merged.insert(core_key, idx);
                idx
            };

            canonical_to_merged[canonical_idx] = merged_idx;

            let entry = merged_lookaheads.entry(merged_idx).or_default();
            for item in state {
                entry.entry(item.core()).or_default().insert(item.lookahead);
            }
        }

        for (source_idx, transitions) in canonical_transitions {
            let merged_source = canonical_to_merged[source_idx];
            for (symbol, target_idx) in transitions {
                let merged_target = canonical_to_merged[target_idx];
                let entry = merged_transitions.entry(merged_source).or_default();
                if let Some(existing) = entry.insert(symbol.clone(), merged_target) {
                    debug_assert_eq!(
                        existing, merged_target,
                        "LALR merge produced inconsistent goto target for a merged state"
                    );
                }
            }
        }

        (merged_states, merged_transitions, merged_lookaheads)
    }

    fn closure_lr1(&self, items: &FxHashSet<LR1Item>) -> FxHashSet<LR1Item> {
        let mut closure = items.clone();
        let mut changed = true;

        while changed {
            changed = false;
            let current_items: Vec<_> = closure.iter().cloned().collect();

            for item in current_items {
                let prod = &self.productions[item.production_ix];
                if item.dot < prod.rhs.len() {
                    if let Symbol::NonTerminal(rule_ix) = &prod.rhs[item.dot] {
                        let lookaheads =
                            self.first_of_suffix(&prod.rhs[item.dot + 1..], item.lookahead);
                        for (i, p) in self.productions.iter().enumerate() {
                            if p.lhs == *rule_ix {
                                for lookahead in &lookaheads {
                                    let new_item = LR1Item::new(i, 0, *lookahead);
                                    if closure.insert(new_item) {
                                        changed = true;
                                    }
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

    fn sort_lr1_items(&self, items: &FxHashSet<LR1Item>) -> Vec<LR1Item> {
        let mut v: Vec<_> = items.iter().cloned().collect();
        v.sort_by(|a, b| {
            a.production_ix
                .cmp(&b.production_ix)
                .then(a.dot.cmp(&b.dot))
                .then(a.lookahead.cmp(&b.lookahead))
        });
        v
    }

    fn first_of_suffix(&self, suffix: &[Symbol], fallback_lookahead: usize) -> FxHashSet<usize> {
        let mut result = FxHashSet::default();
        let mut nullable_prefix = true;

        for symbol in suffix {
            match symbol {
                Symbol::Terminal(idx) => {
                    result.insert(*idx);
                    nullable_prefix = false;
                    break;
                }
                Symbol::NonTerminal(rule_idx) => {
                    let firsts = self.first_sets.get(rule_idx).cloned().unwrap_or_default();
                    let mut is_nullable = false;
                    for first in firsts {
                        match first {
                            Some(term_idx) => {
                                result.insert(term_idx);
                            }
                            None => {
                                is_nullable = true;
                            }
                        }
                    }

                    if !is_nullable {
                        nullable_prefix = false;
                        break;
                    }
                }
            }
        }

        if nullable_prefix {
            result.insert(fallback_lookahead);
        }

        result
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
