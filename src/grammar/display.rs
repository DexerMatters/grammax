use std::fmt;

use dashmap::DashSet;

use crate::{
    Grammar,
    grammar::{
        GrammarError, GrammarInfo,
        ir::{NormalizedGrammarNode, State},
        norm::RuleTable,
    },
};

impl RuleTable {
    fn get_rule_name(&self, idx: usize) -> String {
        self.rule_names
            .get(idx)
            .filter(|n| !n.is_empty())
            .map(|n| n.to_string())
            .unwrap_or_else(|| format!("@{}", idx))
    }
    fn mark_used(&self, node: &NormalizedGrammarNode, used: &mut Vec<bool>) {
        match node {
            NormalizedGrammarNode::Terminal(_) => {}
            NormalizedGrammarNode::Reference(idx) => {
                if *idx < used.len() && !used[*idx] {
                    used[*idx] = true;
                    self.mark_used(&self.rules[*idx], used);
                }
            }
            NormalizedGrammarNode::Field(_, inner) => {
                self.mark_used(inner, used);
            }
            NormalizedGrammarNode::Sequence(nodes) | NormalizedGrammarNode::Alternative(nodes) => {
                for n in nodes {
                    self.mark_used(n, used);
                }
            }
        }
    }

    fn format_node(&self, node: &NormalizedGrammarNode) -> String {
        self.format_node_inner(node, false)
    }

    fn format_node_inner(&self, node: &NormalizedGrammarNode, parent_is_seq: bool) -> String {
        const RESET: &str = "\x1b[0m";
        const BOLD: &str = "\x1b[1m";
        const GREY: &str = "\x1b[90m";

        match node {
            NormalizedGrammarNode::Terminal(matcher) => {
                format!("{}{}{}", GREY, matcher.display(), RESET)
            }
            NormalizedGrammarNode::Reference(index) => {
                format!("{}{}{}", BOLD, self.get_rule_name(*index), RESET)
            }
            NormalizedGrammarNode::Field(name, inner) => {
                format!("{}:{}", name, self.format_node_inner(inner, false))
            }
            NormalizedGrammarNode::Sequence(nodes) => {
                let parts: Vec<String> = nodes
                    .iter()
                    .map(|n| self.format_node_inner(n, true))
                    .collect();
                let content = parts.join(" ");
                // Only wrap in parens if inside an Alternative
                if parent_is_seq {
                    content
                } else {
                    format!("({})", content)
                }
            }
            NormalizedGrammarNode::Alternative(nodes) => {
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

        // Find which rules are actually referenced
        let mut used = vec![false; self.rules.len()];
        for (i, _) in self.rule_names.iter().enumerate() {
            if !self.rule_names[i].is_empty() {
                used[i] = true;
                self.mark_used(&self.rules[i], &mut used);
            }
        }

        // Calculate max name width for alignment (only for rules that will be shown)
        let max_width = self
            .rule_names
            .iter()
            .enumerate()
            .filter(|(i, n)| !n.is_empty() || used[*i])
            .map(|(i, n)| {
                if n.is_empty() {
                    format!("@{}", i).len()
                } else {
                    n.len()
                }
            })
            .max()
            .unwrap_or(10);

        for (i, rule) in self.rules.iter().enumerate() {
            // Skip unused anonymous rules
            if i < self.rule_names.len() && self.rule_names[i].is_empty() && !used[i] {
                continue;
            }

            let name = if i < self.rule_names.len() && !self.rule_names[i].is_empty() {
                self.rule_names[i].to_string()
            } else {
                format!("@{}", i)
            };

            let formatted_rule = self.format_node(rule);
            let padded_name = format!("{:<width$}", name, width = max_width);
            writeln!(
                f,
                "  {}{}{} {} {}",
                BOLD, padded_name, RESET, "→", formatted_rule
            )?;
        }
        Ok(())
    }
}

impl fmt::Display for Grammar {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        const RESET: &str = "\x1b[0m";
        const BOLD: &str = "\x1b[1m";
        const GREEN: &str = "\x1b[32m";
        const YELLOW: &str = "\x1b[33m";
        const RED: &str = "\x1b[31m";
        const GRAY: &str = "\x1b[90m";

        write!(f, "{}", self.table)?;

        // Collect unique infos and errors to avoid duplicates
        let unique_recursions = {
            let seen = DashSet::new();
            self.infos
                .iter()
                .filter_map(|info| match info {
                    GrammarInfo::RecursionDetected(idx) => {
                        if seen.insert(idx) {
                            Some(idx)
                        } else {
                            None
                        }
                    }
                    _ => None,
                })
                .collect::<Vec<_>>()
        };

        let unique_relays = {
            let seen = DashSet::new();
            self.infos
                .iter()
                .filter_map(|info| match info {
                    GrammarInfo::DirectReference(idx) => {
                        if seen.insert(idx) {
                            Some(idx)
                        } else {
                            None
                        }
                    }
                    _ => None,
                })
                .collect::<Vec<_>>()
        };

        // Display warnings
        if !unique_recursions.is_empty() || !unique_relays.is_empty() {
            writeln!(f)?;
            writeln!(f, "{}{}{} Warnings {}{}", YELLOW, BOLD, "━", RESET, GRAY)?;
            for idx in unique_recursions {
                writeln!(
                    f,
                    "  {} ⚠  {} Rule {}{}{} has recursion",
                    YELLOW,
                    RESET,
                    BOLD,
                    self.name(*idx),
                    RESET,
                )?;
            }
            for idx in unique_relays {
                writeln!(
                    f,
                    "  {} ⚠  {} Rule {}{}{} is a relay reference",
                    YELLOW,
                    RESET,
                    BOLD,
                    self.name(*idx),
                    RESET,
                )?;
            }
        }

        // Display errors
        if !self.errors.is_empty() {
            writeln!(f)?;
            writeln!(f, "{}{}{} Errors {}{}", RED, BOLD, "━", RESET, GRAY)?;
            for error in &self.errors {
                match error {
                    GrammarError::InfiniteConsumption(idx) => {
                        writeln!(
                            f,
                            "  {} ✗  {} Rule {}{}{} has infinite consumption",
                            RED,
                            RESET,
                            BOLD,
                            self.name(*idx),
                            RESET,
                        )?;
                    }
                }
            }
        }

        // Summary
        writeln!(f)?;
        if self.errors.is_empty() && self.infos.is_empty() {
            writeln!(
                f,
                "{}{}{} ✓ Grammar is valid {}{}{}",
                GREEN, BOLD, "━", RESET, GREEN, RESET
            )?;
        }

        Ok(())
    }
}

impl fmt::Display for NormalizedGrammarNode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NormalizedGrammarNode::Terminal(matcher) => {
                write!(f, "{}", matcher.display())
            }
            NormalizedGrammarNode::Reference(index) => {
                write!(f, "@{}", index)
            }
            NormalizedGrammarNode::Field(name, node) => {
                write!(f, "{}:{}", name, node)
            }
            NormalizedGrammarNode::Sequence(nodes) => {
                let parts: Vec<String> = nodes.iter().map(|n| n.to_string()).collect();
                write!(f, "({})", parts.join(" "))
            }
            NormalizedGrammarNode::Alternative(nodes) => {
                let parts: Vec<String> = nodes.iter().map(|n| n.to_string()).collect();
                write!(f, "({})", parts.join(" | "))
            }
        }
    }
}
