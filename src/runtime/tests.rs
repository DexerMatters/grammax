#[cfg(feature = "webui")]
use crate::interface::webui::WebPreviewInterface;
#[cfg(feature = "webui")]
use crate::{
    new_grammar,
    parsec::{ParserConfig, recovery::RecoveryConfig, words::*},
    runtime::{Interactive, RuntimeListener},
    semantic::{ASTCell, MapOutput, RuleMap},
};

use crate::parsec::Parser;
use crate::runtime::delta::generate_commands_incremental;
use crate::semantic::Command;
use crate::semantic::command::NodePath;

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
