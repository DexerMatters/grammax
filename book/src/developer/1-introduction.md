# Introduction

**Grammax** is a framework for building incremental compiler frontends as a stack of persistent layers. It follows the ["terraced fields" design](../design/0-terraces.md): edits flow downward as transactions, and missing data is fetched upward through lazy demand.

This part of the book explains how that system works internally. The goal is to make the runtime model easy to follow, so you can build your own layers, passes, and interfaces without guessing how the pieces fit together.

## What This Part Covers

- `grammax::scheme` defines the core contracts: `IR`, `Pass`, `Command`, `Transaction`, `LazyResult`, `ObserveError`, `Demand`, and `Pipeline`.
- `grammax::scheme::layers` contains the built-in layers used by the standard frontend.
- `grammax::scheme::passes` contains the built-in passes that connect those layers.
- `grammax::runtime` and `grammax::interface` wrap a composed tree into an interactive service.

The standard frontend looks like this:

```text
SourceText
    |
ParserPass
    v
ParseTreeIR
    |
IncrementalLowerer
    v
AstArena
```

Each layer has one clear job:

- **SourceText** stores editable document text.
- **ParseTreeIR** stores the lossless CST and parser messages.
- **AstArena** stores user-facing AST values derived from the CST.

Each pass also has one clear job:

- **ParserPass** turns source transactions into CST transactions.
- **IncrementalLowerer** turns CST transactions into AST transactions.

Lazy loading is not implemented inside those passes. Instead, the pipeline handles it automatically:

- each index type can declare which upstream index it depends on through `Demand<U>`;
- the root layer can override `IR::resolve` to fetch missing external data;
- once the missing upstream data arrives, the normal `push()` path runs and fills the downstream layer.

So when you read the rest of this guide, keep this model in mind:

- layers store data;
- passes translate transactions;
- the pipeline propagates demand;
- the runtime exposes the whole thing as a live service.

The following chapters explain those responsibilities one by one.


