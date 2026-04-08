# Interactive

A Grammax compiler becomes interactive when a composed tree is wrapped in a runtime service and exposed through an interface.

That split keeps the core compiler logic independent from the way a user talks to it.

- the tree describes the compiler itself;
- the runtime owns the live worker and dispatcher;
- the interface defines the operations available to the outside world.

## Building A Runtime

The runtime starts from a composed tree:

```rust
use grammax::interface::BasicInterface;
use grammax::runtime::compiler::Build;
use grammax::scheme::layers::ParseTreeIR;
use grammax::scheme::passes::ParserPass;

let runtime = Build::new().then(
    ParserPass::new(grammar),
    ParseTreeIR::with_grammar(grammar),
    |build, _cst_observer| build.build_runtime::<BasicInterface<_>>(grammar),
);
```

`build_runtime::<I>(grammar)` wraps the typed tree into `RuntimeService<Tree, I>`, where `I` is the chosen interface type.

The runtime service owns the `GlobalEventDispatcher`. The interface methods become the public API that the frontend uses.

The built-in interfaces in the repository are:

- `BasicInterface<Tree>` for simple editing and querying;
- `CliInterface<Tree>` for terminal-oriented inspection;
- `WebPreviewInterface<Tree>` for browser-facing CST preview workflows;
- `LanguageServerInterface<Tree, I>` for the LSP-based frontend used by the `vsclsp` feature.

## The Interface Trait

Every interface implements `Interface<Tree>`:

```rust
pub trait Interface<Tree: TypedTree> {
    fn new(ged: GlobalEventDispatcher, grammar: &'static grammar::Grammar) -> Self
    where
        Self: Sized;

    fn ged(&self) -> &GlobalEventDispatcher;

    // helper methods omitted
}
```

The runtime creates the dispatcher. The interface does not build it itself.

Instead, the interface receives the dispatcher and uses the helper methods on `Interface<Tree>` to send typed requests safely.

Because the tree shape is encoded in the type system, an interface can state exactly which layers it needs. For example:

```rust
impl<Tree: TypedTree> Interface<Tree> for WebPreviewInterface<Tree>
where
    Tree: ContainsPath<Here, Target = SourceText>
        + ContainsPath<Down<Here>, Target = ParseTreeIR>,
```

That means the interface only compiles for trees that contain:

- `SourceText` at `Here`;
- `ParseTreeIR` at `Down<Here>`.

## Querying A Layer

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

This is the current CST query surface:

- `ParseTreeQuery::Message(uri)` returns parser messages;
- `ParseTreeQuery::Allocator` returns the allocator;
- `ParseTreeQuery::Path(DocumentNodePath)` returns a node view.

The optional revision parameter lets the interface ask for a specific accepted revision instead of "whatever is current right now."

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

## Root Source Resolution

The runtime can also ask the interface to resolve missing source data.

This is how source text enters the system when the source layer has not seen a URI yet. The current LSP interface keeps the latest text it receives from `did_open`, so `resolve_source` can serve an open document even if that URI is not loaded locally yet. If the URI was never opened in the editor, it falls back to `fetch_text()` for file-backed documents on disk.

That keeps source loading out of `SourceText` itself. The layer remains a plain text store, while the interface decides where text comes from and can choose between live editor content and the filesystem.

## Writing A Good Custom Interface

In practice, a clean custom interface usually follows a small set of rules:

- keep only frontend-facing state in the interface struct;
- express required layers through `ContainsPath` bounds;
- use `query_layer()` for typed reads;
- use `edit_source_text()` or `edit_source_text_till()` for writes;
- provide a source-resolution hook only if the frontend owns how source documents are loaded;
- translate runtime errors into the protocol your frontend actually speaks.

If your frontend needs streaming updates instead of request/response behavior, keep the `LayerObserver`s produced during tree construction and run them beside the runtime service. The runtime API and raw observers are meant to work together.