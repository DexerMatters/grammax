#[cfg(feature = "webui")]
use crate::{
    interface::BasicInterface,
    new_grammar,
    parsec::{tree::GreenId, words::*},
    runtime::{CompilerBuilder, ComposedCompiler, ParserPass},
    scheme::layers::{NodePath, ParseTreeIR, ParseTreeQuery},
};

#[cfg(feature = "webui")]
use crate::{interface::webui::WebPreviewInterface, runtime::RuntimeService};

#[cfg(feature = "webui")]
use std::thread;

#[cfg(feature = "webui")]
#[test]
fn test_tap_prints_cst_commands() {
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

    let (layer, source_observer) = CompilerBuilder::new().tap();
    thread::spawn(move || {
        while let Some(transaction) = source_observer.recv() {
            println!("======Received Source transaction:");
            for cmd in transaction.iter() {
                println!("Source Command: {:?}", cmd);
            }
        }
    });
    let (pass, observer) = layer
        .then_pass(ParserPass::new(grammar))
        .then_layer(ParseTreeIR::default())
        .tap();

    thread::spawn(move || {
        while let Some(transaction) = observer.recv() {
            println!("======Received CST transaction:");
            for cmd in transaction.iter() {
                println!("CST Command: {:?}", cmd);
            }
        }
    });

    let runtime = RuntimeService::<WebPreviewInterface>::new(grammar, move |evt_tx| {
        ComposedCompiler::from_pass_with_events(pass, evt_tx)
    });

    runtime.run().expect("runtime failed");
}

#[cfg(feature = "webui")]
#[test]
fn test_arith_commands() {
    let grammar = new_grammar!(
        start where
        start -> r!(expr) + tt(EndOfInput)
        expr -> r!(add) | r!(mul) | r!(primary)
        add  -> field("lhs:", r!(expr)) + tt("+") + field("rhs:", r!(expr).drop(1))
        mul  -> field("lhs:", r!(expr).drop(1)) + tt("*") + field("rhs:", r!(expr).drop(2))
        primary -> tt(NUMS) | tt("(") + r!(expr) + tt(")")
    );

    let (pass, observer) = CompilerBuilder::new()
        .then_pass(ParserPass::new(grammar))
        .then_layer(ParseTreeIR::default())
        .tap();

    thread::spawn(move || {
        while let Some(transaction) = observer.recv() {
            println!("======Current parse tree:");
            let result = observer.query(ParseTreeQuery::Path(NodePath::root()));
            let result = result.expect("Runtime query failed");
            let tree = result.expect("Query is bad for this layer");
            let green_id = tree
                .downcast_ref::<GreenId>()
                .expect("Value is not a GreenId");
            println!("GreenId at root: {:?}", green_id);

            println!("======Received transaction:");
            for cmd in transaction.iter() {
                println!("{:?}", cmd);
            }
        }
    });

    let runtime = RuntimeService::<BasicInterface>::new(grammar, move |evt_tx| {
        ComposedCompiler::from_pass_with_events(pass, evt_tx)
    });

    runtime.insert(0, "1 + 2 * 3").unwrap();
    runtime.replace(0, 1, "4").unwrap();
}
