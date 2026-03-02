use crate::{
    interface::{BasicInterface, webui::WebPreviewInterface},
    new_grammar,
    parsec::{ParserConfig, display::format_ast, recovery::RecoveryConfig, words::*},
    runtime::{Interactive, RuntimeListener},
    semantic::{ASTCell, MapOutput, RuleMap},
};

use crate::parsec::Parser;
use crate::parsec::tree::TreeAllocRefExt;
use crate::runtime::delta::generate_commands_incremental;
use crate::semantic::Command;
use crate::semantic::command::NodePath;

#[test]
fn test_expr_example() {
    let grammar = new_grammar!(
        start where
        start   -> r!(json) + tt(EndOfInput)
        json    -> r!(object) | r!(array) | r!(string) | r!(number) | r!(boolean) | r!(null)
        object  -> tt("{") + sep(r!(pair), tt(",")) + tt("}")
        pair    -> field("key", r!(string)) + tt(":") + field("value", r!(json))
        array   -> tt("[") + sep(r!(json), tt(",")) + tt("]")
        string  -> tt("\"") + t(STRING) + t("\"")
        number  -> tt(NUMS)
        boolean -> tt("true") | tt("false")
        null    -> tt("null")
    );

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Json {
        Object(Vec<(String, ASTCell<JsonPrimitive>)>),
        Error,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum JsonPrimitive {
        Null,
        Boolean(bool),
        Number(u64),
        Array(Vec<ASTCell<Json>>),
        String(String),
    }

    let expr_map = RuleMap::new()
        .on_rule("object", |cx| {
            let entries = cx
                .next_each_rule("pair")
                .map(|mut pair| {
                    let _ = pair.step_in();
                    let _ = pair.next_field("key");
                    let key = pair.text_trimmed().trim_matches('"').to_string();
                    let _ = pair.next_field("value");
                    let value = pair.mapped::<JsonPrimitive>();
                    let _ = pair.step_out();
                    (key, value)
                })
                .collect();
            MapOutput::node(Json::Object(entries))
        })
        .on_rule("null", |_| MapOutput::node(JsonPrimitive::Null))
        .on_rule("boolean", |cx| {
            let text = cx.text_trimmed();
            let value = text == "true";
            MapOutput::node(JsonPrimitive::Boolean(value))
        })
        .on_rule("number", |cx| {
            MapOutput::node(JsonPrimitive::Number(
                cx.text_trimmed().parse().unwrap_or(0),
            ))
        })
        .on_rule("string", |cx| {
            let text = cx.text_trimmed();
            let value = text
                .strip_prefix('"')
                .and_then(|s| s.strip_suffix('"'))
                .unwrap_or(text)
                .to_string();
            MapOutput::node(JsonPrimitive::String(value))
        })
        .on_rule("array", |cx| {
            MapOutput::node(JsonPrimitive::Array(cx.mapped_children().collect()))
        })
        .on_error(|_| MapOutput::node(Json::Error));

    let listener = RuntimeListener::new().after_update(move |result| {
        println!("==== Updated source: {}", result.source_text);
        println!("> Commands: \n");
        for cmd in &result.semantic_commands {
            println!("  {:?}", cmd);
        }
    });

    let runtime = Interactive::new(grammar)
        .with_map::<Json, _>(expr_map)
        .with_listener(listener)
        .with_parser_config(ParserConfig {
            simple_ast: true,
            recovery: RecoveryConfig::default(),
        })
        .finish::<WebPreviewInterface>();

    runtime.run().unwrap();
}

#[test]
fn test_semantic_commands() {
    let expr_grammar = new_grammar!(
        start where
        start -> r!(expr) + tt(EndOfInput)
        expr -> r!(add) | r!(mul) | r!(primary)
        add  -> field("lhs", r!(expr)) + tt("+") + field("rhs", r!(expr).drop(1))
        mul  -> field("lhs", r!(expr).drop(1)) + tt("*") + field("rhs", r!(expr).drop(2))
        primary -> tt(NUMS) | tt("(") + r!(expr) + tt(")")
    );
    #[derive(Debug, Clone, PartialEq, Eq)]
    enum ExprIr {
        Number(u64),
        Add(ASTCell<ExprIr>, ASTCell<ExprIr>),
        Mul(ASTCell<ExprIr>, ASTCell<ExprIr>),
        Error,
    }

    fn expr_map() -> RuleMap<ExprIr> {
        RuleMap::new()
            .on_rule("add", |mut cx| {
                let _ = cx.step_in();
                let _ = cx.next_field("lhs");
                let lhs = cx.mapped();
                let _ = cx.next_field("rhs");
                let rhs = cx.mapped();
                let _ = cx.step_out();
                MapOutput::node(ExprIr::Add(lhs, rhs))
            })
            .on_rule("mul", |mut cx| {
                let _ = cx.step_in();
                let _ = cx.next_field("lhs");
                let lhs = cx.mapped();
                let _ = cx.next_field("rhs");
                let rhs = cx.mapped();
                let _ = cx.step_out();
                MapOutput::node(ExprIr::Mul(lhs, rhs))
            })
            .on_rule("primary", |mut cx| {
                if cx.step_in() && cx.next_rule("expr") {
                    MapOutput::alias(cx.mapped::<ExprIr>())
                } else {
                    MapOutput::node(ExprIr::Number(cx.text_trimmed().parse().unwrap_or(0)))
                }
            })
            .on_error(|_cx| MapOutput::node(ExprIr::Error))
    }

    let listener = RuntimeListener::new().after_update(move |result| {
        println!("> Updated source: {}", result.source_text);
        println!("> Mapped IR: {:?}", result.semantic_ir_root.unwrap());
        println!("> Commands: \n");
        for cmd in &result.semantic_commands {
            println!("  {:?}", cmd);
        }
        println!(
            "> AST {}",
            format_ast(
                &result.current_parser.grammar,
                result.current_tree,
                &result.current_parser.alloc,
                result.source_text
            )
        );
        println!("> Duration: {}µs", result.metrics.total_duration_us);
        println!("> Metrics: {:#?}", result.metrics);
    });

    let runtime = Interactive::new(expr_grammar)
        .with_map::<ExprIr, _>(expr_map())
        .with_listener(listener)
        .with_parser_config(ParserConfig {
            simple_ast: true,
            recovery: RecoveryConfig::default(),
        })
        .finish::<WebPreviewInterface>();
    runtime.run().unwrap();
}

#[test]
fn test_incremental_leaf_update_emits_replace_command() {
    let grammar = new_grammar!(
        start where
        start -> r!(expr) + tt(EndOfInput)
        expr -> r!(add) | r!(mul) | r!(primary)
        add  -> field("lhs", r!(expr)) + tt("+") + field("rhs", r!(expr).drop(1))
        mul  -> field("lhs", r!(expr).drop(1)) + tt("*") + field("rhs", r!(expr).drop(2))
        primary -> tt(NUMS) | tt("(") + r!(expr) + tt(")")
    );

    let mut parser = Parser::new(grammar);
    let old = parser.parse_text("1+1");
    let old_root = old.root.green;
    let new = parser.parse_text("1+12");
    let new_root = new.root.green;

    let leaf_path = NodePath(vec![0, 0, 2, 0, 0, 0]);

    fn green_at_path(
        alloc: &crate::parsec::tree::TreeAllocRef,
        root: usize,
        path: &[usize],
    ) -> Option<usize> {
        let mut cur = root;
        for &ix in path {
            let node = alloc.get_node(cur);
            cur = *node.children.get(ix)?;
        }
        Some(cur)
    }

    fn offset_at_path(
        alloc: &crate::parsec::tree::TreeAllocRef,
        root: usize,
        path: &[usize],
    ) -> Option<usize> {
        let mut cur = root;
        let mut offset = 0usize;
        for &ix in path {
            let node = alloc.get_node(cur);
            if ix > node.children.len() {
                return None;
            }
            for &child in node.children.iter().take(ix) {
                offset += alloc.get_node(child).width;
            }
            cur = *node.children.get(ix)?;
        }
        Some(offset)
    }

    let old_green =
        green_at_path(&parser.alloc, old_root, &leaf_path.0).expect("old leaf path should exist");
    let new_green =
        green_at_path(&parser.alloc, new_root, &leaf_path.0).expect("new leaf path should exist");
    let new_offset = offset_at_path(&parser.alloc, new_root, &leaf_path.0)
        .expect("new leaf offset should be computable");

    let commands = generate_commands_incremental(
        &parser.alloc,
        &leaf_path,
        old_green,
        new_green,
        new_offset,
        "1+12",
        false,
    );

    assert!(
        commands
            .iter()
            .any(|c| matches!(c, Command::ReplaceNodeAtPath { path, .. } if path.0 == leaf_path.0)),
        "leaf update should emit ReplaceNodeAtPath for non-root path; commands: {commands:?}"
    );

    let delete_paths: Vec<Vec<usize>> = commands
        .iter()
        .filter_map(|c| match c {
            Command::DeleteNodeAtPath { path } if !path.0.is_empty() => Some(path.0.clone()),
            _ => None,
        })
        .collect();
    let insert_paths: Vec<Vec<usize>> = commands
        .iter()
        .filter_map(|c| match c {
            Command::InsertNodeAtPath { path, .. } if !path.0.is_empty() => Some(path.0.clone()),
            _ => None,
        })
        .collect();

    let has_non_root_delete_insert_pair = delete_paths
        .iter()
        .any(|d| insert_paths.iter().any(|i| i == d));
    assert!(
        !has_non_root_delete_insert_pair,
        "leaf update should not emit delete+insert pair on same non-root path; commands: {commands:?}"
    );
}
