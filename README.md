# Grammax 

Next generation solution for a neat language server. 

# Progress 

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
- [ ] Stabilize error recovery
- [ ] Pretty print for errors
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
- [ ] Simple reactive framework 
- [ ] Language server protocol integration

## Documentation
- [ ] Inline documentation for public APIs
- [ ] Examples for public APIs
- [ ] Mdbook documentation for the design and implementation details
... 
