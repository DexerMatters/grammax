use std::sync::Arc;

use crate::grammar_old::{dsl::GrammarNode, ir::NormalizedGrammarNode};

use dashmap::{DashMap, DashSet};
use rustc_hash::FxHashSet;

use crate::grammar_old::norm::NormalizedGrammarNode::*;

#[derive(Clone, Debug)]
pub struct RuleTable {
    pub rule_names: Vec<&'static str>,
    pub rule_descriptions: Vec<&'static str>,
    pub rules: Vec<NormalizedGrammarNode>,
    pub left_rec: Vec<Option<LeftRecInfo>>,
}

#[derive(Clone, Debug)]
pub struct LeftRecInfo {
    pub base: Vec<NormalizedGrammarNode>,
    pub tail: Vec<NormalizedGrammarNode>,
    pub tail_fields: Vec<Option<&'static str>>,
}

impl RuleTable {
    pub fn new(initial_rules: Vec<NormalizedGrammarNode>) -> Self {
        let len = initial_rules.len();
        Self {
            rule_names: vec![""; len],
            rule_descriptions: vec![""; len],
            rules: initial_rules,
            left_rec: vec![None; len],
        }
    }

    pub fn compute_from(&mut self, start: GrammarNode, start_name: &'static str) {
        self.discover_and_desugar(start, start_name);
        self.optimize_relay_nodes();
        self.collapse_relay_references();
        self.expand_leading_alternatives();
        self.simplify_epsilon();
        self.eliminate_left_recursion();
        self.factor_common_prefixes();
        self.simplify_epsilon();
        // self.sort_alternatives();
    }

    pub fn sort_alternatives(&mut self) {
        // We need a clone of rules to lookup complexities while sorting,
        // OR we compute complexities first.
        let complexities: Vec<usize> = self
            .rules
            .iter()
            .enumerate()
            .map(|(i, _)| self.calculate_complexity(i, &mut Vec::new()))
            .collect();

        for rule in &mut self.rules {
            Self::sort_node_alternatives(rule, &complexities);
        }
    }

    fn calculate_complexity(&self, rule_ix: usize, visited: &mut Vec<usize>) -> usize {
        if visited.contains(&rule_ix) {
            return usize::MAX; // Recursive = Complex
        }
        visited.push(rule_ix);
        let res = self.estimate_node_complexity(&self.rules[rule_ix], visited);
        visited.pop();
        res
    }

    fn sort_node_alternatives(node: &mut NormalizedGrammarNode, complexities: &[usize]) {
        match node {
            NormalizedGrammarNode::Alternative(alts) => {
                for alt in alts.iter_mut() {
                    Self::sort_node_alternatives(alt, complexities);
                }
                alts.sort_by(|a, b| {
                    Self::static_estimate_complexity(a, complexities)
                        .cmp(&Self::static_estimate_complexity(b, complexities))
                });
            }
            NormalizedGrammarNode::Sequence(seq) => {
                for n in seq.iter_mut() {
                    Self::sort_node_alternatives(n, complexities);
                }
            }
            NormalizedGrammarNode::Field(_, n) => Self::sort_node_alternatives(n, complexities),
            _ => {}
        }
    }

    fn estimate_node_complexity(
        &self,
        node: &NormalizedGrammarNode,
        visited: &mut Vec<usize>,
    ) -> usize {
        match node {
            NormalizedGrammarNode::Terminal(m) => {
                if m.display() == "ε" {
                    usize::MAX
                } else {
                    1
                }
            }
            NormalizedGrammarNode::Reference(ix) => self.calculate_complexity(*ix, visited),
            NormalizedGrammarNode::Sequence(seq) => seq
                .iter()
                .map(|n| self.estimate_node_complexity(n, visited))
                .fold(0, |acc, x| acc.saturating_add(x)),
            NormalizedGrammarNode::Alternative(alts) => alts
                .iter()
                .map(|n| self.estimate_node_complexity(n, visited))
                .min()
                .unwrap_or(0),
            NormalizedGrammarNode::Field(_, n) => self.estimate_node_complexity(n, visited),
        }
    }

