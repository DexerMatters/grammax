# Parser

In Grammax, a **parser** is a component that takes an input string and produces a **parse tree** based on the grammar rules you defined. The parser is responsible for analyzing the structure of your input and classifying it according to the grammar. The resulting parse tree represents the hierarchical structure of the input, where each node corresponds to a rule in the grammar.

In the following chapters, we will explore how to use the parser to parse input text and work with the resulting parse tree. We will cover the following topics:

- [Basic usage](./4-1-basic.md): How to create a parser from a grammar and parse input text.
- [Parse tree](./4-2-tree.md): Understanding the structure of the parse tree, visualizing it, traversing it, and transforming it into what you need.
- [Reparser](./4-3-reparser.md): How to make edits to the input text and update the parse tree efficiently without re-parsing everything from scratch.