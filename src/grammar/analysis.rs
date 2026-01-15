use std::{cell::OnceCell, fs, io, path::Path, rc::Rc};

use dashmap::{DashMap, DashSet};
use ndarray::Array2;

use crate::{
    grammar::norm::{NormalizedGrammarNode, RuleTable},
    parsec::words::Matcher,
};

type RefIx = usize;

#[derive(Clone, Debug)]
pub enum State {
    Tok(RefIx, Rc<dyn Matcher>),
    Seq(RefIx, usize, usize), // sequence: (left_state, right_state)
    Alt(RefIx, usize, usize), // alternative: (left_state, right_state)
}

impl State {
    pub fn ref_ix(&self) -> RefIx {
        match self {
            State::Tok(ix, _) | State::Seq(ix, _, _) | State::Alt(ix, _, _) => *ix,
        }
    }
}

#[derive(Clone, Debug)]
pub struct GrammarGraphAnalysis {
    pub states: Vec<State>,
    pub distance_matrix: OnceCell<Array2<f32>>,
    pub closure_matrix: OnceCell<Array2<f32>>,

    pub left_distance_matrix: OnceCell<Array2<f32>>,
    pub left_closure_matrix: OnceCell<Array2<f32>>,

    pub terminal_states: OnceCell<Vec<usize>>,
    pub recursive_states: OnceCell<Vec<usize>>,
    pub infinite_states: OnceCell<Vec<usize>>,
}

impl GrammarGraphAnalysis {
    pub fn from_table(table: &RuleTable, start: usize) -> Self {
        let rule_edges = rule_reference_edges(table);
        let is_recursive = recursive_rules(&rule_edges);

        let mut builder = Builder::new(table, is_recursive);
        builder.rule(start);
        Self {
            states: builder.states,

            distance_matrix: OnceCell::new(),
            closure_matrix: OnceCell::new(),

            left_distance_matrix: OnceCell::new(),
            left_closure_matrix: OnceCell::new(),

            terminal_states: OnceCell::new(),
            recursive_states: OnceCell::new(),
            infinite_states: OnceCell::new(),
        }
    }

    pub fn export(&self, path: impl AsRef<Path>) -> Result<(), io::Error> {
        let mut file = fs::File::create(path)?;

        Ok(())
    }

    pub fn rule_set(&self) -> DashSet<usize> {
        self.states.iter().map(|s| s.ref_ix()).collect()
    }

    pub fn terminal_states(&self) -> &Vec<usize> {
        self.terminal_states.get_or_init(|| {
            self.states
                .iter()
                .enumerate()
                .filter_map(|(i, state)| match state {
                    State::Tok(_, _) => Some(i),
                    _ => None,
                })
                .collect()
        })
    }

    pub fn recursive_states(&self) -> &Vec<usize> {
        self.recursive_states.get_or_init(|| {
            let transitive_closure = self.transitive_closure();
            transitive_closure
                .diag()
                .indexed_iter()
                .filter_map(|(i, &value)| if !value.is_nan() { Some(i) } else { None })
                .collect()
        })
    }

    pub fn left_recursive_states(&self) -> Vec<usize> {
        let left_closure = self.left_transitive_closure();
        left_closure
            .diag()
            .indexed_iter()
            .filter_map(|(i, &value)| if !value.is_nan() { Some(i) } else { None })
            .collect()
    }

    pub fn infinite_states(&self) -> &Vec<usize> {
        self.infinite_states.get_or_init(|| {
            let transitive_closure = self.transitive_closure();
            transitive_closure
                .indexed_iter()
                .filter_map(|((i, j), &value)| {
                    if i == j && value.is_infinite() {
                        Some(i)
                    } else {
                        None
                    }
                })
                .collect()
        })
    }

    pub fn distance_matrix(&self) -> &Array2<f32> {
        self.distance_matrix
            .get_or_init(|| self.compute_distance_matrix())
    }

