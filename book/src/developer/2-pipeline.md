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
CompilerBuilder::new()
    .then_pass(ParserPass::new(grammar))
    .then_layer(ParseTreeIR::default())
    .then_pass(IncrementalLowerer::new(grammar, mapper))
    .then_layer(AstArena::default())
```

where `new` is the default layer for the source text; `then_pass` and `then_layer` are just methods for sequentially adding passes and layers. The pipeline is automatically set up afterwards.

## Observing the pipeline

**Tap** is adapted from the concept of "tap" in Unix pipelines. It allows you to observe the commands flowing through the pipeline just like setting a tap on a water pipe. You can set up a tap on after any layer to query its current state and observe the commands flowing from it.

You can instance a tap like this:

```rust
let (pass, observer) = CompilerBuilder::new()
    .then_pass(ParserPass::new(grammar))
    .then_layer(ParseTreeIR::default())
    .tap();
```

where `pass` is an instance of `ExpectPass` from which you can build the rest of the compiler, and `observer` is an instance of `LayerObserver` which you can use to observe the commands flowing from the layer and query its current state. For example, you can print out the commands from `ParseTreeIR` in real time like this:

```rust
thread::spawn(move || {
    while let Some(transaction) = observer.recv() {
        println!("======Received transaction======");
        for cmd in transaction.iter() {
             println!("{:?}", cmd);
        }
    }
});
```

By using `recv` method, the observer acts like a receiver of a channel, waiting for transactions to arrive and printing them out in real time. You can also query the current state of the layer by using the `query` method:

```rust
thread::spawn(move || {
    while let Some(transaction) = observer.recv() {
        println!("======Current tree root======");
        let result = observer.query(ParseTreeQuery::Path(NodePath::root()));
        let result = result.expect("Runtime query failed");
        let tree = result.expect("Query is bad for this layer");
        let green_id = tree
            .downcast_ref::<GreenId>()
            .expect("Value is not a GreenId");
        println!("GreenId at root: {:?}", green_id);
    }
});
```

It looks verbose because the `query` does not miss any possible error and returns a `Result<Result<_, _>, _>` object. The first wrapper is for runtime errors, which may occur when the query handle is not set or the query fails for some runtime reason. The second wrapper is for errors provided by the queried layer, which may occur when the query is bad (e.g., asking for a non-existent value with out-of-range index). You can unwrap the two layers of `Result` to get the value you want, which is a type-erased `GreenId` in this case. (Note that the value is type-erased because the layer may return any type of value. You can not only query the root of the tree but also the allocator or parser messages, which are specified in `ParseTreeQuery`.)


