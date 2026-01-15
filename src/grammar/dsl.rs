use std::{
    ops::{self, RangeBounds},
    rc::Rc,
};

use crate::parsec::words::Matcher;

#[derive(Clone)]
pub enum GrammarNode {
    Terminal(Rc<dyn Matcher>),
    Alternative(Vec<GrammarNode>),
    Sequence(Vec<GrammarNode>),
    Reference(fn() -> GrammarNode, &'static str),
    Repetition {
        node: Box<GrammarNode>,
        min: usize,
        max: Option<usize>,
    },
}

pub fn r(f: fn() -> GrammarNode, name: &'static str) -> GrammarNode {
    GrammarNode::Reference(f, name)
}

pub fn t<M: Matcher + 'static>(matcher: M) -> GrammarNode {
    GrammarNode::Terminal(Rc::new(matcher))
}

pub fn seq(nodes: Vec<GrammarNode>) -> GrammarNode {
    GrammarNode::Sequence(nodes)
}

pub fn alt(nodes: Vec<GrammarNode>) -> GrammarNode {
    GrammarNode::Alternative(nodes)
}

pub fn many(node: GrammarNode) -> GrammarNode {
    GrammarNode::Repetition {
        node: Box::new(node),
        min: 0,
        max: None,
    }
}

pub fn some(node: GrammarNode) -> GrammarNode {
    GrammarNode::Repetition {
        node: Box::new(node),
        min: 1,
        max: None,
    }
}

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
    }
}

impl ops::Add for GrammarNode {
    type Output = GrammarNode;

    fn add(self, rhs: GrammarNode) -> Self::Output {
        match self {
            GrammarNode::Sequence(mut nodes) => {
                nodes.push(rhs);
                GrammarNode::Sequence(nodes)
            }
            _ => GrammarNode::Sequence(vec![self, rhs]),
        }
    }
}

impl ops::BitOr for GrammarNode {
    type Output = GrammarNode;

    fn bitor(self, rhs: GrammarNode) -> Self::Output {
        match self {
            GrammarNode::Alternative(mut nodes) => {
                nodes.push(rhs);
                GrammarNode::Alternative(nodes)
            }
            _ => GrammarNode::Alternative(vec![self, rhs]),
        }
    }
}
