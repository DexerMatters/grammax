use std::{fmt, rc::Rc};

use crate::grammar::{dsl::GrammarNode, ir::NormalizedGrammarNode};

use dashmap::{DashMap, DashSet};

use crate::grammar::norm::NormalizedGrammarNode::*;

#[derive(Clone, Debug)]
pub struct RuleTable {
    pub rule_names: Vec<&'static str>,
    pub rules: Vec<NormalizedGrammarNode>,
    pub left_rec: Vec<Option<LeftRecInfo>>,
}

#[derive(Clone, Debug)]
pub struct LeftRecInfo {
    pub base: Vec<NormalizedGrammarNode>,
    pub tail: Vec<NormalizedGrammarNode>,
}

impl fmt::Display for RuleTable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        const RESET: &str = "\x1b[0m";
        const BOLD: &str = "\x1b[1m";

        // Find which rules are actually referenced
        let mut used = vec![false; self.rules.len()];
        for (i, _) in self.rule_names.iter().enumerate() {
            if !self.rule_names[i].is_empty() {
                used[i] = true;
                self.mark_used(&self.rules[i], &mut used);
            }
        }

        // Calculate max name width for alignment (only for rules that will be shown)
        let max_width = self
            .rule_names
            .iter()
            .enumerate()
            .filter(|(i, n)| !n.is_empty() || used[*i])
            .map(|(i, n)| {
                if n.is_empty() {
                    format!("@{}", i).len()
                } else {
                    n.len()
                }
            })
            .max()
            .unwrap_or(10);

        for (i, rule) in self.rules.iter().enumerate() {
            // Skip unused anonymous rules
            if i < self.rule_names.len() && self.rule_names[i].is_empty() && !used[i] {
                continue;
            }

            let name = if i < self.rule_names.len() && !self.rule_names[i].is_empty() {
                self.rule_names[i].to_string()
            } else {
                format!("@{}", i)
            };

            let formatted_rule = self.format_node(rule);
            let padded_name = format!("{:<width$}", name, width = max_width);
            writeln!(
                f,
                "  {}{}{} {} {}",
                BOLD, padded_name, RESET, "→", formatted_rule
            )?;
        }
        Ok(())
    }
}

impl RuleTable {
    fn mark_used(&self, node: &NormalizedGrammarNode, used: &mut Vec<bool>) {
        match node {
            Terminal(_) => {}
            Reference(idx) => {
                if *idx < used.len() && !used[*idx] {
                    used[*idx] = true;
                    self.mark_used(&self.rules[*idx], used);
                }
            }
            Sequence(nodes) | Alternative(nodes) => {
                for n in nodes {
                    self.mark_used(n, used);
                }
            }
        }
    }

    fn get_rule_name(&self, idx: usize) -> String {
        self.rule_names
            .get(idx)
            .filter(|n| !n.is_empty())
            .map(|n| n.to_string())
            .unwrap_or_else(|| format!("@{}", idx))
    }
    fn format_node(&self, node: &NormalizedGrammarNode) -> String {
        self.format_node_inner(node, false)
    }

    fn format_node_inner(&self, node: &NormalizedGrammarNode, parent_is_seq: bool) -> String {
        const RESET: &str = "\x1b[0m";
        const BOLD: &str = "\x1b[1m";
        const GREY: &str = "\x1b[90m";

        match node {
            Terminal(matcher) => {
                format!("{}{}{}", GREY, matcher.display(), RESET)
            }
            Reference(index) => {
                format!("{}{}{}", BOLD, self.get_rule_name(*index), RESET)
            }
            Sequence(nodes) => {
                let parts: Vec<String> = nodes
                    .iter()
                    .map(|n| self.format_node_inner(n, true))
                    .collect();
                let content = parts.join(" ");
                // Only wrap in parens if inside an Alternative
                if parent_is_seq {
                    content
                } else {
                    format!("({})", content)
                }
            }
            Alternative(nodes) => {
                let parts: Vec<String> = nodes
                    .iter()
                    .map(|n| self.format_node_inner(n, false))
                    .collect();
                parts.join(" | ")
            }
        }
    }
}

