use rustc_hash::FxHashMap;
use std::collections::HashMap;

use crate::grammar::dsl::GrammarNode;
use crate::grammar::expr_detect::ExpressionDetector;
use crate::grammar::ir::{NormalizedNode, OperatorInfo, Production, RuleInfo, Symbol};
use crate::parsec::words::MatcherRef;

/// Normalized grammar optimized for LR parsing
#[derive(Debug, Clone)]
pub struct RuleTable {
    pub rules: Vec<RuleInfo>,
    pub productions: Vec<Production>,
    pub terminals: Vec<MatcherRef>,
    pub terminal_map: FxHashMap<String, usize>, // terminal display -> index
    pub operator_tables: HashMap<usize, Vec<OperatorInfo>>, // rule_ix -> operators
    pub start_rule: usize,
}

impl RuleTable {
    /// Normalizes a grammar from DSL representation
    pub fn normalize(start: GrammarNode, start_name: &'static str) -> Self {
        let mut normalizer = Normalizer::new();

        // Phase 1: Discover all rules and desugar
        let user_start_ix = normalizer.discover_rule(start, start_name);
        normalizer.desugar_all();

        // Phase 2: Detect expression rules
        let (mut rules, operator_tables) = normalizer.detect_expressions();

        // Inject augmented start rule to ensure Accept is only on EOF
        // $root -> start
        let root_name = "$root";
        let root_node = NormalizedNode::Reference(user_start_ix);
        let root_rule = RuleInfo {
            name: root_name,
            description: root_name,
            node: root_node,
            is_expression: false,
        };
        rules.push(root_rule);
        let start_ix = rules.len() - 1;

        // Phase 3: Optimize for LR parsing
        let rules = Self::optimize_for_lr(rules);

        // Phase 4: Generate productions
        let (productions, terminals, terminal_map) = Self::generate_productions(&rules);

        RuleTable {
            rules,
            productions,
            terminals,
            terminal_map,
            operator_tables,
            start_rule: start_ix,
        }
    }

    /// Optimizes rules for LR parsing (prefix factoring, simplification)
    fn optimize_for_lr(mut rules: Vec<RuleInfo>) -> Vec<RuleInfo> {
        // Factor common prefixes for better LR state sharing
        for rule in &mut rules {
            if !rule.is_expression {
                // Don't optimize expression rules
                rule.node = Self::factor_prefixes(rule.node.clone());
            }
        }

        rules
    }

    /// Factors common prefixes in alternatives
    fn factor_prefixes(node: NormalizedNode) -> NormalizedNode {
        match node {
            NormalizedNode::Alternative(alts) => {
                // Group alternatives by first symbol
                let mut prefix_groups: FxHashMap<String, Vec<Vec<NormalizedNode>>> =
                    FxHashMap::default();

                for alt in alts {
                    match alt {
                        NormalizedNode::Sequence(mut parts) if !parts.is_empty() => {
                            let first = parts.remove(0);
                            let key = Self::node_key(&first);
                            prefix_groups.entry(key).or_default().push({
                                let mut v = vec![first];
                                v.extend(parts);
                                v
                            });
                        }
                        other => {
                            let key = Self::node_key(&other);
                            prefix_groups.entry(key).or_default().push(vec![other]);
                        }
                    }
                }

                // Rebuild alternatives with factored prefixes
                let mut new_alts = Vec::new();
                for (_, group) in prefix_groups {
                    if group.len() == 1 {
                        new_alts.push(NormalizedNode::Sequence(group[0].clone()));
                    } else {
                        // Extract common prefix
                        let prefix = group[0][0].clone();
                        let suffixes: Vec<_> = group
                            .into_iter()
                            .map(|mut parts| {
                                parts.remove(0); // Remove prefix
                                if parts.is_empty() {
                                    NormalizedNode::Sequence(vec![]) // Empty sequence for epsilon
                                } else if parts.len() == 1 {
                                    parts.into_iter().next().unwrap()
                                } else {
                                    NormalizedNode::Sequence(parts)
                                }
                            })
                            .collect();

                        let suffix_alt = if suffixes.len() == 1 {
                            suffixes.into_iter().next().unwrap()
                        } else {
                            NormalizedNode::Alternative(suffixes)
                        };

                        new_alts.push(NormalizedNode::Sequence(vec![prefix, suffix_alt]));
                    }
                }

                if new_alts.len() == 1 {
                    new_alts.into_iter().next().unwrap()
                } else {
                    NormalizedNode::Alternative(new_alts)
                }
            }
            other => other,
        }
    }

