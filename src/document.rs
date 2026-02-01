//! Public [`Document`] type for pre-parsing HTML once and reusing it.

use dom_query::Document as DomDocument;

use crate::error::Result;
use crate::options::{Options, ReaderableOptions};
use crate::readability::{Article, Readability};
use crate::readerable::is_probably_readerable_doc;

/// A pre-parsed HTML document.
///
/// `Document` holds the parsed DOM tree and a reference to the original HTML string,
/// allowing you to call [`is_probably_readerable`](Document::is_probably_readerable) and
/// [`parse`](Document::parse) without re-parsing the HTML each time.
///
/// # Typical Usage
///
/// ```rust
/// use legible::Document;
///
/// let html = r#"
///     <html>
///     <head><title>My Article</title></head>
///     <body>
///         <article>
///             <h1>Article Title</h1>
///             <p>This is the main content of the article. It contains several
///             paragraphs of text that make up the body of the article.</p>
///             <p>More content here to ensure we have enough text for the
///             readability algorithm to work with properly.</p>
///         </article>
///     </body>
///     </html>
/// "#;
///
/// let doc = Document::new(html);
///
/// if doc.is_probably_readerable(None) {
///     let article = doc.parse(Some("https://example.com"), None);
///     // ...
/// }
/// ```
///
/// # Ownership
///
/// - [`is_probably_readerable`](Document::is_probably_readerable) borrows `&self` because
///   the readability check is read-only.
/// - [`parse`](Document::parse) consumes `self` because the extraction algorithm mutates
///   the DOM during content extraction.
pub struct Document<'a> {
    doc: DomDocument,
    html: &'a str,
}

impl<'a> Document<'a> {
    /// Create a new `Document` by parsing the given HTML string.
    ///
    /// # Example
    ///
    /// ```rust
    /// use legible::Document;
    ///
    /// let doc = Document::new("<html><body><p>Hello</p></body></html>");
    /// ```
    pub fn new(html: &'a str) -> Self {
        let doc = DomDocument::from(html);
        Self { doc, html }
    }

    /// Check if this document is probably readerable.
    ///
    /// This is a quick heuristic check that borrows the document, so you can
    /// still call [`parse`](Document::parse) afterwards.
    ///
    /// See [`is_probably_readerable`](crate::is_probably_readerable) for details.
    pub fn is_probably_readerable(&self, options: Option<ReaderableOptions>) -> bool {
        is_probably_readerable_doc(&self.doc, options)
    }

    /// Parse the document and extract the article content.
    ///
    /// This consumes the `Document` because the extraction algorithm mutates
    /// the DOM during processing.
    ///
    /// See [`parse`](crate::parse) for details on arguments and errors.
    pub fn parse(self, url: Option<&str>, options: Option<Options>) -> Result<Article> {
        Readability::from_document(self.doc, self.html, url, options).parse()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_readerable_check() {
        let long_text = "a".repeat(600);
        let html = format!("<html><body><p>{}</p></body></html>", long_text);
        let doc = Document::new(&html);
        assert!(doc.is_probably_readerable(None));
    }

    #[test]
    fn test_not_readerable() {
        let html = "<html><body><p>Short</p></body></html>";
        let doc = Document::new(html);
        assert!(!doc.is_probably_readerable(None));
    }

    #[test]
    fn test_parse() {
        let html = r#"
            <html>
            <head><title>Test Article</title></head>
            <body>
                <article>
                    <h1>Test Article</h1>
                    <p>This is the main content of the article. It contains several
                    paragraphs of text that make up the body of the article.</p>
                    <p>More content here to ensure we have enough text for the
                    readability algorithm to work with properly.</p>
                </article>
            </body>
            </html>
        "#;
        let doc = Document::new(html);
        let article = doc.parse(Some("https://example.com"), None).unwrap();
        assert!(!article.title.is_empty());
        assert!(!article.content.is_empty());
    }

    #[test]
    fn test_readerable_then_parse() {
        let long_text = "a ".repeat(300);
        let html = format!(
            r#"
            <html>
            <head><title>Test Article</title></head>
            <body>
                <article>
                    <h1>Test Article</h1>
                    <p>{}</p>
                </article>
            </body>
            </html>
        "#,
            long_text
        );
        let doc = Document::new(&html);
        assert!(doc.is_probably_readerable(None));
        let article = doc.parse(None, None).unwrap();
        assert!(!article.content.is_empty());
    }

    #[test]
    fn test_parse_no_body() {
        let html = "<html><head><title>No Body</title></head></html>";
        let doc = Document::new(html);
        assert!(doc.parse(None, None).is_err());
    }

    #[test]
    fn test_parse_invalid_url() {
        let html = "<html><body><p>Content</p></body></html>";
        let doc = Document::new(html);
        assert!(doc.parse(Some("not a valid url ://"), None).is_err());
    }
}
