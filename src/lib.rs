//! # Legible
//!
//! Legible extracts the main article from an HTML document. It removes navigation,
//! advertisements, sidebars, and other unrelated content. Legible is a Rust port of
//! Mozilla's [Readability.js](https://github.com/mozilla/readability).
//!
//! ## Extract an article
//!
//! Use [`parse`] for most applications:
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
//!             <p>This is the main content of the article.</p>
//!             <p>This second paragraph contains more article text.</p>
//!         </article>
//!         <footer>Footer</footer>
//!     </body>
//!     </html>
//! "#;
//!
//! match parse(html, Some("https://example.com/articles/1"), None) {
//!     Ok(article) => {
//!         println!("Title: {}", article.title);
//!         println!("HTML: {}", article.content);
//!         println!("Markdown: {}", article.markdown_content);
//!         println!("Text: {}", article.text_content);
//!     }
//!     Err(error) => eprintln!("Error: {error}"),
//! }
//! ```
//!
//! The optional URL must be absolute. Legible uses it as the base URL for relative
//! links and media URLs. Relative URLs stay relative if you pass `None`.
//!
//! [`Article`] provides HTML, CommonMark, normalized plain text, and article metadata.
//!
//! ## Check a document before extraction
//!
//! [`is_probably_readerable`] performs a quick content check. This check is a
//! heuristic. A `true` result does not guarantee successful extraction. A `false`
//! result does not prove that the document has no article.
//!
//! Use [`Document`] if you want to run the check and then extract the article.
//! `Document` prevents a second HTML parse.
//!
//! ```rust
//! use legible::Document;
//!
//! let text = "Article text. ".repeat(30);
//! let html = format!("<article><p>{text}</p></article>");
//! let document = Document::new(&html);
//!
//! if document.is_probably_readerable(None) {
//!     let result = document.parse(Some("https://example.com/articles/1"), None);
//!     // Use the extraction result.
//! }
//! ```
//!
//! The check borrows the document. Extraction consumes it because extraction changes
//! the internal document tree.
//!
//! ## Configure extraction
//!
//! Use [`Options`] to configure extraction. Use [`ReaderableOptions`] to configure the
//! quick content check.
//!
//! ```rust
//! use legible::{Options, parse};
//!
//! let options = Options::new()
//!     .char_threshold(250)
//!     .keep_classes(true)
//!     .disable_json_ld(true);
//!
//! let result = parse(
//!     "<html><body><article><p>Article text</p></article></body></html>",
//!     Some("https://example.com/articles/1"),
//!     Some(options),
//! );
//! ```
//!
//! ## Security
//!
//! **Do not render [`Article::content`] without sanitizing it.**
//!
//! Legible cleans article content, but it is not an HTML security sanitizer. The HTML
//! can contain unsafe attributes, URLs, or other source markup. Apply a sanitizer that
//! matches your security policy before you render the HTML.
//!
//! [`Article::markdown_content`] does not contain raw HTML. It removes links and images
//! that have unsupported URI schemes. If you convert the Markdown to HTML, sanitize
//! that HTML according to your application's security policy.

mod cleaning;
mod constants;
mod document;
mod dom;
mod error;
mod logging;
mod markdown;
mod metadata;
mod options;
mod readability;
mod readerable;
mod scoring;
mod text;

pub use document::Document;
pub use error::{Error, Result};
pub use options::{Options, ReaderableOptions};
pub use readability::Article;
pub use readerable::is_probably_readerable;

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
pub fn parse(html: &str, url: Option<&str>, options: Option<Options>) -> Result<Article> {
    Document::new(html).parse(url, options)
}
