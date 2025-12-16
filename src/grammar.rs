use core::fmt;
use std::{collections::BTreeSet, hash};

use indexmap::IndexSet;

use crate::{core::utils::Range, words::Matcher};

#[derive(Debug, Clone)]
pub enum EvaluationError {
    UndecidableRule(String),
    AlwaysFails,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum GrammarError {
    Placeholder,
    RuleMismatch { expected: usize },
    TokenMismatch { expected: String },
}

pub type Result<T> = std::result::Result<T, EvaluationError>;

pub type RuleFn = fn() -> GrammarNode;

pub enum GrammarNode {
    Terminal(Box<dyn Matcher>),
    Choice(Vec<GrammarNode>),
    Sequence(Vec<GrammarNode>),
    Reference(RuleFn, &'static str),
    Optional(Box<GrammarNode>),
    Some(Box<GrammarNode>),
    Many(Box<GrammarNode>),
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
    Mu,
}

#[derive(Debug)]
pub struct Rule {
    pub name: &'static str,
    pub node: NormalizedNode,
    pub is_recursive: bool,
}

impl hash::Hash for Rule {
    fn hash<H: hash::Hasher>(&self, state: &mut H) {
        self.name.hash(state);
    }
}

impl PartialEq for Rule {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}

impl Eq for Rule {}

pub struct Grammar {
    rules: IndexSet<Rule>,
    start: usize,
}

impl TryFrom<GrammarNode> for Grammar {
    type Error = EvaluationError;
    fn try_from(node: GrammarNode) -> Result<Self> {
        let mut rules = IndexSet::new();
        let start = normalize(node, &mut rules, 0)?;
        let start_rule = Rule {
            name: "START",
            node: start,
            is_recursive: false,
        };
        rules.insert_before(0, start_rule);
        Ok(Grammar { rules, start: 0 })
    }
}

fn normalize(
    node: GrammarNode,
    rules: &mut IndexSet<Rule>,
    current: usize,
) -> Result<NormalizedNode> {
    use GrammarNode::*;
    use NormalizedNode as N;
    match node {
        Terminal(m) => Ok(N::Terminal(m)),
        Reference(f, name) => {
            let
        }
    }
}
