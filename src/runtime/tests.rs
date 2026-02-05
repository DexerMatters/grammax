use std::{
    sync::{
        Arc,
        atomic::{self, AtomicIsize, AtomicUsize},
    },
    thread,
};

use crate::{
    new_grammar,
    parsec::{fmt::Display, parser::ParserConfig, words::*},
    runtime::{Interactive, Listener},
    utils::Span,
};

#[test]
fn test_example() {
    let grammar = new_grammar!(
        json where
        json    -> r!(object) | r!(array) | r!(string) | r!(number) | r!(boolean) | r!(null)
        object  -> tt("{") + sep(r!(pair), tt(",")) + tt("}")
        pair    -> r!(string) + tt(":") + r!(json)
        array   -> tt("[") + sep(r!(json), tt(",")) + tt("]")
        string  -> tt("\"") + t(STRING) + tt("\"")
        number  -> tt(NUMS)
        boolean -> tt("true") | tt("false")
        null    -> tt("null")
    );
    let listener = Listener::new().with_on_updated(|result| {
        eprintln!("Updated source text:\n{}", result.source_text);
        eprintln!(
            "Updated parse tree:\n{}",
            result.current_tree.display(&result.current_parser)
        );
        eprintln!(
            "Reparsed tree:\n{}",
            result.reparsed_tree.display(&result.current_parser)
        );
        eprintln!("Offset: {}", result.current_tree.offset);
        eprintln!("Messages:");
        eprintln!("{}", result.messages.display(&result.current_parser));
    });
    let time = Arc::new(atomic::AtomicIsize::new(0));
    let time_clone = time.clone();

    let comp_counter = Arc::new(AtomicUsize::new(0));
    let comp_clone = comp_counter.clone();
    let comp_clone2 = comp_counter.clone();

    let reuse_counter = Arc::new(AtomicUsize::new(0));
    let reuse_clone = reuse_counter.clone();
    let reuse_clone2 = reuse_counter.clone();

    let memo_counter = Arc::new(AtomicUsize::new(0));
    let memo_clone = memo_counter.clone();
    let memo_clone2 = memo_counter.clone();

    let config = ParserConfig::recovering()
        .with_on_computation_hook(move |_parser| {
            comp_counter.fetch_add(1, atomic::Ordering::SeqCst);
        })
        .with_on_node_reuse_hook(move |_parser| {
            reuse_counter.fetch_add(1, atomic::Ordering::SeqCst);
        })
        .with_on_memo_hit_hook(move |_parser| {
            memo_counter.fetch_add(1, atomic::Ordering::SeqCst);
        })
        .with_on_start_parse_hook(move |_parser| {
            comp_clone.store(0, atomic::Ordering::SeqCst);
            reuse_clone.store(0, atomic::Ordering::SeqCst);
            memo_clone.store(0, atomic::Ordering::SeqCst);
            time_clone.store(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_millis() as isize,
                atomic::Ordering::SeqCst,
            );
        })
        .with_on_finish_parse_hook(move |_parser| {
            let comp_count = comp_clone2.load(atomic::Ordering::SeqCst);
            let reuse_count = reuse_clone2.load(atomic::Ordering::SeqCst);
            let memo_count = memo_clone2.load(atomic::Ordering::SeqCst);
            eprintln!(
                "Computations: {} | Node reuse: {} | Memo: {} | Efficiency: {:.1}%",
                comp_count,
                reuse_count,
                memo_count,
                if comp_count + reuse_count > 0 {
                    (reuse_count as f64 / (comp_count + reuse_count) as f64) * 100.0
                } else {
                    0.0
                }
            );
            let end_time = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as isize;
            let start_time = time.load(atomic::Ordering::SeqCst);
            eprintln!("Time taken: {} ms", end_time - start_time);
        })
        .with_memo(1000);

    let runtime = Interactive::new_with_listener(grammar, config, listener);
    runtime.run().unwrap();
    runtime.insert(0, r#"{"name": "ok"}"#.to_string()).unwrap();

    runtime.insert(13, r#", "age": 21"#.to_string()).unwrap();

    runtime
        .update(Span::new(21, 24), "null".to_string())
        .unwrap();

    thread::sleep(std::time::Duration::from_millis(100));
}
