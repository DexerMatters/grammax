use std::rc::Rc;

use dashmap::{DashMap, DashSet};

use crate::{
    grammar::{
        ir::{NormalizedGrammarNode, RefIx, State},
        norm::RuleTable,
    },
    parsec::words::Matcher,
};

impl State {
    pub fn ref_ix(&self) -> RefIx {
        match self {
            State::Tok(ix, _)
            | State::Seq(ix, _)
            | State::Alt(ix, _)
            | State::LeftRec(ix, _, _) => *ix,
        }
    }

    /// Get child state indices for this state
    pub fn children(&self) -> &[usize] {
        match self {
            State::Tok(_, _) => &[],
            State::Seq(_, children) | State::Alt(_, children) => children,
            State::LeftRec(_, _, _) => &[],
        }
    }
}

#[derive(Clone, Debug)]
pub struct GrammarStateAnalysis {
    pub states: Vec<State>,
    pub start_state: usize,
}

impl GrammarStateAnalysis {
    pub fn from_table(table: &RuleTable, start: usize) -> Self {
        let rule_edges = rule_reference_edges(table);
        let is_recursive = recursive_rules(&rule_edges);

        let mut builder = Builder::new(table, is_recursive);
        let start_state = builder.rule(start);
        Self {
            states: builder.states,
            start_state,
        }
    }

    pub fn rule_set(&self) -> DashSet<usize> {
        self.states.iter().map(|s| s.ref_ix()).collect()
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
    seq: DashMap<(RefIx, Vec<usize>), usize>,
    alt: DashMap<(RefIx, Vec<usize>), usize>,

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

        // Allocate placeholders only for recursive rules
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

        if let Some(info) = &self.table.left_rec[ix] {
            let root_id = if self.root[ix] != usize::MAX {
                self.root[ix]
            } else {
                let id = self.states.len();
                self.root[ix] = id;
                self.states.push(State::Tok(ix, Rc::new("")));
                id
            };

            let base_children: Vec<usize> = info.base.iter().map(|n| self.node(ix, n)).collect();
            let tail_children: Vec<usize> = info.tail.iter().map(|n| self.node(ix, n)).collect();

            self.states[root_id] = State::LeftRec(ix, base_children, tail_children);
            self.register(root_id);
            return root_id;
        }

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
            Sequence(nodes) => self.mk_seq_nary(cur_ref_ix, nodes),
            Alternative(nodes) => self.mk_alt_nary(cur_ref_ix, nodes),
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
        let id = self.states.len();
        self.states.push(State::Alt(ref_ix, children));
        self.alt.insert(key, id);
        id
    }

    fn register(&mut self, id: usize) {
        match &self.states[id] {
            State::Tok(ix, m) => {
                self.tok.insert((*ix, m.display()), id);
            }
            State::Seq(ix, children) => {
                self.seq.insert((*ix, children.clone()), id);
            }
            State::Alt(ix, children) => {
                self.alt.insert((*ix, children.clone()), id);
            }
            State::LeftRec(_, _, _) => {}
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
                self.states[root_id] = State::Alt(ref_ix, children);
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
