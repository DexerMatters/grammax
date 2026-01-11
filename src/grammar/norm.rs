use std::{collections::HashSet, fmt, ops, rc::Rc};

use crate::{grammar::dsl::GrammarNode, words::Matcher};

#[derive(Clone, Debug)]
pub enum NormalizedGrammarNode {
    Terminal(Rc<dyn Matcher>),
    Alternative(Vec<NormalizedGrammarNode>),
    Sequence(Vec<NormalizedGrammarNode>),
    Reference(usize),
}

use NormalizedGrammarNode::*;

impl ops::Add for NormalizedGrammarNode {
    type Output = NormalizedGrammarNode;

    fn add(self, other: NormalizedGrammarNode) -> NormalizedGrammarNode {
        match (self, other) {
            (Sequence(mut seq1), Sequence(seq2)) => {
                seq1.extend(seq2);
                Sequence(seq1)
            }
            (Sequence(mut seq), node) => {
                seq.push(node);
                Sequence(seq)
            }
            (node, Sequence(mut seq)) => {
                seq.insert(0, node);
                Sequence(seq)
            }
            (node1, node2) => Sequence(vec![node1, node2]),
        }
    }
}

impl ops::BitOr for NormalizedGrammarNode {
    type Output = NormalizedGrammarNode;

    fn bitor(self, other: NormalizedGrammarNode) -> NormalizedGrammarNode {
        match (self, other) {
            (Alternative(mut alt1), Alternative(alt2)) => {
                alt1.extend(alt2);
                Alternative(alt1)
            }
            (Alternative(mut alt), node) | (node, Alternative(mut alt)) => {
                alt.insert(0, node);
                Alternative(alt)
            }
            (node1, node2) => Alternative(vec![node1, node2]),
        }
    }
}

impl fmt::Display for NormalizedGrammarNode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Terminal(matcher) => {
                write!(f, "{}", matcher.display())
            }
            Reference(index) => {
                write!(f, "@{}", index)
            }
            Sequence(nodes) => {
                let parts: Vec<String> = nodes.iter().map(|n| n.to_string()).collect();
                write!(f, "({})", parts.join(" "))
            }
            Alternative(nodes) => {
                let parts: Vec<String> = nodes.iter().map(|n| n.to_string()).collect();
                write!(f, "({})", parts.join(" | "))
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct RuleTable {
    pub rule_names: Vec<&'static str>,
    pub rules: Vec<NormalizedGrammarNode>,
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
        if idx < self.rule_names.len() {
            let n = self.rule_names[idx];
            if n.is_empty() {
                format!("@{}", idx)
            } else {
                n.to_string()
            }
        } else {
            format!("@{}", idx)
        }
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
        Self {
            rule_names: vec![""; initial_rules.len()],
            rules: initial_rules,
        }
    }

    pub fn compute_from(&mut self, start: GrammarNode, start_name: &'static str) {
        let mut rule_names: Vec<&'static str> = vec![start_name];
        let mut rules: Vec<NormalizedGrammarNode> = Vec::new();
        let mut rule_map: std::collections::HashMap<&'static str, GrammarNode> =
            std::collections::HashMap::new();

        // Store the start rule
        rule_map.insert(start_name, start.clone());

        // First pass: discover all referenced rule names and their nodes
        let mut discovered_refs: HashSet<&'static str> = HashSet::new();
        self.discover_and_collect_references(&start, &mut discovered_refs, &mut rule_map);

        // Add discovered references to rule_names in order
        for name in discovered_refs.iter() {
            if *name != start_name && !rule_names.contains(name) {
                rule_names.push(name);
            }
        }

        // Set rule_names before normalizing so indices can be computed correctly
        self.rule_names = rule_names.clone();

        // Now normalize each rule in order
        let mut visited: HashSet<&'static str> = HashSet::new();

        for rule_name in rule_names.iter() {
            if let Some(grammar_node) = rule_map.get(rule_name) {
                let (normalized, anon_rules, anon_names) =
                    self.normalize_node(grammar_node, &mut visited);
                rules.push(normalized);

                // Add anonymous rules created from Repetition
                for (name, node) in anon_names.into_iter().zip(anon_rules.into_iter()) {
                    self.rule_names.push(name);
                    rules.push(node);
                }
            }
        }

        self.rules = rules;
        self.optimize_relay_nodes();
    }

    fn optimize_relay_nodes(&mut self) {
        let max_iterations = 10; // Prevent infinite loops

        for _ in 0..max_iterations {
            let mut changed = false;

            // Find and inline one level of relays
            for i in 0..self.rules.len() {
                if let Reference(target_idx) = &self.rules[i] {
                    let target_idx = *target_idx;
                    // Check if this is a relay we should inline
                    let should_inline = (i < self.rule_names.len()
                        && self.rule_names[i].is_empty())
                        || (target_idx < self.rule_names.len()
                            && self.rule_names[target_idx].is_empty());

                    if should_inline && target_idx < self.rules.len() {
                        // Replace this relay with the target's content, remapping self-references
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
        discovered: &mut HashSet<&'static str>,
        rule_map: &mut std::collections::HashMap<&'static str, GrammarNode>,
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
        visited: &mut HashSet<&'static str>,
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
                // Find the index of this rule by name
                let idx = self
                    .rule_names
                    .iter()
                    .position(|&n| n == *name)
                    .unwrap_or_else(|| {
                        // If not found yet, this will be assigned when processing
                        // We'll add it to the end for now
                        self.rule_names.len()
                    });
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
                // For repetitions, create an anonymous rule that handles the recursion
                let (norm_inner, mut anon, mut names) = self.normalize_node(node, visited);
                anon_rules.append(&mut anon);
                anon_names.append(&mut names);

                // Create the anonymous rule index
                let anon_idx = self.rule_names.len() + anon_rules.len();

                // Generate the repetition rule
                // If min = 0, max = None: A* (many) -> ε | A A*
                // If min = 1, max = None: A+ (some) -> A | A A+
                // If min = m, max = Some(n): A{m,n} -> appropriate structure

                let rep_rule = match (min, max) {
                    (0, None) => {
                        // A* -> ε | A A*
                        // We'll represent ε as empty sequence and create the recursion
                        let self_ref = Reference(anon_idx);
                        Alternative(vec![
                            Terminal(Rc::new(())), // ε
                            norm_inner + self_ref, // A A*
                        ])
                    }
                    (1, None) => {
                        // A+ -> A | A A+
                        let self_ref = Reference(anon_idx);
                        Alternative(vec![norm_inner.clone(), norm_inner + self_ref])
                    }
                    (min_count, Some(max_count)) => {
                        // A{m,n} - build alternatives for each valid count
                        let mut alternatives = Vec::new();
                        for count in *min_count..=*max_count {
                            let mut seq_nodes = vec![norm_inner.clone(); count];
                            let alternative = if seq_nodes.is_empty() {
                                Terminal(Rc::new(())) // ε
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
                        // A{m,} -> A...A A*  (m times, then A*)
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
}
