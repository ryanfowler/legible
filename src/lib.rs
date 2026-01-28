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
//! use legible::Readability;
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
//! let readability = Readability::new(html, Some("https://example.com"), None);
//! match readability.parse() {
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
//! ## Configuration
//!
//! Use the [`Options`] builder to customize parsing behavior:
//!
//! ```rust
//! use legible::{Readability, Options};
//!
//! let html = "<html><body><article>Content...</article></body></html>";
//!
//! let options = Options::new()
//!     .char_threshold(250)        // Minimum article length (default: 500)
//!     .keep_classes(true)         // Preserve CSS classes in output
//!     .disable_json_ld(true);     // Skip JSON-LD metadata extraction
//!
//! let readability = Readability::new(html, Some("https://example.com"), Some(options));
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
//! let article = readability.parse()?;
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
mod dom;
mod error;
mod metadata;
mod options;
mod readability;
mod readerable;
mod scoring;

pub use error::{ReadabilityError, Result};
pub use options::{Options, ReaderableOptions};
pub use readability::{Article, Readability};
pub use readerable::is_probably_readerable;
