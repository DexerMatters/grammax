use crate::new_grammar;

#[test]
fn test_grammar_norm() {
    let grammar = new_grammar!(
        expr where
        expr   -> (r!(expr) | t(())) + t("+") + r!(mul) | r!(mul)
        mul    -> t("num")
    );

    println!("===== Grammar =====");
    println!("{}", grammar);

    println!("\n===== States =====");
    for (ix, state) in grammar.analysis.states.iter().enumerate() {
        println!("State {}: {:?}", ix, state);
    }
}

#[test]
fn test_left_recursion_elimination() {
    let grammar = new_grammar!(
        expr where
        expr   -> r!(expr) + t("+") + r!(term) | r!(expr) + t("-") + r!(term) | r!(term)
        term   -> t("num")
    );

    println!("===== Left-Recursion Elimination Test =====");
    println!("{}", grammar);

    // After elimination, we should have:
    // expr → term expr'
    // expr' → "+" term expr' | "-" term expr' | ε
    // term → "num"

    println!("\nRules count: {}", grammar.table.rules.len());
    println!("Rule names: {:?}", grammar.table.rule_names);
}

#[test]
fn test_left_recursion_single_branch() {
    let grammar = new_grammar!(
        expr where
        expr   -> r!(expr) + t("op") | t("base")
    );

    println!("===== Single Left-Recursive Branch =====");
    println!("{}", grammar);

    // Should transform: expr → base expr' ; expr' → op expr' | ε
    assert_eq!(
        grammar.table.rules.len(),
        2,
        "Should have 2 rules after LR elimination"
    );
}

#[test]
fn test_no_left_recursion() {
    let grammar = new_grammar!(
        expr where
        expr   -> r!(term) + t("+") | r!(term)
        term   -> t("num")
    );

    println!("===== No Left-Recursion (Right-Recursive) =====");
    println!("{}", grammar);

    // Should remain unchanged since there's no left-recursion
    assert_eq!(grammar.table.rules.len(), 2, "Should have 2 rules (no LR)");
}
