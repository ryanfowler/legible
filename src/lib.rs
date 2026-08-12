//! Extract relevant content and metadata from HTML.
//!
//! Legible keeps an extracted DOM and renders each requested output format lazily.
//! Markdown is the primary output, with HTML and normalized plain text available from
//! the same extracted tree. Legible does not use a browser, execute JavaScript, fetch
//! URLs, or make network requests.
//!
//! ```rust
//! use legible::extract;
//!
//! let html = r#"
//!     <html lang="en">
//!       <head><title>Building a cache</title></head>
//!       <body>
//!         <nav>Navigation</nav>
//!         <main><p>This page explains how to build a cache.</p></main>
//!       </body>
//!     </html>
//! "#;
//! let page = extract(html, Some("https://example.com/cache"))?;
//!
//! println!("{}", page.markdown());
//! if let Some(title) = &page.metadata().title {
//!     println!("{title}");
//! }
//! # Ok::<(), legible::Error>(())
//! ```
//!
//! Use [`Extractor::builder`] to set the durable extraction limits and features.
//!
//! ```rust
//! use legible::Extractor;
//!
//! let extractor = Extractor::builder()
//!     .max_elements(100_000)
//!     .structured_data(true)
//!     .build();
//! let page = extractor.extract("<main><p>Content</p></main>", None)?;
//! let markdown = page
//!     .markdown_builder()
//!     .links(true)
//!     .images(false)
//!     .render();
//! # Ok::<(), legible::Error>(())
//! ```
//!
//! # Security
//!
//! Use [`ExtractedPage::safe_html`] for normal downstream display.
//! [`ExtractedPage::html`] is not sanitized and remains available when a caller needs
//! raw extracted markup. Markdown output contains no raw HTML and omits destinations
//! with unsupported URI schemes.

mod candidate;
mod cleaning;
mod constants;
mod diagnostics;
mod dom;
mod error;
mod extraction;
mod extractor;
mod html;
mod logging;
mod markdown;
mod metadata;
mod normalize;
mod page;
mod page_kind;
mod quality;
mod scoring;
mod specialized;
mod text;

pub use diagnostics::{
    AttemptRejectionReason, CandidateSourceInfo, ContentMetricsInfo, ExtractionAttempt,
    ExtractionDiagnostics, ExtractionStrategyInfo, QualityInfo, RootInfo, RootSelectionReasonInfo,
};
pub use error::{Error, Result};
pub use extractor::{ContentHint, ContentTag, Extractor, ExtractorBuilder};
pub use metadata::{
    Metadata, MetadataDiagnostics, MetadataFieldDiagnostics, MetadataListFieldDiagnostics,
    MetadataSource, MetadataValue,
};
pub use page::{ExtractedPage, HtmlBuilder, MarkdownBuilder};

/// Extracts relevant content and metadata from an HTML document.
///
/// Use [`Extractor`] when you want to reuse a configuration. The optional `url` must
/// be absolute. Relative links and media URLs stay relative when `url` is `None`.
///
/// # Errors
///
/// This function returns [`Error::InvalidUrl`], [`Error::NoBody`],
/// [`Error::NoContent`], or [`Error::TooManyElements`] when applicable.
pub fn extract(html: &str, url: Option<&str>) -> Result<ExtractedPage> {
    Extractor::default().extract(html, url)
}
