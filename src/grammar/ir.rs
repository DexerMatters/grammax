use std::ops;

use serde::{Deserialize, Serialize};

use crate::parsec::words::MatcherRef;

/// Normalized representation of grammar after desugaring
#[derive(Clone, Debug)]
pub enum NormalizedNode {
    Terminal(MatcherRef),
    Alternative(Vec<NormalizedNode>),
    Sequence(Vec<NormalizedNode>),
    Reference(usize), // Rule index
    Field(&'static str, Box<NormalizedNode>),
}

use NormalizedNode::*;

impl ops::Add for NormalizedNode {
    type Output = NormalizedNode;

    fn add(self, other: NormalizedNode) -> NormalizedNode {
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

impl ops::BitOr for NormalizedNode {
    type Output = NormalizedNode;

    fn bitor(self, other: NormalizedNode) -> NormalizedNode {
        match (self, other) {
            (Alternative(mut alt1), Alternative(alt2)) => {
                alt1.extend(alt2);
                Alternative(alt1)
            }
            (Alternative(mut alt), node) => {
                alt.push(node);
                Alternative(alt)
            }
            (node, Alternative(mut alt)) => {
                alt.insert(0, node);
                Alternative(alt)
            }
            (node1, node2) => Alternative(vec![node1, node2]),
        }
    }
}

/// Rule reference information
#[derive(Clone, Debug)]
pub struct RuleInfo {
    pub name: &'static str,
    pub description: &'static str,
    pub node: NormalizedNode,
}

/// Production rule for LR parsing
#[derive(Clone, Debug)]
pub struct Production {
    pub lhs: usize, // Rule index
    pub rhs: Vec<Symbol>,
    pub field_positions: Vec<(usize, &'static str)>, // (position, field_name)
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Symbol {
    Terminal(usize),    // Index into terminal table
    NonTerminal(usize), // Rule index
}
