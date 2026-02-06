use std::sync::Arc;

use crate::{
    grammar::recovery::{ErrorRecoveryStrategy, RecoverySpecs},
    parsec::{Parser, msg::ParserMessages, tree::TreeAllocRefExt},
    runtime::reparser::{ReparserConfig, Zipper},
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
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum EditKind {
    Insertion,
    Deletion,
    Update,
}

pub(crate) fn pick_candidate(
    ctx: &mut StrategyContext,
    edit_span: Span,
    kind: EditKind,
) -> Option<StrategyCandidate> {
    let mut best: Option<StrategyCandidate> = None;
    let best_with_clean_surrounding: Option<StrategyCandidate> = None;

    for zipper in ctx.zippers.iter().rev() {
        if zipper.level < ctx.config.min_level {
            continue;
        }

        let candidate = match kind {
            EditKind::Insertion => evaluate_candidate(ctx, zipper, edit_span, true),
            EditKind::Deletion => evaluate_candidate(ctx, zipper, edit_span, false),
            EditKind::Update => evaluate_candidate(ctx, zipper, edit_span, true),
        };

        let Some(candidate) = candidate else {
            continue;
        };

        let is_clean = candidate.score.errors_outside == 0;
        if is_clean {
            if should_replace(&best_with_clean_surrounding, &candidate) {
                return Some(candidate);
            }
        } else if should_replace(&best, &candidate) {
            best = Some(candidate);
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
) -> Option<StrategyCandidate> {
    ctx.parser.messages.clear();

    ctx.parser.set_incremental_insert_pos(None);

    let new_green = ctx.parser.parse_rule(zipper.rule_ix, zipper.offset)?;

    let new_width = {
        let new_node = ctx.parser.alloc.get_node(new_green);
        new_node.width
    };

    let expected_width = (zipper.old_width as isize + ctx.delta) as usize;

    if new_width != expected_width {
        return None;
    }

    let check_end = zipper.offset + new_width;

    if enforce_region_end {
        if let Some(specs) = ctx.specs {
            if let Some(region_end) = specs.region_end_at(ctx.span.start) {
                if check_end > region_end && ctx.config.enforce_region_end {
                    return None;
                }
            }
        }
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
                return None;
            }
        }
    }

    let (mut errors_inside, mut errors_outside) = count_errors(&ctx.parser.messages, edit_span);

    if ctx.config.min_level > 0 {
        errors_inside = 0;
        errors_outside = 0;
    }

    if errors_inside + errors_outside > 0 {
        if let (Some(strategy), Some(state_id)) = (
            ctx.recovery_strategy,
            ctx.parser
                .grammar
                .analysis
                .state_id_for_rule(zipper.rule_ix),
        ) {
            if !strategy.can_recover_at(state_id) {
                errors_outside += 1;
            }
        }
    }

    let level = std::cmp::Reverse(zipper.level);

    let score = CandidateScore {
        errors_outside,
        errors_inside,
        level,
    };

    Some(StrategyCandidate {
        score,
        green: new_green,
        messages: Arc::new(ctx.parser.messages.clone()),
        newly_computed_nodes: ctx.parser.newly_computed_nodes(),
        newly_computed_tokens: ctx.parser.newly_computed_tokens(),
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
