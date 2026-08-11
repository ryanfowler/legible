//! Errors from content extraction.

use thiserror::Error;

/// Errors from content extraction.
///
/// # Example
///
/// ```rust
/// use legible::{Error, extract};
///
/// match extract("<html><body></body></html>", None) {
///     Ok(page) => println!("Text: {}", page.text()),
///     Err(Error::NoContent) => println!("The document has no relevant content."),
///     Err(Error::NoBody) => println!("The document has no body."),
///     Err(error) => println!("Extraction error: {error}"),
/// }
/// ```
#[derive(Error, Debug)]
pub enum Error {
    /// The document exceeds the configured element limit.
    ///
    /// The first value is the number of HTML elements in the document. The second value
    /// is the limit set by [`ExtractorBuilder::max_elements`](crate::ExtractorBuilder::max_elements).
    #[error("Aborting parsing document; {0} elements found (max: {1})")]
    TooManyElements(usize, usize),

    /// Legible cannot extract nonempty relevant content.
    ///
    /// The document can have too little readable text, or its structure can have no
    /// identifiable content region.
    #[error("Failed to extract relevant content from the document")]
    NoContent,

    /// The parsed document has no `<body>` element.
    #[error("No body found in document")]
    NoBody,

    /// The specified base URL is invalid.
    ///
    /// The `url` parameter of [`extract()`](crate::extract) or
    /// [`Extractor::extract`](crate::Extractor::extract) caused this error.
    #[error("Invalid URL: {0}")]
    InvalidUrl(#[from] url::ParseError),

    /// The configured exact content root does not match an element.
    #[error("The requested content root was not found")]
    ContentRootNotFound,
}

/// A result from content extraction.
pub type Result<T> = std::result::Result<T, Error>;
