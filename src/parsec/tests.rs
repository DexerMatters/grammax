use crate::new_grammar;
use crate::parsec::display::{format_ast, format_messages};
use crate::parsec::parser::Parser;
use crate::parsec::tree::TreeAllocRefExt;
use crate::parsec::words::{EndOfInput, NUMS, STRING};

#[test]
fn test_simple_arithmetic_precedence() {
    let grammar = new_grammar!(
        start where
        start -> r!(expr)
        expr -> r!(add) | r!(mul) | r!(primary)
        add  -> field("lhs:", r!(expr)) + t("+") + field("rhs:", r!(expr).drop(1))
        mul  -> field("lhs:", r!(expr).drop(1)) + t("*") + field("rhs:", r!(expr).drop(2))
        primary -> tt(NUMS) | t("(") + r!(expr) + t(")")
    );

    let mut parser = Parser::new(grammar.clone());

    println!("Grammar:\n{}", parser.grammar.table);

    let text = "1+4*x+4";
    let result = parser.parse_text(text);

    let output = format_ast(&parser.grammar, &result.root, &parser.alloc, parser.text());
    println!("AST 1+4*3+4:\n{}", output);
    println!(
        "Messages:\n{}",
        format_messages(&parser.grammar, &result.messages)
    );

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
fn test_json() {
    let grammar = new_grammar!(
        json where
        json    -> r!(object) | r!(array) | r!(string) | r!(number) | r!(boolean) | r!(null)
        object  -> tt("{") + sep(r!(pair), tt(",")) + tt("}")
        pair    -> field("key", r!(string)) + tt(":") + field("value", r!(json))
        array   -> tt("[") + sep(r!(json), tt(",")) + tt("]")
        string  -> tt("\"") + t(STRING) + tt("\"")
        number  -> tt(NUMS)
        boolean -> tt("true") | tt("false")
        null    -> tt("null")
    );

    let mut parser = Parser::new(grammar.clone());

    println!("Grammar:\n{}", parser.grammar.table);

    let text = r#"{
        "name": "John",
        "age": 30,
        "isStudent": true,
        "scores": [85, x, 92],
        "address": {
            "street": "123 Main St",
            "city": "Anytown"
        },
        "nullValue": null
    }
    "#;
    let result = parser.parse_text(text);

    let output = format_ast(&parser.grammar, &result.root, &parser.alloc, parser.text());
    println!("AST:\n{}", output);
    println!(
        "Messages:\n{}",
        format_messages(&parser.grammar, &result.messages)
    );
}

#[test]
fn test_repetition() {
    let grammar = new_grammar!(
        start where
        start -> r!(expr)
        expr -> tt("[") + sep(field("arg:", r!(expr)), tt(",")) + tt("]") | tt(NUMS)
    );

    let mut parser = Parser::new(grammar);

    // Test with valid input first
    let text = "[1,[6, 4],3]";
    let result = parser.parse_text(text);
    println!("Grammar:\n{}", parser.grammar.table);
    println!(
        "AST (valid):\n{}",
        format_ast(&parser.grammar, &result.root, &parser.alloc, parser.text())
    );
    assert!(
        result.messages.is_empty(),
        "Expected no parse errors for valid input"
    );

    // Test with error input
    let text2 = "[1,[4, x],3]";
    let result2 = parser.parse_text(text2);
    println!(
        "\nAST (with error 1):\n{}",
        format_ast(&parser.grammar, &result2.root, &parser.alloc, parser.text())
    );
    println!(
        "Messages:\n{}",
        format_messages(&parser.grammar, &result2.messages)
    );

    // We expect exactly one error at the position of 'x'
    assert!(
        !result2.messages.is_empty(),
        "Expected parse errors for invalid input"
    );

    // Test with error input
    let text2 = "[1,[x, 4],3]";
    let result2 = parser.parse_text(text2);
    println!(
        "\nAST (with error 2):\n{}",
        format_ast(&parser.grammar, &result2.root, &parser.alloc, parser.text())
    );
    println!(
        "Messages:\n{}",
        format_messages(&parser.grammar, &result2.messages)
    );

    // We expect exactly one error at the position of 'x'
    assert!(
        !result2.messages.is_empty(),
        "Expected parse errors for invalid input"
    );
}

#[test]
fn test_right_associativity_check() {}
