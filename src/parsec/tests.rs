use crate::new_grammar_no_cache;
use crate::parsec::ParserConfig;
use crate::parsec::msg::ErrorMessage;
use crate::parsec::parser::Parser;
use crate::parsec::view::ViewAction;
use crate::scheme::layers::AstCell;

#[test]
fn test_simple_whitespaces() {
    let grammar = new_grammar_no_cache!(
      lilies where lilies -> r!(exprs)
      exprs -> r!(strings)
      strings -> r!(normal_string)
      normal_string -> tt('"') + t(STRING) + t('"')
    );

    let mut parser = Parser::new(grammar);

    println!("Grammar:\n{}", grammar.table);

    let text = r#""""#;
    let result = parser.parse_text(text);

    let output = result.format_ast();
    println!("AST {}:\n{}", text, output);
    println!("Messages:\n{}", result.format_messages());

    assert!(
        result.messages.is_empty(),
        "expected successful parse, got: {}",
        result.format_messages()
    );
}

#[test]
fn test_simple_arithmetic_precedence() {
    let grammar = new_grammar_no_cache!(
        start where
        start -> r!(expr) + tt(EndOfInput)
        expr -> r!(add) | r!(mul) | r!(primary)
        add  -> r!(expr) + tt("+") + r!(expr).drop(1)
        mul  -> r!(expr).drop(1) + tt("*") + r!(expr).drop(2)
        primary -> tt(NUMBER) | tt("(") + r!(expr) + tt(")")
    );

    #[derive(PartialEq, Eq, Debug)]
    enum Expr {
        Number(u32),
        Add(Box<Expr>, Box<Expr>),
        Mul(Box<Expr>, Box<Expr>),
        Error,
    }

    let text = "1 + 2 * (3 + 4)";
    let result = grammar.parse(text);
    let view = result.view();
    let viewer = result
        .viewer()
        .on_error(|_, _| ViewAction::Exact(Expr::Error))
        .on_rule("expr", |_, _| ViewAction::<Expr>::Relay)
        .on_rule("add", |ctx, view| {
            let lhs = view.first().view(ctx);
            let rhs = view.last().view(ctx);
            ViewAction::Exact(Expr::Add(Box::new(lhs), Box::new(rhs)))
        })
        .on_rule("mul", |ctx, view| {
            let lhs = view.first().view(ctx);
            let rhs = view.last().view(ctx);
            ViewAction::Exact(Expr::Mul(Box::new(lhs), Box::new(rhs)))
        })
        .on_rule("primary", |ctx, view| {
            if let Some(expr_view) = view.try_nth(1) {
                return ViewAction::Exact(expr_view.view(ctx));
            }
            let number = view.first().text_trimmed().parse::<u32>().unwrap();
            ViewAction::Exact(Expr::Number(number))
        });

    let ast: Expr = view.view(&viewer);
    assert_eq!(
        ast,
        Expr::Add(
            Box::new(Expr::Number(1)),
            Box::new(Expr::Mul(
                Box::new(Expr::Number(2)),
                Box::new(Expr::Add(
                    Box::new(Expr::Number(3)),
                    Box::new(Expr::Number(4))
                ))
            ))
        )
    );
}

#[test]
fn test_json() {
    let grammar = new_grammar_no_cache!(
        start where
        start   -> r!(json) + tt(EndOfInput)
        json    -> r!(object) | r!(array) | r!(string) | r!(number) | r!(boolean) | r!(null)
        object  -> tt("{") + sep(r!(pair), tt(",")) + tt("}")
        pair    -> field("key", r!(string)) + tt(":") + field("value", r!(json))
        array   -> tt("[") + sep(r!(json), tt(",")) + tt("]")
        string  -> tt("\"") + t(STRING) + t("\"")
        number  -> tt(NUMBER)
        boolean -> tt("true") | tt("false")
        null    -> tt("null")
    );

    let mut parser = Parser::new(grammar);

    println!("Grammar:\n{}", grammar.table);

    let text = r#""""#;
    let result = parser.parse_text(text);

    println!("AST:\n{}", result.format_ast());
    println!("Messages:\n{}", result.format_messages());
}

#[test]
fn test_recovery_strategy_is_wired_for_current_text() {
    let my_grammar = new_grammar_no_cache! {
        // The rule to start parsing from
        start where
        // FORMAT: rule_name -> rule_body
        start -> r!(expr) + t(EndOfInput)
        expr -> r!(add) | r!(sub) | r!(mul) | r!(div) | r!(num)
        add -> r!(expr) + t('+') + r!(expr).drop(2)
        sub -> r!(expr) + t('-') + r!(expr).drop(2)
        mul -> r!(expr).drop(2) + t('*') + r!(expr).drop(4)
        div -> r!(expr).drop(2) + t('/') + r!(expr).drop(4)
        num -> t(NUMBER) | t('(') + r!(expr) + t(')')
    };

    let result = my_grammar.parse("1+12*4/5-2+5-4+5");
    let output = result.format_ast();

    println!("{}", output);
    println!("Messages:\n{}", result.format_messages());
}

#[test]
fn test_delimited_content_with_custom_delimiter() {
    let grammar = new_grammar_no_cache!(
        start where
        start -> r!(single_quoted) + tt(EndOfInput)
        single_quoted -> tt("'")
            + t(crate::parsec::words::RegexMatcher::new(r#"[^']*"#))
            + tt("'")
    );

    let mut parser = Parser::new(grammar);
    let result = parser.parse_text("'abc'");

    assert!(
        result.messages.is_empty(),
        "expected successful parse, got: {}",
        result.format_messages()
    );
}

#[test]
fn test_custom_delimiter_recovery_inserts_missing_close() {
    let grammar = new_grammar_no_cache!(
        start where
        start -> r!(single_quoted) + tt(EndOfInput)
        single_quoted -> tt("'")
            + t(crate::parsec::words::RegexMatcher::new(r#"[^']*"#))
            + tt("'")
    );

    let mut parser = Parser::new(grammar);
    let result = parser.parse_text("'abc");

    assert!(
        result
            .messages
            .iter()
            .any(|m| matches!(m.message, ErrorMessage::MissingToken { .. })),
        "expected missing-token recovery, got: {}",
        result.format_messages()
    );
}
