use std::time::Duration;

use crate::{
    interface::BasicInterface,
    new_grammar,
    parsec::words::*,
    runtime::{
        CompilerBuilder, RuntimeSelector, RuntimeService, RuntimeSignal,
        compiler::{ComposedCompiler, delete_span, insert_at, replace_span},
    },
    scheme::{layers::RedGreenTreeIR, passes::ParserPass},
    utils::Span,
};

#[test]
fn test_json() {
    let _grammar = new_grammar!(
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
        .then_layer(RedGreenTreeIR::default())
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
fn test_service_subscription_uses_same_selector_logic_as_completion() {
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

    let runtime = RuntimeService::<BasicInterface>::new(grammar, move |evt_tx| {
        let pass = CompilerBuilder::new()
            .then_pass(ParserPass::new(grammar))
            .then_layer(RedGreenTreeIR::default());
        ComposedCompiler::from_pass_with_events(pass, evt_tx)
    });

    let rx = runtime.subscribe(
        RuntimeSelector::events().with_completion(crate::runtime::CompletionPolicy::Settled),
    );

    let response = runtime.insert(0, r#"{"key": 42}"#).expect("submit");
    let signal = rx
        .recv_timeout(Duration::from_secs(1))
        .expect("subscription event");

    match signal {
        RuntimeSignal::Event { event } => {
            assert_eq!(event.revision, 1);
            assert!(event.payload.to_json().is_array());
        }
        other => panic!("expected event signal, got {other:?}"),
    }

    match response {
        RuntimeSignal::Event { event } => {
            assert_eq!(event.revision, 1);
            assert!(event.payload.to_json().is_array());
        }
        other => panic!("expected event signal, got {other:?}"),
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
        .then_layer(RedGreenTreeIR::default())
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

#[test]
fn test_server_style_delete_delete_insert_exposes_transient_empty_value() {
    let grammar = new_grammar!(
        start where
        start   -> r!(json) + tt(EndOfInput)
        json    -> r!(object) | r!(array) | r!(string) | r!(number) | r!(boolean) | r!(null)
        object  -> tt("{") + sep(r!(pair), tt(",")) + tt("}")
        pair    -> field("key", r!(string)) + tt(":") + field("value", r!(json))
        array   -> tt("[") + sep(r!(json), tt(",")) + tt("]")
        string  -> tt("\"") + t(STRING) + tt("\"")
        number  -> tt(NUMS)
        boolean -> tt("true") | tt("false")
        null    -> tt("null")
    );

    let (layer, source_obs) = CompilerBuilder::new().tap();
    let (cst_pass, cst_obs) = layer
        .then_pass(ParserPass::new(grammar))
        .then_layer(RedGreenTreeIR::default())
        .tap();

    let mut compiler = ComposedCompiler::from_pass(cst_pass);

    let initial = "{\n\"a\": 11,\n\"b\": 22\n}";
    compiler
        .submit_source(insert_at(0, initial))
        .expect("submit initial");
    let (_source_rev1, source_txn1) = source_obs
        .updates
        .recv_timeout(Duration::from_millis(500))
        .expect("timed out waiting for initial source txn");
    let (_cst_rev1, cst_txn1) = cst_obs
        .updates
        .recv_timeout(Duration::from_millis(500))
        .expect("timed out waiting for initial cst txn");

    compiler
        .submit_source(delete_span(Span::new(8, 9)))
        .expect("delete second digit");
    let (_source_rev2, source_txn2) = source_obs
        .updates
        .recv_timeout(Duration::from_millis(500))
        .expect("timed out waiting for first delete source txn");
    let (_cst_rev2, cst_txn2) = cst_obs
        .updates
        .recv_timeout(Duration::from_millis(500))
        .expect("timed out waiting for first delete cst txn");

    compiler
        .submit_source(delete_span(Span::new(7, 8)))
        .expect("delete first digit");
    let (_source_rev3, source_txn3) = source_obs
        .updates
        .recv_timeout(Duration::from_millis(500))
        .expect("timed out waiting for second delete source txn");
    let (_cst_rev3, cst_txn3) = cst_obs
        .updates
        .recv_timeout(Duration::from_millis(500))
        .expect("timed out waiting for second delete cst txn");

    compiler
        .submit_source(insert_at(7, "x"))
        .expect("insert replacement char");
    let (_source_rev4, source_txn4) = source_obs
        .updates
        .recv_timeout(Duration::from_millis(500))
        .expect("timed out waiting for insert source txn");
    let (_cst_rev4, cst_txn4) = cst_obs
        .updates
        .recv_timeout(Duration::from_millis(500))
        .expect("timed out waiting for insert cst txn");

    let source1: Vec<String> = source_txn1.iter().map(|cmd| format!("{cmd:?}")).collect();
    let cst1: Vec<String> = cst_txn1.iter().map(|cmd| format!("{cmd:?}")).collect();
    let source2: Vec<String> = source_txn2.iter().map(|cmd| format!("{cmd:?}")).collect();
    let cst2: Vec<String> = cst_txn2.iter().map(|cmd| format!("{cmd:?}")).collect();
    let source3: Vec<String> = source_txn3.iter().map(|cmd| format!("{cmd:?}")).collect();
    let cst3: Vec<String> = cst_txn3.iter().map(|cmd| format!("{cmd:?}")).collect();
    let source4: Vec<String> = source_txn4.iter().map(|cmd| format!("{cmd:?}")).collect();
    let cst4: Vec<String> = cst_txn4.iter().map(|cmd| format!("{cmd:?}")).collect();

    println!("=== initial source ===\n{}", source1.join("\n"));
    println!("=== initial cst ===\n{}", cst1.join("\n"));
    println!("=== delete second digit source ===\n{}", source2.join("\n"));
    println!("=== delete second digit cst ===\n{}", cst2.join("\n"));
    println!("=== delete first digit source ===\n{}", source3.join("\n"));
    println!("=== delete first digit cst ===\n{}", cst3.join("\n"));
    println!("=== insert x source ===\n{}", source4.join("\n"));
    println!("=== insert x cst ===\n{}", cst4.join("\n"));

    assert!(
        cst2.iter()
            .any(|cmd| cmd.contains("Token") && cmd.contains("text: \" 1\"")),
        "First delete should stay local to the number token: {:?}",
        cst2
    );

    let leaked_after_second_delete = [
        "text: \",\"",
        "text: \"\\n\\\"\"",
        "text: \"b\"",
        "text: \"\\\"\"",
        "text: \":\"",
        "text: \" 22\"",
    ];
    for leaked in leaked_after_second_delete {
        assert!(
            cst3.iter()
                .any(|cmd| cmd.contains("UnexpectedToken") && cmd.contains(leaked)),
            "Second delete should reproduce the server spill into the next pair ({leaked}): {:?}",
            cst3
        );
    }

    assert!(
        cst4.iter()
            .any(|cmd| cmd.contains("UnexpectedToken") && cmd.contains("text: \" x\"")),
        "Final insert should only replace the already-corrupted local error node: {:?}",
        cst4
    );
}
