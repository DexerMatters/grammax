use indexmap::{IndexSet, set::MutableValues};
use std::collections::{HashMap, HashSet};

use crate::{core::grammar::analysis::*, grammar::*, grammar_dsl::*};

mod analysis;
mod diagnosis;
mod threading;

pub use analysis::analyze_node_properties;

// Pass 1: Normalize the grammar structure
pub fn normalize(
    node: GrammarNode,
    rules: &mut IndexSet<Rule>,
    in_progress: &mut Vec<RuleName>,
) -> Result<NormalizedNode> {
    use GrammarNode as G;
    use NormalizedNodeKind as N;
    let props = DEFAULT_RULE_NODE_PROPS;
    match node {
        G::Terminal(m) => Ok(NormalizedNode {
            kind: N::Terminal(m),
            properties: props,
        }),
        G::Choice(choices) => {
            let choice_nodes = choices
                .into_iter()
                .map(|n| normalize(n, rules, in_progress))
                .collect::<Result<Vec<_>>>()?;
            Ok(NormalizedNode {
                kind: N::Choice(choice_nodes),
                properties: props,
            })
        }
        G::Sequence(seq) => {
            let seq_nodes = seq
                .into_iter()
                .map(|n| normalize(n, rules, in_progress))
                .collect::<Result<Vec<_>>>()?;
            Ok(NormalizedNode {
                kind: N::Sequence(seq_nodes),
                properties: props,
            })
        }
        G::Optional(opt) => {
            let normalized = normalize(*opt, rules, in_progress)?;

            // Wrap in a helper rule if not already a reference
            let inner_ref = if let N::Reference(idx) = normalized.kind {
                idx
            } else {
                let mut inner_rule_name = in_progress
                    .last()
                    .cloned()
                    .unwrap_or(RuleName::new("__anonymous__"));
                inner_rule_name.meta += 1;

                let inner_idx = rules.len();
                rules.insert(Rule {
                    name: inner_rule_name,
                    node: normalized,
                    properties: DEFAULT_RULE_PROPS,
                });
                inner_idx
            };

            Ok(NormalizedNode {
                kind: N::Choice(vec![
                    NormalizedNode {
                        kind: N::Reference(inner_ref),
                        properties: props.clone(),
                    },
                    NormalizedNode {
                        kind: NormalizedNodeKind::Sequence(vec![]),
                        properties: props.clone(),
                    },
                ]),
                properties: props,
            })
        }
        G::Reference(f, name) => {
            let name = RuleName::new(name);
            normalize(G::_Reference(f, name), rules, in_progress)
        }
        G::_Reference(f, name) => {
            let proto = Rule::placeholder(name.clone());
            // If the rule is already defined, use the existing reference
            if let Some(idx) = rules.get_index_of(&proto) {
                Ok(NormalizedNode {
                    kind: N::Reference(idx),
                    properties: props,
                })
            }
            // If the rule is currently being processed, we have a cycle
            else if in_progress.contains(&name) {
                // Return a forward reference - the rule will be at this index
                Ok(NormalizedNode {
                    kind: N::Reference(rules.len()),
                    properties: props,
                })
            }
            // Otherwise, define the rule
            else {
                let idx = rules.len();
                rules.insert(Rule {
                    name: name.clone().into(),
                    node: NormalizedNode {
                        kind: NormalizedNodeKind::Placeholder,
                        properties: DEFAULT_RULE_NODE_PROPS,
                    },
                    properties: DEFAULT_RULE_PROPS,
                });
                in_progress.push(name);
                let node = normalize(f(), rules, in_progress)?;
                in_progress.pop();
                // Update the placeholder rule with the actual normalized node
                if let Some(rule) = rules.get_index_mut2(idx) {
                    rule.node = node;
                }
                Ok(NormalizedNode {
                    kind: N::Reference(idx),
                    properties: props,
                })
            }
        }
        G::Some(node) => {
            // Some (one or more): inner | (inner some)
            let normalized = normalize(*node, rules, in_progress)?;

            // Ensure the normalized node is a reference so we can use it multiple times
            let inner_ref = if let N::Reference(idx) = normalized.kind {
                idx
            } else {
                // Create a helper rule for the inner node
                let mut inner_rule_name = in_progress
                    .last()
                    .cloned()
                    .unwrap_or(RuleName::new("__anonymous__"));
                inner_rule_name.meta += 1;

                let inner_idx = rules.len();
                rules.insert(Rule {
                    name: inner_rule_name,
                    node: normalized,
                    properties: DEFAULT_RULE_PROPS,
                });
                inner_idx
            };

            // Create the Some rule: inner | (inner some)
            let mut some_rule_name = in_progress
                .last()
                .cloned()
                .unwrap_or(RuleName::new("__anonymous__"));
            some_rule_name.meta += 2; // Use +2 to avoid collision with inner rule (+1)

            let some_idx = rules.len();
            // Placeholder to allow recursion
            rules.insert(Rule {
                name: some_rule_name.clone(),
                node: NormalizedNode {
                    kind: NormalizedNodeKind::Placeholder,
                    properties: DEFAULT_RULE_NODE_PROPS,
                },
                properties: DEFAULT_RULE_PROPS,
            });

            let some_node = NormalizedNode {
                kind: N::Choice(vec![
                    NormalizedNode {
                        kind: N::Reference(inner_ref),
                        properties: props.clone(),
                    },
                    NormalizedNode {
                        kind: N::Sequence(vec![
                            NormalizedNode {
                                kind: N::Reference(inner_ref),
                                properties: props.clone(),
                            },
                            NormalizedNode {
                                kind: N::Reference(some_idx),
                                properties: props.clone(),
                            },
                        ]),
                        properties: props.clone(),
                    },
                ]),
                properties: props.clone(),
            };

            if let Some(rule) = rules.get_index_mut2(some_idx) {
                rule.node = some_node;
            }

            Ok(NormalizedNode {
                kind: N::Reference(some_idx),
                properties: props,
            })
        }
        G::Many(node) => {
            // Many (zero or more): ε | (inner many)
            let normalized = normalize(*node, rules, in_progress)?;

            // Ensure the normalized node is a reference so we can use it multiple times
            let inner_ref = if let N::Reference(idx) = normalized.kind {
                idx
            } else {
                // Create a helper rule for the inner node
                let mut inner_rule_name = in_progress
                    .last()
                    .cloned()
                    .unwrap_or(RuleName::new("__anonymous__"));
                inner_rule_name.meta += 1;

                let inner_idx = rules.len();
                rules.insert(Rule {
                    name: inner_rule_name,
                    node: normalized,
                    properties: DEFAULT_RULE_PROPS,
                });
                inner_idx
            };

            // Create the Many rule: ε | (inner many)
            let mut many_rule_name = in_progress
                .last()
                .cloned()
                .unwrap_or(RuleName::new("__anonymous__"));
            many_rule_name.meta += 2; // Use +2 to avoid collision with inner rule (+1)

            let many_idx = rules.len();
            // Placeholder to allow recursion
            rules.insert(Rule {
                name: many_rule_name.clone(),
                node: NormalizedNode {
                    kind: NormalizedNodeKind::Placeholder,
                    properties: DEFAULT_RULE_NODE_PROPS,
                },
                properties: DEFAULT_RULE_PROPS,
            });

            let many_node = NormalizedNode {
                kind: N::Choice(vec![
                    // Empty sequence (ε) - allows zero matches
                    NormalizedNode {
                        kind: N::Sequence(vec![]),
                        properties: props.clone(),
                    },
                    NormalizedNode {
                        kind: N::Sequence(vec![
                            NormalizedNode {
                                kind: N::Reference(inner_ref),
                                properties: props.clone(),
                            },
                            NormalizedNode {
                                kind: N::Reference(many_idx),
                                properties: props.clone(),
                            },
                        ]),
                        properties: props.clone(),
                    },
                ]),
                properties: props.clone(),
            };

            if let Some(rule) = rules.get_index_mut2(many_idx) {
                rule.node = many_node;
            }

            Ok(NormalizedNode {
                kind: N::Reference(many_idx),
                properties: props,
            })
        }
    }
}

