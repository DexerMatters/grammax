# Introduction

Grammax is a framework for building incremental compiler frontends as a stack of persistent layers. It follows the [terraced fields design](../design/0-terraces.md): edits flow downward as transactions, and missing data is fetched upward through lazy demand.

If you are new to the project, this part of the book is the best place to start. The goal is to explain the runtime model in plain terms so you can understand how the pieces fit together before you write your own layer, pass, or interface.

## The Big Picture

The core idea is simple:

- layers store state;
- passes transform transactions between layers;
- the pipeline moves transactions downward and demand upward;
- the runtime turns the whole tree into a live service.

That separation is what makes Grammax incremental. A parser pass does not reach out and mutate the AST directly. An AST layer does not repair source text on its own. Instead, each layer only knows how to answer queries about its own data and how to apply transactions that were already constructed for it.

## What This Part Covers

- `grammax::scheme` defines the core contracts: `IR`, `Pass`, `Command`, `Transaction`, `LazyResult`, `ObserveError`, `Demand`, and `Pipeline`.
- `grammax::scheme::layers` contains the built-in layers used by the standard frontend.
- `grammax::scheme::passes` contains the built-in passes that connect those layers.
- `grammax::runtime` and `grammax::interface` wrap a composed tree into an interactive service.

The standard frontend is usually arranged like this:

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

- `SourceText` stores editable document text.
- `ParseTreeIR` stores the lossless CST and parser messages.
- `AstArena` stores user-facing AST values derived from the CST.

Each pass also has one clear job:

- `ParserPass` turns source transactions into CST transactions.
- `IncrementalLowerer` turns CST transactions into AST transactions.

That is the whole architecture in one sentence: source changes become transactions, transactions flow downward, and missing data is pulled upward only when the runtime needs it.

## Lazy Loading

Lazy loading is not encoded inside individual passes. The pipeline handles it automatically.

- each downstream index type can declare which upstream index it depends on through `Demand<U>`;
- the root layer can use `IR::resolve` when it knows how to produce missing data itself;
- in the standard runtime, the interface can also provide root data for the source layer, such as loading a missing file-backed URI;
- once the missing data arrives, the normal `push()` path runs and fills the downstream layer.

That means lazy behavior stays declarative. A layer does not need to know who triggered the query, and a pass does not need to contain special-case loading logic. The dependency is written once, close to the index type that actually needs it.

## How To Read The Rest Of This Guide

The next chapters explain the system in this order:

- [layers](./2-layers.md) explains the `IR` contract, transactions, demand, and the built-in layers;
- [pipeline](./3-pipeline.md) explains how the builder wires layers together and how queries travel through the tree;
- [interactive](./4-interactive.md) explains how a composed tree becomes a runtime service with an interface on top.

If you keep the four nouns below in mind, the rest of the book becomes much easier to follow:

- layer = owns data;
- pass = translates transactions;
- pipeline = propagates transactions and demand;
- interface = talks to the outside world.


