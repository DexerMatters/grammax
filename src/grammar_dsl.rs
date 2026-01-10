use core::fmt;
use std::ops;
use std::rc::Rc;

use crate::words::Matcher;

pub type RuleFn = fn() -> GrammarNode;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct RuleProperties {
    pub is_recursive: bool,
    pub is_trivial: bool,
}

pub const DEFAULT_RULE_PROPS: RuleProperties = RuleProperties {
    is_recursive: false,
    is_trivial: false,
};

impl RuleProperties {
    pub fn new(is_recursive: bool, is_trivial: bool) -> Self {
        Self {
            is_recursive,
            is_trivial,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct RuleNodeProperties {
    pub is_nullable: bool,
    pub is_consuming: bool,
    pub max_depth: usize,
    pub min_depth: usize,
    pub min_consuming_steps: usize,
    pub max_consuming_steps: usize,
}

pub const DEFAULT_RULE_NODE_PROPS: RuleNodeProperties = RuleNodeProperties {
    is_nullable: false,
    is_consuming: false,
    max_depth: 0,
    min_depth: 0,
    min_consuming_steps: 0,
    max_consuming_steps: 0,
};

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct RuleName {
    pub name: String,
    pub meta: usize,
}

impl RuleName {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            meta: 0,
        }
    }
    pub fn new_meta(name: impl Into<String>, meta: usize) -> Self {
        Self {
            name: name.into(),
            meta,
        }
    }
    pub fn is_trivial(&self) -> bool {
        self.name.starts_with("_") || self.meta >= 1
    }
}

impl fmt::Display for RuleName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}{}",
            self.name,
            if self.meta > 0 {
                format!("_{}", self.meta)
            } else {
                "".to_string()
            }
        )
    }
}

impl From<String> for RuleName {
    fn from(s: String) -> Self {
        Self { name: s, meta: 0 }
    }
}

impl From<&str> for RuleName {
    fn from(s: &str) -> Self {
        Self {
            name: s.to_string(),
            meta: 0,
        }
    }
}

pub enum GrammarNode {
    Terminal(Rc<dyn Matcher>),
    Choice(Vec<GrammarNode>),
    Sequence(Vec<GrammarNode>),
    Reference(RuleFn, &'static str),
    _Reference(RuleFn, RuleName),
    Optional(Box<GrammarNode>),
    Some(Box<GrammarNode>),
    Many(Box<GrammarNode>),
}

impl GrammarNode {
    pub fn is_reference(&self) -> bool {
        matches!(self, GrammarNode::Reference(_, _))
    }
}

#[derive(Debug, Clone)]
pub struct NormalizedNode {
    pub kind: NormalizedNodeKind,
    pub properties: RuleNodeProperties,
}

impl NormalizedNode {
    #[inline]
    pub fn never_halt(&self) -> bool {
        self.properties.min_depth == usize::MAX && !self.properties.is_consuming
    }
    #[inline]
    pub fn never_succeed(&self) -> bool {
        self.properties.min_depth == usize::MAX && self.properties.is_consuming
    }
    #[inline]
    pub fn non_terminable(&self) -> bool {
        self.properties.max_depth == usize::MAX
    }
    #[inline]
    pub fn fixed_terminal(&self) -> bool {
        !self.non_terminable()
            && self.properties.max_consuming_steps == self.properties.min_consuming_steps
    }
    #[inline]
    pub fn variadic_terminal(&self) -> bool {
        !self.non_terminable()
            && self.properties.max_consuming_steps > self.properties.min_consuming_steps
    }
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.properties.max_consuming_steps == 0
    }
    #[inline]
    pub fn possibly_empty(&self) -> bool {
        self.properties.min_consuming_steps == 0
    }
}

#[derive(Debug)]
pub enum NormalizedNodeKind {
    Terminal(Rc<dyn Matcher>),
    Choice(Vec<NormalizedNode>),
    Sequence(Vec<NormalizedNode>),
    Reference(usize),
    Placeholder,
}

impl Clone for NormalizedNodeKind {
    fn clone(&self) -> Self {
        match self {
            NormalizedNodeKind::Terminal(m) => NormalizedNodeKind::Terminal(Rc::clone(m)),
            NormalizedNodeKind::Choice(nodes) => {
                NormalizedNodeKind::Choice(nodes.iter().map(|n| n.clone()).collect())
            }
            NormalizedNodeKind::Sequence(nodes) => {
                NormalizedNodeKind::Sequence(nodes.iter().map(|n| n.clone()).collect())
            }
            NormalizedNodeKind::Reference(idx) => NormalizedNodeKind::Reference(*idx),
            NormalizedNodeKind::Placeholder => NormalizedNodeKind::Placeholder,
        }
    }
}

impl NormalizedNodeKind {
    pub fn is_reference(&self) -> bool {
        matches!(self, NormalizedNodeKind::Reference(_))
    }
    pub fn null() -> Self {
        NormalizedNodeKind::Sequence(vec![])
    }
}

pub struct NormalizedNodeWalker;

impl NormalizedNodeWalker {
    pub fn for_each(node: &NormalizedNode, mut f: impl FnMut(&NormalizedNode)) {
        fn walk(node: &NormalizedNode, f: &mut impl FnMut(&NormalizedNode)) {
            f(node);
            match &node.kind {
                NormalizedNodeKind::Choice(nodes) | NormalizedNodeKind::Sequence(nodes) => {
                    for n in nodes {
                        walk(n, f);
                    }
                }
                _ => {}
            }
        }

        walk(node, &mut f);
    }

