use std::{collections::HashSet, sync::Arc};

use dashmap::DashMap;

use crate::grammar::{
    ir::{BridgeGrammar, NormalizedGrammarNode, RefIx, Scope, SeparableScope, State},
    norm::RuleTable,
};
use crate::parsec::words::MatcherRef;

impl State {
    pub fn ref_ix(&self) -> RefIx {
        match self {
            State::Tok(ix, _)
            | State::Seq(ix, _)
            | State::Alt(ix, _, _)
            | State::Field(ix, _, _)
            | State::LeftRec(ix, _, _, _) => *ix,
        }
    }
}

#[derive(Clone, Debug)]
pub struct GrammarStateAnalysis {
    pub states: Vec<State>,
    pub start_state: usize,
    pub rule_roots: Vec<usize>,
    pub is_recursive: Vec<bool>,
    pub bridge: BridgeGrammar,
    pub list_rules: Vec<usize>,
    pub list_tail_rules: Vec<usize>,
    pub rule_terminators: Vec<Vec<String>>,
    pub is_trivial: Vec<bool>,
}

impl GrammarStateAnalysis {
    pub fn from_table(table: &RuleTable, start: usize) -> Self {
        let rule_edges = rule_reference_edges(table);
        let is_recursive = recursive_rules(&rule_edges);

        let mut builder = Builder::new(table, is_recursive);
        let start_state = builder.rule(start);

        let mut list_rules = Vec::new();
        let mut list_tail_rules = Vec::new();
        let mut is_trivial = Vec::new();
        for (ix, name) in table.rule_names.iter().enumerate() {
            is_trivial.push(name.starts_with('@'));
            if *name == "@sep" {
                list_rules.push(ix);
            } else if *name == "@sep_tail" {
                list_tail_rules.push(ix);
            }
        }

        let mut analysis = Self {
            states: builder.states,
            start_state,
            rule_roots: builder.root,
            is_recursive: builder.is_recursive,
            bridge: BridgeGrammar {
                scopes: Vec::new(),
                reefs: Vec::new(),
                separable_scopes: Vec::new(),
            },
            list_rules,
            list_tail_rules,
            rule_terminators: vec![Vec::new(); table.rules.len()],
            is_trivial,
        };
        analysis.bridge = analysis.derive_bridge_grammar();
        analysis.rule_terminators = analysis.compute_terminators();
        analysis
    }

    pub fn is_list_rule(&self, rule_ix: usize) -> bool {
        self.list_rules.contains(&rule_ix)
    }

    pub fn is_list_tail_rule(&self, rule_ix: usize) -> bool {
        self.list_tail_rules.contains(&rule_ix)
    }

    pub fn state_id_for_rule(&self, rule_ix: usize) -> Option<usize> {
        self.rule_roots
            .get(rule_ix)
            .copied()
            .filter(|&id| id != usize::MAX)
    }
    pub fn derive_bridge_grammar(&self) -> BridgeGrammar {
        let mut scopes = Vec::new();
        let mut reef_set = HashSet::new();
        let mut separable_scopes = Vec::new();

        for state in &self.states {
            if let State::Tok(_, matcher) = state {
                if let Some(s) = matcher.preview() {
                    if !s.is_empty() {
                        reef_set.insert(s.to_string());
                    }
                }
            }

            if let State::Seq(_, children) = state {
                let rule_ix = state.ref_ix();
                let is_rule_recursive =
                    rule_ix < self.is_recursive.len() && self.is_recursive[rule_ix];
                let has_recursive_child = children.iter().any(|&child| {
                    let child_rule_ix = self.states[child].ref_ix();
                    child_rule_ix < self.is_recursive.len() && self.is_recursive[child_rule_ix]
                });

                if !is_rule_recursive && !has_recursive_child {
                    continue;
                }

                if children.len() < 2 {
                    continue;
                }

                let first = &self.states[children[0]];
                let last = &self.states[children[children.len() - 1]];

                if let (State::Tok(_, m1), State::Tok(_, m2)) = (first, last) {
                    if let (Some(s1), Some(s2)) = (m1.preview(), m2.preview()) {
                        if !s1.is_empty() && !s2.is_empty() && s1 != s2 {
                            let scope = Scope {
                                open: s1.to_string(),
                                close: s2.to_string(),
                                rule_ix,
                            };
                            if !scopes.contains(&scope) {
                                scopes.push(scope);
                            }
                        }
                    }
                }

                self.detect_separable_scopes(state, children, &mut separable_scopes);
            }
        }

        let reefs = reef_set.into_iter().collect();
        BridgeGrammar {
            scopes,
            reefs,
            separable_scopes,
        }
    }

