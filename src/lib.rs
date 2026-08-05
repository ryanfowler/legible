//! Legible extracts readable article content from HTML.
//!
//! Use [`extract`] for one extraction. Use [`Extractor`] for reusable configuration.
//! The returned [`Article`] stores a compact immutable tree and renders HTML, Markdown,
//! or normalized text only when requested.
//!
//! ```rust
//! let article = legible::extract("<article><h1>Title</h1><p>Article text.</p></article>")?;
//! println!("{}", article.title().unwrap_or("Untitled"));
//! let html = article.to_html();
//! # Ok::<(), legible::Error>(())
//! ```
//!
//! [`Article::to_html`] does not return sanitized HTML. Apply a sanitizer before you
//! render the result. The quick [`is_probably_readable`] check is a heuristic and can
//! return false positives or false negatives.

#![allow(deprecated)]

mod article;
mod article_tree;
mod cleaning;
mod constants;
mod document;
mod dom;
mod error;
mod extractor;
mod logging;
mod markdown;
mod metadata;
mod options;
mod readability;
mod readerable;
mod scoring;

pub use article::{
    Article, ArticleMetadata, Author, BulletMarker, HeadingStyle, ImageMetadata, MarkdownOptions,
    TextDirection, TextOptions, TextSeparator,
};
pub use document::Document;
pub use error::{Error, ParseError, Result};
pub use extractor::{
    ClassPolicy, EmbedPolicy, Extractor, ExtractorBuilder, Heuristics, MetadataSources, extract,
    extract_with_url,
};
pub use options::{Options, ReadabilityOptions, ReaderableOptions};
pub use readability::LegacyArticle;
pub use readerable::is_probably_readerable;

/// Performs the quick readability heuristic with the corrected API spelling.
pub fn is_probably_readable(html: &str, options: Option<ReadabilityOptions>) -> bool {
    is_probably_readerable(html, options)
}

/// Extract article content and metadata from an HTML document.
///
/// Use this function for a single extraction. Use [`Document`] if you first call
/// [`is_probably_readerable`]. A `Document` prevents a second HTML parse.
///
/// # Parameters
///
/// * `html` is the source HTML.
/// * `url` is an optional absolute base URL. Legible resolves relative link and media
///   URLs against this value. Relative URLs stay relative if this value is `None`.
/// * `options` configures extraction. Default options apply if this value is `None`.
///
/// # Errors
///
/// This function returns:
///
/// * [`Error::InvalidUrl`] if `url` is not a valid absolute URL.
/// * [`Error::NoBody`] if the parsed document has no `<body>` element.
/// * [`Error::NoContent`] if Legible cannot extract nonempty article content.
/// * [`Error::TooManyElements`] if the document exceeds
///   [`Options::max_elems_to_parse`].
///
/// # Example
///
/// ```rust
/// use legible::{Options, parse};
///
/// let html = "<html><body><article><p>Article content.</p></article></body></html>";
/// let options = Options::new().char_threshold(250);
/// let result = parse(
///     html,
///     Some("https://example.com/articles/1"),
///     Some(options),
/// );
/// ```
#[deprecated(
    since = "0.5.0",
    note = "use extract(), Extractor, and Article rendering methods"
)]
pub fn parse(html: &str, url: Option<&str>, options: Option<Options>) -> Result<legacy::Article> {
    let document = Document::parse(html)?;
    let options = options.unwrap_or_default();
    readability::Readability::from_document(document.doc, document.html, url, &options).parse()
}

/// Compatibility API for applications that still use the 0.4 result fields.
pub mod legacy {
    pub use crate::options::Options;
    pub use crate::readability::LegacyArticle as Article;
}
