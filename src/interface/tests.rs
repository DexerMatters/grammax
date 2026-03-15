#[cfg(feature = "webui")]
use color_print::cprintln;

#[cfg(feature = "webui")]
use crate::{
    interface::BasicInterface,
    new_grammar,
    parsec::{view::NodeView, words::*},
    runtime::{BuildTree, CompilerBuilder, Down, Here, Observe, ParserPass},
    scheme::{
        layers::{AstArena, AstCell, AstVec, NodePath, ParseTreeIR},
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
    #[derive(Debug, Clone, PartialEq)]
    enum Json {
        Object(AstVec<Json>),        // children in arena: Pair nodes
        Pair(String, AstCell<Json>), // key text, typed value cell
        Array(AstVec<Json>),         // children in arena: Json nodes
        String(String),
        Number(f64),
        Boolean(bool),
        Null,
        Error,
    }

    let mapper = AstMapper::new()
        .skip_rule("start")
        .on_error(|ctx, _node| Some(ctx.emit(Json::Error)))
        // json is purely a dispatch-level rule; forward straight through.
        .on_rule("json", |ctx, node| ctx.forward_first_child(node))
        // object: emit a stable AstVec rooted at this node's path.
        // Pair nodes are stored as direct-ish children in the arena.
        .on_rule("object", |ctx, node| {
            Some(ctx.emit(Json::Object(ctx.collect_vec(node))))
        })
        // pair: read key text from CST; resolve value through the mapper.
        .on_rule("pair", |ctx, node| {
            // key: navigate to the `string` node and grab the STRING token (child 1).
            let key_str_node = node.try_first_with_field("key")?;
            let key = key_str_node
                .try_nth(1)
                .map(|n| ctx.read_text(n))
                .unwrap_or_default();
            // value: ctx.read_cell resolves through json → concrete type → anchor path.
            let value: AstCell<Json> = ctx.read_cell(node.try_first_with_field("value")?)?;
            Some(ctx.emit(Json::Pair(key, value)))
        })
        // array: emit a stable AstVec; Json children are at descendant paths.
        .on_rule("array", |ctx, node| {
            Some(ctx.emit(Json::Array(ctx.collect_vec(node))))
        })
        // string: middle child (index 1) is the STRING token content (no quotes).
        .on_rule("string", |ctx, node| {
            let content = node
                .try_nth(1)
                .map(|n| ctx.read_text(n))
                .unwrap_or_default();
            Some(ctx.emit(Json::String(content)))
        })
        .on_rule("number", |ctx, node| {
            let n: f64 = ctx.read_text(node).parse().unwrap_or(0.0);
            Some(ctx.emit(Json::Number(n)))
        })
        .on_rule("boolean", |ctx, node| {
            Some(ctx.emit(Json::Boolean(ctx.read_text(node) == "true")))
        })
        .on_rule("null", |ctx, _node| Some(ctx.emit(Json::Null)))
        // key field: read text directly in the pair handler above; nothing to store.
        .skip_field("key")
        // value field: forward into the json node so `read_cell` resolves it.
        .on_field("value", |ctx, node| ctx.forward_first_child(node));

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

    let pass = CompilerBuilder::new()
        .then(ParserPass::new(grammar), ParseTreeIR::default())
        .then(
            IncrementalLowerer::new(grammar, mapper),
            AstArena::default(),
        );
    let cst_observer = pass.observe::<Down<Here>>();
    let ast_observer = pass.observe::<Down<Down<Here>>>();
    thread::spawn(move || {
        while let Some(transaction) = cst_observer.recv() {
            println!("=== CST transaction ===");
            for cmd in transaction.iter() {
                println!("CST Command  {:?}", cmd);
            }
        }
    });

    thread::spawn(move || {
        while let Some(transaction) = ast_observer.recv() {
            if let Ok(root) = ast_observer.query(NodePath::root()) {
                println!("=== Current JSON AST: {:?} ===", root);
            }
            println!("=== AST transaction ===");
            for cmd in transaction.iter() {
                println!("AST Command  {:?}", cmd);
            }
        }
    });

    let runtime = pass.build_runtime::<BasicInterface<_>>(grammar);
    runtime
        .insert(0, r#"{"name": "John", "age": 30, "is_student": false}"#)
        .unwrap();
    thread::sleep(std::time::Duration::from_millis(10));
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
