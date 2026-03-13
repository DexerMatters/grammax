#[cfg(feature = "webui")]
use crate::{
    interface::BasicInterface,
    new_grammar,
    parsec::words::*,
    runtime::{BuildTree, CompilerBuilder, Down, Here, Observe, ParserPass},
    scheme::layers::{NodePath, ParseTreeIR, ParseTreeQuery, ParseTreeValue},
};

#[cfg(feature = "webui")]
use crate::interface::webui::WebPreviewInterface;

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

    let layer = CompilerBuilder::new();
    let source_observer = layer.observe::<Here>();
    thread::spawn(move || {
        while let Some(transaction) = source_observer.recv() {
            println!("======Received Source transaction:");
            for cmd in transaction.iter() {
                println!("Source Command: {:?}", cmd);
            }
        }
    });
    let pass = layer.then(ParserPass::new(grammar), ParseTreeIR::default());
    let observer = pass.observe::<Down<Here>>();

    thread::spawn(move || {
        while let Some(transaction) = observer.recv() {
            println!("======Received CST transaction:");
            for cmd in transaction.iter() {
                println!("CST Command: {:?}", cmd);
            }
        }
    });

    let runtime = pass.build_runtime::<WebPreviewInterface<_>>(grammar);

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

    let pass = CompilerBuilder::new().then(ParserPass::new(grammar), ParseTreeIR::default());
    let observer = pass.observe::<Down<Here>>();

    thread::spawn(move || {
        while let Some(transaction) = observer.recv() {
            println!("======Current parse tree:");
            let result = observer.query(ParseTreeQuery::Path(NodePath::root()));
            let tree = result.expect("Runtime query failed");
            let green_id = match &tree {
                ParseTreeValue::GreenId(id) => id,
                other => panic!("expected GreenId, got {other:?}"),
            };
            println!("GreenId at root: {:?}", green_id);

            println!("======Received transaction:");
            for cmd in transaction.iter() {
                println!("{:?}", cmd);
            }
        }
    });

    let runtime = pass.build_runtime::<BasicInterface<_>>(grammar);

    runtime.insert(0, "1 + 2 * 3").unwrap();
    runtime.replace(0, 1, "4").unwrap();
    thread::sleep(std::time::Duration::from_millis(100));
}
