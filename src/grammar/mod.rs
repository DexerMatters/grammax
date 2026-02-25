pub(crate) mod analysis;
pub mod display;
pub mod dsl;
pub(crate) mod ir;
pub(crate) mod norm;
pub(crate) mod recovery;

use std::sync::Arc;

use rustc_hash::FxHashMap;

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

#[derive(Clone)]
pub struct Grammar {
    pub(crate) table: norm::RuleTable,
    pub(crate) analysis: Arc<analysis::GrammarStateAnalysis>,
    /// Pre-warmed incremental LR analyses for every non-start rule.
    /// Built once at grammar construction time so no parse path ever pays
    /// LR-table-construction cost at runtime.
    pub(crate) rule_analyses: FxHashMap<usize, Arc<analysis::GrammarStateAnalysis>>,
}

impl Grammar {
    pub fn new(node: dsl::GrammarNode, start_rule: &'static str) -> Self {
        let table = norm::RuleTable::normalize(node, start_rule);
        let analysis = Arc::new(analysis::GrammarStateAnalysis::from_table(
            &table,
            table.start_rule,
        ));

        let rule_analyses = Self::build_rule_analyses(&table);

        Self {
            table,
            analysis,
            rule_analyses,
        }
    }

    /// Build incremental LR analyses for every non-start rule.
    /// Each rule needs a thin wrapper production so the LR automaton
    /// can treat it as an independent parse entry point.
    fn build_rule_analyses(
        table: &norm::RuleTable,
    ) -> FxHashMap<usize, Arc<analysis::GrammarStateAnalysis>> {
        use ir::{NormalizedNode, Production, RuleInfo, Symbol};

        let mut map = FxHashMap::default();
        for rule_ix in 0..table.rules.len() {
            if rule_ix == table.start_rule {
                continue;
            }
            let mut wrapped = table.clone();
            let wrapper_ix = wrapped.rules.len();
            wrapped.rules.push(RuleInfo {
                name: "$inc_root",
                description: "$inc_root",
                node: NormalizedNode::Reference(rule_ix),
                is_expression: false,
            });
            wrapped.productions.push(Production {
                lhs: wrapper_ix,
                rhs: vec![Symbol::NonTerminal(rule_ix)],
                field_positions: vec![],
            });
            wrapped.start_rule = wrapper_ix;
            let a = Arc::new(analysis::GrammarStateAnalysis::from_table(
                &wrapped, wrapper_ix,
            ));
            map.insert(rule_ix, a);
        }
        map
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
