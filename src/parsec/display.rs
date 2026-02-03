use crate::parsec::{parser::Parser, tree::Tag};

const RESET: &str = "\x1b[0m";
const DIM: &str = "\x1b[2m";
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const RED: &str = "\x1b[31m";

impl Parser<'_> {
    /// Pretty-print the AST with tree frames and rule ids.
    pub fn display(&self, id: usize) -> String {
        let mut out = String::new();
        let rule_name = |ix| format!("@{}", ix);
        let mut stack = Vec::new();
        self.display_with_indent(
            id, "", true, true, 0, &mut out, &rule_name, &self.text, &mut stack,
        );
        out
    }

    /// Pretty-print the AST with tree frames and rule names.
    pub fn display_with_rules<F>(&self, id: usize, rule_name: F) -> String
    where
        F: Fn(usize) -> String,
    {
        let mut out = String::new();
        let mut stack = Vec::new();
        self.display_with_indent(
            id, "", true, true, 0, &mut out, &rule_name, &self.text, &mut stack,
        );
        out
    }

    fn display_with_indent<F>(
        &self,
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
            let node = self.alloc.get_node(id);
            let (label, extra) = self.format_label(node, rule_name, input, offset);
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
        let node = self.alloc.get_node(id);

        let branch = if is_root {
            ""
        } else if is_last {
            "└─ "
        } else {
            "├─ "
        };

        if let Tag::Field { rule_ix: _, name } = &node.tag {
            if let Some(&child_id) = node.children.first() {
                let child = self.alloc.get_node(child_id);
                let marker = format!("{}{}:{} ", YELLOW, name, RESET);
                let (child_label, child_extra) = self.format_label(child, rule_name, input, offset);
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
                            .map(|&id| self.alloc.get_node(id).width)
                            .sum::<usize>();
                    self.display_with_indent(
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

        let (label, extra) = self.format_label(node, rule_name, input, offset);
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
            self.display_with_indent(
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
            child_offset += self.alloc.get_node(*child_id).width;
        }

        stack.pop();
    }

    fn format_label<F>(
        &self,
        node: &crate::parsec::tree::GreenNode,
        rule_name: &F,
        input: &str,
        offset: usize,
    ) -> (String, String)
    where
        F: Fn(usize) -> String,
    {
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
