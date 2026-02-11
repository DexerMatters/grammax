use std::fmt::Write;

use rustc_hash::{FxHashMap, FxHashSet};

use crate::grammar::Grammar;
use crate::grammar::analysis::EOF_TOKEN;
use crate::grammar::ir::Symbol;
use crate::parsec::msg::{ErrorMessage, ParserMessage};
use crate::parsec::tree::{GreenId, RedNode, Tag, TreeAllocRef, TreeAllocRefExt};

const RESET: &str = "\x1b[0m";
const DIM: &str = "\x1b[2m";
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const RED: &str = "\x1b[31m";

pub fn format_ast(grammar: &Grammar, root: &RedNode, alloc: &TreeAllocRef, source: &str) -> String {
    let mut out = String::new();
    let mut stack = Vec::new();
    display_with_indent(
        grammar,
        alloc,
        source,
        root.green,
        "",
        true,
        true,
        root.offset,
        &mut out,
        &mut stack,
    );
    out
}

pub fn format_messages(grammar: &Grammar, messages: &[ParserMessage]) -> String {
    let mut out = String::new();

    for (idx, msg) in messages.iter().enumerate() {
        if idx > 0 {
            out.push('\n');
        }

        match &msg.message {
            ErrorMessage::UnexpectedToken { expected } => {
                let expected = format_expected(grammar, expected);
                let _ = write!(
                    out,
                    "{}Unexpected Token{} at [{}, {}]{}",
                    RED, RESET, msg.span.start, msg.span.end, expected
                );
            }
            ErrorMessage::MissingToken { expected } => {
                let expected = format_expected(grammar, expected);
                let _ = write!(
                    out,
                    "{}Missing Token{} at [{}, {}]{}",
                    RED, RESET, msg.span.start, msg.span.end, expected
                );
            }
            ErrorMessage::Custom(code) => {
                let _ = write!(
                    out,
                    "{}Error{} [{}] at [{}, {}]",
                    RED, RESET, code, msg.span.start, msg.span.end
                );
            }
        }
    }

    out
}

fn display_with_indent(
    grammar: &Grammar,
    alloc: &TreeAllocRef,
    source: &str,
    id: GreenId,
    prefix: &str,
    is_last: bool,
    is_root: bool,
    offset: usize,
    out: &mut String,
    stack: &mut Vec<GreenId>,
) {
    if stack.contains(&id) {
        let node = alloc.get_node(id);
        let (label, extra) = format_label(grammar, alloc, source, id, offset);
        let width = format!(" {}[width: {}]{}", DIM, node.width, RESET);

        let branch = if is_root {
            ""
        } else if is_last {
            "└─ "
        } else {
            "├─ "
        };

        let _ = write!(
            out,
            "{}{}{}{}{}{} {}↻{}",
            prefix, branch, label, width, extra, " ", DIM, RESET
        );
        return;
    }

    stack.push(id);
    let node = alloc.get_node(id);
    if is_silent_token(alloc, id) {
        stack.pop();
        return;
    }

    let branch = if is_root {
        ""
    } else if is_last {
        "└─ "
    } else {
        "├─ "
    };

    if let Tag::Field { name, .. } = &node.tag {
        if let Some(&child_id) = node.children.first() {
            let child = alloc.get_node(child_id);
            let marker = format!("{}{}:{} ", YELLOW, name, RESET);
            let (child_label, child_extra) = format_label(grammar, alloc, source, child_id, offset);
            let label = format!("{}{}", marker, child_label);
            let width = format!(" {}[width: {}]{}", DIM, node.width, RESET);

            let _ = write!(out, "{}{}{}{}{}", prefix, branch, label, width, child_extra);

            if child.children.is_empty() {
                stack.pop();
                return;
            }

            out.push('\n');

            let child_prefix = if prefix.is_empty() {
                if is_last {
                    "   ".to_string()
                } else {
                    "│  ".to_string()
                }
            } else if is_last {
                format!("{}   ", prefix)
            } else {
                format!("{}│  ", prefix)
            };

            let visible_grandchildren: Vec<GreenId> = child
                .children
                .iter()
                .copied()
                .filter(|child_id| !is_silent_token(alloc, *child_id))
                .collect();
            let mut running_offset = offset;
            for (idx, &grandchild_id) in visible_grandchildren.iter().enumerate() {
                let last = idx + 1 == visible_grandchildren.len();
                display_with_indent(
                    grammar,
                    alloc,
                    source,
                    grandchild_id,
                    &child_prefix,
                    last,
                    false,
                    running_offset,
                    out,
                    stack,
                );
                if !last {
                    out.push('\n');
                }
                running_offset += alloc.get_node(grandchild_id).width;
            }
            stack.pop();
            return;
        }
    }

    let (label, extra) = format_label(grammar, alloc, source, id, offset);
    let width = format!(" {}[width: {}]{}", DIM, node.width, RESET);

    let _ = write!(out, "{}{}{}{}{}", prefix, branch, label, width, extra);

    if node.children.is_empty() {
        stack.pop();
        return;
    }

    out.push('\n');

    let child_prefix = if prefix.is_empty() {
        if is_last {
            "   ".to_string()
        } else {
            "│  ".to_string()
        }
    } else if is_last {
        format!("{}   ", prefix)
    } else {
        format!("{}│  ", prefix)
    };

    let visible_children: Vec<GreenId> = node
        .children
        .iter()
        .copied()
        .filter(|child_id| !is_silent_token(alloc, *child_id))
        .collect();
    let mut child_offset = offset;
    for (idx, &child_id) in visible_children.iter().enumerate() {
        let last = idx + 1 == visible_children.len();
        display_with_indent(
            grammar,
            alloc,
            source,
            child_id,
            &child_prefix,
            last,
            false,
            child_offset,
            out,
            stack,
        );
        if !last {
            out.push('\n');
        }
        child_offset += alloc.get_node(child_id).width;
    }

    stack.pop();
}

