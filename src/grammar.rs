use std::{collections::HashSet, ops};

use crate::words::Matcher;

#[derive(Debug, Clone, Copy)]
pub enum GrammarError {
    NoTermination,
}

pub type Result<T> = std::result::Result<T, GrammarError>;

pub type Rule = fn() -> Grammar;

pub enum Grammar {
    Terminal(Box<dyn Matcher>),
    Choice(Vec<Grammar>),
    Sequence(Vec<Grammar>),
    Reference(Rule),
}

impl Grammar {
    pub fn eval(&self) -> Option<GrammarError> {
        todo!()
    }

    pub fn is_terminable(&self) -> Option<Rule> {
        fn helper(g: &Grammar, visited: &mut HashSet<usize>) -> Option<Rule> {
            match g {
                Grammar::Terminal(_) => None,
                Grammar::Choice(gs) | Grammar::Sequence(gs) => {
                    for g in gs {
                        if let Some(r) = helper(g, visited) {
                            return Some(r);
                        }
                    }
                    None
                }
                Grammar::Reference(r) => {
                    let ptr = *r as usize;
                    if visited.contains(&ptr) {
                        return Some(*r);
                    }
                    visited.insert(ptr);
                    helper(&r(), visited)
                }
            }
        }
        let mut visited = HashSet::new();
        helper(self, &mut visited)
    }

    pub fn will_always_fail(&self) -> bool {}
}

pub fn t<M>(s: M) -> Grammar
where
    M: Matcher + 'static,
{
    Grammar::Terminal(Box::new(s))
}

pub fn r(f: fn() -> Grammar) -> Grammar {
    Grammar::Reference(f)
}

impl ops::Add for Grammar {
    type Output = Grammar;

    fn add(self, rhs: Grammar) -> Grammar {
        match (self, rhs) {
            (Grammar::Sequence(mut left_seq), Grammar::Sequence(right_seq)) => {
                left_seq.extend(right_seq);
                Grammar::Sequence(left_seq)
            }
            (Grammar::Sequence(mut left_seq), right) => {
                left_seq.push(right);
                Grammar::Sequence(left_seq)
            }
            (left, Grammar::Sequence(mut right_seq)) => {
                let mut new_seq = vec![left];
                new_seq.append(&mut right_seq);
                Grammar::Sequence(new_seq)
            }
            (left, right) => Grammar::Sequence(vec![left, right]),
        }
    }
}

impl ops::BitOr for Grammar {
    type Output = Grammar;

    fn bitor(self, rhs: Grammar) -> Grammar {
        match (self, rhs) {
            (Grammar::Choice(mut left_choices), Grammar::Choice(right_choices)) => {
                left_choices.extend(right_choices);
                Grammar::Choice(left_choices)
            }
            (Grammar::Choice(mut left_choices), right) => {
                left_choices.push(right);
                Grammar::Choice(left_choices)
            }
            (left, Grammar::Choice(mut right_choices)) => {
                let mut new_choices = vec![left];
                new_choices.append(&mut right_choices);
                Grammar::Choice(new_choices)
            }
            (left, right) => Grammar::Choice(vec![left, right]),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::any::type_name_of_val;

    use super::*;

    #[test]
    fn test_choice_combination() {
        fn test() -> Grammar {
            t("expr") | t("term") | t("factor")
        }

        println!("{}", type_name_of_val(&test));
    }

    #[test]
    fn test_is_terminable() {
        // A = A @ T and A = T @ A are terminable
        // A = A and A = B; B = A are non-terminable

        // Case 1: A = A + T (Sequence)
        fn rule_a_seq() -> Grammar {
            r(rule_a_seq) + t("T")
        }
        assert!(
            rule_a_seq().is_terminable().is_none(),
            "A = A + T should be terminable"
        );

        // Case 2: A = T + A (Sequence)
        fn rule_a_seq_2() -> Grammar {
            t("T") + r(rule_a_seq_2)
        }
        assert!(
            rule_a_seq_2().is_terminable().is_none(),
            "A = T + A should be terminable"
        );

        // Case 3: A = A | T (Choice)
        fn rule_a_choice() -> Grammar {
            r(rule_a_choice) | t("T")
        }
        assert!(
            rule_a_choice().is_terminable().is_none(),
            "A = A | T should be terminable"
        );

        // Case 4: A = T | A (Choice)
        fn rule_a_choice_2() -> Grammar {
            t("T") | r(rule_a_choice_2)
        }
        assert!(
            rule_a_choice_2().is_terminable().is_none(),
            "A = T | A should be terminable"
        );

        // Case 5: A = A (Loop)
        fn rule_loop() -> Grammar {
            r(rule_loop)
        }
        assert!(
            rule_loop().is_terminable().is_some(),
            "A = A should be non-terminable"
        );

        // Case 6: A = B; B = A (Mutual Loop)
        fn rule_b() -> Grammar {
            r(rule_a_mutual)
        }
        fn rule_a_mutual() -> Grammar {
            r(rule_b)
        }
        assert!(
            rule_a_mutual().is_terminable().is_some(),
            "A = B; B = A should be non-terminable"
        );
    }
}