    /// Generates a key for grouping nodes by their first symbol
    fn node_key(node: &NormalizedNode) -> String {
        match node {
            NormalizedNode::Terminal(m) => m.display(),
            NormalizedNode::Reference(ix) => format!("ref_{}", ix),
            NormalizedNode::Field(name, inner) => {
                format!("field_{}_{}", name, Self::node_key(inner))
            }
            _ => "complex".to_string(),
        }
    }

    /// Generates LR productions from normalized rules
    fn generate_productions(
        rules: &[RuleInfo],
    ) -> (Vec<Production>, Vec<MatcherRef>, FxHashMap<String, usize>) {
        let mut productions = Vec::new();
        let mut terminals = Vec::new();
        let mut terminal_map = FxHashMap::default();

        for (lhs, rule) in rules.iter().enumerate() {
            Self::node_to_productions(
                lhs,
                &rule.node,
                &mut productions,
                &mut terminals,
                &mut terminal_map,
            );
        }

        (productions, terminals, terminal_map)
    }

    /// Converts a normalized node into productions
    fn node_to_productions(
        lhs: usize,
        node: &NormalizedNode,
        productions: &mut Vec<Production>,
        terminals: &mut Vec<MatcherRef>,
        terminal_map: &mut FxHashMap<String, usize>,
    ) {
        match node {
            NormalizedNode::Alternative(alts) => {
                for alt in alts {
                    Self::node_to_productions(lhs, alt, productions, terminals, terminal_map);
                }
            }
            NormalizedNode::Sequence(parts) => {
                let mut rhs = Vec::new();
                let mut field_positions = Vec::new();

                for (pos, part) in parts.iter().enumerate() {
                    if let Some((sym, field_name)) =
                        Self::part_to_symbol(part, terminals, terminal_map)
                    {
                        rhs.push(sym);
                        if let Some(name) = field_name {
                            field_positions.push((pos, name));
                        }
                    }
                }

                productions.push(Production {
                    lhs,
                    rhs,
                    field_positions,
                });
            }
            NormalizedNode::Terminal(m) => {
                let term_ix = Self::get_or_add_terminal(m.clone(), terminals, terminal_map);
                productions.push(Production {
                    lhs,
                    rhs: vec![Symbol::Terminal(term_ix)],
                    field_positions: vec![],
                });
            }
            NormalizedNode::Reference(ix) => {
                productions.push(Production {
                    lhs,
                    rhs: vec![Symbol::NonTerminal(*ix)],
                    field_positions: vec![],
                });
            }
            NormalizedNode::Field(name, inner) => {
                // Unwrap field and mark position
                let mut rhs = Vec::new();
                if let Some((sym, _)) = Self::part_to_symbol(inner, terminals, terminal_map) {
                    rhs.push(sym);
                }
                productions.push(Production {
                    lhs,
                    rhs,
                    field_positions: vec![(0, name)],
                });
            }
        }
    }

    /// Converts a node part to a symbol
    fn part_to_symbol(
        node: &NormalizedNode,
        terminals: &mut Vec<MatcherRef>,
        terminal_map: &mut FxHashMap<String, usize>,
    ) -> Option<(Symbol, Option<&'static str>)> {
        match node {
            NormalizedNode::Terminal(m) => {
                let ix = Self::get_or_add_terminal(m.clone(), terminals, terminal_map);
                Some((Symbol::Terminal(ix), None))
            }
            NormalizedNode::Reference(ix) => Some((Symbol::NonTerminal(*ix), None)),
            NormalizedNode::Field(name, inner) => {
                Self::part_to_symbol(inner, terminals, terminal_map)
                    .map(|(sym, _)| (sym, Some(*name)))
            }
            _ => None,
        }
    }

