pub(crate) mod analysis;
pub(crate) mod bridge;
pub(crate) mod cache;
pub mod display;
pub mod dsl;
pub(crate) mod ir;
pub(crate) mod norm;
pub(crate) mod recovery;

use std::fs;
use std::hash::{Hash, Hasher};
use std::io;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use rustc_hash::FxHashMap;

/// Static storage for cached grammars
/// Maps cache key to static reference of Grammar
static GRAMMAR_CACHE: OnceLock<Mutex<FxHashMap<u64, &'static Grammar>>> = OnceLock::new();

fn grammar_cache() -> &'static Mutex<FxHashMap<u64, &'static Grammar>> {
    GRAMMAR_CACHE.get_or_init(|| Mutex::new(FxHashMap::default()))
}

/// A macro to create a grammar rule reference in the DSL which name is the same as the function name.
/// For example, `r!(expr)` will create a reference to a rule named "expr".
#[macro_export]
macro_rules! r {
    ($fn:ident) => {
        $crate::grammar::dsl::r($fn, stringify!($fn))
    };
}

/// A macro that introduces an embedded DSL for defining grammars in more ergonomic way than using `Grammar::new` directly.
///
/// # Example
/// ```
/// let grammar = new_grammar! {
///     start where
///     start -> r!(expr) + t(EndOfInput)
///     expr -> r!(expr) "+" r!(term) | r!(term)
///     term -> t(NUMS) | "(" r!(expr) ")"
/// };
/// ```
///
/// is equivalent to:
///
/// ```
/// fn start() -> GrammarNode {
///     r!(expr) + t(EndOfInput)
/// }
///
/// fn expr() -> GrammarNode {
///     r!(expr) + r!(term) | r!(term)
/// }
///
/// fn term() -> GrammarNode {
///     t(NUMS) | "(" + r!(expr) + ")"
/// }
///
/// let grammar = Grammar::new(start(), "start");
/// ```
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

/// The grammar structure, which contains the normalized rule table and analyses for parsing and error recovery.
///
/// The grammar instance is always a static reference.
#[derive(Clone)]
pub struct Grammar {
    pub(crate) table: norm::RuleTable,
    pub(crate) analysis: Arc<analysis::GrammarStateAnalysis>,
    pub(crate) rule_analyses: FxHashMap<usize, Arc<analysis::GrammarStateAnalysis>>,
    /// Bracket pairs derived at grammar-analysis time (§3 of Nilsson-Nyman 2009).
    /// Used by the scope-recovery layer to skip malformed scope bodies.
    pub(crate) bridge_specs: Vec<bridge::BridgeSpec>,
    /// Delimiter terminals (e.g. `,`, `;`, `:`) derived at grammar-analysis time.
    /// Scope recovery may stop at these terminals to resume parsing earlier.
    pub(crate) recovery_delimiters: Vec<usize>,
}

