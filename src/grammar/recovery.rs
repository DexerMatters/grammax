use crate::parsec::words::MatcherRef;
use std::collections::HashSet;

/// Region in the source text for error recovery
#[derive(Clone, Debug)]
pub struct Region {
    pub start: usize,
    pub end: usize,
    pub indent: usize,
}

/// Recovery specifications for error handling during parsing
#[derive(Clone, Debug)]
pub struct RecoverySpecs {
    pub regions: Vec<Region>,
    pub line_starts: Vec<usize>,
    pub line_indents: Vec<usize>,
    pub strategy: ErrorRecoveryStrategy,
}

/// Strategy for error recovery in LR parsing
#[derive(Clone, Debug)]
pub struct ErrorRecoveryStrategy {
    pub sync_tokens: Vec<MatcherRef>,
    pub recovery_states: HashSet<usize>,
}

impl ErrorRecoveryStrategy {
    pub fn new() -> Self {
        Self {
            sync_tokens: Vec::new(),
            recovery_states: HashSet::new(),
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
        Self::from_text_with_strategy(text, ErrorRecoveryStrategy::new())
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

        let regions = Self::compute_regions(&line_starts, &line_indents, text.len());

        Self {
            regions,
            line_starts,
            line_indents,
            strategy,
        }
    }

    fn compute_regions(line_starts: &[usize], line_indents: &[usize], text_len: usize) -> Vec<Region> {
        let mut regions = Vec::new();
        if line_starts.is_empty() {
            return regions;
        }

        let mut stack: Vec<(usize, usize)> = Vec::new();
        
        for (line_idx, &line_start) in line_starts.iter().enumerate() {
            let indent = line_indents.get(line_idx).copied().unwrap_or(0);
            
            while let Some(&(_, prev_indent)) = stack.last() {
                if indent > prev_indent {
                    break;
                }
                if let Some((region_start, _)) = stack.pop() {
                    regions.push(Region {
                        start: region_start,
                        end: line_start,
                        indent: prev_indent,
                    });
                }
            }
            
            stack.push((line_start, indent));
        }

        while let Some((region_start, indent)) = stack.pop() {
            regions.push(Region {
                start: region_start,
                end: text_len,
                indent,
            });
        }

        regions
    }

    pub fn line_number(&self, pos: usize) -> usize {
        self.line_starts
            .binary_search(&pos)
            .unwrap_or_else(|i| i.saturating_sub(1))
    }

    pub fn column_number(&self, pos: usize) -> usize {
        let line = self.line_number(pos);
        pos - self.line_starts[line]
    }
}
