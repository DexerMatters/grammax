use crate::{
    grammar::ir::State,
    parsec::{
        Parser,
        msg::ParserMessages,
        tree::{RedNode, Tag, TreeAllocRefExt},
    },
};
use std::collections::HashSet;

const RESET: &str = "\x1b[0m";
const DIM: &str = "\x1b[2m";
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const RED: &str = "\x1b[31m";

pub trait Display {
    fn display(&self, parser: &Parser) -> String;
}

impl Display for RedNode {
    fn display(&self, parser: &Parser) -> String {
        let mut out = String::new();
        let rule_name = |ix: usize| {
            if let Some(name) = parser.grammar.table.rule_names.get(ix) {
                name.to_string()
            } else {
                format!("@{}", ix)
            }
        };
        let mut stack = Vec::new();
        display_with_indent(
            parser,
            self.green,
            "",
            true,
            true,
            self.offset,
            &mut out,
            &rule_name,
            &parser.text,
            &mut stack,
        );
        out
    }
}

impl Display for State {
    fn display(&self, parser: &Parser) -> String {
        let Some(ix) = parser
            .grammar
            .analysis
            .states
            .iter()
            .position(|state| std::ptr::eq(state, self))
        else {
            return display_state_inner(self, parser, &mut HashSet::new());
        };

        let mut seen = HashSet::new();
        display_state_index(parser, ix, &mut seen)
    }
}

impl Display for ParserMessages {
    fn display(&self, parser: &Parser) -> String {
        if self.is_empty() {
            return String::new();
        }

        let mut out = String::new();
        out.push_str("\n");

        let mut seen = HashSet::new();
        for (i, msg) in self.iter().enumerate() {
            if i > 0 {
                out.push_str("\n");
            }

            match &msg.message {
                crate::parsec::msg::ErrorMessage::MissingToken { expected } => {
                    let missing = format_missing_expected(parser, expected, &mut seen);
                    out.push_str(&format!(
                        "  {}Missing Token{} {} at [{}, {}]",
                        RED, RESET, missing, msg.span.start, msg.span.end
                    ));
                }
                crate::parsec::msg::ErrorMessage::UnexpectedToken { expected } => {
                    out.push_str(&format!(
                        "  {}Unexpected Token{} at [{}, {}]",
                        RED, RESET, msg.span.start, msg.span.end
                    ));

                    if !expected.is_empty() {
                        out.push_str("\n  Expected: ");
                        let expected_states: Vec<String> = expected
                            .iter()
                            .filter_map(|&ix| format_expected_state(parser, ix, &mut seen))
                            .collect();

                        if !expected_states.is_empty() {
                            out.push_str(&expected_states.join(" or "));
                        }
                    }
                }
            }
        }

        out
    }
}

fn display_state_index(parser: &Parser, ix: usize, seen: &mut HashSet<usize>) -> String {
    if !seen.insert(ix) {
        return "…".to_string();
    }

    let state = &parser.grammar.analysis.states[ix];
    let rendered = display_state_inner(state, parser, seen);
    seen.remove(&ix);
    rendered
}

fn display_state_inner(state: &State, parser: &Parser, seen: &mut HashSet<usize>) -> String {
    let rule_names = &parser.grammar.table.rule_names;
    let rule_descs = &parser.grammar.table.rule_descriptions;

    let rule_ix = state.ref_ix();
    if let Some(name) = rule_names.get(rule_ix) {
        if !name.starts_with('@') && !name.is_empty() {
            let desc = rule_descs[rule_ix];
            return if desc.is_empty() {
                name.to_string()
            } else {
                desc.to_string()
            };
        }
    }

    match state {
        State::Field(_, name, _) => name.to_string(),
        State::Tok(_, matcher) => matcher.preview().unwrap_or_else(|| matcher.display()),
        State::Seq(_, children) => children
            .iter()
            .map(|&child_ix| display_state_index(parser, child_ix, seen))
            .collect::<Vec<_>>()
            .join(" "),
        State::Alt(_, children, _) => {
            let parts: Vec<String> = children
                .iter()
                .map(|&child_ix| display_state_index(parser, child_ix, seen))
                .collect();

            match parts.len() {
                0 => "nothing".to_string(),
                1 => format!("one of {}", parts[0]),
                2 => format!("one of {} and {}", parts[0], parts[1]),
                _ => {
                    let last = parts.last().unwrap();
                    let comma_list = parts[..parts.len() - 1].join(", ");
                    format!("one of {}, and {}", comma_list, last)
                }
            }
        }
        State::LeftRec(..) => "recursion".to_string(),
    }
}

fn format_expected_state(parser: &Parser, ix: usize, seen: &mut HashSet<usize>) -> Option<String> {
    parser
        .grammar
        .analysis
        .states
        .get(ix)
        .map(|_| display_state_index(parser, ix, seen))
}

