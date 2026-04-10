use multimap::MultiMap;

use crate::scheme::{IR, LazyResult, Span, Transaction};

type ScopeIx = usize;
type EdgeIx = (ScopeIx, ScopeIx);
type Label = &'static str;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Path {
    Scope(ScopeIx),
    Edge(ScopeIx, Label, Box<Path>),
}

impl Path {
    pub fn scope(ix: ScopeIx) -> Self {
        Self::Scope(ix)
    }
    pub fn edge(from: ScopeIx, label: Label, to: Path) -> Self {
        Self::Edge(from, label, Box::new(to))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Datum {
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct ScopeGraphQuery {
    start: ScopeIx,
    regex: String,
    datum: Datum,
}

#[derive(Debug, Clone)]
pub enum ScopeGraphError {}

#[derive(Debug, Clone)]
pub struct ScopeGraphValue {
    datum: Datum,
    path: Path,
}

pub struct ScopeGraphIR {
    pub edges: MultiMap<EdgeIx, Label>,
    pub scopes: Vec<Datum>,
}

impl ScopeGraphIR {
    pub fn new() -> Self {
        Self {
            edges: MultiMap::new(),
            scopes: Vec::new(),
        }
    }

    pub fn add_scope(&mut self, datum: Datum) -> ScopeIx {
        let ix = self.scopes.len();
        self.scopes.push(datum);
        ix
    }

    pub fn add_edge(&mut self, from: ScopeIx, to: ScopeIx, label: Label) {
        self.edges.insert((from, to), label);
    }
}

impl IR for ScopeGraphIR {
    type Ix = ScopeGraphQuery;
    type Value = ScopeGraphValue;
    type Fault = ScopeGraphError;

    fn query(&self, index: Self::Ix) -> LazyResult<Self::Value, Self::Fault> {
        todo!()
    }

    fn apply_transaction(&mut self, transaction: Transaction<Self>) -> Result<(), Self::Fault>
    where
        Self: Sized,
    {
        todo!()
    }
}