    fn get_or_add_terminal(
        matcher: MatcherRef,
        terminals: &mut Vec<MatcherRef>,
        terminal_map: &mut FxHashMap<String, usize>,
    ) -> usize {
        let key = matcher.display();
        if let Some(&ix) = terminal_map.get(&key) {
            ix
        } else {
            let ix = terminals.len();
            terminals.push(matcher);
            terminal_map.insert(key, ix);
            ix
        }
    }
}

/// Grammar normalizer
struct Normalizer {
    rules: Vec<RuleInfo>,
    rule_map: FxHashMap<String, usize>, // name -> index
}

impl Normalizer {
    fn new() -> Self {
        Self {
            rules: Vec::new(),
            rule_map: FxHashMap::default(),
        }
    }

    /// Discovers all rules recursively from the start rule
    fn discover_rule(&mut self, node: GrammarNode, name: &'static str) -> usize {
        // Check if already discovered
        if let Some(&ix) = self.rule_map.get(name) {
            return ix;
        }

        // Create placeholder
        let ix = self.rules.len();
        self.rule_map.insert(name.to_string(), ix);
        self.rules.push(RuleInfo {
            name,
            description: "",
            node: NormalizedNode::Reference(ix), // Temporary
            is_expression: false,
        });

        // Normalize the node
        let normalized = self.normalize_node(node);
        self.rules[ix].node = normalized;

        ix
    }

    /// Normalizes a DSL node
    fn normalize_node(&mut self, node: GrammarNode) -> NormalizedNode {
        match node {
            GrammarNode::Terminal(m) => NormalizedNode::Terminal(m),

            GrammarNode::Alternative(alts) => {
                let normalized: Vec<_> = alts
                    .into_iter()
                    .map(|alt| self.normalize_node(alt))
                    .collect();
                NormalizedNode::Alternative(normalized)
            }

            GrammarNode::Sequence(parts) => {
                let normalized: Vec<_> = parts
                    .into_iter()
                    .map(|part| self.normalize_node(part))
                    .collect();
                NormalizedNode::Sequence(normalized)
            }

            GrammarNode::Reference(f, name) => {
                let rule_node = f();
                let ix = self.discover_rule(rule_node, name);
                NormalizedNode::Reference(ix)
            }

            GrammarNode::Field(name, inner) => {
                let normalized = self.normalize_node(*inner);
                NormalizedNode::Field(name, Box::new(normalized))
            }

            // Desugar repetitions
            GrammarNode::Repetition { node, min, max } => self.desugar_repetition(*node, min, max),

            GrammarNode::SeparatedRepetition {
                node,
                separator,
                min,
                max,
            } => self.desugar_separated_repetition(*node, *separator, min, max),
        }
    }

    /// Desugars repetition into recursive rules
    fn desugar_repetition(
        &mut self,
        node: GrammarNode,
        min: usize,
        max: Option<usize>,
    ) -> NormalizedNode {
        let item = self.normalize_node(node);

        match (min, max) {
            (0, Some(1)) => {
                // Optional: A? becomes (A | ε)
                NormalizedNode::Alternative(vec![
                    item,
                    NormalizedNode::Sequence(vec![]), // Empty
                ])
            }
            (0, None) => {
                // Many: A* becomes A_list -> A A_list | ε
                // Create anonymous rule
                let list_ix = self.create_list_rule(item, false);
                NormalizedNode::Reference(list_ix)
            }
            (1, None) => {
                // Some: A+ becomes A_list -> A A_list | A
                let list_ix = self.create_list_rule(item, true);
                NormalizedNode::Reference(list_ix)
            }
            _ => {
                // General case: expand min times, then optional repetition
                let mut parts = vec![item.clone(); min];
                if max.map_or(true, |m| m > min) {
                    let remaining = max.map(|m| m - min);
                    let rest = self.desugar_repetition(self.denormalize_node(item), 0, remaining);
                    parts.push(rest);
                }
                if parts.len() == 1 {
                    parts.into_iter().next().unwrap()
                } else {
                    NormalizedNode::Sequence(parts)
                }
            }
        }
    }

