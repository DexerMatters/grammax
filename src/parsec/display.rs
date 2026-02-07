use std::fmt::Write;

use crate::grammar::Grammar;
use crate::parsec::tree::{GreenId, RedNode, Tag, TreeAllocRef, TreeAllocRefExt};

const RESET: &str = "\x1b[0m";
const DIM: &str = "\x1b[2m";
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const RED: &str = "\x1b[31m";

pub struct AstPrinter<'a> {
    grammar: &'a Grammar,
    alloc: &'a TreeAllocRef,
    source: &'a str,
    output: String,
    stack: Vec<GreenId>,
}

impl<'a> AstPrinter<'a> {
    pub fn new(grammar: &'a Grammar, alloc: &'a TreeAllocRef, source: &'a str) -> Self {
        Self {
            grammar,
            alloc,
            source,
            output: String::new(),
            stack: Vec::new(),
        }
    }

    pub fn print(mut self, root: &RedNode) -> String {
        self.print_recursive(root.green, "", true, true, root.offset);
        self.output
    }

    fn print_recursive(
        &mut self,
        id: GreenId,
        prefix: &str,
        is_last: bool,
        is_root: bool,
        offset: usize,
    ) {
        if self.stack.contains(&id) {
            let node = self.alloc.get_node(id);
            let (label, extra) = self.format_label(id, offset);
            let width = format!(" {}[width: {}]{}", DIM, node.width, RESET);

            let branch = if is_root {
                ""
            } else if is_last {
                "└─ "
            } else {
                "├─ "
            };

            let _ = write!(
                self.output,
                "{}{}{}{}{}{} {}↻{}",
                prefix, branch, label, width, extra, " ", DIM, RESET
            );
            return;
        }

        self.stack.push(id);
        let node = self.alloc.get_node(id);

        let branch = if is_root {
            ""
        } else if is_last {
            "└─ "
        } else {
            "├─ "
        };

        if let Tag::Field { name, .. } = &node.tag {
            if let Some(&child_id) = node.children.first() {
                let child = self.alloc.get_node(child_id);
                let marker = format!("{}{}:{} ", YELLOW, name, RESET);
                let (child_label, child_extra) = self.format_label(child_id, offset);
                let label = format!("{}{}", marker, child_label);
                let width = format!(" {}[width: {}]{}", DIM, node.width, RESET);

                let _ = write!(
                    self.output,
                    "{}{}{}{}{}",
                    prefix, branch, label, width, child_extra
                );

                if child.children.is_empty() {
                    self.stack.pop();
                    return;
                }

                self.output.push('\n');

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
                    self.print_recursive(
                        grandchild_id,
                        &child_prefix,
                        last,
                        false,
                        running_offset,
                    );

                    if !last {
                        self.output.push('\n');
                    }
                    running_offset += self.alloc.get_node(grandchild_id).width;
                }
                self.stack.pop();
                return;
            }
        }

        let (label, extra) = self.format_label(id, offset);
        let width = format!(" {}[width: {}]{}", DIM, node.width, RESET);

        let _ = write!(
            self.output,
            "{}{}{}{}{}",
            prefix, branch, label, width, extra
        );

        if node.children.is_empty() {
            self.stack.pop();
            return;
        }

        self.output.push('\n');

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
            self.print_recursive(child_id, &child_prefix, last, false, child_offset);
            if !last {
                self.output.push('\n');
            }
            child_offset += self.alloc.get_node(child_id).width;
        }

        self.stack.pop();
    }

    fn format_label(&self, id: GreenId, offset: usize) -> (String, String) {
        let node = self.alloc.get_node(id);
        match &node.tag {
            Tag::Rule { rule_ix } => {
                let name = self.grammar.name(*rule_ix);
                if name.starts_with('@') {
                    (String::new(), String::new())
                } else {
                    (name.to_string(), String::new())
                }
            }
            Tag::Token { .. } => {
                if node.children.is_empty() {
                    let end = offset.saturating_add(node.width).min(self.source.len());
                    let slice = self.source.get(offset..end).unwrap_or("");
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
                (format!("{}error:[{}]{}", RED, err_desc, RESET), String::new())
            }
            Tag::Root => ("ROOT".to_string(), String::new()),
        }
    }
}

pub fn format_ast(grammar: &Grammar, root: &RedNode, alloc: &TreeAllocRef, source: &str) -> String {
    let printer = AstPrinter::new(grammar, alloc, source);
    printer.print(root)
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
