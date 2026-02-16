use std::thread;
use std::{fmt::Display, sync::Arc};

use parking_lot::lock_api::Mutex;

use crate::{
    new_grammar,
    parsec::{
        ParserConfig,
        display::{format_ast, format_messages},
        recovery::RecoveryConfig,
        tree::RedNode,
        words::*,
    },
    runtime::{Interactive, RuntimeConfig, RuntimeListener},
};

#[test]
fn test_expr_example() {
    let grammar = new_grammar!(
        start where
        start -> r!(expr) + tt(EndOfInput)
        expr -> r!(add) | r!(mul) | r!(primary)
        add  -> r!(primary) + tt("+") + r!(expr)
        mul  -> r!(primary) + tt("*") + r!(expr)
        primary -> tt(NUMS) | tt("(") + r!(expr) + tt(")")
    );
    println!("===== Grammar =====");
    println!("{}", grammar.table);

    let listener = RuntimeListener::new()
        .before_update(|| {
            eprintln!("Update started...");
        })
        .after_update(|result, duration| {
            eprintln!("Updated source text:\n{}", result.source_text);
            // eprintln!(
            //     "Updated parse tree:\n{}",
            //     result.current_tree.display(&result.current_parser)
            // );
            eprintln!(
                "Reparsed tree:\n{}",
                format_ast(
                    &result.current_parser.grammar,
                    result.reparsed_tree,
                    &result.current_parser.alloc,
                    result.current_parser.text(),
                )
            );
            eprintln!("Offset: {}", result.reparsed_tree.offset);
            eprintln!("Messages:");
            eprintln!(
                "{}",
                format_messages(&result.current_parser.grammar, &result.messages)
            );
            eprintln!("Update took: {:?}", duration);
        });
    let runtime = Interactive::new(grammar)
        .with_listener(listener)
        .with_config(RuntimeConfig {
            parser: ParserConfig {
                simple_ast: false,
                recovery: RecoveryConfig::default(),
            },
            ..RuntimeConfig::default()
        })
        .finish();
    runtime.run().unwrap();
    runtime.insert(0, "1 + 1").unwrap();
    thread::sleep(std::time::Duration::from_millis(100));
}

#[test]
fn test_example() {
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
    let listener = RuntimeListener::new()
        .before_update(|| {
            eprintln!("=== Update started ===");
        })
        .after_update(|result, duration| {
            eprintln!("Updated source text:\n{}", result.source_text);
            // eprintln!(
            //     "Updated parse tree:\n{}",
            //     result.current_tree.display(&result.current_parser)
            // );
            eprintln!(
                "Reparsed parse tree:\n{}",
                format_ast(
                    &result.current_parser.grammar,
                    result.reparsed_tree,
                    &result.current_parser.alloc,
                    result.current_parser.text(),
                )
            );

            eprintln!("Messages:");
            eprintln!(
                "{}",
                format_messages(&result.current_parser.grammar, &result.messages)
            );
            eprintln!("Update took: {:?}", duration);
        });

    let runtime = Interactive::new(grammar)
        .with_listener(listener)
        .with_parser_config(ParserConfig {
            simple_ast: true,
            recovery: RecoveryConfig::default(),
        })
        .finish();
    runtime.run().unwrap();
    runtime.insert(0, r#"{"name": dDexerd}"#).unwrap();
    runtime.insert(16, r#", "age": 30"#).unwrap();
    runtime.delete(16, 27).unwrap();
    runtime.insert(3, "x").unwrap();
    println!("====Inserted 'x' at offset 3");
    runtime.insert(3, "x").unwrap();

    thread::sleep(std::time::Duration::from_millis(10000));
}

#[derive(Debug, Clone, PartialEq)]
enum Expr {
    Add(Box<Expr>, Box<Expr>),
    Mul(Box<Expr>, Box<Expr>),
    Num(i64),
    Error,
}

fn eval(expr: &Expr) -> Expr {
    match expr {
        Expr::Add(l, r) => match (eval(l), eval(r)) {
            (Expr::Num(lv), Expr::Num(rv)) => Expr::Num(lv + rv),
            (x, y) => Expr::Add(Box::new(x), Box::new(y)),
        },
        Expr::Mul(l, r) => match (eval(l), eval(r)) {
            (Expr::Num(lv), Expr::Num(rv)) => Expr::Num(lv * rv),
            (x, y) => Expr::Mul(Box::new(x), Box::new(y)),
        },
        Expr::Num(n) => Expr::Num(*n),
        Expr::Error => Expr::Error,
    }
}

impl Display for Expr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Expr::Add(l, r) => write!(f, "({} + {})", l, r),
            Expr::Mul(l, r) => write!(f, "({} * {})", l, r),
            Expr::Num(n) => write!(f, "{}", n),
            Expr::Error => write!(f, "<error>"),
        }
    }
}

#[test]
fn test_semantic_commands() {
    let grammar = new_grammar!(
        start where
        start -> r!(expr) + tt(EndOfInput)
        expr -> r!(add) | r!(mul) | r!(primary)
        add  -> r!(expr) + tt("+") + r!(expr).drop(1)
        mul  -> r!(expr).drop(1) + tt("*") + r!(expr).drop(2)
        primary -> tt(NUMS) | tt("(") + r!(expr) + tt(")")
    );

    let listener = RuntimeListener::new().after_update(move |result, duration| {
        println!("=== After Update ===");
        println!("Duration (wall): {:?}", duration);
        println!("{}", result.metrics.summary());
        println!("Source Text:\n{}", result.source_text);

        for cmd in &result.semantic_commands {
            match cmd {
                crate::semantic::Command::Create(id, name) => {
                    eprintln!("Applied command: Create({}, \"{}\")", id, name);
                }
                crate::semantic::Command::CreateToken(id, val) => {
                    eprintln!("Applied command: CreateToken({}, \"{}\")", id, val);
                }
                crate::semantic::Command::Replace(old_id, new_id) => {
                    eprintln!("Applied command: Replace({}, {})", old_id, new_id);
                }
                crate::semantic::Command::Delete(id) => {
                    eprintln!("Applied command: Delete({})", id);
                }
                crate::semantic::Command::Insert(parent_id, child_id) => {
                    eprintln!("Applied command: Insert({}, {})", parent_id, child_id);
                }
            }
        }
    });

    let runtime = Interactive::new(grammar)
        .with_listener(listener)
        .with_parser_config(ParserConfig {
            simple_ast: true,
            recovery: RecoveryConfig::default(),
        })
        .finish();

    runtime.run().unwrap();
    runtime.insert(0, "1 + 4 * 4 + (5 + 5) * 2").unwrap();
    runtime.update(0, 1, "3 * 5").unwrap();
    runtime.insert(0, "2 + 2 + ").unwrap();
    runtime.delete(0, 4).unwrap();
    thread::sleep(std::time::Duration::from_millis(100));
}
