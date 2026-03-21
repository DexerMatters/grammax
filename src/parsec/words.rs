use std::{
    fmt::Debug,
    ops,
    sync::{Arc, OnceLock},
};

use regex::Regex;
use serde::{Deserialize, Serialize};

#[typetag::serde(tag = "matcher_type")]
pub trait Matcher: Debug + Send + Sync {
    fn matches(&self, input: &str, pos: &mut usize) -> Option<usize>;

    fn display(&self) -> String;

    fn is_nullable(&self) -> bool;

    fn is_consuming(&self) -> bool;

    fn preview(&self) -> Option<&str> {
        None
    }

    fn then<U>(self, other: U) -> Sequence
    where
        Self: Sized + 'static,
        U: IntoMatcher,
    {
        Sequence::new(self, other)
    }

    fn or<U>(self, other: U) -> Alternative
    where
        Self: Sized + 'static,
        U: IntoMatcher,
    {
        Alternative::new(self, other)
    }

    fn repeat<R>(self, range: R) -> Repeat
    where
        Self: Sized + 'static,
        R: ops::RangeBounds<usize>,
    {
        Repeat::new(self, range)
    }

    fn times(self, n: usize) -> Repeat
    where
        Self: Sized + 'static,
    {
        Repeat::new(self, n..=n)
    }
}

pub type MatcherRef = Arc<dyn Matcher + Send + Sync + 'static>;
pub type MatcherBox = Box<dyn Matcher + Send + Sync + 'static>;

pub trait IntoMatcher {
    fn into_matcher_box(self) -> MatcherBox;

    fn into_matcher_ref(self) -> MatcherRef
    where
        Self: Sized,
    {
        Arc::from(self.into_matcher_box())
    }
}

impl<M> IntoMatcher for M
where
    M: Matcher + Send + Sync + 'static,
{
    fn into_matcher_box(self) -> MatcherBox {
        Box::new(self)
    }
}

impl IntoMatcher for MatcherBox {
    fn into_matcher_box(self) -> MatcherBox {
        self
    }
}

impl IntoMatcher for &str {
    fn into_matcher_box(self) -> MatcherBox {
        Box::new(OwnedLiteral(self.to_string()))
    }
}

impl IntoMatcher for String {
    fn into_matcher_box(self) -> MatcherBox {
        Box::new(OwnedLiteral(self))
    }
}

impl IntoMatcher for char {
    fn into_matcher_box(self) -> MatcherBox {
        Box::new(CharLiteral {
            ch: self,
            ch_str: self.to_string(),
        })
    }
}

