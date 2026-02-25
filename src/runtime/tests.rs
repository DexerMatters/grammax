use crate::{
    new_grammar,
    parsec::{ParserConfig, recovery::RecoveryConfig, words::*},
    runtime::{Interactive, RuntimeListener},
    semantic::{ASTCell, MapOutput, RuleMap},
};

#[test]
fn test_expr_example() {
    let grammar = new_grammar!(
        json where
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
        println!("> Mapped IR: {:?}", result.semantic_ir_root.unwrap());
        println!("> Duration: {}µs", result.metrics.total_duration_us);
        println!("> Commands: {:?}", result.semantic_commands);
        println!("> Metrics: {:#?}", result.metrics);
    });

    let runtime = Interactive::new(grammar)
        .with_map::<Json, _>(expr_map)
        .with_listener(listener)
        .with_parser_config(ParserConfig {
            simple_ast: true,
            recovery: RecoveryConfig::default(),
        })
        .finish();

    runtime.run().unwrap();
    runtime
        .insert(
            0,
            r#"{
                "name": "Alice",
                "age": 30,
                "isStudent": false,
                "courses": ["Math", "Science"],
                "address": {
                    "street": "123 Main St",
                    "city": "Anytown"
                },
                "nullValue": null
            }"#,
        )
        .unwrap();
    runtime.insert(1, r#" "good" : 123, "#).unwrap();
    runtime.insert(2, r#" "bad": [true, false, 44], "#).unwrap();
    runtime.exit().unwrap();
    runtime.join().unwrap();
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
        println!("> Duration: {}µs", result.metrics.total_duration_us);
        println!("> Commands: {:?}", result.semantic_commands);
        println!("> Metrics: {:#?}", result.metrics);
    });

    let runtime = Interactive::new(expr_grammar)
        .with_map::<ExprIr, _>(expr_map())
        .with_listener(listener)
        .with_parser_config(ParserConfig {
            simple_ast: true,
            recovery: RecoveryConfig::default(),
        })
        .finish();

    runtime.run().unwrap();
    runtime.insert(0, "1 + 4 * 4 + (5 + 5) * 2").unwrap();
    runtime.update(0, 1, "3 * 5").unwrap();
    runtime.insert(0, "2 + 2 + ").unwrap();
    runtime.delete(0, 4).unwrap();
    runtime.exit().unwrap();
    runtime.join().unwrap();
}
