# Interactive

A Grammax compiler becomes **interactive** when a built tree is wrapped in a runtime service and exposed through an interface. The runtime drives the event loop. The interface decides what requests the outside world may make.

This separation is important:

- the tree describes the compiler;
- the runtime hosts the compiler;
- the interface describes the public control surface.

## Runtime Service

You build a runtime from a fully composed tree:

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

`build_runtime::<I>(grammar)` wraps the typed tree into `RuntimeService<Tree, I>`, where `I` is your chosen interface. The service owns the runtime dispatcher and exposes the interface methods directly.

Predefined interfaces include:

- `BasicInterface<Tree>` for direct source-text editing and simple queries;
- `CliInterface<Tree>` for terminal-oriented inspection of parse results;
- `WebPreviewInterface<Tree>` for browser-facing CST previews.

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

The `GlobalEventDispatcher` is the runtime-owned request channel. Interfaces do not construct it themselves. They receive it from `RuntimeService` and use the helper methods on `Interface<Tree>` to issue typed requests safely.

Because the tree shape is part of the type system, interfaces can declare exactly which layers they require. For example:

```rust
impl<Tree: TypedTree> Interface<Tree> for WebPreviewInterface<Tree>
where
    Tree: ContainsPath<Here, Target = SourceText>
        + ContainsPath<Down<Here>, Target = ParseTreeIR>,
```

This means the interface will only compile for trees that contain `SourceText` at `Here` and `ParseTreeIR` one step below it.

## Querying a Layer

The central helper is `query_layer::<Path>(revision, index)`.

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

This example shows the exact current CST query surface:

- parser messages are queried as `ParseTreeQuery::Message(uri)`;
- the allocator is queried as `ParseTreeQuery::Allocator`;
- tree structure is queried as `ParseTreeQuery::Path(DocumentNodePath)` and returned as `ParseTreeValue::View`.

The optional revision parameter lets the interface query a specific accepted revision instead of the latest available state.

## Editing Source Text

For source editing, the two most important helpers are:

- `edit_source_text(uri, start, end, text)`;
- `edit_source_text_till::<Path>(uri, start, end, text)`.

The first sends an edit and returns the accepted revision. The second also waits for the chosen downstream layer to emit its transaction for that revision.

This is especially useful in request/response frontends:

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

Here `Down<Here>` means: apply the text edit at the source layer, then wait until the parse-tree layer emits the corresponding CST transaction.

## Writing a Good Custom Interface

The cleanest interfaces in Grammax usually follow a simple pattern:

- keep only frontend-facing state in the interface struct;
- express layer requirements through `ContainsPath` bounds;
- use `query_layer()` for typed reads;
- use `edit_source_text()` or `edit_source_text_till()` for writes;
- translate runtime errors into the protocol your frontend actually speaks.

If your frontend needs raw layer streaming rather than request/response access, keep the `LayerObserver`s produced during tree composition and run them beside the runtime service. The runtime and the observers are meant to coexist.