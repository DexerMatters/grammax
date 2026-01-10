use std::collections::HashSet;

use indexmap::{IndexSet, set::MutableValues};

use crate::{grammar::Rule, grammar_dsl::*};

#[inline]
fn add1_depth(depth: usize) -> usize {
    if depth == usize::MAX {
        usize::MAX
    } else {
        depth.saturating_add(1)
    }
}

#[inline]
fn top_props() -> RuleNodeProperties {
    RuleNodeProperties {
        is_nullable: false,
        is_consuming: false,
        min_depth: usize::MAX,
        max_depth: 0,
        min_consuming_steps: usize::MAX,
        max_consuming_steps: 0,
    }
}

fn init_props(rules: &mut IndexSet<Rule>) {
    // Start from a lattice “top” for min_depth (usize::MAX) and a “bottom” for booleans/max_depth.
    // This makes the fixed-point monotone and prevents non-terminating growth.
    for idx in 0..rules.len() {
        let Some(rule) = rules.get_index_mut2(idx) else {
            continue;
        };

        NormalizedNodeWalker::for_each_mut(&mut rule.node, |n| {
            match &n.kind {
                NormalizedNodeKind::Terminal(m) => {
                    let is_nullable = m.is_nullable();
                    let is_consuming = m.is_consuming();
                    n.properties = RuleNodeProperties {
                        is_nullable,
                        is_consuming,
                        min_depth: 0,
                        max_depth: 0,
                        min_consuming_steps: if is_consuming { 1 } else { 0 },
                        max_consuming_steps: if is_consuming { 1 } else { 0 },
                    };
                }
                NormalizedNodeKind::Sequence(nodes) if nodes.is_empty() => {
                    // Epsilon
                    n.properties = RuleNodeProperties {
                        is_nullable: true,
                        is_consuming: false,
                        min_depth: 0,
                        max_depth: 0,
                        min_consuming_steps: 0,
                        max_consuming_steps: 0,
                    };
                }
                _ => {
                    n.properties = top_props();
                }
            }
        });
    }
}

pub fn is_rule_recursive(rule_idx: usize, rules: &IndexSet<Rule>) -> bool {
    let mut visited = HashSet::new();
    let mut stack = vec![rule_idx];
    visited.insert(rule_idx);

    while let Some(current) = stack.pop() {
        if let Some(rule) = rules.get_index(current) {
            let mut refs = Vec::new();
            NormalizedNodeWalker::collect_references(&rule.node, &mut refs);

            for &ref_idx in &refs {
                if ref_idx == rule_idx {
                    // Found a reference back to the original rule
                    return true;
                }
                if visited.insert(ref_idx) {
                    stack.push(ref_idx);
                }
            }
        }
    }

    false
}

pub fn analyze_node_properties(rules: &mut IndexSet<Rule>) {
    init_props(rules);

    let mut changed = true;
    while changed {
        changed = false;

        // Snapshot rule-root properties for reference resolution
        let target_props: Vec<_> = (0..rules.len())
            .map(|i| rules.get_index(i).map(|r| r.node.properties))
            .collect();
        let target_is_recursive: Vec<_> = (0..rules.len())
            .map(|i| rules.get_index(i).map(|r| r.properties.is_recursive))
            .collect();

        // Update reference nodes from target rule props
        for idx in 0..rules.len() {
            let Some(rule) = rules.get_index_mut2(idx) else {
                continue;
            };
            NormalizedNodeWalker::for_each_mut(&mut rule.node, |n| {
                update_reference_props(n, &target_props, &target_is_recursive, &mut changed);
            });
        }

        // Bottom-up update for non-reference nodes
        for idx in 0..rules.len() {
            let Some(rule) = rules.get_index_mut2(idx) else {
                continue;
            };
            NormalizedNodeWalker::post_order_mut(&mut rule.node, |n| {
                update_node_props(n, &mut changed);
            });
        }
    }
}

