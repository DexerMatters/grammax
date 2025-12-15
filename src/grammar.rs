use core::fmt;
use std::{collections::HashSet, ops};

use crate::words::{EndOfInput, Matcher, StartOfInput};

#[derive(Debug, Clone, Copy)]
pub enum GrammarError {
    UndecidableRule(&'static str),
    AlwaysFails,
}

pub type Result<T> = std::result::Result<T, GrammarError>;

pub type Rule = fn() -> GrammarNode;

pub enum GrammarNode {
    Terminal(Box<dyn Matcher>),
    Choice(Vec<GrammarNode>),
    Sequence(Vec<GrammarNode>),
    Reference(Rule, &'static str),
}

pub(crate) enum _GrammarNode {
    Terminal(Box<dyn Matcher>),
    Choice(Vec<_GrammarNode>),
    Sequence(Vec<_GrammarNode>),
    Tagged(usize, Box<_GrammarNode>),
    Mu(usize),
}

pub struct Grammar {
    nodes: Option<_GrammarNode>,
    rules: Vec<_GrammarNode>,
    tags: Vec<&'static str>,
}

impl Grammar {
    pub fn new(grammar: GrammarNode) -> Result<Self> {
        let mut g = Grammar {
            nodes: None,
            rules: Vec::new(),
            tags: Vec::new(),
        };
        g.normalize(grammar);

        if let Some(rule_idx) = g.undecidable_rule() {
            return Err(GrammarError::UndecidableRule(g.tags[rule_idx]));
        }

        Ok(g)
    }

    fn normalize(&mut self, g: GrammarNode) {
        if self.nodes.is_some() {
            return;
        }

        use std::collections::HashMap;

        let mut rule_map: HashMap<usize, usize> = HashMap::new();
        let mut stack: HashSet<usize> = HashSet::new();

        fn helper(
            g: GrammarNode,
            rules: &mut Vec<_GrammarNode>,
            tags: &mut Vec<&'static str>,
            rule_map: &mut HashMap<usize, usize>,
            stack: &mut HashSet<usize>,
        ) -> _GrammarNode {
            match g {
                GrammarNode::Terminal(matcher) => _GrammarNode::Terminal(matcher),
                GrammarNode::Choice(choices) => {
                    let normalized_choices: Vec<_GrammarNode> = choices
                        .into_iter()
                        .map(|c| helper(c, rules, tags, rule_map, stack))
                        .collect();
                    _GrammarNode::Choice(normalized_choices)
                }
                GrammarNode::Sequence(seq) => {
                    let normalized_seq: Vec<_GrammarNode> = seq
                        .into_iter()
                        .map(|s| helper(s, rules, tags, rule_map, stack))
                        .collect();
                    _GrammarNode::Sequence(normalized_seq)
                }
                GrammarNode::Reference(rule, name) => {
                    let rule_ptr = rule as usize;

                    // Check if we're in a recursive loop
                    if stack.contains(&rule_ptr) {
                        let rule_index = *rule_map
                            .get(&rule_ptr)
                            .expect("Rule should be in map if it's on the stack");
                        return _GrammarNode::Mu(rule_index);
                    }

                    // Check if we've already processed this rule
                    if let Some(&rule_index) = rule_map.get(&rule_ptr) {
                        return _GrammarNode::Mu(rule_index);
                    }

                    // Allocate a new rule index
                    let rule_index = rules.len();
                    rule_map.insert(rule_ptr, rule_index);
                    tags.push(name);

                    // Reserve space in rules vector with a placeholder
                    // We use EndOfInput as a temporary placeholder
                    rules.push(_GrammarNode::Terminal(Box::new(EndOfInput)));

                    // Mark this rule as being processed (on the stack)
                    stack.insert(rule_ptr);

                    // Expand the rule first to get its content
                    let expanded = helper(rule(), rules, tags, rule_map, stack);

                    // Remove from stack after processing
                    stack.remove(&rule_ptr);

                    // Update the rule in the rules vector
                    let tagged = _GrammarNode::Tagged(rule_index, Box::new(expanded));
                    rules[rule_index] = tagged;

                    _GrammarNode::Mu(rule_index)
                }
            }
        }

        let node = helper(
            g,
            &mut self.rules,
            &mut self.tags,
            &mut rule_map,
            &mut stack,
        );
        self.nodes = Some(node);
    }

