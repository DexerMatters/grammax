use std::time::Duration;

use crate::{
    interface::BasicInterface,
    new_grammar,
    parsec::words::*,
    runtime::{
        CompilerBuilder, RuntimeSelector, RuntimeService, RuntimeSignal,
        compiler::{ComposedCompiler, insert_at},
    },
    scheme::{layers::RedGreenTreeIR, passes::ParserPass},
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
            assert!(event.payload.is_array());
        }
        other => panic!("expected event signal, got {other:?}"),
    }

    match response {
        RuntimeSignal::Event { event } => {
            assert_eq!(event.revision, 1);
            assert!(event.payload.is_array());
        }
        other => panic!("expected event signal, got {other:?}"),
    }
}
