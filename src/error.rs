//! Errors from article extraction.

use thiserror::Error;

/// An HTML parser failure.
#[derive(Error, Debug)]
#[error("{message}")]
pub struct ParseError {
    pub(crate) message: String,
}
impl ParseError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

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
    /// Extraction configuration is invalid.
    #[error("invalid extraction configuration: {message}")]
    InvalidConfiguration { message: String },

    /// HTML parsing could not create a document tree.
    #[error("HTML parsing failed: {0}")]
    Parse(#[from] ParseError),

    /// The input exceeds the configured byte limit.
    #[error("document input is {actual} bytes; limit is {limit}")]
    InputTooLarge { actual: usize, limit: usize },

    /// The document exceeds the configured element limit.
    ///
    #[error("document contains more than {limit} elements")]
    TooManyElements { limit: usize },

    /// Legible cannot extract nonempty article content.
    ///
    /// The document can have too little readable text, or its structure can have no
    /// identifiable article.
    #[error("no readable article content was found")]
    NoContent,

    /// The parsed document has no `<body>` element.
    #[error("the document has no body")]
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
