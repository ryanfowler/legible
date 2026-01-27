//! Error types for the legible crate.

use thiserror::Error;

/// Errors that can occur during article parsing.
#[derive(Error, Debug)]
pub enum ReadabilityError {
    /// The document contains too many elements to parse safely.
    #[error("Aborting parsing document; {0} elements found (max: {1})")]
    TooManyElements(usize, usize),

    /// No article content could be extracted from the document.
    #[error("Failed to extract article content from the document")]
    NoContent,

    /// The document has no body element.
    #[error("No body found in document")]
    NoBody,

    /// URL parsing error.
    #[error("Invalid URL: {0}")]
    InvalidUrl(#[from] url::ParseError),

    /// JSON parsing error (for JSON-LD metadata).
    #[error("JSON parsing error: {0}")]
    JsonError(#[from] serde_json::Error),
}

/// Result type alias for readability operations.
pub type Result<T> = std::result::Result<T, ReadabilityError>;
