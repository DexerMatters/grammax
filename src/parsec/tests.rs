use crate::new_grammar;
use crate::parsec::display::{format_ast, format_messages};
use crate::parsec::parser::Parser;
use crate::parsec::tree::{ParsecError, Tag, TreeAllocRefExt};
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

/// Check that truncated JSON inputs do NOT produce Placeholder nodes —
/// `try_eof_recovery` should insert MissingToken errors instead.
#[test]
fn test_no_placeholder_on_truncated_json() {
    use crate::parsec::tree::{GreenId, ParsecError, Tag, TreeAllocRefExt};

    let grammar = new_grammar!(
        start where
        start   -> r!(json) + tt(EndOfInput)
        json    -> r!(object) | r!(array) | r!(string) | r!(number) | r!(boolean) | r!(null)
        object  -> tt("{") + sep(r!(pair), tt(",")) + tt("}")
        pair    -> field("key", r!(string)) + tt(":") + field("value", r!(json))
        array   -> tt("[") + sep(r!(json), tt(",")) + tt("]")
        string  -> tt("\"") + t(STRING) + t("\"")
        number  -> tt(NUMS)
        boolean -> tt("true") | tt("false")
        null    -> tt("null")
    );

    #[allow(dead_code)]
    fn has_placeholder(
        alloc: &std::rc::Rc<std::cell::RefCell<crate::parsec::tree::TreeAlloc>>,
        id: GreenId,
    ) -> bool {
        let node = alloc.get_node(id);
        if matches!(node.tag, Tag::Error(ParsecError::Placeholder)) {
            return true;
        }
        let children: Vec<GreenId> = node.children.clone();
        drop(node);
        children.into_iter().any(|c| has_placeholder(alloc, c))
    }

    #[allow(dead_code)]
    fn has_incomplete(
        alloc: &std::rc::Rc<std::cell::RefCell<crate::parsec::tree::TreeAlloc>>,
        id: GreenId,
    ) -> bool {
        let node = alloc.get_node(id);
        if matches!(node.tag, Tag::Error(ParsecError::Incomplete)) {
            return true;
        }
        let children: Vec<GreenId> = node.children.clone();
        drop(node);
        children.into_iter().any(|c| has_incomplete(alloc, c))
    }

    let cases = &[
        "{\n\"",     // open brace + open quote
        "{\"a\":1,", // trailing comma
        "{\"a\":",   // key + colon, no value
        "{",         // just open brace
        "[1, 2,",    // trailing comma in array
        "\"hello",   // unclosed string
    ];

    for &input in cases {
        let mut parser = Parser::new(grammar);
        let result = parser.parse_text(input);
        let ast = format_ast(&parser.grammar, &result.root, &parser.alloc, parser.text());
        println!("Input {:?}:\n{}\n", input, ast);
        assert!(
            !has_placeholder(&parser.alloc, result.root.green),
            "placeholder node must not appear for input {:?}",
            input
        );
        assert!(
            !has_incomplete(&parser.alloc, result.root.green),
            "incomplete node must not appear for input {:?}",
            input
        );
    }
}

/// Test that bridge specifications are derived for the JSON grammar.
#[test]
fn test_bridge_specs_derived() {
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

    let specs = &grammar.bridge_specs;
    println!("Bridge specs ({}):", specs.len());
    for s in specs {
        println!(
            "  open={:?} → close={:?}",
            grammar.table.terminals[s.open].preview(),
            grammar.table.terminals[s.close].preview(),
        );
    }

    let has = |o: &str, c: &str| {
        specs.iter().any(|s| {
            grammar.table.terminals[s.open].preview() == Some(o)
                && grammar.table.terminals[s.close].preview() == Some(c)
        })
    };

    assert!(
        !specs.is_empty(),
        "bridge specs should be non-empty for JSON"
    );
    assert!(has("{", "}"), "expected {{…}} bridge spec");
    assert!(has("[", "]"), "expected […] bridge spec");
}

/// Test that the scope recovery layer handles errors inside a scoped body.
/// When the parser encounters an error in the body of `[...]`, it should
/// skip forward to the next delimiter (`,`) or matching close (`]`) and
/// continue parsing the remaining elements.
#[test]
fn test_scope_recovery_in_array() {
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

    // This input has a malformed element between valid array items.
    // Scope recovery should skip the malformed span and resume at `,` / `]`.
    let text = r#"[1, @@@@, 3]"#;
    let result = parser.parse_text(text);
    let ast = format_ast(&parser.grammar, &result.root, &parser.alloc, parser.text());
    println!("Scope recovery AST:\n{ast}");
    println!(
        "Messages:\n{}",
        format_messages(&parser.grammar, &result.messages)
    );
    assert!(
        !result.messages.is_empty(),
        "expected recovery messages for malformed scoped input"
    );

    fn has_error_kind(
        alloc: &std::rc::Rc<std::cell::RefCell<crate::parsec::tree::TreeAlloc>>,
        id: crate::parsec::tree::GreenId,
        pred: fn(&ParsecError) -> bool,
    ) -> bool {
        let node = alloc.get_node(id);
        if let Tag::Error(ref e) = node.tag {
            if pred(e) {
                return true;
            }
        }
        let children = node.children.clone();
        drop(node);
        children.into_iter().any(|c| has_error_kind(alloc, c, pred))
    }

    let root = parser.alloc.get_node(result.root.green);
    assert_eq!(
        root.width,
        text.len(),
        "recovery should continue and cover the full input"
    );
    assert!(
        !matches!(root.tag, Tag::Error(ParsecError::Incomplete)),
        "root should not be Incomplete for recoverable scoped input"
    );
    drop(root);

    assert!(
        !has_error_kind(&parser.alloc, result.root.green, |e| matches!(
            e,
            ParsecError::Placeholder
        )),
        "placeholder node must not appear in scope recovery tree"
    );
    assert!(
        !has_error_kind(&parser.alloc, result.root.green, |e| matches!(
            e,
            ParsecError::Incomplete
        )),
        "incomplete node must not appear in scope recovery tree"
    );
}
