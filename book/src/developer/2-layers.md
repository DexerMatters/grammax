# Layers

Grammax treats a frontend as a stack of layers connected by passes.

The separation is strict and intentional:

- layers own state;
- passes derive new state from upstream transactions;
- transactions move downward;
- demand moves upward.

That is the heart of the terraced-fields model. A lower layer does not reach into an upper layer and mutate it directly. An upper layer does not patch a lower layer by hand. Instead, the runtime coordinates everything through queries, transactions, and lazy demand.

```text
SourceText
    ↓ push
ParseTreeIR
    ↓ push
AstArena

AstArena query
    ↑ demand
ParseTreeIR query
    ↑ demand
Root source resolution
```

## What A Layer Is

An `IR` is the contract for one layer.

The exact shape is small on purpose:

```rust
pub trait IR {
    type Ix;
    type Value;
    type Fault;

    fn query(&self, index: Self::Ix) -> LazyResult<Self::Value, Self::Fault>;

    fn apply(&mut self, transaction: Transaction<Self>) -> Result<(), Self::Fault>
    where
        Self: Sized;

    fn resolve(&mut self, index: Self::Ix) -> ResolveOutcome<Self>
    where
        Self: Sized;
}
```

There are only three things a layer needs to know how to do:

1. answer a query for one index;
2. apply a self-contained transaction;
3. optionally resolve a missing root index.

That keeps the layer focused. The layer does not need to know about the entire compiler. It only needs to know how to store and retrieve its own values.

## Query Results

`query()` returns a `LazyResult`:

```rust
pub enum LazyResult<V, F> {
    Present(V),
    Absent,
    Fault(F),
}
```

The three outcomes mean different things.

- `Present(V)` means the layer can answer right now.
- `Absent` means the value is missing for now, but the runtime may still be able to obtain it.
- `Fault(F)` means the request is permanently wrong for domain reasons.

That distinction matters.

`Absent` is part of normal lazy evaluation. It is not an error. It simply means the value is not currently stored.

`Fault(F)` is different. It means the caller asked a meaningful question, but the layer can never represent the answer correctly.

Good examples:

- `SourceText` can be `Absent` for a URI that has not been loaded yet;
- `ParseTreeIR` can be `Absent` for a path that has not been parsed yet;
- `AstArena` can return `Fault(...)` when the caller asks for a value through the wrong Rust type.

Because of that, `IR::Fault` should stay small and precise. Permanent domain problems belong there. Temporary runtime conditions do not.

## Root Resolution

`IR::resolve` is the escape hatch for a root layer.

Most layers leave the default behavior in place and return `Impossible`. That is fine. A layer only needs to implement `resolve` if it knows how to produce missing data from outside the pipeline.

The return type is:

```rust
pub enum ResolveOutcome<R: IR> {
    Done(Transaction<R>),
    Blocked,
    Impossible,
}
```

- `Done(txn)` means the layer produced a transaction that fills the missing entry;
- `Blocked` means the layer cannot finish yet, but a later retry might work;
- `Impossible` means this layer will never be able to resolve that index.

In the current frontend, the source layer itself is just a mutable store. If missing source text needs to come from disk, the runtime asks the interface to provide it. That keeps the storage layer simple and keeps file-loading policy outside the core data structure.

## Observation Errors

Once a layer is observed through a live pipeline, the caller sees `ObserveError` instead of raw `LazyResult`:

```rust
pub enum ObserveError<F> {
    NotReady,
    Disconnected,
    Absent,
    Fault(F),
    Impossible,
}
```

These cases mean:

- `NotReady` means the query channel is not wired yet;
- `Disconnected` means the pipeline is gone;
- `Absent` means the layer is still missing the value after lazy demand was tried;
- `Fault(F)` means the layer returned a permanent domain error;
- `Impossible` means the pipeline determined that the requested value can never be produced.

The helper `is_resolvable()` checks the transient cases: `NotReady`, `Disconnected`, and `Absent`.

