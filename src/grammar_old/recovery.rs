use crate::grammar_old::{Grammar, ir::State};
use crate::parsec_old::words::MatcherRef;
use std::collections::HashSet;

#[derive(Clone, Debug)]
pub struct Region {
    pub start: usize,
    pub end: usize,
    pub indent: usize,
}

#[derive(Clone, Debug)]
pub struct RecoverySpecs {
    pub regions: Vec<Region>,
    pub line_starts: Vec<usize>,
    pub line_indents: Vec<usize>,
    pub strategy: ErrorRecoveryStrategy,
}

#[derive(Clone, Debug)]
pub struct ErrorRecoveryStrategy {
    pub sync_tokens: Vec<MatcherRef>,
    pub recovery_states: HashSet<usize>,
}

impl ErrorRecoveryStrategy {
    pub fn from_grammar(grammar: &Grammar) -> Self {
        let mut sync_tokens = Vec::new();
        let mut recovery_states = HashSet::new();

        for (state_id, state) in grammar.analysis.states.iter().enumerate() {
            match state {
                State::Tok(_, matcher) => {
                    // Check if the matcher produces a literal string or one of its components is a literal.
                    // If it does, we treat it as a synchronization token.
                    if matcher.preview().is_some() || matcher.display() == "EOF" {
                        sync_tokens.push(matcher.clone());
                    }
                }
                State::Alt(_, children, _) => {
                    recovery_states.insert(state_id);
                    for &child in children {
                        recovery_states.insert(child);
                    }
                }
                _ => {}
            }
        }

        Self {
            sync_tokens,
            recovery_states,
        }
    }

    pub fn find_sync_point(&self, text: &str, start_pos: usize) -> Option<usize> {
        let mut pos = start_pos;
        while pos < text.len() {
            for matcher in &self.sync_tokens {
                let mut test_pos = pos;
                if matcher.matches(text, &mut test_pos).is_some() {
                    return Some(pos);
                }
            }
            pos += 1;
        }
        None
    }

    pub fn can_recover_at(&self, state_id: usize) -> bool {
        self.recovery_states.contains(&state_id)
    }
}

impl RecoverySpecs {
    pub fn from_text(text: &str) -> Self {
        Self::from_text_with_strategy(
            text,
            ErrorRecoveryStrategy {
                sync_tokens: Vec::new(),
                recovery_states: HashSet::new(),
            },
        )
    }

    pub fn from_text_with_strategy(text: &str, strategy: ErrorRecoveryStrategy) -> Self {
        let mut line_starts = vec![0];
        let mut line_indents = vec![0];

        for (i, b) in text.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push(i + 1);
                let mut indent = 0;
                let line_start = i + 1;
                while line_start + indent < text.len() {
                    match text.as_bytes()[line_start + indent] {
                        b' ' => indent += 1,
                        b'\t' => indent += 4,
                        _ => break,
                    }
                }
                line_indents.push(indent);
            }
        }

        let mut regions = Vec::new();
        let mut stack: Vec<(usize, usize)> = Vec::new();
        let mut prev_indent = 0usize;

        for (idx, &line_start) in line_starts.iter().enumerate() {
            let line_end = if idx + 1 < line_starts.len() {
                line_starts[idx + 1].saturating_sub(1)
            } else {
                text.len()
            };

            let line = &text[line_start..line_end];
            let mut indent = 0usize;
            let mut seen_non_ws = false;
            for ch in line.chars() {
                match ch {
                    ' ' => indent += 1,
                    '\t' => indent += 4,
                    _ => {
                        seen_non_ws = true;
                        break;
                    }
                }
            }

            if !seen_non_ws {
                continue;
            }

            if indent > prev_indent {
                stack.push((indent, line_start));
            } else if indent < prev_indent {
                while let Some(&(top_indent, start)) = stack.last() {
                    if top_indent > indent {
                        stack.pop();
                        regions.push(Region {
                            start,
                            end: line_start,
                            indent: top_indent,
                        });
                    } else {
                        break;
                    }
                }
            }

            prev_indent = indent;
        }

        let end = text.len();
        while let Some((indent, start)) = stack.pop() {
            regions.push(Region { start, end, indent });
        }

        regions.sort_by_key(|r| r.end - r.start);

        Self {
            regions,
            line_starts,
            line_indents,
            strategy,
        }
    }

    pub fn region_end_at(&self, pos: usize) -> Option<usize> {
        self.regions
            .iter()
            .find(|r| r.start <= pos && pos < r.end)
            .map(|r| r.end)
    }

    pub fn next_line_start(&self, pos: usize) -> Option<usize> {
        self.line_starts.iter().copied().find(|&s| s > pos)
    }

    pub fn indent_at(&self, pos: usize) -> usize {
        let line_idx = self.line_starts.iter().position(|&s| s > pos).unwrap_or(0);
        if line_idx > 0 && line_idx <= self.line_indents.len() {
            self.line_indents[line_idx - 1]
        } else {
            0
        }
    }

    pub fn forward_skip_to_decrease(&self, pos: usize, current_indent: usize) -> Option<usize> {
        let mut search_pos = pos;
        while let Some(next_line) = self.next_line_start(search_pos) {
            let next_indent = self.indent_at(next_line);
            if next_indent < current_indent {
                return Some(next_line);
            }
            search_pos = next_line;
        }
        None
    }

    pub fn backward_skip_to_decrease(&self, pos: usize, current_indent: usize) -> Option<usize> {
        let mut line_idx = self.line_starts.iter().position(|&s| s > pos)?;
        if line_idx == 0 {
            return None;
        }
        line_idx -= 1;

        while line_idx > 0 {
            let prev_indent = if line_idx > 0 && line_idx <= self.line_indents.len() {
                self.line_indents[line_idx - 1]
            } else {
                0
            };
            if prev_indent < current_indent {
                return self.line_starts.get(line_idx).copied();
            }
            line_idx -= 1;
        }
        None
    }
}
