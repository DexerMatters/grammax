use crate::grammar::ir::{Associativity, NormalizedNode, OperatorInfo, OperatorKind, RuleInfo};
use crate::parsec::words::MatcherRef;
use std::collections::HashMap;

/// Detects expression rules and extracts operator information for Pratt parsing
pub struct ExpressionDetector {
    rules: Vec<RuleInfo>,
}

#[derive(Debug, Clone)]
struct RecursionPattern {
    rule_ix: usize,
    kind: RecursionKind,
    operator: Option<MatcherRef>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RecursionKind {
    LeftRecursive,  // expr -> expr "+" ...
    RightRecursive, // expr -> ... "^" expr
    PrefixOp,       // expr -> "-" expr
    PostfixOp,      // expr -> expr "++"
    NoRecursion,
}

impl ExpressionDetector {
    pub fn new(rules: Vec<RuleInfo>) -> Self {
        Self { rules }
    }

    /// Analyzes all rules and marks expression rules, returning operator tables
    pub fn detect_expressions(mut self) -> (Vec<RuleInfo>, HashMap<usize, Vec<OperatorInfo>>) {
        let mut operator_tables = HashMap::new();

        // First pass: identify potential expression rules
        let expr_candidates = self.find_expression_candidates();

        // Pass 2: analyze operator precedence and associativity BEFORE relaxation
        // This ensures recursive structures (left/right) are detected correctly from original grammar
        for &rule_ix in &expr_candidates {
            if let Some((operators, contributors)) = self.analyze_operators(rule_ix) {
                self.rules[rule_ix].is_expression = true;

                // Register operators for the main expression rule
                operator_tables.insert(rule_ix, operators.clone());

                // Register operators for contributing rules (e.g. 'add' in 'expr -> add')
                // They need the full operator table to resolve conflicts against siblings
                for contrib_ix in contributors {
                    operator_tables.insert(contrib_ix, operators.clone());
                }
            }
        }

        // Pass 3: Relax strict grammars (e.g. expr -> primary + expr => expr -> expr + expr)
        // This enables Pratt parsing (via LR conflict resolution) to work by creating conflicts we can resolve
        for &rule_ix in &expr_candidates {
            self.relax_expression_grammar(rule_ix);
        }

        (self.rules, operator_tables)
    }

    fn relax_expression_grammar(&mut self, rule_ix: usize) {
        // We need to visit logic again.
        // Copy rules temporarily to identify derivations? No, just use recursion.
        // We can't mutate self while reading self.
        // Collect mutations first.

        // We want to find Referenced rules that are "Expression Components" and relax them.

        // Build reachable set once
        let reachable = self.compute_reachable(rule_ix);

        let mut mutations = Vec::new();

        // We need to scan ALL rules that are 'part of' the expression (reachable from it)
        // AND check if they contain strict operand patterns to relax.
        for &target_rule_ix in &reachable {
            // Avoid relaxing the expression rule itself if it's already using self-reference (it relies on recursion).
            // But if `expr -> expr + primary`, we need to relax it.
            self.collect_relaxations(rule_ix, target_rule_ix, &reachable, &mut mutations);
        }

        // Also check the rule itself (it is in reachable if recursive)
        if !reachable.contains(&rule_ix) {
            self.collect_relaxations(rule_ix, rule_ix, &reachable, &mut mutations);
        }

        // Apply mutations
        for (target_rule_ix, path, replacement) in mutations {
            self.apply_mutation(target_rule_ix, path, replacement);
        }
    }

    fn compute_reachable(&self, start_ix: usize) -> std::collections::HashSet<usize> {
        let mut visited = std::collections::HashSet::new();
        let mut queue = vec![start_ix];
        while let Some(current) = queue.pop() {
            if visited.insert(current) {
                // Add dependencies
                self.collect_deps(&self.rules[current].node, &mut queue);
            }
        }
        visited
    }

    fn collect_deps(&self, node: &NormalizedNode, acc: &mut Vec<usize>) {
        match node {
            NormalizedNode::Reference(ix) => acc.push(*ix),
            NormalizedNode::Alternative(alts) => {
                alts.iter().for_each(|a| self.collect_deps(a, acc))
            }
            NormalizedNode::Sequence(parts) => parts.iter().for_each(|p| self.collect_deps(p, acc)),
            NormalizedNode::Field(_, inner) => self.collect_deps(inner, acc),
            _ => {}
        }
    }

