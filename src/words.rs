use std::{
    collections::btree_map::Range,
    fmt::Debug,
    ops::{self, Index, IndexMut},
};

use crate::utils::Span;

#[derive(Debug, Clone)]
pub struct EndOfInput;
#[derive(Debug, Clone)]
pub struct StartOfInput;
#[derive(Debug, Clone)]
pub struct Alternative<T, U>(T, U);
#[derive(Debug, Clone)]
pub struct Sequence<T, U>(T, U);
#[derive(Debug, Clone)]
pub struct Repeat<T, R: ops::RangeBounds<usize>>(T, R);

pub trait Lexical<T>
where
    Self: IntoIterator<Item = T>
        + Index<usize, Output = T>
        + Index<ops::Range<usize>, Output = [T]>
        + IndexMut<usize, Output = T>
        + IndexMut<ops::Range<usize>, Output = [T]>,
    for<'a> &'a Self: IntoIterator<Item = &'a T>,
    T: Clone + PartialEq + Eq,
{
    fn len(&self) -> usize;
    fn span(&self) -> Span {
        Span {
            start: 0,
            end: self.len(),
        }
    }
    fn slice(&self, span: Span) -> &[T] {
        &self[span.start..span.end]
    }
    fn slice_mut(&mut self, span: Span) -> &mut [T] {
        &mut self[span.start..span.end]
    }
}

impl<T: Clone + PartialEq + Eq> Lexical<T> for Vec<T> {
    fn len(&self) -> usize {
        self.len()
    }
}

pub struct State<'a> {
    input: &'a str,
    position: usize,
}

pub trait Matcher: Debug {
    fn matches(&self, state: &mut State) -> bool;
    fn display(&self) -> String {
        String::from("<terminal>")
    }
    fn is_nullable(&self) -> bool;

    fn is_consuming(&self) -> bool {
        !self.is_nullable()
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

impl Matcher for &str {
    fn matches(&self, state: &mut State) -> bool {
        let end_pos = state.position + self.len();
        if end_pos <= state.input.len() && &state.input[state.position..end_pos] == *self {
            state.position = end_pos;
            true
        } else {
            false
        }
    }

    fn display(&self) -> String {
        format!("\"{}\"", self)
    }

    fn is_nullable(&self) -> bool {
        self.len() == 0
    }
}

impl Matcher for char {
    fn matches(&self, state: &mut State) -> bool {
        if let Some(next_char) = state.input[state.position..].chars().next() {
            if next_char == *self {
                state.position += next_char.len_utf8();
                return true;
            }
        }
        false
    }

    fn display(&self) -> String {
        format!("'{}'", self)
    }

    fn is_nullable(&self) -> bool {
        false
    }
}

impl Matcher for EndOfInput {
    fn matches(&self, state: &mut State) -> bool {
        state.position >= state.input.len()
    }

    fn display(&self) -> String {
        String::from("EOF")
    }

    fn is_nullable(&self) -> bool {
        true
    }
}

impl Matcher for StartOfInput {
    fn matches(&self, state: &mut State) -> bool {
        state.position == 0
    }

    fn display(&self) -> String {
        String::from("SOF")
    }

    fn is_nullable(&self) -> bool {
        true
    }
}

impl<T, U> Matcher for Alternative<T, U>
where
    T: Matcher,
    U: Matcher,
{
    fn matches(&self, state: &mut State) -> bool {
        let original_position = state.position;
        if self.0.matches(state) {
            true
        } else {
            state.position = original_position;
            self.1.matches(state)
        }
    }
    fn is_nullable(&self) -> bool {
        self.0.is_nullable() || self.1.is_nullable()
    }
}

impl<T, U> Matcher for Sequence<T, U>
where
    T: Matcher,
    U: Matcher,
{
    fn matches(&self, state: &mut State) -> bool {
        self.0.matches(state) && self.1.matches(state)
    }
    fn is_nullable(&self) -> bool {
        self.0.is_nullable() && self.1.is_nullable()
    }
}

impl<R, T> Matcher for Repeat<T, R>
where
    T: Matcher,
    R: ops::RangeBounds<usize> + Debug,
{
    fn matches(&self, state: &mut State) -> bool {
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

        let original_position = state.position;
        let mut count = 0;

        while count < max && self.0.matches(state) {
            count += 1;
        }

        if count >= min && count <= max {
            true
        } else {
            state.position = original_position;
            false
        }
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
}
