use std::fmt::Write;

use crate::grammar::Grammar;
use crate::grammar::analysis::EOF_TOKEN;
use crate::parsec::msg::{ErrorMessage, ParserMessage};
use crate::parsec::tree::{GreenId, RedNode, Tag, TreeAllocRef, TreeAllocRefExt};
use crate::utils::{Position, TextIndex, advance_position_by_bytes};

const RESET: &str = "\x1b[0m";
const DIM: &str = "\x1b[2m";
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const RED: &str = "\x1b[31m";

pub fn format_ast(grammar: &Grammar, root: &RedNode, alloc: &TreeAllocRef, source: &str) -> String {
    let mut out = String::new();
    let mut stack = Vec::new();
    let root_byte_offset = TextIndex::new(source)
        .position_to_byte_with_text(root.position, source)
        .unwrap_or(0);
    display_with_indent(
        grammar,
        alloc,
        source,
        root.green,
        "",
        true,
        true,
        root.position,
        root_byte_offset,
        &mut out,
        &mut stack,
    );
    out
}

pub fn format_messages_with_source(
    grammar: &Grammar,
    messages: &[ParserMessage],
    source: &str,
) -> String {
    use std::collections::BTreeMap;

    let mut out = String::new();
    let lines: Vec<&str> = source.lines().collect();

    let mut grouped: BTreeMap<(Position, Position, &str), (bool, Vec<usize>)> = BTreeMap::new();

    for msg in messages {
        let (msg_type, is_missing) = match &msg.message {
            ErrorMessage::UnexpectedToken { .. } => ("unexpected", false),
            ErrorMessage::MissingToken { .. } => ("missing", true),
            ErrorMessage::InternalError { .. } => ("internal", false),
        };

        let key = (msg.span.start, msg.span.end, msg_type);

        match &msg.message {
            ErrorMessage::UnexpectedToken { expected }
            | ErrorMessage::MissingToken { expected } => {
                let entry = grouped.entry(key).or_insert((is_missing, Vec::new()));
                entry.1.extend(expected.iter().copied());
            }
            ErrorMessage::InternalError { .. } => {
                grouped.entry(key).or_insert((is_missing, Vec::new()));
            }
        }
    }

    // Print accumulated errors
    let mut first = true;
    for ((start_pos, _end_pos, msg_type), (is_missing, mut expected)) in grouped {
        if !first {
            out.push_str("\n\n");
        }
        first = false;

        let start_line = start_pos.line;
        let start_col = start_pos.character;
        let end_line = start_pos.line;
        let end_col = start_pos.character;

        let display_type = match msg_type {
            "unexpected" => "error: unexpected token",
            "missing" => "error: missing token",
            _ => "error",
        };

        let _ = write!(
            out,
            "{}{}{} at {}:{}",
            RED, display_type, RESET, start_line, start_col
        );

        // Show context lines
        format_error_context(
            &mut out, &lines, start_line, end_line, start_col, end_col, is_missing,
        );

        // Show expected tokens (deduplicated)
        if !expected.is_empty() {
            expected.sort_unstable();
            expected.dedup();
            let expected_str = format_expected_friendly(grammar, &expected);
            let _ = write!(out, "\n  {}expected:{} {}", YELLOW, RESET, expected_str);
        }
    }

    out
}

fn format_error_context(
    out: &mut String,
    lines: &[&str],
    start_line: usize,
    end_line: usize,
    start_col: usize,
    end_col: usize,
    is_missing: bool,
) {
    if lines.is_empty() || start_line >= lines.len() {
        return;
    }

    let line_idx = start_line;
    let context_before = if line_idx > 0 { 1 } else { 0 };
    let context_after = if line_idx + 1 < lines.len() { 1 } else { 0 };

    let first_line = line_idx.saturating_sub(context_before);
    let last_line = (line_idx + context_after).min(lines.len() - 1);
    let gutter_width = format!("{}", last_line).len().max(1);

    // Show lines before
    for i in first_line..line_idx {
        let _ = write!(out, "\n{:>width$} | {}", i, lines[i], width = gutter_width);
    }

    // Show error line with red coloring
    let _ = write!(
        out,
        "\n{}{:>width$} | {}{}",
        RED,
        line_idx,
        lines[line_idx],
        RESET,
        width = gutter_width
    );

    // Show underline/arrow
    let underline_indent = gutter_width + 3; // gutter + " | "
    let _ = write!(out, "\n{}", " ".repeat(underline_indent));
    let _ = write!(out, "{}", " ".repeat(start_col));

    if is_missing {
        // For missing tokens, show single arrow
        let _ = write!(out, "{}^{}", RED, RESET);
    } else {
        // For unexpected tokens, show underline spanning the error
        if start_line == end_line && start_col < end_col {
            let len = end_col - start_col;
            let _ = write!(out, "{}", RED);
            let _ = write!(out, "{}", "~".repeat(len));
            let _ = write!(out, "{}", RESET);
        } else {
            let _ = write!(out, "{}", RED);
            let _ = write!(out, "^{}", RESET);
        }
    }

    // Show lines after
    for i in (line_idx + 1)..=last_line {
        let _ = write!(out, "\n{:>width$} | {}", i, lines[i], width = gutter_width);
    }
}