    fn detect_separable_scopes(
        &self,
        state: &State,
        children: &[usize],
        out: &mut Vec<SeparableScope>,
    ) {
        if children.len() < 3 {
            return;
        }

        let rule_ix = state.ref_ix();

        let first_ix = children[0];
        let middle_start = 1;
        let middle_end = children.len().saturating_sub(1);

        if middle_start >= middle_end {
            return;
        }

        let middle_len = middle_end - middle_start;
        if middle_len == 0 {
            return;
        }

        let mut separator_token = None;
        if middle_len > 1 {
            for i in middle_start..middle_end {
                if let State::Tok(_, m) = &self.states[children[i]] {
                    if let Some(s) = m.preview() {
                        if !s.is_empty()
                            && s != "{"
                            && s != "}"
                            && s != "["
                            && s != "]"
                            && s != "("
                            && s != ")"
                        {
                            separator_token = Some(s.to_string());
                            break;
                        }
                    }
                }
            }
        }

        if let Some(separator) = separator_token {
            let item_rule_ix = self.states[first_ix].ref_ix();
            let scope = SeparableScope {
                rule_ix,
                item_rule_ix,
                separator,
                wrapper_rule_ix: None,
            };
            if !out.contains(&scope) {
                out.push(scope);
            }
        }
    }

    fn compute_terminators(&self) -> Vec<Vec<String>> {
        let mut terminators = vec![Vec::new(); self.rule_roots.len()];

        for scope in &self.bridge.separable_scopes {
            let mut sep_copy = scope.separator.clone();
            sep_copy.push('\0');
            if !terminators[scope.rule_ix].contains(&sep_copy) {
                terminators[scope.rule_ix].push(sep_copy);
            }
        }

        for scope in &self.bridge.scopes {
            let mut close_copy = scope.close.clone();
            close_copy.push('\0');
            if !terminators[scope.rule_ix].contains(&close_copy) {
                terminators[scope.rule_ix].push(close_copy);
            }
        }

        let reefs = &self.bridge.reefs;
        for (_rule_ix, terms) in terminators.iter_mut().enumerate() {
            for reef in reefs {
                let mut reef_copy = reef.clone();
                reef_copy.push('\0');
                if !terms.contains(&reef_copy) {
                    terms.push(reef_copy);
                }
            }

            terms.sort();
            terms.dedup();

            for term in terms.iter_mut() {
                if !term.is_empty() && term.ends_with('\0') {
                    term.pop();
                }
            }
        }

        terminators
    }
}

fn rule_reference_edges(table: &RuleTable) -> Vec<Vec<usize>> {
    use NormalizedGrammarNode::*;
    fn walk(node: &NormalizedGrammarNode, out: &mut Vec<usize>) {
        match node {
            Terminal(_) => {}
            Reference(ix) => out.push(*ix),
            Field(_, inner) => walk(inner, out),
            Sequence(nodes) | Alternative(nodes) => nodes.iter().for_each(|n| walk(n, out)),
        }
    }

    let mut edges = vec![Vec::new(); table.rules.len()];
    for (i, rule) in table.rules.iter().enumerate() {
        walk(rule, &mut edges[i]);
    }
    edges
}

fn recursive_rules(edges: &[Vec<usize>]) -> Vec<bool> {
    mark_sccs(edges, |comp, edges| {
        comp.len() > 1 || (comp.len() == 1 && edges[comp[0]].iter().any(|&x| x == comp[0]))
    })
}

struct Builder<'a> {
    table: &'a RuleTable,
    is_recursive: Vec<bool>,

    states: Vec<State>,
    tok: DashMap<(RefIx, String), usize>,
    seq: DashMap<(RefIx, Vec<usize>), usize>,
    alt: DashMap<(RefIx, Vec<usize>), usize>,
    field: DashMap<(RefIx, &'static str, usize), usize>,

    root: Vec<usize>,
    built: Vec<bool>,
}

impl<'a> Builder<'a> {
    fn new(table: &'a RuleTable, is_recursive: Vec<bool>) -> Self {
        let n = table.rules.len();
        let mut this = Self {
            table,
            is_recursive,
            states: Vec::new(),
            tok: DashMap::new(),
            seq: DashMap::new(),
            alt: DashMap::new(),
            field: DashMap::new(),
            root: vec![usize::MAX; n],
            built: vec![false; n],
        };

        // Allocate placeholders only for recursive rules
        for ix in 0..n {
            if this.is_recursive[ix] {
                this.root[ix] = this.states.len();
                this.states.push(State::Tok(ix, Arc::new("")));
            }
        }

        this
    }

