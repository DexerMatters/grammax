use std::time::Duration;

use crate::{
    interface::BasicInterface,
    new_grammar,
    parsec::words::*,
    runtime::{
        BuildTree, CompilerBuilder, Down, End, Here, Observe, ObservedLayer, Then,
        compiler::insert_at,
    },
    scheme::{
        layers::{AstArena, ParseTreeIR, SourceText},
        passes::{IncrementalLowerer, ParserPass},
    },
};

type CstTree = Then<SourceText, ParserPass, End<ParseTreeIR>>;

#[test]
fn test_json() {
    let grammar = new_grammar!(
        json where
        json    -> r!(object) | r!(array) | r!(string) | r!(number) | r!(boolean) | r!(null)
        object  -> tt("{") + sep(r!(pair), tt(",")) + tt("}")
        pair    -> field("key", r!(string)) + tt(":") + field("value", r!(json))
        array   -> tt("[") + sep(r!(json), tt(",")) + tt("]")
        string  -> tt("\"") + t(STRING) + t("\"")
        number  -> t(NUMS)
        boolean -> tt("true") | tt("false")
        null    -> tt("null")
    );

    let compiler = CompilerBuilder::new()
        .then(ParserPass::new(grammar), ParseTreeIR::default())
        .then(IncrementalLowerer::new(grammar, ()), AstArena::default());

    let compiler = compiler.build_runtime::<BasicInterface<_>>(grammar);

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
        number  -> tt(NUMS)
        boolean -> tt("true") | tt("false")
        null    -> tt("null")
    );

    let cst_tree: CstTree =
        CompilerBuilder::new().then(ParserPass::new(grammar), ParseTreeIR::default());
    let cst_obs: ObservedLayer<CstTree, Down<Here>> = cst_tree.observe::<Down<Here>>();

    let mut compiler = cst_tree.build();

    // Submit a small JSON document.
    compiler
        .submit_source(insert_at(0, r#"{"key": 42}"#))
        .expect("submit");

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