    fn undecidable_rule(&self) -> Option<usize> {
        let n = self.rules.len();
        let mut nullable = vec![false; n];
        let mut changed = true;

        // Fixpoint computation for nullable
        while changed {
            changed = false;
            for (i, rule) in self.rules.iter().enumerate() {
                if !nullable[i] {
                    if self.is_nullable(rule, &nullable) {
                        nullable[i] = true;
                        changed = true;
                    }
                }
            }
        }

        // Cycle detection
        let mut visited = vec![false; n];
        let mut stack = vec![false; n];

        for i in 0..n {
            if self.find_cycle(i, &nullable, &mut visited, &mut stack) {
                return Some(i);
            }
        }
        None
    }

    fn is_nullable(&self, node: &_GrammarNode, nullable_rules: &[bool]) -> bool {
        match node {
            _GrammarNode::Terminal(m) => m.is_nullable(),
            _GrammarNode::Choice(opts) => opts.iter().any(|o| self.is_nullable(o, nullable_rules)),
            _GrammarNode::Sequence(seq) => seq.iter().all(|s| self.is_nullable(s, nullable_rules)),
            _GrammarNode::Tagged(_, inner) => self.is_nullable(inner, nullable_rules),
            _GrammarNode::Mu(idx) => nullable_rules[*idx],
        }
    }

    fn find_cycle(
        &self,
        idx: usize,
        nullable_rules: &[bool],
        visited: &mut [bool],
        stack: &mut [bool],
    ) -> bool {
        if stack[idx] {
            return true;
        }
        if visited[idx] {
            return false;
        }

        visited[idx] = true;
        stack[idx] = true;

        let calls = self.get_non_consuming_calls(&self.rules[idx], nullable_rules);
        for target in calls {
            if self.find_cycle(target, nullable_rules, visited, stack) {
                return true;
            }
        }

        stack[idx] = false;
        false
    }