impl Grammar {
    /// Create a new grammar from a DSL grammar node and a start rule name.
    ///
    /// The grammar is cached and returned as a static reference. If it has not changed since
    /// the last time it was created, it will be loaded from the cache instead of being reprocessed.
    pub fn new(node: dsl::GrammarNode, start_rule: &'static str) -> &'static Self {
        let cache_key = Self::cache_key_from_dsl(&node, start_rule);

        let cache = grammar_cache().lock().unwrap();
        if let Some(grammar) = cache.get(&cache_key) {
            return grammar;
        }
        drop(cache);

        // Check on-disk cache
        let grammar = if let Some(grammar) = cache::load(cache_key) {
            Box::leak(Box::new(grammar))
        } else {
            let grammar = Self::new_uncached(node, start_rule);
            let _ = cache::store(cache_key, grammar);
            grammar
        };

        grammar_cache().lock().unwrap().insert(cache_key, grammar);

        grammar
    }

    /// Create a new grammar without using the cache.
    pub fn new_uncached(node: dsl::GrammarNode, start_rule: &'static str) -> &'static Self {
        let table = norm::RuleTable::normalize(node, start_rule);
        let analysis = Arc::new(analysis::GrammarStateAnalysis::from_table(
            &table,
            table.start_rule,
        ));
        let rule_analyses = Self::build_rule_analyses(&table);
        let bridge_specs = bridge::derive_bridge_specs(&table);
        let recovery_delimiters = bridge::derive_recovery_delimiters(&table);

        let grammar = Self {
            table,
            analysis,
            rule_analyses,
            bridge_specs,
            recovery_delimiters,
        };

        Box::leak(Box::new(grammar))
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

    fn cache_key_from_dsl(node: &dsl::GrammarNode, start_rule: &'static str) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        start_rule.hash(&mut hasher);
        Self::hash_dsl_node(node, &mut hasher);

        hasher.finish()
    }

    fn hash_dsl_node(
        node: &dsl::GrammarNode,
        hasher: &mut std::collections::hash_map::DefaultHasher,
    ) {
        use dsl::GrammarNode;

        match node {
            GrammarNode::Terminal(matcher) => {
                0u8.hash(hasher);
                matcher.display().hash(hasher);
                matcher.preview().hash(hasher);
                matcher.is_nullable().hash(hasher);
                matcher.is_consuming().hash(hasher);
            }
            GrammarNode::Alternative(nodes) => {
                1u8.hash(hasher);
                nodes.len().hash(hasher);
                for child in nodes {
                    Self::hash_dsl_node(child, hasher);
                }
            }
            GrammarNode::Sequence(nodes) => {
                2u8.hash(hasher);
                nodes.len().hash(hasher);
                for child in nodes {
                    Self::hash_dsl_node(child, hasher);
                }
            }
            GrammarNode::Reference(_, name) => {
                3u8.hash(hasher);
                name.hash(hasher);
            }
            GrammarNode::Field(name, inner) => {
                4u8.hash(hasher);
                name.hash(hasher);
                Self::hash_dsl_node(inner, hasher);
            }
            GrammarNode::Drop { node, count } => {
                5u8.hash(hasher);
                count.hash(hasher);
                Self::hash_dsl_node(node, hasher);
            }
            GrammarNode::Repetition { node, min, max } => {
                6u8.hash(hasher);
                min.hash(hasher);
                max.hash(hasher);
                Self::hash_dsl_node(node, hasher);
            }
            GrammarNode::SeparatedRepetition {
                node,
                separator,
                min,
                max,
            } => {
                7u8.hash(hasher);
                min.hash(hasher);
                max.hash(hasher);
                Self::hash_dsl_node(node, hasher);
                Self::hash_dsl_node(separator, hasher);
            }
        }
    }

    /// Get the name of a rule by its index. If the rule has no name, return a generated name based on its index.
    pub fn name(&self, rule_idx: usize) -> &'static str {
        self.table
            .rules
            .get(rule_idx)
            .map(|r| r.name)
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| format!("@{}", rule_idx).leak())
    }

    /// Add metadata to a rule by its name.
    ///
    /// When the meta is a string, it will be used as the rule's description, which can be displayed in error messages.
    pub fn in_which(mut self, rule_name: &'static str, meta: impl RuleMeta) -> Self {
        if let Some(idx) = self.table.rules.iter().position(|r| r.name == rule_name) {
            meta.apply(&mut self, idx);
        } else {
            panic!("Rule '{}' not found in grammar", rule_name);
        }
        self
    }

    /// Load a grammar from a .gmx file.
    pub fn load_from(path: &Path) -> io::Result<&'static Self> {
        let bytes = fs::read(path)?;
        let grammar = cache::deserialize_grammar_file(&bytes)?;
        Ok(Box::leak(Box::new(grammar)))
    }

    /// Save this grammar to a .gmx file.
    pub fn save_to(&self, path: &Path) -> io::Result<()> {
        let bytes = cache::serialize_grammar_file(self)?;
        fs::create_dir_all(path.parent().unwrap_or_else(|| Path::new(".")))?;
        fs::write(path, bytes)
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
