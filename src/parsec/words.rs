use std::{
    fmt::Debug,
    ops::{self},
    sync::{Arc, OnceLock},
};

use regex::Regex;

/// A trait representing a matcher that lexically matches a portion of the input string.
pub trait Matcher: Debug {
    /// Attempts to match the input string starting from the given position.
    ///
    /// Returns `Some(length)` if the match is successful, where `length` is the number of characters matched, and advances the position by that length.
    /// Returns `None` if the match fails, and does not modify the position.
    fn matches<'a>(&self, input: &'a str, pos: &mut usize) -> Option<usize>;

    /// Provides a human-readable description of the matcher, which can be used in error messages or debugging output.
    fn display(&self) -> String;

    /// Indicates whether this matcher may match an empty string (may not consume anything).
    fn is_nullable(&self) -> bool;

    /// Indicates whether this matcher always consumes at least one character when it succeeds.
    fn is_consuming(&self) -> bool;

    /// Provides a preview string that represents the expected input for this matcher, if available. This can be used for error recovery.
    fn preview(&self) -> Option<&str> {
        None
    }

    /// Combines this matcher with another matcher in a sequence, meaning that both must match in order.
    fn then<U>(self, other: U) -> Sequence<Self, U>
    where
        Self: Sized,
        U: Matcher,
    {
        Sequence(self, other)
    }

    /// Combines this matcher with another matcher in an alternative, meaning that either can match.
    fn or<U>(self, other: U) -> Alternative<Self, U>
    where
        Self: Sized,
        U: Matcher,
    {
        Alternative(self, other)
    }

    /// Repeats this matcher according to the specified range.
    fn times<R>(self, range: R) -> Repeat<Self, R>
    where
        Self: Sized,
        R: ops::RangeBounds<usize>,
    {
        Repeat(self, range)
    }
}

/// A convenient type alias for a reference-counted matcher, allowing for shared ownership and dynamic dispatch.
pub type MatcherRef = Arc<dyn Matcher + Send + Sync + 'static>;

/// A matcher that matches the end of the input string.
#[derive(Debug, Clone, Copy)]
pub struct EndOfInput;

/// A matcher that represents an alternative between two matchers, meaning that either can match.
#[derive(Debug, Clone, Copy)]
pub struct Alternative<T, U>(pub T, pub U);

/// A matcher that represents a sequence of two matchers, meaning that both must match in order.
#[derive(Debug, Clone, Copy)]
pub struct Sequence<T, U>(pub T, pub U);

/// A matcher that represents a repetition of another matcher according to a specified range.
#[derive(Debug, Clone, Copy)]
pub struct Repeat<T, R: ops::RangeBounds<usize>>(pub T, pub R);

#[doc(hidden)]
#[derive(Debug, Clone, Copy)]
pub struct CustomChar {
    pub predicate: fn(char) -> bool,
    pub pick: fn() -> &'static str,
    pub description: &'static str,
}

#[derive(Debug, Clone)]
pub struct RegexMatcher {
    pub pattern: Regex,
    pub is_nullable: OnceLock<bool>,
    pub is_consuming: OnceLock<bool>,
}

impl RegexMatcher {
    pub fn new(pattern: &str) -> Self {
        Self {
            pattern: Regex::new(pattern).unwrap(),
            is_nullable: OnceLock::new(),
            is_consuming: OnceLock::new(),
        }
    }
}

impl Matcher for RegexMatcher {
    fn matches<'a>(&self, input: &'a str, pos: &mut usize) -> Option<usize> {
        let start = *pos;
        if let Some(mat) = self.pattern.find(&input[*pos..]) {
            if mat.start() == 0 {
                *pos += mat.end();
                return Some(*pos - start);
            }
        }
        None
    }

    fn display(&self) -> String {
        format!("regex({})", self.pattern)
    }

    fn is_nullable(&self) -> bool {
        *self.is_nullable.get_or_init(|| self.pattern.is_match(""))
    }

    fn is_consuming(&self) -> bool {
        *self.is_consuming.get_or_init(|| !self.pattern.is_match(""))
    }
}

/// A named matcher that associates a human-readable name with another matcher, which can be useful for error messages and debugging.
#[derive(Debug, Clone, Copy)]
pub struct NamedMatcher<M: Matcher> {
    pub name: &'static str,
    pub matcher: M,
}

impl<M: Matcher> NamedMatcher<M> {
    /// Creates a new named matcher with the given name and underlying matcher.
    pub const fn new(name: &'static str, matcher: M) -> NamedMatcher<M> {
        NamedMatcher { name, matcher }
    }
}

