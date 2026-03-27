#[cfg(feature = "webui")]
use color_print::cprintln;

#[cfg(feature = "webui")]
use crate::{
    interface::{BasicInterface, Interface, webui::WebPreviewInterface},
    new_grammar,
    runtime::{BuildTree, CompilerBuilder, Down, Here, Observe, ParserPass},
    scheme::{
        Span, URI,
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
        Object(AstVec<Json>),
        Pair(String, AstCell<Json>),
        Array(AstVec<Json>),
        String(String),
        Number(f64),
        Boolean(bool),
        Null,
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
            ctx.emit(Json::Object(ctx.collect_vec(node)))
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
            Some(ctx.emit(Json::Pair(key, value)))
        })
        // array: emit a stable AstVec; Json children are at descendant paths.
        .rule("array", |ctx, node| {
            ctx.emit(Json::Array(ctx.collect_vec(node)))
        })
        // string: middle child (index 1) is the STRING token content (no quotes).
        .rule("string", |ctx, node| {
            let content = node
                .try_nth(1)
                .map(|n| ctx.read_text(n))
                .unwrap_or_default();
            ctx.emit(Json::String(content))
        })
        .rule("number", |ctx, node| {
            let n: f64 = ctx.read_text(node).parse().unwrap_or(0.0);
            ctx.emit(Json::Number(n))
        })
        .rule("boolean", |ctx, node| {
            ctx.emit(Json::Boolean(ctx.read_text(node) == "true"))
        })
        .rule("null", |ctx, _node| ctx.emit(Json::Null))
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

    let pass = CompilerBuilder::new()
        .then(ParserPass::new(grammar), ParseTreeIR::with_grammar(grammar))
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
                cprintln!("<yellow>CST Command: {:?}</>", cmd);
            }
        }
    });

    thread::spawn(move || {
        while let Some(transaction) = ast_observer.recv() {
            if let Ok(root) = ast_observer.query(DocumentNodePath::root(URI::default())) {
                println!("=== Current JSON AST ===");
                cprintln!("<cyan>{:#?}</>", root);
            }
            println!("=== AST transaction ===");
            for cmd in transaction.iter() {
                cprintln!("<green>AST Command  {:?}</>", cmd);
            }
        }
    });

    let runtime = pass.build_runtime::<WebPreviewInterface<_>>(grammar);
    runtime.run().unwrap();
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
        .error(|ctx, _node| ctx.emit(Expr::Error))
        .rule("expr", |ctx, node| ctx.forward_first(node))
        .on_rule("primary", |ctx, node| {
            if let Some(expr_node) = node.try_first_with_rule("expr") {
                return Some(ctx.forward(expr_node));
            }
            let token = node.each().iter().find(|c| c.token_name().is_some())?;
            let num: usize = ctx.read_text(token).parse().unwrap_or(0);
            Some(ctx.emit(Expr::Num(num)))
        })
        .field("lhs:", |ctx, node| ctx.forward_first(node))
        .field("rhs:", |ctx, node| ctx.forward_first(node))
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
        primary -> tt(NUMBER) | tt("(") + r!(expr) + tt(")")
    );

    let pass = CompilerBuilder::new()
        .then(ParserPass::new(grammar), ParseTreeIR::with_grammar(grammar))
        .then(IncrementalLowerer::new(grammar, mapper), AstArena::new());
    let parser_observer = pass.observe::<Down<Here>>();
    let observer = pass.observe::<Down<Down<Here>>>();

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

    let runtime = pass.build_runtime::<BasicInterface<_>>(grammar);

    let _ = runtime.query_source_text(None, &URI::default(), Span::new(0, usize::MAX));
    thread::sleep(std::time::Duration::from_millis(10));
}
