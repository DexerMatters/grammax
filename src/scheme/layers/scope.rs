use std::fmt::Display;

use multimap::MultiMap;
use regex::Regex;
use rustc_hash::FxHashMap;

use crate::scheme::{Command, IR, LazyResult, Span, Transaction};

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
    span: Span,
}

impl<T> Datum<T>
where
    T: Display + Clone + Eq,
{
    pub fn name(name: String, span: Span) -> Self {
        Self {
            kind: DatumKind::Name(name),
            span,
        }
    }

    pub fn value(value: T, span: Span) -> Self {
        Self {
            kind: DatumKind::Value(value),
            span,
        }
    }

    pub fn empty(span: Span) -> Self {
        Self {
            kind: DatumKind::Empty,
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

pub struct ScopeGraphIR<T>
where
    T: Display + Clone + Eq,
{
    edges: MultiMap<EdgeIx, Label>,
    adjacency: FxHashMap<ScopeIx, Vec<(ScopeIx, Label)>>,
    scopes: Vec<Datum<T>>,
    staging: FxHashMap<usize, ScopeGraphAnswer<T>>,
}

impl<T> ScopeGraphIR<T>
where
    T: Display + Clone + Eq,
{
    pub fn new() -> Self {
        Self {
            edges: MultiMap::new(),
            adjacency: FxHashMap::default(),
            scopes: vec![Datum::empty(Span::empty())], // Entry
            staging: FxHashMap::default(),
        }
    }

    pub fn add_scope(&mut self, datum: Datum<T>) -> ScopeIx {
        let ix = self.scopes.len();
        self.scopes.push(datum);
        ix
    }

    pub fn add_edge(&mut self, from: ScopeIx, to: ScopeIx, label: Label) {
        self.edges.insert((from, to), label);
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
    type Ix = ScopeGraphQuery<T>;
    type Value = ScopeGraphAnswer<T>;
    type Fault = ();

    fn query(&self, index: Self::Ix) -> LazyResult<Self::Value, Self::Fault> {
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

    fn apply_transaction(&mut self, transaction: Transaction<Self>) -> Result<(), Self::Fault>
    where
        Self: Sized,
    {
        self.staging.clear();
        for command in transaction.iter() {
            match command {
                Command::Create { id, value } => {
                    self.staging.insert(*id, value.clone());
                }
                Command::Insert { index: _, id: _ } => {
                    todo!()
                }
                Command::Delete { index: _ } => {
                    todo!()
                }
                Command::Replace { index: _, id: _ } => {
                    todo!()
                }
            }
        }

        Ok(())
    }
}
