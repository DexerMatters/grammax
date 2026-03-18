# Words

**Words** or tokens are the basic building blocks of a language. In Grammax, it is called **matchers** for word consumers. They are designed to read the input stream and sometimes *consume* it, which means they can advance the input position. Matchers are used in grammar rules to define how the language should be lexically structured.

## Built-in matchers

There are some built-in atomic matchers provided by Grammax, which you can use directly in your grammar. Here is a table of the built-in matchers:

|Matcher|Description|Example|
|-|-|-|
|`EOF`|Matches the end of the input string|||
|`DIGIT`|Matches a single digit|`0`, `5`, `9`|
|`HEX_DIGIT`|Matches a single hexadecimal digit|`0`, `5`, `9`, `A`, `F`|
|`OCTAL_DIGIT`|Matches a single octal digit|`0`, `5`, `7`|
|`NUMBER`|Matches one or more digits|`123456789`|
|`OCTAL_NUMBER`|Matches an octal number|`12345`|
|`HEX_NUMBER`|Matches a hexadecimal number|`0CFA`|
|`INTEGER`|Matches an integer in multiple formats (decimal, octal, hexadecimal)|`124`, `0x9F`, `035`|
|`FLOAT`|Matches a floating point number|`.123`, `0.34`, `124.43`|
|`ALPHABETS`|Matches one or more alphabets|`abcde`|
|`ALPHANUMBER`|Matches one or more alphanumeric characters|`var123`|
|`IDENT`|Matches an identifier (alphanumeric and underscores, starting with a letter or underscore)|`_var1`|
|`STRING`|Matches characters with escaping|`\"Apple\"` `Hello\nworld`|
|`WHITESPACES`|Matches zero or more whitespace characters and newlines||
|`regex("pattern")`|Matches a regex pattern|`regex("[a-z]+")` matches `hello`|


Character, number and string literals can also be used as matchers. They match the exact characters in the input. For example, `'+'` matches the plus sign, and `"if"` matches the keyword "if". Note that string literals are case-sensitive. You can write `()` to match any empty string, which is useful for defining rules that can be optional or have empty alternatives. It is equivalent to `""`.

There are also some compound matchers that can be constructed from other matchers:

|Matcher|Description|Example|
|-|-|-|
|`token(matcher)`|Matches the given matcher, but also consumes any leading whitespace|`token("if")` matches `   if`|
|`matcher.repeat(range)`|Matches the given matcher repeated according to the specified range (e.g. `0..`, `1..`, `0..3`, etc.)|`"a".repeat(3..5)` matches `"aaa"`, `"aaaa"`, or `"aaaaa"`|
|`matcher.or(other)`|Matches either the given matcher or the other matcher|`"a".or(())` matches an optional `"a"`|
|`matcher.then(matcher)`|Matches the given matcher followed by the other matcher|`"a".then(NUMBER)` matches `"a123"`|
|`named(name, matcher)`|Defines a named matcher that can be pretty printed in error messages||


## Usage

You can use these matchers solely as a lexer to tokenize the input string. This may look a bit redundant and verbose because Grammax is designed to be a parser generator. Matchers alone are not very powerful and seldom used in practice.

```rust
let color = "0xFFEECC";
let mut pos = 0;

let result = "0x".then(HEX_DIGIT.times(6)).matches(color, &mut pos);
assert_eq!(result, Some(8)); // The length of the matched substring "0xFFEECC"
assert_eq!(pos, 8); // The input position has advanced to the end of the string

```

## Custom matchers

There are two ways to define custom matchers in Grammax:

### The `Matcher` trait

Every matcher mentioned above implements the `Matcher` trait, which defines a common interface for matching input strings. You can implement this trait for your own custom matchers. For example:

```rust
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SkipToEndMatcher;

#[typetag::serde]
impl Matcher for SkipToEndMatcher {
    fn matches(&self, input: &str, pos: &mut usize) -> Option<usize> {
        let remaining = &input[*pos..];
        let len = remaining.len();
        *pos += len; // Consume the entire remaining input
        Some(len)
    }

    fn display(&self) -> String {
        String::from("skip to end")
    }

    fn is_nullable(&self) -> bool {
        true // When the input is empty, it can still match and consume nothing
    }

    fn is_consuming(&self) -> bool {
        false // Maybe the input is empty
    }
}
```

`matches` is the core method that defines how the matcher works. It takes the input string and a mutable reference to the current position, and returns an `Option<usize>` indicating the length of the matched substring if successful, or `None` if it fails to match. `display` is used for error messages, `is_nullable` indicates whether the matcher can match an empty string, and `is_consuming` indicates whether the matcher advances the input position.

If you want custom matchers to be persisted through cache and `save_to`, make them serde-serializable and register them with `#[typetag::serde]` as shown above.

### The `OneOf` struct

`OneOf` matches a single character from a string set. `OneOf("xxxxx")` means “match one of these characters”. It implements `Matcher`, so you can use it directly in rules. For example:

```rust
pub const VOWEL: OneOf = OneOf("aeiouAEIOU");
```
