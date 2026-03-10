use crate::new_grammar;
use crate::parsec::display::{format_ast, format_messages};
use crate::parsec::parser::Parser;
use crate::parsec::tree::TreeAllocRefExt;
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

    let output = result.format_ast();
    println!("AST {}:\n{}", text, output);
    println!("Messages:\n{}", result.format_messages());

    assert!(
        result.messages.is_empty(),
        "Expected no errors, got: {}",
        result.format_messages()
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

    let text = "ddd+5";
    let result = parser.parse_text(text);

    let output = result.format_ast();
    println!("AST {}:\n{}", text, output);
    println!("Messages:\n{}", result.format_messages());

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
"a"#;
    let result = parser.parse_text(text);

    println!("AST:\n{}", result.format_ast());
    println!("Messages:\n{}", result.format_messages());
}

#[test]
fn test_recovery_strategy_is_wired_for_current_text() {
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

    let mut parser = Parser::new(grammar);
    parser.set_text("{\n  \"k\": 1,\n  \"v\": 2\n}");

    let specs = parser.recovery_specs().expect("recovery specs");
    assert!(!specs.regions.is_empty());
    assert!(specs.strategy.sync_tokens.len() >= 2);
}

#[test]
fn test_parse_text_handles_closing_quote_after_partial_string() {
    let grammar = new_grammar!(
        start where
        start   -> r!(json) + tt(EndOfInput)
        json    -> r!(object) | r!(array) | r!(string) | r!(number) | r!(boolean) | r!(null)
        object  -> tt("{") + sep(r!(pair), tt(",")) + tt("}")
        pair    -> field("key", r!(string)) + tt(":") + field("value", r!(json))
        array   -> tt("[") + sep(r!(json), tt(",")) + tt("]")
        string  -> tt("\"") + t(STRING) + tt("\"")
        number  -> tt(NUMS)
        boolean -> tt("true") | tt("false")
        null    -> tt("null")
    );

    let mut parser = Parser::new(grammar);
    let result = parser.parse_text("{\"a\"");
}