fn display_with_indent(
    grammar: &Grammar,
    alloc: &TreeAllocRef,
    source: &str,
    id: GreenId,
    prefix: &str,
    is_last: bool,
    is_root: bool,
    position: Position,
    byte_offset: usize,
    out: &mut String,
    stack: &mut Vec<GreenId>,
) {
    if stack.contains(&id) {
        let node = alloc.get_node(id);
        let (label, extra) = format_label(grammar, alloc, source, id, byte_offset);
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
            let (child_label, child_extra) =
                format_label(grammar, alloc, source, child_id, byte_offset);
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
            let mut running_position = position;
            let mut running_byte_offset = byte_offset;
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
                    running_position,
                    running_byte_offset,
                    out,
                    stack,
                );
                if !last {
                    out.push('\n');
                }
                let width = alloc.get_node(grandchild_id).width;
                running_position =
                    advance_position_by_bytes(source, running_byte_offset, running_position, width);
                running_byte_offset += width;
            }
            stack.pop();
            return;
        }
    }

    let (label, extra) = format_label(grammar, alloc, source, id, byte_offset);
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
    let mut child_position = position;
    let mut child_byte_offset = byte_offset;
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
            child_position,
            child_byte_offset,
            out,
            stack,
        );
        if !last {
            out.push('\n');
        }
        let width = alloc.get_node(child_id).width;
        child_position =
            advance_position_by_bytes(source, child_byte_offset, child_position, width);
        child_byte_offset += width;
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
    byte_offset: usize,
) -> (String, String) {
    let node = alloc.get_node(id);
    match &node.tag {
        Tag::Rule { rule_ix, .. } => {
            let name = grammar.name(*rule_ix);
            if name.starts_with('@') {
                (String::new(), String::new())
            } else {
                (name.to_string(), String::new())
            }
        }
        Tag::Token { .. } => {
            if node.children.is_empty() {
                let end = byte_offset.saturating_add(node.width).min(source.len());
                let slice = source.get(byte_offset..end).unwrap_or("");
                let text = pretty_string(slice.to_string());
                (format!("{}{}{}", GREEN, text, RESET), String::new())
            } else {
                (String::new(), String::new())
            }
        }
        Tag::Field { name, .. } => (format!("{}{}:{}", YELLOW, name, RESET), String::new()),
        Tag::Error(errors) => {
            let err_desc = format!("{:?}", errors);
            (format!("{}[{}]{}", RED, err_desc, RESET), String::new())
        }
    }
}

fn format_expected_friendly(grammar: &Grammar, expected: &[usize]) -> String {
    if expected.is_empty() {
        return "<unknown>".to_string();
    }

    let mut names = Vec::new();

    for &id in expected {
        if id == EOF_TOKEN {
            names.push("end of input".to_string());
        } else if let Some(matcher) = grammar.table.terminals.get(id) {
            let mut display = matcher.display();

            // Strip "char_predicate* " prefix (implementation detail)
            if display.starts_with("char_predicate* ") {
                display = display
                    .strip_prefix("char_predicate* ")
                    .unwrap_or(&display)
                    .to_string();
            }

            // Show friendly names
            names.push(match display.as_str() {
                "string" => "string literal".to_string(),
                "ident" => "identifier".to_string(),
                "number" => "number".to_string(),
                "identifier" => "identifier".to_string(),
                "whitespaces" => "whitespace".to_string(),
                s if s.starts_with('"') && s.ends_with('"') => display, // Already quoted
                s if s.starts_with('\'') && s.ends_with('\'') => display, // Already quoted
                s => s.to_string(),
            });
        }
    }

    // Deduplicate and join
    names.sort();
    names.dedup();
    if names.len() > 1 {
        let last = names.pop().unwrap();
        format!("{} or {}", names.join(", "), last)
    } else {
        names.join(", ")
    }
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