impl RuleTable {
    pub fn new(initial_rules: Vec<NormalizedGrammarNode>) -> Self {
        let len = initial_rules.len();
        Self {
            rule_names: vec![""; len],
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
    }

    fn discover_and_desugar(&mut self, start: GrammarNode, start_name: &'static str) {
        let mut rule_names: Vec<&'static str> = vec![start_name];
        let mut rule_map: DashMap<&'static str, GrammarNode> = DashMap::new();

        rule_map.insert(start_name, start.clone());

        let mut discovered_refs: DashSet<&'static str> = DashSet::new();
        self.discover_and_collect_references(&start, &mut discovered_refs, &mut rule_map);

        for name in discovered_refs.iter() {
            if *name != start_name && !rule_names.contains(&name) {
                rule_names.push(&name);
            }
        }

        self.rule_names = rule_names.clone();

        let mut rules: Vec<NormalizedGrammarNode> = Vec::new();

        let mut visited: DashSet<&'static str> = DashSet::new();

        for rule_name in rule_names.iter() {
            if let Some(grammar_node) = rule_map.get(rule_name) {
                let (normalized, anon_rules, anon_names) =
                    self.normalize_node(&grammar_node, &mut visited);
                rules.push(normalized);

                for (name, node) in anon_names.into_iter().zip(anon_rules.into_iter()) {
                    self.rule_names.push(name);
                    rules.push(node);
                }
            }
        }

        self.rules = rules;
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
        Terminal(Rc::new(()))
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
                            Terminal(Rc::new(()))
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
        discovered: &mut DashSet<&'static str>,
        rule_map: &mut DashMap<&'static str, GrammarNode>,
    ) {
        match node {
            GrammarNode::Terminal(_) => {}

            GrammarNode::Reference(func, name) => {
                if !discovered.contains(name) {
                    discovered.insert(name);
                    // Call the function to get the actual rule
                    let rule_node = func();
                    rule_map.insert(name, rule_node.clone());
                    // Recursively discover references in this rule
                    self.discover_and_collect_references(&rule_node, discovered, rule_map);
                }
            }

            GrammarNode::Sequence(nodes) | GrammarNode::Alternative(nodes) => {
                for n in nodes {
                    self.discover_and_collect_references(n, discovered, rule_map);
                }
            }

            GrammarNode::Repetition { node, .. } => {
                self.discover_and_collect_references(node, discovered, rule_map);
            }
        }
    }

    /// Normalize a GrammarNode into a NormalizedGrammarNode
    /// Returns (normalized_node, anonymous_rules, anonymous_rule_names)
    fn normalize_node(
        &self,
        node: &GrammarNode,
        visited: &mut DashSet<&'static str>,
    ) -> (
        NormalizedGrammarNode,
        Vec<NormalizedGrammarNode>,
        Vec<&'static str>,
    ) {
        let mut anon_rules = Vec::new();
        let mut anon_names = Vec::new();

        let normalized = match node {
            GrammarNode::Terminal(matcher) => Terminal(Rc::clone(matcher)),

            GrammarNode::Reference(_, name) => {
                let idx = self
                    .rule_names
                    .iter()
                    .position(|&n| n == *name)
                    .unwrap_or_else(|| self.rule_names.len());
                Reference(idx)
            }

            GrammarNode::Sequence(nodes) => {
                let mut normalized_nodes = Vec::new();
                for n in nodes {
                    let (norm_node, mut anon, mut names) = self.normalize_node(n, visited);
                    normalized_nodes.push(norm_node);
                    anon_rules.append(&mut anon);
                    anon_names.append(&mut names);
                }
                Sequence(normalized_nodes)
            }

            GrammarNode::Alternative(nodes) => {
                let mut normalized_nodes = Vec::new();
                for n in nodes {
                    let (norm_node, mut anon, mut names) = self.normalize_node(n, visited);
                    normalized_nodes.push(norm_node);
                    anon_rules.append(&mut anon);
                    anon_names.append(&mut names);
                }
                Alternative(normalized_nodes)
            }

            GrammarNode::Repetition { node, min, max } => {
                let (norm_inner, mut anon, mut names) = self.normalize_node(node, visited);
                anon_rules.append(&mut anon);
                anon_names.append(&mut names);

                let anon_idx = self.rule_names.len() + anon_rules.len();

                let rep_rule = match (min, max) {
                    (0, None) => {
                        let self_ref = Reference(anon_idx);
                        Alternative(vec![Terminal(Rc::new(())), norm_inner + self_ref])
                    }
                    (1, None) => {
                        let self_ref = Reference(anon_idx);
                        Alternative(vec![norm_inner.clone(), norm_inner + self_ref])
                    }
                    (min_count, Some(max_count)) => {
                        let mut alternatives = Vec::new();
                        for count in *min_count..=*max_count {
                            let mut seq_nodes = vec![norm_inner.clone(); count];
                            let alternative = if seq_nodes.is_empty() {
                                Terminal(Rc::new(()))
                            } else {
                                let mut result = seq_nodes.remove(0);
                                for node in seq_nodes {
                                    result = result + node;
                                }
                                result
                            };
                            alternatives.push(alternative);
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
                anon_names.push("");

                Reference(anon_idx)
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
            let node = self.rules[i].clone();
            let alts = match node {
                Alternative(nodes) => nodes,
                n => vec![n],
            };

            let mut betas = Vec::new();
            let mut alphas = Vec::new();

            for alt in alts {
                if let Some(alpha) = Self::strip_left_rec(i, &alt) {
                    alphas.push(alpha);
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
            tail_alts.push(Terminal(Rc::new(())));

            let new_rule = Self::alt_or_single(base_alts);
            let tail_rule = Self::alt_or_single(tail_alts);

            new_rules[i] = new_rule;
            new_rules.push(tail_rule);
            left_rec[i] = Some(LeftRecInfo {
                base: betas,
                tail: alphas,
            });
        }

        self.rules = new_rules;
        self.rule_names = new_names;
        left_rec.resize(self.rules.len(), None);
        self.left_rec = left_rec;
    }

    fn strip_left_rec(
        rule_ix: usize,
        node: &NormalizedGrammarNode,
    ) -> Option<NormalizedGrammarNode> {
        match node {
            Reference(ix) if *ix == rule_ix => Some(Terminal(Rc::new(()))),
            Sequence(nodes) => {
                if nodes.is_empty() {
                    None
                } else if let Reference(ix) = &nodes[0] {
                    if *ix == rule_ix {
                        let rest = &nodes[1..];
                        if rest.is_empty() {
                            Some(Terminal(Rc::new(())))
                        } else if rest.len() == 1 {
                            Some(rest[0].clone())
                        } else {
                            Some(Sequence(rest.to_vec()))
                        }
                    } else {
                        None
                    }
                } else {
                    None
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
