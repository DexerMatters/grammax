# Introduction

**Grammax** is a state-of-art incremental runtime for general parsing tasks using context-free grammar. It is tailored for compiler front-end implementation and language server development. It significantly decreases the workload of designing and making a compiler while not sacrificing customization and versatility.

## Features

> [!WARNING]
> In progress...

## Principles

>*A compiler should be like terraces.<br>Coding as the water source irrigates from top to bottom, <br>IRs the platforms storing the water, <br>Passes the dikes processing the flow.*

- **Incremental design**
Incremental design is adopted by more and more frameworks. Instead of computing everything, it smartly detects which group of units need to be recomputed and updates them partially. This makes users *fearless* to editing large projects while staying faithful to what you expect.

- **Command-based pipeline**
Besides the traditional way of incremental design by reusing cache, the runtime are stratified in a pipeline and communicates with commands, which contain the deltas from every update, propagating from top to bottom. This achieves the real *full increment* for Grammax.

- **First-class Laziness**
Laziness is a powerful tool for incremental design, but it is often an afterthought. Grammax makes it a first-class citizen: every layer can lazily populate missing entries on demand, and every pass can lazily react to missing downstream entries. This allows Grammax to handle large projects without sacrificing responsiveness.

- **Modularization**
Grammax has designed a series of friendly interfaces, allowing users to develop modules (such like IRs and passes) naturally within the principles.

## This book

This book is a comprehensive guide for users and developers of Grammax. It also shares the design and implementation details of Grammax for those who are interested in the internals of the framework. The book is divided into three main sections: User Guide, Developer Guide, and Design and Implementation.

- [The user guide](./user/1-installation.md) provides a step-by-step tutorial for users to get started with Grammax, including installation, a tour of the framework, designing grammar, using the parser and compiler, and interactive features.
After reading the user guide, you will be able to design your own grammar, build a parser and basic compiler using Grammax, and use the interactive features to test your grammar.

- [The developer guide](./developer/1-introduction.md) introduces the design of the command-based pipeline in Grammax, including how to compose a compiler with IRs and passes, how to observe the commands flowing through the pipeline, and how to interact with the compiler through interfaces. After reading the developer guide, you will be able to design IRs and passes, compose your own compiler pipeline and interact with it with your custom interface. These all are under the incremental design and command-based pipeline scheme of Grammax.

- [The design and implementation](./design/0-terraces.md) shares the design and implementation details of Grammax, focusing on "how it works". It unmasks the design of the "terraced field" structure and makes better understanding of the achievement of the incremental design and command-based pipeline. After reading this section, you will have a deeper understanding of the design and implementation of Grammax, which can help you to use it more effectively and even contribute to its development.

## Contribution
> [!WARNING]
> In progress...

## License

The Grammax source and documentation are released under [MIT License](https://opensource.org/license/mit).