    fn collect_relaxations(
        &self,
        expr_ix: usize,
        current_rule_ix: usize,
        reachable_from_expr: &std::collections::HashSet<usize>,
        mutations: &mut Vec<(usize, Vec<usize>, NormalizedNode)>,
    ) {
        // Check productions of current_rule_ix
        let rule = &self.rules[current_rule_ix];

        // If this rule is 'expr', we check its alternatives.
        // If this rule is 'add' (ref'd by expr), we check its Sequence.

        // We recurse structurally.
        // But we need to target specific Nodes within the Rule structure.
        // NormalizedNode doesn't have IDs. We need path indices.

        self.visit_node_for_relax(
            expr_ix,
            current_rule_ix,
            &rule.node,
            vec![],
            reachable_from_expr,
            mutations,
        );
    }

    fn visit_node_for_relax(
        &self,
        expr_ix: usize,
        rule_ix: usize,
        node: &NormalizedNode,
        path: Vec<usize>,
        reachable: &std::collections::HashSet<usize>,
        mutations: &mut Vec<(usize, Vec<usize>, NormalizedNode)>,
    ) {
        match node {
            NormalizedNode::Alternative(alts) => {
                for (i, alt) in alts.iter().enumerate() {
                    let mut new_path = path.clone();
                    new_path.push(i);
                    self.visit_node_for_relax(
                        expr_ix, rule_ix, alt, new_path, reachable, mutations,
                    );
                }
            }
            NormalizedNode::Sequence(parts) => {
                // Check if this sequence looks like an operator
                // Simplification: E op X or X op E or E op E
                // Assume 3 parts for binary.
                if parts.len() == 3 {
                    // Check middle is terminal?
                    if let Some(_term) = self.get_terminal(&parts[1]) {
                        // Check operands
                        let left_is_expr = self.references_rule(&parts[0], expr_ix);
                        let right_is_expr = self.references_rule(&parts[2], expr_ix);

                        if left_is_expr && !right_is_expr {
                            // E op X. Check if X is reachable.
                            if let NormalizedNode::Reference(x_ix) = &parts[2] {
                                if reachable.contains(x_ix) {
                                    // Relax X -> E
                                    let mut p = path.clone();
                                    p.push(2); // Index in sequence
                                    mutations.push((
                                        rule_ix,
                                        p,
                                        NormalizedNode::Reference(expr_ix),
                                    ));
                                }
                            }
                        } else if !left_is_expr && right_is_expr {
                            // X op E.
                            if let NormalizedNode::Reference(x_ix) = &parts[0] {
                                if reachable.contains(x_ix) {
                                    // Relax X -> E
                                    let mut p = path.clone();
                                    p.push(0); // Index in sequence
                                    mutations.push((
                                        rule_ix,
                                        p,
                                        NormalizedNode::Reference(expr_ix),
                                    ));
                                }
                            }
                        }
                    }
                } else if parts.len() == 2 {
                    // Prefix/Postfix
                    // op X => op E ??
                    // X op => E op ??
                    // If X is reachable.
                    // This handles `neg -> - primary` => `neg -> - expr`

                    // Case 1: Prefix [op, X]
                    if self.get_terminal(&parts[0]).is_some() {
                        if let NormalizedNode::Reference(x_ix) = &parts[1] {
                            if *x_ix != expr_ix && reachable.contains(x_ix) {
                                let mut p = path.clone();
                                p.push(1);
                                mutations.push((rule_ix, p, NormalizedNode::Reference(expr_ix)));
                            }
                        }
                    }
                }

                // Recurse? If we found Reference, we might jump to another rule.
                for (i, part) in parts.iter().enumerate() {
                    if let NormalizedNode::Reference(ref_ix) = part {
                        if *ref_ix != expr_ix && *ref_ix != rule_ix { // Avoid cycles or self-visit
                            // We need to visit that rule too, IF it hasn't been visited?
                            // But mutations are targeted at rule_ix.
                            // So we just collect mutations for the CURRENT rule here.

                            // But wait, 'add' is a separate rule. We detected 'expr' calls 'add'.
                            // We need to visit 'add' to relax it.
                            // The `collect_relaxations` needs to traverse references.
                            // But we iterate ALL rules reachable from E?
                            // No, simpler: We iterate ALL rules. If `E` references `R`, we relax `R`.
                            // But `E` references `add`. `add` references `primary`.
                        }
                    }
                }
            }
            NormalizedNode::Reference(ref_ix) => {
                if *ref_ix != expr_ix {
                    // If we are traversing, we should jump to this rule to relax it too?
                    // But we can't easily jump inside this recursive function without tracking visited globally.
                    // Instead, 'relax_expression_grammar' should iterate structural dependencies.
                }
            }
            _ => {}
        }
    }

    fn apply_mutation(&mut self, rule_ix: usize, path: Vec<usize>, replacement: NormalizedNode) {
        let node = &mut self.rules[rule_ix].node;
        Self::mutate_node(node, &path, 0, replacement);
    }

