use std::{thread, time::Duration};

use crate::{
    interface::BasicInterface,
    new_grammar,
    parsec::words::*,
    runtime::{
        CompilerBuilder, RuntimeService, RuntimeSignal,
        compiler::{ComposedCompiler, delete_span, insert_at, replace_span},
    },
    scheme::{layers::ParseTreeIR, passes::ParserPass},
    utils::Span,
};

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

    let (cst_pass, cst_obs) = CompilerBuilder::new()
        .then_pass(ParserPass::new(grammar))
        .then_layer(ParseTreeIR::default())
        .tap();

    // thread::spawn(move || {
    //     for update in cst_obs.updates.iter() {
    //         println!(
    //             "=== CST update: revision {}, {} commands ===",
    //             update.0,
    //             update.1.len()
    //         );
    //         for cmd in update.1.iter() {
    //             println!("  {cmd:?}");
    //         }
    //     }
    // });

    let compiler = RuntimeService::<BasicInterface>::new(grammar, move |evt_tx| {
        ComposedCompiler::from_pass_with_events(cst_pass, evt_tx)
    });

    compiler.insert(0, r#"{"name": "John"}"#).expect("submit");
    compiler.update(10, 14, "Doe").expect("update");
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

    let (cst_pass, cst_obs) = CompilerBuilder::new()
        .then_pass(ParserPass::new(grammar))
        .then_layer(ParseTreeIR::default())
        .tap();

    let mut compiler = ComposedCompiler::from_pass(cst_pass);

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

#[test]
fn test_incremental_parse_narrow_error_region() {
    // Regression test: replacing a valid number with an invalid identifier
    // should produce a narrow error (only the invalid token), not cascading
    // errors that delete valid siblings from the tree.
    //
    // ["a", 1, 1]  →  ["a", x, 1]
    // Position of first '1' in the array: ["a", 1, 1]
    //  0123456789...
    // [ " a " ,   1 ,   1 ]
    // 0 1 2 3 4 5 6 7 8 9 10
    //
    // Replacing '1' at pos 6 with 'x' (same length → delta 0).

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

    let (cst_pass, cst_obs) = CompilerBuilder::new()
        .then_pass(ParserPass::new(grammar))
        .then_layer(ParseTreeIR::default())
        .tap();

    let mut compiler = ComposedCompiler::from_pass(cst_pass);

    // Step 1: submit the initial valid document.
    let initial = r#"["a", 1, 1]"#;
    compiler
        .submit_source(insert_at(0, initial))
        .expect("submit initial");
    let (_rev1, txn1) = cst_obs
        .updates
        .recv_timeout(Duration::from_millis(500))
        .expect("timed out waiting for initial parse");
    println!("=== Initial document: {} commands ===", txn1.len());
    for cmd in txn1.iter() {
        println!("  {cmd:?}");
    }

    // Step 2: replace '1' at position 6 with 'x'.
    // "["a", 1, 1]" → "["a", x, 1]"
    compiler
        .submit_source(replace_span(Span::new(6, 7), "x"))
        .expect("submit edit");
    let (_rev2, txn2) = cst_obs
        .updates
        .recv_timeout(Duration::from_millis(500))
        .expect("timed out waiting for incremental parse");
    println!("=== After edit (1→x at pos 6): {} commands ===", txn2.len());
    for cmd in txn2.iter() {
        println!("  {cmd:?}");
    }

    // The incremental update must NOT include any MissingToken commands —
    // all delimiters (',' and ']') are still present in the source.
    let has_missing_token = txn2
        .iter()
        .any(|cmd| format!("{cmd:?}").contains("MissingToken"));
    assert!(
        !has_missing_token,
        "Incremental parse produced MissingToken nodes for valid delimiters!\nCommands: {:?}",
        txn2.as_slice()
    );
}
