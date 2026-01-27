//! # Legible
//!
//! A Rust port of Mozilla's Readability.js for extracting readable content from web pages.
//!
//! This library provides functionality to extract the main content from HTML documents,
//! stripping away navigation, ads, and other non-content elements to produce clean,
//! readable article content.
//!
//! ## Quick Start
//!
//! ```rust
//! use legible::{Readability, Options};
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
//! let mut readability = Readability::new(html, Some("https://example.com"), None);
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

pub mod cleaning;
pub mod constants;
pub mod dom;
pub mod error;
pub mod metadata;
pub mod options;
pub mod readability;
pub mod readerable;
pub mod scoring;

// Re-export main types for convenience
pub use error::{ReadabilityError, Result};
pub use options::{Options, ReaderableOptions};
pub use readability::{Article, Readability};
pub use readerable::is_probably_readerable;
