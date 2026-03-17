# Grammar

In Grammax, a grammar is defined as a set of rules that describe how to parse a language. Each rule consists of a name and a definition, where the name is an identifier and the definition is an expression that describes how to match the rule. The grammar is used by the parser to understand the structure of the input text and build the corresponding parse tree.

In the following chapters, we will explore how to design and write grammars using Grammax step by step. We will start with the basic building blocks of grammars, such as words and rules, and then move on to more advanced features of the DSL.

- [Words](./3-1-words.md) are the basic building blocks of terminals in a grammar. They can be defined using matchers, regular expressions, or string literals. We will learn how to use words to define the lexical structure of a language.
- [Rules](./3-2-rules.md) are the core components of a grammar. They define how to combine terminals and other rules to form the syntax of a language. We will learn how to write rules using Rust codes.
- [DSL](./3-3-dsl.md) is a domain-specific language provided by Grammax for writing grammars. It allows you to define rules, terminals, and combinations in a concise and readable way. We will learn how to use the DSL to write grammars efficiently.
