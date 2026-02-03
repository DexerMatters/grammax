use crate::{
    new_grammar,
    parsec::{
        parser::{Parser, ParserConfig},
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
    let mut parser = Parser::new_with_config("x+x-x+x", &grammar, ParserConfig::recovering());
    let green = parser.parse_text().green;
    println!("States:");
    for (i, state) in grammar.analysis.states.iter().enumerate() {
        println!("State {}: {:?}", i, state);
    }
    println!(
        "{}",
        parser.display_with_rules(green, |ix| grammar.name(ix).to_string())
    );
}

#[test]
fn test_parser_many_grammar() {
    let grammar = new_grammar!(
        absurd where
        absurd -> (r!(absurd) + t("a")) | t(EndOfInput)
    );
    println!("Grammar:\n{}", grammar);
    let mut parser = Parser::new("aaaaaa", &grammar);
    let green = parser.parse_text().green;
    println!(
        "{}",
        parser.display_with_rules(green, |ix| grammar.name(ix).to_string())
    );

    let grammar2 = new_grammar!(
        absurd where
        absurd -> many(t("a")) + t(EndOfInput)
    );
    let mut parser2 = Parser::new_with_config("aaaaaa", &grammar2, ParserConfig::recovering());
    let green2 = parser2.parse_text().green;

    println!("Grammar:\n{}", grammar2);
    println!(
        "{}",
        parser2.display_with_rules(green2, |ix| grammar2.name(ix).to_string())
    );
}

#[test]
fn test_error_recovery() {
    let grammar = new_grammar!(
        tuple where
        tuple -> t("(") + sep(t(token(NUMS)), t(",")) + t(")")
    );

    println!("Grammar:\n{}", grammar);
    let mut parser = Parser::new_with_config("(12,3x3,44)", &grammar, ParserConfig::recovering());
    let green = parser.parse_text().green;
    let output = parser.display_with_rules(green, |ix| grammar.name(ix).to_string());
    println!("{}", output);
}

#[test]
fn test_json_parser() {
    let grammar = new_grammar!(
        json where
        json    -> r!(object) | r!(array) | r!(string) | r!(number) | r!(boolean) | r!(null)
        object  -> t(token("{")) + sep(r!(pair), t(token(","))) + t(token("}"))
        pair    -> r!(string) + t(token(":")) + r!(json)
        array   -> t(token("[")) + sep(r!(json), t(token(","))) + t(token("]"))
        string  -> t(token("\"")) + t(STRING) + t("\"")
        number  -> t(token(NUMS))
        boolean -> t(token("true")) | t(token("false"))
        null    -> t(token("null"))
    );

    let mut parser = Parser::new_with_config(
        r#"{
            "name": "John Doe",
            "is_student": fase,
            "details": {
                "id": 1234x5,
                "major": "Computer Science",
                "dick_length": 17
            },
            "courses": ["Math", "Science", "Art"],
            "age": 21
        }"#,
        &grammar,
        ParserConfig::recovering(),
    );
    println!("Grammar:\n{}", grammar);
    let green = parser.parse_text().green;
    println!(
        "{}",
        parser.display_with_rules(green, |ix| grammar.name(ix).to_string())
    );
}

#[test]
fn test_json_error_recovery() {
    let grammar = new_grammar!(
        json where
        json    -> r!(object) | r!(array) | r!(string) | r!(number) | r!(boolean) | r!(null)
        object  -> t(token("{")) + sep(r!(pair), t(token(","))) + t(token("}"))
        pair    -> r!(string) + t(token(":")) + r!(json)
        array   -> t(token("[")) + sep(r!(json), t(token(","))) + t(token("]"))
        string  -> t(token("\"")) + t(STRING) + t("\"")
        number  -> t(token(NUMS))
        boolean -> t(token("true")) | t(token("false"))
        null    -> t(token("null"))
    );

    let mut parser = Parser::new_with_config(
        r#"{"name": "ok", "bad": true, "age": 21}"#,
        &grammar,
        ParserConfig::recovering(),
    );
    println!("Grammar:\n{}", grammar);
    let green = parser.parse_text().green;
    println!(
        "{}",
        parser.display_with_rules(green, |ix| grammar.name(ix).to_string())
    );
}

#[test]
fn test_missing_token_insertion() {
    let grammar = new_grammar!(
        stmt where
        stmt -> r!(if_stmt)
        if_stmt -> t(token("if")) + r!(cond) + r!(block)
        cond -> t(token("(")) + t(token("true")) + t(token(")"))
        block -> t(token("{")) + t(token("}"))
    );

    let code = r#"if (true { }"#; // Missing ')'

    let mut parser = Parser::new_with_config(code, &grammar, ParserConfig::recovering());
    println!("Missing Token Test");
    println!("Code: {}", code);
    let green = parser.parse_text().green;
    println!(
        "{}",
        parser.display_with_rules(green, |ix| grammar.name(ix).to_string())
    );
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
        }
    }"#;

    let mut parser = Parser::new_with_config(code, &grammar, ParserConfig::recovering());
    println!("Selection Schema - Current Structure Test");
    println!("Code: {}", code);
    let green = parser.parse_text().green;
    println!(
        "{}",
        parser.display_with_rules(green, |ix| grammar.name(ix).to_string())
    );
}
