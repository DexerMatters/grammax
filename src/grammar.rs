use core::fmt;
use std::hash;

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
    pub is_recursive: bool,
    pub is_nullable: bool,
}

impl Rule {
    pub fn new(
        name: &'static str,
        node: NormalizedNode,
        is_recursive: bool,
        is_nullable: bool,
    ) -> Self {
        Self {
            name,
            node,
            is_recursive,
            is_nullable,
        }
    }

    pub fn new_placeholder(name: &'static str) -> Self {
        Self {
            name,
            node: NormalizedNode::Placeholder,
            is_recursive: false,
            is_nullable: false,
        }
    }

    pub fn new_unnamed(node: NormalizedNode, is_recursive: bool, is_nullable: bool) -> Self {
        Self {
            name: "",
            node,
            is_recursive,
            is_nullable,
        }
    }
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

impl fmt::Display for Grammar {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, rule) in self.rules.iter().enumerate() {
            if i > 0 {
                writeln!(f)?;
            }
            write!(f, "{} ::= ", rule.name)?;
            display_node(&rule.node, f, &self.rules)?;
        }
        Ok(())
    }
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
            node: shift_references(start.node, 1),
            is_recursive: false,
            is_nullable: start.is_nullable,
        };

        let mut final_rules = IndexSet::new();
        final_rules.insert(start_rule);
        final_rules.extend(shifted_rules);

        // Fixpoint iteration for is_nullable
        loop {
            let mut updates = Vec::new();
            for (i, rule) in final_rules.iter().enumerate() {
                let null = calculate_nullable(&rule.node, &final_rules);
                if rule.is_nullable != null {
                    updates.push((i, null));
                }
            }
            if updates.is_empty() {
                break;
            }
            for (i, null) in updates {
                let rule = final_rules.get_index_mut2(i).unwrap();
                rule.is_nullable = null;
            }
        }

        Ok(Grammar { rules: final_rules })
    }
}

fn calculate_nullable(node: &NormalizedNode, rules: &IndexSet<Rule>) -> bool {
    use NormalizedNode as N;
    match node {
        N::Terminal(m) => m.is_nullable(),
        N::Placeholder => false,
        N::Reference(idx) => rules.get_index(*idx).map_or(false, |r| r.is_nullable),
        N::Choice(nodes) => nodes.iter().any(|n| calculate_nullable(n, rules)),
        N::Sequence(nodes) => nodes.iter().all(|n| calculate_nullable(n, rules)),
    }
}

fn display_node(
    node: &NormalizedNode,
    f: &mut fmt::Formatter<'_>,
    rules: &IndexSet<Rule>,
) -> fmt::Result {
    use NormalizedNode as N;
    match node {
        N::Terminal(m) => write!(f, "{}", m.display()),
        N::Placeholder => write!(f, "⊥"),
        N::Reference(idx) => {
            if let Some(rule) = rules.get_index(*idx) {
                write!(f, "{}", rule.name)
            } else {
                write!(f, "<invalid:{}>", idx)
            }
        }
        N::Choice(choices) => {
            for (i, choice) in choices.iter().enumerate() {
                if i > 0 {
                    write!(f, " | ")?;
                }
                display_node(choice, f, rules)?;
            }
            Ok(())
        }
        N::Sequence(seq) => {
            if seq.is_empty() {
                write!(f, "ε")
            } else {
                for (i, item) in seq.iter().enumerate() {
                    if i > 0 {
                        write!(f, " ")?;
                    }
                    // Wrap complex nodes in parentheses
                    match item {
                        N::Choice(_) => {
                            write!(f, "(")?;
                            display_node(item, f, rules)?;
                            write!(f, ")")?;
                        }
                        _ => display_node(item, f, rules)?,
                    }
                }
                Ok(())
            }
        }
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

fn normalize(node: GrammarNode, rules: &mut IndexSet<Rule>, current: usize) -> Result<Rule> {
    use GrammarNode as G;
    use NormalizedNode as N;
    match node {
        G::Terminal(m) => {
            let null = m.is_nullable();
            Ok(Rule::new_unnamed(N::Terminal(m), false, null))
        }
        G::Choice(choices) => {
            let mut nodes = Vec::new();
            let mut recursive = false;
            let mut nullable = false;

            for choice in choices {
                let r = normalize(choice, rules, current)?;
                nodes.push(r.node);
                recursive |= r.is_recursive;
                nullable |= r.is_nullable;
            }
            Ok(Rule::new_unnamed(N::Choice(nodes), recursive, nullable))
        }
        G::Sequence(seq) => {
            let mut nodes = Vec::new();
            let mut recursive = false;
            let mut nullable = true;

            for item in seq {
                let r = normalize(item, rules, current)?;
                nodes.push(r.node);
                recursive |= r.is_recursive;
                nullable &= r.is_nullable;
            }
            Ok(Rule::new_unnamed(N::Sequence(nodes), recursive, nullable))
        }
        G::Optional(opt) => {
            let r = normalize(*opt, rules, current)?;
            let node = N::Choice(vec![N::Sequence(vec![]), r.node]);
            Ok(Rule::new_unnamed(node, r.is_recursive, true))
        }
        G::Reference(f, name) => {
            let proto = Rule::new_placeholder(name);

            if let Some(idx) = rules.get_index_of(&proto) {
                let rule = rules.get_index(idx).unwrap();
                let is_recursive = matches!(rule.node, N::Placeholder);
                Ok(Rule::new_unnamed(
                    N::Reference(idx),
                    is_recursive,
                    rule.is_nullable,
                ))
            } else {
                let idx = rules.len();
                rules.insert(Rule::new_placeholder(name));

                let r = normalize(f(), rules, current)?;

                let rule = rules.get_index_mut2(idx).unwrap();
                rule.node = r.node;
                rule.is_recursive = r.is_recursive;
                rule.is_nullable = r.is_nullable;

                Ok(Rule::new_unnamed(
                    N::Reference(idx),
                    r.is_recursive,
                    r.is_nullable,
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::r;

    #[test]
    fn test_normalize_terminal() {
        fn a() -> GrammarNode {
            r!(b)
        }

        fn b() -> GrammarNode {
            r!(c)
        }

        fn c() -> GrammarNode {
            r!(a)
        }

        let grammar = Grammar::try_from(a()).unwrap();
        println!("\nBNF Form:");
        println!("{:#?}", grammar.rules);
    }
}
