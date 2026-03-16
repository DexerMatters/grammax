use std::sync::Arc;

use rust_embed::Embed;
use rustc_hash::FxHashMap;

use crate::{
    grammar::{
        Grammar, GrammarError,
        edsl::{self, GrammarNode},
    },
    new_grammar_no_cache,
    parsec::{
        self,
        view::{ViewAction, Viewer},
        words::{self, EndOfInput, IDENT, Matcher, NUMS, NamedMatcher, RegexMatcher, STRING},
    },
    utils::Span,
};

thread_local! {
    static REGEXP_MATCHER: NamedMatcher<RegexMatcher> = NamedMatcher::new(
        "regexp",
        RegexMatcher::new(r#"([^/\\\r\n]|\\.)+"#),
    );
    #[allow(dead_code)]
    static GRAMMAX_DSL_GRAMMAR_PROTOTYPE: &'static Grammar = new_grammar_no_cache! {
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
        terminal -> (tt("(") + r!(expr) + tt(")")) | opt(t("!")) + r!(primary)
        primary -> r!(token) | r!(literal) | r!(regexp)
        token -> tt("IDENT") | tt("STRING") | tt("NUMBER") | tt("ALPHANUMS") | tt("ALPHABETS") | tt("EOF")
        literal -> tt('"') + t(STRING) + t('"')
        regexp -> tt('/') + t(REGEXP_MATCHER.with(|x| x.clone())) + t('/')
    };
}

#[inline(always)]
pub fn grammax_dsl_grammar() -> &'static Grammar {
    // SAFE: The grammar is loaded from a precompiled binary, which is guaranteed to be valid and immutable.
    Grammar::load_from_binary(Asset::get("grammax.gmx.bin").unwrap().data)
        .expect("Failed to load grammar")
}

impl Grammar {
    pub fn interpret(dsl: impl AsRef<str>) -> Result<&'static Self, GrammarError> {
        let mut parser = parsec::Parser::new(grammax_dsl_grammar());
        let result = parser.parse_text(dsl.as_ref());
        if !result.messages.is_empty() {
            return Err(GrammarError::ParseError(result.format_messages()));
        }
        translate_dsl_grammar(result)
    }

    pub fn interpret_file(
        path: impl AsRef<std::path::Path>,
    ) -> Result<&'static Self, GrammarError> {
        let text = std::fs::read_to_string(path).map_err(|e| GrammarError::IoError(e))?;
        Self::interpret(&text)
    }
}

#[derive(Embed)]
#[folder = "grammars/"]
#[include = "grammax.gmx.bin"]
struct Asset;

