use std::{
    fmt::Debug,
    ops::{self},
    sync::Arc,
};

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

/// A matcher that matches the start of the input string.
#[derive(Debug, Clone, Copy)]
pub struct StartOfInput;

/// A matcher that represents an alternative between two matchers, meaning that either can match.
#[derive(Debug, Clone, Copy)]
pub struct Alternative<T, U>(pub T, pub U);

/// A matcher that represents a sequence of two matchers, meaning that both must match in order.
#[derive(Debug, Clone, Copy)]
pub struct Sequence<T, U>(pub T, pub U);

/// A matcher that represents a repetition of another matcher according to a specified range.
#[derive(Debug, Clone, Copy)]
pub struct Repeat<T, R: ops::RangeBounds<usize>>(pub T, pub R);

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
pub const NUMS: NamedMatcher<Repeat<fn(char) -> bool, ops::RangeFrom<usize>>> =
    NamedMatcher::new("number", Repeat(|c: char| c.is_ascii_digit(), 1..));

/// A predefined matcher that matches a sequence of ASCII alphabetic characters, representing an identifier.
pub const ALPHAS: NamedMatcher<Repeat<fn(char) -> bool, ops::RangeFrom<usize>>> =
    NamedMatcher::new("identifier", Repeat(|c: char| c.is_ascii_alphabetic(), 1..));

/// A predefined matcher that matches a sequence of ASCII alphanumeric characters or underscores, representing an identifier that can include digits.
pub const ALPHANUMS: NamedMatcher<Repeat<fn(char) -> bool, ops::RangeFrom<usize>>> =
    NamedMatcher::new(
        "alphanum",
        Repeat(|c: char| c.is_ascii_alphanumeric() || c == '_', 1..),
    );

const fn string_char(c: char) -> bool {
    c != '"' && c != '\n' && c != '\r'
}

/// A predefined matcher that matches a JSON string literal, which is a sequence of characters enclosed in double quotes, excluding unescaped double quotes and newlines.
pub const STRING: NamedMatcher<Repeat<fn(char) -> bool, ops::RangeFrom<usize>>> =
    NamedMatcher::new("json_string", Repeat(string_char, 0..));

/// A predefined matcher that matches a sequence of whitespace characters, which can be used to skip irrelevant spaces in the input.
pub const WHITESPACES: NamedMatcher<Repeat<fn(char) -> bool, ops::RangeFrom<usize>>> =
    NamedMatcher::new("whitespaces", Repeat(|c: char| c.is_whitespace(), 1..));

/// A helper function that creates a matcher which matches the given matcher preceded by optional leading whitespace. This is useful for token matchers that should ignore leading spaces.
pub const fn token<M: Matcher>(
    matcher: M,
) -> Sequence<Repeat<fn(char) -> bool, ops::RangeFrom<usize>>, M> {
    Sequence(
        Repeat(|c: char| c.is_whitespace() || c == '\n' || c == '\r', 0..),
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

impl Matcher for fn(char) -> bool {
    fn matches<'a>(&self, input: &'a str, pos: &mut usize) -> Option<usize> {
        let start = *pos;
        if let Some(next_char) = input[*pos..].chars().next() {
            if self(next_char) {
                *pos += next_char.len_utf8();
                return Some(*pos - start);
            }
        }
        None
    }

    fn display(&self) -> String {
        String::from("char_predicate")
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

impl Matcher for StartOfInput {
    fn matches<'a>(&self, _input: &'a str, pos: &mut usize) -> Option<usize> {
        if *pos == 0 { Some(0) } else { None }
    }

    fn display(&self) -> String {
        String::from("SOF")
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
