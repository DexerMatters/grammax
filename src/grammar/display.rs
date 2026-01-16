use std::fmt;

use dashmap::DashSet;

use crate::{
    Grammar,
    grammar::{GrammarError, GrammarInfo, ir::NormalizedGrammarNode},
};

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
            NormalizedGrammarNode::Sequence(nodes) => {
                let parts: Vec<String> = nodes.iter().map(|n| n.to_string()).collect();
                write!(f, "({})", parts.join(" "))
            }
            NormalizedGrammarNode::Alternative(nodes) => {
                let parts: Vec<String> = nodes.iter().map(|n| n.to_string()).collect();
                write!(f, "({})", parts.join(" | "))
            }
            NormalizedGrammarNode::Field(name, inner) => {
                write!(f, "{}[{}]", name, inner)
            }
        }
    }
}
