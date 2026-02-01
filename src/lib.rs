//! # Legible
//!
//! A Rust port of Mozilla's [Readability.js](https://github.com/mozilla/readability)
//! for extracting readable content from web pages.
//!
//! This library provides functionality to extract the main content from HTML documents,
//! stripping away navigation, ads, and other non-content elements to produce clean,
//! readable article content.
//!
//! ## Quick Start
//!
//! ```rust
//! use legible::parse;
//!
//! let html = r#"
//!     <html>
//!     <head><title>My Article</title></head>
//!     <body>
//!         <nav>Navigation</nav>
//!         <article>
//!             <h1>Article Title</h1>
//!             <p>This is the main content of the article. It contains several
//!             paragraphs of text that make up the body of the article.</p>
//!             <p>More content here to ensure we have enough text for the
//!             readability algorithm to work with properly.</p>
//!         </article>
//!         <footer>Footer</footer>
//!     </body>
//!     </html>
//! "#;
//!
//! match parse(html, Some("https://example.com"), None) {
//!     Ok(article) => {
//!         println!("Title: {}", article.title);
//!         println!("Byline: {:?}", article.byline);
//!         println!("Content: {}", article.content);
//!         println!("Text: {}", article.text_content);
//!     }
//!     Err(e) => eprintln!("Error: {}", e),
//! }
//! ```
//!
//! The returned [`Article`] contains:
//! - `title` - The article title
//! - `content` - The article content as HTML
//! - `text_content` - The article content as plain text
//! - `byline` - The author byline
//! - `excerpt` - A short excerpt from the article
//! - `site_name` - The site name
//! - `published_time` - The published time
//! - `dir` - Text direction (ltr or rtl)
//! - `lang` - Document language
//! - `length` - Length of the text content
//!
//! ## Checking Readability
//!
//! You can quickly check if a document is likely to be parseable without running
//! the full algorithm:
//!
//! ```rust
//! use legible::is_probably_readerable;
//!
//! let html = "<html><body><article>Long article content...</article></body></html>";
//! if is_probably_readerable(html, None) {
//!     println!("Document appears to be readerable");
//! }
//! ```
//!
//! ## Pre-parsed Document
//!
//! If you want to check readability before parsing, use [`Document`] to avoid
//! parsing the HTML twice:
//!
//! ```rust
//! use legible::Document;
//!
//! let html = r#"
//!     <html>
//!     <head><title>My Article</title></head>
//!     <body>
//!         <article>
//!             <h1>Article Title</h1>
//!             <p>This is the main content of the article. It contains several
//!             paragraphs of text that make up the body of the article.</p>
//!             <p>More content here to ensure we have enough text for the
//!             readability algorithm to work with properly.</p>
//!         </article>
//!     </body>
//!     </html>
//! "#;
//!
//! let doc = Document::new(html);
//!
//! if doc.is_probably_readerable(None) {
//!     match doc.parse(Some("https://example.com"), None) {
//!         Ok(article) => println!("Title: {}", article.title),
//!         Err(e) => eprintln!("Error: {}", e),
//!     }
//! }
//! ```
//!
//! ## Configuration
//!
//! Use the [`Options`] builder to customize parsing behavior:
//!
//! ```rust
//! use legible::{parse, Options};
//!
//! let html = "<html><body><article>Content...</article></body></html>";
//!
//! let options = Options::new()
//!     .char_threshold(250)        // Minimum article length (default: 500)
//!     .keep_classes(true)         // Preserve CSS classes in output
//!     .disable_json_ld(true);     // Skip JSON-LD metadata extraction
//!
//! let article = parse(html, Some("https://example.com"), Some(options));
//! ```
//!
//! See [`Options`] for all available configuration options.
//!
//! ## Security
//!
//! The extracted HTML content is **unsanitized** and may contain malicious scripts or
//! other dangerous content from the source document. Before rendering this HTML in a
//! browser or other context where scripts could execute, you should sanitize it using
//! a library like [`ammonia`](https://docs.rs/ammonia):
//!
//! ```rust,ignore
//! let article = parse(html, Some(url), None)?;
//! let safe_html = ammonia::clean(&article.content);
//! ```
//!
//! ## How It Works
//!
//! Legible implements the same algorithm as Readability.js:
//!
//! 1. **Document Preparation** - Removes scripts, normalizes markup, fixes lazy-loaded images
//! 2. **Metadata Extraction** - Extracts title, byline, and other metadata from JSON-LD,
//!    OpenGraph tags, and meta elements
//! 3. **Content Scoring** - Scores DOM nodes based on tag type, text density, and class/id patterns
//! 4. **Candidate Selection** - Identifies the highest-scoring content container
//! 5. **Content Cleaning** - Removes low-scoring elements, empty containers, and non-content markup

mod cleaning;
mod constants;
mod document;
mod dom;
mod error;
mod logging;
mod metadata;
mod options;
mod readability;
mod readerable;
mod scoring;
mod selectors;

pub use document::Document;
pub use error::{Error, Result};
pub use options::{Options, ReaderableOptions};
pub use readability::Article;
pub use readerable::is_probably_readerable;

/// Parse an HTML document and extract the article content.
///
/// This is the main entry point for content extraction. It parses the HTML, identifies
/// the main article content, and returns an [`Article`] with the extracted content
/// and metadata.
///
/// # Arguments
///
/// * `html` - The HTML content to parse
/// * `url` - Optional base URL for resolving relative links. If provided, relative URLs
///   in the extracted content will be converted to absolute URLs.
/// * `options` - Optional [`Options`] to customize parsing behavior
///
/// # Errors
///
/// Returns an error if:
/// - The provided URL is invalid ([`Error::InvalidUrl`])
/// - The document has no `<body>` element ([`Error::NoBody`])
/// - No article content could be extracted ([`Error::NoContent`])
/// - The document exceeds `max_elems_to_parse` ([`Error::TooManyElements`])
///
/// # Example
///
/// ```rust
/// use legible::{parse, Options};
///
/// let html = "<html><body><article>Content...</article></body></html>";
///
/// // Basic usage
/// let article = parse(html, None, None);
///
/// // With URL for resolving relative links
/// let article = parse(html, Some("https://example.com/article"), None);
///
/// // With custom options
/// let options = Options::new().char_threshold(250);
/// let article = parse(html, Some("https://example.com"), Some(options));
/// ```
pub fn parse(html: &str, url: Option<&str>, options: Option<Options>) -> Result<Article> {
    Document::new(html).parse(url, options)
}
