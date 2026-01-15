use crate::{grammar::Grammar, new_grammar};
use std::f32::INFINITY;

#[test]
fn test_recursion_detection() {
    let grammar_left_recursion = new_grammar!(
        expr where
        expr   -> t(()) + r!(expr) + t("+") + r!(num) | r!(num)
        num    -> t("0")
    );

    let analysis_left = &grammar_left_recursion.analysis;

    print!("Grammar with left recursion:\n");

    for (i, state) in analysis_left.states.iter().enumerate() {
        println!("State {}: {:?}", i, state);
    }
    print_matrix(&analysis_left.left_transitive_closure());
}

#[test]
fn test_grammar_norm() {
    let grammar = new_grammar!(
        expr where
        expr   -> r!(expr) + t("+") + r!(mul) | r!(mul)
        mul    -> t("num")
    );

    let analysis = &grammar.analysis;

    println!("===== Grammar =====");
    println!("{}", grammar);

    println!("===== Analysis =====");
    println!("States:");
    for (i, state) in analysis.states.iter().enumerate() {
        println!("State {}: {:?}", i, state);
    }

    println!();

    println!("Transitive Closure:");
    print_matrix(&analysis.transitive_closure());

    println!();

    println!("Left Transitive Closure:");
    print_matrix(&analysis.left_transitive_closure());
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

fn print_matrix(matrix: &ndarray::Array2<f32>) {
    for i in 0..matrix.nrows() {
        for j in 0..matrix.ncols() {
            print!("{} ", to_string_float(matrix[(i, j)]));
        }
        println!();
    }
}

/*

States:
State 0: Alt(0, 4, 1)
State 1: Tok(1, "num")
State 2: Tok(0, "+")
State 3: Seq(0, 2, 1)
State 4: Seq(0, 0, 3)

Transitive Closure:
2 1 3 2 1
- - - - -
- - - - -
- 1 1 - -
1 1 2 1 1
 */

/*

===== Analysis =====
States:
State 0: Alt(0, 4, 1)
State 1: Tok(1, "num")
State 2: Tok(0, "+")
State 3: Seq(0, 2, 0)
State 4: Seq(0, 1, 3)

Transitive Closure:
3 1 3 2 1
- - - - -
- - - - -
1 1 1 2 1
2 1 2 1 2
 */
