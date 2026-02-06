use std::{
    sync::{
        Arc,
        atomic::{self, AtomicUsize},
    },
    thread,
};

use crate::{
    new_grammar,
    parsec::{ParserConfig, ParserListener, fmt::Display, words::*},
    runtime::{Interactive, RuntimeListener},
};

#[test]
fn test_expr_example() {
    let grammar = new_grammar!(
        start where
        start -> many(r!(stmt) + t(token(";"))) + t(token(EndOfInput))
        stmt  -> r!(assignment) | r!(if_stmt)
        assignment -> t(token("x")) + t(token("=")) + t(token(NUMS))
        if_stmt -> t(token("if")) + r!(cond) + r!(block)
        cond -> t(token("(")) + r!(boolean) + t(token(")"))
        boolean -> t(token("true")) | t(token("false"))
        block -> t(token("{")) + many(r!(stmt) + t(token(";"))) + t(token("}"))
    );
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
                result.reparsed_tree.display(&result.current_parser)
            );
            eprintln!("Offset: {}", result.reparsed_tree.offset);
            eprintln!("Messages:");
            eprintln!("{}", result.messages.display(&result.current_parser));
            eprintln!("Update took: {:?}", duration);
        });
    let runtime = Interactive::new(grammar)
        .with_listener(listener)
        .with_parser_config(ParserConfig::new().with_simple_ast(false))
        .finish();
    runtime.run().unwrap();
    runtime.insert(0, "x = 12;".to_string()).unwrap();
    runtime
        .insert(7, "if (true) { x = 34; x = 56; x = 44; };".to_string())
        .unwrap();
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
                "Reparsed tree:\n{}",
                result.reparsed_tree.display(&result.current_parser)
            );
            eprintln!("Offset: {}", result.reparsed_tree.offset);
            eprintln!("Messages:");
            eprintln!("{}", result.messages.display(&result.current_parser));
            for node in &result.newly_computed_tokens {
                eprintln!(
                    "Newly computed tokens: {:?} - {:?}",
                    node,
                    result.current_parser.text[node.start..node.end].to_string()
                );
            }
            eprintln!("Update took: {:?}", duration);
        });

    let runtime = Interactive::new(grammar)
        .with_listener(listener)
        .with_parser_config(ParserConfig::new().with_simple_ast(true))
        .finish();
    runtime.run().unwrap();
    runtime
        .insert(0, r#"{"name": "Dexer"}"#.to_string())
        .unwrap();
    runtime.insert(16, r#", "age": 30"#.to_string()).unwrap();
    runtime.delete(16, 27).unwrap();
    runtime.insert(3, "x".to_string()).unwrap();

    thread::sleep(std::time::Duration::from_millis(100));
}
