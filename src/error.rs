//! Error types for the legible crate.

use thiserror::Error;

/// Errors that can occur during article parsing.
///
/// These errors are returned by [`Readability::parse()`](crate::Readability::parse) when
/// content extraction fails.
///
/// # Example
///
/// ```rust
/// use legible::{Readability, ReadabilityError};
///
/// let html = "<html><body></body></html>";
/// let readability = Readability::new(html, None, None);
///
/// match readability.parse() {
///     Ok(article) => println!("Success: {}", article.title),
///     Err(ReadabilityError::NoContent) => println!("No article content found"),
///     Err(ReadabilityError::NoBody) => println!("Document has no body"),
///     Err(e) => println!("Other error: {}", e),
/// }
/// ```
#[derive(Error, Debug)]
pub enum ReadabilityError {
    /// The document contains too many elements to parse safely.
    ///
    /// This error is returned when the document exceeds the `max_elems_to_parse` limit
    /// set in [`Options`](crate::Options). The first value is the actual count, the second
    /// is the configured maximum.
    #[error("Aborting parsing document; {0} elements found (max: {1})")]
    TooManyElements(usize, usize),

    /// No article content could be extracted from the document.
    ///
    /// This typically means the document doesn't contain enough readable content,
    /// or the content structure doesn't match expected article patterns.
    #[error("Failed to extract article content from the document")]
    NoContent,

    /// The document has no `<body>` element.
    #[error("No body found in document")]
    NoBody,

    /// URL parsing error.
    ///
    /// This can occur when processing URLs in the document content.
    #[error("Invalid URL: {0}")]
    InvalidUrl(#[from] url::ParseError),

    /// JSON parsing error (for JSON-LD metadata).
    ///
    /// This occurs when the document contains malformed JSON-LD metadata.
    /// Note: JSON-LD errors don't prevent content extraction; the metadata
    /// is simply skipped.
    #[error("JSON parsing error: {0}")]
    JsonError(#[from] serde_json::Error),
}

/// Result type alias for readability operations.
pub type Result<T> = std::result::Result<T, ReadabilityError>;