    fn static_estimate_complexity(node: &NormalizedGrammarNode, complexities: &[usize]) -> usize {
        match node {
            NormalizedGrammarNode::Terminal(m) => {
                if m.display() == "ε" {
                    usize::MAX
                } else {
                    1
                }
            }
            NormalizedGrammarNode::Reference(ix) => complexities[*ix],
            NormalizedGrammarNode::Sequence(seq) => {
                if seq.is_empty() {
                    return usize::MAX;
                }
                seq.iter()
                    .map(|n| Self::static_estimate_complexity(n, complexities))
                    .fold(0, |acc, x| acc.saturating_add(x))
            }
            NormalizedGrammarNode::Alternative(alts) => alts
                .iter()
                .map(|n| Self::static_estimate_complexity(n, complexities))
                .min()
                .unwrap_or(0),
            NormalizedGrammarNode::Field(_, n) => Self::static_estimate_complexity(n, complexities),
        }
    }

    fn discover_and_desugar(&mut self, start: GrammarNode, start_name: &'static str) {
        let mut rule_names: Vec<&'static str> = vec![start_name];
        let mut rule_map: DashMap<&'static str, GrammarNode> = DashMap::new();

        rule_map.insert(start_name, start.clone());

        let mut discovered_order: Vec<&'static str> = Vec::new();
        let mut discovered_set: FxHashSet<&'static str> = FxHashSet::default();
        self.discover_and_collect_references(
            &start,
            &mut discovered_order,
            &mut discovered_set,
            &mut rule_map,
        );

        for name in discovered_order {
            if name != start_name && !rule_names.contains(&name) {
                rule_names.push(name);
            }
        }

        self.rule_names = rule_names.clone();

        let named_rule_names = self.rule_names.clone();
        let named_count = named_rule_names.len();

        let mut rules: Vec<NormalizedGrammarNode> = Vec::with_capacity(named_count);
        let mut anon_rules_all: Vec<NormalizedGrammarNode> = Vec::new();
        let mut anon_names_all: Vec<&'static str> = Vec::new();

        let mut visited: DashSet<&'static str> = DashSet::new();

        for rule_name in named_rule_names.iter() {
            if let Some(grammar_node) = rule_map.get(rule_name) {
                let anon_base = named_count + anon_rules_all.len();
                let (normalized, mut anon_rules, mut anon_names) =
                    self.normalize_node(&grammar_node, &mut visited, anon_base, &named_rule_names);
                rules.push(normalized);
                anon_rules_all.append(&mut anon_rules);
                anon_names_all.append(&mut anon_names);
            }
        }

        self.rule_names.extend(anon_names_all);
        rules.extend(anon_rules_all);
        self.rules = rules;
        self.rule_descriptions = vec![""; self.rules.len()];
    }

    fn factor_common_prefixes(&mut self) {
        for rule in &mut self.rules {
            *rule = Self::factor_node(rule.clone());
        }
    }

    fn expand_leading_alternatives(&mut self) {
        for rule in &mut self.rules {
            *rule = Self::expand_node(rule.clone());
        }
    }

    fn expand_node(node: NormalizedGrammarNode) -> NormalizedGrammarNode {
        match node {
            Field(name, inner) => Field(name, Box::new(Self::expand_node(*inner))),
            Sequence(mut nodes) => {
                if let Some(Alternative(alts)) = nodes.first().cloned() {
                    let rest = nodes.split_off(1);
                    let expanded = alts
                        .into_iter()
                        .map(|alt| {
                            let mut seq = Vec::with_capacity(1 + rest.len());
                            seq.push(alt);
                            seq.extend(rest.clone());
                            Sequence(seq)
                        })
                        .map(Self::expand_node)
                        .collect();
                    return Alternative(expanded);
                }
                if let Some(Field(name, inner)) = nodes.first().cloned() {
                    if let Alternative(alts) = *inner {
                        let rest = nodes.split_off(1);
                        let expanded = alts
                            .into_iter()
                            .map(|alt| {
                                let mut seq = Vec::with_capacity(1 + rest.len());
                                seq.push(Field(name, Box::new(alt)));
                                seq.extend(rest.clone());
                                Sequence(seq)
                            })
                            .map(Self::expand_node)
                            .collect();
                        return Alternative(expanded);
                    }
                }
                let out = nodes.into_iter().map(Self::expand_node).collect();
                Sequence(out)
            }
            Alternative(nodes) => Alternative(nodes.into_iter().map(Self::expand_node).collect()),
            _ => node,
        }
    }

