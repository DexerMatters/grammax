use std::time::Duration;

use crate::{
    interface::BasicInterface,
    new_grammar, new_grammar_no_cache,
    parsec::Parser,
    runtime::{BuildTree, CompilerBuilder, Down, End, Here, Observe, ObservedLayer, Then},
    scheme::{
        layers::{AstArena, ParseTreeIR, SourceText},
        passes::{IncrementalLowerer, ParserPass, reparser::Reparser},
    },
    utils::Position,
};

type CstTree = Then<SourceText, ParserPass, End<ParseTreeIR>>;

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
    reparser.insert((0, 0), "1 + 2 * 3").unwrap();
    println!("{}", reparser.current_view());

    reparser.delete(((0, 0), (0, 3))).unwrap();
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

    let compiler = CompilerBuilder::new()
        .then(ParserPass::new(grammar), ParseTreeIR::default())
        .then(IncrementalLowerer::new(grammar, ()), AstArena::default());

    let compiler = compiler.build_runtime::<BasicInterface<_>>(grammar);

    compiler
        .insert(Position::zero(), r#"{"name": "John"}"#)
        .expect("submit");
    compiler.replace(((0, 10), (0, 14)), "Doe").expect("update");
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

    let cst_tree: CstTree =
        CompilerBuilder::new().then(ParserPass::new(grammar), ParseTreeIR::default());
    let cst_obs: ObservedLayer<CstTree, Down<Here>> = cst_tree.observe::<Down<Here>>();

    let compiler = cst_tree.build_runtime::<BasicInterface<_>>(grammar);
    compiler
        .insert((0, 0), r#"{"name": "John"}"#)
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
