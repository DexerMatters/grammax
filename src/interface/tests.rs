#[cfg(feature = "webui")]
use color_print::cprintln;

#[cfg(feature = "webui")]
use crate::{
    interface::BasicInterface,
    new_grammar,
    parsec::{view::NodeView, words::*},
    runtime::{BuildTree, CompilerBuilder, Down, Here, Observe, ParserPass},
    scheme::{
        layers::{AstArena, AstCell, NodePath, ParseTreeIR},
        passes::{AstMapper, IncrementalLowerer},
    },
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
    #[derive(Debug, Clone, PartialEq)]
    enum Expr {
        Num(usize),
        Add(AstCell<Expr>, AstCell<Expr>),
        Mul(AstCell<Expr>, AstCell<Expr>),
        Error,
    }

    let mapper = AstMapper::new()
        .skip_rule("start")
        .on_error(|ctx, _node| Some(ctx.emit(Expr::Error)))
        .on_rule("expr", |ctx, node| ctx.forward_first_child(node))
        .on_rule("primary", |ctx, node| {
            if let Some(expr_node) = node.try_first_with_rule("expr") {
                return Some(ctx.forward(expr_node));
            }
            let token = node.each().iter().find(|c| c.token_name().is_some())?;
            let num: usize = ctx.read_text(token).parse().unwrap_or(0);
            Some(ctx.emit(Expr::Num(num)))
        })
        .on_field("lhs:", |ctx, node| ctx.forward_first_child(node))
        .on_field("rhs:", |ctx, node| ctx.forward_first_child(node))
        .on_rule("add", |ctx, node| {
            let lhs = ctx.read_cell(node.try_first_with_field("lhs:")?)?;
            let rhs = ctx.read_cell(node.try_first_with_field("rhs:")?)?;
            Some(ctx.emit(Expr::Add(lhs, rhs)))
        })
        .on_rule("mul", |ctx, node| {
            let lhs = ctx.read_cell(node.try_first_with_field("lhs:")?)?;
            let rhs = ctx.read_cell(node.try_first_with_field("rhs:")?)?;
            Some(ctx.emit(Expr::Mul(lhs, rhs)))
        });

    let grammar = new_grammar!(
        start where
        start -> r!(expr) + tt(EndOfInput)
        expr -> r!(add) | r!(mul) | r!(primary)
        add  -> field("lhs:", r!(expr)) + tt("+") + field("rhs:", r!(expr).drop(1))
        mul  -> field("lhs:", r!(expr).drop(1)) + tt("*") + field("rhs:", r!(expr).drop(2))
        primary -> tt(NUMS) | tt("(") + r!(expr) + tt(")")
    );

    let pass = CompilerBuilder::new()
        .then(ParserPass::new(grammar), ParseTreeIR::default())
        .then(IncrementalLowerer::new(grammar, mapper), AstArena::new());
    let parser_observer = pass.observe::<Down<Here>>();
    let observer = pass.observe::<Down<Down<Here>>>();

    thread::spawn(move || {
        while let Some(transaction) = parser_observer.recv() {
            println!("======Received CST transaction:");
            for cmd in transaction.iter() {
                cprintln!("<yellow>CST Command: {:?}</>", cmd);
            }
        }
    });

    thread::spawn(move || {
        while let Some(transaction) = observer.recv() {
            let Ok(result) = observer.query(NodePath::root()) else {
                continue;
            };
            println!("=== Current AST: {:?} ===", result);
            println!("======Received AST transaction:");
            for cmd in transaction.iter() {
                cprintln!("<green>AST Command: {:?}</>", cmd);
            }
        }
    });

    let runtime = pass.build_runtime::<BasicInterface<_>>(grammar);

    runtime.insert(0, "1 + 2 * 3").unwrap();
    runtime.replace(0, 1, "x").unwrap();
    thread::sleep(std::time::Duration::from_millis(10));
}
