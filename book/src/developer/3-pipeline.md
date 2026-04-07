# Pipeline

Grammax runs each stage of a compiler as a concurrent pipeline node. Each node owns one downstream layer, receives upstream transactions, applies its pass, and answers queries against its current state.

You do not have to manage that concurrency manually. The builder API and the observer API hide most of it behind ordinary Rust types.

## Building A Linear Pipeline

The public entry point is `Build::new()`. It starts with a root `SourceText` layer.

You extend the pipeline one stage at a time with `then(...)`:

```rust
Build::new().then(pass, seed, |build, observer| {
    // `build` is the larger pipeline
    // `observer` watches the new downstream layer
})
```

Each `then(...)` call does two things at once:

- add one new downstream layer;
- give you a `LayerObserver` for that new layer.

So the normal way to read the builder is:

1. add a stage;
2. optionally keep its observer;
3. continue building from there.

If you know monadic APIs, `then(...)` behaves like a typed bind. If you do not, just read it as "attach one more stage and continue."

Example:

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
let pipeline = Build::new()
    .then(P1, A, |pipeline, a| {
        pipeline.then(P2, B, |pipeline, b| {
            pipeline.then(P3, C, |pipeline, c| {
                let _ = a;
                let _ = b;
                let _ = c;
                pipeline
            })
        })
    });
```

Read that example from top to bottom:

1. start from `SourceText`;
2. attach `P1` to create layer `A`;
3. attach `P2` to create layer `B`;
4. attach `P3` to create layer `C`;
5. keep any observers you care about;
6. return the final typed tree.

A typical frontend looks like this:

```rust
Build::new().then(ParserPass::new(grammar), ParseTreeIR::with_grammar(grammar), |build, cst_observer| {
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

That pipeline is still rooted at `SourceText`, but it now has CST and AST layers beneath it.

## Branching A Pipeline

Use `fanout(...)` when one layer should feed two independent branches.

Conceptually, `fanout(...)` duplicates the current layer into two builder continuations. Each branch can then grow on its own.

The important rules are:

- each branch is typed independently;
- each branch gets its own observers;
- later runtime code reaches branches through path types such as `Down<P>` and `Another<P>`.

Example:

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
let pipeline = Build::new()
    .then(P1, A, |pipeline, a| {
        pipeline.fanout(
            |left| {
                left.then(P2, B, |left, b| {
                    left.then(P4, D, |left, d| {
                        let _ = a;
                        let _ = b;
                        let _ = d;
                        left
                    })
                })
            },
            |right| {
                right.then(P3, C, |right, c| {
                    let _ = c;
                    right
                })
            },
        )
    });
```

The key benefit is safety: each closure only receives the branch it is allowed to extend.

## Layer Observation

`LayerObserver<Repr>` is the public handle for a live layer.

You usually get it during pipeline construction and move it into whichever task needs it.

An observer does two jobs:

- receive transactions emitted by the layer;
- query the current state of the layer.

Example: printing every CST transaction.

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

If the revision number does not matter, `recv()` and `try_recv()` return only the transaction.

## Querying Through An Observer

Observers can also query the current state directly:

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

There are two query modes:

- `query(index)` performs normal lazy demand.
- `query_strict(index)` skips that extra demand step and returns `ObserveError::Absent` immediately if the entry is missing.

Normal lazy demand works like this:

1. the layer sees `Absent`;
2. the pipeline asks the index for its upstream dependency through `Demand<U>`;
3. the upstream query runs synchronously;
4. if needed, the root layer resolves the missing data;
5. the resulting transaction flows back down through normal `push()` calls;
6. the original reply is completed when the requested value finally appears.

So the observer API still feels simple even though the pipeline may be doing multi-layer lazy propagation behind the scenes.

## Runtime Paths

When a tree is wrapped by the runtime, paths are described with the same type-level vocabulary used by `ContainsPath`:

- `Here` means the current layer;
- `Down<P>` means go into the left child, then continue with `P`;
- `Another<P>` means go into the right child, then continue with `P`.

For a linear pipeline `SourceText -> ParseTreeIR -> AstArena`, the common paths are:

- `Here` for `SourceText`;
- `Down<Here>` for `ParseTreeIR`;
- `Down<Down<Here>>` for `AstArena`.

For a forked tree, `Another<P>` addresses the right branch.

These path types are not just documentation. They are the static proof that the layer you want to query really exists in that tree.


