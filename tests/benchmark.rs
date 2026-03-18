use grammax::new_grammar_no_cache;

#[test]
fn test_json_parser_benchmark() {
    let grammar = new_grammar_no_cache!(
        start where
        start   -> r!(json) + tt(EndOfInput)
        json    -> r!(object) | r!(array) | r!(string) | r!(number) | r!(boolean) | r!(null)
        object  -> tt("{") + sep(r!(pair), tt(",")) + tt("}")
        pair    -> field("key", r!(string)) + tt(":") + field("value", r!(json))
        array   -> tt("[") + sep(r!(json), tt(",")) + tt("]")
        string  -> tt("\"") + t(STRING) + t("\"")
        number  -> tt(NUMBER)
        boolean -> tt("true") | tt("false")
        null    -> tt("null")
    );

    let result = grammar.parse(r#"123"#);
}
