use rustc_hash::{FxHashMap, FxHashSet};
use serde::{Deserialize, Serialize};

use crate::grammar::ir::{Production, Symbol};
use crate::grammar::norm::RuleTable;
use crate::parsec::words::MatcherRef;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TerminalPrecedence {
    /// Terminal index to prioritize for insertion.
    pub terminal: usize,
    /// Lookahead terminals for which `terminal` should be preferred.
    pub before: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BridgeSpec {
    /// Terminal index of the opening delimiter (e.g. `{`, `[`, `(`).
    pub open: usize,
    /// Terminal index of the closing delimiter (e.g. `}`, `]`, `)`).
    pub close: usize,
    /// Terminals that can appear anywhere inside the bridged scope.
    pub included: Vec<usize>,
    /// Candidate insertions to prioritize for specific lookahead terminals.
    pub precedence: Vec<TerminalPrecedence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DelimitedTerminalRule {
    /// Opening delimiter terminal index.
    pub open: usize,
    /// Closing delimiter terminal index.
    pub close: usize,
    /// Content terminals that are valid only inside this delimiter pair.
    pub content: Vec<usize>,
}

pub fn derive_recovery_delimiters(table: &RuleTable) -> Vec<usize> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for prod in &table.productions {
        for (idx, sym) in prod.rhs.iter().enumerate() {
            let Symbol::Terminal(term_idx) = sym else {
                continue;
            };
            let Some(preview) = table.terminals[*term_idx].preview() else {
                continue;
            };
            if !is_punctuation_literal(preview) {
                continue;
            }

            let left_is_nt = idx > 0 && matches!(prod.rhs[idx - 1], Symbol::NonTerminal(_));
            let right_is_nt =
                idx + 1 < prod.rhs.len() && matches!(prod.rhs[idx + 1], Symbol::NonTerminal(_));

            if (left_is_nt || right_is_nt) && seen.insert(*term_idx) {
                out.push(*term_idx);
            }
        }
    }

    out
}

/// Derive delimiter rules for "content terminals between delimiters", e.g.
/// `" ... STRING ... "` or `/* ... CONTENT ... */`.
pub fn derive_delimited_terminal_rules(table: &RuleTable) -> Vec<DelimitedTerminalRule> {
    let mut content_by_pair: FxHashMap<(usize, usize), FxHashSet<usize>> = FxHashMap::default();

    for prod in &table.productions {
        let rhs = &prod.rhs;
        for i in 0..rhs.len() {
            let Symbol::Terminal(open) = rhs[i] else {
                continue;
            };
            let Some(open_preview) = table.terminals[open].preview() else {
                continue;
            };
            if !is_punctuation_literal(open_preview) {
                continue;
            }

            for j in (i + 2)..rhs.len() {
                let Symbol::Terminal(close) = rhs[j] else {
                    continue;
                };
                let Some(close_preview) = table.terminals[close].preview() else {
                    continue;
                };
                if !is_punctuation_literal(close_preview) {
                    continue;
                }

                let mut content = Vec::new();
                for sym in &rhs[i + 1..j] {
                    let Symbol::Terminal(term_idx) = sym else {
                        continue;
                    };
                    if table.terminals[*term_idx].preview().is_none() {
                        content.push(*term_idx);
                    }
                }

                if content.is_empty() {
                    continue;
                }

                let entry = content_by_pair.entry((open, close)).or_default();
                entry.extend(content);
            }
        }
    }

    let mut out = content_by_pair
        .into_iter()
        .map(|((open, close), content)| {
            let mut content = content.into_iter().collect::<Vec<_>>();
            content.sort_unstable();
            DelimitedTerminalRule {
                open,
                close,
                content,
            }
        })
        .collect::<Vec<_>>();
    out.sort_by_key(|rule| (rule.open, rule.close));
    out
}

/// Content terminals that should only be considered while inside an inline
/// delimiter scope (derived from grammar structure, not matcher names).
pub fn derive_bracketed_terminals(table: &RuleTable) -> Vec<usize> {
    let mut out = FxHashSet::default();
    for rule in derive_delimited_terminal_rules(table) {
        out.extend(rule.content);
    }
    let mut out = out.into_iter().collect::<Vec<_>>();
    out.sort_unstable();
    out
}

pub fn derive_bracketed_delimiters(table: &RuleTable) -> Vec<usize> {
    let mut out = FxHashSet::default();
    for rule in derive_delimited_terminal_rules(table) {
        out.insert(rule.open);
        out.insert(rule.close);
    }
    let mut out = out.into_iter().collect::<Vec<_>>();
    out.sort_unstable();
    out
}

fn is_punctuation_literal(preview: &str) -> bool {
    !preview.is_empty()
        && preview
            .chars()
            .all(|c| !c.is_ascii_alphanumeric() && !c.is_ascii_whitespace())
}

pub fn derive_bridge_specs(table: &RuleTable) -> Vec<BridgeSpec> {
    let first_sets = compute_first_sets(table);
    let is_literal = |idx: usize| -> bool {
        table
            .terminals
            .get(idx)
            .and_then(|m: &MatcherRef| m.preview())
            .is_some()
    };

    let mut seen = std::collections::HashSet::new();
    let mut specs = Vec::new();

    for prod in &table.productions {
        collect_bridges_from_production(
            prod,
            table,
            &first_sets,
            is_literal,
            &mut seen,
            &mut specs,
        );
    }

    specs
}

fn collect_bridges_from_production(
    prod: &Production,
    table: &RuleTable,
    first_sets: &FirstSets,
    is_literal: impl Fn(usize) -> bool,
    seen: &mut std::collections::HashSet<(usize, usize)>,
    out: &mut Vec<BridgeSpec>,
) {
    let rhs = &prod.rhs;
    let n = rhs.len();

    for i in 0..n {
        let Symbol::Terminal(open_idx) = rhs[i] else {
            continue;
        };
        if !is_literal(open_idx) {
            continue;
        }

        for j in (i + 1)..n {
            let Symbol::Terminal(close_idx) = rhs[j] else {
                continue;
            };
            if !is_literal(close_idx) {
                continue;
            }
            // Same terminal on both sides → not a bracket pair
            if open_idx == close_idx {
                continue;
            }
            // Must have at least one NonTerminal between i and j
            let has_nonterminal = rhs[i + 1..j]
                .iter()
                .any(|s| matches!(s, Symbol::NonTerminal(_)));
            if !has_nonterminal {
                continue;
            }

            if seen.insert((open_idx, close_idx)) {
                let body = &rhs[i + 1..j];
                out.push(BridgeSpec {
                    open: open_idx,
                    close: close_idx,
                    included: collect_included_terminals(table, body),
                    precedence: collect_precedence_rules(table, first_sets, body),
                });
            }
        }
    }
}

type FirstSets = FxHashMap<usize, FxHashSet<Option<usize>>>;

fn compute_first_sets(table: &RuleTable) -> FirstSets {
    let mut first_sets: FirstSets = FxHashMap::default();
    let mut changed = true;

    while changed {
        changed = false;

        for prod in &table.productions {
            let mut nullable = true;

            for sym in &prod.rhs {
                match sym {
                    Symbol::Terminal(term_ix) => {
                        changed |= first_sets
                            .entry(prod.lhs)
                            .or_default()
                            .insert(Some(*term_ix));
                        nullable = false;
                        break;
                    }
                    Symbol::NonTerminal(rule_ix) => {
                        let nested = first_sets.get(rule_ix).cloned().unwrap_or_default();
                        let mut has_epsilon = false;
                        for term in nested {
                            match term {
                                Some(term_ix) => {
                                    changed |= first_sets
                                        .entry(prod.lhs)
                                        .or_default()
                                        .insert(Some(term_ix));
                                }
                                None => has_epsilon = true,
                            }
                        }
                        if !has_epsilon {
                            nullable = false;
                            break;
                        }
                    }
                }
            }

            if nullable {
                changed |= first_sets.entry(prod.lhs).or_default().insert(None);
            }
        }
    }

    first_sets
}

fn first_of_sequence(symbols: &[Symbol], first_sets: &FirstSets) -> (FxHashSet<usize>, bool) {
    let mut terminals = FxHashSet::default();
    let mut nullable = true;

    for sym in symbols {
        match sym {
            Symbol::Terminal(term_ix) => {
                terminals.insert(*term_ix);
                nullable = false;
                break;
            }
            Symbol::NonTerminal(rule_ix) => {
                let nested = first_sets.get(rule_ix).cloned().unwrap_or_default();
                let mut has_epsilon = false;
                for term in nested {
                    match term {
                        Some(term_ix) => {
                            terminals.insert(term_ix);
                        }
                        None => has_epsilon = true,
                    }
                }
                if !has_epsilon {
                    nullable = false;
                    break;
                }
            }
        }
    }

    (terminals, nullable)
}

fn collect_included_terminals(table: &RuleTable, body: &[Symbol]) -> Vec<usize> {
    let mut included = FxHashSet::default();
    let mut queue = Vec::new();
    let mut visited = FxHashSet::default();

    for sym in body {
        match sym {
            Symbol::Terminal(term_ix) => {
                included.insert(*term_ix);
            }
            Symbol::NonTerminal(rule_ix) => queue.push(*rule_ix),
        }
    }

    while let Some(rule_ix) = queue.pop() {
        if !visited.insert(rule_ix) {
            continue;
        }

        for prod in table.productions.iter().filter(|prod| prod.lhs == rule_ix) {
            for sym in &prod.rhs {
                match sym {
                    Symbol::Terminal(term_ix) => {
                        included.insert(*term_ix);
                    }
                    Symbol::NonTerminal(next_rule_ix) => queue.push(*next_rule_ix),
                }
            }
        }
    }

    let mut included = included.into_iter().collect::<Vec<_>>();
    included.sort_unstable();
    included
}

fn collect_precedence_rules(
    table: &RuleTable,
    first_sets: &FirstSets,
    body: &[Symbol],
) -> Vec<TerminalPrecedence> {
    let mut precedence: FxHashMap<usize, FxHashSet<usize>> = FxHashMap::default();
    let mut queue = Vec::new();
    let mut visited = FxHashSet::default();

    collect_precedence_from_sequence(body, first_sets, &mut precedence);

    for sym in body {
        if let Symbol::NonTerminal(rule_ix) = sym {
            queue.push(*rule_ix);
        }
    }

    while let Some(rule_ix) = queue.pop() {
        if !visited.insert(rule_ix) {
            continue;
        }

        for prod in table.productions.iter().filter(|prod| prod.lhs == rule_ix) {
            collect_precedence_from_sequence(&prod.rhs, first_sets, &mut precedence);
            for sym in &prod.rhs {
                if let Symbol::NonTerminal(next_rule_ix) = sym {
                    queue.push(*next_rule_ix);
                }
            }
        }
    }

    let mut precedence = precedence
        .into_iter()
        .map(|(terminal, before)| {
            let mut before = before.into_iter().collect::<Vec<_>>();
            before.sort_unstable();
            TerminalPrecedence { terminal, before }
        })
        .collect::<Vec<_>>();
    precedence.sort_by_key(|rule| rule.terminal);
    precedence
}

fn collect_precedence_from_sequence(
    symbols: &[Symbol],
    first_sets: &FirstSets,
    precedence: &mut FxHashMap<usize, FxHashSet<usize>>,
) {
    for idx in 0..symbols.len() {
        let Symbol::Terminal(term_ix) = symbols[idx] else {
            continue;
        };

        let (followers, _) = first_of_sequence(&symbols[idx + 1..], first_sets);
        if followers.is_empty() {
            continue;
        }

        precedence.entry(term_ix).or_default().extend(followers);
    }
}

#[cfg(test)]
mod tests {
    use crate::new_grammar;

    #[test]
    fn test_bridge_specs_json() {
        let grammar = new_grammar!(
            json where
            json    -> r!(object) | r!(array) | r!(string) | r!(number) | r!(boolean) | r!(null)
            object  -> tt("{") + sep(r!(pair), tt(",")) + tt("}")
            pair    -> field("key", r!(string)) + tt(":") + field("value", r!(json))
            array   -> tt("[") + sep(r!(json), tt(",")) + tt("]")
            string  -> tt("\"") + t(STRING) + tt("\"")
            number  -> tt(NUMBER)
            boolean -> tt("true") | tt("false")
            null    -> tt("null")
        );

        // Object "{…}" and array "[…]" should both be detected.
        let specs = &grammar.bridge_specs;
        let has = |o: &str, c: &str| {
            specs.iter().any(|s| {
                grammar.table.terminals[s.open].preview() == Some(o)
                    && grammar.table.terminals[s.close].preview() == Some(c)
            })
        };

        assert!(has("{", "}"), "expected {{…}} bridge");
        assert!(has("[", "]"), "expected […] bridge");
        // Comma-separated internals should not produce spurious pairs
        assert!(!has(",", "}"), "comma→}} is not a bridge");

        let object_spec = specs
            .iter()
            .find(|spec| {
                grammar.table.terminals[spec.open].preview() == Some("{")
                    && grammar.table.terminals[spec.close].preview() == Some("}")
            })
            .expect("expected object bridge spec");

        let quote_ix = grammar
            .table
            .terminals
            .iter()
            .position(|matcher| matcher.preview() == Some("\""))
            .expect("quote terminal");
        let comma_ix = grammar
            .table
            .terminals
            .iter()
            .position(|matcher| matcher.preview() == Some(","))
            .expect("comma terminal");
        let colon_ix = grammar
            .table
            .terminals
            .iter()
            .position(|matcher| matcher.preview() == Some(":"))
            .expect("colon terminal");

        assert!(object_spec.included.contains(&quote_ix));
        assert!(
            object_spec
                .precedence
                .iter()
                .any(|rule| { rule.terminal == comma_ix && rule.before.contains(&quote_ix) })
        );
        assert!(
            object_spec
                .precedence
                .iter()
                .any(|rule| { rule.terminal == colon_ix && rule.before.contains(&quote_ix) })
        );
    }
}
