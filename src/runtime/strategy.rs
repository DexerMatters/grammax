use std::sync::Arc;

use rustc_hash::FxHashMap;

use crate::{
    grammar::recovery::{ErrorRecoveryStrategy, RecoverySpecs},
    parsec::{
        Parser,
        msg::ParserMessages,
        tree::{ParsecError, Tag, TreeAllocRefExt},
    },
    runtime::{
        metrics::EditMetrics,
        reparser::{ReparserConfig, Zipper},
    },
    utils::Span,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct CandidateScore {
    /// Error count outside the edit region (prioritized over inside errors).
    errors_outside: usize,
    /// Error count inside the edit region.
    errors_inside: usize,
    /// Zipper level wrapped in Reverse: deeper zippers (higher level) sort lower,
    /// naturally preferring narrow reparses over shallow ones.
    level: std::cmp::Reverse<usize>,
}

impl CandidateScore {
    pub(crate) fn new(errors_outside: usize, errors_inside: usize, level: usize) -> Self {
        Self {
            errors_outside,
            errors_inside,
            level: std::cmp::Reverse(level),
        }
    }

    /// Returns `true` when neither inside nor outside the edit region has errors.
    pub(crate) fn is_error_free(&self) -> bool {
        self.errors_outside == 0 && self.errors_inside == 0
    }
}

pub(crate) struct StrategyCandidate {
    pub score: CandidateScore,
    pub green: usize,
    pub messages: Arc<ParserMessages>,
    pub newly_computed_nodes: Vec<Span>,
    pub newly_computed_tokens: Vec<Span>,
    pub zipper: Zipper,
}

pub(crate) struct StrategyContext<'a> {
    pub parser: &'a mut Parser,
    pub span: Span,
    pub delta: isize,
    pub specs: Option<&'a RecoverySpecs>,
    pub recovery_strategy: Option<&'a ErrorRecoveryStrategy>,
    pub zippers: &'a [Zipper],
    pub config: ReparserConfig,
    pub metrics: Option<&'a mut EditMetrics>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EditKind {
    Insertion,
    Deletion,
    Update,
}

#[derive(Clone)]
struct MemoizedCandidate {
    green: usize,
    messages: Arc<ParserMessages>,
    newly_computed_nodes: Vec<Span>,
    newly_computed_tokens: Vec<Span>,
    errors_inside: usize,
    errors_outside: usize,
}

pub(crate) fn pick_candidate(
    ctx: &mut StrategyContext,
    edit_span: Span,
    kind: EditKind,
) -> Option<StrategyCandidate> {
    let eval_start = if ctx.metrics.is_some() {
        Some(std::time::Instant::now())
    } else {
        None
    };

    let mut best: Option<StrategyCandidate> = None;
    let mut best_with_clean_surrounding: Option<StrategyCandidate> = None;
    let mut candidates_evaluated = 0;
    let mut candidate_memo: FxHashMap<(usize, usize, usize, bool), Option<MemoizedCandidate>> =
        FxHashMap::default();

    for zipper in ctx.zippers.iter().rev() {
        if zipper.level < ctx.config.min_level {
            continue;
        }

        let candidate = match kind {
            EditKind::Insertion => {
                evaluate_candidate(ctx, zipper, edit_span, true, &mut candidate_memo)
            }
            EditKind::Deletion => {
                evaluate_candidate(ctx, zipper, edit_span, false, &mut candidate_memo)
            }
            EditKind::Update => {
                evaluate_candidate(ctx, zipper, edit_span, true, &mut candidate_memo)
            }
        };

        let Some(candidate) = candidate else {
            continue;
        };

        candidates_evaluated += 1;

        let is_clean = candidate.score.errors_outside == 0;
        if is_clean {
            if should_replace(&best_with_clean_surrounding, &candidate) {
                best_with_clean_surrounding = Some(candidate);
                if best_with_clean_surrounding
                    .as_ref()
                    .is_some_and(|c| c.score.errors_inside == 0)
                {
                    break;
                }
            }
        } else if should_replace(&best, &candidate) {
            best = Some(candidate);
        }
    }

    if let Some(m) = &mut ctx.metrics {
        m.candidates_evaluated = candidates_evaluated;
        if let Some(start) = eval_start {
            m.candidate_evaluation_us = start.elapsed().as_micros();
        }
    }

    best_with_clean_surrounding.or(best)
}

fn should_replace(best: &Option<StrategyCandidate>, candidate: &StrategyCandidate) -> bool {
    match best {
        None => true,
        Some(best_candidate) => candidate.score < best_candidate.score,
    }
}

