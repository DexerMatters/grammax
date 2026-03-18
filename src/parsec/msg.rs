use crate::utils::Span;

/// Error messages generated during parsing, which can be either unexpected tokens, missing tokens, or custom messages.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum ErrorMessage {
    UnexpectedToken { expected: Vec<usize> },
    MissingToken { expected: Vec<usize> },
    Custom(usize),
}

/// A parser message consists of a span in the input text and an error message, used for error reporting and recovery.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct ParserMessage {
    pub span: Span,
    pub message: ErrorMessage,
}

impl ParserMessage {
    /// Creates a new parser message for an unexpected token, with the given span and expected rule indices.
    pub fn new_unexpected(span: Span, expected: Vec<usize>) -> Self {
        Self {
            span,
            message: ErrorMessage::UnexpectedToken { expected },
        }
    }

    /// Creates a new parser message for a missing token, with the given span and expected green rule indices.
    pub fn new_missing(span: Span, expected: Vec<usize>) -> Self {
        Self {
            span,
            message: ErrorMessage::MissingToken { expected },
        }
    }
}

pub(crate) type ParserMessages = Vec<ParserMessage>;
