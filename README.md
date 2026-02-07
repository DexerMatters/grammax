# Grammax 

Next generation solution for a neat language server. 

# Progress 

- [x] Basic grammar DSL
- [x] Design normalized grammar 
- [x] Normalize repetitive and optional rules
- [x] Extract sync tokens and delimiters
- [x] Derive bridge parsing specs
- [x] Left recursion elimination 
- [x] Shrunken alternative branches by merging common terms
- [x] Red-green tree
- [ ] Store and load processed grammar and analysis results 
- [x] Pretty print for AST
- [x] Basic logic of the LL parser
- [x] Memoize look-ahead probing
- [ ] Leading-word optimization for parsing alternatives
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
- [ ] Faster node cache invalidation
- [x] Basic ascending-based reparsing
- [x] Strategy-based reparsing
- [ ] Further localize reparsing range
- [ ] Stabilize reparsing
- [ ] Perfect parser listeners
- [ ] Perfect runtime listeners
- [ ] Basic interactive platform
- [ ] IR ready for semantic analysis of red-green AST
- [ ] Specified meta information on AST nodes
- [ ] Simple reactive framework 
... 