fn evaluate_candidate(
    ctx: &mut StrategyContext,
    zipper: &Zipper,
    edit_span: Span,
    enforce_region_end: bool,
    memo: &mut FxHashMap<(usize, usize, usize, bool), Option<MemoizedCandidate>>,
) -> Option<StrategyCandidate> {
    let expected_width_signed = zipper.old_width as isize + ctx.delta;
    if expected_width_signed < 0 {
        return None;
    }
    let expected_width = expected_width_signed as usize;

    let memo_key = (
        zipper.rule_ix,
        zipper.offset,
        expected_width,
        enforce_region_end,
    );
    if let Some(cached) = memo.get(&memo_key) {
        let cached = cached.as_ref()?;
        return Some(StrategyCandidate {
            score: CandidateScore {
                errors_outside: cached.errors_outside,
                errors_inside: cached.errors_inside,
                level: std::cmp::Reverse(zipper.level),
            },
            green: cached.green,
            messages: Arc::clone(&cached.messages),
            newly_computed_nodes: cached.newly_computed_nodes.clone(),
            newly_computed_tokens: cached.newly_computed_tokens.clone(),
            zipper: zipper.clone(),
        });
    }

    ctx.parser.messages.clear();
    ctx.parser.newly_computed_nodes.clear();
    ctx.parser.newly_computed_tokens.clear();

    ctx.parser.set_insert_pos(None);

    let parse_start = if ctx.metrics.is_some() {
        Some(std::time::Instant::now())
    } else {
        None
    };

    let parse_result = ctx
        .parser
        .parse_rule(zipper.rule_ix, zipper.offset, expected_width);

    // Stop timer before early-return so failed parses are also measured.
    if let Some(m) = &mut ctx.metrics {
        if let Some(start) = parse_start {
            m.parse_rule_total_us += start.elapsed().as_micros();
        }
    }

    let new_green = parse_result?;

    let new_width = {
        let new_node = ctx.parser.alloc.get_node(new_green);
        // Reject Incomplete: parse_rule gave up completely — never a valid candidate.
        // UnexpectedToken/MissingToken roots are valid error-recovery outputs; Incomplete is not.
        if matches!(new_node.tag, Tag::Error(ParsecError::Incomplete)) {
            memo.insert(memo_key, None);
            return None;
        }
        new_node.width
    };

    if new_width != expected_width {
        memo.insert(memo_key, None);
        return None;
    }

    let check_end = zipper.offset + new_width;

    if enforce_region_end && ctx.config.enforce_region_end {
        let _ = (ctx.specs, check_end);
    }

    if let Some(strategy) = ctx.recovery_strategy {
        if let Some(sync_point) = strategy.find_sync_point(ctx.parser.text(), ctx.span.end) {
            let is_insertion = enforce_region_end;
            let insertion_spans_sync_point =
                is_insertion && ctx.span.start <= sync_point && check_end > sync_point;

            if !insertion_spans_sync_point
                && sync_point >= zipper.offset
                && check_end > sync_point
                && ctx.config.enforce_sync_bound
            {
                memo.insert(memo_key, None);
                return None;
            }
        }
    }

    let (mut errors_inside, mut errors_outside) = count_errors(&ctx.parser.messages, edit_span);

    if ctx.config.min_level > 0 {
        errors_inside = 0;
        errors_outside = 0;
    }

    let score = CandidateScore::new(errors_outside, errors_inside, zipper.level);

    let messages = Arc::new(ctx.parser.messages.clone());
    let newly_computed_nodes = ctx.parser.newly_computed_nodes();
    let newly_computed_tokens = ctx.parser.newly_computed_tokens();

    memo.insert(
        memo_key,
        Some(MemoizedCandidate {
            green: new_green,
            messages: Arc::clone(&messages),
            newly_computed_nodes: newly_computed_nodes.clone(),
            newly_computed_tokens: newly_computed_tokens.clone(),
            errors_inside,
            errors_outside,
        }),
    );

    Some(StrategyCandidate {
        score,
        green: new_green,
        messages,
        newly_computed_nodes,
        newly_computed_tokens,
        zipper: zipper.clone(),
    })
}

fn count_errors(messages: &ParserMessages, edit_span: Span) -> (usize, usize) {
    let mut inside = 0usize;
    let mut outside = 0usize;

    for msg in messages {
        let span = msg.span;
        let overlaps = span.start < edit_span.end && span.end > edit_span.start;
        if overlaps {
            inside += 1;
        } else {
            outside += 1;
        }
    }

    (inside, outside)
}
