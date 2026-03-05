use crate::new_grammar;
use crate::parsec::display::{format_ast, format_messages};
use crate::parsec::parser::Parser;
use crate::parsec::words::{EndOfInput, NUMS, STRING};

#[test]
fn test_simple_whitespaces() {
    let grammar = new_grammar!(
        start where
        start -> r!(expr) + tt(EndOfInput)
        expr -> r!(list1) | r!(list2)
        id -> tt(NUMS)
        list1 -> tt("(") + sep(r!(id), tt(",")) + tt(")")
        list2 -> tt("(") + sep(r!(id), t(" ")) + tt(")")
    );

    let mut parser = Parser::new(grammar);

    println!("Grammar:\n{}", parser.grammar.table);

    let text = "(1  22)";
    let result = parser.parse_text(text);

    let output = format_ast(&parser.grammar, &result.root, &parser.alloc, parser.text());
    println!("AST {}:\n{}", text, output);
    println!(
        "Messages:\n{}",
        format_messages(&parser.grammar, &result.messages)
    );

    assert!(
        result.messages.is_empty(),
        "Expected no errors, got: {}",
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
fn test_simple_arithmetic_precedence() {
    let grammar = new_grammar!(
        start where
        start -> r!(expr) + tt(EndOfInput)
        expr -> r!(add) | r!(mul) | r!(primary)
        add  -> field("lhs:", r!(expr)) + tt("+") + field("rhs:", r!(expr).drop(1))
        mul  -> field("lhs:", r!(expr).drop(1)) + tt("*") + field("rhs:", r!(expr).drop(2))
        primary -> tt(NUMS) | tt("(") + r!(expr) + tt(")")
    );

    let mut parser = Parser::new(grammar);

    println!("Grammar:\n{}", parser.grammar.table);

    let text = "1+1+1";
    let result = parser.parse_text(text);

    let output = format_ast(&parser.grammar, &result.root, &parser.alloc, parser.text());
    println!("AST {}:\n{}", text, output);
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
        string  -> tt("\"") + t(STRING) + t("\"")
        number  -> tt(NUMS)
        boolean -> tt("true") | tt("false")
        null    -> tt("null")
    );

    let mut parser = Parser::new(grammar);

    println!("Grammar:\n{}", parser.grammar.table);

    let text = r#"{
        "a": 12
    
    "#;
    let result = parser.parse_text(text);

    let output = format_ast(&parser.grammar, &result.root, &parser.alloc, parser.text());
    println!("AST:\n{}", output);
    println!(
        "Messages:\n{}",
        format_messages(&parser.grammar, &result.messages)
    );
}
