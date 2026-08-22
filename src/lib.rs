//! Extract relevant content and metadata from HTML.
//!
//! Legible compiles selected HTML into a private semantic representation and renders each requested
//! output format lazily. Markdown is the primary output. Canonical HTML and normalized
//! plain text come from the same private representation. The public output contract is
//! Markdown, canonical semantic HTML, normalized text, metadata, and scalar metrics.
//! Legible does not use a browser, execute JavaScript, fetch URLs, or make network requests.
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
//! Use [`ParseBudget`] to bound parser and JSON-LD work when the input is not
//! trusted. Zero-valued limits are unlimited, except that JSON-LD depth uses an
//! internal safety cap. The default extractor has no caller-configured limit.
//!
//! # Security
//!
//! [`ExtractedPage::html`] returns canonical semantic HTML. It cannot contain source
//! scripts, event handlers, arbitrary attributes, or unsupported URI schemes.
//! Markdown output contains no raw HTML.

mod budget;
mod candidate;
mod cleaning;
mod constants;
mod diagnostics;
mod document;
mod dom;
mod error;
mod extraction;
mod extractor;
#[cfg(feature = "bench-instrumentation")]
pub mod instrumentation;
#[cfg(not(feature = "bench-instrumentation"))]
mod instrumentation;
mod logging;
mod metadata;
mod normalize;
mod page;
mod page_kind;
mod prepared;
mod quality;
mod render;
mod scan;
mod scoring;
mod specialized;
mod tokens;

pub use budget::ParseBudget;
pub use diagnostics::{
    AcceptanceExceptionInfo, AttemptRejectionReason, CandidateSourceInfo, CleanupActionInfo,
    CleanupActionKind, ContentMetricsInfo, ExtractionAttempt, ExtractionDiagnostics,
    ExtractionStrategyInfo, NormalizationCountsInfo, QualityInfo, RepresentationMetricsInfo,
    RootInfo, RootSelectionReasonInfo, SemanticCategoryCoverageInfo, SemanticCoverageCategory,
    SemanticCoverageInfo,
};
pub use error::{Error, Result};
pub use extractor::{ContentHint, ContentTag, Extractor, ExtractorBuilder};
pub use metadata::{
    Metadata, MetadataDiagnostics, MetadataFieldDiagnostics, MetadataListFieldDiagnostics,
    MetadataSource, MetadataValue,
};
pub use page::{ExtractedPage, HtmlBuilder, MarkdownBuilder};

#[cfg(feature = "bench-instrumentation")]
pub use instrumentation::{
    ExtractionCounters, InstrumentationSnapshot, Phase, PhaseDurations, SnapshotKind,
};

/// Extracts relevant content and metadata from an HTML document.
///
/// Use [`Extractor`] when you want to reuse a configuration. The optional `url` must
/// be absolute. Relative links and media URLs stay relative when `url` is `None`.
///
/// # Errors
///
/// This function returns [`Error::InvalidUrl`], [`Error::NoBody`],
/// [`Error::NoContent`], [`Error::TooManyElements`], [`Error::ResourceLimit`],
/// or [`Error::Parse`] when applicable. [`Error::ContentRootNotFound`] applies
/// only when you configure an exact root with [`ExtractorBuilder::content_root`].
pub fn extract(html: &str, url: Option<&str>) -> Result<ExtractedPage> {
    Extractor::default().extract(html, url)
}