pub fn diagnose_grammar(rules: &IndexSet<Rule>) -> Vec<EvaluationError> {
    let mut errors = Vec::new();
    for rule in rules.iter() {
        if let Some(err) = diagnosis::diagnose_rule(rule) {
            errors.push(err);
        }
    }
    errors
}

// Pass 2: Update rule properties (is_recursive and is_trivial)
pub fn update_rule_properties(rules: &mut IndexSet<Rule>) {
    // Mark trivial rules (by name or single reference)
    for idx in 0..rules.len() {
        let Some(rule) = rules.get_index_mut2(idx) else {
            continue;
        };
        let is_name_trivial = rule.name.is_trivial();
        let is_single_ref = matches!(&rule.node.kind, NormalizedNodeKind::Reference(_));
        rule.properties.is_trivial = is_name_trivial || is_single_ref;
    }

    // Detect recursive rules
    for idx in 0..rules.len() {
        if is_rule_recursive(idx, rules) {
            let Some(rule) = rules.get_index_mut2(idx) else {
                continue;
            };
            rule.properties.is_recursive = true;
        }
    }
}

// Pass 3: Inline trivial non-recursive rules, and collapse recursive trivial rules into wrappers
pub fn inline_trivial_rules(rules: &mut IndexSet<Rule>, start: NormalizedNode) -> NormalizedNode {
    use std::collections::HashMap;

    // Collapse wrappers: non-trivial rules wrapping recursive trivial rules
    let mut collapsed = Vec::new();
    for idx in 0..rules.len() {
        let Some(rule) = rules.get_index(idx) else {
            continue;
        };
        let NormalizedNodeKind::Reference(target_idx) = rule.node.kind else {
            continue;
        };
        if rule.name.is_trivial() {
            continue;
        }

        let Some(target) = rules.get_index(target_idx) else {
            continue;
        };
        if !target.name.is_trivial() || !target.properties.is_recursive {
            continue;
        }

        let inlined = redirect_references(&target.node, target_idx, idx);
        let Some(rule_mut) = rules.get_index_mut2(idx) else {
            continue;
        };
        rule_mut.node = inlined;
        rule_mut.properties.is_recursive = true;
        collapsed.push(target_idx);
    }

    // Mark collapsed rules for removal
    for &idx in &collapsed {
        let Some(rule) = rules.get_index_mut2(idx) else {
            continue;
        };
        rule.properties.is_recursive = false;
    }

    // Identify rules to inline (trivial and non-recursive)
    let to_inline: Vec<usize> = (0..rules.len())
        .filter(|&idx| {
            rules
                .get_index(idx)
                .map(|r| r.properties.is_trivial && !r.properties.is_recursive)
                .unwrap_or(false)
        })
        .collect();

    if to_inline.is_empty() {
        return start;
    }

    // Build inline map from rules to their nodes
    let inline_map: HashMap<usize, NormalizedNode> = to_inline
        .iter()
        .filter_map(|&idx| rules.get_index(idx).map(|r| (idx, r.node.clone())))
        .collect();

    // Inline in all non-inlined rules
    for idx in 0..rules.len() {
        if to_inline.contains(&idx) {
            continue;
        }
        let Some(rule) = rules.get_index(idx) else {
            continue;
        };
        let inlined = inline_references(&rule.node, &inline_map);
        let Some(rule_mut) = rules.get_index_mut2(idx) else {
            continue;
        };
        rule_mut.node = inlined;
    }

    // Inline in start node
    let start = inline_references(&start, &inline_map);

    // Remove inlined rules in reverse order
    let mut sorted = to_inline.clone();
    sorted.sort_by(|a, b| b.cmp(a));
    for &idx in &sorted {
        rules.shift_remove_index(idx);
    }

    // Build index remapping
    let index_map: HashMap<usize, usize> = (0..rules.len() + to_inline.len())
        .scan(0, |new_idx, old_idx| {
            if !to_inline.contains(&old_idx) {
                let result = *new_idx;
                *new_idx += 1;
                Some((old_idx, result))
            } else {
                Some((old_idx, usize::MAX))
            }
        })
        .filter(|(_, new)| *new != usize::MAX)
        .collect();

    // Update refs in remaining rules
    for idx in 0..rules.len() {
        let Some(rule) = rules.get_index(idx) else {
            continue;
        };
        let updated = update_references_after_removal(&rule.node, &index_map);
        let Some(rule_mut) = rules.get_index_mut2(idx) else {
            continue;
        };
        rule_mut.node = updated;
    }

    update_references_after_removal(&start, &index_map)
}