    fn simplify_epsilon(&mut self) {
        for rule in &mut self.rules {
            *rule = Self::simplify_node(rule.clone());
        }
    }

    fn simplify_node(node: NormalizedGrammarNode) -> NormalizedGrammarNode {
        match node {
            Field(name, inner) => {
                let simplified = Self::simplify_node(*inner);
                if Self::is_epsilon(&simplified) {
                    simplified
                } else {
                    Field(name, Box::new(simplified))
                }
            }
            Sequence(nodes) => {
                let mut out: Vec<NormalizedGrammarNode> = Vec::new();
                for n in nodes.into_iter().map(Self::simplify_node) {
                    if Self::is_epsilon(&n) {
                        continue;
                    }
                    match n {
                        Sequence(inner) => out.extend(inner),
                        other => out.push(other),
                    }
                }

                if out.is_empty() {
                    Self::epsilon_node()
                } else {
                    Self::seq_or_single(out)
                }
            }
            Alternative(nodes) => {
                let mut out: Vec<NormalizedGrammarNode> = Vec::new();
                for n in nodes.into_iter().map(Self::simplify_node) {
                    match n {
                        Alternative(inner) => out.extend(inner),
                        other => out.push(other),
                    }
                }
                Self::alt_or_single(out)
            }
            _ => node,
        }
    }

    fn is_epsilon(node: &NormalizedGrammarNode) -> bool {
        match node {
            Terminal(m) => m.display() == "ε",
            _ => false,
        }
    }

    fn epsilon_node() -> NormalizedGrammarNode {
        Terminal(Arc::new(()))
    }

    fn seq_or_single(mut seq: Vec<NormalizedGrammarNode>) -> NormalizedGrammarNode {
        if seq.len() == 1 {
            seq.pop().unwrap()
        } else {
            Sequence(seq)
        }
    }

    fn alt_or_single(mut alts: Vec<NormalizedGrammarNode>) -> NormalizedGrammarNode {
        if alts.len() == 1 {
            alts.pop().unwrap()
        } else {
            Alternative(alts)
        }
    }

