pub(crate) mod analysis;
pub mod display;
pub mod dsl;
pub(crate) mod ir;
pub(crate) mod norm;
pub(crate) mod recovery;

use std::sync::Arc;

#[macro_export]
macro_rules! r {
    ($fn:ident) => {
        $crate::grammar::dsl::r($fn, stringify!($fn))
    };
}

#[macro_export]
macro_rules! new_grammar {
	($start: ident where $($name: ident -> $node: expr)*) => {
		{
			#[allow(unused_imports)]
			use $crate::{grammar::dsl::*, r};
			$(fn $name() -> GrammarNode { $node })*
			$crate::grammar::Grammar::new($start(), stringify!($start))
		}
	};
}

#[derive(Debug, Clone)]
pub enum GrammarError {
    InfiniteConsumption(usize),
}

#[derive(Debug, Clone)]
pub enum GrammarInfo {
    RecursionDetected(usize),
    DirectReference(usize),
}

#[derive(Debug, Clone)]
pub struct Grammar {
    pub(crate) table: norm::RuleTable,
    pub(crate) analysis: Arc<analysis::GrammarStateAnalysis>,
}

impl Grammar {
    pub fn new(node: dsl::GrammarNode, start_rule: &'static str) -> Self {
        let table = norm::RuleTable::normalize(node, start_rule);
        let analysis = Arc::new(analysis::GrammarStateAnalysis::from_table(
            &table,
            table.start_rule,
        ));

        Self { table, analysis }
    }

    pub fn name(&self, rule_idx: usize) -> &'static str {
        self.table
            .rules
            .get(rule_idx)
            .map(|r| r.name)
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| format!("@{}", rule_idx).leak())
    }

    pub fn in_which(mut self, rule_name: &'static str, meta: impl RuleMeta) -> Self {
        if let Some(idx) = self.table.rules.iter().position(|r| r.name == rule_name) {
            meta.apply(&mut self, idx);
        } else {
            panic!("Rule '{}' not found in grammar", rule_name);
        }
        self
    }
}

#[allow(private_bounds)]
pub trait RuleMeta: SealedRuleMeta {}

trait SealedRuleMeta {
    fn apply(&self, grammar: &mut Grammar, rule_idx: usize);
}

impl SealedRuleMeta for &'static str {
    fn apply(&self, grammar: &mut Grammar, rule_idx: usize) {
        if let Some(rule) = grammar.table.rules.get_mut(rule_idx) {
            rule.description = *self;
        }
    }
}

impl RuleMeta for &'static str {}
