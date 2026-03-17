# DSL

Grammax introduces a domain-specific language (DSL) for writing grammars, which is designed akin to PEG (Parsing Expression Grammar). The grammar of the DSL is also defined and compiled by Grammax itself, making it a self-hosted DSL. The DSL is designed to be simple and intuitive for grammar authors, while also being powerful enough to express complex grammars.

Intrinsically, the DSL's grammar is defined as follows:

```rust
new_grammar_no_cache! {
    table where
    table -> sep(r!(rule), t('\n')) + tt(EOF)
    rule -> field("name", tt(IDENT)) + tt("->") + field("definition", r!(expr))
    expr -> r!(alternative) | r!(sequence) | r!(fields) | r!(optional) | r!(some) | r!(many) | r!(drop) | r!(terminal) | r!(reference)
    alternative -> r!(expr).drop(1) + tt("|") + r!(expr)
    sequence -> r!(expr).drop(2) + t(" ") + r!(expr).drop(1)
    fields -> field("field_name", tt(IDENT)) + tt(":") + r!(expr).drop(3)
    drop -> r!(reference) + t("/") + t(NUMBER)
    optional -> r!(expr).drop(4) + t("?")
    many -> r!(expr).drop(6) + opt(t("{") + field("sep", r!(expr)) + tt("}")) + t("*")
    some -> r!(expr).drop(6) + opt(t("{") + field("sep", r!(expr)) + tt("}")) + t("+")
    reference -> tt(IDENT)
    terminal -> (tt("(") + r!(expr) + tt(")")) | opt(tt("!")) + r!(primary)
    primary -> r!(token) | r!(literal) | r!(regexp)
    token -> tt("IDENT") | tt("STRING") | tt("NUMBER") | tt("ALPHANUMBER") | tt("ALPHABETS") | tt("EOF")
    literal -> tt('"') + t(STRING) + t('"')
    regexp -> tt('/') + t(REGEXP_MATCHER.with(|x| x.clone())) + t('/')
};
```

Let's break down the code snippet above to understand how the DSL is defined and learn how to write grammars using it.

## Rules

The DSL allows you to define rules using the syntax `rule_name -> expression`, where `rule_name` is the name of the rule and `expression` is a combination of terminals, references to other rules, and operators. The `->` symbol separates the rule name from its definition.

Rules are separated by newlines, such like:

```
hello -> "hello"
world -> "world"
```

It defines two rules, `hello` and `world`, which match the string literals "hello" and "world" respectively.

The first rule written in the DSL is regarded as the start rule of the grammar, which is the entry point for parsing. In the example above, `hello` is the start rule.

## Terminals

The DSL provides several built-in terminals that you can use in your grammar definitions. These terminals include:

| Terminal Name | Description |
| --- | --- |
| `IDENT` | Matches identifiers, which are sequences of letters, digits, and underscores that do not start with a digit. |
| `STRING` | Matches string literals, which are sequences of characters enclosed in double quotes. |
| `NUMBER` | Matches numeric literals, which are sequences of digits. |
| `ALPHANUMBER` | Matches sequences of letters and digits. |
| `ALPHABETS` | Matches sequences of letters. |
| `EOF` | Matches the end of the input. |
| `/.../` | Matches sequences of characters that match the specified regular expression. |
| `"..."` | Matches the exact string literal specified between the double quotes. |

These terminals, by default, are defined to skip leading whitespace and newlines, making it easier to write grammars without worrying about whitespace handling. However, you can still write `!` before a terminal to disable this behavior if you want to match whitespace explicitly.

## References

In the DSL, you can reference other rules by using their names. The naming convention for rules is the same way as Rust identifiers, which means they can contain letters, digits, and underscores, but cannot start with a digit. And usually they are lowercase to distinguish them from built-in terminals. For example:

```
lot_of_numbers -> number{","}+
number -> NUMBER
```

## Combinations

The DSL allows you to combine expressions using various operators to define more complex grammar rules. The main combination operators include:

