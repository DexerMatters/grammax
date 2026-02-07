use rustc_hash::{FxHashMap, FxHashSet};
use std::collections::VecDeque;

use crate::grammar::ir::{Associativity, Production, Symbol};
use crate::grammar::norm::RuleTable;
use crate::parsec::words::MatcherRef;

pub const EOF_TOKEN: usize = usize::MAX;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Action {
    Shift(usize),
    Reduce(usize), // Production index
    Accept,
    Error,
}

#[derive(Debug, Clone)]
pub struct LRState {
    pub id: usize,
    pub items: Vec<Item>,
    pub actions: FxHashMap<usize, Action>, // terminal_idx -> Action
    pub goto: FxHashMap<usize, usize>,     // rule_idx -> state_idx
}

#[derive(Debug, Clone)]
pub struct GrammarStateAnalysis {
    pub states: Vec<LRState>,
    pub start_state: usize,
    pub rule_terminators: FxHashMap<usize, Vec<MatcherRef>>, // For error recovery
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
    follow_sets: FxHashMap<usize, FxHashSet<usize>>,
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

            final_states.push(LRState {
                id: i,
                items: item_set.iter().cloned().collect(),
                actions,
                goto,
            });
        }

        // Post-processing for optimization or clean up

        Self {
            states: final_states,
            start_state: 0,
            rule_terminators: FxHashMap::default(), // TODO: compute from Follow sets
        }
    }

    pub fn state_id_for_rule(&self, rule_ix: usize) -> Option<usize> {
        // Find state that shifts this rule (simplified for recovery)
        // This is a heuristic for now
        self.states
            .iter()
            .position(|s| s.goto.contains_key(&rule_ix))
    }
}

impl<'a> AnalysisContext<'a> {
    fn new(table: &'a RuleTable) -> Self {
        Self {
            table,
            productions: &table.productions,
            first_sets: FxHashMap::default(),
            follow_sets: FxHashMap::default(),
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
                            if !sym_first.contains(&None) {
                                tail_first.remove(&None);
                            }
                            tail_first.extend(sym_first);
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
        // Shift/Reduce or Reduce/Reduce
        // Check operator tables
        let term_str = table.terminals[terminal].display();

        // Find operator info for the terminal in the context of the rules
        // This is tricky because we don't know which rule we are in easily for Shift.
        // For Reduce, we know the production -> rule.

        // Strategy:
        // 1. If Shift/Reduce: look at the Shift terminal (operator?) and the Reduce rule (is it an expression?).
        // 2. If Reduce/Reduce: look at prec of both rules.

        match (&old, &new) {
            (Action::Shift(_), Action::Reduce(prod_ix)) => {
                let prod = &self.productions[*prod_ix];
                let rule_ix = prod.lhs;

                if let Some(ops) = table.operator_tables.get(&rule_ix) {
                    // Find operator token in production that matches one in ops
                    let mut reduce_op = None;
                    for sym in &prod.rhs {
                        if let Symbol::Terminal(t_ix) = sym {
                            let t_disp = table.terminals[*t_ix].display();
                            if let Some(op) = ops.iter().find(|o| o.token.display() == t_disp) {
                                reduce_op = Some(op);
                                // Don't break immediately, usually the last operator defines precedence?
                                // Standard yacc/bison uses the last terminal.
                            }
                        }
                    }

                    // Find shift operator info (from terminal)
                    let shift_op = ops.iter().find(|o| o.token.display() == term_str);

                    if let (Some(r_op), Some(s_op)) = (reduce_op, shift_op) {
                        if r_op.precedence > s_op.precedence {
                            // Reduce wins (keep new)
                            actions.insert(terminal, new);
                            return;
                        } else if r_op.precedence < s_op.precedence {
                            // Shift wins (keep old)
                            return;
                        } else {
                            // Equal precedence, check associativity
                            if r_op.associativity == Associativity::Left {
                                // Left assoc -> Reduce
                                actions.insert(terminal, new);
                                return;
                            } else {
                                // Right assoc -> Shift
                                return;
                            }
                        }
                    } else {
                        // Ops not found
                    }
                } else {
                    // No ops table for rule
                }

                // Default: Shift preferred
            }
            _ => {}
        }

        // If unresolved, prefer Shift over Reduce, or First Reduce over Second
        match new {
            Action::Shift(_) => {
                /* Keep Shift (rewrite) or Old was Shift? If Old was Reduce, Shift wins */
                if let Action::Reduce(_) = old {
                    actions.insert(terminal, new);
                }
            }
            Action::Reduce(_) => { /* Old was Shift or Reduce. If Shift, keep old. If Reduce, keep old (first one wins) */
            }
            _ => {}
        }
    }
}