## Transactions

A transaction is a closed batch of commands that updates one layer.

```rust
pub enum Command<Repr: IR> {
    Create { id: usize, value: Repr::Value },
    Insert { index: Repr::Ix, id: usize },
    Delete { index: Repr::Ix },
    Replace { index: Repr::Ix, id: usize },
}

pub type Transaction<Repr> = Arc<Vec<Command<Repr>>>;
```

The important invariant is that `Insert` and `Replace` may only refer to `id`s created earlier in the same transaction.

That makes each transaction self-contained. The layer can apply it without depending on hidden state elsewhere in the program.

In practice, transactions usually look like this:

1. create new values with `Create`;
2. attach them with `Insert` or `Replace`;
3. remove old entries with `Delete` when needed.

For example, replacing `John` with `Doe` in source text could be encoded as:

```text
Create { id: 0, value: "Doe" }
Replace { index: DocumentSpan { .. }, id: 0 }
```

The parsing pass then turns that source transaction into a CST transaction.

## Passes

A pass is the bridge between two layers.

```rust
pub trait Pass<U: IR, D: IR> {
    fn push(
        &mut self,
        upstream: &LayerObserver<U>,
        downstream: &D,
        txn: &[Command<U>],
    ) -> Vec<Command<D>>;
}
```

`push()` reacts to an upstream transaction that already happened. Its job is to compute the downstream transaction caused by that upstream change.

Common examples:

- parsing an edited source document into CST commands;
- lowering changed CST regions into AST commands;
- emitting nothing when an upstream change has no downstream effect.

Passes do not implement lazy demand directly. They only translate transactions.

## Demand

Lazy demand is declared on index types, not in pass code.

The trait is:

```rust
pub trait Demand<U: IR> {
    fn upstream_index(&self) -> Option<U::Ix>;
}
```

Implement this on a downstream index type `D::Ix` to say: "if this downstream entry is missing, which upstream entry should be demanded first?"

The pipeline calls `upstream_index()` automatically whenever a non-strict query sees `Absent`.

- Return `Some(u_ix)` if there is a meaningful upstream dependency.
- Return `None` if the index has no upstream dependency.

Examples of `None`:

- identity fan-out cases, where the pipeline is only forwarding data;
- self-contained queries such as `ParseTreeQuery::Allocator`.

### How Demand Propagates

When a query to layer `D` returns `Absent`, the pipeline does this:

1. call `index.upstream_index()`;
2. if it gets `Some(u_ix)`, synchronously query the upstream layer with that index;
3. continue recursively until some layer can answer;
4. if the chain reaches a root layer, let that root resolve the missing data;
5. when a transaction is finally produced, let it flow back down through normal `push()` calls;
6. re-check deferred queries as each layer updates.

This means demand can cross many layers, but the code that expresses the dependency stays local and simple: one `Demand<U>` implementation on the downstream index type.

## Built-In Layers

The standard frontend uses three layers that line up with the common compiler stages:

- `SourceText` stores editable source documents, applies text edits, and caches text snapshots;
- `ParseTreeIR` stores the lossless CST and parser messages;
- `AstArena` stores user-facing AST values derived from the CST.

These layers are intentionally boring. Their job is to hold state and answer queries, not to hide policy or orchestration inside their data structures.

## Implementation Guidance

When you implement a custom layer or pass, these rules usually lead to the cleanest design:

- make `query()` cheap and side-effect-free;
- use `Absent` for missing data, not malformed requests;
- keep `Fault` small and domain-specific;
- make `push()` derive only from the upstream transaction and observable upstream state;
- implement `Demand<U>` once per `(downstream index type, upstream layer)` pair;
- return `None` from `upstream_index()` when there is no meaningful upstream dependency;
- only implement `IR::resolve` on a root layer that can fetch data from outside the pipeline.

The built-in `ParserPass` and `IncrementalLowerer` are good reference implementations of this model.

