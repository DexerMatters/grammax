use std::fmt;
use crate::grammar::norm::RuleTable;
use crate::grammar::ir::{NormalizedNode, Production, Symbol};

impl RuleTable {
    fn get_rule_name(&self, idx: usize) -> String {
        self.rules
            .get(idx)
            .and_then(|r| Some(r.name))
            .filter(|n| !n.is_empty())
            .map(|n| n.to_string())
            .unwrap_or_else(|| format!("@{}", idx))
    }

    fn mark_used(&self, node: &NormalizedNode, used: &mut Vec<bool>) {
        match node {
            NormalizedNode::Terminal(_) => {}
            NormalizedNode::Reference(idx) => {
                if *idx < used.len() && !used[*idx] {
                    used[*idx] = true;
                    if let Some(rule) = self.rules.get(*idx) {
                        self.mark_used(&rule.node, used);
                    }
                }
            }
            NormalizedNode::Field(_, inner) => {
                self.mark_used(inner, used);
            }
            NormalizedNode::Sequence(nodes) | NormalizedNode::Alternative(nodes) => {
                for n in nodes {
                    self.mark_used(n, used);
                }
            }
        }
    }

    fn format_node(&self, node: &NormalizedNode) -> String {
        self.format_node_inner(node, false)
    }

    fn format_node_inner(&self, node: &NormalizedNode, parent_is_seq: bool) -> String {
        const RESET: &str = "\x1b[0m";
        const BOLD: &str = "\x1b[1m";
        const GREY: &str = "\x1b[90m";

        match node {
            NormalizedNode::Terminal(matcher) => {
                format!("{}{}{}", GREY, matcher.display(), RESET)
            }
            NormalizedNode::Reference(index) => {
                format!("{}{}{}", BOLD, self.get_rule_name(*index), RESET)
            }
            NormalizedNode::Field(name, inner) => {
                format!("{}:{}", name, self.format_node_inner(inner, false))
            }
            NormalizedNode::Sequence(nodes) => {
                let parts: Vec<String> = nodes
                    .iter()
                    .map(|n| self.format_node_inner(n, true))
                    .collect();
                let content = parts.join(" ");
                if parent_is_seq {
                    content
                } else {
                    format!("({})", content)
                }
            }
            NormalizedNode::Alternative(nodes) => {
                let parts: Vec<String> = nodes
                    .iter()
                    .map(|n| self.format_node_inner(n, false))
                    .collect();
                parts.join(" | ")
            }
        }
    }
}

impl fmt::Display for RuleTable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        const RESET: &str = "\x1b[0m";
        const BOLD: &str = "\x1b[1m";

        let mut used = vec![false; self.rules.len()];
        for (i, rule) in self.rules.iter().enumerate() {
            if !rule.name.is_empty() {
                used[i] = true;
                self.mark_used(&rule.node, &mut used);
            }
        }

        let max_width = self
            .rules
            .iter()
            .enumerate()
            .filter(|(i, r)| !r.name.is_empty() || used[*i])
            .map(|(_, r)| r.name.len())
            .max()
            .unwrap_or(0);

        for (i, rule) in self.rules.iter().enumerate() {
            if !rule.name.is_empty() || used[i] {
                let name = if rule.name.is_empty() {
                    format!("@{}", i)
                } else {
                    rule.name.to_string()
                };
                
                writeln!(
                    f,
                    "{}{:<width$}{} → {}",
                    BOLD,
                    name,
                    RESET,
                    self.format_node(&rule.node),
                    width = max_width
                )?;
            }
        }

        Ok(())
    }
}

impl fmt::Display for Production {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} →", self.lhs)?;
        for symbol in &self.rhs {
            write!(f, " {}", symbol)?;
        }
        Ok(())
    }
}

impl fmt::Display for Symbol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Symbol::Terminal(idx) => write!(f, "T{}", idx),
            Symbol::NonTerminal(idx) => write!(f, "N{}", idx),
        }
    }
}
