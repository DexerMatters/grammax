use core::fmt;
use std::{collections::BTreeSet, hash};

use indexmap::{IndexSet, set::MutableValues};

use crate::{core::utils::Range, grammar_dsl::*, words::Matcher};

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

        // Shift all references by 1 to make room for START at index 0
        let shifted_rules: IndexSet<Rule> = rules
            .into_iter()
            .map(|mut rule| {
                rule.node = shift_references(rule.node, 1);
                rule
            })
            .collect();

        let start_rule = Rule {
            name: "START",
            node: shift_references(start.0, 1),
            is_recursive: false,
        };

        let mut final_rules = IndexSet::new();
        final_rules.insert(start_rule);
        final_rules.extend(shifted_rules);

        Ok(Grammar {
            rules: final_rules,
            start: 0,
        })
    }
}

fn shift_references(node: NormalizedNode, offset: usize) -> NormalizedNode {
    use NormalizedNode as N;
    match node {
        N::Reference(idx) => N::Reference(idx + offset),
        N::Choice(nodes) => N::Choice(
            nodes
                .into_iter()
                .map(|n| shift_references(n, offset))
                .collect(),
        ),
        N::Sequence(nodes) => N::Sequence(
            nodes
                .into_iter()
                .map(|n| shift_references(n, offset))
                .collect(),
        ),
        n => n,
    }
}

fn normalize(
    node: GrammarNode,
    rules: &mut IndexSet<Rule>,
    current: usize,
) -> Result<(NormalizedNode, bool)> {
    use GrammarNode as G;
    use NormalizedNode as N;
    match node {
        G::Terminal(m) => Ok((N::Terminal(m), false)),
        G::Choice(choices) => choices
            .into_iter()
            .map(|n| normalize(n, rules, current))
            .collect::<Result<Vec<_>>>()
            .map(|results| {
                let (nodes, recursives): (Vec<_>, Vec<_>) = results.into_iter().unzip();
                (N::Choice(nodes), recursives.into_iter().any(|r| r))
            }),
        G::Sequence(seq) => seq
            .into_iter()
            .map(|n| normalize(n, rules, current))
            .collect::<Result<Vec<_>>>()
            .map(|results| {
                let (nodes, recursives): (Vec<_>, Vec<_>) = results.into_iter().unzip();
                (N::Sequence(nodes), recursives.into_iter().any(|r| r))
            }),
        G::Reference(f, name) => {
            let proto = Rule {
                name,
                node: N::Placeholder,
                is_recursive: false,
            };
            // If the rule is already being processed or defined
            if let Some(idx) = rules.get_index_of(&proto) {
                // Check if it's currently being normalized (still has Placeholder placeholder)
                let is_recursive = matches!(rules.get_index(idx).unwrap().node, N::Placeholder);
                Ok((N::Reference(idx), is_recursive))
            }
            // Otherwise, define the rule
            else {
                // Insert placeholder first to detect cycles
                let idx = rules.len();
                rules.insert(Rule {
                    name,
                    node: N::Placeholder,
                    is_recursive: false,
                });

                // Now normalize the rule body
                let (node, is_recursive) = normalize(f(), rules, current)?;

                // Replace the placeholder with the actual node
                let rule = rules.get_index_mut2(idx).unwrap();
                rule.node = node;
                rule.is_recursive = is_recursive;

                Ok((N::Reference(idx), is_recursive))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{r, words::State};

    #[test]
    fn test_normalize_terminal() {
        fn a() -> GrammarNode {
            t("a") + r!(b)
        }

        fn b() -> GrammarNode {
            t("b") + r!(c)
        }

        fn c() -> GrammarNode {
            t("c")
        }

        let grammar = Grammar::try_from(a()).unwrap();
        println!("{:#?}", grammar.rules);
    }
}
