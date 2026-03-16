use std::{
    ops::{self, RangeBounds},
    sync::Arc,
};

use crate::parsec::words::{Matcher, MatcherRef, token};
use crate::utils::Span;
use rustc_hash::FxHashMap;

#[doc(hidden)]
#[derive(Clone, Debug, Hash)]
pub enum GrammarNode {
    Terminal(MatcherRef, Span),
    Alternative(Vec<GrammarNode>, Span),
    Sequence(Vec<GrammarNode>, Span),
    /// Bound reference resolved at compile-time via function pointer
    Reference(fn() -> GrammarNode, &'static str, Span),
    /// Unbound reference resolved at runtime via GrammarRegistry
    UnboundReference(String, Span),
    Field(&'static str, Box<GrammarNode>, Span),
    Drop {
        node: Box<GrammarNode>,
        count: usize,
        span: Span,
    },
    Repetition {
        node: Box<GrammarNode>,
        min: usize,
        max: Option<usize>,
        span: Span,
    },
    SeparatedRepetition {
        node: Box<GrammarNode>,
        separator: Box<GrammarNode>,
        min: usize,
        max: Option<usize>,
        span: Span,
    },
}

impl GrammarNode {
    pub fn drop(self, count: usize) -> Self {
        GrammarNode::Drop {
            node: Box::new(self),
            count,
            span: Span::empty(),
        }
    }
}

// Helper functions for DSL

/// Defines a named rule reference in the grammar.
///
/// Use the `r!` macro for more ergonomic syntax when defining rules.
pub fn r(f: fn() -> GrammarNode, name: &'static str) -> GrammarNode {
    GrammarNode::Reference(f, name, Span::empty())
}

/// Defines a terminal matcher in the grammar.
pub fn t<M: Matcher + Send + Sync + 'static>(matcher: M) -> GrammarNode {
    GrammarNode::Terminal(Arc::new(matcher), Span::empty())
}

/// Defines a terminal matcher which skips leading trivia (whitespace/newlines).
pub fn tt<M: Matcher + Send + Sync + 'static>(matcher: M) -> GrammarNode {
    GrammarNode::Terminal(Arc::new(token(matcher)), Span::empty())
}

/// Defines a sequence of grammar nodes.
///
/// Use the `+` operator for more ergonomic syntax when defining sequences.
pub fn seq(nodes: Vec<GrammarNode>) -> GrammarNode {
    GrammarNode::Sequence(nodes, Span::empty())
}

/// Defines an alternative between grammar nodes.
///
/// Use the `|` operator for more ergonomic syntax when defining alternatives.
pub fn alt(nodes: Vec<GrammarNode>) -> GrammarNode {
    GrammarNode::Alternative(nodes, Span::empty())
}

/// Defines an optional grammar node (zero or one occurrence).
///
/// It is equivalent to `repeat(node, 0..=1)` or `node | seq(vec![])`.
pub fn opt(node: GrammarNode) -> GrammarNode {
    GrammarNode::Repetition {
        node: Box::new(node),
        min: 0,
        max: Some(1),
        span: Span::empty(),
    }
}

/// Defines a grammar node that can be repeated with specified bounds.
///
/// The `range` parameter specifies the minimum and maximum number of occurrences. For example:
/// - `0..` means zero or more occurrences (equivalent to `many`)
/// - `1..` means one or more occurrences (equivalent to `some`)
/// - `0..=1` means zero or one occurrence (equivalent to `opt`)
pub fn repeat<R: RangeBounds<usize>>(node: GrammarNode, range: R) -> GrammarNode {
    let min = match range.start_bound() {
        std::ops::Bound::Included(&n) => n,
        std::ops::Bound::Excluded(&n) => n + 1,
        std::ops::Bound::Unbounded => 0,
    };
    let max = match range.end_bound() {
        std::ops::Bound::Included(&n) => Some(n),
        std::ops::Bound::Excluded(&n) => Some(n.saturating_sub(1)),
        std::ops::Bound::Unbounded => None,
    };
    GrammarNode::Repetition {
        node: Box::new(node),
        min,
        max,
        span: Span::empty(),
    }
}

/// Defines a grammar node that can be repeated zero or more times.
///
/// It is equivalent to `repeat(node, 0..)`.
pub fn many(node: GrammarNode) -> GrammarNode {
    GrammarNode::Repetition {
        node: Box::new(node),
        min: 0,
        max: None,
        span: Span::empty(),
    }
}

/// Defines a grammar node that can be repeated one or more times.
///
/// It is equivalent to `repeat(node, 1..)`.
pub fn some(node: GrammarNode) -> GrammarNode {
    GrammarNode::Repetition {
        node: Box::new(node),
        min: 1,
        max: None,
        span: Span::empty(),
    }
}

/// Defines a grammar node that can be repeated zero or more times with a separator between occurrences.
pub fn sep(node: GrammarNode, separator: GrammarNode) -> GrammarNode {
    GrammarNode::SeparatedRepetition {
        node: Box::new(node),
        separator: Box::new(separator),
        min: 0,
        max: None,
        span: Span::empty(),
    }
}

/// Defines a grammar node that can be repeated one or more times with a separator between occurrences.
pub fn sep1(node: GrammarNode, separator: GrammarNode) -> GrammarNode {
    GrammarNode::SeparatedRepetition {
        node: Box::new(node),
        separator: Box::new(separator),
        min: 1,
        max: None,
        span: Span::empty(),
    }
}

/// Defines a named field in the grammar, which is convenient for AST construction and semantic analysis.
pub fn field(name: &'static str, node: GrammarNode) -> GrammarNode {
    GrammarNode::Field(name, Box::new(node), Span::empty())
}

// Operator overloading for ergonomic DSL

impl ops::Add for GrammarNode {
    type Output = GrammarNode;

    fn add(self, rhs: GrammarNode) -> Self::Output {
        match self {
            GrammarNode::Sequence(mut nodes, span) => {
                nodes.push(rhs);
                GrammarNode::Sequence(nodes, span)
            }
            _ => GrammarNode::Sequence(vec![self, rhs], Span::empty()),
        }
    }
}

impl ops::BitOr for GrammarNode {
    type Output = GrammarNode;

    fn bitor(self, rhs: GrammarNode) -> Self::Output {
        match self {
            GrammarNode::Alternative(mut nodes, span) => {
                nodes.push(rhs);
                GrammarNode::Alternative(nodes, span)
            }
            _ => GrammarNode::Alternative(vec![self, rhs], Span::empty()),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct GrammarRegistry {
    rules: FxHashMap<&'static str, GrammarNode>,
}

impl GrammarRegistry {
    pub fn new() -> Self {
        GrammarRegistry {
            rules: FxHashMap::default(),
        }
    }

    pub fn from_map(rules: FxHashMap<&'static str, GrammarNode>) -> Self {
        GrammarRegistry { rules }
    }

    pub fn get(&self, name: &str) -> Option<&GrammarNode> {
        self.rules.get(name)
    }
}

impl std::ops::Index<&str> for GrammarRegistry {
    type Output = GrammarNode;

    fn index(&self, name: &str) -> &Self::Output {
        &self.rules[name]
    }
}

impl Default for GrammarRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl FromIterator<(&'static str, GrammarNode)> for GrammarRegistry {
    fn from_iter<T: IntoIterator<Item = (&'static str, GrammarNode)>>(iter: T) -> Self {
        GrammarRegistry {
            rules: iter.into_iter().collect(),
        }
    }
}