    fn get_non_consuming_calls(&self, node: &_GrammarNode, nullable_rules: &[bool]) -> Vec<usize> {
        let mut calls = Vec::new();
        match node {
            _GrammarNode::Terminal(_) => {}
            _GrammarNode::Choice(opts) => {
                for o in opts {
                    calls.extend(self.get_non_consuming_calls(o, nullable_rules));
                }
            }
            _GrammarNode::Sequence(seq) => {
                for item in seq {
                    calls.extend(self.get_non_consuming_calls(item, nullable_rules));
                    if !self.is_nullable(item, nullable_rules) {
                        break;
                    }
                }
            }
            _GrammarNode::Tagged(_, inner) => {
                calls.extend(self.get_non_consuming_calls(inner, nullable_rules))
            }
            _GrammarNode::Mu(idx) => calls.push(*idx),
        }
        calls
    }
}

impl fmt::Display for Grammar {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Helper function to format _GrammarNode
        fn format_node(
            node: &_GrammarNode,
            f: &mut fmt::Formatter<'_>,
            tags: &[&'static str],
            in_sequence: bool,
        ) -> fmt::Result {
            match node {
                _GrammarNode::Terminal(matcher) => {
                    write!(f, "{}", matcher.display())
                }
                _GrammarNode::Choice(choices) => {
                    if in_sequence {
                        write!(f, "( ")?;
                    }
                    for (i, choice) in choices.iter().enumerate() {
                        if i > 0 {
                            write!(f, " | ")?;
                        }
                        format_node(choice, f, tags, false)?;
                    }
                    if in_sequence {
                        write!(f, " )")?;
                    }
                    Ok(())
                }
                _GrammarNode::Sequence(seq) => {
                    for (i, item) in seq.iter().enumerate() {
                        if i > 0 {
                            write!(f, " ")?;
                        }
                        format_node(item, f, tags, true)?;
                    }
                    Ok(())
                }
                _GrammarNode::Tagged(idx, _inner) => write!(f, "<{}>", tags[*idx]),
                _GrammarNode::Mu(idx) => write!(f, "<{}>", tags[*idx]),
            }
        }

        // Write the main grammar if it exists
        if let Some(ref node) = self.nodes {
            format_node(node, f, &self.tags, false)?;
            writeln!(f)?;
        }

        // Write all the rules in BNF format
        for rule in self.rules.iter() {
            // Extract the tag index and inner content from Tagged wrapper
            if let _GrammarNode::Tagged(tag_idx, inner) = rule {
                write!(f, "<{}> ::= ", self.tags[*tag_idx])?;
                format_node(inner, f, &self.tags, false)?;
                writeln!(f)?;
            } else {
                // This shouldn't happen, but handle it gracefully
                format_node(rule, f, &self.tags, false)?;
                writeln!(f)?;
            }
        }

        Ok(())
    }
}

#[inline]
pub fn t<M>(s: M) -> GrammarNode
where
    M: Matcher + 'static,
{
    GrammarNode::Terminal(Box::new(s))
}

#[macro_export]
macro_rules! r {
    ($f:expr) => {
        GrammarNode::Reference($f, stringify!($f))
    };
}

#[inline]
pub fn r_named(f: fn() -> GrammarNode, name: &'static str) -> GrammarNode {
    GrammarNode::Reference(f, name)
}

#[inline]
pub fn end() -> GrammarNode {
    t(EndOfInput)
}

#[inline]
pub fn start() -> GrammarNode {
    t(StartOfInput)
}

#[inline]
pub fn choice<I>(choices: I) -> GrammarNode
where
    I: IntoIterator<Item = GrammarNode>,
{
    GrammarNode::Choice(choices.into_iter().collect())
}

#[inline]
pub fn seq<I>(seq: I) -> GrammarNode
where
    I: IntoIterator<Item = GrammarNode>,
{
    GrammarNode::Sequence(seq.into_iter().collect())
}

impl ops::Add for GrammarNode {
    type Output = GrammarNode;

    fn add(self, rhs: GrammarNode) -> GrammarNode {
        match (self, rhs) {
            (GrammarNode::Sequence(mut left_seq), GrammarNode::Sequence(right_seq)) => {
                left_seq.extend(right_seq);
                GrammarNode::Sequence(left_seq)
            }
            (GrammarNode::Sequence(mut left_seq), right) => {
                left_seq.push(right);
                GrammarNode::Sequence(left_seq)
            }
            (left, GrammarNode::Sequence(mut right_seq)) => {
                let mut new_seq = vec![left];
                new_seq.append(&mut right_seq);
                GrammarNode::Sequence(new_seq)
            }
            (left, right) => GrammarNode::Sequence(vec![left, right]),
        }
    }
}

impl ops::BitOr for GrammarNode {
    type Output = GrammarNode;

    fn bitor(self, rhs: GrammarNode) -> GrammarNode {
        match (self, rhs) {
            (GrammarNode::Choice(mut left_choices), GrammarNode::Choice(right_choices)) => {
                left_choices.extend(right_choices);
                GrammarNode::Choice(left_choices)
            }
            (GrammarNode::Choice(mut left_choices), right) => {
                left_choices.push(right);
                GrammarNode::Choice(left_choices)
            }
            (left, GrammarNode::Choice(mut right_choices)) => {
                let mut new_choices = vec![left];
                new_choices.append(&mut right_choices);
                GrammarNode::Choice(new_choices)
            }
            (left, right) => GrammarNode::Choice(vec![left, right]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_grammar() {
        fn a() -> GrammarNode {
            t("a") + r!(a) + end() | r!(b)
        }

        fn b() -> GrammarNode {
            r![a] | t("b") | t("c")
        }

        let grammar = Grammar::new(r!(a));
        match grammar {
            Ok(g) => println!("{}", g),
            Err(e) => println!("Error: {:?}", e),
        }
    }
}
