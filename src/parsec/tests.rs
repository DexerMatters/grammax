use crate::new_grammar;
use crate::parsec::display::{format_ast, format_messages};
use crate::parsec::parser::Parser;
use crate::parsec::tree::{Tag, TreeAllocRefExt};
use crate::parsec::words::{EndOfInput, NUMS, STRING};

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

    let text = "1+4*3+4";
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
        "name": "John",
        "age": 30,
        "isStudent": 33,
        "scores": [85, 90, 92],
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

#[test]
fn test_parse_rule_partial_pair() {
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
    let text = r#"{"name": "Dexer", "age": 30}"#;
    let _ = parser.parse_text(text);

    let pair_ix = parser
        .grammar
        .table
        .rules
        .iter()
        .position(|r| r.name == "pair")
        .expect("pair rule exists");

    let target = r#" "age": 30"#;
    let start = text.find(target).expect("target pair exists");
    let green = parser
        .parse_rule(pair_ix, start, target.len())
        .expect("pair should parse as bounded rule");
    let node = parser.alloc.get_node(green);

    assert_eq!(node.width, target.len());
    assert!(matches!(node.tag, Tag::Rule { rule_ix } if rule_ix == pair_ix));
    assert!(parser.messages.is_empty());
}

#[test]
fn test_parse_text_primes_reuse_cache() {
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
    parser.configure_reuse(true, 512, true);
    parser.reset_reuse_stats();

    let text = r#"{"name": "Dexer"}"#;
    let _ = parser.parse_text(text);
    assert!(
        parser.reuse_stats().inserts > 0,
        "full parse should prime reuse cache"
    );

    let object_ix = parser
        .grammar
        .table
        .rules
        .iter()
        .position(|r| r.name == "object")
        .expect("object rule exists");

    let green = parser
        .parse_rule(object_ix, 0, text.len())
        .expect("object should parse from cache");
    assert_eq!(parser.alloc.get_node(green).width, text.len());
    assert!(
        parser.newly_computed_tokens().is_empty(),
        "cache hit should not report freshly computed tokens"
    );
}
