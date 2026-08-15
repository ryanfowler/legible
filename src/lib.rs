//! Extract relevant content and metadata from HTML.
//!
//! Legible compiles selected HTML into a semantic document and renders each requested
//! output format lazily. Markdown is the primary output. Canonical HTML and normalized
//! plain text come from the same semantic document. Legible does not use a browser, execute JavaScript, fetch
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
//! [`ExtractedPage::html`] returns canonical semantic HTML. It cannot contain source
//! scripts, event handlers, arbitrary attributes, or unsupported URI schemes.
//! [`ExtractedPage::safe_html`] is an alias for the same output. Markdown output
//! contains no raw HTML.

mod candidate;
mod cleaning;
mod constants;
mod diagnostics;
pub mod document;
mod dom;
mod error;
mod extraction;
mod extractor;
#[cfg(test)]
mod html;
mod logging;
#[cfg(test)]
mod markdown;
mod metadata;
mod normalize;
mod page;
mod page_kind;
mod quality;
mod render;
mod scoring;
mod specialized;
#[cfg(test)]
mod text;

pub use diagnostics::{
    AttemptRejectionReason, CandidateSourceInfo, CleanupActionInfo, CleanupActionKind,
    ContentMetricsInfo, ExtractionAttempt, ExtractionDiagnostics, ExtractionStrategyInfo,
    NormalizationCountsInfo, QualityInfo, RootInfo, RootSelectionReasonInfo,
};
pub use document::{
    Callout, CalloutKind, CodeBlock, Document, DocumentNode, DocumentStats, FootnoteDefinition,
    FootnoteId, Image, Link, List, ListKind, MathFormat, MathValue, Media, MediaKind, NodeKind,
    Table, TableAlignment, TableCell, TaskMarker, TextValue,
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
