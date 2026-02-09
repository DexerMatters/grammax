use std::fmt::Write;

use crate::grammar::Grammar;
use crate::grammar::analysis::EOF_TOKEN;
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

            let mut running_offset = offset;
            for (idx, &grandchild_id) in child.children.iter().enumerate() {
                let last = idx + 1 == child.children.len();
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

    let mut child_offset = offset;
    for (idx, &child_id) in node.children.iter().enumerate() {
        let last = idx + 1 == node.children.len();
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

    let mut names = Vec::new();
    for &id in expected {
        if id == EOF_TOKEN {
            names.push("<EOF>".to_string());
        } else if let Some(matcher) = grammar.table.terminals.get(id) {
            names.push(matcher.display());
        } else {
            names.push(format!("#{}", id));
        }
    }

    format!("\n  Expected: {}", names.join(" or "))
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
