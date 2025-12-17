use core::fmt;
use std::{collections::HashSet, hash};

use indexmap::{IndexSet, set::MutableValues};

use crate::grammar_dsl::*;

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
}

impl TryFrom<GrammarNode> for Grammar {
    type Error = EvaluationError;
    fn try_from(node: GrammarNode) -> Result<Self> {
        let mut rules = IndexSet::new();
        let start = normalize(node, &mut rules)?;

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
            node: shift_references(start, 1),
        };

        let mut final_rules = IndexSet::new();
        final_rules.insert(start_rule);
        final_rules.extend(shifted_rules);

        Ok(Grammar { rules: final_rules })
    }
}

impl fmt::Display for Grammar {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use NormalizedNode as N;

        fn needs_paren(node: &NormalizedNode) -> bool {
            matches!(node, N::Choice(_))
        }

        fn fmt_node(
            grammar: &Grammar,
            node: &NormalizedNode,
            cur_idx: usize,
            f: &mut fmt::Formatter<'_>,
        ) -> fmt::Result {
            match node {
                N::Terminal(m) => write!(f, "{}", m.display()),
                N::Reference(idx) => {
                    let name = grammar
                        .rules
                        .get_index(*idx)
                        .map(|r| r.name)
                        .unwrap_or("<unknown>");
                    write!(f, "{}", name)
                }
                N::Placeholder => write!(f, "<placeholder>"),
                N::Sequence(parts) => {
                    let mut first = true;
                    for p in parts.iter() {
                        if !first {
                            write!(f, " ")?;
                        }
                        first = false;
                        if needs_paren(p) {
                            write!(f, "(")?;
                            fmt_node(grammar, p, cur_idx, f)?;
                            write!(f, ")")?;
                        } else {
                            fmt_node(grammar, p, cur_idx, f)?;
                        }
                    }
                    Ok(())
                }
                N::Choice(alts) => {
                    let mut first = true;
                    for a in alts.iter() {
                        if !first {
                            write!(f, " | ")?;
                        }
                        first = false;
                        if matches!(a, N::Sequence(_)) {
                            write!(f, "(")?;
                            fmt_node(grammar, a, cur_idx, f)?;
                            write!(f, ")")?;
                        } else {
                            fmt_node(grammar, a, cur_idx, f)?;
                        }
                    }
                    Ok(())
                }
            }
        }

        for (i, rule) in self.rules.iter().enumerate() {
            write!(f, "{} ::= ", rule.name)?;
            fmt_node(self, &rule.node, i, f)?;
            if i + 1 < self.rules.len() {
                writeln!(f)?;
            }
        }
        Ok(())
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

fn normalize(node: GrammarNode, rules: &mut IndexSet<Rule>) -> Result<NormalizedNode> {
    normalize_impl(node, rules, &mut HashSet::new())
}

fn normalize_impl(
    node: GrammarNode,
    rules: &mut IndexSet<Rule>,
    in_progress: &mut HashSet<&'static str>,
) -> Result<NormalizedNode> {
    use GrammarNode as G;
    use NormalizedNode as N;
    match node {
        G::Terminal(m) => Ok(N::Terminal(m)),
        G::Choice(choices) => choices
            .into_iter()
            .map(|n| normalize_impl(n, rules, in_progress))
            .collect::<Result<Vec<_>>>()
            .map(N::Choice),
        G::Sequence(seq) => seq
            .into_iter()
            .map(|n| normalize_impl(n, rules, in_progress))
            .collect::<Result<Vec<_>>>()
            .map(N::Sequence),
        G::Optional(opt) => Ok(N::Choice(vec![
            normalize_impl(*opt, rules, in_progress)?,
            N::null(),
        ])),
        G::Reference(f, name) => {
            let proto = Rule {
                name,
                node: N::Placeholder,
            };
            // If the rule is already defined, use the existing reference
            if let Some(idx) = rules.get_index_of(&proto) {
                Ok(N::Reference(idx))
            }
            // If the rule is currently being processed, we have a cycle - use placeholder
            else if in_progress.contains(name) {
                Ok(N::Reference(rules.len()))
            }
            // Otherwise, define the rule
            else {
                let idx = rules.len();
                rules.insert(Rule {
                    name,
                    node: N::Placeholder,
                });
                in_progress.insert(name);
                let node = normalize_impl(f(), rules, in_progress)?;
                in_progress.remove(name);
                // Update the placeholder rule with the actual normalized node
                if let Some(rule) = rules.get_index_mut2(idx) {
                    rule.node = node;
                }
                Ok(N::Reference(idx))
            }
        }
        _ => unimplemented!(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::r;

    #[test]
    fn test_normalize_terminal() {
        fn a() -> GrammarNode {
            t("a") + r!(b)
        }

        fn b() -> GrammarNode {
            t("b") + r!(c)
        }

        fn c() -> GrammarNode {
            t("c") + r!(a)
        }

        let grammar = Grammar::try_from(a()).unwrap();
        println!("{:#?}", grammar.rules);
    }
}
