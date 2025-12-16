use std::ops;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub fn new(start: usize, end: usize) -> Self {
        Span { start, end }
    }
    pub fn new_len(offset: usize, len: usize) -> Self {
        Span {
            start: offset,
            end: offset + len,
        }
    }
    pub fn empty() -> Self {
        Span { start: 0, end: 0 }
    }
    pub fn len(&self) -> usize {
        self.end - self.start
    }
}

impl ops::Add for Span {
    type Output = Span;

    fn add(self, other: Span) -> Span {
        Span {
            start: self.start,
            end: other.end,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Range {
    pub start: usize,
    pub end: Option<usize>,
}

impl<R: ops::RangeBounds<usize>> From<R> for Range {
    fn from(range: R) -> Self {
        let start = match range.start_bound() {
            ops::Bound::Included(&s) => s,
            ops::Bound::Excluded(&s) => s + 1,
            ops::Bound::Unbounded => 0,
        };
        let end = match range.end_bound() {
            ops::Bound::Included(&e) => Some(e + 1),
            ops::Bound::Excluded(&e) => Some(e),
            ops::Bound::Unbounded => None,
        };
        Range { start, end }
    }
}

impl From<Range> for ops::RangeFrom<usize> {
    fn from(range: Range) -> Self {
        ops::RangeFrom { start: range.start }
    }
}

pub(crate) trait Unzip<A, B> {
    fn unzip2(self) -> (Vec<A>, Vec<B>);
}