fn is_silent_token(alloc: &TreeAllocRef, id: GreenId) -> bool {
    let node = alloc.get_node(id);
    matches!(node.tag, Tag::Token { .. }) && node.children.is_empty() && node.width == 0
}

fn format_label(
    grammar: &Grammar,
    alloc: &TreeAllocRef,
    source: &str,
    id: GreenId,
    offset: usize,
) -> (String, String) {
    let node = alloc.get_node(id);
    match &node.tag {
        Tag::Rule { rule_ix } => {
            let name = grammar.name(*rule_ix);
            if name.starts_with('@') {
                (String::new(), String::new())
            } else {
                (name.to_string(), String::new())
            }
        }
        Tag::Token { .. } => {
            if node.children.is_empty() {
                let end = offset.saturating_add(node.width).min(source.len());
                let slice = source.get(offset..end).unwrap_or("");
                let text = pretty_string(slice.to_string());
                (format!("{}{}{}", GREEN, text, RESET), String::new())
            } else {
                (String::new(), String::new())
            }
        }
        Tag::Field { name, .. } => (format!("{}{}:{}", YELLOW, name, RESET), String::new()),
        Tag::Error(errors) => {
            let err_desc = errors
                .iter()
                .map(|e| format!("{:?}", e))
                .collect::<Vec<_>>()
                .join(", ");
            (
                format!("{}error:[{}]{}", RED, err_desc, RESET),
                String::new(),
            )
        }
        Tag::Root => ("ROOT".to_string(), String::new()),
    }
}

fn format_expected(grammar: &Grammar, expected: &[usize]) -> String {
    if expected.is_empty() {
        return String::new();
    }

    let expected_terms: FxHashSet<usize> = expected.iter().copied().filter(|id| *id != EOF_TOKEN).collect();
    let first_sets = compute_first_sets(grammar);
    let mut exact_rules = Vec::new();
    for (rule_ix, firsts) in &first_sets {
        let terms: Vec<usize> = firsts.iter().copied().flatten().collect();
        if terms.is_empty() {
            continue;
        }
        let name = grammar.name(*rule_ix);
        if terms.len() == expected_terms.len()
            && terms.iter().all(|t| expected_terms.contains(t))
            && !name.starts_with('@')
            && !name.starts_with('$')
        {
            exact_rules.push(*rule_ix);
        }
    }

    let mut names = Vec::new();
    if !exact_rules.is_empty() {
        exact_rules.sort_unstable();
        exact_rules.dedup();
        for rule_ix in exact_rules {
            names.push(format!("rule#{}({})", rule_ix, grammar.name(rule_ix)));
        }
    } else {
        for &id in expected {
            if id == EOF_TOKEN {
                names.push("<EOF>".to_string());
            } else if let Some(matcher) = grammar.table.terminals.get(id) {
                let mut rule_ids: Vec<(usize, &'static str)> = terminal_rule_ids(grammar, id)
                    .into_iter()
                    .filter(|(_, name)| !name.starts_with('@') && !name.starts_with('$'))
                    .collect();
                rule_ids.sort_unstable_by_key(|(rule_ix, _)| *rule_ix);
                rule_ids.dedup_by_key(|(rule_ix, _)| *rule_ix);
                if !rule_ids.is_empty() {
                    for (rule_ix, name) in rule_ids {
                        names.push(format!("rule#{}({})", rule_ix, name));
                    }
                } else {
                    names.push(format!("term#{}({})", id, matcher.display()));
                }
            } else {
                names.push(format!("term#{}", id));
            }
        }
    }

    format!("\n  Expected: {}", names.join(" or "))
}

fn terminal_rule_ids(grammar: &Grammar, terminal_id: usize) -> Vec<(usize, &'static str)> {
    let mut out = Vec::new();
    for prod in &grammar.table.productions {
        let Some(first) = prod.rhs.first() else {
            continue;
        };
        if let Symbol::Terminal(t) = first {
            if *t == terminal_id {
                out.push((prod.lhs, grammar.name(prod.lhs)));
            }
        }
    }
    out
}

fn compute_first_sets(grammar: &Grammar) -> FxHashMap<usize, FxHashSet<Option<usize>>> {
    let mut first_sets: FxHashMap<usize, FxHashSet<Option<usize>>> = FxHashMap::default();

    let mut changed = true;
    while changed {
        changed = false;
        for prod in &grammar.table.productions {
            let lhs = prod.lhs;
            let mut nullable = true;
            for sym in &prod.rhs {
                match sym {
                    Symbol::Terminal(t) => {
                        let set = first_sets.entry(lhs).or_default();
                        if set.insert(Some(*t)) {
                            changed = true;
                        }
                        nullable = false;
                        break;
                    }
                    Symbol::NonTerminal(rule_ix) => {
                        let sym_first = first_sets.get(rule_ix).cloned().unwrap_or_default();
                        for f in sym_first.iter().copied() {
                            if f.is_some() {
                                let set = first_sets.entry(lhs).or_default();
                                if set.insert(f) {
                                    changed = true;
                                }
                            }
                        }
                        if !sym_first.contains(&None) {
                            nullable = false;
                            break;
                        }
                    }
                }
            }
            if nullable {
                let set = first_sets.entry(lhs).or_default();
                if set.insert(None) {
                    changed = true;
                }
            }
        }
    }

    first_sets
}

fn pretty_string(s: String) -> String {
    let mut out = String::new();
    for c in s.chars() {
        match c {
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{{{:x}}}", c as u32)),
            c => out.push(c),
        }
    }
    out
}
