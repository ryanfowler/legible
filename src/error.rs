//! Errors from article extraction.

use thiserror::Error;

/// Errors from article extraction.
///
/// [`parse()`](crate::parse) and [`Document::parse`](crate::Document::parse) return these
/// errors.
///
/// # Example
///
/// ```rust
/// use legible::{Error, parse};
///
/// match parse("<html><body></body></html>", None, None) {
///     Ok(article) => println!("Title: {}", article.title),
///     Err(Error::NoContent) => println!("The document has no article content."),
///     Err(Error::NoBody) => println!("The document has no body."),
///     Err(error) => println!("Extraction error: {error}"),
/// }
/// ```
#[derive(Error, Debug)]
pub enum Error {
    /// The document exceeds the configured element limit.
    ///
    /// The first value is the number of HTML elements in the document. The second value
    /// is [`Options::max_elems_to_parse`](crate::Options::max_elems_to_parse).
    #[error("Aborting parsing document; {0} elements found (max: {1})")]
    TooManyElements(usize, usize),

    /// Legible cannot extract nonempty article content.
    ///
    /// The document can have too little readable text, or its structure can have no
    /// identifiable article.
    #[error("Failed to extract article content from the document")]
    NoContent,

    /// The parsed document has no `<body>` element.
    #[error("No body found in document")]
    NoBody,

    /// The specified base URL is invalid.
    ///
    /// The `url` parameter of [`parse()`](crate::parse) or
    /// [`Document::parse`](crate::Document::parse) caused this error.
    #[error("Invalid URL: {0}")]
    InvalidUrl(#[from] url::ParseError),
}

/// A result from article extraction.
pub type Result<T> = std::result::Result<T, Error>;
