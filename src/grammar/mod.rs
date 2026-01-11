mod analysis;
pub mod dsl;
mod norm;

#[cfg(test)]
mod tests;

use std::fmt;

#[macro_export]
macro_rules! r {
    ($fn:ident) => {
        crate::grammar::dsl::r($fn, stringify!($fn))
    };
}

pub enum GrammarError {
    InfiniteConsumption(usize),
}

pub enum GrammarInfo {
    RecursionDetected(usize),
    DirectReference(usize),
}

pub struct Grammar {
    pub(crate) analysis: analysis::GrammarGraphAnalysis,
    table: norm::RuleTable,
    errors: Vec<GrammarError>,
    infos: Vec<GrammarInfo>,
}

impl Grammar {
    pub fn new(node: dsl::GrammarNode, start_rule: &'static str) -> Self {
        let mut table = norm::RuleTable::new(vec![]);
        table.compute_from(node, start_rule);

        let analysis = analysis::GrammarGraphAnalysis::from_table(&table, 0);
        let infinite_consumptions = analysis
            .infinite_states()
            .iter()
            .map(|i| GrammarError::InfiniteConsumption(analysis.states[*i].ref_ix()))
            .collect();
        let recursions: Vec<_> = analysis
            .recursive_states()
            .iter()
            .map(|i| GrammarInfo::RecursionDetected(analysis.states[*i].ref_ix()))
            .collect();
        let direct_refs: Vec<_> = (0..table.rules.len())
            .filter(|&i| !analysis.rule_set().contains(&i))
            .map(|i| GrammarInfo::DirectReference(i))
            .collect();

        Self {
            analysis,
            table,
            errors: infinite_consumptions,
            infos: recursions.into_iter().chain(direct_refs).collect(),
        }
    }
    pub fn name(&self, rule_idx: usize) -> &'static str {
        if self.table.rule_names[rule_idx].is_empty() {
            format!("@{}", rule_idx).leak()
        } else {
            self.table.rule_names[rule_idx]
        }
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
            let mut seen = std::collections::HashSet::new();
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
            let mut seen = std::collections::HashSet::new();
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
                "{}{}{} ✓ Grammar is valid {}{}",
                GREEN, BOLD, "━", RESET, GREEN
            )?;
        }

        Ok(())
    }
}