    /// Desugars separated repetition
    fn desugar_separated_repetition(
        &mut self,
        node: GrammarNode,
        separator: GrammarNode,
        min: usize,
        _max: Option<usize>,
    ) -> NormalizedNode {
        let item = self.normalize_node(node);
        let sep = self.normalize_node(separator);

        if min == 0 {
            // sep(A, ",") -> A ("," A)* | ε
            let tail_ix = self.create_sep_tail_rule(sep, item.clone());
            NormalizedNode::Alternative(vec![
                NormalizedNode::Sequence(vec![item, NormalizedNode::Reference(tail_ix)]),
                NormalizedNode::Sequence(vec![]), // Empty
            ])
        } else {
            // sep1(A, ",") -> A ("," A)*
            let tail_ix = self.create_sep_tail_rule(sep, item.clone());
            NormalizedNode::Sequence(vec![item, NormalizedNode::Reference(tail_ix)])
        }
    }

    /// Creates a list rule for repetition
    fn create_list_rule(&mut self, item: NormalizedNode, non_empty: bool) -> usize {
        let ix = self.rules.len();
        let name = Box::leak(format!("@list_{}", ix).into_boxed_str());

        let node = if non_empty {
            // A_list -> A A_list | A
            NormalizedNode::Alternative(vec![
                NormalizedNode::Sequence(vec![item.clone(), NormalizedNode::Reference(ix)]),
                item,
            ])
        } else {
            // A_list -> A A_list | ε
            NormalizedNode::Alternative(vec![
                NormalizedNode::Sequence(vec![item, NormalizedNode::Reference(ix)]),
                NormalizedNode::Sequence(vec![]),
            ])
        };

        self.rules.push(RuleInfo {
            name,
            description: "",
            node,
            is_expression: false,
        });
        self.rule_map.insert(name.to_string(), ix);

        ix
    }

    /// Creates a separator tail rule
    fn create_sep_tail_rule(&mut self, sep: NormalizedNode, item: NormalizedNode) -> usize {
        let ix = self.rules.len();
        let name = Box::leak(format!("@sep_tail_{}", ix).into_boxed_str());

        // tail -> ("," A)*  becomes  tail -> "," A tail | ε
        let node = NormalizedNode::Alternative(vec![
            NormalizedNode::Sequence(vec![sep, item, NormalizedNode::Reference(ix)]),
            NormalizedNode::Sequence(vec![]),
        ]);

        self.rules.push(RuleInfo {
            name,
            description: "",
            node,
            is_expression: false,
        });
        self.rule_map.insert(name.to_string(), ix);

        ix
    }

    /// Placeholder for converting normalized back to DSL (for repetition desugaring)
    fn denormalize_node(&self, node: NormalizedNode) -> GrammarNode {
        match node {
            NormalizedNode::Terminal(m) => GrammarNode::Terminal(m),
            NormalizedNode::Alternative(alts) => GrammarNode::Alternative(
                alts.into_iter().map(|a| self.denormalize_node(a)).collect(),
            ),
            NormalizedNode::Sequence(parts) => GrammarNode::Sequence(
                parts
                    .into_iter()
                    .map(|p| self.denormalize_node(p))
                    .collect(),
            ),
            NormalizedNode::Reference(ix) => {
                let name = self.rules[ix].name;
                GrammarNode::Reference(|| unreachable!(), name)
            }
            NormalizedNode::Field(name, inner) => {
                GrammarNode::Field(name, Box::new(self.denormalize_node(*inner)))
            }
        }
    }

    fn desugar_all(&mut self) {
        // All desugaring happens during normalize_node
    }

    /// Detects expression rules using the expression detector
    fn detect_expressions(self) -> (Vec<RuleInfo>, HashMap<usize, Vec<OperatorInfo>>) {
        let detector = ExpressionDetector::new(self.rules);
        detector.detect_expressions()
    }
}