/// A predefined matcher that matches a sequence of ASCII digits, representing a number.
pub const NUMS: NamedMatcher<Repeat<CustomChar, ops::RangeFrom<usize>>> = NamedMatcher::new(
    "number",
    Repeat(
        CustomChar {
            predicate: |c| c.is_ascii_digit(),
            pick: || "0123456789",
            description: "digit",
        },
        1..,
    ),
);

/// A predefined matcher that matches a sequence of ASCII alphabetic characters, representing an identifier.
pub const ALPHAS: NamedMatcher<Repeat<CustomChar, ops::RangeFrom<usize>>> = NamedMatcher::new(
    "identifier",
    Repeat(
        CustomChar {
            predicate: |c| c.is_ascii_alphabetic(),
            pick: || "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ",
            description: "alphabetic character",
        },
        1..,
    ),
);

/// A predefined matcher that matches a sequence of ASCII alphanumeric characters or underscores, representing an identifier that can include digits.
pub const ALPHANUMS: NamedMatcher<Repeat<CustomChar, ops::RangeFrom<usize>>> = NamedMatcher::new(
    "alphanum",
    Repeat(
        CustomChar {
            predicate: |c| c.is_ascii_alphanumeric() || c == '_',
            pick: || "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_",
            description: "alphanumeric character or underscore",
        },
        1..,
    ),
);

pub fn regex(pattern: &str) -> RegexMatcher {
    RegexMatcher {
        pattern: Regex::new(pattern).unwrap(),
        is_nullable: OnceLock::new(),
        is_consuming: OnceLock::new(),
    }
}

/// A matcher for identifiers accepted by most programming languages.
/// Starts with a letter or underscore, followed by letters, digits, or underscores.
#[derive(Debug, Clone, Copy)]
pub struct IdentMatcher;

impl Matcher for IdentMatcher {
    fn matches<'a>(&self, input: &'a str, pos: &mut usize) -> Option<usize> {
        let start = *pos;

        // First character must be letter or underscore
        if let Some(first_char) = input[*pos..].chars().next() {
            if !first_char.is_ascii_alphabetic() && first_char != '_' {
                return None;
            }
            *pos += first_char.len_utf8();
        } else {
            return None;
        }

        // Additional characters can be alphanumeric or underscore
        while *pos < input.len() {
            if let Some(c) = input[*pos..].chars().next() {
                if c.is_ascii_alphanumeric() || c == '_' {
                    *pos += c.len_utf8();
                } else {
                    break;
                }
            } else {
                break;
            }
        }

        Some(*pos - start)
    }

    fn display(&self) -> String {
        String::from("identifier")
    }

    fn is_nullable(&self) -> bool {
        false
    }

    fn is_consuming(&self) -> bool {
        true
    }
}

/// A predefined matcher that matches identifiers accepted by most programming languages.
/// Identifiers must start with a letter or underscore, followed by zero or more letters, digits, or underscores.
pub const IDENT: NamedMatcher<IdentMatcher> = NamedMatcher::new("ident", IdentMatcher);

/// A matcher for string content that handles escape sequences (e.g., \n, \t, \\, \").
/// Matches a sequence where each character is either:
/// - A regular character (not ", \, newline)
/// - An escape sequence starting with \ followed by any character
#[derive(Debug, Clone, Copy)]
pub struct CharMatcherWithEscapes;

impl Matcher for CharMatcherWithEscapes {
    fn matches<'a>(&self, input: &'a str, pos: &mut usize) -> Option<usize> {
        let start = *pos;

        if let Some(first_char) = input[*pos..].chars().next() {
            if first_char == '\\' {
                // Escape sequence: backslash followed by a valid escape character
                *pos += first_char.len_utf8();
                if let Some(next_char) = input[*pos..].chars().next() {
                    // Valid escape characters: n, t, r, \, ", ', b, f, v, 0, x, u
                    match next_char {
                        'n' | 't' | 'r' | '\\' | '"' | '\'' | 'b' | 'f' | 'v' | '0' | 'x' | 'u' => {
                            *pos += next_char.len_utf8();
                            Some(*pos - start)
                        }
                        _ => {
                            // Invalid escape sequence - backtrack
                            *pos = start;
                            None
                        }
                    }
                } else {
                    // Backslash at end of input - backtrack
                    *pos = start;
                    None
                }
            } else if first_char != '"' && first_char != '\n' && first_char != '\r' {
                // Regular character (not quote, backslash, or newline)
                *pos += first_char.len_utf8();
                Some(*pos - start)
            } else {
                None
            }
        } else {
            None
        }
    }

    fn display(&self) -> String {
        String::from("characters including escapes")
    }

    fn is_nullable(&self) -> bool {
        false
    }

    fn is_consuming(&self) -> bool {
        true
    }
}

