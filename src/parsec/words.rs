use std::{
    fmt::Debug,
    ops::{self},
    sync::Arc,
};

#[derive(Debug, Clone, Copy)]
pub struct EndOfInput;
#[derive(Debug, Clone, Copy)]
pub struct StartOfInput;
#[derive(Debug, Clone, Copy)]
pub struct Alternative<T, U>(T, U);
#[derive(Debug, Clone, Copy)]
pub struct Sequence<T, U>(T, U);
#[derive(Debug, Clone, Copy)]
pub struct Repeat<T, R: ops::RangeBounds<usize>>(T, R);

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

pub trait Matcher: Debug {
    fn matches<'a>(&self, input: &'a str, pos: &mut usize) -> Option<usize>;
    fn display(&self) -> String;
    fn is_nullable(&self) -> bool;
    fn is_consuming(&self) -> bool;
    /// Returns the literal string this matcher matches, if it's a literal.
    /// Variable matchers (char predicates, ranges) return None.
    fn preview(&self) -> Option<String> {
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

    fn preview(&self) -> Option<String> {
        Some(self.to_string())
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

    fn preview(&self) -> Option<String> {
        Some(self.to_string())
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
        true
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
        true
    }

    fn is_consuming(&self) -> bool {
        false
    }
}

impl<T, U> Matcher for Alternative<T, U>
where
    T: Matcher,
    U: Matcher,
{
    fn matches<'a>(&self, input: &'a str, pos: &mut usize) -> Option<usize> {
        let original_position = *pos;
        if let Some(matched) = self.0.matches(input, pos) {
            Some(matched)
        } else {
            *pos = original_position;
            self.1.matches(input, pos)
        }
    }
    fn display(&self) -> String {
        format!("({}/{})", self.0.display(), self.1.display())
    }
    fn is_nullable(&self) -> bool {
        self.0.is_nullable() || self.1.is_nullable()
    }
    fn is_consuming(&self) -> bool {
        self.0.is_consuming() || self.1.is_consuming()
    }
}

impl<T, U> Matcher for Sequence<T, U>
where
    T: Matcher,
    U: Matcher,
{
    fn matches<'a>(&self, input: &'a str, pos: &mut usize) -> Option<usize> {
        let original_position = *pos;
        if let Some(matched1) = self.0.matches(input, pos) {
            if let Some(matched2) = self.1.matches(input, pos) {
                Some(matched1 + matched2)
            } else {
                *pos = original_position;
                None
            }
        } else {
            *pos = original_position;
            None
        }
    }
    fn display(&self) -> String {
        format!("({} {})", self.0.display(), self.1.display())
    }
    fn is_nullable(&self) -> bool {
        self.0.is_nullable() && self.1.is_nullable()
    }
    fn is_consuming(&self) -> bool {
        self.0.is_consuming() || self.1.is_consuming()
    }
    fn preview(&self) -> Option<String> {
        self.0.preview().or(self.1.preview())
    }
}

impl<R, T> Matcher for Repeat<T, R>
where
    T: Matcher,
    R: ops::RangeBounds<usize> + Debug,
{
    fn matches<'a>(&self, input: &'a str, pos: &mut usize) -> Option<usize> {
        use std::ops::Bound;

        let min = match self.1.start_bound() {
            Bound::Included(&n) => n,
            Bound::Excluded(&n) => n + 1,
            Bound::Unbounded => 0,
        };

        let max = match self.1.end_bound() {
            Bound::Included(&n) => n,
            Bound::Excluded(&n) => n.saturating_sub(1),
            Bound::Unbounded => usize::MAX,
        };

        let original_position = *pos;
        let mut count = 0;
        let mut stopped_at_eof = false;

        while count < max {
            let before = *pos;
            if self.0.matches(input, pos).is_some() {
                if *pos == before {
                    break;
                }
                count += 1;
            } else {
                if *pos >= input.len() {
                    stopped_at_eof = true;
                }
                break;
            }
        }

        if max == usize::MAX && stopped_at_eof && self.0.is_consuming() {
            // *pos = original_position;
            // return None;
        }

        if count >= min && count <= max {
            Some(*pos - original_position)
        } else {
            *pos = original_position;
            None
        }
    }
    fn display(&self) -> String {
        format!("({:?} x {:?})", self.0.display(), self.1)
    }
    fn is_nullable(&self) -> bool {
        use std::ops::Bound;

        let min = match self.1.start_bound() {
            Bound::Included(&n) => n,
            Bound::Excluded(&n) => n + 1,
            Bound::Unbounded => 0,
        };

        min == 0 || self.0.is_nullable()
    }
    fn is_consuming(&self) -> bool {
        use std::ops::Bound;

        let min = match self.1.start_bound() {
            Bound::Included(&n) => n,
            Bound::Excluded(&n) => n + 1,
            Bound::Unbounded => 0,
        };

        min > 0 && self.0.is_consuming()
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

    fn preview(&self) -> Option<String> {
        self.matcher.preview()
    }
}