    fn factor_node(node: NormalizedGrammarNode) -> NormalizedGrammarNode {
        match node {
            Terminal(_) | Reference(_) => node,
            Field(name, inner) => Field(name, Box::new(Self::factor_node(*inner))),
            Sequence(nodes) => {
                let mut out = Vec::with_capacity(nodes.len());
                for n in nodes {
                    out.push(Self::factor_node(n));
                }
                Sequence(out)
            }
            Alternative(nodes) => {
                let mut alts: Vec<Vec<NormalizedGrammarNode>> = nodes
                    .into_iter()
                    .map(|n| match Self::factor_node(n) {
                        Sequence(seq) => seq,
                        other => vec![other],
                    })
                    .collect();

                if alts.len() < 2 {
                    let single = alts.pop().unwrap_or_default();
                    return Self::seq_or_single(single);
                }

                let mut tail_groups: Vec<(
                    String,
                    Vec<NormalizedGrammarNode>,
                    Vec<NormalizedGrammarNode>,
                )> = Vec::new();
                let mut singletons: Vec<Vec<NormalizedGrammarNode>> = Vec::new();

                for seq in alts {
                    if seq.len() < 2 {
                        singletons.push(seq);
                        continue;
                    }

                    let tail = seq[1..].to_vec();
                    let key = tail
                        .iter()
                        .map(|n| n.to_string())
                        .collect::<Vec<_>>()
                        .join("\u{1f}");
                    let head = seq[0].clone();

                    if let Some((_, _, heads)) = tail_groups.iter_mut().find(|(k, _, _)| *k == key)
                    {
                        heads.push(head);
                    } else {
                        tail_groups.push((key, tail, vec![head]));
                    }
                }

                let mut merged: Vec<Vec<NormalizedGrammarNode>> = Vec::new();

                for (_, tail, heads) in tail_groups {
                    if heads.len() == 1 {
                        let mut seq = Vec::with_capacity(1 + tail.len());
                        seq.push(heads[0].clone());
                        seq.extend(tail.into_iter());
                        merged.push(seq);
                    } else {
                        let mut seq = Vec::with_capacity(1 + tail.len());
                        seq.push(Alternative(heads));
                        seq.extend(tail.into_iter());
                        merged.push(seq);
                    }
                }

                merged.extend(singletons.into_iter());

                let mut grouped: Vec<(String, Vec<Vec<NormalizedGrammarNode>>)> = Vec::new();
                for seq in merged {
                    let key = seq
                        .get(0)
                        .map(|n| n.to_string())
                        .unwrap_or_else(|| "".to_string());
                    if let Some((_, group)) = grouped.iter_mut().find(|(k, _)| *k == key) {
                        group.push(seq);
                    } else {
                        grouped.push((key, vec![seq]));
                    }
                }

                let mut new_alts = Vec::new();
                for (_, group) in grouped {
                    if group.len() == 1 {
                        let seq = group.into_iter().next().unwrap();
                        new_alts.push(Self::seq_or_single(seq));
                        continue;
                    }

                    let mut prefix_len = 0;
                    loop {
                        let first = match group[0].get(prefix_len) {
                            Some(n) => n,
                            None => break,
                        };
                        let first_key = first.to_string();
                        if group.iter().all(|seq| {
                            seq.get(prefix_len).map(|n| n.to_string()) == Some(first_key.clone())
                        }) {
                            prefix_len += 1;
                        } else {
                            break;
                        }
                    }

                    if prefix_len == 0 {
                        for seq in group {
                            new_alts.push(Self::seq_or_single(seq));
                        }
                        continue;
                    }

                    let prefix = group[0][..prefix_len].to_vec();
                    let mut suffixes = Vec::with_capacity(group.len());
                    for seq in group {
                        let rest = &seq[prefix_len..];
                        let suffix = if rest.is_empty() {
                            Terminal(Arc::new(()))
                        } else if rest.len() == 1 {
                            rest[0].clone()
                        } else {
                            Sequence(rest.to_vec())
                        };
                        suffixes.push(Self::factor_node(suffix));
                    }

                    let mut seq = prefix;
                    seq.push(Alternative(suffixes));
                    new_alts.push(Sequence(seq));
                }

                if new_alts.len() == 1 {
                    new_alts.remove(0)
                } else {
                    Alternative(new_alts)
                }
            }
        }
    }

    fn optimize_relay_nodes(&mut self) {
        let max_iterations = 10;

        for _ in 0..max_iterations {
            let mut changed = false;

            // Find and inline one level of relays
            for i in 0..self.rules.len() {
                if let Reference(target_idx) = &self.rules[i] {
                    let target_idx = *target_idx;
                    let should_inline = (i < self.rule_names.len()
                        && self.rule_names[i].is_empty())
                        || (target_idx < self.rule_names.len()
                            && self.rule_names[target_idx].is_empty());

                    if should_inline && target_idx < self.rules.len() {
                        let replacement =
                            self.remap_self_references(&self.rules[target_idx], target_idx, i);
                        self.rules[i] = replacement;
                        changed = true;
                    }
                }
            }

            if !changed {
                break;
            }
        }
    }

    fn collapse_relay_references(&mut self) {
        let mut map = vec![0; self.rules.len()];
        for i in 0..self.rules.len() {
            let mut visiting = Vec::new();
            map[i] = self.resolve_relay(i, &mut visiting);
        }

        for rule in &mut self.rules {
            *rule = Self::remap_references_static(rule, &map);
        }
    }