    pub fn for_each_mut(node: &mut NormalizedNode, mut f: impl FnMut(&mut NormalizedNode)) {
        fn walk(node: &mut NormalizedNode, f: &mut impl FnMut(&mut NormalizedNode)) {
            f(node);
            match &mut node.kind {
                NormalizedNodeKind::Choice(nodes) | NormalizedNodeKind::Sequence(nodes) => {
                    for n in nodes {
                        walk(n, f);
                    }
                }
                _ => {}
            }
        }

        walk(node, &mut f);
    }

    pub fn post_order_mut(node: &mut NormalizedNode, mut f: impl FnMut(&mut NormalizedNode)) {
        fn walk(node: &mut NormalizedNode, f: &mut impl FnMut(&mut NormalizedNode)) {
            match &mut node.kind {
                NormalizedNodeKind::Choice(nodes) | NormalizedNodeKind::Sequence(nodes) => {
                    for n in nodes {
                        walk(n, f);
                    }
                }
                _ => {}
            }
            f(node);
        }

        walk(node, &mut f);
    }

    pub fn collect_references(node: &NormalizedNode, out: &mut Vec<usize>) {
        Self::for_each(node, |n| {
            if let NormalizedNodeKind::Reference(idx) = n.kind {
                out.push(idx);
            }
        });
    }

    pub fn map(
        node: &NormalizedNode,
        mut f: impl FnMut(NormalizedNode) -> NormalizedNode,
    ) -> NormalizedNode {
        fn walk(
            node: &NormalizedNode,
            f: &mut impl FnMut(NormalizedNode) -> NormalizedNode,
        ) -> NormalizedNode {
            let mapped_kind = match &node.kind {
                NormalizedNodeKind::Terminal(m) => NormalizedNodeKind::Terminal(Rc::clone(m)),
                NormalizedNodeKind::Reference(idx) => NormalizedNodeKind::Reference(*idx),
                NormalizedNodeKind::Placeholder => NormalizedNodeKind::Placeholder,
                NormalizedNodeKind::Choice(nodes) => {
                    NormalizedNodeKind::Choice(nodes.iter().map(|n| walk(n, f)).collect())
                }
                NormalizedNodeKind::Sequence(nodes) => {
                    NormalizedNodeKind::Sequence(nodes.iter().map(|n| walk(n, f)).collect())
                }
            };

            let mapped = NormalizedNode {
                kind: mapped_kind,
                properties: node.properties,
            };

            f(mapped)
        }

        walk(node, &mut f)
    }
}

#[inline]
pub fn t<M: Matcher + 'static>(matcher: M) -> GrammarNode {
    GrammarNode::Terminal(Rc::new(matcher))
}

#[inline]
pub fn r(rule: RuleFn, name: &'static str) -> GrammarNode {
    GrammarNode::Reference(rule, name)
}

#[inline]
pub fn choice(nodes: impl IntoIterator<Item = GrammarNode>) -> GrammarNode {
    GrammarNode::Choice(nodes.into_iter().collect())
}

#[inline]
pub fn seq(nodes: impl IntoIterator<Item = GrammarNode>) -> GrammarNode {
    GrammarNode::Sequence(nodes.into_iter().collect())
}

#[inline]
pub fn opt(node: impl Into<GrammarNode>) -> GrammarNode {
    GrammarNode::Optional(Box::new(node.into()))
}

#[inline]
pub fn some(node: impl Into<GrammarNode>) -> GrammarNode {
    GrammarNode::Some(Box::new(node.into()))
}

#[inline]
pub fn many(node: impl Into<GrammarNode>) -> GrammarNode {
    GrammarNode::Many(Box::new(node.into()))
}

#[macro_export]
macro_rules! r {
    ($rule_fn:expr) => {
        $crate::grammar_dsl::r($rule_fn, stringify!($rule_fn))
    };
}

impl ops::Add for GrammarNode {
    type Output = GrammarNode;

    fn add(self, rhs: Self) -> Self::Output {
        match (self, rhs) {
            (GrammarNode::Sequence(mut left), GrammarNode::Sequence(right)) => {
                left.extend(right);
                GrammarNode::Sequence(left)
            }
            (GrammarNode::Sequence(mut left), right) => {
                left.push(right);
                GrammarNode::Sequence(left)
            }
            (left, GrammarNode::Sequence(mut right)) => {
                right.insert(0, left);
                GrammarNode::Sequence(right)
            }
            (left, right) => GrammarNode::Sequence(vec![left, right]),
        }
    }
}

impl ops::BitOr for GrammarNode {
    type Output = GrammarNode;

    fn bitor(self, rhs: Self) -> Self::Output {
        match (self, rhs) {
            (GrammarNode::Choice(mut left), GrammarNode::Choice(right)) => {
                left.extend(right);
                GrammarNode::Choice(left)
            }
            (GrammarNode::Choice(mut left), right) => {
                left.push(right);
                GrammarNode::Choice(left)
            }
            (left, GrammarNode::Choice(mut right)) => {
                right.insert(0, left);
                GrammarNode::Choice(right)
            }
            (left, right) => GrammarNode::Choice(vec![left, right]),
        }
    }
}
