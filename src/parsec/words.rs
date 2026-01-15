use std::{
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

pub trait Matcher: Debug {
    fn matches(&self, input: &str, pos: &mut usize) -> bool;
    fn display(&self) -> String;
    fn is_nullable(&self) -> bool;

    fn is_consuming(&self) -> bool;

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

impl Matcher for () {
    fn matches(&self, _input: &str, _pos: &mut usize) -> bool {
        true
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

impl Matcher for &str {
    fn matches(&self, input: &str, pos: &mut usize) -> bool {
        let end_pos = *pos + self.len();
        if end_pos <= input.len() && &input[*pos..end_pos] == *self {
            *pos = end_pos;
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

    fn is_consuming(&self) -> bool {
        self.len() > 0
    }
}

impl Matcher for char {
    fn matches(&self, input: &str, pos: &mut usize) -> bool {
        if let Some(next_char) = input[*pos..].chars().next() {
            if next_char == *self {
                *pos += next_char.len_utf8();
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

    fn is_consuming(&self) -> bool {
        true
    }
}

impl Matcher for EndOfInput {
    fn matches(&self, input: &str, pos: &mut usize) -> bool {
        *pos >= input.len()
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
    fn matches(&self, _input: &str, pos: &mut usize) -> bool {
        *pos == 0
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
    fn matches(&self, input: &str, pos: &mut usize) -> bool {
        let original_position = *pos;
        if self.0.matches(input, pos) {
            true
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
    fn matches(&self, input: &str, pos: &mut usize) -> bool {
        self.0.matches(input, pos) && self.1.matches(input, pos)
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
}

impl<R, T> Matcher for Repeat<T, R>
where
    T: Matcher,
    R: ops::RangeBounds<usize> + Debug,
{
    fn matches(&self, input: &str, pos: &mut usize) -> bool {
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

        while count < max && self.0.matches(input, pos) {
            count += 1;
        }

        if count >= min && count <= max {
            true
        } else {
            *pos = original_position;
            false
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