    fn resolve_relay(&self, ix: usize, visiting: &mut Vec<usize>) -> usize {
        if visiting.contains(&ix) {
            return ix;
        }
        visiting.push(ix);
        let target = match &self.rules[ix] {
            Reference(j) if *j != ix => self.resolve_relay(*j, visiting),
            _ => ix,
        };
        visiting.pop();
        target
    }

    fn remap_references_static(
        node: &NormalizedGrammarNode,
        map: &[usize],
    ) -> NormalizedGrammarNode {
        match node {
            Terminal(_) => node.clone(),
            Reference(ix) => Reference(map[*ix]),
            Field(name, inner) => Field(*name, Box::new(Self::remap_references_static(inner, map))),
            Sequence(nodes) => Sequence(
                nodes
                    .iter()
                    .map(|n| Self::remap_references_static(n, map))
                    .collect(),
            ),
            Alternative(nodes) => Alternative(
                nodes
                    .iter()
                    .map(|n| Self::remap_references_static(n, map))
                    .collect(),
            ),
        }
    }

    /// Remap self-references when inlining a rule
    fn remap_self_references(
        &self,
        node: &NormalizedGrammarNode,
        from_idx: usize,
        to_idx: usize,
    ) -> NormalizedGrammarNode {
        match node {
            Terminal(_) => node.clone(),
            Reference(idx) => {
                if *idx == from_idx {
                    Reference(to_idx)
                } else {
                    Reference(*idx)
                }
            }
            Field(name, inner) => Field(
                *name,
                Box::new(self.remap_self_references(inner, from_idx, to_idx)),
            ),
            Sequence(nodes) => Sequence(
                nodes
                    .iter()
                    .map(|n| self.remap_self_references(n, from_idx, to_idx))
                    .collect(),
            ),
            Alternative(nodes) => Alternative(
                nodes
                    .iter()
                    .map(|n| self.remap_self_references(n, from_idx, to_idx))
                    .collect(),
            ),
        }
    }

    /// Discover and collect all rule references
    fn discover_and_collect_references(
        &self,
        node: &GrammarNode,
        discovered_order: &mut Vec<&'static str>,
        discovered_set: &mut FxHashSet<&'static str>,
        rule_map: &mut DashMap<&'static str, GrammarNode>,
    ) {
        match node {
            GrammarNode::Terminal(_) => {}

            GrammarNode::Reference(func, name) => {
                if !discovered_set.contains(name) {
                    discovered_set.insert(name);
                    discovered_order.push(name);
                    // Call the function to get the actual rule
                    let rule_node = func();
                    rule_map.insert(name, rule_node.clone());
                    // Recursively discover references in this rule
                    self.discover_and_collect_references(
                        &rule_node,
                        discovered_order,
                        discovered_set,
                        rule_map,
                    );
                }
            }

            GrammarNode::Sequence(nodes) | GrammarNode::Alternative(nodes) => {
                for n in nodes {
                    self.discover_and_collect_references(
                        n,
                        discovered_order,
                        discovered_set,
                        rule_map,
                    );
                }
            }

            GrammarNode::Field(_, node) => {
                self.discover_and_collect_references(
                    node,
                    discovered_order,
                    discovered_set,
                    rule_map,
                );
            }

            GrammarNode::Repetition { node, .. } => {
                self.discover_and_collect_references(
                    node,
                    discovered_order,
                    discovered_set,
                    rule_map,
                );
            }

            GrammarNode::SeparatedRepetition {
                node, separator, ..
            } => {
                self.discover_and_collect_references(
                    node,
                    discovered_order,
                    discovered_set,
                    rule_map,
                );
                self.discover_and_collect_references(
                    separator,
                    discovered_order,
                    discovered_set,
                    rule_map,
                );
            }
        }
    }

