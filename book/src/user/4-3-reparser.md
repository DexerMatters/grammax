# Reparser

**Reparsing** is the process of incremental updating of a parse tree in response to edits to the input text. The `Reparser` struct provides an interface for performing reparsing operations on a parse tree, allowing for efficient updates without needing to re-parse the entire input from scratch.

## Creating a reparser

To create a `Reparser`, you can use the `from_parser` method, which takes an existing `Parser` instance and initializes the reparser with the current parse tree and grammar:

```rust
let parser = Parser::new(grammar);
let mut reparser = Reparser::from_parser(parser);
```

## Performing edits

You can perform edits using the `replace`, `insert`, and `delete` methods. 
- `replace` allows you to replace a span of text with new text.
- `insert` allows you to insert new text at a specific position.
- `delete` allows you to delete a span of text.

```rust
// Insert "hello" at position 0
reparser.insert(0, "1 + 2 * 3")?; // Current text is "1 + 2 * 3"
// Replace "2" with "4"
reparser.replace(4, 5, "4")?; // Current text is "1 + 4 * 3"
// Delete "1 + "
reparser.delete(0, 4)?; // Current text is "4 * 3"
```

As you can see, these methods return a `Result` indicating whether the edit was successful or if there was an error. There are three types of errors that can occur:

Out of bounds
  : This error occurs when the specified span for an edit is outside the bounds of the current text. For example, trying to replace text from position 10 to 15 when the current text only has 5 characters would result in this error.
  
Invalid span
  : This error occurs when the specified span is invalid, such as when the start position is greater than the end position. For example, trying to replace text from position 5 to position 3 would result in this error.

No incremental candidates
  : This error occurs when the reparser is unable to find any candidates for incremental re-parsing. It is a bug if this error is returned, as it indicates that the reparser failed to find any way to update the parse tree incrementally.

If an edit is successful, the reparser will return a list of `Command`s that represent the changes to be made to the parse tree. It is crucial to the subsequent processing of the parse tree which we will talk about later.

For example, for the edit that replaces "2" with "4" in the expression "1 + 2 * 3",

```rust
let result = reparser.replace(4, 5, "4")?;
for command in result {
    println!("{:?}", command);
}

/* Output:

Create { id: 1, value: Node(Token { rule_ix: 3, text: " 4", field: "" }) }
Replace { index: Path(NodePath([0, 0, 2, 0, 0, 0, 0])), id: 1 }
*/

```

The output shows that a new node with ID 1 was created to represent the token " 4", and then the existing node at the specified path in the parse tree was replaced with this new node.

We will talk more about commands in [the compiler chapter](./5-compiler.md).

## Reading the current state

You can read the current state of the parse tree after edits using the `current_view` or `current_viewer` methods. Their usage is exactly the same as the [`Parser`'s `view` and `viewer` methods](./4-2-tree.md#navigating-the-tree), reflecting the current state of the parse tree after edits have been applied. For example:

```rust
reparser.insert(0, "1 + 2 * 3")?;
println!("{}", reparser.current_view());

/* Output:
start [width: 9]
   └─ expr [width: 9]
      └─ add [width: 9]
         ├─ expr [width: 1]
         │  └─ primary [width: 1]
         │     └─ 1 [width: 1]
         ├─  + [width: 2]
         └─ expr [width: 6]
            └─ mul [width: 6]
               ├─ expr [width: 2]
               │  └─ primary [width: 2]
               │     └─  2 [width: 2]
               ├─  * [width: 2]
               └─ expr [width: 2]
                  └─ primary [width: 2]
                     └─  3 [width: 2]
*/

reparser.delete(0, 3)?;
println!("{}", reparser.current_view());

/* Output:
start [width: 6]
   └─ expr [width: 6]
      └─ mul [width: 6]
         ├─ expr [width: 2]
         │  └─ primary [width: 2]
         │     └─  2 [width: 2]
         ├─  * [width: 2]
         └─ expr [width: 2]
            └─ primary [width: 2]
               └─  3 [width: 2]
*/
```

You can also call `current_text` to get the current text and `current_messages` to get the current messages after edits.