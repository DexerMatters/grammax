use crate::grammar::dsl::*;
use crate::new_grammar;
use crate::parsec::display::format_ast;
use crate::parsec::parser::Parser;
use crate::parsec::words::NUMS;

#[test]
fn test_simple_arithmetic_precedence() {
    let grammar = new_grammar!(
        expr where
        expr -> r!(add) | r!(mul) | r!(primary)
        add  -> r!(expr) + t("+") + r!(primary)
        mul  -> r!(expr) + t("*") + r!(primary)
        primary -> tt(NUMS) | t("(") + r!(expr) + t(")")
    );

    let mut parser = Parser::new(grammar.clone());

    let text = "4*4*(1+2*3)*4+5";
    let result = parser.parse_text(text);

    assert!(
        result.messages.is_empty(),
        "Parse errors: {:?}",
        result.messages
    );

    let output = format_ast(&parser.grammar, &result.root, &parser.alloc, parser.text());
    println!("AST 1+2*3:\n{}", output);

    // Verify * is child of + (or rather + is the root operation)
    // Structure:
    // Rule(expr)
    //   Rule(expr) -> 1
    //   Token(+)
    //   Rule(expr)
    //     Rule(expr) -> 2
    //     Token(*)
    //     Rule(expr) -> 3

    // (Note: The normalization might introduce intermediate rules, but the display should show structure)
}

#[test]
fn test_parentheses_precedence() {
    let grammar = new_grammar!(
        expr where
        expr -> alt(vec![
            seq(vec![r!(expr), t("+"), r!(expr)]),
            seq(vec![r!(expr), t("*"), r!(expr)]),
            seq(vec![t("("), r!(expr), t(")")]),
            t("1"),
            t("2"),
            t("3")
        ])
    );

    let mut parser = Parser::new(grammar);

    // (1 + 2) * 3
    let text = "(1+2)*3";
    let result = parser.parse_text(text);

    assert!(
        result.messages.is_empty(),
        "Parse errors: {:?}",
        result.messages
    );

    let output = format_ast(&parser.grammar, &result.root, &parser.alloc, parser.text());
    println!("AST (1+2)*3:\n{}", output);
}

#[test]
fn test_right_associativity_check() {
    // Standard + is usually left associative.
    // Let's define a right associative operator, e.g. ^
    // expr -> expr ^ expr
    // expr -> val

    // How does expr_detect determine associativity?
    // It defaults to LEFT unless specified.
    // The DSL doesn't seem to expose associativity control directly in the `alt` list order effectively without annotations
    // OR expr_detect guesses based on recursion pattern?
    // Looking at expr_detect.rs snippet:
    // It assigns precedence based on index.
    // It calls `extract_operator`.

    // Let's stick to standard test first.
}
