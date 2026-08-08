//! A parsed HTML document for a readability check followed by extraction.
use crate::dom::Dom;
use crate::error::Result;
use crate::options::{Options, ReaderableOptions};
use crate::readability::{Article, Readability};
use crate::readerable::is_probably_readerable_doc;
use std::sync::LazyLock;

static DEFAULT_OPTIONS: LazyLock<Options> = LazyLock::new(Options::default);

/// A parsed HTML document.
///
/// Use `Document` if you want to run a readability check and then extract the article.
/// It prevents a second HTML parse.
///
/// # Example
///
/// ```rust
/// use legible::Document;
///
/// let text = "Article text. ".repeat(30);
/// let html = format!("<article><h1>Title</h1><p>{text}</p></article>");
/// let document = Document::new(&html);
///
/// if document.is_probably_readerable(None) {
///     let result = document.parse(Some("https://example.com/articles/1"), None);
///     // Use the extraction result.
/// }
/// ```
///
/// # Ownership
///
/// [`Document::is_probably_readerable`] borrows the document. You can extract the
/// article after the check. [`Document::parse`] consumes the document because
/// extraction changes the internal document tree.
pub struct Document<'a> {
    pub(crate) doc: Dom,
    html: &'a str,
}
impl<'a> Document<'a> {
    /// Parses an HTML string and stores the document tree.
    ///
    /// The document borrows `html`. The source string must exist for as long as the
    /// document.
    pub fn new(html: &'a str) -> Self {
        Self {
            doc: Dom::parse_document(html).expect("HTML DOM node limit exceeded"),
            html,
        }
    }
    /// Checks if this document probably contains readable article content.
    ///
    /// This quick check is a heuristic. A `true` result does not guarantee successful
    /// extraction. A `false` result does not prove that the document has no article.
    /// This method borrows the document, so you can call [`Document::parse`] after it.
    /// Default options apply if `options` is `None`.
    pub fn is_probably_readerable(&self, options: Option<ReaderableOptions>) -> bool {
        is_probably_readerable_doc(&self.doc, options)
    }

    /// Extracts article content and metadata from this document.
    ///
    /// This method consumes the document because extraction changes the internal
    /// document tree. `url` must be an absolute base URL if it is present. Default
    /// extraction options apply if `options` is `None`.
    ///
    /// See [`parse`](crate::parse) for all parameters and errors.
    pub fn parse(self, url: Option<&str>, options: Option<Options>) -> Result<Article> {
        let options = options.as_ref().unwrap_or(&DEFAULT_OPTIONS);
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

    #[test]
    fn resolves_urls_against_the_first_base_with_href() {
        let text = "This article contains enough text to be extracted. ".repeat(8);
        let html = format!(
            "<svg><base href=\"https://evil.example/\"></svg><base><base href=\"/assets/\"><article><p>{text}<a href=\"story.html\">Read more</a></p></article>"
        );
        let article = Document::new(&html)
            .parse(Some("https://example.com/read/page.html"), None)
            .unwrap();

        assert!(
            article
                .content
                .contains(r#"href="https://example.com/assets/story.html""#)
        );
    }
}
