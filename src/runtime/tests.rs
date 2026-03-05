#[cfg(feature = "webui")]
use crate::interface::BasicInterface;
#[cfg(feature = "webui")]
use crate::interface::webui::WebPreviewInterface;
#[cfg(feature = "webui")]
use crate::{
    parsec::{ParserConfig, recovery::RecoveryConfig, words::*},
    runtime::{Interactive, RuntimeListener},
    semantic::{ASTCell, MapOutput, RuleMap},
};

use crate::new_grammar;
use crate::parsec::Parser;
use crate::runtime::Command;
use crate::runtime::command::NodePath;
use crate::runtime::delta::generate_commands_incremental;

/// Verifies that adding a character inside a deeply-nested node (e.g. typing "a" into a
/// partially-written JSON string key) produces a *minimal* set of semantic commands rather than
/// recreating the entire tree.  Before the fix, the delta algorithm would bail out at root with a
/// full Delete+Insert of every node in the tree.
#[test]
fn test_json_insert_char_minimal_commands() {
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

    let mut parser = Parser::new(grammar);

    // Parse the "before" text: an unclosed JSON object with an unterminated string key.
    let old = parser.parse_text("{\n\"a\": ");
    let old_root = old.root.green;

    // Parse the "after" text: the user typed one character "a" inside the string.
    let new = parser.parse_text("{\n\"a\": 1");
    let new_root = new.root.green;

    let root_path = NodePath(vec![]);

    let cmds = generate_commands_incremental(
        &parser.alloc,
        &root_path,
        old_root,
        new_root,
        0,
        "{\n\"a\": 1",
        true,
    );

    println!("Commands for incremental update from `{{\\n\"a\": ` to `{{\\n\"a\": 1`:");
    for cmd in &cmds {
        println!("  {cmd:?}");
    }

    // Must not recreate the entire tree: no top-level Delete+Insert pair.
    let has_root_delete = cmds
        .iter()
        .any(|c| matches!(c, Command::DeleteNodeAtPath { path } if path.0.is_empty()));
    assert!(
        !has_root_delete,
        "should not emit DeleteNodeAtPath at root — full tree was recreated; cmds: {cmds:?}"
    );

    // No stale/duplicate Delete commands at the same path.
    let delete_paths: Vec<&NodePath> = cmds
        .iter()
        .filter_map(|c| match c {
            Command::DeleteNodeAtPath { path } => Some(path),
            _ => None,
        })
        .collect();
    let unique_deletes: std::collections::HashSet<Vec<usize>> =
        delete_paths.iter().map(|p| p.0.clone()).collect();
    assert_eq!(
        delete_paths.len(),
        unique_deletes.len(),
        "duplicate DeleteNodeAtPath commands detected; cmds: {cmds:?}"
    );
}

#[cfg(feature = "webui")]
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
                .children_with_rule("pair")
                .into_iter()
                .filter_map(|pair| {
                    let key_text = pair
                        .child_with_field("key")
                        .map(|k| k.text_trimmed().trim_matches('"').to_string())?;
                    let value = pair
                        .child_with_field("value")
                        .and_then(|v| v.first_child_ast::<JsonPrimitive>())?;
                    Some((key_text, value))
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
            MapOutput::node(JsonPrimitive::Array(cx.mapped_children()))
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

#[cfg(feature = "webui")]
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
            .on_rule("add", |cx| {
                let lhs = cx
                    .child_with_field("lhs")
                    .and_then(|child| child.first_child_ast::<ExprIr>());
                let rhs = cx
                    .child_with_field("rhs")
                    .and_then(|child| child.first_child_ast::<ExprIr>());
                match (lhs, rhs) {
                    (Some(l), Some(r)) => MapOutput::node(ExprIr::Add(l, r)),
                    _ => MapOutput::node(ExprIr::Error),
                }
            })
            .on_rule("mul", |cx| {
                let lhs = cx
                    .child_with_field("lhs")
                    .and_then(|child| child.first_child_ast::<ExprIr>());
                let rhs = cx
                    .child_with_field("rhs")
                    .and_then(|child| child.first_child_ast::<ExprIr>());
                match (lhs, rhs) {
                    (Some(l), Some(r)) => MapOutput::node(ExprIr::Mul(l, r)),
                    _ => MapOutput::node(ExprIr::Error),
                }
            })
            .on_rule("primary", |cx| {
                if let Some(expr) = cx.first_child_with_rule("expr") {
                    if let Some(ast) = expr.first_child_ast::<ExprIr>() {
                        return MapOutput::alias(ast);
                    }
                }
                MapOutput::node(ExprIr::Number(cx.text_trimmed().parse().unwrap_or(0)))
            })
            .on_error(|_cx| MapOutput::node(ExprIr::Error))
    }

    let listener = RuntimeListener::new().after_update(move |result| {
        println!("> Updated source: {}", result.source_text);
        println!("> Commands: \n");
        for cmd in &result.semantic_commands {
            println!("  {:?}", cmd);
        }
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
fn test_semantic_commands_() {
    let expr_grammar = new_grammar!(
        start where
        start -> r!(expr) + tt(EndOfInput)
        expr -> r!(add) | r!(primary)
        add  -> r!(expr) + tt("+") + r!(expr).drop(1)
        primary -> tt(NUMS)
    );

    let listener = RuntimeListener::new().after_update(move |result| {
        println!("> Updated source: {}", result.source_text);
        println!("> Commands: \n");
        for cmd in &result.semantic_commands {
            println!("  {:?}", cmd);
        }
    });

    let runtime = Interactive::new(expr_grammar)
        .with_listener(listener)
        .with_parser_config(ParserConfig {
            simple_ast: true,
            recovery: RecoveryConfig::default(),
        })
        .finish::<BasicInterface>();
    runtime.run().unwrap();
    runtime.insert(0, "1+2").unwrap();
}