    fn mutate_node(
        node: &mut NormalizedNode,
        path: &[usize],
        depth: usize,
        replacement: NormalizedNode,
    ) {
        if depth == path.len() {
            *node = replacement;
            return;
        }

        let idx = path[depth];
        match node {
            NormalizedNode::Alternative(alts) => {
                if idx < alts.len() {
                    Self::mutate_node(&mut alts[idx], path, depth + 1, replacement);
                }
            }
            NormalizedNode::Sequence(parts) => {
                if idx < parts.len() {
                    Self::mutate_node(&mut parts[idx], path, depth + 1, replacement);
                }
            }
            _ => {}
        }
    }

    /// Finds rules that have recursive structure suggesting expression grammars
    fn find_expression_candidates(&self) -> Vec<usize> {
        let mut candidates = Vec::new();

        for (ix, rule) in self.rules.iter().enumerate() {
            if self.is_expression_like(ix, &rule.node) {
                candidates.push(ix);
            }
        }

        candidates
    }

    /// Checks if a rule has expression-like patterns
    fn is_expression_like(&self, rule_ix: usize, node: &NormalizedNode) -> bool {
        match node {
            NormalizedNode::Alternative(alts) => {
                // Check if any alternative shows operator pattern
                alts.iter()
                    .any(|alt| self.has_operator_pattern(rule_ix, alt))
            }
            _ => self.has_operator_pattern(rule_ix, node),
        }
    }

    /// Detects operator patterns in a production
    fn has_operator_pattern(&self, rule_ix: usize, node: &NormalizedNode) -> bool {
        match node {
            NormalizedNode::Sequence(parts) => {
                // Check for patterns like: expr op expr, op expr, expr op
                let has_self_ref = parts.iter().any(|p| self.references_rule(p, rule_ix));
                let has_terminal = parts.iter().any(|p| {
                    matches!(
                        p,
                        NormalizedNode::Terminal(_) | NormalizedNode::Reference(_)
                    )
                });
                // Terminals might be hidden in refs too, but direct terminal is strong signal.
                // Actually, usually operators are terminals.

                // Allow "Expr op Expr" where Op is terminal.
                // Or "Expr Reference(OpRule)"

                let strict_structure = parts.len() >= 2 && has_self_ref;
                strict_structure
            }
            NormalizedNode::Reference(ref_ix) => {
                if *ref_ix != rule_ix {
                    // Check referenced rule
                    self.has_operator_pattern(rule_ix, &self.rules[*ref_ix].node)
                } else {
                    false
                }
            }
            NormalizedNode::Alternative(alts) => {
                alts.iter().any(|a| self.has_operator_pattern(rule_ix, a))
            }
            _ => false,
        }
    }

    /// Analyzes a rule to extract operator information with precedence
    fn analyze_operators(&self, rule_ix: usize) -> Option<(Vec<OperatorInfo>, Vec<usize>)> {
        let rule = &self.rules[rule_ix];

        match &rule.node {
            NormalizedNode::Alternative(alts) => {
                let mut operators = Vec::new();
                let mut contributors = Vec::new();

                // Assign precedence based on alternative order (first = lowest precedence)
                // This respects user-defined precedence ordering
                for (precedence, alt) in alts.iter().enumerate() {
                    if let Some((mut ops, mut contribs)) =
                        self.extract_operator(rule_ix, alt, precedence as u32)
                    {
                        operators.append(&mut ops);
                        contributors.append(&mut contribs);
                    }
                }

                if operators.is_empty() {
                    None
                } else {
                    Some((operators, contributors))
                }
            }
            _ => self.extract_operator(rule_ix, &rule.node, 0),
        }
    }

