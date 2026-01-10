use core::fmt;
use std::hash;

use indexmap::IndexSet;

use crate::{core::grammar::*, grammar_dsl::*};

#[derive(Debug, Clone)]
pub enum EvaluationError {
    UndecidableRule(String),
    AlwaysFails(String),
}

impl fmt::Display for EvaluationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EvaluationError::UndecidableRule(name) => {
                write!(
                    f,
                    "The rule '{}' is an infinite recursion without consuming input.",
                    name
                )
            }
            EvaluationError::AlwaysFails(name) => {
                write!(f, "The rule '{}' always fails to match input.", name)
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum GrammarError {
    Placeholder,
    RuleMismatch { expected: usize },
    TokenMismatch { expected: String },
}

pub type Result<T> = std::result::Result<T, Vec<EvaluationError>>;

#[derive(Debug)]
pub struct Rule {
    pub name: RuleName,
    pub node: NormalizedNode,

    pub properties: RuleProperties,
}

impl Rule {
    pub fn new(name: impl Into<RuleName>, node: NormalizedNode) -> Self {
        Self {
            name: name.into(),
            node,
            properties: DEFAULT_RULE_PROPS,
        }
    }

    pub fn placeholder(name: impl Into<RuleName>) -> Self {
        Self {
            name: name.into(),
            node: NormalizedNode {
                kind: NormalizedNodeKind::Placeholder,
                properties: DEFAULT_RULE_NODE_PROPS,
            },
            properties: DEFAULT_RULE_PROPS,
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

impl TryFrom<GrammarNode> for Grammar {
    type Error = Vec<EvaluationError>;
    fn try_from(node: GrammarNode) -> Result<Self> {
        let mut rules = IndexSet::new();
        let mut start = normalize(node, &mut rules, &mut Vec::new())?;

        // Pass 2: Update rule properties (is_recursive and is_trivial)
        update_rule_properties(&mut rules);

        // Pass 3: Inline trivial non-recursive rules
        start = inline_trivial_rules(&mut rules, start);

        // Shift all references by 1 to make room for START at index 0
        let shifted_rules: IndexSet<Rule> = rules
            .into_iter()
            .map(|mut rule| {
                rule.node = shift_references(rule.node, 1);
                rule
            })
            .collect();

        let start_rule = Rule::new("START", shift_references(start, 1));

        let mut final_rules = IndexSet::new();
        final_rules.insert(start_rule);
        final_rules.extend(shifted_rules);

        // START is inserted after the earlier passes, so analyze properties again
        // to populate its node properties (nullable/consuming/depths).
        update_more_rule_properties(&mut final_rules);

        // Diagnose grammar for errors
        let errors = diagnose_grammar(&final_rules);
        if errors.is_empty() {
            Ok(Self { rules: final_rules })
        } else {
            Err(errors)
        }
    }
}

impl fmt::Display for Grammar {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use NormalizedNodeKind as N;

        fn needs_paren(node: &NormalizedNode) -> bool {
            matches!(node.kind, N::Choice(_))
        }

        fn fmt_node(
            grammar: &Grammar,
            node: &NormalizedNode,
            cur_idx: usize,
            f: &mut fmt::Formatter<'_>,
        ) -> fmt::Result {
            match &node.kind {
                N::Terminal(m) => write!(f, "{}", m.display()),
                N::Reference(idx) => match grammar.rules.get_index(*idx) {
                    Some(r) => write!(f, "{}", r.name),
                    None => write!(f, "<invalid-ref-{}>", idx),
                },
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
                        if matches!(a.kind, N::Sequence(_)) {
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
    use NormalizedNodeKind as N;
    let NormalizedNode { kind, properties } = node;
    let new_kind = match kind {
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
        other => other,
    };
    NormalizedNode {
        kind: new_kind,
        properties,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::r;

    #[test]
    fn test_normalize_terminal() {
        fn term() -> GrammarNode {
            t("a")
        }

        fn expr() -> GrammarNode {
            (r!(expr) + t("+") + r!(term)) | r!(term)
        }

        let grammar = Grammar::try_from(r!(expr));

        match grammar {
            Err(errors) => {
                println!("Grammar has errors:");
                errors.iter().for_each(|e| println!(" - {}", e));
            }
            Ok(grammar) => {
                println!("Number of rules: {}", grammar.rules.len());
                println!("Grammar:\n{}", grammar);
                for (idx, rule) in grammar.rules.iter().enumerate() {
                    println!("Rule {}: {:#?}", idx, rule);
                }
            }
        }
    }
}
