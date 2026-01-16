use std::{fmt, ops, rc::Rc};

use crate::parsec::words::Matcher;

#[derive(Clone, Debug)]
pub enum NormalizedGrammarNode {
    Terminal(Rc<dyn Matcher>),
    Alternative(Vec<NormalizedGrammarNode>),
    Sequence(Vec<NormalizedGrammarNode>),
    Reference(usize),
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

impl fmt::Display for NormalizedGrammarNode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Terminal(matcher) => {
                write!(f, "{}", matcher.display())
            }
            Reference(index) => {
                write!(f, "@{}", index)
            }
            Sequence(nodes) => {
                let parts: Vec<String> = nodes.iter().map(|n| n.to_string()).collect();
                write!(f, "({})", parts.join(" "))
            }
            Alternative(nodes) => {
                let parts: Vec<String> = nodes.iter().map(|n| n.to_string()).collect();
                write!(f, "({})", parts.join(" | "))
            }
        }
    }
}

pub type RefIx = usize;

#[derive(Clone, Debug)]
pub enum State {
    Tok(RefIx, Rc<dyn Matcher>),
    Seq(RefIx, Vec<usize>),
    Alt(RefIx, Vec<usize>),
    LeftRec(RefIx, Vec<usize>, Vec<usize>),
}
