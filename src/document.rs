//! Public [`Document`] type for pre-parsing HTML once and reusing it.
use crate::dom::Dom;
use crate::error::Result;
use crate::options::{Options, ReaderableOptions};
use crate::readability::{Article, Readability};
use crate::readerable::is_probably_readerable_doc;

/// A pre-parsed HTML document.
///
/// `Document` lets you check readability and extract content without parsing the HTML
/// twice.
///
/// # Example
///
/// ```rust
/// use legible::Document;
///
/// let html = r#"
///     <html><body><article>
///         <h1>Article title</h1>
///         <p>This article has enough text for the readability check to inspect.</p>
///     </article></body></html>
/// "#;
/// let doc = Document::new(html);
///
/// if doc.is_probably_readerable(None) {
///     let article = doc.parse(Some("https://example.com"), None);
///     // Use the extracted article.
/// }
/// ```
///
/// # Ownership
///
/// [`Document::is_probably_readerable`] borrows the document because the check is
/// read-only. [`Document::parse`] consumes the document because extraction mutates the
/// DOM.
pub struct Document<'a> {
    pub(crate) doc: Dom,
    html: &'a str,
}
impl<'a> Document<'a> {
    /// Parse an HTML string into a reusable document.
    pub fn new(html: &'a str) -> Self {
        Self {
            doc: Dom::parse_document(html).expect("HTML DOM node limit exceeded"),
            html,
        }
    }
    /// Check if this document probably contains readable article content.
    ///
    /// This method borrows the document. You can call [`Document::parse`] after it.
    /// See [`is_probably_readerable`](crate::is_probably_readerable) for more details.
    pub fn is_probably_readerable(&self, options: Option<ReaderableOptions>) -> bool {
        is_probably_readerable_doc(&self.doc, options)
    }

    /// Extract article content and metadata from this document.
    ///
    /// This method consumes the document because extraction mutates the DOM. See
    /// [`parse`](crate::parse) for details about arguments and errors.
    pub fn parse(self, url: Option<&str>, options: Option<Options>) -> Result<Article> {
        Readability::from_document(self.doc, self.html, url, options).parse()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_readerable_check() {
        let s = "a ".repeat(300);
        assert!(Document::new(&format!("<p>{s}</p>")).is_probably_readerable(None));
    }
    #[test]
    fn test_not_readerable() {
        assert!(!Document::new("<p>Short</p>").is_probably_readerable(None));
    }

    #[test]
    fn empty_documents_have_no_content() {
        for html in [
            "",
            "<html><head><title>No body content</title></head></html>",
            "<html><body><img src=\"article.jpg\" alt=\"Article image\"></body></html>",
        ] {
            assert!(matches!(
                Document::new(html).parse(None, None),
                Err(crate::Error::NoContent)
            ));
        }
    }
}
