# Introduction

**Grammax** is a framework for building incremental compiler frontends as a stack of persistent layers. It follows the ["terraced fields" design](../design/0-terraces.md): updates flow downward as transactions, while queries flow upward as lazy requests. The result is not a single monolithic compiler pass, but a composed system of small databases that can react to edits and answer questions at any layer.

This developer guide describes that system from the inside. The goal is not only to show which APIs exist, but to make the intended semantics precise enough that you can implement your own layers, passes, and interfaces without guessing.

## What This Part Covers

- `grammax::scheme` defines the core contracts: `IR`, `Pass`, `Command`, `Transaction`, `LazyResult`, `ObserveError`, `Pipeline`, and the predefined layer types.
- `grammax::scheme::layers` contains the built-in layers used by the default frontend stack.
- `grammax::scheme::passes` contains the built-in passes that connect those layers.
- `grammax::runtime` and `grammax::interface` wrap a composed tree into an interactive service.

The standard frontend shipped with Grammax is organized like this:

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

Each layer has a narrow responsibility:

- **SourceText** stores editable document text.
- **ParseTreeIR** stores the lossless CST and parser messages.
- **AstArena** stores the user-facing abstract tree derived from the CST.

Each pass also has a narrow responsibility:

- **ParserPass** translates source transactions into CST transactions and can lazily populate missing CST entries.
- **IncrementalLowerer** translates CST changes into AST changes and can lazily populate missing AST entries.

The chapters that follow explain how those responsibilities are expressed in the type system and in the runtime behavior.


