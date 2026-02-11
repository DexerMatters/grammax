use std::thread;

use crate::{
    new_grammar,
    parsec::{
        ParserConfig,
        display::{format_ast, format_messages},
        recovery::RecoveryConfig,
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
    runtime.insert(0, "1 + 1".to_string()).unwrap();
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
                    result.current_tree,
                    &result.current_parser.alloc,
                    result.current_parser.text(),
                )
            );
            for spans in result.newly_computed_tokens {
                eprintln!(
                    "Newly computed token {}",
                    result.source_text[spans.start..spans.end].escape_debug()
                );
            }
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
    runtime
        .insert(0, r#"{"name": sDexers}"#.to_string())
        .unwrap();
    runtime.insert(16, r#", "age": 30"#.to_string()).unwrap();
    runtime.delete(16, 27).unwrap();
    runtime.insert(3, "x".to_string()).unwrap();
    println!("====Inserted 'x' at offset 3");
    runtime.insert(3, "x".to_string()).unwrap();

    thread::sleep(std::time::Duration::from_millis(10000));
}
