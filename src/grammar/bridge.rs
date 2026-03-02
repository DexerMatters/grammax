use serde::{Deserialize, Serialize};

use crate::grammar::ir::{Production, Symbol};
use crate::grammar::norm::RuleTable;
use crate::parsec::words::MatcherRef;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BridgeSpec {
    /// Terminal index of the opening delimiter (e.g. `{`, `[`, `(`).
    pub open: usize,
    /// Terminal index of the closing delimiter (e.g. `}`, `]`, `)`).
    pub close: usize,
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

fn is_punctuation_literal(preview: &str) -> bool {
    !preview.is_empty()
        && preview
            .chars()
            .all(|c| !c.is_ascii_alphanumeric() && !c.is_ascii_whitespace())
}

pub fn derive_bridge_specs(table: &RuleTable) -> Vec<BridgeSpec> {
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
        collect_bridges_from_production(prod, is_literal, &mut seen, &mut specs);
    }

    specs
}

fn collect_bridges_from_production(
    prod: &Production,
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
                out.push(BridgeSpec {
                    open: open_idx,
                    close: close_idx,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::new_grammar;
    use crate::parsec::words::{NUMS, STRING};

    #[test]
    fn test_bridge_specs_json() {
        let grammar = new_grammar!(
            json where
            json    -> r!(object) | r!(array) | r!(string) | r!(number) | r!(boolean) | r!(null)
            object  -> tt("{") + sep(r!(pair), tt(",")) + tt("}")
            pair    -> field("key", r!(string)) + tt(":") + field("value", r!(json))
            array   -> tt("[") + sep(r!(json), tt(",")) + tt("]")
            string  -> tt("\"") + t(STRING) + tt("\"")
            number  -> tt(NUMS)
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
    }
}