    fn rule(&mut self, ix: usize) -> usize {
        if self.built[ix] {
            return self.root[ix];
        }
        self.built[ix] = true;

        if let Some(info) = &self.table.left_rec[ix] {
            let root_id = if self.root[ix] != usize::MAX {
                self.root[ix]
            } else {
                let id = self.states.len();
                self.root[ix] = id;
                self.states.push(State::Tok(ix, Arc::new("")));
                id
            };

            let base_children: Vec<usize> = info.base.iter().map(|n| self.node(ix, n)).collect();
            let tail_children: Vec<usize> = info.tail.iter().map(|n| self.node(ix, n)).collect();

            self.states[root_id] =
                State::LeftRec(ix, base_children, tail_children, info.tail_fields.clone());
            self.register(root_id);
            return root_id;
        }

        if self.is_recursive[ix] {
            let root_id = self.root[ix];
            self.emit_into(root_id, ix, &self.table.rules[ix]);
            root_id
        } else {
            if let NormalizedGrammarNode::Reference(target_ix) = &self.table.rules[ix] {
                if !self.table.rule_names[ix].starts_with('@') {
                    let child = self.rule(*target_ix);
                    let root_id = self.mk_seq(ix, vec![child]);
                    self.root[ix] = root_id;
                    return root_id;
                }
            }
            let root_id = self.node(ix, &self.table.rules[ix]);
            self.root[ix] = root_id;
            root_id
        }
    }

    fn node(&mut self, cur_ref_ix: RefIx, node: &NormalizedGrammarNode) -> usize {
        use NormalizedGrammarNode::*;
        match node {
            Terminal(m) => self.mk_tok(cur_ref_ix, m),
            Reference(ix) => self.rule(*ix),
            Sequence(nodes) => self.mk_seq_nary(cur_ref_ix, nodes),
            Alternative(nodes) => self.mk_alt_nary(cur_ref_ix, nodes),
            Field(name, inner) => self.mk_field(cur_ref_ix, name, inner),
        }
    }

    fn mk_tok(&mut self, ref_ix: RefIx, matcher: &MatcherRef) -> usize {
        let key = (ref_ix, matcher.display());
        if let Some(id) = self.tok.get(&key) {
            return *id;
        }
        let id = self.states.len();
        self.states.push(State::Tok(ref_ix, Arc::clone(matcher)));
        self.tok.insert(key, id);
        id
    }

    /// Create an n-ary sequence state directly from a list of nodes
    fn mk_seq_nary(&mut self, ref_ix: RefIx, nodes: &[NormalizedGrammarNode]) -> usize {
        let children: Vec<usize> = nodes.iter().map(|n| self.node(ref_ix, n)).collect();
        self.mk_seq(ref_ix, children)
    }

    /// Create an n-ary alternative state directly from a list of nodes
    fn mk_alt_nary(&mut self, ref_ix: RefIx, nodes: &[NormalizedGrammarNode]) -> usize {
        let children: Vec<usize> = nodes.iter().map(|n| self.node(ref_ix, n)).collect();
        self.mk_alt(ref_ix, children)
    }

    fn mk_field(
        &mut self,
        ref_ix: RefIx,
        name: &'static str,
        inner: &NormalizedGrammarNode,
    ) -> usize {
        let child = self.node(ref_ix, inner);
        let key = (ref_ix, name, child);
        if let Some(id) = self.field.get(&key) {
            return *id;
        }
        let id = self.states.len();
        self.states.push(State::Field(ref_ix, name, child));
        self.field.insert(key, id);
        id
    }

    fn mk_seq(&mut self, ref_ix: RefIx, children: Vec<usize>) -> usize {
        let key = (ref_ix, children.clone());
        if let Some(id) = self.seq.get(&key) {
            return *id;
        }
        let id = self.states.len();
        self.states.push(State::Seq(ref_ix, children));
        self.seq.insert(key, id);
        id
    }

    fn mk_alt(&mut self, ref_ix: RefIx, children: Vec<usize>) -> usize {
        let key = (ref_ix, children.clone());
        if let Some(id) = self.alt.get(&key) {
            return *id;
        }
        // Detect if any child is an epsilon (empty sequence or nullable)
        let has_epsilon = children.iter().any(|&child_id| self.is_nullable(child_id));
        let id = self.states.len();
        self.states.push(State::Alt(ref_ix, children, has_epsilon));
        self.alt.insert(key, id);
        id
    }