    /// Normalize a GrammarNode into a NormalizedGrammarNode
    /// Returns (normalized_node, anonymous_rules, anonymous_rule_names)
    fn normalize_node(
        &self,
        node: &GrammarNode,
        visited: &mut DashSet<&'static str>,
        anon_base: usize,
        named_rule_names: &[&'static str],
    ) -> (
        NormalizedGrammarNode,
        Vec<NormalizedGrammarNode>,
        Vec<&'static str>,
    ) {
        let mut anon_rules = Vec::new();
        let mut anon_names = Vec::new();

        let mut normalize_child =
            |node: &GrammarNode,
             anon_rules: &mut Vec<NormalizedGrammarNode>,
             anon_names: &mut Vec<&'static str>| {
                let (norm_node, mut anon, mut names) =
                    self.normalize_node(node, visited, anon_base, named_rule_names);
                anon_rules.append(&mut anon);
                anon_names.append(&mut names);
                norm_node
            };

        let normalized = match node {
            GrammarNode::Terminal(matcher) => Terminal(Arc::clone(matcher)),

            GrammarNode::Reference(_, name) => {
                let idx = named_rule_names
                    .iter()
                    .position(|&n| n == *name)
                    .unwrap_or_else(|| named_rule_names.len());
                Reference(idx)
            }

            GrammarNode::Sequence(nodes) => {
                let mut normalized_nodes = Vec::new();
                for n in nodes {
                    let norm_node = normalize_child(n, &mut anon_rules, &mut anon_names);
                    normalized_nodes.push(norm_node);
                }
                Sequence(normalized_nodes)
            }

            GrammarNode::Alternative(nodes) => {
                let mut normalized_nodes = Vec::new();
                for n in nodes {
                    let norm_node = normalize_child(n, &mut anon_rules, &mut anon_names);
                    normalized_nodes.push(norm_node);
                }
                Alternative(normalized_nodes)
            }

            GrammarNode::Field(name, node) => {
                let norm_inner = normalize_child(node, &mut anon_rules, &mut anon_names);
                Field(*name, Box::new(norm_inner))
            }

            GrammarNode::Repetition { node, min, max } => {
                let norm_inner = normalize_child(node, &mut anon_rules, &mut anon_names);

                let anon_idx = anon_base + anon_rules.len();

                let rep_rule = match (min, max) {
                    (0, None) => {
                        let self_ref = Reference(anon_idx);
                        Alternative(vec![norm_inner + self_ref, Terminal(Arc::new(()))])
                    }
                    (1, None) => {
                        let self_ref = Reference(anon_idx);
                        Alternative(vec![norm_inner.clone(), norm_inner + self_ref])
                    }
                    (min_count, Some(max_count)) => {
                        let mut alternatives = Vec::new();
                        let start = if *min_count == 0 { 1 } else { *min_count };
                        for count in start..=*max_count {
                            let mut seq_nodes = vec![norm_inner.clone(); count];
                            let alternative = if seq_nodes.is_empty() {
                                Terminal(Arc::new(()))
                            } else {
                                let mut result = seq_nodes.remove(0);
                                for node in seq_nodes {
                                    result = result + node;
                                }
                                result
                            };
                            alternatives.push(alternative);
                        }
                        if *min_count == 0 {
                            alternatives.push(Terminal(Arc::new(())));
                        }
                        if alternatives.len() == 1 {
                            alternatives.pop().unwrap()
                        } else {
                            Alternative(alternatives)
                        }
                    }
                    (min_count, None) => {
                        let self_ref = Reference(anon_idx);
                        let mut result = norm_inner.clone();
                        for _ in 1..*min_count {
                            result = result + norm_inner.clone();
                        }
                        result + self_ref
                    }
                };

                anon_rules.push(rep_rule);
                anon_names.push("@rep");

                Reference(anon_idx)
            }

            GrammarNode::SeparatedRepetition {
                node,
                separator,
                min,
                max,
            } => {
                let norm_inner = normalize_child(node, &mut anon_rules, &mut anon_names);
                let norm_sep = normalize_child(separator, &mut anon_rules, &mut anon_names);

                let list_idx = anon_base + anon_rules.len();
                let tail_idx = list_idx + 1;
                let tail_ref = Reference(tail_idx);

                let make_sequence = |count: usize| -> NormalizedGrammarNode {
                    if count == 0 {
                        Self::epsilon_node()
                    } else {
                        let mut parts = Vec::with_capacity(count * 2 - 1);
                        for i in 0..count {
                            parts.push(norm_inner.clone());
                            if i + 1 < count {
                                parts.push(norm_sep.clone());
                            }
                        }
                        Self::seq_or_single(parts)
                    }
                };

                let list_rule = match (min, max) {
                    (0, None) => Alternative(vec![
                        norm_inner.clone() + tail_ref.clone(),
                        Self::epsilon_node(),
                    ]),
                    (1, None) => norm_inner.clone() + tail_ref.clone(),
                    (min_count, Some(max_count)) => {
                        let mut alternatives = Vec::new();
                        let start = if *min_count == 0 { 1 } else { *min_count };
                        for count in start..=*max_count {
                            alternatives.push(make_sequence(count));
                        }
                        if *min_count == 0 {
                            alternatives.push(Self::epsilon_node());
                        }
                        if alternatives.len() == 1 {
                            alternatives.pop().unwrap()
                        } else {
                            Alternative(alternatives)
                        }
                    }
                    (min_count, None) => {
                        let prefix = make_sequence(*min_count);
                        prefix + tail_ref.clone()
                    }
                };

                let tail_rule = Alternative(vec![
                    norm_sep.clone() + norm_inner.clone() + tail_ref.clone(),
                    Self::epsilon_node(),
                ]);

                anon_rules.push(list_rule);
                anon_names.push("@sep");
                anon_rules.push(tail_rule);
                anon_names.push("@sep_tail");

                Reference(list_idx)
            }
        };

        (normalized, anon_rules, anon_names)
    }