impl IntoMatcher for () {
    fn into_matcher_box(self) -> MatcherBox {
        Box::new(EmptyMatcher)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct EndOfInput;

#[derive(Debug, Serialize, Deserialize)]
pub struct Alternative {
    pub left: MatcherBox,
    pub right: MatcherBox,
}

impl Alternative {
    pub fn new<L: IntoMatcher, R: IntoMatcher>(left: L, right: R) -> Self {
        Self {
            left: left.into_matcher_box(),
            right: right.into_matcher_box(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Sequence {
    pub first: MatcherBox,
    pub second: MatcherBox,
}

impl Sequence {
    pub fn new<L: IntoMatcher, R: IntoMatcher>(first: L, second: R) -> Self {
        Self {
            first: first.into_matcher_box(),
            second: second.into_matcher_box(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Repeat {
    pub matcher: MatcherBox,
    pub min: usize,
    pub max: Option<usize>,
}

impl Repeat {
    pub fn new<M: IntoMatcher, R: ops::RangeBounds<usize>>(matcher: M, range: R) -> Self {
        let min = match range.start_bound() {
            ops::Bound::Included(&n) => n,
            ops::Bound::Excluded(&n) => n + 1,
            ops::Bound::Unbounded => 0,
        };
        let max = match range.end_bound() {
            ops::Bound::Included(&n) => Some(n),
            ops::Bound::Excluded(&n) => Some(n.saturating_sub(1)),
            ops::Bound::Unbounded => None,
        };
        Self {
            matcher: matcher.into_matcher_box(),
            min,
            max,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct NamedMatcher {
    pub name: String,
    pub matcher: MatcherBox,
}

impl NamedMatcher {
    pub fn new(name: impl Into<String>, matcher: impl IntoMatcher) -> NamedMatcher {
        NamedMatcher {
            name: name.into(),
            matcher: matcher.into_matcher_box(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct OneOf(pub &'static str);

impl Serialize for OneOf {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.0)
    }
}

impl<'de> Deserialize<'de> for OneOf {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let chars = String::deserialize(deserializer)?;
        Ok(OneOf(Box::leak(chars.into_boxed_str())))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegexMatcher {
    pub pattern: String,
    #[serde(skip, default)]
    compiled: OnceLock<Regex>,
    #[serde(skip, default)]
    is_nullable: OnceLock<bool>,
    #[serde(skip, default)]
    is_consuming: OnceLock<bool>,
}

impl RegexMatcher {
    pub fn new(pattern: &str) -> Self {
        Regex::new(pattern).unwrap();
        Self {
            pattern: pattern.to_string(),
            compiled: OnceLock::new(),
            is_nullable: OnceLock::new(),
            is_consuming: OnceLock::new(),
        }
    }

    fn compiled(&self) -> &Regex {
        self.compiled
            .get_or_init(|| Regex::new(&self.pattern).expect("invalid regex pattern"))
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct IntegerMatcher;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct FloatMatcher;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct IdentMatcher;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct CharMatcherWithEscapes;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct NumberMatcher;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct OctalNumberMatcher;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct HexNumberMatcher;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct AlphabetsMatcher;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct AlphanumberMatcher;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct StringMatcher;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct WhitespacesMatcher;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OwnedLiteral(pub String);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CharLiteral {
    pub ch: char,
    ch_str: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct EmptyMatcher;

#[derive(Debug, Serialize, Deserialize)]
pub struct TokenizedMatcher {
    pub inner: MatcherBox,
}

#[typetag::serde]
impl Matcher for RegexMatcher {
    fn matches(&self, input: &str, pos: &mut usize) -> Option<usize> {
        let start = *pos;
        if let Some(mat) = self.compiled().find(&input[*pos..]) {
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
        *self
            .is_nullable
            .get_or_init(|| self.compiled().is_match(""))
    }

    fn is_consuming(&self) -> bool {
        *self
            .is_consuming
            .get_or_init(|| !self.compiled().is_match(""))
    }
}

#[typetag::serde]
impl Matcher for IntegerMatcher {
    fn matches(&self, input: &str, pos: &mut usize) -> Option<usize> {
        let start = *pos;
        let remaining = &input[*pos..];

        if remaining.starts_with("0x") || remaining.starts_with("0X") {
            *pos += 2;
            if HEX_NUMBER.matches(input, pos).is_some() {
                return Some(*pos - start);
            }
            *pos = start;
            return None;
        }

        if remaining.starts_with('0') && remaining.len() > 1 {
            if let Some(next_char) = remaining[1..].chars().next() {
                if next_char.is_ascii_digit() && next_char < '8' {
                    *pos += 1;
                    if OCTAL_NUMBER.matches(input, pos).is_some() {
                        return Some(*pos - start);
                    }
                }
            }
        }

        if NUMBER.matches(input, pos).is_some() {
            return Some(*pos - start);
        }

        *pos = start;
        None
    }

    fn display(&self) -> String {
        String::from("integer")
    }

    fn is_nullable(&self) -> bool {
        false
    }

    fn is_consuming(&self) -> bool {
        true
    }
}

#[typetag::serde]
impl Matcher for FloatMatcher {
    fn matches(&self, input: &str, pos: &mut usize) -> Option<usize> {
        let start = *pos;
        let mut temp_pos = *pos;
        let _ = NUMBER.matches(input, &mut temp_pos);

        if temp_pos >= input.len() || input.chars().nth(temp_pos).unwrap_or(' ') != '.' {
            return None;
        }

        temp_pos += 1;

        if NUMBER.matches(input, &mut temp_pos).is_none() {
            return None;
        }

        *pos = temp_pos;
        Some(*pos - start)
    }

    fn display(&self) -> String {
        String::from("float")
    }

    fn is_nullable(&self) -> bool {
        false
    }

    fn is_consuming(&self) -> bool {
        true
    }
}

#[typetag::serde]
impl Matcher for IdentMatcher {
    fn matches(&self, input: &str, pos: &mut usize) -> Option<usize> {
        let start = *pos;

        if let Some(first_char) = input[*pos..].chars().next() {
            if !first_char.is_ascii_alphabetic() && first_char != '_' {
                return None;
            }
            *pos += first_char.len_utf8();
        } else {
            return None;
        }

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

#[typetag::serde]
impl Matcher for CharMatcherWithEscapes {
    fn matches(&self, input: &str, pos: &mut usize) -> Option<usize> {
        let start = *pos;

        if let Some(first_char) = input[*pos..].chars().next() {
            if first_char == '\\' {
                *pos += first_char.len_utf8();
                if let Some(next_char) = input[*pos..].chars().next() {
                    match next_char {
                        'n' | 't' | 'r' | '\\' | '"' | '\'' | 'b' | 'f' | 'v' | '0' | 'x' | 'u' => {
                            *pos += next_char.len_utf8();
                            Some(*pos - start)
                        }
                        _ => {
                            *pos = start;
                            None
                        }
                    }
                } else {
                    *pos = start;
                    None
                }
            } else if first_char != '"' && first_char != '\n' && first_char != '\r' {
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

#[typetag::serde]
impl Matcher for NumberMatcher {
    fn matches(&self, input: &str, pos: &mut usize) -> Option<usize> {
        match_one_or_more(input, pos, |c| c.is_ascii_digit())
    }

    fn display(&self) -> String {
        String::from("number")
    }

    fn is_nullable(&self) -> bool {
        false
    }

    fn is_consuming(&self) -> bool {
        true
    }
}

#[typetag::serde]
impl Matcher for OctalNumberMatcher {
    fn matches(&self, input: &str, pos: &mut usize) -> Option<usize> {
        match_one_or_more(input, pos, |c| c.is_ascii_digit() && c < '8')
    }

    fn display(&self) -> String {
        String::from("octal_number")
    }

    fn is_nullable(&self) -> bool {
        false
    }

    fn is_consuming(&self) -> bool {
        true
    }
}

#[typetag::serde]
impl Matcher for HexNumberMatcher {
    fn matches(&self, input: &str, pos: &mut usize) -> Option<usize> {
        match_one_or_more(input, pos, |c| c.is_ascii_hexdigit())
    }

    fn display(&self) -> String {
        String::from("hex_number")
    }

    fn is_nullable(&self) -> bool {
        false
    }

    fn is_consuming(&self) -> bool {
        true
    }
}

#[typetag::serde]
impl Matcher for AlphabetsMatcher {
    fn matches(&self, input: &str, pos: &mut usize) -> Option<usize> {
        match_one_or_more(input, pos, |c| c.is_ascii_alphabetic())
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

#[typetag::serde]
impl Matcher for AlphanumberMatcher {
    fn matches(&self, input: &str, pos: &mut usize) -> Option<usize> {
        match_one_or_more(input, pos, |c| c.is_ascii_alphanumeric() || c == '_')
    }

    fn display(&self) -> String {
        String::from("alphanum")
    }

    fn is_nullable(&self) -> bool {
        false
    }

    fn is_consuming(&self) -> bool {
        true
    }
}

#[typetag::serde]
impl Matcher for StringMatcher {
    fn matches(&self, input: &str, pos: &mut usize) -> Option<usize> {
        let start = *pos;
        let esc = CharMatcherWithEscapes;
        loop {
            let before = *pos;
            if esc.matches(input, pos).is_some() {
                continue;
            }
            *pos = before;
            break;
        }
        Some(*pos - start)
    }

    fn display(&self) -> String {
        String::from("string")
    }

    fn is_nullable(&self) -> bool {
        true
    }

    fn is_consuming(&self) -> bool {
        true
    }
}

#[typetag::serde]
impl Matcher for WhitespacesMatcher {
    fn matches(&self, input: &str, pos: &mut usize) -> Option<usize> {
        match_one_or_more(input, pos, |c| c.is_whitespace())
    }

    fn display(&self) -> String {
        String::from("whitespaces")
    }

    fn is_nullable(&self) -> bool {
        false
    }

    fn is_consuming(&self) -> bool {
        true
    }
}

#[typetag::serde]
impl Matcher for EmptyMatcher {
    fn matches(&self, _input: &str, _pos: &mut usize) -> Option<usize> {
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

#[typetag::serde]
impl Matcher for OneOf {
    fn matches(&self, input: &str, pos: &mut usize) -> Option<usize> {
        let start = *pos;
        if let Some(next_char) = input[*pos..].chars().next() {
            if self.0.contains(next_char) {
                *pos += next_char.len_utf8();
                return Some(*pos - start);
            }
        }
        None
    }

    fn display(&self) -> String {
        format!("one_of(\"{}\")", self.0)
    }

    fn is_nullable(&self) -> bool {
        false
    }

    fn is_consuming(&self) -> bool {
        true
    }
}

#[typetag::serde]
impl Matcher for OwnedLiteral {
    fn matches(&self, input: &str, pos: &mut usize) -> Option<usize> {
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

#[typetag::serde]
impl Matcher for CharLiteral {
    fn matches(&self, input: &str, pos: &mut usize) -> Option<usize> {
        let start = *pos;
        if let Some(next_char) = input[*pos..].chars().next() {
            if next_char == self.ch {
                *pos += next_char.len_utf8();
                return Some(*pos - start);
            }
        }
        None
    }

    fn display(&self) -> String {
        format!("'{}'", self.ch)
    }

    fn is_nullable(&self) -> bool {
        false
    }

    fn is_consuming(&self) -> bool {
        true
    }

    fn preview(&self) -> Option<&str> {
        Some(&self.ch_str)
    }
}

#[typetag::serde]
impl Matcher for EndOfInput {
    fn matches(&self, input: &str, pos: &mut usize) -> Option<usize> {
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

#[typetag::serde]
impl Matcher for Alternative {
    fn matches(&self, input: &str, pos: &mut usize) -> Option<usize> {
        let start = *pos;
        if let Some(len) = self.left.matches(input, pos) {
            return Some(len);
        }
        *pos = start;
        self.right.matches(input, pos)
    }

    fn display(&self) -> String {
        format!("({} | {})", self.left.display(), self.right.display())
    }

    fn is_nullable(&self) -> bool {
        self.left.is_nullable() || self.right.is_nullable()
    }

    fn is_consuming(&self) -> bool {
        self.left.is_consuming() && self.right.is_consuming()
    }

    fn preview(&self) -> Option<&str> {
        let left = self.left.preview();
        let right = self.right.preview();
        if left.is_some() && left == right {
            left
        } else {
            None
        }
    }
}

#[typetag::serde]
impl Matcher for Sequence {
    fn matches(&self, input: &str, pos: &mut usize) -> Option<usize> {
        let start = *pos;
        if self.first.matches(input, pos).is_none() {
            return None;
        }
        if self.second.matches(input, pos).is_none() {
            *pos = start;
            return None;
        }
        Some(*pos - start)
    }

    fn display(&self) -> String {
        format!("{} {}", self.first.display(), self.second.display())
    }

    fn is_nullable(&self) -> bool {
        self.first.is_nullable() && self.second.is_nullable()
    }

    fn is_consuming(&self) -> bool {
        self.first.is_consuming() || self.second.is_consuming()
    }

    fn preview(&self) -> Option<&str> {
        if self.first.is_nullable() {
            self.second.preview().or_else(|| self.first.preview())
        } else {
            self.first.preview()
        }
    }
}

#[typetag::serde]
impl Matcher for Repeat {
    fn matches(&self, input: &str, pos: &mut usize) -> Option<usize> {
        let start = *pos;
        let mut count = 0;

        while self.max.map_or(true, |m| count < m) {
            let before = *pos;
            match self.matcher.matches(input, pos) {
                Some(len) if len > 0 => {
                    count += 1;
                }
                Some(_) => {
                    *pos = before;
                    break;
                }
                None => {
                    *pos = before;
                    break;
                }
            }
        }

        if count >= self.min {
            Some(*pos - start)
        } else {
            *pos = start;
            None
        }
    }

    fn display(&self) -> String {
        format!("{}*", self.matcher.display())
    }

    fn is_nullable(&self) -> bool {
        self.min == 0
    }

    fn is_consuming(&self) -> bool {
        self.matcher.is_consuming()
    }

    fn preview(&self) -> Option<&str> {
        if self.min == 0 {
            None
        } else {
            self.matcher.preview()
        }
    }
}

#[typetag::serde]
impl Matcher for NamedMatcher {
    fn matches(&self, input: &str, pos: &mut usize) -> Option<usize> {
        self.matcher.matches(input, pos)
    }

    fn display(&self) -> String {
        self.name.clone()
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

#[typetag::serde]
impl Matcher for TokenizedMatcher {
    fn matches(&self, input: &str, pos: &mut usize) -> Option<usize> {
        let start = *pos;
        consume_while(input, pos, |c| c.is_whitespace() || c == '\n' || c == '\r');
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

pub const NUMBER: NumberMatcher = NumberMatcher;
pub const OCTAL_NUMBER: OctalNumberMatcher = OctalNumberMatcher;
pub const HEX_NUMBER: HexNumberMatcher = HexNumberMatcher;
pub const IDENT: IdentMatcher = IdentMatcher;
pub const INTEGER: IntegerMatcher = IntegerMatcher;
pub const FLOAT: FloatMatcher = FloatMatcher;
pub const ALPHABETS: AlphabetsMatcher = AlphabetsMatcher;
pub const ALPHANUMBER: AlphanumberMatcher = AlphanumberMatcher;
pub const EOF: EndOfInput = EndOfInput;
pub const STRING: StringMatcher = StringMatcher;
pub const WHITESPACES: WhitespacesMatcher = WhitespacesMatcher;

pub const DIGIT: OneOf = OneOf("0123456789");
pub const OCTAL_DIGIT: OneOf = OneOf("01234567");
pub const HEX_DIGIT: OneOf = OneOf("0123456789abcdefABCDEF");

pub fn regex(pattern: &str) -> RegexMatcher {
    RegexMatcher::new(pattern)
}

pub fn named<M: IntoMatcher>(name: impl Into<String>, matcher: M) -> NamedMatcher {
    NamedMatcher::new(name, matcher)
}

pub fn token<M: IntoMatcher>(matcher: M) -> TokenizedMatcher {
    TokenizedMatcher {
        inner: matcher.into_matcher_box(),
    }
}

fn consume_while<F>(input: &str, pos: &mut usize, mut predicate: F) -> usize
where
    F: FnMut(char) -> bool,
{
    let start = *pos;
    while *pos < input.len() {
        if let Some(c) = input[*pos..].chars().next() {
            if predicate(c) {
                *pos += c.len_utf8();
            } else {
                break;
            }
        } else {
            break;
        }
    }
    *pos - start
}

fn match_one_or_more<F>(input: &str, pos: &mut usize, mut predicate: F) -> Option<usize>
where
    F: FnMut(char) -> bool,
{
    let start = *pos;
    let len = consume_while(input, pos, |c| predicate(c));
    if len > 0 {
        Some(*pos - start)
    } else {
        *pos = start;
        None
    }
}
