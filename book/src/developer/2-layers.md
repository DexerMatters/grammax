# Layers

Grammax treats a frontend as a stack of **layers** connected by **passes**. A layer is a queryable, mutable database. A pass is the rule that keeps one database derived from another.

That division is strict:

- layers own state;
- passes derive state;
- transactions move downward;
- queries move upward.

This is the core of the terraced-field design. A lower layer never mutates an upper layer directly, and an upper layer never reaches into a lower layer to "fix" it. Coordination happens only through transactions and lazy queries.

```text
SourceText
    ↓ push
ParseTreeIR
    ↓ push
AstArena

AstArena query
    ↑ pull
ParseTreeIR query
    ↑ pull
SourceText query
```

## IR

In Grammax, an IR is the contract for one layer.

```rust
pub trait IR {
    type Ix;
    type Value;
    type Fault;

    fn query(&self, index: Self::Ix) -> LazyResult<Self::Value, Self::Fault>;

    fn apply_transaction(&mut self, transaction: Transaction<Self>) -> Result<(), Self::Fault>
    where
        Self: Sized;
}

pub enum LazyResult<V, F> {
    Present(V),
    Absent,
    Fault(F),
}
```

This API deliberately separates three states that many systems collapse into one loose "error":

- `Present(V)` means the layer can answer the query now.
- `Absent` means the requested index is not populated yet.
- `Fault(F)` means the index is meaningful, but the request is invalid or permanently impossible for domain reasons.

That distinction matters because `Absent` is part of normal lazy evaluation. It is not a failure. It is a signal that a pass may be able to synthesize the missing entry. By contrast, `Fault(F)` is terminal for that query.

This is why `IR::Fault` is intentionally narrow: it should contain only **permanent domain faults**. Anything transient belongs in the observation layer, not inside the IR value space.

For example:

- `SourceText` can be `Absent` for a URI that has not been loaded yet.
- `ParseTreeIR` can be `Absent` for a path that has not been parsed yet.
- `AstArena` can return `Fault(TypeMismatch { .. })` if the caller asks for a node as the wrong Rust type.

## Observation Errors

Once a layer is observed through a pipeline, the caller no longer sees only raw `LazyResult`. It sees runtime conditions as well.

```rust
pub enum ObserveError<F> {
    NotReady,
    Disconnected,
    Absent,
    Fault(F),
    Exhausted,
}
```

These variants have precise meaning:

- `NotReady` means the query channel has not been wired yet.
- `Disconnected` means the pipeline is gone.
- `Absent` means the layer still has no value after the pipeline tried to help.
- `Fault(F)` forwards a permanent domain fault from the layer.
- `Exhausted` means the pass explicitly gave up: it knows the requested index will never be produced.

The helper `is_resolvable()` is the official test for transient states. Today that means `NotReady`, `Disconnected`, and `Absent`.

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

The important invariant is that `Insert` and `Replace` refer only to `id`s created in the same transaction. A transaction should be self-contained. That keeps application simple and deterministic, and it makes downstream observation meaningful because every emitted batch is a coherent state transition.

In practice, most transactions follow a two-phase shape:

1. Build new values with `Create`.
2. Attach them with `Insert`, `Replace`, or remove old entries with `Delete`.

For a source edit replacing `John` with `Doe`, a source-layer transaction might look like this:

```text
Create { id: 0, value: "Doe" }
Replace { index: DocumentSpan { .. }, id: 0 }
```

The parsing pass then turns that source transaction into a CST transaction that creates new syntax nodes and rewires the affected path in `ParseTreeIR`.

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

    fn pull(
        &mut self,
        upstream: &LayerObserver<U>,
        downstream: &D,
        index: D::Ix,
    ) -> PullOutcome<D>;
}

pub enum PullOutcome<Repr: IR> {
    Ready(Vec<Command<Repr>>),
    Pending,
    Dead,
}
```

The two methods are complementary, but they are not interchangeable.

### `push`

`push` reacts to an upstream transaction that already happened. It is eager, directional, and transaction-oriented. Its job is to derive the downstream delta caused by the upstream delta.

Typical `push` behavior:

- parse an edited source document into new CST commands;
- lower changed CST regions into AST commands;
- emit nothing when the upstream transaction does not affect the downstream layer.

### `pull`

`pull` reacts to a missing downstream query. It is lazy, demand-driven, and index-oriented. Its job is to answer the question: "what commands, if any, would populate this exact downstream index?"

The return values are intentionally strict:

- `Ready(cmds)` means the pass can populate the requested index now.
- `Pending` means the request is still unresolved and should be retried after future upstream activity.
- `Dead` means this pass will never be able to produce that index.

`Pending` is named from the state of the **pull request**, not from the direction of the dependency. It does not mean that background work is running. It means the request remains open because future upstream data may change the answer.

That leads to an important rule for pass authors:

- return `Pending` only when a future upstream transaction could legitimately make the index available;
- return `Dead` when the index is structurally impossible or permanently unsupported.

If you confuse those two, the runtime will keep retrying a request that should have been terminated.

## Implementation Guidance

When implementing a custom layer or pass, the following heuristics usually lead to the cleanest behavior:

- Make `query()` cheap and side-effect-free.
- Use `Absent` for missing data, not for malformed requests.
- Keep `Fault` small and domain-specific.
- Make `push()` derive only from the upstream transaction and observable upstream state.
- Make `pull()` produce the smallest coherent downstream transaction that answers the missing index.
- Prefer `Dead` over `Pending` unless you can point to a concrete future upstream event that may change the outcome.

The built-in `ParserPass` and `IncrementalLowerer` are good reference implementations of this model.

