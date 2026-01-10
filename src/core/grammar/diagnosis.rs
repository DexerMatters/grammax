use crate::{
    grammar::{EvaluationError, Rule},
    grammar_dsl::*,
};

pub fn diagnose_rule(rule: &Rule) -> Option<EvaluationError> {
    // Non-terminable rules
    if rule.node.never_halt() {
        return Some(EvaluationError::UndecidableRule(rule.name.to_string()));
    } else if rule.node.never_succeed() {
        return Some(EvaluationError::AlwaysFails(rule.name.to_string()));
    }

    None
}
