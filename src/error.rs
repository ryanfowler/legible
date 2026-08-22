//! Errors from content extraction.

use thiserror::Error;

use crate::dom::{ParseError, ParseLimitKind};

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

    /// The input exceeds a configured parser or structured-data resource limit.
    ///
    /// `resource` identifies the limited resource. `limit` is the configured
    /// maximum. The error does not always include the observed value because
    /// some limits stop work before the full value is known.
    #[error("Aborting parsing document; {resource} limit exceeded (max: {limit})")]
    ResourceLimit {
        /// Name of the resource that exceeded its limit.
        resource: &'static str,
        /// Configured maximum for the resource.
        limit: usize,
    },

    /// The input could not be converted into Legible's internal DOM.
    #[error("Failed to parse document: {0}")]
    Parse(String),

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

impl Error {
    pub(crate) fn from_parse_error(error: ParseError) -> Self {
        match error {
            ParseError::Dom(error) => Self::Parse(error.to_string()),
            ParseError::Limit(limit) => match limit.kind {
                ParseLimitKind::Elements => Self::TooManyElements(limit.observed, limit.limit),
                ParseLimitKind::Nodes => Self::ResourceLimit {
                    resource: "DOM nodes",
                    limit: limit.limit,
                },
                ParseLimitKind::TotalAttributes => Self::ResourceLimit {
                    resource: "total attributes",
                    limit: limit.limit,
                },
                ParseLimitKind::AttributesPerElement => Self::ResourceLimit {
                    resource: "attributes per element",
                    limit: limit.limit,
                },
                ParseLimitKind::TextBytes => Self::ResourceLimit {
                    resource: "text bytes",
                    limit: limit.limit,
                },
                ParseLimitKind::Depth => Self::ResourceLimit {
                    resource: "element depth",
                    limit: limit.limit,
                },
            },
        }
    }

    pub(crate) fn resource_limit(resource: &'static str, limit: usize) -> Self {
        Self::ResourceLimit { resource, limit }
    }
}
