use crate::{
    new_grammar,
    parsec::{
        Parser,
        fmt::Display,
        words::{EndOfInput, NUMS, STRING, token},
    },
};

#[test]
fn test_parser() {
    let grammar = new_grammar!(
        start where
        start  -> r!(expr) + t(EndOfInput)
        expr   -> r!(expr) + t("+") + r!(num) | r!(expr) + t("-") + r!(num) | r!(num)
        num    -> t("x")
    );

    println!("Grammar:\n{}", grammar);
    let mut parser = Parser::new(grammar);
    let result = parser.parse_text("x+x-x+x");
    println!("{}", result.root.display(&parser));
    println!("Messages:");
    println!("{}", result.messages.display(&parser));
}

#[test]
fn test_parser_many_grammar() {
    let grammar = new_grammar!(
        absurd where
        absurd -> (r!(absurd) + t("a")) | t(EndOfInput)
    );
    println!("Grammar:\n{}", grammar);
    let mut parser = Parser::new(grammar);
    let root = parser.parse_text("aaaaaa").root;
    println!("{}", root.display(&parser));

    let grammar2 = new_grammar!(
        absurd where
        absurd -> many(t("a")) + t(EndOfInput)
    );
    let mut parser2 = Parser::new(grammar2);
    let root2 = parser2.parse_text("aaaaaa").root;
    println!("{}", root2.display(&parser2));
}

#[test]
fn test_error_recovery() {
    let grammar = new_grammar!(
        tuple where
        tuple -> t("(") + sep(t(token(NUMS)), t(",")) + t(")")
    );

    println!("Grammar:\n{}", grammar);
    let mut parser = Parser::new(grammar);
    let result = parser.parse_text("(12,3x3,44)");
    let output = result.root.display(&parser);
    println!("{}", output);
    println!("Messages:");
    println!("{}", result.messages.display(&parser));
}

#[test]
fn test_statement_parser() {
    let grammar = new_grammar!(
        start where
        start -> many(r!(stmt) + t(token(";"))) + t(token(EndOfInput))
        stmt  -> r!(assignment) | r!(if_stmt)
        assignment -> t(token("x")) + t(token("=")) + t(token(NUMS))
        if_stmt -> t(token("if")) + r!(cond) + r!(block)
        cond -> t(token("(")) + t(token("true")) + t(token(")"))
        block -> t(token("{")) + many(r!(stmt) + t(token(";"))) + t(token("}"))
    )
    .in_which("block", "a code block that can contain statements")
    .in_which(
        "cond",
        "A condition that must be true for the if statement to execute",
    );

    println!("Grammar:\n{}", grammar);

    let mut parser = Parser::new(grammar);
    let code = r#"
        x = 10;
        x = 20;
        if (true)
        "#;
    let result = parser.parse_text(code);
    println!("{}", result.root.display(&parser));
    println!("Messages:");
    println!("{}", result.messages.display(&parser));
}

#[test]
fn test_json_error_recovery() {
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

    println!("Grammar:\n{}", grammar);

    let mut parser = Parser::new(grammar);
    let code = r#"{"name": "ok", "age":nulll}"#;
    let result = parser.parse_text(code);
    println!("{}", result.root.display(&parser));
    println!("Messages:");
    println!("{}", result.messages.display(&parser));
}

#[test]
fn test_selection_schema_current() {
    let grammar = new_grammar!(
        start where
        start -> r!(stmt) + t(token(EndOfInput))
        stmt -> r!(assignment) | r!(if_stmt)
        assignment -> t(token("x")) + t(token("=")) + t(token(NUMS))
        if_stmt -> t(token("if")) + r!(cond) + r!(block)
        cond -> t(token("(")) + t(token("true")) + t(token(")"))
        block -> t(token("{")) + many(r!(stmt)) + t(token("}"))
    );

    let code = r#"
    if (true) {
        if (true) {
            x = 4
        
    }"#;

    let mut parser = Parser::new(grammar);
    println!("Selection Schema - Current Structure Test");
    println!("Code: {}", code);
    let result = parser.parse_text(code);
    println!("{}", result.root.display(&parser));
    println!("Messages:");
    println!("{}", result.messages.display(&parser));
}
