use crate::grammar::GrammarError;
use crate::grammar::ir::{NormalizedNode, Production, Symbol};
use crate::grammar::norm::RuleTable;
use crate::utils::Span;
use std::fmt;

const RESET: &str = "\x1b[0m";
const RED: &str = "\x1b[31m";

impl RuleTable {
    fn get_rule_name(&self, idx: usize) -> String {
        self.rules
            .get(idx)
            .and_then(|r| Some(r.name))
            .filter(|n| !n.is_empty())
            .map(|n| n.to_string())
            .unwrap_or_else(|| format!("@{}", idx))
    }

    fn mark_used(&self, node: &NormalizedNode, used: &mut Vec<bool>) {
        match node {
            NormalizedNode::Terminal(_) => {}
            NormalizedNode::Reference(idx) => {
                if *idx < used.len() && !used[*idx] {
                    used[*idx] = true;
                    if let Some(rule) = self.rules.get(*idx) {
                        self.mark_used(&rule.node, used);
                    }
                }
            }
            NormalizedNode::Field(_, inner) => {
                self.mark_used(inner, used);
            }
            NormalizedNode::Sequence(nodes) | NormalizedNode::Alternative(nodes) => {
                for n in nodes {
                    self.mark_used(n, used);
                }
            }
        }
    }

    fn format_node(&self, node: &NormalizedNode) -> String {
        self.format_node_inner(node, false)
    }

    fn format_node_inner(&self, node: &NormalizedNode, parent_is_seq: bool) -> String {
        const RESET: &str = "\x1b[0m";
        const BOLD: &str = "\x1b[1m";
        const GREY: &str = "\x1b[90m";

        match node {
            NormalizedNode::Terminal(matcher) => {
                format!("{}{}{}", GREY, matcher.display(), RESET)
            }
            NormalizedNode::Reference(index) => {
                format!("{}{}{}", BOLD, self.get_rule_name(*index), RESET)
            }
            NormalizedNode::Field(name, inner) => {
                format!("{}:{}", name, self.format_node_inner(inner, false))
            }
            NormalizedNode::Sequence(nodes) => {
                let parts: Vec<String> = nodes
                    .iter()
                    .map(|n| self.format_node_inner(n, true))
                    .collect();
                let content = parts.join(" ");
                if parent_is_seq {
                    content
                } else {
                    format!("({})", content)
                }
            }
            NormalizedNode::Alternative(nodes) => {
                let parts: Vec<String> = nodes
                    .iter()
                    .map(|n| self.format_node_inner(n, false))
                    .collect();
                parts.join(" | ")
            }
        }
    }
}

impl fmt::Display for RuleTable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        const RESET: &str = "\x1b[0m";
        const BOLD: &str = "\x1b[1m";

        let mut used = vec![false; self.rules.len()];
        for (i, rule) in self.rules.iter().enumerate() {
            if !rule.name.is_empty() {
                used[i] = true;
                self.mark_used(&rule.node, &mut used);
            }
        }

        let max_width = self
            .rules
            .iter()
            .enumerate()
            .filter(|(i, r)| !r.name.is_empty() || used[*i])
            .map(|(_, r)| r.name.len())
            .max()
            .unwrap_or(0);

        for (i, rule) in self.rules.iter().enumerate() {
            if !rule.name.is_empty() || used[i] {
                let name = if rule.name.is_empty() {
                    format!("@{}", i)
                } else {
                    rule.name.to_string()
                };

                writeln!(
                    f,
                    "{}{:<width$}{} → {}",
                    BOLD,
                    name,
                    RESET,
                    self.format_node(&rule.node),
                    width = max_width
                )?;
            }
        }

        Ok(())
    }
}

impl fmt::Display for Production {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} →", self.lhs)?;
        for symbol in &self.rhs {
            write!(f, " {}", symbol)?;
        }
        Ok(())
    }
}

impl fmt::Display for Symbol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Symbol::Terminal(idx) => write!(f, "T{}", idx),
            Symbol::NonTerminal(idx) => write!(f, "N{}", idx),
        }
    }
}