/// A predefined matcher that matches string content with support for escape sequences.
/// Matches characters in a string literal, including escaped sequences like \n, \t, \\, \", etc.
pub const STRING: NamedMatcher<Repeat<CharMatcherWithEscapes, ops::RangeFrom<usize>>> =
    NamedMatcher::new("string", Repeat(CharMatcherWithEscapes, 0..));

/// A predefined matcher that matches a sequence of whitespace characters, which can be used to skip irrelevant spaces in the input.
pub const WHITESPACES: NamedMatcher<Repeat<CustomChar, ops::RangeFrom<usize>>> = NamedMatcher::new(
    "whitespaces",
    Repeat(
        CustomChar {
            predicate: |c| c.is_whitespace(),
            pick: || " \t\r\n",
            description: "whitespace",
        },
        1..,
    ),
);

/// A helper function that creates a matcher which matches the given matcher preceded by optional leading whitespace. This is useful for token matchers that should ignore leading spaces.
pub const fn token<M: Matcher>(
    matcher: M,
) -> Sequence<Repeat<CustomChar, ops::RangeFrom<usize>>, M> {
    Sequence(
        Repeat(
            CustomChar {
                predicate: |c| c.is_whitespace() || c == '\n' || c == '\r',
                pick: || " \t\r\n",
                description: "whitespace or newline",
            },
            0..,
        ),
        matcher,
    )
}

// Matcher implementations

impl Matcher for () {
    fn matches<'a>(&self, _input: &'a str, _pos: &mut usize) -> Option<usize> {
        Some(0)
    }

    fn display(&self) -> String {
        String::from("ε")
    }

    fn is_nullable(&self) -> bool {
        true
    }

    fn is_consuming(&self) -> bool {
        false
    }
}

impl Matcher for CustomChar {
    fn matches<'a>(&self, input: &'a str, pos: &mut usize) -> Option<usize> {
        let start = *pos;
        if let Some(next_char) = input[*pos..].chars().next() {
            if (self.predicate)(next_char) {
                *pos += next_char.len_utf8();
                return Some(*pos - start);
            }
        }
        None
    }

    fn display(&self) -> String {
        String::from(self.description)
    }

    fn is_nullable(&self) -> bool {
        false
    }

    fn is_consuming(&self) -> bool {
        true
    }
}

impl Matcher for &str {
    fn matches<'a>(&self, input: &'a str, pos: &mut usize) -> Option<usize> {
        let start = *pos;
        if input[*pos..].starts_with(*self) {
            *pos += self.len();
            Some(*pos - start)
        } else {
            None
        }
    }

    fn display(&self) -> String {
        format!("\"{}\"", self)
    }

    fn is_nullable(&self) -> bool {
        self.len() == 0
    }

    fn is_consuming(&self) -> bool {
        self.len() > 0
    }

    fn preview(&self) -> Option<&str> {
        Some(*self)
    }
}

impl Matcher for char {
    fn matches<'a>(&self, input: &'a str, pos: &mut usize) -> Option<usize> {
        let start = *pos;
        if let Some(next_char) = input[*pos..].chars().next() {
            if next_char == *self {
                *pos += next_char.len_utf8();
                return Some(*pos - start);
            }
        }
        None
    }

    fn display(&self) -> String {
        format!("'{}'", self)
    }

    fn is_nullable(&self) -> bool {
        false
    }

    fn is_consuming(&self) -> bool {
        true
    }
}

impl Matcher for EndOfInput {
    fn matches<'a>(&self, input: &'a str, pos: &mut usize) -> Option<usize> {
        if *pos >= input.len() { Some(0) } else { None }
    }

    fn display(&self) -> String {
        String::from("EOF")
    }

    fn is_nullable(&self) -> bool {
        false
    }

    fn is_consuming(&self) -> bool {
        false
    }
}

impl<T: Matcher, U: Matcher> Matcher for Alternative<T, U> {
    fn matches<'a>(&self, input: &'a str, pos: &mut usize) -> Option<usize> {
        let start = *pos;
        if let Some(len) = self.0.matches(input, pos) {
            return Some(len);
        }
        *pos = start;
        self.1.matches(input, pos)
    }

    fn display(&self) -> String {
        format!("({} | {})", self.0.display(), self.1.display())
    }

    fn is_nullable(&self) -> bool {
        self.0.is_nullable() || self.1.is_nullable()
    }

    fn is_consuming(&self) -> bool {
        self.0.is_consuming() && self.1.is_consuming()
    }

    fn preview(&self) -> Option<&str> {
        let left = self.0.preview();
        let right = self.1.preview();
        if left.is_some() && left == right {
            left
        } else {
            None
        }
    }
}