    fn compute_distance_matrix(&self) -> Array2<f32> {
        let n = self.states.len();
        // Initialize with NaN for non-reachable states
        let mut mat = Array2::from_elem((n, n), f32::NAN);

        for i in 0..n {
            match &self.states[i] {
                State::Seq(_, l, r) => {
                    mat[(i, *l)] = 1.0;
                    mat[(i, *r)] = 1.0;
                }
                State::Alt(_, l, r) => {
                    mat[(i, *l)] = 1.0;
                    mat[(i, *r)] = 1.0;
                }
                _ => {}
            }
        }

        mat
    }

    pub fn left_distance_matrix(&self) -> &Array2<f32> {
        self.left_distance_matrix
            .get_or_init(|| self.compute_left_distance_matrix())
    }

    fn compute_left_distance_matrix(&self) -> Array2<f32> {
        let n = self.states.len();
        // Initialize with NaN for non-reachable states
        let mut mat = Array2::from_elem((n, n), f32::NAN);

        for i in 0..n {
            match &self.states[i] {
                State::Seq(_, l, _) => match &self.states[*l] {
                    State::Tok(_, _) => {
                        mat[(*l, i)] = 1.0;
                    }
                },
                State::Alt(_, l, _) => {
                    mat[(*l, i)] = 1.0;
                }
                _ => {}
            }
        }

        mat
    }

    pub fn transitive_closure(&self) -> &Array2<f32> {
        self.closure_matrix
            .get_or_init(|| self.compute_transitive_closure())
    }

    fn compute_transitive_closure(&self) -> Array2<f32> {
        self.compute_closure_impl(self.distance_matrix())
    }

    pub fn left_transitive_closure(&self) -> &Array2<f32> {
        self.left_closure_matrix
            .get_or_init(|| self.compute_left_closure())
    }

    fn compute_left_closure(&self) -> Array2<f32> {
        self.compute_closure_impl(self.left_distance_matrix())
    }

    fn compute_closure_impl(&self, distance_matrix: &Array2<f32>) -> Array2<f32> {
        let n = self.states.len();
        let mut dist = distance_matrix.clone();

        // Mark all direct self-loops as infinite (cannot improve these paths)
        for i in 0..n {
            if !dist[(i, i)].is_nan() {
                dist[(i, i)] = f32::INFINITY;
            }
        }

        // Floyd-Warshall with state-aware operations:
        // - For Seq states: add costs (both branches required)
        // - For Alt states: take minimum (choose cheaper alternative)
        for k in 0..n {
            for i in 0..n {
                for j in 0..n {
                    // Skip diagonal updates if already marked as infinite
                    if i == j && dist[(i, j)].is_infinite() {
                        continue;
                    }

                    let i_to_k = dist[(i, k)];
                    let k_to_j = dist[(k, j)];

                    if i_to_k.is_nan() || k_to_j.is_nan() {
                        continue;
                    }
                    if i_to_k.is_infinite() || k_to_j.is_infinite() {
                        continue;
                    }

                    let via_k = match &self.states[k] {
                        State::Seq(_, _, _) => i_to_k + k_to_j,
                        State::Alt(_, _, _) => i_to_k.min(k_to_j),
                        State::Tok(_, _) => i_to_k + k_to_j,
                    };

                    if dist[(i, j)].is_nan() || dist[(i, j)] > via_k {
                        dist[(i, j)] = via_k;
                    }
                }
            }
        }

        dist
    }
}

