use std::{ops, rc::Rc, sync::Arc};

use crate::parsec::words::{Matcher, MatcherRef};

#[derive(Clone, Debug)]
pub enum NormalizedGrammarNode {
    Terminal(MatcherRef),
    Alternative(Vec<NormalizedGrammarNode>),
    Sequence(Vec<NormalizedGrammarNode>),
    Reference(usize),
    Field(&'static str, Box<NormalizedGrammarNode>),
}

use NormalizedGrammarNode::*;

impl ops::Add for NormalizedGrammarNode {
    type Output = NormalizedGrammarNode;

    fn add(self, other: NormalizedGrammarNode) -> NormalizedGrammarNode {
        match (self, other) {
            (Sequence(mut seq1), Sequence(seq2)) => {
                seq1.extend(seq2);
                Sequence(seq1)
            }
            (Sequence(mut seq), node) => {
                seq.push(node);
                Sequence(seq)
            }
            (node, Sequence(mut seq)) => {
                seq.insert(0, node);
                Sequence(seq)
            }
            (node1, node2) => Sequence(vec![node1, node2]),
        }
    }
}

impl ops::BitOr for NormalizedGrammarNode {
    type Output = NormalizedGrammarNode;

    fn bitor(self, other: NormalizedGrammarNode) -> NormalizedGrammarNode {
        match (self, other) {
            (Alternative(mut alt1), Alternative(alt2)) => {
                alt1.extend(alt2);
                Alternative(alt1)
            }
            (Alternative(mut alt), node) | (node, Alternative(mut alt)) => {
                alt.insert(0, node);
                Alternative(alt)
            }
            (node1, node2) => Alternative(vec![node1, node2]),
        }
    }
}

pub type RefIx = usize;

#[derive(Clone, Debug)]
pub enum State {
    Tok(RefIx, MatcherRef),
    Seq(RefIx, Vec<usize>),
    Alt(RefIx, Vec<usize>, bool), // (rule_ix, children, has_epsilon)
    Field(RefIx, &'static str, usize),
    LeftRec(RefIx, Vec<usize>, Vec<usize>, Vec<Option<&'static str>>),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Scope {
    pub open: String,
    pub close: String,
    pub rule_ix: usize,
}

#[derive(Clone, Debug)]
pub struct BridgeGrammar {
    pub scopes: Vec<Scope>,
    pub reefs: Vec<String>,
}