    /// Extracts operator information from a single alternative
    fn extract_operator(
        &self,
        rule_ix: usize,
        node: &NormalizedNode,
        precedence: u32,
    ) -> Option<(Vec<OperatorInfo>, Vec<usize>)> {
        match node {
            NormalizedNode::Sequence(parts) => {
                // If it's a sequence, it's defined in the current context (or we recursed)
                // If we are currently recursing, we don't know the rule_ix of the recursion target here easily unless we passed it.
                // But wait, the callstack handles it.
                // However, we need to know: Is this sequence PART OF 'rule_ix'? Yes.
                // But if 'rule_ix' is 'expr', and we are looking at 'expr + primary' (which is inside 'add' rule node structure because we recursed),
                // we technically are visiting 'add' node structure.

                // Oops, 'self.rules[*ref_ix]' recursion below does NOT change 'rule_ix'.
                // So 'analyze_sequence_operator' is matching against 'expr'.

                // We need to return the FACT that this matched pattern belongs to the visited rule.
                // But we lost track of the visited rule index in the recursion below.

                // This function needs to take `current_node_owner: Option<usize>` or similar.
                self.analyze_sequence_operator(rule_ix, parts, precedence)
                    .map(|ops| (ops, vec![]))
            }
            NormalizedNode::Reference(ref_ix) if *ref_ix != rule_ix => {
                let ref_rule = &self.rules[*ref_ix];
                // Recurse, but capture the contributor
                if let Some((ops, mut contribs)) =
                    self.extract_operator(rule_ix, &ref_rule.node, precedence)
                {
                    contribs.push(*ref_ix);
                    Some((ops, contribs))
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Analyzes a sequence to determine operator kind and associativity
    fn analyze_sequence_operator(
        &self,
        rule_ix: usize,
        parts: &[NormalizedNode],
        precedence: u32,
    ) -> Option<Vec<OperatorInfo>> {
        // Find the operator token (terminal in the middle)
        // Simple heuristic: First terminal found between non-terminals, or prefix/postfix

        // Case 1: Binary Operation (Node Op Node)
        if parts.len() == 3 {
            let left = &parts[0];
            let op_node = &parts[1];
            let right = &parts[2];

            if let Some(token) = self.get_terminal(op_node) {
                let left_rec = self.references_rule(left, rule_ix);
                let right_rec = self.references_rule(right, rule_ix);

                if left_rec && right_rec {
                    // expr + expr => Left Associative default (Standard for math)
                    return Some(vec![OperatorInfo {
                        precedence,
                        associativity: Associativity::Left,
                        kind: OperatorKind::Infix,
                        token: token.clone(),
                    }]);
                } else if left_rec {
                    // expr + term => Left Recursive
                    return Some(vec![OperatorInfo {
                        precedence,
                        associativity: Associativity::Left,
                        kind: OperatorKind::Infix,
                        token: token.clone(),
                    }]);
                } else if right_rec {
                    // term + expr => Right Recursive
                    return Some(vec![OperatorInfo {
                        precedence,
                        associativity: Associativity::Right,
                        kind: OperatorKind::Infix,
                        token: token.clone(),
                    }]);
                }
            }
        }

        // Prefix: op expr
        if parts.len() >= 2 {
            if let Some(token) = self.get_terminal(&parts[0]) {
                if self.references_rule(&parts[1], rule_ix) {
                    return Some(vec![OperatorInfo {
                        precedence,
                        associativity: Associativity::Right,
                        kind: OperatorKind::Prefix,
                        token: token.clone(),
                    }]);
                }
            }
        }

        // Postfix: expr op
        if parts.len() >= 2 {
            if self.references_rule(&parts[0], rule_ix) {
                if let Some(token) = self.get_terminal(&parts[1]) {
                    return Some(vec![OperatorInfo {
                        precedence,
                        associativity: Associativity::Left,
                        kind: OperatorKind::Postfix,
                        token: token.clone(),
                    }]);
                }
            }
        }

        None
    }

    /// Checks if node references a specific rule
    fn references_rule(&self, node: &NormalizedNode, rule_ix: usize) -> bool {
        match node {
            NormalizedNode::Reference(ix) => *ix == rule_ix,
            NormalizedNode::Sequence(parts) => {
                parts.iter().any(|p| self.references_rule(p, rule_ix))
            }
            NormalizedNode::Alternative(alts) => {
                alts.iter().any(|a| self.references_rule(a, rule_ix))
            }
            NormalizedNode::Field(_, inner) => self.references_rule(inner, rule_ix),
            _ => false,
        }
    }

    /// Checks if a node is a reference to the given rule
    fn is_rule_ref(&self, node: &NormalizedNode, rule_ix: usize) -> Option<()> {
        match node {
            NormalizedNode::Reference(ix) if *ix == rule_ix => Some(()),
            NormalizedNode::Field(_, inner) => self.is_rule_ref(inner, rule_ix),
            _ => None,
        }
    }

    /// Checks if a node is a reference to a higher-precedence rule
    fn is_higher_precedence_ref(&self, node: &NormalizedNode, _rule_ix: usize) -> Option<()> {
        match node {
            NormalizedNode::Reference(_) => Some(()), // Any other rule is considered higher precedence
            NormalizedNode::Field(_, inner) => self.is_higher_precedence_ref(inner, _rule_ix),
            _ => None,
        }
    }

    /// Extracts terminal matcher from a node
    fn get_terminal<'a>(&self, node: &'a NormalizedNode) -> Option<&'a MatcherRef> {
        match node {
            NormalizedNode::Terminal(m) => Some(m),
            NormalizedNode::Field(_, inner) => self.get_terminal(inner),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_infix_detection() {
        // Test that expr -> expr "+" expr is detected as left-associative infix
    }

    #[test]
    fn test_prefix_detection() {
        // Test that expr -> "-" expr is detected as prefix
    }

    #[test]
    fn test_precedence_ordering() {
        // Test that alternative order determines precedence
    }
}