    fn eliminate_left_recursion(&mut self) {
        let original_len = self.rules.len();
        let mut new_rules = self.rules.clone();
        let mut new_names = self.rule_names.clone();
        let mut left_rec = vec![None; new_rules.len()];

        for i in 0..original_len {
            for j in 0..i {
                let target = new_rules[j].clone();
                new_rules[i] = Self::expand_leading_reference(new_rules[i].clone(), j, &target);
            }

            let node = new_rules[i].clone();
            let alts = match node {
                Alternative(nodes) => nodes,
                n => vec![n],
            };

            let mut betas = Vec::new();
            let mut alphas = Vec::new();
            let mut alpha_fields = Vec::new();

            for alt in alts {
                if let Some((alpha, field_name)) = Self::strip_left_rec(i, &alt) {
                    alphas.push(alpha);
                    alpha_fields.push(field_name);
                } else {
                    betas.push(alt);
                }
            }

            if alphas.is_empty() || betas.is_empty() {
                continue;
            }

            let tail_idx = new_rules.len();
            let tail_ref = Reference(tail_idx);
            new_names.push("");

            let mut base_alts = Vec::new();
            for beta in &betas {
                base_alts.push(Self::append_rec(beta, &tail_ref));
            }

            let mut tail_alts = Vec::new();
            for alpha in &alphas {
                tail_alts.push(Self::append_rec(alpha, &tail_ref));
            }
            tail_alts.push(Terminal(Arc::new(())));

            let new_rule = Self::alt_or_single(base_alts);
            let tail_rule = Self::alt_or_single(tail_alts);

            new_rules[i] = new_rule;
            new_rules.push(tail_rule);
            left_rec[i] = Some(LeftRecInfo {
                base: betas,
                tail: alphas,
                tail_fields: alpha_fields,
            });
        }

        self.rules = new_rules;
        self.rule_names = new_names;
        left_rec.resize(self.rules.len(), None);
        self.left_rec = left_rec;
    }

    fn expand_leading_reference(
        node: NormalizedGrammarNode,
        target_ix: usize,
        target_rule: &NormalizedGrammarNode,
    ) -> NormalizedGrammarNode {
        let target_alts = match target_rule {
            Alternative(alts) => alts.clone(),
            other => vec![other.clone()],
        };

        Self::expand_leading_reference_with(node, target_ix, &target_alts)
    }