fn update_reference_props(
    n: &mut NormalizedNode,
    target_props: &[Option<RuleNodeProperties>],
    target_is_recursive: &[Option<bool>],
    changed: &mut bool,
) {
    let NormalizedNodeKind::Reference(ref_idx) = &n.kind else {
        return;
    };
    let Some(Some(props)) = target_props.get(*ref_idx) else {
        return;
    };
    let is_recursive = target_is_recursive
        .get(*ref_idx)
        .and_then(|v| *v)
        .unwrap_or(false);

    let new_props = RuleNodeProperties {
        is_nullable: props.is_nullable,
        is_consuming: props.is_consuming,
        min_depth: add1_depth(props.min_depth),
        max_depth: if is_recursive {
            usize::MAX
        } else {
            add1_depth(props.max_depth)
        },
        min_consuming_steps: props.min_consuming_steps,
        max_consuming_steps: if is_recursive {
            usize::MAX
        } else {
            props.max_consuming_steps
        },
    };

    if n.properties != new_props {
        *changed = true;
        n.properties = new_props;
    }
}

fn update_node_props(n: &mut NormalizedNode, changed: &mut bool) {
    use NormalizedNodeKind as N;

    let new_props = match &n.kind {
        N::Terminal(m) => {
            let is_nullable = m.is_nullable();
            let is_consuming = m.is_consuming();
            RuleNodeProperties {
                is_nullable,
                is_consuming,
                min_depth: 0,
                max_depth: 0,
                min_consuming_steps: if is_consuming { 1 } else { 0 },
                max_consuming_steps: if is_consuming { 1 } else { 0 },
            }
        }
        N::Placeholder => top_props(),
        N::Reference(_) => return,
        N::Choice(nodes) => choice_props(nodes),
        N::Sequence(nodes) => sequence_props(nodes),
    };

    if n.properties != new_props {
        *changed = true;
        n.properties = new_props;
    }
}

fn choice_props(nodes: &[NormalizedNode]) -> RuleNodeProperties {
    if nodes.is_empty() {
        return RuleNodeProperties {
            is_nullable: true,
            is_consuming: false,
            min_depth: 0,
            max_depth: 0,
            min_consuming_steps: 0,
            max_consuming_steps: 0,
        };
    }

    let min_child = nodes
        .iter()
        .map(|c| c.properties.min_depth)
        .min()
        .unwrap_or(usize::MAX);
    let max_child = nodes
        .iter()
        .map(|c| c.properties.max_depth)
        .max()
        .unwrap_or(0);
    let min_consuming = nodes
        .iter()
        .map(|c| c.properties.min_consuming_steps)
        .min()
        .unwrap_or(usize::MAX);
    let max_consuming = nodes
        .iter()
        .map(|c| c.properties.max_consuming_steps)
        .max()
        .unwrap_or(0);

    RuleNodeProperties {
        is_nullable: nodes.iter().any(|c| c.properties.is_nullable),
        is_consuming: nodes.iter().any(|c| c.properties.is_consuming),
        min_depth: add1_depth(min_child),
        max_depth: add1_depth(max_child),
        min_consuming_steps: min_consuming,
        max_consuming_steps: max_consuming,
    }
}

fn sequence_props(nodes: &[NormalizedNode]) -> RuleNodeProperties {
    if nodes.is_empty() {
        return RuleNodeProperties {
            is_nullable: true,
            is_consuming: false,
            min_depth: 0,
            max_depth: 0,
            min_consuming_steps: 0,
            max_consuming_steps: 0,
        };
    }

    let min_child = nodes
        .iter()
        .map(|c| c.properties.min_depth)
        .max()
        .unwrap_or(usize::MAX);
    let max_child = nodes
        .iter()
        .map(|c| c.properties.max_depth)
        .max()
        .unwrap_or(0);
    let min_consuming = nodes
        .iter()
        .map(|c| c.properties.min_consuming_steps)
        .fold(0usize, |acc, x| acc.saturating_add(x));
    let max_consuming = nodes
        .iter()
        .map(|c| c.properties.max_consuming_steps)
        .fold(0usize, |acc, x| acc.saturating_add(x));

    RuleNodeProperties {
        is_nullable: nodes.iter().all(|c| c.properties.is_nullable),
        is_consuming: nodes.iter().any(|c| c.properties.is_consuming),
        min_depth: add1_depth(min_child),
        max_depth: add1_depth(max_child),
        min_consuming_steps: min_consuming,
        max_consuming_steps: max_consuming,
    }
}
