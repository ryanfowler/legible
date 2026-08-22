//! Errors from content extraction.

use thiserror::Error;

use crate::dom::{ParseError, ParseLimitKind};

/// A parser or structured-data resource with a configurable limit.
///
/// The value identifies the resource in [`Error::ResourceLimit`].
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResourceLimitKind {
    /// Input HTML bytes.
    InputBytes,
    /// Allocated DOM nodes.
    DomNodes,
    /// HTML elements.
    Elements,
    /// Attributes across the document.
    TotalAttributes,
    /// Attributes on one element.
    AttributesPerElement,
    /// Text bytes in the DOM.
    TextBytes,
    /// Element nesting depth.
    ElementDepth,
    /// JSON-LD script bytes.
    JsonLdBytes,
    /// Typed JSON-LD items.
    JsonLdItems,
    /// JSON-LD nesting depth.
    JsonLdDepth,
}

impl ResourceLimitKind {
    /// Returns a stable machine-readable name for the resource.
    pub const fn name(self) -> &'static str {
        match self {
            Self::InputBytes => "input_bytes",
            Self::DomNodes => "dom_nodes",
            Self::Elements => "elements",
            Self::TotalAttributes => "total_attributes",
            Self::AttributesPerElement => "attributes_per_element",
            Self::TextBytes => "text_bytes",
            Self::ElementDepth => "element_depth",
            Self::JsonLdBytes => "json_ld_bytes",
            Self::JsonLdItems => "json_ld_items",
            Self::JsonLdDepth => "json_ld_depth",
        }
    }
}

impl std::fmt::Display for ResourceLimitKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let label = match self {
            Self::InputBytes => "input bytes",
            Self::DomNodes => "DOM nodes",
            Self::Elements => "elements",
            Self::TotalAttributes => "total attributes",
            Self::AttributesPerElement => "attributes per element",
            Self::TextBytes => "text bytes",
            Self::ElementDepth => "element depth",
            Self::JsonLdBytes => "JSON-LD bytes",
            Self::JsonLdItems => "JSON-LD items",
            Self::JsonLdDepth => "JSON-LD depth",
        };
        formatter.write_str(label)
    }
}

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
    /// `observed` is the number of HTML elements that Legible found. `limit` is
    /// the value set by [`ExtractorBuilder::max_elements`](crate::ExtractorBuilder::max_elements).
    #[error("Aborting parsing document; {observed} elements found (max: {limit})")]
    TooManyElements {
        /// Number of HTML elements found.
        observed: usize,
        /// Configured maximum number of HTML elements.
        limit: usize,
    },

    /// The input exceeds a configured parser or structured-data resource limit.
    ///
    /// `resource` identifies the limited resource. `limit` is the configured
    /// maximum. The error does not always include the observed value because
    /// some limits stop work before the full value is known.
    #[error("Aborting parsing document; {resource} limit exceeded (max: {limit})")]
    ResourceLimit {
        /// Resource that exceeded its limit.
        resource: ResourceLimitKind,
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
                ParseLimitKind::Elements => Self::TooManyElements {
                    observed: limit.observed,
                    limit: limit.limit,
                },
                ParseLimitKind::Nodes => Self::ResourceLimit {
                    resource: ResourceLimitKind::DomNodes,
                    limit: limit.limit,
                },
                ParseLimitKind::TotalAttributes => Self::ResourceLimit {
                    resource: ResourceLimitKind::TotalAttributes,
                    limit: limit.limit,
                },
                ParseLimitKind::AttributesPerElement => Self::ResourceLimit {
                    resource: ResourceLimitKind::AttributesPerElement,
                    limit: limit.limit,
                },
                ParseLimitKind::TextBytes => Self::ResourceLimit {
                    resource: ResourceLimitKind::TextBytes,
                    limit: limit.limit,
                },
                ParseLimitKind::Depth => Self::ResourceLimit {
                    resource: ResourceLimitKind::ElementDepth,
                    limit: limit.limit,
                },
            },
        }
    }

    pub(crate) fn resource_limit(resource: ResourceLimitKind, limit: usize) -> Self {
        Self::ResourceLimit { resource, limit }
    }
}

#[cfg(test)]
mod tests {
    use super::ResourceLimitKind;

    #[test]
    fn resource_limit_names_are_stable() {
        assert_eq!(ResourceLimitKind::JsonLdBytes.name(), "json_ld_bytes");
        assert_eq!(ResourceLimitKind::DomNodes.to_string(), "DOM nodes");
    }
}
