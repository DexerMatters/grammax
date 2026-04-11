use std::fmt::Display;

use regex::Regex;
use rustc_hash::{FxHashMap, FxHashSet};

use crate::scheme::{Command, IR, LazyResult, Span, Transaction, URI};

type ScopeIx = usize;
type EdgeIx = (ScopeIx, ScopeIx);
type Label = &'static str;

pub type WFDatum<T> = fn(&Datum<T>) -> bool;

pub type WFLabel = Regex;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Path {
    Dest(ScopeIx),
    Edge(ScopeIx, Label, Box<Path>),
}

impl Path {
    pub const fn dest(dest: ScopeIx) -> Self {
        Self::Dest(dest)
    }

    pub fn edge(from: ScopeIx, label: Label, path: Path) -> Self {
        Self::Edge(from, label, Box::new(path))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DatumKind<T>
where
    T: Display + Clone + Eq,
{
    Name(String),
    Value(T),
    Empty,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Datum<T>
where
    T: Display + Clone + Eq,
{
    kind: DatumKind<T>,
    uri: URI,
    span: Span,
}

impl<T> Datum<T>
where
    T: Display + Clone + Eq,
{
    pub fn name(name: String, uri: URI, span: Span) -> Self {
        Self {
            kind: DatumKind::Name(name),
            uri,
            span,
        }
    }

    pub fn value(value: T, uri: URI, span: Span) -> Self {
        Self {
            kind: DatumKind::Value(value),
            uri,
            span,
        }
    }

    pub fn empty(uri: URI, span: Span) -> Self {
        Self {
            kind: DatumKind::Empty,
            uri,
            span,
        }
    }

    pub fn satisfy(&self, wf: WFDatum<T>) -> bool {
        wf(self)
    }
}

#[derive(Debug, Clone)]
pub struct ScopeGraphQuery<T>
where
    T: Display + Clone + Eq,
{
    pub start: ScopeIx,
    pub wf_label: WFLabel,
    pub wf_dest: WFDatum<T>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ScopeGraphAnswer<T>
where
    T: Display + Clone + Eq,
{
    pub path: Path,
    pub dest: Datum<T>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ScopeGraphIndex {
    Scope(ScopeIx),
    Edge(ScopeIx, ScopeIx, Label),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ScopeGraphValue<T>
where
    T: Display + Clone + Eq,
{
    Datum(Datum<T>),
    Edge,
}

pub struct ScopeGraphIR<T>
where
    T: Display + Clone + Eq,
{
    edges: FxHashMap<EdgeIx, FxHashSet<Label>>,
    adjacency: FxHashMap<ScopeIx, Vec<(ScopeIx, Label)>>,
    scopes: Vec<Datum<T>>,
}

impl<T> ScopeGraphIR<T>
where
    T: Display + Clone + Eq,
{
    pub fn new() -> Self {
        Self {
            edges: FxHashMap::default(),
            adjacency: FxHashMap::default(),
            scopes: vec![Datum::empty(URI::default(), Span::empty())], // Entry
        }
    }

    pub fn add_scope(&mut self, datum: Datum<T>) -> ScopeIx {
        let ix = self.scopes.len();
        self.scopes.push(datum);
        ix
    }

    pub fn add_edge(&mut self, from: ScopeIx, to: ScopeIx, label: Label) {
        self.edges.entry((from, to)).or_default().insert(label);
        let neighbors = self.adjacency.entry(from).or_default();
        if !neighbors.contains(&(to, label)) {
            neighbors.push((to, label));
        }
    }
}

impl<T> IR for ScopeGraphIR<T>
where
    T: Display + Clone + Eq,
{
    type Query = ScopeGraphQuery<T>;
    type Answer = ScopeGraphAnswer<T>;
    type Index = ScopeGraphIndex;
    type Value = ScopeGraphValue<T>;
    type Fault = ();

    fn query(&self, index: Self::Query) -> LazyResult<Self::Answer, Self::Fault> {
        use rustc_hash::FxHashSet;
        use std::collections::VecDeque;

        let ScopeGraphQuery {
            start,
            wf_label,
            wf_dest,
        } = index;

        let Some(start_datum) = self.scopes.get(start) else {
            return LazyResult::Absent;
        };

        if wf_dest(start_datum) && wf_label.is_match("") {
            return LazyResult::Present(ScopeGraphAnswer {
                path: Path::dest(start),
                dest: start_datum.clone(),
            });
        }

        #[derive(Debug, Clone)]
        struct SearchState {
            scope: ScopeIx,
            parent: Option<usize>,
            from: ScopeIx,
            label: Label,
        }

        let mut states = vec![SearchState {
            scope: start,
            parent: None,
            from: start,
            label: "",
        }];
        let mut queue = VecDeque::from([0usize]);
        let mut visited: FxHashSet<ScopeIx> = FxHashSet::default();
        visited.insert(start);

        while let Some(cur_ix) = queue.pop_front() {
            let cur_scope = states[cur_ix].scope;

            let Some(neighbors) = self.adjacency.get(&cur_scope) else {
                continue;
            };

            for &(to, label) in neighbors {
                if visited.contains(&to) {
                    continue;
                }
                if !wf_label.is_match(label) {
                    continue;
                }

                visited.insert(to);
                let next_ix = states.len();
                states.push(SearchState {
                    scope: to,
                    parent: Some(cur_ix),
                    from: cur_scope,
                    label,
                });

                let datum = &self.scopes[to];
                if wf_dest(datum) {
                    let mut path = Path::dest(to);
                    let mut walk_ix = next_ix;
                    while let Some(parent_ix) = states[walk_ix].parent {
                        let step = &states[walk_ix];
                        path = Path::edge(step.from, step.label, path);
                        walk_ix = parent_ix;
                    }
                    return LazyResult::Present(ScopeGraphAnswer {
                        path,
                        dest: datum.clone(),
                    });
                }

                queue.push_back(next_ix);
            }
        }

        LazyResult::Absent
    }

    fn apply(&mut self, transaction: Transaction<Self>) -> Result<(), Self::Fault>
    where
        Self: Sized,
    {
        let mut staged: FxHashMap<usize, ScopeGraphValue<T>> = FxHashMap::default();
        for command in transaction.iter() {
            match command {
                Command::Create { id, value } => {
                    staged.insert(*id, value.clone());
                }
                Command::Insert { index, id } => match index {
                    ScopeGraphIndex::Scope(ix) => {
                        if let Some(ScopeGraphValue::Datum(datum)) = staged.remove(id) {
                            while self.scopes.len() <= *ix {
                                self.scopes
                                    .push(Datum::empty(URI::default(), Span::empty()));
                            }
                            self.scopes[*ix] = datum;
                        }
                    }
                    ScopeGraphIndex::Edge(from, to, label) => {
                        staged.remove(id);
                        self.add_edge(*from, *to, label);
                    }
                },
                Command::Delete { index } => match index {
                    ScopeGraphIndex::Scope(ix) => {
                        if *ix < self.scopes.len() {
                            self.scopes[*ix] = Datum::empty(URI::default(), Span::empty());
                            self.adjacency.remove(ix);
                            for neighbors in self.adjacency.values_mut() {
                                neighbors.retain(|(to, _)| to != ix);
                            }
                            self.edges.retain(|(f, t), _| f != ix && t != ix);
                        }
                    }
                    ScopeGraphIndex::Edge(from, to, label) => {
                        if let Some(labels) = self.edges.get_mut(&(*from, *to)) {
                            labels.remove(label);
                            if labels.is_empty() {
                                self.edges.remove(&(*from, *to));
                            }
                        }
                        if let Some(neighbors) = self.adjacency.get_mut(from) {
                            neighbors.retain(|(t, l)| t != to || l != label);
                        }
                    }
                },
                Command::Replace { index, id } => match index {
                    ScopeGraphIndex::Scope(ix) => {
                        if let Some(ScopeGraphValue::Datum(datum)) = staged.remove(id) {
                            if *ix < self.scopes.len() {
                                self.scopes[*ix] = datum;
                            }
                        }
                    }
                    ScopeGraphIndex::Edge(..) => {
                        staged.remove(id); // edges are binary; replace is a no-op
                    }
                },
            }
        }
        Ok(())
    }
}
