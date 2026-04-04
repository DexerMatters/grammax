use std::{
    collections::HashMap,
    fs,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use crate::{
    interface::{BasicInterface, Interface},
    new_grammar, new_grammar_no_cache,
    parsec::Parser,
    runtime::{Build, Down, Here},
    scheme::{
        self, IR, LayerObserver, LazyResult, Span, URI,
        layers::{AstArena, DocumentNodePath, ParseTreeIR, ParseTreeQuery, ParseTreeValue},
        passes::{IncrementalLowerer, ParserPass, reparser::Reparser},
    },
};

#[derive(Debug, Clone, PartialEq, Eq)]
enum LazyNumberFault {
    UnknownStaging(usize),
}

#[derive(Debug, Clone, Default)]
struct LazyNumberIR {
    values: HashMap<URI, i64>,
    staging: HashMap<usize, i64>,
}

impl scheme::IR for LazyNumberIR {
    type Ix = URI;
    type Value = i64;
    type Fault = LazyNumberFault;

    fn query(&self, index: URI) -> LazyResult<i64, LazyNumberFault> {
        match self.values.get(&index).copied() {
            Some(v) => LazyResult::Present(v),
            None => LazyResult::Absent,
        }
    }

    fn apply_transaction(&mut self, txn: scheme::Transaction<Self>) -> Result<(), LazyNumberFault> {
        self.staging.clear();
        for cmd in txn.iter() {
            match cmd {
                scheme::Command::Create { id, value } => {
                    self.staging.insert(*id, *value);
                }
                scheme::Command::Insert { index, id } | scheme::Command::Replace { index, id } => {
                    let value = *self
                        .staging
                        .get(id)
                        .ok_or(LazyNumberFault::UnknownStaging(*id))?;
                    self.values.insert(*index, value);
                }
                scheme::Command::Delete { index } => {
                    self.values.remove(index);
                }
            }
        }
        Ok(())
    }
}

#[test]
fn test_arith_reparser() {
    let grammar = new_grammar_no_cache!(
        start where
        start -> r!(expr) + tt(EndOfInput)
        expr -> r!(add) | r!(mul) | r!(primary)
        add  -> r!(expr) + tt("+") + r!(expr).drop(1)
        mul  -> r!(expr).drop(1) + tt("*") + r!(expr).drop(2)
        primary -> tt(NUMBER) | tt("(") + r!(expr) + tt(")")
    );

    let parser = Parser::new(grammar);
    let mut reparser = Reparser::from_parser(parser);
    reparser.insert(0, "1 + 2 * 3").unwrap();
    println!("{}", reparser.current_view());

    reparser.delete(0, 3).unwrap();
    println!("{}", reparser.current_view());
}

#[test]
fn test_json() {
    let grammar = new_grammar!(
        json where
        json    -> r!(object) | r!(array) | r!(string) | r!(number) | r!(boolean) | r!(null)
        object  -> tt("{") + sep(r!(pair), tt(",")) + tt("}")
        pair    -> field("key", r!(string)) + tt(":") + field("value", r!(json))
        array   -> tt("[") + sep(r!(json), tt(",")) + tt("]")
        string  -> tt("\"") + t(STRING) + t("\"")
        number  -> t(NUMBER)
        boolean -> tt("true") | tt("false")
        null    -> tt("null")
    );

    let compiler = Build::new().then(
        ParserPass::new(grammar),
        ParseTreeIR::with_grammar(grammar),
        |b, _cst_obs| {
            b.then(
                IncrementalLowerer::new(grammar, ()),
                AstArena::default(),
                |b, _ast_obs| b.build_runtime::<BasicInterface<_>>(grammar),
            )
        },
    );

    compiler.insert(0, r#"{"name": "John"}"#).expect("submit");
    compiler.replace(10, 14, "Doe").expect("update");
}

#[test]
fn test_tap_prints_cst_commands() {
    let grammar = new_grammar!(
        json where
        json    -> r!(object) | r!(array) | r!(string) | r!(number) | r!(boolean) | r!(null)
        object  -> tt("{") + sep(r!(pair), tt(",")) + tt("}")
        pair    -> field("key", r!(string)) + tt(":") + field("value", r!(json))
        array   -> tt("[") + sep(r!(json), tt(",")) + tt("]")
        string  -> tt("\"") + t(STRING) + tt("\"")
        number  -> tt(NUMBER)
        boolean -> tt("true") | tt("false")
        null    -> tt("null")
    );

    let (compiler, cst_obs) = Build::new().then(
        ParserPass::new(grammar),
        ParseTreeIR::with_grammar(grammar),
        |b, obs| (b.build_runtime::<BasicInterface<_>>(grammar), obs),
    );

    compiler.insert(0, r#"{"name": "John"}"#).expect("submit");

    // The observer receives one update per submitted transaction.
    let (revision, txn) = cst_obs
        .updates
        .recv_timeout(Duration::from_millis(500))
        .expect("observer timed out waiting for update");

    println!("=== revision {revision} — {} CST command(s) ===", txn.len());
    for cmd in txn.iter() {
        println!("  {cmd:?}");
    }
}

#[test]
fn test_lazy_query_loads_file_from_disk() {
    let grammar = new_grammar_no_cache!(
        start where
        start -> r!(expr) + tt(EndOfInput)
        expr -> r!(add) | r!(primary)
        add  -> r!(expr) + tt("+") + r!(expr).drop(1)
        primary -> tt(NUMBER)
    );

    let compiler = Build::new().then(
        ParserPass::new(grammar),
        ParseTreeIR::with_grammar(grammar),
        |b, _cst_obs| b.build_runtime::<BasicInterface<_>>(grammar),
    );

    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before unix epoch")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("grammax-lazy-{stamp}.txt"));
    fs::write(&path, "12+3").expect("write temp source file");

    let uri = URI::new("file", path.to_string_lossy());

    let lazy_root = compiler
        .query_layer::<Down<Here>>(None, ParseTreeQuery::Path(DocumentNodePath::root(uri)))
        .expect("lazy parse query should load the file and build CST");
    match lazy_root {
        ParseTreeValue::View(view) => {
            let rendered = format!("{view}");
            assert!(rendered.contains("12"), "unexpected CST: {rendered}");
            assert!(rendered.contains("+"), "unexpected CST: {rendered}");
        }
        other => panic!("expected parse tree view, got {other:?}"),
    }

    let source = compiler
        .query_source_text(None, &uri, Span::new(0, usize::MAX))
        .expect("source query should observe the lazy-loaded file");
    assert_eq!(source.as_ref().as_str(), "12+3");

    let revision = compiler
        .edit_source_text(&uri, 0, 2, "7")
        .expect("editing a lazy-loaded file should work");
    let updated = compiler
        .query_layer::<Down<Here>>(
            Some(revision),
            ParseTreeQuery::Path(DocumentNodePath::root(uri)),
        )
        .expect("updated CST query should succeed after lazy load");

    match updated {
        ParseTreeValue::View(view) => {
            let rendered = format!("{view}");
            assert!(rendered.contains("7"), "unexpected updated CST: {rendered}");
            assert!(
                !rendered.contains("12"),
                "unexpected updated CST: {rendered}"
            );
        }
        other => panic!("expected parse tree view after edit, got {other:?}"),
    }

    let _ = fs::remove_file(path);
}
