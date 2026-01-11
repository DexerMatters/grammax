use crate::grammar::Grammar;
use std::f32::INFINITY;

#[test]
fn test_grammar_norm() {
    use crate::{grammar::dsl::*, r};
    fn expr() -> GrammarNode {
        r!(expr) + t("+") + r!(number) | r!(number)
    }

    fn number() -> GrammarNode {
        t("xx")
    }

    let grammar = Grammar::new(expr(), "expr");
    let analysis = &grammar.analysis;

    println!("Grammar:");
    println!("{}", grammar);

    println!("States:");
    for (i, state) in analysis.states.iter().enumerate() {
        println!("State {}: {:?}", i, state);
    }

    println!("Transitive Closure:");
    let closure = analysis.transitive_closure();
    for i in 0..closure.nrows() {
        for j in 0..closure.ncols() {
            print!("{} ", to_string_float(closure[(i, j)]));
        }
        println!();
    }
}

fn to_string_float(value: f32) -> String {
    if value == INFINITY {
        "∞".to_string()
    } else if value.is_nan() {
        "-".to_string()
    } else {
        format!("{}", value)
    }
}