    fn is_nullable(&self, state_id: usize) -> bool {
        match self.states.get(state_id) {
            Some(State::Seq(_, children)) => {
                children.is_empty() || children.iter().all(|&c| self.is_nullable(c))
            }
            Some(State::Alt(_, children, _)) => children.iter().any(|&c| self.is_nullable(c)),
            Some(State::Tok(_, m)) => m.is_nullable(),
            Some(State::Field(_, _, child)) => self.is_nullable(*child),
            _ => false,
        }
    }

    fn register(&mut self, id: usize) {
        match &self.states[id] {
            State::Tok(ix, m) => {
                self.tok.insert((*ix, m.display()), id);
            }
            State::Seq(ix, children) => {
                self.seq.insert((*ix, children.clone()), id);
            }
            State::Alt(ix, children, _) => {
                self.alt.insert((*ix, children.clone()), id);
            }
            State::Field(ix, name, child) => {
                self.field.insert((*ix, *name, *child), id);
            }
            State::LeftRec(_, _, _, _) => {}
        }
    }

    fn emit_into(&mut self, root_id: usize, ref_ix: RefIx, node: &NormalizedGrammarNode) {
        use NormalizedGrammarNode::*;
        match node {
            Terminal(m) => {
                self.states[root_id] = State::Tok(ref_ix, m.clone());
                self.register(root_id);
            }
            Reference(ix) => {
                let target = self.rule(*ix);
                self.states[root_id] = self.states[target].clone();
                self.register(root_id);
            }
            Sequence(nodes) => {
                let children: Vec<usize> = nodes.iter().map(|n| self.node(ref_ix, n)).collect();
                self.states[root_id] = State::Seq(ref_ix, children);
                self.register(root_id);
            }
            Alternative(nodes) => {
                let children: Vec<usize> = nodes.iter().map(|n| self.node(ref_ix, n)).collect();
                let has_epsilon = children.iter().any(|&child_id| self.is_nullable(child_id));
                self.states[root_id] = State::Alt(ref_ix, children, has_epsilon);
                self.register(root_id);
            }
            Field(name, inner) => {
                let child = self.node(ref_ix, inner);
                self.states[root_id] = State::Field(ref_ix, *name, child);
                self.register(root_id);
            }
        }
    }
}

// Generic Tarjan SCC algorithm. mark is called on each SCC; if it returns true, nodes are marked.
fn mark_sccs<F>(edges: &[Vec<usize>], mut mark: F) -> Vec<bool>
where
    F: FnMut(&[usize], &[Vec<usize>]) -> bool,
{
    struct T {
        idx: usize,
        stack: Vec<usize>,
        on: Vec<bool>,
        index: Vec<Option<usize>>,
        low: Vec<usize>,
        marked: Vec<bool>,
    }

    impl T {
        fn new(n: usize) -> Self {
            Self {
                idx: 0,
                stack: Vec::new(),
                on: vec![false; n],
                index: vec![None; n],
                low: vec![0; n],
                marked: vec![false; n],
            }
        }

        fn dfs<F>(&mut self, v: usize, edges: &[Vec<usize>], mark: &mut F)
        where
            F: FnMut(&[usize], &[Vec<usize>]) -> bool,
        {
            self.index[v] = Some(self.idx);
            self.low[v] = self.idx;
            self.idx += 1;
            self.stack.push(v);
            self.on[v] = true;

            for &w in &edges[v] {
                if self.index[w].is_none() {
                    self.dfs(w, edges, mark);
                    self.low[v] = self.low[v].min(self.low[w]);
                } else if self.on[w] {
                    self.low[v] = self.low[v].min(self.index[w].unwrap());
                }
            }

            if self.low[v] == self.index[v].unwrap() {
                let mut comp = Vec::new();
                loop {
                    let w = self.stack.pop().unwrap();
                    self.on[w] = false;
                    comp.push(w);
                    if w == v {
                        break;
                    }
                }
                if mark(&comp, edges) {
                    comp.iter().for_each(|&x| self.marked[x] = true);
                }
            }
        }
    }

    let mut t = T::new(edges.len());
    for v in 0..edges.len() {
        if t.index[v].is_none() {
            t.dfs(v, edges, &mut mark);
        }
    }
    t.marked
}
