# Pipeline

Grammax executes each stage of a compiler as a concurrent pipeline node. Each node owns one downstream layer, receives upstream transactions, applies its pass, and serves queries against its current state.

That concurrency is intentionally hidden behind typed builders and observers. As a developer, you compose a tree, store or move the observers you care about, and then hand the final tree to the runtime.

## Building a Linear Pipeline

The public builder is `Build::new()`. It starts from a root `SourceText` layer and extends the tree one stage at a time.

The API is continuation-based, but the most useful way to *think* about it is monadic: each composition step takes the current typed tree, attaches one more stage, and passes the enriched tree to the next step.

In other words, `then(...)` behaves like a typed bind:

- it consumes the current builder;
- it produces a larger builder;
- it also yields the observer for the newly created layer;
- the continuation decides what to do next with that enriched context.

The surface syntax is Rust closures, but the semantic model is close to a monadic pipeline construction where every step receives the accumulated structure and continues from there.

The core shape is:

```rust
Build::new().then(pass, seed, |build, observer| {
    // `build` is the extended tree
    // `observer` watches the newly added downstream layer
})
```

This shape is deliberate. Every time you add a stage, Grammax gives you two things immediately:

- the new typed builder;
- a `LayerObserver` for the layer you just created.

That means observation is part of composition, not an afterthought. In monadic terms, the observer is extra context returned together with the new pipeline state.

For example:

```text
SourceText
  |
  +--P1--> A
            |
            +--P2--> B
                      |
                      +--P3--> C
```

```rust
pipeline = Build::new()
    .then(P1, A, |pipeline, a| {
        // Observer `a` is introduced at the same step that creates `A`.
        pipeline.then(P2, B, |pipeline, b| {
            // The next step can use both the accumulated pipeline and observer `b`.
            pipeline.then(P3, C, |pipeline, c| {
                // At this point `a`, `b`, and `c` can be stored, moved into tasks,
                // or passed into the runtime setup.
                pipeline
            })
        })
    })
```

Here `P1`, `P2`, and `P3` are passes, while `A`, `B`, and `C` are the downstream layers they create.

Read it as a sequence of dependent steps:

1. start from the source layer;
2. bind `P1` to create layer `A` and obtain observer `a`;
3. bind `P2` to create layer `B` and obtain observer `b`;
4. bind `P3` to create layer `C` and obtain observer `c`;
5. return the final pipeline.

The important part is that each step can see what earlier steps produced. You do not separately build a tree and later rediscover its handles. The handles are introduced exactly where the stage is introduced.

A typical frontend pipeline looks like this:

```rust
Build::new().then(ParserPass::new(grammar), ParseTreeIR::default(), |build, cst_observer| {
    build.then(
        IncrementalLowerer::new(grammar, mapper),
        AstArena::<AstMapAny>::default(),
        |build, ast_observer| {
            let _ = cst_observer;
            let _ = ast_observer;
            build
        },
    )
})
```

The resulting tree is still rooted at `SourceText`, but now contains two derived layers beneath it.

## Branching a Pipeline

The branching API is `fanout(...)`.

Conceptually, `fanout(...)` duplicates the current layer into two independent builder continuations. Each continuation describes a complete branch starting from the same upstream state.

In that sense, a branched pipeline is still built monadically, but the bound value is copied into two branch-local continuations instead of being threaded into just one.

The exact generic type becomes large quickly, which is normal. The important part is conceptual:

- each branch is typed independently;
- each branch receives its own observer;
- path types such as `Down<P>` and `Another<P>` describe how runtime code reaches those branches later.

For example:

```text
SourceText
  |
  +--P1--> A
            |
            +--P2--> B
            |         |
            |         +--P4--> D
            |
            +--P3--> C
```

```rust
pipeline = Build::new()
    .then(P1, A, |pipeline, a| {
        pipeline.fanout(
            |left_branch| {
                left_branch.then(P2, B, |left_branch, b| {
                // This continuation can only extend the left branch.
                    left_branch.then(P4, D, |left_branch, d| {
                        // `a`, `b`, and `d` are available on the left path.
                        left_branch
                    })
                })
            },
            |right_branch| {
                right_branch.then(P3, C, |right_branch, c| {
                    // The right branch receives `c` and cannot accidentally extend the left side.
                    right_branch
                })
            },
        )
    })
```

This should be read as:

1. build the common prefix once with `P1` producing `A`;
2. split that prefix into two typed branches with `fanout(...)`;
3. build the left branch as `P2 -> B -> P4 -> D`;
4. build the right branch as `P3 -> C`;
5. store or route the observers that matter to your interface or tooling.

The builder API may look unusual at first, but this is exactly why it scales: each closure receives only the branch it is allowed to extend, so impossible compositions become unrepresentable.

## Layer Observation

`LayerObserver<Repr>` is the public handle for a live layer in a built tree. You usually obtain it from the builder continuation and move it into whichever task needs to watch that layer.

An observer does two jobs:

- receive transactions emitted by the layer;
- query the current state of the layer.

Printing every CST transaction is straightforward:

```rust
std::thread::spawn(move || {
    while let Some((revision, transaction)) = cst_observer.recv_update() {
        println!("revision = {revision}");
        for cmd in transaction.iter() {
            println!("{cmd:?}");
        }
    }
});
```

If you do not care about revisions, `recv()` and `try_recv()` strip the revision number and return only the transaction.

## Querying Through an Observer

Observers can also query their layer directly:

```rust
let result = cst_observer.query(ParseTreeQuery::Path(DocumentNodePath::root(uri)));
match result {
    Ok(ParseTreeValue::View(view)) => {
        println!("root node = {view}");
    }
    Ok(other) => {
        panic!("expected ParseTreeValue::View, got {other:?}");
    }
    Err(err) => {
        panic!("query failed: {err:?}");
    }
}
```

Two query modes are available:

- `query(index)` performs normal lazy resolution. If the layer reports `Absent`, the pipeline may call `pull()` on the pass and retry.
- `query_strict(index)` skips that extra demand step and reports `ObserveError::Absent` immediately when the entry is missing.

This is the practical meaning of lazy resources in Grammax: the observer asks for a value, the layer answers if it already has it, and the pass gets exactly one chance to materialize it on demand.

## Path Types in Runtime Code

When a tree is later wrapped by the runtime, paths are described with the same type-level vocabulary used by `ContainsPath`:

- `Here` means the current layer;
- `Down<P>` means descend into the left child and continue with `P`;
- `Another<P>` means descend into the right child and continue with `P`.

For a linear pipeline `SourceText -> ParseTreeIR -> AstArena`, the useful paths are:

- `Here` for `SourceText`;
- `Down<Here>` for `ParseTreeIR`;
- `Down<Down<Here>>` for `AstArena`.

For a forked tree, `Another<P>` addresses the right branch.

These paths are more than documentation. They are the static proof that a runtime interface is querying a layer that actually exists.


