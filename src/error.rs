//! Error types for the legible crate.

use thiserror::Error;

/// Errors that can occur during article parsing.
///
/// These errors are returned by [`parse()`](crate::parse) when content extraction fails.
///
/// # Example
///
/// ```rust
/// use legible::{parse, Error};
///
/// let html = "<html><body></body></html>";
///
/// match parse(html, None, None) {
///     Ok(article) => println!("Success: {}", article.title),
///     Err(Error::NoContent) => println!("No article content found"),
///     Err(Error::NoBody) => println!("Document has no body"),
///     Err(e) => println!("Other error: {}", e),
/// }
/// ```
#[derive(Error, Debug)]
pub enum Error {
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
    /// This error is returned when the URL provided to [`parse()`](crate::parse)
    /// is invalid and cannot be parsed.
    #[error("Invalid URL: {0}")]
    InvalidUrl(#[from] url::ParseError),
}

/// Result type alias for readability operations.
pub type Result<T> = std::result::Result<T, Error>;
