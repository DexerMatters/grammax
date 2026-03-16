# Interactive

We say a compiler is **interactive** when it can continuously handle updates from the world outside and output what we want by observing it.

In this chapter, we will introduce how to make your compiler interactive. We will analyze the existing interfaces defined in Grammax (`BasicInterface`, `WebPreviewInterface`, and `CliInterface`) and learn how to implement an interface for your own frontend. After understanding the interactive design, you will be able to make your compiler interactive and build your own interface for it.

## Runtime service

**Runtime service** is a wrapper around the compiler that continuously observes the world outside, receives updates from it, feeds them into the compiler, and outputs the results back to the caller. 

```rust
let tree = CompilerBuilder::new()
    .then(ParserPass::new(grammar), ParseTreeIR::default());

let runtime = tree.build_runtime::<WebPreviewInterface<_>>(grammar);

runtime.run().expect("Runtime failed unexpectedly");
```

The compiler is built first as a typed tree. Then `build_runtime::<InterfaceType>(grammar)` wraps that tree into a `RuntimeService<Tree, InterfaceType>`. The interface type is generic over the concrete tree type, so `WebPreviewInterface<_>` means “build a web preview interface for this exact tree”.

`RuntimeService` owns the background event loop and dereferences to the chosen interface implementation, so you call interface methods directly on the runtime value.

Different interfaces provide different APIs through the service. For example,

```rust
let tree = CompilerBuilder::new()
    .then(ParserPass::new(grammar), ParseTreeIR::default());

let runtime = tree.build_runtime::<BasicInterface<_>>(grammar);

runtime.insert(0, "1 + 2 * 3").unwrap();
runtime.replace(0, 1, "4").unwrap();
```

Here we use `BasicInterface<Tree>`, which provides simple APIs for inserting and updating the source text. We can call `insert()` and `replace()` on the runtime service to send updates to the compiler, which will then process them and output the results back to the caller.

## Custom Interface

**Interface** is a trait that offers necessary methods for developers to implement their own interfaces. It defines how the compiler interacts with the outside world and what APIs it provides to the caller.

```rust
pub trait Interface<Tree: TypedTree> {
    fn new(ged: GlobalEventDispatcher, grammar: &'static grammar::Grammar) -> Self
    where
        Self: Sized;
    fn ged(&self) -> &GlobalEventDispatcher;

    ...
}
```

To implement an interface, your struct usually needs to hold a `GlobalEventDispatcher` and any other frontend state you need, such as a static reference to the grammar. You are not allowed to instantiate a global event dispatcher directly. `new()` is usually called by `RuntimeService`, which provides the dispatcher to the interface.

There are multiple defined methods in the `Interface` trait to assist you to interact with the compiler. For example, the method `edit_source_text` allows you to send updates to the compiler by specifying the range of the source text to be updated and the new text. The method `query_layer` allows you to query the current state of a layer in the pipeline by sending a query and receiving a response.

Since locating the layer is statically determined by the type-level path, the interface has to be parameterized by the tree type to specify what layers it can accessThe predefined interfaces in Grammax are also tree-parameterized:

- `BasicInterface<Tree>` only requires that `Tree` contains `SourceText` at `Here`;
- `CliInterface<Tree>` requires `SourceText` at `Here` and `ParseTreeIR` at `Down<Here>`;
- `WebPreviewInterface<Tree>` has the same requirement as `CliInterface<Tree>`.

Taking `WebPreviewInterface<Tree>` as an example, the implementation of `Interface<Tree>` for it looks like this:

```rust
impl<Tree: TypedTree> Interface<Tree> for WebPreviewInterface<Tree>
where
    Tree: ContainsPath<Here, Target = SourceText>
        + ContainsPath<Down<Here>, Target = ParseTreeIR>,
```

Here `ContainsPath<Here, Target = SourceText>` means that the tree must contain a layer of `SourceText` at `Here`, and `ContainsPath<Down<Here>, Target = ParseTreeIR>` means that the tree must contain a layer of `ParseTreeIR` at `Down<Here>`. This design lets each interface declare exactly which layers it needs from the compiler tree to radically avoid runtime errors caused by missing layers. 

This design lets each interface declare exactly which layers it needs from the compiler tree.

### Querying the pipeline

The code below is adapted from the implementation of `CliInterface<Tree>` to show how to use `query_layer()` to query the current state of a layer in the pipeline.

```rust
let messages = match self.query_layer::<Down<Here>>(
        Some(rev),
        ParseTreeQuery::Message,
    )? {
        ParseTreeValue::Messages(m) => m,
        other => {
            return Err(runtime::RuntimeError::UndefinedBehavior {
                message: format!("expected Messages, got {other:?}"),
            });
        }
    };

let alloc = match self.query_layer::<Down<Here>>(
        Some(rev),
        ParseTreeQuery::Allocator,
    )? {
        ParseTreeValue::Allocator(a) => a,
        other => {
            return Err(runtime::RuntimeError::UndefinedBehavior {
                message: format!("expected Allocator, got {other:?}"),
            });
        }
    };

let root_id = match self.query_layer::<Down<Here>>(
        Some(rev),
        ParseTreeQuery::Path(NodePath::root()),
    )? {
        ParseTreeValue::GreenId(id) => id,
        other => {
            return Err(runtime::RuntimeError::UndefinedBehavior {
                message: format!("expected GreenId, got {other:?}"),
            });
        }
    };
```

`query_layer::<Path>(...)` requires you to specify a type-level path to the layer you want to query, an index of the layer and an optional revision ID. The path is the same as the one used in `observe::<Path>()`.

In the standard interactive tree, `Here` is the source text and `Down<Here>` is the parse tree layer, so `query_layer::<Down<Here>>(...)` means “query `ParseTreeIR`”.

- `query_layer::<Here>(...)` queries the source text layer;
- `query_layer::<Down<Here>>(...)` queries the layer directly below the source text;
- `query_layer::<Down<Down<Here>>>(...)` queries the next layer below that.

**Revision** is a concept related to the state of the layer. Each update of the source text will trigger a new revision. The revision ID is a monotonically increasing number that represents the order of the revisions. By specifying a revision ID, you can query the state of the layer after a specific update.

### Editing the source text

There are two methods for editing the source text: `edit_source_text(...)` and `edit_source_text_till::<Path>(...)`. They both send updates to the compiler and return a revision ID, but the latter also returns the transaction emitted by a chosen layer after the update. This is useful when your frontend wants the updated layer output immediately.

The code below is adapted from `WebPreviewInterface<Tree>`:

```rust
match body {
    WebAction::ApplyTextEdit { span, text } => this
        .edit_source_text_till::<Down<Here>>(span.start, span.end, &text)
        .map(|(_, transaction)| {
            rouille::Response::json(&commands_to_web_json(&transaction))
        })
        .unwrap_or_else(|e| rouille::Response::json(&e).with_status_code(500)),
    ...
}
```

Here `Down<Here>` means that the web frontend wants the transaction emitted by the parse tree layer after the text edit, which consists of commands that update the parse tree according to the new source text. The returned transaction is then converted to JSON and sent back to the caller.