# Rules

Grammar is a set of **rules** that define how to match input strings and build the semantic structure. Each rule consists of a name and a **grammar node**, which is a combination of matchers and references to other rules. 

In Grammax, rules are defined using functions that return a `GrammarNode`. The function name is the rule name, and the returned `GrammarNode` defines how to match that rule.

## Grammar nodes

Firstly, let's comprehend the usage of grammar nodes, which are the building blocks of rules. A grammar node can be a [matcher](./3-1-words.md), a reference to another rule, or a combination of both. Matchers are used to match specific patterns in the input string, while rule references allow us to build more complex structures by referring to other rules.

### Terminals

Terminals are the basic building blocks of grammar nodes. They are defined using matchers. There are two functions that help us create terminal grammar nodes from matchers:
- `t(matcher)`: Creates a terminal grammar node from the given matcher.
- `tt(matcher)`: Short for `t(token(matcher))`, which creates a terminal grammar node that matches the given matcher while consuming any leading whitespace and newlines.

The following example defines a rule named `color` that matches a hexadecimal color code:

```rust
fn color() -> GrammarNode {
    t("0x".then(HEX_DIGIT.times(6)))
}
```

### Combinations
Grammar nodes can be composed using combinators to create more complex patterns. In Grammax, it is recommended to use the operators to combine grammar nodes, as they provide a more concise and readable syntax. The following operators are available for combining grammar nodes:

- `a | b`: Represents an alternative between two grammar nodes. It matches either `a` or `b`.
- `a + b`: Represents a sequence of two grammar nodes. It matches `a` followed by `b`.

There are also functions that can be used to create repetitions and optional patterns:

- `many(node)`: Matches zero or more occurrences of the given grammar node.
- `some(node)`: Matches one or more occurrences of the given grammar node.
- `opt(node)`: Matches zero or one occurrence of the given grammar node.
- `repeat(node, range)`: Matches the given grammar node repeated according to the specified range (e.g. `0..`, `1..`, `0..3`, etc.).
- `sep(node, sep)`: Matches zero or more occurrences of the given grammar node separated by the specified grammar node `sep`.
- `sep1(node, sep)`: Matches one or more occurrences of the given grammar node separated by the specified grammar node `sep`.

### Fields
Fields are used to scope a part of the grammar node and give it a name. This is useful to specify the semantic meaning of that part and to extract it later when building the abstract syntax tree (AST). A field is defined using the `field(name, node)` function, where `name` is the name of the field and `node` is the grammar node that defines what to match for that field. For example, the following rule defines two fields, `lhs` and `rhs`, for the left-hand side and right-hand side of an addition expression:

```rust
fn addition() -> GrammarNode {
    field("lhs", t(NUMBER)) + t("+") + field("rhs", t(NUMBER))
}
```

### Rule references
A grammar node can also be a reference to another rule. This allows us to build more complex (such as recursive) structures by referring to other rules. A rule reference is created using the `r!(rule_name)` function, where `rule_name` is the name and function variable of the rule being referenced. For example, the following rule defines a recursive structure for unwrapping parentheses around a number:

```rust
fn pimary() -> GrammarNode {
    t(NUMBER)
}
fn parens() -> GrammarNode {
    t("(") + r!(parens) + t(")") | r!(pimary)
}
```

### Dropping, precedence and associativity

In Grammax, precedence and associativity of operator expressions can be defined using a powerful feature called **dropping**. Dropping allows you to exclude certain alternatives from a rule reference, which is essential for controlling the recursive structure of the grammar and ensuring that it correctly captures the intended precedence and associativity of operators.

By default, when a rule reference refers to the entire structure of the target rule. With dropping, you can specify the subnodes of the target rule to be included in the reference site. This is crucial for building a grammar for operator with precedence and associativity. Assuming we have a rule `m` defined with multiple alternatives `a | b | c`, we can write `r!(m).drop(1)` to refer to `m` without the first alternative `a`, and `r!(m).drop(2)` to refer to `m` without the first two alternatives `a` and `b`. 

Dropping is required to satisfy the following conditions:
- Dropping can only be applied to rule references, and it must be applied at the reference site, not the definition site.
- The target rule must have at least two alternatives, and the number of alternatives to drop must be less than the total number of alternatives in the target rule.

Dropping is powerful to abstract precedence and associativity in traditional operator patterns into a more general form.

- *Precedence*: By dropping more alternatives from the target rule, you can restrict the recursive structure of the grammar to only match smaller structures, which gives it higher precedence.
- *Associativity*: By dropping more alternatives from the right-hand side of a rule, you can allow for the left-hand side of the expression to be matched as a larger structure, while the right-hand side is restricted to smaller structures, which gives it left-associativity. Conversely, by dropping more alternatives from the left-hand side of a rule, you can allow for the right-hand side of the expression to be matched as a larger structure, while the left-hand side is restricted to smaller structures, which gives it right-associativity.

