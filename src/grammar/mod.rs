pub mod analysis;
pub mod display;
pub mod dsl;
pub(crate) mod ir;
mod norm;
pub mod recovery;

#[cfg(test)]
mod tests;

use std::{fmt, sync::Arc};

use dashmap::DashSet;

#[macro_export]
macro_rules! r {
    ($fn:ident) => {
        crate::grammar::dsl::r($fn, stringify!($fn))
    };
}

#[macro_export]
macro_rules! new_grammar {
    ($start: ident where $($name: ident -> $node: expr)*) => {
        {
            #[allow(unused_imports)]
            use crate::{grammar::dsl::*, r};
            $(fn $name() -> GrammarNode { $node })*
            crate::grammar::Grammar::new($start(), stringify!($start))
        }
    };
}

#[derive(Debug)]
pub enum GrammarError {
    InfiniteConsumption(usize),
}

#[derive(Debug)]
pub enum GrammarInfo {
    RecursionDetected(usize),
    DirectReference(usize),
}

#[derive(Debug)]
pub struct Grammar {
    pub(crate) table: norm::RuleTable,
    pub(crate) analysis: Arc<analysis::GrammarStateAnalysis>,
    errors: Vec<GrammarError>,
    infos: Vec<GrammarInfo>,
}

impl Grammar {
    pub fn new(node: dsl::GrammarNode, start_rule: &'static str) -> Self {
        let mut table = norm::RuleTable::new(vec![]);
        table.compute_from(node, start_rule);

        let analysis = Arc::new(analysis::GrammarStateAnalysis::from_table(&table, 0));
        Self {
            table,
            analysis,
            errors: vec![],
            infos: vec![],
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