impl<T: Matcher, U: Matcher> Matcher for Sequence<T, U> {
    fn matches<'a>(&self, input: &'a str, pos: &mut usize) -> Option<usize> {
        let start = *pos;
        if self.0.matches(input, pos).is_none() {
            return None;
        }
        if self.1.matches(input, pos).is_none() {
            *pos = start;
            return None;
        }
        Some(*pos - start)
    }

    fn display(&self) -> String {
        format!("{} {}", self.0.display(), self.1.display())
    }

    fn is_nullable(&self) -> bool {
        self.0.is_nullable() && self.1.is_nullable()
    }

    fn is_consuming(&self) -> bool {
        self.0.is_consuming() || self.1.is_consuming()
    }

    fn preview(&self) -> Option<&str> {
        if self.0.is_nullable() {
            self.1.preview().or_else(|| self.0.preview())
        } else {
            self.0.preview()
        }
    }
}

impl<T: Matcher, R: ops::RangeBounds<usize> + Debug> Matcher for Repeat<T, R> {
    fn matches<'a>(&self, input: &'a str, pos: &mut usize) -> Option<usize> {
        let start = *pos;
        let min = match self.1.start_bound() {
            ops::Bound::Included(&n) => n,
            ops::Bound::Excluded(&n) => n + 1,
            ops::Bound::Unbounded => 0,
        };
        let max = match self.1.end_bound() {
            ops::Bound::Included(&n) => Some(n),
            ops::Bound::Excluded(&n) => Some(n.saturating_sub(1)),
            ops::Bound::Unbounded => None,
        };

        let mut count = 0;
        while max.map_or(true, |m| count < m) {
            let before = *pos;
            if self.0.matches(input, pos).is_some() {
                count += 1;
            } else {
                *pos = before;
                break;
            }
        }

        if count >= min {
            Some(*pos - start)
        } else {
            *pos = start;
            None
        }
    }

    fn display(&self) -> String {
        format!("{}*", self.0.display())
    }

    fn is_nullable(&self) -> bool {
        match self.1.start_bound() {
            ops::Bound::Included(&0) | ops::Bound::Excluded(&0) | ops::Bound::Unbounded => true,
            _ => false,
        }
    }

    fn is_consuming(&self) -> bool {
        self.0.is_consuming()
    }

    fn preview(&self) -> Option<&str> {
        match self.1.start_bound() {
            ops::Bound::Included(&0) | ops::Bound::Excluded(&0) | ops::Bound::Unbounded => None,
            _ => self.0.preview(),
        }
    }
}

impl<M: Matcher> Matcher for NamedMatcher<M> {
    fn matches<'a>(&self, input: &'a str, pos: &mut usize) -> Option<usize> {
        self.matcher.matches(input, pos)
    }

    fn display(&self) -> String {
        self.name.to_string()
    }

    fn is_nullable(&self) -> bool {
        self.matcher.is_nullable()
    }

    fn is_consuming(&self) -> bool {
        self.matcher.is_consuming()
    }

    fn preview(&self) -> Option<&str> {
        self.matcher.preview()
    }
}

/// A matcher for dynamically created owned string literals.
/// Used internally for deserializing cached matchers.
#[derive(Debug)]
pub struct OwnedLiteral(pub String);

impl Matcher for OwnedLiteral {
    fn matches<'a>(&self, input: &'a str, pos: &mut usize) -> Option<usize> {
        let start = *pos;
        if input[*pos..].starts_with(&self.0) {
            *pos += self.0.len();
            Some(*pos - start)
        } else {
            None
        }
    }

    fn display(&self) -> String {
        format!("\"{}\"", self.0)
    }

    fn is_nullable(&self) -> bool {
        self.0.is_empty()
    }

    fn is_consuming(&self) -> bool {
        !self.0.is_empty()
    }

    fn preview(&self) -> Option<&str> {
        Some(&self.0)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct TokenizedMatcher {
    pub inner: MatcherRef,
}

impl Matcher for TokenizedMatcher {
    fn matches<'a>(&self, input: &'a str, pos: &mut usize) -> Option<usize> {
        let start = *pos;
        let _ = token(()).0.matches(input, pos).or_else(|| Some(0));
        if self.inner.matches(input, pos).is_some() {
            Some(*pos - start)
        } else {
            *pos = start;
            None
        }
    }

    fn display(&self) -> String {
        format!("char_predicate* {}", self.inner.display())
    }

    fn is_nullable(&self) -> bool {
        self.inner.is_nullable()
    }

    fn is_consuming(&self) -> bool {
        self.inner.is_consuming()
    }

    fn preview(&self) -> Option<&str> {
        self.inner.preview()
    }
}