fn rule_reference_edges(table: &RuleTable) -> Vec<Vec<usize>> {
    use NormalizedGrammarNode::*;
    fn walk(node: &NormalizedGrammarNode, out: &mut Vec<usize>) {
        match node {
            Terminal(_) => {}
            Reference(ix) => out.push(*ix),
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
    seq: DashMap<(RefIx, usize, usize), usize>,
    alt: DashMap<(RefIx, usize, usize), usize>,

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
            root: vec![usize::MAX; n],
            built: vec![false; n],
        };

        // Allocate placeholders only for recursive rules.
        for ix in 0..n {
            if this.is_recursive[ix] {
                this.root[ix] = this.states.len();
                this.states.push(State::Tok(ix, Rc::new("")));
            }
        }

        this
    }

    fn rule(&mut self, ix: usize) -> usize {
        if self.built[ix] {
            return self.root[ix];
        }
        self.built[ix] = true;

        if self.is_recursive[ix] {
            let root_id = self.root[ix];
            self.emit_into(root_id, ix, &self.table.rules[ix]);
            root_id
        } else {
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
            Sequence(nodes) => self.fold(nodes, cur_ref_ix, true),
            Alternative(nodes) => self.fold(nodes, cur_ref_ix, false),
        }
    }

    fn mk_tok(&mut self, ref_ix: RefIx, matcher: &Rc<dyn Matcher>) -> usize {
        let key = (ref_ix, matcher.display());
        if let Some(id) = self.tok.get(&key) {
            return *id;
        }
        let id = self.states.len();
        self.states.push(State::Tok(ref_ix, matcher.clone()));
        self.tok.insert(key, id);
        id
    }

    fn mk_seq(&mut self, ref_ix: RefIx, l: usize, r: usize) -> usize {
        let key = (ref_ix, l, r);
        if let Some(id) = self.seq.get(&key) {
            return *id;
        }
        let id = self.states.len();
        self.states.push(State::Seq(ref_ix, l, r));
        self.seq.insert(key, id);
        id
    }

    fn mk_alt(&mut self, ref_ix: RefIx, l: usize, r: usize) -> usize {
        let key = (ref_ix, l, r);
        if let Some(id) = self.alt.get(&key) {
            return *id;
        }
        let id = self.states.len();
        self.states.push(State::Alt(ref_ix, l, r));
        self.alt.insert(key, id);
        id
    }

    // Right-associative fold. If `is_seq` then combine via Seq else Alt.
    fn fold(&mut self, nodes: &[NormalizedGrammarNode], ref_ix: RefIx, is_seq: bool) -> usize {
        let mut it = nodes.iter().rev();
        let mut result = self.node(ref_ix, it.next().expect("Empty fold"));
        for n in it {
            let left = self.node(ref_ix, n);
            result = if is_seq {
                self.mk_seq(ref_ix, left, result)
            } else {
                self.mk_alt(ref_ix, left, result)
            };
        }
        result
    }

    fn register(&mut self, id: usize) {
        match &self.states[id] {
            State::Tok(ix, m) => {
                self.tok.insert((*ix, m.display()), id);
            }
            State::Seq(ix, l, r) => {
                self.seq.insert((*ix, *l, *r), id);
            }
            State::Alt(ix, l, r) => {
                self.alt.insert((*ix, *l, *r), id);
            }
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
            Sequence(nodes) => self.emit_fold(root_id, ref_ix, nodes, true),
            Alternative(nodes) => self.emit_fold(root_id, ref_ix, nodes, false),
        }
    }

    fn emit_fold(
        &mut self,
        root_id: usize,
        ref_ix: RefIx,
        nodes: &[NormalizedGrammarNode],
        is_seq: bool,
    ) {
        let mut it = nodes.iter().rev().peekable();
        let mut result = self.node(ref_ix, it.next().expect("Empty fold"));

        while let Some(n) = it.next() {
            let left = self.node(ref_ix, n);
            if it.peek().is_none() {
                self.states[root_id] = if is_seq {
                    State::Seq(ref_ix, left, result)
                } else {
                    State::Alt(ref_ix, left, result)
                };
                self.register(root_id);
                return;
            }

            result = if is_seq {
                self.mk_seq(ref_ix, left, result)
            } else {
                self.mk_alt(ref_ix, left, result)
            };
        }

        // Single element: root is an alias.
        self.states[root_id] = self.states[result].clone();
        self.register(root_id);
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
