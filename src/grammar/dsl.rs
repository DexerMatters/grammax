use std::sync::Arc;

use rust_embed::Embed;
use rustc_hash::FxHashMap;

use crate::{
    grammar::{
        Grammar, GrammarError,
        edsl::{self, GrammarNode},
    },
    new_grammar,
    parsec::{
        self,
        view::View,
        words::{self, EndOfInput, IDENT, Matcher, NUMS, STRING},
    },
};

thread_local! {
    pub static GRAMMAX_DSL_GRAMMAR: &'static Grammar = new_grammar! {
        table where
        table -> sep(r!(rule), t('\n')) + tt(EndOfInput)
        rule -> field("name", tt(IDENT)) + tt("->") + field("definition", r!(expr))
        expr -> r!(alternative) | r!(sequence) | r!(fields) | r!(drop) | r!(some) | r!(many) | r!(terminal) | r!(reference)
        alternative -> r!(expr).drop(1) + tt("|") + r!(expr)
        sequence -> r!(expr).drop(2) + t(" ") + r!(expr).drop(1)
        fields -> field("field_name", tt(IDENT)) + tt(":") + r!(expr).drop(3)
        drop -> r!(expr).drop(4) + t("/") + tt(NUMS)
        many -> r!(expr).drop(6) + opt(t("{") + field("sep", r!(expr)) + tt("}")) + t("*")
        some -> r!(expr).drop(6) + opt(t("{") + field("sep", r!(expr)) + tt("}")) + t("+")
        reference -> tt(IDENT)
        terminal -> (tt("(") + r!(expr) + tt(")")) | r!(literal) | r!(token)
        token -> tt("IDENT") | tt("STRING") | tt("NUMBER") | tt("ALPHANUMS") | tt("ALPHABETS") | tt("EOF")
        literal -> tt('"') + tt(STRING) + tt('"')
    };
}

#[derive(Embed)]
#[folder = "grammars/"]
#[include = "grammax.gmx.bin"]
struct Asset;