| Operator | Description |
| --- | --- |
| `a \| b` | Represents an alternative, meaning that the expression can match either the left-hand side or the right-hand side. |
| `a b` | Represents a sequence, meaning that the expression must match the left-hand side followed by the right-hand side. |
| `s: a` | Represents a [field](./3-2-rules.md#fields), allowing you to assign a name to a part of the expression for later reference. |
| `r/n` | Represents [dropping](./3-2-rules.md#dropping-precedence-and-associativity), allowing you to drop the count `n` of alternatives from a reference to resolve ambiguity. |
| `a?` | Represents an optional expression, meaning that the expression can match zero or one time. |
| `a*` | Represents zero or more repetitions of the preceding expression. |
| `a+` | Represents one or more repetitions of the preceding expression. |
| `a{sep}*` | Represents zero or more repetitions of the preceding expression, separated by the specified separator. |
| `a{sep}+` | Represents one or more repetitions of the preceding expression, separated by the specified separator. |

where `a`, `b`, and `sep` can be any valid expressions in the DSL, `r` is the reference name, `s` is the name of a field, and `n` is a positive integer.

> It is worth noting that the grammar itself is whitespace-sensitive, meaning that the placement of spaces can affect how the grammar is parsed. An inconspicuous example is the difference among `r/n` `r /n` `r/ n`, and `r / n`. The first one is a valid drop expression, while the other three are not, because the spaces break the expected pattern for a drop expression. It is the same way for repetition expressions (`a*`, `a+`, `a{sep}*`, `a{sep}+`). So when writing grammars in the DSL, it's important to be mindful of whitespace and ensure that it is used correctly to avoid parsing errors.

## Precedence

| Operator | Precedence | Associativity |
| --- | --- | --- |
| `\|` (alternative) | 1 | Right |
| ` ` (sequence) | 2 | Right |
| `:` (field) | 3 | None |
| `?` (optional) | 4 | None |
| `*` (zero or more) `+` (one or more) `{sep}*` (zero or more with separator) `{sep}+` (one or more with separator) | 5 | None |
| `/` (drop) | 6 | None |
| `!` (disable skipping) | 7 | None |

`None` means the operator does not associate with itself, so you cannot chain multiple occurrences of the operator without parentheses. For example, `a?` is valid, but `a??` is not valid without parentheses to clarify the grouping, like `(a?)?`.

> It is not recommended to make high-order repetitions like `(a*)*`, because they can lead to excessive backtracking and performance issues.

## Examples

- JSON grammar:
```ebnf
start    -> json EOF
json     -> object | array | string | number | boolean | null
object   -> "{" pair{","}* "}"
pair     -> key:string ":" value:json
array    -> "[" json{","}* "]"
string   -> "\"" STRING "\""
number   -> NUMBER
boolean  -> "true" | "false"
null     -> "null"
```

- Arithmetic expression grammar:
```ebnf
start   -> expr EOF
expr    -> add | sub | mul | div | primary
add     -> expr "+" expr/2
sub     -> expr "-" expr/2
mul     -> expr/2 "*" expr/4
div     -> expr/2 "/" expr/4
primary -> NUMBER | "(" expr ")"
```

- Grammar for the DSL itself:
```ebnf
table       -> rule{"\n"}* EOF
rule        -> name:IDENT "->" definition:expr
expr        -> alternative | sequence | fields | optional | some | many | drop | terminal | reference
alternative -> expr/1 "|" expr
sequence    -> expr/2 !" " expr/1
fields      -> field_name:IDENT ":" expr/3
drop        -> reference !"/" !NUMBER
optional    -> expr/4 !"?"
many        -> expr/6 (!"{" sep:expr "}")? !"*"
some        -> expr/6 (!"{" sep:expr "}")? !"+"
reference   -> IDENT
terminal    -> "(" expr ")" | "!"? primary
primary     -> token | literal | regexp
token       -> "IDENT" | "STRING" | "NUMBER" | "ALPHANUMBER" | "ALPHABETS" | "EOF"
literal     -> "\"" !STRING !"\""
regexp      -> "/" !/([^\/\\\r\n]|\\.)+/ !"/"
```

## Interpret the DSL

You can use `Grammar::interpret(dsl_text)` to interpret the DSL text into a `Grammar` instance, which can then be used for parsing. For example:

```rust
let dsl_text = r#"
start   -> expr EOF
expr    -> add | sub | mul | div | primary
add     -> expr "+" expr/2
sub     -> expr "-" expr/2
mul     -> expr/2 "*" expr/4
div     -> expr/2 "/" expr/4
primary -> NUMBER | "(" expr ")"
"#;

let grammar = Grammar::interpret(dsl_text).expect("Failed to interpret DSL");
assert!(grammar.test("1 + 2 * 3"));
```

You can also use `Grammar::interpret_file(file_path)` to read the DSL text from a file and interpret it into a `Grammar` instance.

For example, you can compile the DSL to binary grammar and save it to a file like this:

```rust
Grammar::interpret_file("path/to/grammar.gmx")?
        .save_to("path/to/grammar.gmx.bin")?;
```

Compared to defining the grammar embedded in Rust code, using the DSL may contain undefined references or other errors that can only be detected at runtime. Therefore, the interpretation always returns a `Result` to indicate whether the interpretation is successful or not. If the interpretation fails, the error message will provide details about the issue, such as undefined references or syntax errors in the DSL text.