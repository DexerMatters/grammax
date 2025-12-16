use std::ops;

use crate::words::Matcher;

pub type RuleFn = fn() -> GrammarNode;

pub enum GrammarNode {
    Terminal(Box<dyn Matcher>),
    Choice(Vec<GrammarNode>),
    Sequence(Vec<GrammarNode>),
    Reference(RuleFn, &'static str),
    // Optional(Box<GrammarNode>),
    // Some(Box<GrammarNode>),
    // Many(Box<GrammarNode>),
}

impl GrammarNode {
    pub fn is_reference(&self) -> bool {
        matches!(self, GrammarNode::Reference(_, _))
    }
}

#[derive(Debug)]
pub enum NormalizedNode {
    Terminal(Box<dyn Matcher>),
    Choice(Vec<NormalizedNode>),
    Sequence(Vec<NormalizedNode>),
    Reference(usize),
    Placeholder,
}

#[inline]
pub fn t<M: Matcher + 'static>(matcher: M) -> GrammarNode {
    GrammarNode::Terminal(Box::new(matcher))
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