fn translate_dsl_grammar(result: parsec::Result<'_>) -> Result<&'static Grammar, GrammarError> {
    let view: View = result.view();
    let mut registry_map = FxHashMap::default();
    let mut start_rule = None;
    for rule_view in view
        .into_each()
        .into_iter()
        .filter(|v| v.rule_name() == Some("rule"))
    {
        let (name, node) = view_rule(rule_view.into().unwrap());
        if start_rule.is_none() {
            start_rule = Some(name);
        }
        registry_map.insert(name, node);
    }

    let registry = edsl::GrammarRegistry::from_map(registry_map);
    start_rule
        .map(|start| Grammar::new_uncached_with_registry(start, registry))
        .unwrap_or_else(|| Err(GrammarError::NoStartRule))
}

fn view_rule(view: View) -> (&'static str, GrammarNode) {
    let name = view.next_field("name").map(|n| n.text().trim()).unwrap();
    let name = Box::leak(name.to_string().into_boxed_str());

    let node = view
        .next_field("definition")
        .and_then(|d| d.into())
        .map(view_expr)
        .unwrap();

    println!("Parsed rule: {} -> {:?}", name, node);

    (name, node)
}

fn view_expr(view: View) -> GrammarNode {
    match view.rule_name().unwrap_or("") {
        "expr" => view
            .into()
            .map(view_expr)
            .unwrap_or_else(|| view_leaf_expr(view)),
        "alternative" => {
            let exprs: Vec<_> = view.into_each_rule("expr").map(view_expr).collect();
            let mut flattened = Vec::new();
            for expr in exprs {
                match expr {
                    GrammarNode::Alternative(inner) => flattened.extend(inner),
                    node => flattened.push(node),
                }
            }

            match flattened.len() {
                1 => flattened.into_iter().next().unwrap(),
                _ => GrammarNode::Alternative(flattened),
            }
        }
        "sequence" => {
            let exprs: Vec<_> = view.into_each_rule("expr").map(view_expr).collect();
            match exprs.len() {
                1 => exprs.into_iter().next().unwrap(),
                _ => GrammarNode::Sequence(exprs),
            }
        }
        "fields" => {
            let field_name = view
                .into_each_field("field_name")
                .next()
                .and_then(|field| field.into())
                .map(|n| n.text().trim())
                .unwrap_or("");
            let field_name = Box::leak(field_name.to_string().into_boxed_str()) as &'static str;

            let expr = view
                .into_each_rule("expr")
                .next()
                .map(view_expr)
                .unwrap_or_else(|| {
                    panic!(
                        "Missing field expression in fields node: {}",
                        view.display()
                    )
                });

            GrammarNode::Field(field_name, Box::new(expr))
        }
        "drop" => {
            let expr = view
                .into_each_rule("expr")
                .next()
                .map(view_expr)
                .unwrap_or_else(|| panic!("Missing expression in drop node: {}", view.display()));
            let count = view
                .into_each()
                .find_map(|child| child.text().trim().parse::<usize>().ok())
                .unwrap_or_else(|| panic!("Missing drop count in drop node: {}", view.display()));

            GrammarNode::Drop {
                node: Box::new(expr),
                count,
            }
        }
        "some" => view_repetition_expr(view, 1),
        "many" => view_repetition_expr(view, 0),
        "terminal" => view_terminal(view),
        "reference" => {
            let ident = Box::leak(view.text().trim().to_string().into_boxed_str());
            GrammarNode::UnboundReference(ident.to_string())
        }
        "token" => view_token(view),
        "literal" => view_literal(view),
        _ => unimplemented!(
            "Unsupported expression type: {}",
            view.rule_name().unwrap_or("")
        ),
    }
}

fn view_repetition_expr(view: View, min: usize) -> GrammarNode {
    let expr = view
        .into_each_rule("expr")
        .next()
        .map(view_expr)
        .unwrap_or_else(|| panic!("Missing repeated expression in node: {}", view.display()));

    let sep = view.into_each_field("sep").next().and_then(|field| {
        field
            .into()
            .and_then(|value| value.into_each_rule("expr").next())
            .map(view_expr)
    });

    if let Some(separator) = sep {
        GrammarNode::SeparatedRepetition {
            node: Box::new(expr),
            separator: Box::new(separator),
            min,
            max: None,
        }
    } else {
        GrammarNode::Repetition {
            node: Box::new(expr),
            min,
            max: None,
        }
    }
}
fn view_leaf_expr(view: View) -> GrammarNode {
    let text = view.text().trim();
    assert!(
        !text.is_empty(),
        "Expected concrete expr leaf, got empty text"
    );

    if text.starts_with('"') && text.ends_with('"') && text.len() >= 2 {
        let inner = text.trim_matches('"');
        let inner = Box::leak(inner.to_string().into_boxed_str()) as &'static str;
        return GrammarNode::Terminal(Arc::new(words::token(inner)));
    }

    if text.chars().all(|c| c.is_ascii_digit()) {
        return GrammarNode::Terminal(Arc::new(words::token(words::NUMS)));
    }

    if let Some(node) = match text {
        "IDENT" | "STRING" | "NUMBER" | "ALPHANUMS" | "ALPHABETS" | "EOF" => Some(view_token(view)),
        _ => None,
    } {
        return node;
    }

    GrammarNode::UnboundReference(text.to_string())
}

fn view_token(view: View) -> GrammarNode {
    let token_name = view.text().trim();
    let matcher: Arc<dyn Matcher + Send + Sync> = match token_name {
        "IDENT" => Arc::new(words::token(words::IDENT)),
        "STRING" => Arc::new(words::token(words::STRING)),
        "NUMBER" => Arc::new(words::token(words::NUMS)),
        "ALPHANUMS" => Arc::new(words::token(words::ALPHANUMS)),
        "ALPHABETS" => Arc::new(words::token(words::ALPHAS)),
        "EOF" => Arc::new(words::token(words::EndOfInput)),
        _ => panic!("Unsupported token type: {}", token_name),
    };
    GrammarNode::Terminal(matcher)
}

fn view_literal(view: View) -> GrammarNode {
    let text = view
        .into_each()
        .find(|child| {
            let raw = child.text().trim();
            !raw.is_empty() && raw != "\""
        })
        .map(|child| child.text().trim())
        .unwrap_or("");

    let text = Box::leak(text.to_string().into_boxed_str()) as &'static str;
    GrammarNode::Terminal(Arc::new(words::token(text)))
}

fn view_terminal(view: View) -> GrammarNode {
    if let Some(expr) = view.into_each_rule("expr").next() {
        return view_expr(expr);
    }

    if let Some(token) = view.into_each_rule("token").next() {
        return view_token(token);
    }

    if let Some(literal) = view.into_each_rule("literal").next() {
        return view_literal(literal);
    }

    panic!("Unsupported terminal shape: {}", view.display())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parsec::Parser;

    #[test]
    fn test_dsl_grammar() {
        let grammar = Grammar::load_from_binary(Asset::get("grammax.gmx.bin").unwrap().data)
            .expect("Failed to load grammar");
        // let (pass, _) = CompilerBuilder::new()
        //     .then_pass(ParserPass::new(grammar))
        //     .then_layer(RedGreenTreeIR::default())
        //     .tap();

        // let runtime = RuntimeService::<WebPreviewInterface>::new(grammar, move |evt_tx| {
        //     ComposedCompiler::from_pass_with_events(pass, evt_tx)
        // });
        // runtime.run().expect("runtime failed");

        let mut parser = Parser::new(grammar);

        println!("Grammar:\n{}", parser.grammar.table);

        let text = r#"
start -> expr EOF
expr -> add | mul | primary
add -> lhs:expr "+" rhs:expr/1
mul -> lhs:expr/1 "*" rhs:expr/2
primary -> NUMBER | "(" expr ")"
"#;

        let result = parser.parse_text(text);

        let output = result.format_ast();

        println!("AST {}:\n{}", text, output);
        println!("Messages:\n{}", result.format_messages());

        let result = translate_dsl_grammar(result);
        match result {
            Ok(translated_grammar) => {
                println!("Translated Grammar:\n{}", translated_grammar.table);
            }
            Err(e) => {
                println!("Error translating grammar: {:?}", e);
            }
        }
    }
}
