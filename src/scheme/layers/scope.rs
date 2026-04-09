use crate::scheme::Span;

pub enum Edge {
    Lexical,
    Type,
}

pub enum Scope {
    Identifier { name: String, span: Span },
}

pub struct ScopeGraph {
    graph: ultragraph::DynamicGraph<Scope, Edge>,
}
