use std::{
    fmt::Debug,
    ops::{self},
    sync::Arc,
};

pub trait Matcher: Debug {
    fn matches<'a>(&self, input: &'a str, pos: &mut usize) -> Option<usize>;
    fn display(&self) -> String;
    fn is_nullable(&self) -> bool;
    fn is_consuming(&self) -> bool;
    fn preview(&self) -> Option<&str> {
        None
    }
    fn then<U>(self, other: U) -> Sequence<Self, U>
    where
        Self: Sized,
        U: Matcher,
    {
        Sequence(self, other)
    }

    fn or<U>(self, other: U) -> Alternative<Self, U>
    where
        Self: Sized,
        U: Matcher,
    {
        Alternative(self, other)
    }

    fn times<R>(self, range: R) -> Repeat<Self, R>
    where
        Self: Sized,
        R: ops::RangeBounds<usize>,
    {
        Repeat(self, range)
    }
}

pub type MatcherRef = Arc<dyn Matcher + Send + Sync + 'static>;

#[derive(Debug, Clone, Copy)]
pub struct EndOfInput;

#[derive(Debug, Clone, Copy)]
pub struct StartOfInput;

#[derive(Debug, Clone, Copy)]
pub struct Alternative<T, U>(pub T, pub U);

#[derive(Debug, Clone, Copy)]
pub struct Sequence<T, U>(pub T, pub U);

#[derive(Debug, Clone, Copy)]
pub struct Repeat<T, R: ops::RangeBounds<usize>>(pub T, pub R);

#[derive(Debug, Clone, Copy)]
pub struct NamedMatcher<M: Matcher> {
    pub name: &'static str,
    pub matcher: M,
}

impl NamedMatcher<EndOfInput> {
    pub const fn new<M: Matcher>(name: &'static str, matcher: M) -> NamedMatcher<M> {
        NamedMatcher { name, matcher }
    }
}

pub const NUMS: NamedMatcher<Repeat<fn(char) -> bool, ops::RangeFrom<usize>>> =
    NamedMatcher::new("number", Repeat(|c: char| c.is_ascii_digit(), 1..));

pub const ALPHAS: NamedMatcher<Repeat<fn(char) -> bool, ops::RangeFrom<usize>>> =
    NamedMatcher::new("identifier", Repeat(|c: char| c.is_ascii_alphabetic(), 1..));

pub const ALPHANUMS: NamedMatcher<Repeat<fn(char) -> bool, ops::RangeFrom<usize>>> =
    NamedMatcher::new(
        "alphanum",
        Repeat(|c: char| c.is_ascii_alphanumeric() || c == '_', 1..),
    );

const fn string_char(c: char) -> bool {
    c != '"' && c != '\n' && c != '\r'
}

pub const STRING: NamedMatcher<Repeat<fn(char) -> bool, ops::RangeFrom<usize>>> =
    NamedMatcher::new("json_string", Repeat(string_char, 0..));

pub const WHITESPACES: NamedMatcher<Repeat<fn(char) -> bool, ops::RangeFrom<usize>>> =
    NamedMatcher::new("whitespaces", Repeat(|c: char| c.is_whitespace(), 1..));

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
        self.0
            .matches(input, pos)
            .or_else(|| self.1.matches(input, pos))
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
}

impl<T: Matcher, U: Matcher> Matcher for Sequence<T, U> {
    fn matches<'a>(&self, input: &'a str, pos: &mut usize) -> Option<usize> {
        let start = *pos;
        self.0.matches(input, pos)?;
        self.1.matches(input, pos)?;
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
}
