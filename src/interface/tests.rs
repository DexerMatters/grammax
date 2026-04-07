#[cfg(feature = "webui")]
use color_print::cprintln;

#[cfg(feature = "webui")]
use crate::{
    interface::webui::WebPreviewInterface,
    new_grammar,
    runtime::{Build, ParserPass},
    scheme::{
        Span,
        layers::{
            AstArena, AstCell, AstVec, DocumentNodePath, ParseTreeIR, ParseTreeQuery,
            ParseTreeValue,
        },
        passes::{AstMapper, IncrementalLowerer},
    },
};

#[cfg(feature = "webui")]
use std::thread;

#[cfg(feature = "webui")]
#[test]
fn test_tap_prints_cst_commands() {
    #[derive(Debug, Clone, PartialEq)]
    enum Json {
        Object(Span, AstVec<Json>),
        Pair(Span, String, AstCell<Json>),
        Array(Span, AstVec<Json>),
        String(Span, String),
        Number(Span, f64),
        Boolean(Span, bool),
        Null(Span),
        Error,
    }

    let mapper = AstMapper::new()
        .skip_rule("start")
        .error(|ctx, _node| ctx.emit(Json::Error))
        // json is purely a dispatch-level rule; forward straight through.
        .rule("json", |ctx, node| ctx.forward_first(node))
        // object: emit a stable AstVec rooted at this node's path.
        // Pair nodes are stored as direct-ish children in the arena.
        .rule("object", |ctx, node| {
            ctx.emit(Json::Object(node.span(), ctx.collect_vec(node)))
        })
        // pair: read key text from CST; resolve value through the mapper.
        // Uses on_rule because it needs the ? operator
        .on_rule("pair", |ctx, node| {
            // key: navigate to the `string` node and grab the STRING token (child 1).
            let key_str_node = node.try_first_with_field("key")?;
            let key = key_str_node
                .try_nth(1)
                .map(|n| ctx.read_text(n))
                .unwrap_or_default();
            // value: ctx.read_cell resolves through json → concrete type → anchor path.
            let value: AstCell<Json> = ctx.read_cell(node.try_first_with_field("value")?)?;
            Some(ctx.emit(Json::Pair(node.span(), key, value)))
        })
        // array: emit a stable AstVec; Json children are at descendant paths.
        .rule("array", |ctx, node| {
            ctx.emit(Json::Array(node.span(), ctx.collect_vec(node)))
        })
        // string: middle child (index 1) is the STRING token content (no quotes).
        .rule("string", |ctx, node| {
            let content = node
                .try_nth(1)
                .map(|n| ctx.read_text(n))
                .unwrap_or_default();
            ctx.emit(Json::String(node.span(), content))
        })
        .rule("number", |ctx, node| {
            let n: f64 = ctx.read_text(node).parse().unwrap_or(0.0);
            ctx.emit(Json::Number(node.span(), n))
        })
        .rule("boolean", |ctx, node| {
            ctx.emit(Json::Boolean(node.span(), ctx.read_text(node) == "true"))
        })
        .rule("null", |ctx, node| ctx.emit(Json::Null(node.span())))
        // key field: read text directly in the pair handler above; nothing to store.
        .skip_field("key")
        // value field: forward into the json node so `read_cell` resolves it.
        .field("value", |ctx, node| ctx.forward_first(node));

    let grammar = new_grammar!(
        start where
        start   -> r!(json) + tt(EndOfInput)
        json    -> r!(object) | r!(array) | r!(string) | r!(number) | r!(boolean) | r!(null)
        object  -> tt("{") + sep(r!(pair), tt(",")) + tt("}")
        pair    -> field("key", r!(string)) + tt(":") + field("value", r!(json))
        array   -> tt("[") + sep(r!(json), tt(",")) + tt("]")
        string  -> tt("\"") + t(STRING) + tt("\"")
        number  -> tt(NUMBER)
        boolean -> tt("true") | tt("false")
        null    -> tt("null")
    );

    let (pass_runtime, cst_observer, ast_observer) = Build::new().then(
        ParserPass::new(grammar),
        ParseTreeIR::with_grammar(grammar),
        |b, cst_obs| {
            b.then(
                IncrementalLowerer::new(grammar, mapper),
                AstArena::default(),
                |b, ast_obs| {
                    (
                        b.build_runtime::<WebPreviewInterface<_>>(grammar),
                        cst_obs,
                        ast_obs,
                    )
                },
            )
        },
    );
    thread::spawn(move || {
        while let Some(transaction) = cst_observer.recv() {
            println!("=== CST transaction ===");
            for cmd in transaction.iter() {
                cprintln!("<yellow>CST Command: {:?}</>", cmd);
            }
        }
    });

    thread::spawn(move || {
        while let Some(transaction) = ast_observer.recv() {
            if let Ok(root) = ast_observer.query(DocumentNodePath::root("file://undefined")) {
                println!("=== Current JSON AST ===");
                cprintln!("<cyan>{:#?}</>", root);
            }
            println!("=== AST transaction ===");
            for cmd in transaction.iter() {
                cprintln!("<green>AST Command  {:?}</>", cmd);
            }
        }
    });

    pass_runtime.run().unwrap();
}