    fn expand_leading_reference_with(
        node: NormalizedGrammarNode,
        target_ix: usize,
        target_alts: &[NormalizedGrammarNode],
    ) -> NormalizedGrammarNode {
        match node {
            Alternative(nodes) => Alternative(
                nodes
                    .into_iter()
                    .map(|n| Self::expand_leading_reference_with(n, target_ix, target_alts))
                    .collect(),
            ),
            Reference(ix) if ix == target_ix => Self::alt_or_single(target_alts.to_vec()),
            Field(name, inner) => match *inner {
                Reference(ix) if ix == target_ix => Self::alt_or_single(
                    target_alts
                        .iter()
                        .cloned()
                        .map(|alt| Field(name, Box::new(alt)))
                        .collect(),
                ),
                other => Field(name, Box::new(other)),
            },
            Sequence(mut nodes) => {
                if nodes.is_empty() {
                    return Sequence(nodes);
                }

                let first = nodes.remove(0);
                match first {
                    Reference(ix) if ix == target_ix => {
                        let expanded: Vec<NormalizedGrammarNode> = target_alts
                            .iter()
                            .cloned()
                            .map(|alt| Self::concat_alt_with_rest(alt, &nodes))
                            .collect();
                        Self::alt_or_single(expanded)
                    }
                    Field(name, inner) if matches!(*inner, Reference(ix) if ix == target_ix) => {
                        let expanded: Vec<NormalizedGrammarNode> = target_alts
                            .iter()
                            .cloned()
                            .map(|alt| Field(name, Box::new(alt)))
                            .map(|alt| Self::concat_alt_with_rest(alt, &nodes))
                            .collect();
                        Self::alt_or_single(expanded)
                    }
                    other => {
                        let mut seq = Vec::with_capacity(1 + nodes.len());
                        seq.push(other);
                        seq.extend(nodes);
                        Sequence(seq)
                    }
                }
            }
            _ => node,
        }
    }

    fn concat_alt_with_rest(
        alt: NormalizedGrammarNode,
        rest: &[NormalizedGrammarNode],
    ) -> NormalizedGrammarNode {
        match alt {
            Sequence(mut seq) => {
                let mut out = Vec::with_capacity(seq.len() + rest.len());
                out.append(&mut seq);
                out.extend(rest.iter().cloned());
                Self::seq_or_single(out)
            }
            other => {
                if rest.is_empty() {
                    other
                } else {
                    let mut out = Vec::with_capacity(1 + rest.len());
                    out.push(other);
                    out.extend(rest.iter().cloned());
                    Self::seq_or_single(out)
                }
            }
        }
    }

    fn strip_left_rec(
        rule_ix: usize,
        node: &NormalizedGrammarNode,
    ) -> Option<(NormalizedGrammarNode, Option<&'static str>)> {
        match node {
            Reference(ix) if *ix == rule_ix => Some((Terminal(Arc::new(())), None)),
            Field(name, inner) => match inner.as_ref() {
                Reference(ix) if *ix == rule_ix => Some((Terminal(Arc::new(())), Some(*name))),
                _ => Self::strip_left_rec(rule_ix, inner),
            },
            Sequence(nodes) => {
                if nodes.is_empty() {
                    None
                } else {
                    let first = &nodes[0];
                    let (first_inner, field_name) = match first {
                        Field(name, inner) => (inner.as_ref(), Some(*name)),
                        other => (other, None),
                    };
                    if let Reference(ix) = first_inner {
                        if *ix == rule_ix {
                            let rest = &nodes[1..];
                            if rest.is_empty() {
                                Some((Terminal(Arc::new(())), field_name))
                            } else if rest.len() == 1 {
                                Some((rest[0].clone(), field_name))
                            } else {
                                Some((Sequence(rest.to_vec()), field_name))
                            }
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                }
            }
            _ => None,
        }
    }

    fn append_rec(
        node: &NormalizedGrammarNode,
        tail_ref: &NormalizedGrammarNode,
    ) -> NormalizedGrammarNode {
        match node {
            Sequence(nodes) => {
                let mut nodes = nodes.clone();
                nodes.push(tail_ref.clone());
                Sequence(nodes)
            }
            _ => Sequence(vec![node.clone(), tail_ref.clone()]),
        }
    }
}