fn format_missing_expected(
    parser: &Parser,
    expected: &[usize],
    seen: &mut HashSet<usize>,
) -> String {
    let tokens: Vec<String> = expected
        .iter()
        .filter_map(|&ix| {
            parser
                .grammar
                .analysis
                .states
                .get(ix)
                .map(|state| match state {
                    State::Tok(_, matcher) => matcher
                        .preview()
                        .map(|lit| format!("'{}'", lit))
                        .unwrap_or_else(|| format!("'{}'", matcher.display())),
                    _ => format_expected_state(parser, ix, seen).unwrap_or_else(|| "?".to_string()),
                })
        })
        .collect();

    match tokens.len() {
        0 => "?".to_string(),
        1 => tokens[0].clone(),
        2 => format!("one of {} and {}", tokens[0], tokens[1]),
        _ => {
            let last = tokens.last().unwrap();
            let comma_list = tokens[..tokens.len() - 1].join(", ");
            format!("one of {}, and {}", comma_list, last)
        }
    }
}

fn display_with_indent<F>(
    parser: &Parser,
    id: usize,
    prefix: &str,
    is_last: bool,
    is_root: bool,
    offset: usize,
    out: &mut String,
    rule_name: &F,
    input: &str,
    stack: &mut Vec<usize>,
) where
    F: Fn(usize) -> String,
{
    if stack.contains(&id) {
        let node = parser.alloc.get_node(id);
        let (label, extra) = format_label(parser, id, rule_name, input, offset);
        let width = format!(" {}[width: {}]{}", DIM, node.width, RESET);

        let branch = if is_root {
            ""
        } else if is_last {
            "└─ "
        } else {
            "├─ "
        };

        out.push_str(prefix);
        out.push_str(branch);
        out.push_str(&label);
        out.push_str(&width);
        out.push_str(&extra);
        out.push_str(" ");
        out.push_str(DIM);
        out.push_str("↻");
        out.push_str(RESET);
        return;
    }

    stack.push(id);
    let node = parser.alloc.get_node(id);

    let branch = if is_root {
        ""
    } else if is_last {
        "└─ "
    } else {
        "├─ "
    };

    if let Tag::Field { rule_ix: _, name } = &node.tag {
        if let Some(&child_id) = node.children.first() {
            let child = parser.alloc.get_node(child_id);
            let marker = format!("{}{}:{} ", YELLOW, name, RESET);
            let (child_label, child_extra) =
                format_label(parser, child_id, rule_name, input, offset);
            let label = format!("{}{}", marker, child_label);
            let width = format!(" {}[width: {}]{}", DIM, node.width, RESET);

            out.push_str(prefix);
            out.push_str(branch);
            out.push_str(&label);
            out.push_str(&width);
            out.push_str(&child_extra);

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

            for (idx, grandchild_id) in child.children.iter().enumerate() {
                let last = idx + 1 == child.children.len();
                let child_offset = offset
                    + child
                        .children
                        .iter()
                        .take(idx)
                        .map(|&id| parser.alloc.get_node(id).width)
                        .sum::<usize>();
                display_with_indent(
                    parser,
                    *grandchild_id,
                    &child_prefix,
                    last,
                    false,
                    child_offset,
                    out,
                    rule_name,
                    input,
                    stack,
                );
                if !last {
                    out.push('\n');
                }
            }
            stack.pop();
            return;
        }
    }

    let (label, extra) = format_label(parser, id, rule_name, input, offset);
    let width = format!(" {}[width: {}]{}", DIM, node.width, RESET);

    out.push_str(prefix);
    out.push_str(branch);
    out.push_str(&label);
    out.push_str(&width);
    out.push_str(&extra);

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
    for (idx, child_id) in node.children.iter().enumerate() {
        let last = idx + 1 == node.children.len();
        display_with_indent(
            parser,
            *child_id,
            &child_prefix,
            last,
            false,
            child_offset,
            out,
            rule_name,
            input,
            stack,
        );
        if !last {
            out.push('\n');
        }
        child_offset += parser.alloc.get_node(*child_id).width;
    }

    stack.pop();
}

fn format_label<F>(
    parser: &Parser,
    id: usize,
    rule_name: &F,
    input: &str,
    offset: usize,
) -> (String, String)
where
    F: Fn(usize) -> String,
{
    let node = parser.alloc.get_node(id);
    match &node.tag {
        Tag::Rule { rule_ix } => {
            let name = rule_name(*rule_ix);
            if name.starts_with('@') {
                (String::new(), String::new())
            } else {
                (name, String::new())
            }
        }
        Tag::Token { .. } => {
            if node.children.is_empty() {
                let end = offset.saturating_add(node.width).min(input.len());
                let slice = input.get(offset..end).unwrap_or("");
                let text = pretty_string(slice.to_string());
                (
                    format!("{}{}{}", GREEN, pretty_string(text), RESET),
                    String::new(),
                )
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