// Helper: redirect references from old_idx to new_idx (for collapsing wrapper rules)
fn redirect_references(node: &NormalizedNode, old_idx: usize, new_idx: usize) -> NormalizedNode {
    NormalizedNodeWalker::map(node, |n| {
        if let NormalizedNodeKind::Reference(idx) = n.kind {
            if idx == old_idx {
                return NormalizedNode {
                    kind: NormalizedNodeKind::Reference(new_idx),
                    properties: n.properties,
                };
            }
        }
        n
    })
}

// Pass 4: Analyze node properties (is_nullable and is_consuming)
pub fn update_more_rule_properties(rules: &mut IndexSet<Rule>) {
    analyze_node_properties(rules);
}

// Helper: inline references to trivial rules with cycle detection
fn inline_references(
    node: &NormalizedNode,
    inline_map: &HashMap<usize, NormalizedNode>,
) -> NormalizedNode {
    fn inline_rec(
        node: &NormalizedNode,
        inline_map: &HashMap<usize, NormalizedNode>,
        visited: &mut HashSet<usize>,
    ) -> NormalizedNode {
        NormalizedNodeWalker::map(node, |n| {
            let NormalizedNodeKind::Reference(idx) = n.kind else {
                return n;
            };
            let Some(inlined) = inline_map.get(&idx) else {
                return n;
            };
            if !visited.insert(idx) {
                return n;
            } // Cycle: skip
            let result = inline_rec(inlined, inline_map, visited);
            visited.remove(&idx);
            result
        })
    }
    inline_rec(node, inline_map, &mut HashSet::new())
}

// Helper: update references after rules are removed
fn update_references_after_removal(
    node: &NormalizedNode,
    index_map: &HashMap<usize, usize>,
) -> NormalizedNode {
    NormalizedNodeWalker::map(node, |n| {
        let NormalizedNodeKind::Reference(idx) = n.kind else {
            return n;
        };
        let Some(&new_idx) = index_map.get(&idx) else {
            return n;
        };
        NormalizedNode {
            kind: NormalizedNodeKind::Reference(new_idx),
            properties: n.properties,
        }
    })
}
