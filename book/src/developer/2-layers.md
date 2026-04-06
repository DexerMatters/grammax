# Layers

Grammax treats a frontend as a stack of **layers** connected by **passes**.

A layer is a queryable, mutable database. A pass is the rule that keeps one layer derived from the layer above it.

That separation is strict:

- layers own state;
- passes derive state;
- transactions move downward;
- demand moves upward.

This is the core of the terraced-field design. A lower layer does not mutate an upper layer directly, and an upper layer does not reach downward to repair a lower layer. Coordination happens only through transactions, queries, and lazy demand.

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
SourceText resolve
```

## IR

An `IR` is the contract for one layer.

```rust
pub trait IR {
    type Ix;
    type Value;
    type Fault;

    fn query(&self, index: Self::Ix) -> LazyResult<Self::Value, Self::Fault>;

    fn apply_transaction(&mut self, transaction: Transaction<Self>) -> Result<(), Self::Fault>
    where
        Self: Sized;

    // Default returns Impossible. Root layers can override this.
    fn resolve(&mut self, index: Self::Ix) -> ResolveOutcome<Self>
    where
        Self: Sized;
}

pub enum LazyResult<V, F> {
    Present(V),
    Absent,
    Fault(F),
}
```

The important idea is that `query()` has three different outcomes:

- `Present(V)` means the layer can answer right now.
- `Absent` means the value is missing for now, but the pipeline may still be able to obtain it.
- `Fault(F)` means the request is invalid or permanently impossible for domain reasons.

This distinction matters a lot.

`Absent` is part of normal lazy evaluation. It is not a bug. It simply means the requested entry is not currently stored.

`Fault(F)` is different. It means the caller asked a meaningful question in the wrong way, or asked for something the layer can never represent correctly.

That is why `IR::Fault` should stay small and precise. It is for permanent domain problems only. Transient runtime conditions belong somewhere else.

Examples:

- `SourceText` can be `Absent` for a URI that has not been loaded yet.
- `ParseTreeIR` can be `Absent` for a path that has not been parsed yet.
- `AstArena` can return `Fault(...)` when the caller asks for a value as the wrong Rust type.

### `resolve`

`IR::resolve` is the escape hatch for a root layer.

Most layers do not implement it. They just keep the default result: `Impossible`.

A root layer can override it to fetch missing data from outside the pipeline. For example, `SourceText` uses `resolve` to read a file from disk when a missing `file://` URI is queried.

The return type is:

```rust
pub enum ResolveOutcome<R: IR> {
    Done(Transaction<R>),
    Blocked,
    Impossible,
}
```

- `Done(txn)` means the layer produced a transaction that fills the missing entry.
- `Blocked` means the layer cannot finish yet, but a later retry might work.
- `Impossible` means this layer will never be able to resolve that index.

## Observation Errors

Once a layer is observed through a live pipeline, the caller does not see raw `LazyResult` directly. It sees runtime conditions as well.

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

- `NotReady`: the query channel is not wired yet.
- `Disconnected`: the pipeline is gone.
- `Absent`: the value is still missing after the pipeline tried normal lazy demand.
- `Fault(F)`: the layer returned a permanent domain error.
- `Impossible`: the pipeline determined that the requested value can never be produced.

The helper `is_resolvable()` is the official check for transient cases. Today that means `NotReady`, `Disconnected`, and `Absent`.

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

The key invariant is simple: `Insert` and `Replace` may only refer to `id`s created earlier in the same transaction.

That makes every transaction self-contained. A layer can apply it without depending on hidden external state.

In practice, most transactions look like this:

1. create new values with `Create`;
2. attach them with `Insert` or `Replace`;
3. remove old entries with `Delete` when needed.

For example, a source edit replacing `John` with `Doe` could be encoded as:

```text
Create { id: 0, value: "Doe" }
Replace { index: DocumentSpan { .. }, id: 0 }
```

The parsing pass then translates that source transaction into a CST transaction.

## Pass

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

Typical examples:

- parse an edited source document into CST commands;
- lower changed CST regions into AST commands;
- emit nothing if the upstream change has no downstream effect.

Passes do not implement lazy demand directly. They only translate transactions.

## Demand

Lazy demand is declared on index types, not in pass code.

The trait is:

```rust
pub trait Demand<U: IR> {
    fn upstream_index(&self) -> Option<U::Ix>;
}
```

Implement this on a downstream index type `D::Ix` to say: “if this downstream entry is missing, which upstream entry should be demanded first?”

The pipeline calls `upstream_index()` automatically whenever a non-strict query sees `Absent`.

- Return `Some(u_ix)` if there is a meaningful upstream dependency.
- Return `None` if the index has no upstream dependency.

Examples of `None`:

- identity fan-out cases, where the pipeline is only forwarding data;
- self-contained queries such as `ParseTreeQuery::Allocator`.

### How demand propagates

When a query to layer `D` returns `Absent`, the pipeline does this:

1. call `index.upstream_index()`;
2. if it gets `Some(u_ix)`, synchronously query the upstream layer with that index;
3. let the same process continue recursively until some layer can answer;
4. if the chain reaches a root layer, call `IR::resolve` there;
5. when a transaction is finally produced, let it flow back down through normal `push()` calls;
6. re-check deferred queries as each layer updates.

This means demand can cross many layers, but the code that expresses the dependency is still local and simple: one `Demand<U>` implementation on the downstream index type.

## Implementation Guidance

When implementing a custom layer or pass, these rules usually lead to the cleanest design:

- Make `query()` cheap and side-effect-free.
- Use `Absent` for missing data, not malformed requests.
- Keep `Fault` small and domain-specific.
- Make `push()` derive only from the upstream transaction and observable upstream state.
- Implement `Demand<U>` once per `(downstream index type, upstream layer)` pair.
- Return `None` from `upstream_index()` when there is no meaningful upstream dependency.
- Only implement `IR::resolve` on a root layer that can fetch data from outside the pipeline.

The built-in `ParserPass` and `IncrementalLowerer` are good reference implementations of this model.