fn translate_dsl_grammar(result: parsec::Result<'_>) -> Result<&'static Grammar, GrammarError> {
    let viewer = build_dsl_viewer(&result);
    let root = result.view();
    let mut registry_map = FxHashMap::default();
    let mut start_rule = None;
    for rule_view in root.each_with_rule("rule") {
        let (name, node): (String, GrammarNode) = rule_view.view(&viewer);
        let name: &'static str = Box::leak(name.into_boxed_str());
        if start_rule.is_none() {
            start_rule = Some(name);
        }
        registry_map.insert(name, node);
    }

    let registry = edsl::GrammarRegistry::from_map(registry_map);
    start_rule
        .map(|start| Grammar::new_uncached_with_registry(start, registry))
        .unwrap_or_else(|| Err(GrammarError::NoStartRule(Span::empty())))
}

fn build_dsl_viewer(result: &parsec::Result<'_>) -> Viewer {
    result
        .viewer()
        .on_token::<bool, _>("!", |_viewer, _node| ViewAction::Exact(true))
        .on_field::<String, _>("name", |_viewer, node| {
            ViewAction::Exact(node.text_trimmed())
        })
        .on_field::<String, _>("field_name", |_viewer, node| {
            ViewAction::Exact(node.text_trimmed())
        })
        .on_field("definition", |viewer, node| {
            ViewAction::Exact(node[0].view::<GrammarNode>(viewer))
        })
        .on_field("sep", |viewer, node| {
            ViewAction::Exact(node[0].view::<GrammarNode>(viewer))
        })
        .on_rule("expr", |_viewer, _node| ViewAction::<GrammarNode>::Relay)
        .on_rule("primary", |_viewer, _node| ViewAction::<GrammarNode>::Relay)
        .on_rule::<(String, GrammarNode), _>("rule", |viewer, node| {
            ViewAction::Exact((
                node.first_with_field("name").view::<String>(viewer),
                node.first_with_field("definition")
                    .view::<GrammarNode>(viewer),
            ))
        })
        .on_rule("alternative", |viewer, node| {
            let exprs: Vec<_> = node
                .each_with_rule("expr")
                .into_iter()
                .map(|child| child.view::<GrammarNode>(viewer))
                .collect();

            let mut flattened = Vec::new();
            for expr in exprs {
                match expr {
                    GrammarNode::Alternative(inner, _) => flattened.extend(inner),
                    node => flattened.push(node),
                }
            }

            let span = node.span();
            ViewAction::Exact(match flattened.len() {
                1 => flattened.into_iter().next().unwrap(),
                _ => GrammarNode::Alternative(flattened, span),
            })
        })
        .on_rule("sequence", |viewer, node| {
            let exprs: Vec<_> = node
                .each_with_rule("expr")
                .into_iter()
                .map(|child| child.view::<GrammarNode>(viewer))
                .collect();

            let mut flattened = Vec::new();
            for expr in exprs {
                match expr {
                    GrammarNode::Sequence(inner, _) => flattened.extend(inner),
                    node => flattened.push(node),
                }
            }

            let span = node.span();
            ViewAction::Exact(match flattened.len() {
                1 => flattened.into_iter().next().unwrap(),
                _ => GrammarNode::Sequence(flattened, span),
            })
        })
        .on_rule("fields", |viewer, node| {
            let field_name = node.first_with_field("field_name").view::<String>(viewer);
            let expr = node.first_with_rule("expr").view::<GrammarNode>(viewer);

            ViewAction::Exact(GrammarNode::Field(
                Box::new(field_name).leak(),
                Box::new(expr),
                node.span(),
            ))
        })
        .on_rule("drop", |viewer, node| {
            let expr = node.first_with_rule("expr").view::<GrammarNode>(viewer);
            let count = node.last().view::<usize>(viewer);

            ViewAction::Exact(GrammarNode::Drop {
                node: Box::new(expr),
                count,
                span: node.span(),
            })
        })
        .on_rule("some", |viewer, node| {
            let expr = node.first_with_rule("expr").view::<GrammarNode>(viewer);
            let sep = node
                .try_first_with_field("sep")
                .map(|field| field.view::<GrammarNode>(viewer));
            ViewAction::Exact(repetition_node(expr, sep, 1, node.span()))
        })
        .on_rule("many", |viewer, node| {
            let expr = node.first_with_rule("expr").view::<GrammarNode>(viewer);
            let sep = node
                .try_first_with_field("sep")
                .map(|field| field.view::<GrammarNode>(viewer));
            ViewAction::Exact(repetition_node(expr, sep, 0, node.span()))
        })
        .on_rule("terminal", |viewer, node| {
            if let Some(expr) = node.try_first_with_rule("expr") {
                return ViewAction::Exact(expr.view::<GrammarNode>(viewer));
            }

            let is_raw = node
                .try_first_with_token("!")
                .map(|token| token.view::<bool>(viewer))
                .unwrap_or(false);
            let primary = node.first_with_rule("primary");

            if let Some(token) = primary.try_first_with_rule("token") {
                return ViewAction::Exact(grammar_token_from_text(
                    &token.text_trimmed(),
                    token.span(),
                    is_raw,
                ));
            }

            if let Some(regexp) = primary.try_first_with_rule("regexp") {
                return ViewAction::Exact(grammar_regex_from_text(
                    &regexp[1].text(),
                    regexp.span(),
                    is_raw,
                ));
            }

            let literal = primary.try_first_with_rule("literal").unwrap();
            ViewAction::Exact(grammar_literal_from_text(
                &literal[1].text_normalized(),
                literal.span(),
                is_raw,
            ))
        })
        .on_rule("reference", |_viewer, node| {
            ViewAction::Exact(GrammarNode::UnboundReference(
                node.text_trimmed(),
                node.span(),
            ))
        })
        .on_rule("token", |_viewer, node| {
            ViewAction::Exact(grammar_token_from_text(
                &node.text_trimmed(),
                node.span(),
                false,
            ))
        })
        .on_rule("literal", |_viewer, node| {
            ViewAction::Exact(grammar_literal_from_text(
                &node[1].text_normalized(),
                node.span(),
                false,
            ))
        })
        .on_rule("regexp", |_viewer, node| {
            ViewAction::Exact(grammar_regex_from_text(&node[1].text(), node.span(), false))
        })
}

fn repetition_node(
    expr: GrammarNode,
    sep: Option<GrammarNode>,
    min: usize,
    span: Span,
) -> GrammarNode {
    if let Some(separator) = sep {
        GrammarNode::SeparatedRepetition {
            node: Box::new(expr),
            separator: Box::new(separator),
            min,
            max: None,
            span,
        }
    } else {
        GrammarNode::Repetition {
            node: Box::new(expr),
            min,
            max: None,
            span,
        }
    }
}

fn grammar_token_from_text(token_name: &str, span: Span, raw: bool) -> GrammarNode {
    fn mk(raw: bool, matcher: impl Matcher + Send + Sync + 'static, span: Span) -> GrammarNode {
        let matcher: Arc<dyn Matcher + Send + Sync> = if raw {
            Arc::new(matcher)
        } else {
            Arc::new(words::token(matcher))
        };
        GrammarNode::Terminal(matcher, span)
    }
    match token_name {
        "IDENT" => mk(raw, IDENT, span),
        "STRING" => mk(raw, STRING, span),
        "NUMBER" => mk(raw, NUMS, span),
        "ALPHANUMS" => mk(raw, words::ALPHANUMS, span),
        "ALPHABETS" => mk(raw, words::ALPHAS, span),
        "EOF" => mk(raw, EndOfInput, span),
        t => panic!("Unsupported token type: {}", t),
    }
}

fn grammar_literal_from_text(text: &str, span: Span, raw: bool) -> GrammarNode {
    let text = text.trim();
    let text = Box::leak(text.to_string().into_boxed_str()) as &'static str;
    let matcher: Arc<dyn Matcher + Send + Sync> = if raw {
        Arc::new(text)
    } else {
        Arc::new(words::token(text))
    };
    GrammarNode::Terminal(matcher, span)
}

fn grammar_regex_from_text(pattern: &str, span: Span, raw: bool) -> GrammarNode {
    let normalized = pattern.replace("\\/", "/");
    let matcher = words::regex(&normalized);
    let matcher: Arc<dyn Matcher + Send + Sync> = if raw {
        Arc::new(matcher)
    } else {
        Arc::new(words::token(matcher))
    };
    GrammarNode::Terminal(matcher, span)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parsec::Parser;

    #[test]
    fn test_dsl_grammar() {
        let grammar = GRAMMAX_DSL_GRAMMAR_PROTOTYPE.with(|g| *g);
        grammar
            .save_to("/home/dexer/repos/grammax/grammars/grammax.gmx.bin")
            .unwrap();
        let mut parser = Parser::new(grammar);

        println!("Grammar:\n{}", parser.grammar.table);

        let text = r#"
start    -> /\/+/
"#;

        let result = parser.parse_text(text);

        let output = result.format_ast();

        println!("AST {}:\n{}", text, output);
        println!("Messages:\n{}", result.format_messages());

        let result = translate_dsl_grammar(result);
        match result {
            Ok(translated_grammar) => {
                println!("Translated Grammar:\n{}", translated_grammar.table);
                let result = translated_grammar.parse(r#"///"#);
                println!("Parsed AST:\n{}", result.format_ast());
            }
            Err(e) => {
                println!("Error translating grammar: {:?}", e);
            }
        }
    }
}
