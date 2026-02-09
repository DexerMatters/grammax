use crate::utils::Span;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ErrorMessage {
    UnexpectedToken { expected: Vec<usize> },
    MissingToken { expected: Vec<usize> },
    Custom(usize),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ParserMessage {
    pub span: Span,
    pub message: ErrorMessage,
}

impl ParserMessage {
    pub fn new_unexpected(span: Span, expected: Vec<usize>) -> Self {
        Self {
            span,
            message: ErrorMessage::UnexpectedToken { expected },
        }
    }

    pub fn new_missing(span: Span, expected: Vec<usize>) -> Self {
        Self {
            span,
            message: ErrorMessage::MissingToken { expected },
        }
    }
}

pub type ParserMessages = Vec<ParserMessage>;
