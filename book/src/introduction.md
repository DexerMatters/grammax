# Introduction

**Grammax** is a state-of-art incremental runtime for general parsing tasks using context-free grammar. It is tailored for compiler front-end implementation and language server development. It significantly decreases the workload of designing and making a compiler while not sacrificing customization and versatility.

## Features

> [!Warning]
> In progress...

## Principles

>*A compiler should be like terraces.<br>Coding as the water source irrigates from top to bottom, <br>IRs the platforms storing the water, <br>Passes the dikes processing the flow.*

- **Incremental design**
Incremental design is adopted by more and more frameworks. Instead of computing everything, it smartly detects which group of units need to be recomputed and updates them partially. This makes users *fearless* to editing large projects while staying faithful to what you expect.

- **Command-based pipeline**
Besides the traditional way of incremental design by reusing cache, the runtime are stratified in a pipeline and communicates with commands, which contain the deltas from every update, propagating from top to bottom. This achieves the real *full increment* for Grammax.

- **Modularization**
Grammax has designed a series of friendly interfaces, allowing users to develop modules (such like IRs and passes) naturally within the principles.

## Contribution
> [!Warning]
> In progress...

## License

The Grammax source and documentation are released under [MIT License](https://opensource.org/license/mit).