For example, the following rule defines a recursive structure for addition and multiplication expressions, where multiplication has higher precedence than addition and both operators are left-associative:

```rust
fn expr() -> GrammarNode {
    r!(add) | r!(mul) | r!(num)
}

fn add() -> GrammarNode {
    r!(expr) + t('+') + r!(expr).drop(1)
}

fn mul() -> GrammarNode {
    r!(expr).drop(1) + t('*') + r!(expr).drop(2)
}

fn num() -> GrammarNode {
    t(NUMBER) | t('(') + r!(expr) + t(')')
}
```

- *Precedence*: In the definition of `add` and `mul`, it is notable that `mul` drops more of the alternatives from `expr` than `add`, which gives it higher precedence. This restricts the recursive structure of `mul` to only match multiplication expressions (`mul`), while `add` can match both addition and multiplication expressions (`mul` and `add`).
- *Associativity*: Both `add` and `mul` are left-associative because the left-hand references to `expr` drops less than the right-hand ones. This allows for the left-hand side of the expression to be matched as a larger structure, while the right-hand side is restricted to smaller structures.

## Create a grammar

There are three ways to create a grammar in Grammax.

### Using functions
As mentioned above, you can define rules using functions that return `GrammarNode`. This is the most idiomatic way to create a grammar. 

Continuing from the previous example, we can define a complete grammar ready to parse arithmetic expressions with addition and multiplication:

```rust
let grammar = Grammar::new(r!(expr) + t(EOF), "start");
```

We create a grammar starting from a root named "start", which is defined as the rule `expr` followed by the end of file (EOF) matcher.

### Using the macro
Alternatively, in a more ergonomic and recommended way, you can use the `new_grammar!` macro to define a grammar. It is more concise and readable. You can write the grammar akin to PEG (Parsing Expression Grammar) syntax, which is a popular formalism for describing grammars. The macro will automatically generate the necessary functions and structures for the grammar. For example, the same grammar for arithmetic expressions can be defined using the macro as follows:

```rust
let grammar = new_grammar!(
    start where
    start -> r!(expr) + tt(EOF)
    expr -> r!(add) | r!(mul) | r!(primary)
    add  -> r!(expr) + tt("+") + r!(expr).drop(1)
    mul  -> r!(expr).drop(1) + tt("*") + r!(expr).drop(2)
    primary -> tt(NUMBER) | tt("(") + r!(expr) + tt(")")
);
```

`start where` defines the root rule of the grammar, and each subsequent line defines a rule with its name and its grammar node.

### Using the DSL
You can also use [the DSL](./3-3-dsl.md) to define a grammar. The DSL is a domain-specific language that provides a more concise and readable syntax for defining grammars. 

### Writing descriptions for rules

> [!WARNING]
> This feature is still in development and may be subject to change in the future. It is currently not recommended to use this feature in production code.

When defining a grammar, you can write descriptions for some rules using `in_which`. 

### Caching
By default, Grammax generates cache for each grammar when it is initialized. Cache files are stored in the default temporary directory of the operating system, and they are automatically invalidated when the grammar definition changes.

Cache artifacts are always written as grammar bundles (`GMXB` format), the same format used by `save_to`. Legacy non-bundle binaries are not loaded.

If you want to disable caching for a grammar, you can use the `Grammar::new_uncached` method or the `new_grammar_no_cache!` macro. This will create a grammar that does not use caching, which may be useful for testing or when you want to ensure that the grammar is always recompiled from the source code.

## Test a grammar

After creating a grammar, you can test it by calling the `test` or `parse` method on its instance. The `test` method checks if the input string matches the grammar and returns a boolean value, while the `parse` method returns a `Result` that contains the parsed AST or an error message if the input does not match the grammar. 

Continuing from the previous example, we can test the grammar with some input strings:

```rust
assert!(my_grammar.test("1+2*3"));
assert!(my_grammar.test("(1+2)*3"));
assert!(!my_grammar.test("1+2*"));

// Parsing and printing the AST and messages
println!("{}", my_grammar.parse("1*(2+3").format_ast());
println!("{}", my_grammar.parse("1*(2+3").format_messages());
```

In the [parser](./4-parser.md) chapter, we will further talk about the parsing results.

## Compile and save a grammar

In Grammax, grammar compilation is the process of transforming a grammar definition into an optimized internal representation that can be efficiently executed by the parser. Attriuted to caching, grammars are not compiled every time they are used. 

You can manually save the compiled grammar to a file using the `save_to` method on the grammar instance. This is useful for packing up the grammar and sharing it with others without exposing the source code. The compiled grammar can be loaded later using the `Grammar::load_from` method, which will return a grammar instance that is ready to use.

`save_to` always writes a grammar bundle (`GMXB`), which contains the serialized compiled grammar in one binary artifact.

```rust
my_grammar.save_to("my_grammar.bin").unwrap();
let loaded_grammar = Grammar::load_from("my_grammar.bin").unwrap();
assert!(loaded_grammar.test("1+2*3"));
```