#[cfg(feature = "webui")]
#[test]
fn test_arith_commands() {
    #[derive(Debug, Clone, PartialEq)]
    enum Expr {
        Num(Span, usize),
        Add(Span, AstCell<Expr>, AstCell<Expr>),
        Mul(Span, AstCell<Expr>, AstCell<Expr>),
        Error(Span),
    }

    let mapper = AstMapper::new()
        .skip_rule("start")
        .error(|ctx, node| ctx.emit(Expr::Error(node.span())))
        .rule("expr", |ctx, node| ctx.forward_first(node))
        .on_rule("primary", |ctx, node| {
            if let Some(expr_node) = node.try_first_with_rule("expr") {
                return Some(ctx.forward(expr_node));
            }
            let token = node.each().iter().find(|c| c.token_name().is_some())?;
            let num: usize = ctx.read_text(token).parse().unwrap_or(0);
            Some(ctx.emit(Expr::Num(token.span(), num)))
        })
        .field("lhs:", |ctx, node| ctx.forward_first(node))
        .field("rhs:", |ctx, node| ctx.forward_first(node))
        .on_rule("add", |ctx, node| {
            let lhs = ctx.read_cell(node.try_first_with_field("lhs:")?)?;
            let rhs = ctx.read_cell(node.try_first_with_field("rhs:")?)?;
            Some(ctx.emit(Expr::Add(node.span(), lhs, rhs)))
        })
        .on_rule("mul", |ctx, node| {
            let lhs = ctx.read_cell(node.try_first_with_field("lhs:")?)?;
            let rhs = ctx.read_cell(node.try_first_with_field("rhs:")?)?;
            Some(ctx.emit(Expr::Mul(node.span(), lhs, rhs)))
        });

    let grammar = new_grammar!(
        start where
        start -> r!(expr) + tt(EndOfInput)
        expr -> r!(add) | r!(mul) | r!(primary)
        add  -> field("lhs:", r!(expr)) + tt("+") + field("rhs:", r!(expr).drop(1))
        mul  -> field("lhs:", r!(expr).drop(1)) + tt("*") + field("rhs:", r!(expr).drop(2))
        primary -> tt(NUMBER) | tt("(") + r!(expr) + tt(")")
    );

    let (pass_runtime, parser_observer, observer) = Build::new().then(
        ParserPass::new(grammar),
        ParseTreeIR::with_grammar(grammar),
        |b, parser_obs| {
            b.then(
                IncrementalLowerer::new(grammar, mapper),
                AstArena::new(),
                |b, obs| {
                    (
                        b.build_runtime::<WebPreviewInterface<_>>(grammar),
                        parser_obs,
                        obs,
                    )
                },
            )
        },
    );

    thread::spawn(move || {
        while let Some(transaction) = parser_observer.recv() {
            let Ok(result) = parser_observer.query(ParseTreeQuery::Path(DocumentNodePath::root(
                "file://preview",
            ))) else {
                continue;
            };

            match result {
                ParseTreeValue::View(view) => {
                    println!("=== Current CST ===");
                    println!("{}", view);
                }
                other => {
                    println!("=== Current CST query result: {:?} ===", other);
                }
            }
            println!("======Received CST transaction:");
            for cmd in transaction.iter() {
                cprintln!("<yellow>CST Command: {:?}</>", cmd);
            }
        }
    });

    thread::spawn(move || {
        while let Some(transaction) = observer.recv() {
            let Ok(result) = observer.query(DocumentNodePath::root("file://preview")) else {
                continue;
            };
            println!("=== Current AST: {:?} ===", result);
            println!("======Received AST transaction:");
            for cmd in transaction.iter() {
                cprintln!("<green>AST Command: {:?}</>", cmd);
            }
        }
    });

    pass_runtime.run().unwrap();
}
