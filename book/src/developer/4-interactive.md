# Interactive

A Grammax compiler becomes **interactive** when a built tree is wrapped in a runtime service and exposed through an interface.

The split is simple:

- the tree describes the compiler itself;
- the runtime hosts that compiler;
- the interface defines the operations available to the outside world.

## Runtime Service

You create a runtime from a fully composed tree:

```rust
use grammax::interface::BasicInterface;
use grammax::runtime::compiler::Build;
use grammax::scheme::layers::ParseTreeIR;
use grammax::scheme::passes::ParserPass;

let runtime = Build::new().then(
    ParserPass::new(grammar),
    ParseTreeIR::default(),
    |build, _cst_observer| build.build_runtime::<BasicInterface<_>>(grammar),
);
```

`build_runtime::<I>(grammar)` wraps the typed tree into `RuntimeService<Tree, I>`, where `I` is the chosen interface type.

The service owns the runtime dispatcher. The interface methods become the public API you use.

Predefined interfaces include:

- `BasicInterface<Tree>` for simple editing and querying;
- `CliInterface<Tree>` for terminal-oriented inspection;
- `WebPreviewInterface<Tree>` for browser-facing CST preview workflows.

## The `Interface` Trait

Custom interfaces implement `Interface<Tree>`:

```rust
pub trait Interface<Tree: TypedTree> {
    fn new(ged: GlobalEventDispatcher, grammar: &'static grammar::Grammar) -> Self
    where
        Self: Sized;

    fn ged(&self) -> &GlobalEventDispatcher;

    // helper methods omitted
}
```

The `GlobalEventDispatcher` is created by the runtime. Interfaces do not build it themselves.

Instead, an interface receives the dispatcher and uses the helper methods on `Interface<Tree>` to send typed requests safely.

Because the tree shape is encoded in the type system, an interface can state exactly which layers it needs. For example:

```rust
impl<Tree: TypedTree> Interface<Tree> for WebPreviewInterface<Tree>
where
    Tree: ContainsPath<Here, Target = SourceText>
        + ContainsPath<Down<Here>, Target = ParseTreeIR>,
```

This means the interface only compiles for trees that contain:

- `SourceText` at `Here`;
- `ParseTreeIR` at `Down<Here>`.

## Querying a Layer

The main helper is `query_layer::<Path>(revision, index)`.

Example:

```rust
let messages = match self.query_layer::<Down<Here>>(
    Some(rev),
    ParseTreeQuery::Message(self.uri()),
)? {
    ParseTreeValue::Messages(messages) => messages,
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
    ParseTreeValue::Allocator(alloc) => alloc,
    other => {
        return Err(runtime::RuntimeError::UndefinedBehavior {
            message: format!("expected Allocator, got {other:?}"),
        });
    }
};

let root_view = match self.query_layer::<Down<Here>>(
    Some(rev),
    ParseTreeQuery::Path(DocumentNodePath::root(self.uri())),
)? {
    ParseTreeValue::View(view) => view,
    other => {
        return Err(runtime::RuntimeError::UndefinedBehavior {
            message: format!("expected View, got {other:?}"),
        });
    }
};
```

This shows the current CST query surface clearly:

- `ParseTreeQuery::Message(uri)` returns parser messages;
- `ParseTreeQuery::Allocator` returns the allocator;
- `ParseTreeQuery::Path(DocumentNodePath)` returns a node view.

The optional revision parameter lets the interface ask for a specific accepted revision instead of “whatever is current right now.”

## Editing Source Text

For source edits, the two most useful helpers are:

- `edit_source_text(uri, start, end, text)`;
- `edit_source_text_till::<Path>(uri, start, end, text)`.

The first sends the edit and returns the accepted revision.

The second sends the edit and then waits until a chosen downstream layer emits the transaction for that same revision.

That is especially useful in request/response frontends:

```rust
match body {
    WebAction::ApplyTextEdit { span, text } => self
        .edit_source_text_till::<Down<Here>>(&uri, span.start, span.end, &text)
        .map(|(_, transaction)| {
            rouille::Response::json(&commands_to_web_json(&transaction))
        })
        .unwrap_or_else(|err| rouille::Response::json(&err).with_status_code(500)),
    _ => todo!(),
}
```

Here `Down<Here>` means: apply the edit at the source layer, then wait until the parse-tree layer emits the corresponding CST transaction.

## Writing a Good Custom Interface

In practice, a clean custom interface usually follows a small set of rules:

- keep only frontend-facing state in the interface struct;
- express required layers through `ContainsPath` bounds;
- use `query_layer()` for typed reads;
- use `edit_source_text()` or `edit_source_text_till()` for writes;
- translate runtime errors into the protocol your frontend actually speaks.

If your frontend needs streaming updates instead of request/response behavior, keep the `LayerObserver`s produced during tree construction and run them beside the runtime service. The runtime API and raw observers are meant to work together.