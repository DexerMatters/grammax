# Grammax 

Next generation solution for a neat language server. 

# Progress 

## Basic

- [x] Basic grammar DSL
- [x] Design normalized grammar 
- [x] Normalize repetitive and optional rules
- [x] Extract sync tokens and delimiters
- [x] Derive bridge parsing specs
- [x] Red-green tree
- [x] Store and load processed grammar and analysis results 
- [x] Pretty print for AST
- [x] Basic logic of the LR parser
- [x] Temporal LRU cache for node reusing 
- [x] Coarse-grained error recovery
- [x] Fine-grained error recovery (missing tokens)
- [x] Error recovery with scope analysis
- [x] Stabilize error recovery
- [x] Pretty print for errors
- [ ] Warnings for preferred alternatives
- [x] Concurrent runtime platform
- [ ] Fully configurable runtime behaviors 
- [x] Channel communication with the main thread
- [x] Faster node cache invalidation
- [x] Basic ascending-based reparsing
- [x] Strategy-based reparsing
- [ ] Further localize reparsing range
- [ ] Stabilize reparsing
- [ ] Perfect parser listeners
- [ ] Perfect runtime listeners
- [x] Basic interactive platform
- [x] IR ready for semantic analysis of red-green AST
- [ ] Specified meta information on AST nodes
- [x] Simple reactive framework 
- [x] Language server protocol integration

## Framework
- [ ] Scope graph layer for type checkers

## LSP features
- [ ] Semantic highlighting
- [ ] Code completion

## Documentation
- [ ] Inline documentation for public APIs
- [ ] Examples for public APIs
- [ ] Mdbook documentation for the design and implementation details
... 
