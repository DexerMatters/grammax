use std::time::Duration;

use crate::{
    interface::BasicInterface,
    new_grammar, new_grammar_no_cache,
    parsec::Parser,
    runtime::Build,
    scheme::{
        layers::{AstArena, ParseTreeIR},
        passes::{IncrementalLowerer, ParserPass, reparser::Reparser},
    },
};

#[test]
fn test_arith_reparser() {
    let grammar = new_grammar_no_cache!(
        start where
        start -> r!(expr) + tt(EndOfInput)
        expr -> r!(add) | r!(mul) | r!(primary)
        add  -> r!(expr) + tt("+") + r!(expr).drop(1)
        mul  -> r!(expr).drop(1) + tt("*") + r!(expr).drop(2)
        primary -> tt(NUMBER) | tt("(") + r!(expr) + tt(")")
    );

    let parser = Parser::new(grammar);
    let mut reparser = Reparser::from_parser(parser);
    reparser.insert(0, "1 + 2 * 3").unwrap();
    println!("{}", reparser.current_view());

    reparser.delete(0, 3).unwrap();
    println!("{}", reparser.current_view());
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
        number  -> t(NUMBER)
        boolean -> tt("true") | tt("false")
        null    -> tt("null")
    );

    let compiler = Build::new().then(
        ParserPass::new(grammar),
        ParseTreeIR::with_grammar(grammar),
        |b, _cst_obs| {
            b.then(
                IncrementalLowerer::new(grammar, ()),
                AstArena::default(),
                |b, _ast_obs| b.build_runtime::<BasicInterface<_>>(grammar),
            )
        },
    );

    compiler.insert(0, r#"{"name": "John"}"#).expect("submit");
    compiler.replace(10, 14, "Doe").expect("update");
}

#[test]
fn test_tap_prints_cst_commands() {
    let grammar = new_grammar!(
        json where
        json    -> r!(object) | r!(array) | r!(string) | r!(number) | r!(boolean) | r!(null)
        object  -> tt("{") + sep(r!(pair), tt(",")) + tt("}")
        pair    -> field("key", r!(string)) + tt(":") + field("value", r!(json))
        array   -> tt("[") + sep(r!(json), tt(",")) + tt("]")
        string  -> tt("\"") + t(STRING) + tt("\"")
        number  -> tt(NUMBER)
        boolean -> tt("true") | tt("false")
        null    -> tt("null")
    );

    let (compiler, cst_obs) = Build::new().then(
        ParserPass::new(grammar),
        ParseTreeIR::with_grammar(grammar),
        |b, obs| (b.build_runtime::<BasicInterface<_>>(grammar), obs),
    );

    compiler.insert(0, r#"{"name": "John"}"#).expect("submit");

    // The observer receives one update per submitted transaction.
    let (revision, txn) = cst_obs
        .updates
        .recv_timeout(Duration::from_millis(500))
        .expect("observer timed out waiting for update");

    println!("=== revision {revision} — {} CST command(s) ===", txn.len());
    for cmd in txn.iter() {
        println!("  {cmd:?}");
    }
}
