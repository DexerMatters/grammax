use crate::{
    new_grammar,
    parsec::{
        parser::Parser,
        words::{EndOfInput, NUMS, STRING, token},
    },
};

#[test]
fn test_parser() {
    let grammar = new_grammar!(
        expr where
        expr   -> r!(expr) + t("+") + r!(num) | r!(expr) + t("-") + r!(num) | r!(num)
        num    -> t("x")
    );

    println!("Grammar:\n{}", grammar);
    let mut parser = Parser::new("x+x-x+x", &grammar);
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
    let mut parser2 = Parser::new("aaaaaad", &grammar2);
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
    let mut parser = Parser::new("(12,33,44)", &grammar);
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

    let mut parser = Parser::new(
        r#"{
            "name": "John Doe",
            "is_student": false,
            "courses": ["Math", "Science", "Art"],
            "age": 21
        }"#,
        &grammar,
    );
    println!("Grammar:\n{}", grammar);
    let green = parser.parse_text().green;
    println!(
        "{}",
        parser.display_with_rules(green, |ix| grammar.name(ix).to_string())
    );
}
