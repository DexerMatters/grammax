# Pipeline

> *Pipelining is a commonly used concept in everyday life. For example, in the assembly line of a car factory, each specific task—such as installing the engine, installing the hood, and installing the wheels—is often done by a separate work station. The stations carry out their tasks in parallel, each on a different car. Once a car has had one task performed, it moves to the next station. Variations in the time needed to complete the tasks can be accommodated by "buffering" (holding one or more cars in a space between the stations) and/or by "stalling" (temporarily halting the upstream stations), until the next station becomes available.*
>
> *Suppose that assembling one car requires three tasks that take 20, 10, and 15 minutes, respectively. Then, if all three tasks were performed by a single station, the factory would output one car every 45 minutes. By using a pipeline of three stations, the factory would output the first car in 45 minutes, and then a new one every 20 minutes.*
>
> *As this example shows, pipelining does not decrease the latency, that is, the total time for one item to go through the whole system. It does however increase the system's throughput, that is, the rate at which new items are processed after the first one.*  -- Wikipedia

In Grammax, we adopt a command-based pipeline design. Each layer of the compiler is connected through passes that transform commands from one layer to another. Whenever source code is updated, compilation doesn't wait for the previous code to finish compiling before processing current code. Instead, it continuously reads updates and outputs compiled code in a steady stream. In practice, however, compilers using this technique are rare. Most compilers typically apply this technique to the front-end only—code parsing, semantic analysis, macro expansion, etc.—while rarely using it in the back-end (optimization and machine code generation).

In this chapter, we will introduce the design of the command-based pipeline in Grammax, including how to compose a compiler with the pipeline and how to observe the commands flowing through the pipeline. After understanding the pipeline, you will be able to build your own compiler pipeline using Grammax's scheme and observe its internal workings in real time.

## A composed compiler

Pipelines are transparent to developers. You don't need to worry about how to design your compiler in a pipelined way. 

You can normally instance your compiler with `CompilerBuilder` like this:

```rust
let tree = CompilerBuilder::new()
    .then(ParserPass::new(grammar), ParseTreeIR::default())
    .then(IncrementalLowerer::new(grammar, mapper), AstArena::default());
```

`CompilerBuilder::new()` creates a tree whose current layer is the source text. Each call to `then(pass, ir)` adds one more stage below the current leaf and returns a new typed tree. In the example above, the resulting pipeline is:

```text
SourceText
  |
ParseTreeIR
  |
AstArena
```

This means the source text is still the root of the tree, `ParseTreeIR` is one step below it, and `AstArena` is one step below `ParseTreeIR`.

## Composing larger trees

Sequential pipelines are the most common case, but the builder is not limited to a single chain. You can also fork the tree and keep composing on one branch.

For example, the following code builds this shape:

```text
SourceText
  |
IR1
  |
IR2
 / \
IR3 IR4
 |
IR5
```

```rust
let tree = CompilerBuilder::new()
    .then(Pass1::new(), IR1::default())
    .then(Pass2::new(), IR2::default())
    .map_left(|ir1_branch| {
        ir1_branch.map_left(|ir2_leaf| {
            ir2_leaf.fork(
                Pass3::new(),
                IR3::default(),
                Pass4::new(),
                IR4::default(),
            )
        })
    })
    .map_left(|ir1_branch| {
        ir1_branch.map_left(|ir2_branch| {
            ir2_branch.map_left(|ir3_leaf| ir3_leaf.then(Pass5::new(), IR5::default()))
        })
    });
```

The key idea is that `.then()` only extends a plain leaf, while `.map_left()` and `.map_right()` let you reopen an existing branch and keep composing inside it.

As a rule of thumb:

- use `then(pass, ir)` to extend a linear pipeline;
- use `fork(left_pass, left_ir, right_pass, right_ir)` to split one leaf into two branches;
- use `map_left()` or `map_right()` to keep composing inside an already-built branch.

## Observing the pipeline

You can observe any layer in the tree by calling `observe::<Path>()`. The path is a type-level description of how to walk from the root layer to the layer you want:

- `Here` means the current layer;
- `Down<P>` means go to the left child, then continue with `P`;
- `Another<P>` means go to the right child, then continue with `P`.

For the sequential compiler shown above, observation looks like this:

```rust
let tree = CompilerBuilder::new()
    .then(ParserPass::new(grammar), ParseTreeIR::default())
    .then(IncrementalLowerer::new(grammar, mapper), AstArena::default());

let source_observer = tree.observe::<Here>();
let parse_tree_observer = tree.observe::<Down<Here>>();
let ast_observer = tree.observe::<Down<Down<Here>>>();
```

The returned value is a `LayerObserver`. You can use it to receive transactions and to query the current state of that layer.

For example, you can print out the commands from `ParseTreeIR` in real time like this:

```rust
let parse_tree_observer = tree.observe::<Down<Here>>();

thread::spawn(move || {
    while let Some(transaction) = parse_tree_observer.recv() {
        println!("======Received transaction======");
        for cmd in transaction.iter() {
             println!("{:?}", cmd);
        }
    }
});
```

By using `recv()`, the observer acts like a receiver of a channel, waiting for transactions to arrive and printing them out in real time. You can also query the current state of the layer by using `query()`:

```rust
let parse_tree_observer = tree.observe::<Down<Here>>();

let result = parse_tree_observer.query(ParseTreeQuery::Path(NodePath::root()));
match result {
    Ok(ParseTreeValue::GreenId(id)) => {
        println!("GreenId at root: {:?}", id);
    }
    Ok(other) => {
        panic!("expected GreenId, got {other:?}");
    }
    Err(err) => {
        panic!("Runtime query failed: {err:?}");
    }
}
```

The `query()` method returns a nested `Result` so that runtime failures and IR-specific failures are both preserved. This is intentionally verbose: querying a live pipeline can fail because the runtime is unavailable, because the query handle is not installed yet, or because the query itself is invalid for that IR.

For the branched example above, some useful paths are:

- `Here`: `SourceText`
- `Down<Here>`: `IR1`
- `Down<Down<Here>>`: `IR2`
- `Down<Down<Down<Here>>>`: `IR3`
- `Another<Down<Down<Here>>>`: `IR4`
- `Down<Down<Down<Down<Here>>>>`: `IR5`

Once you are comfortable reading these path types, the whole compiler tree becomes statically navigable at compile time.


