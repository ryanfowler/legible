//! # Legible
//!
//! A Rust port of Mozilla's Readability.js for extracting readable content from web pages.
//!
//! This library provides functionality to extract the main content from HTML documents,
//! stripping away navigation, ads, and other non-content elements to produce clean,
//! readable article content.
//!
//! ## Security
//!
//! The extracted HTML content is **unsanitized** and may contain malicious scripts or
//! other dangerous content from the source document. Before rendering this HTML in a
//! browser or other context where scripts could execute, you should sanitize it using
//! a library like [`ammonia`](https://docs.rs/ammonia).
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
//!         println!("Content: {}", article.content);
//!     }
//!     Err(e) => eprintln!("Error: {}", e),
//! }
//! ```
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