/// Format a grammar error with source context showing the problematic span
pub fn format_grammar_error(error: &GrammarError, source: &str) -> String {
    use std::fmt::Write;

    let mut out = String::new();
    let lines: Vec<&str> = source.lines().collect();

    if lines.is_empty() {
        // No source to display, just show the error message
        return format_grammar_error_message(error);
    }

    let span = match error {
        GrammarError::UnboundRuleReference(span, _)
        | GrammarError::DuplicateRuleName(span, _)
        | GrammarError::NoStartRule(span)
        | GrammarError::DropCountExceedsNodeLength(span)
        | GrammarError::DropOnNonReference(span) => span,
        _ => &Span::empty(),
    };

    // If span is empty (start == end == 0), just show the error message
    if span.start == 0 && span.end == 0 {
        return format_grammar_error_message(error);
    }

    let (start_line, start_col) = Span::new(span.start, span.start).start_line_col(source);
    let (end_line, end_col) = Span::new(span.end, span.end).end_line_col(source);

    let error_msg = match error {
        GrammarError::UnboundRuleReference(_, name) => {
            format!("{}error: unbound rule reference '{}'{}", RED, name, RESET)
        }
        GrammarError::DuplicateRuleName(_, name) => {
            format!("{}error: duplicate rule name '{}'{}", RED, name, RESET)
        }
        GrammarError::NoStartRule(_) => {
            format!("{}error: no start rule defined{}", RED, RESET)
        }
        GrammarError::DropCountExceedsNodeLength(_) => {
            format!("{}error: drop count exceeds node length{}", RED, RESET)
        }
        GrammarError::DropOnNonReference(_) => {
            format!(
                "{}error: drop can only be applied to references{}",
                RED, RESET
            )
        }
        GrammarError::IoError(e) => {
            format!("{}error: I/O error - {}{}", RED, e, RESET)
        }
    };

    let _ = write!(out, "{} at {}:{}", error_msg, start_line, start_col);

    // Show context lines
    if source.is_empty() {
        return out;
    }
    format_error_context(&mut out, &lines, start_line, end_line, start_col, end_col);
    out
}

/// Format just the error message without source context
pub fn format_grammar_error_message(error: &GrammarError) -> String {
    match error {
        GrammarError::UnboundRuleReference(_, name) => {
            format!("{}error: unbound rule reference '{}'{}", RED, name, RESET)
        }
        GrammarError::DuplicateRuleName(_, name) => {
            format!("{}error: duplicate rule name '{}'{}", RED, name, RESET)
        }
        GrammarError::NoStartRule(_) => {
            format!("{}error: no start rule defined{}", RED, RESET)
        }
        GrammarError::DropCountExceedsNodeLength(_) => {
            format!("{}error: drop count exceeds node length{}", RED, RESET)
        }
        GrammarError::DropOnNonReference(_) => {
            format!(
                "{}error: drop can only be applied to references{}",
                RED, RESET
            )
        }
        GrammarError::IoError(e) => {
            format!("{}error: I/O error - {}{}", RED, e, RESET)
        }
    }
}

fn format_error_context(
    out: &mut String,
    lines: &[&str],
    start_line: usize,
    end_line: usize,
    start_col: usize,
    end_col: usize,
) {
    use std::fmt::Write;

    if lines.is_empty() || start_line == 0 {
        return;
    }

    let line_idx = start_line - 1; // Convert to 0-indexed
    let context_before = if line_idx > 0 { 1 } else { 0 };
    let context_after = if line_idx + 1 < lines.len() { 1 } else { 0 };

    let first_line = line_idx.saturating_sub(context_before);
    let last_line = (line_idx + context_after).min(lines.len() - 1);
    let gutter_width = format!("{}", last_line + 1).len().max(1);

    // Show lines before
    for i in first_line..line_idx {
        let _ = write!(
            out,
            "\n{:>width$} | {}",
            i + 1,
            lines[i],
            width = gutter_width
        );
    }

    // Show error line with red coloring
    let _ = write!(
        out,
        "\n{}{:>width$} | {}{}",
        RED,
        line_idx + 1,
        lines[line_idx],
        RESET,
        width = gutter_width
    );

    // Show underline/arrow
    let underline_indent = gutter_width + 3; // gutter + " | "
    let _ = write!(out, "\n{}", " ".repeat(underline_indent));
    let _ = write!(out, "{}", " ".repeat(start_col));

    // Show underline spanning the error
    if start_line == end_line && start_col < end_col {
        let len = end_col - start_col;
        let _ = write!(out, "{}", RED);
        let _ = write!(out, "{}", "~".repeat(len.max(1)));
        let _ = write!(out, "{}", RESET);
    } else {
        let _ = write!(out, "{}^{}", RED, RESET);
    }

    // Show lines after
    for i in (line_idx + 1)..=last_line {
        let _ = write!(
            out,
            "\n{:>width$} | {}",
            i + 1,
            lines[i],
            width = gutter_width
        );
    }
}
