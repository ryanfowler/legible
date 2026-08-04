//! A parsed HTML document for a readability check followed by extraction.
use crate::dom::Dom;
use crate::error::Result;
use crate::options::ReaderableOptions;

use crate::readerable::is_probably_readerable_doc;

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
/// let document = Document::parse(&html)?;
///
/// if document.is_probably_readable() {
///     let result = legible::Extractor::default().extract_document(document);
///     // Use the extraction result.
/// }
/// # Ok::<(), legible::Error>(())
/// ```
///
/// # Ownership
///
/// [`Document::is_probably_readerable`] borrows the document. You can extract the
/// article after the check. [`Document::parse`] consumes the document because
/// extraction changes the internal document tree.
pub struct Document<'a> {
    pub(crate) doc: Dom,
    pub(crate) html: &'a str,
}
impl<'a> Document<'a> {
    /// Parses an HTML string and stores the document tree.
    ///
    /// The document borrows `html`. The source string must exist for as long as the
    /// document.
    #[deprecated(since = "0.5.0", note = "use Document::parse")]
    pub fn new(html: &'a str) -> Self {
        Self::parse(html).expect("HTML parsing failed")
    }

    /// Parses HTML into a reusable document.
    pub fn parse(html: &'a str) -> Result<Self> {
        let doc = Dom::parse_document(html)
            .map_err(|error| crate::error::ParseError::new(error.to_string()))?;
        Ok(Self { doc, html })
    }
    /// Checks if this document probably contains readable article content.
    ///
    /// This quick check is a heuristic. A `true` result does not guarantee successful
    /// extraction. A `false` result does not prove that the document has no article.
    /// This method borrows the document. You can later pass the document to
    /// [`Extractor::extract_document`](crate::Extractor::extract_document).
    /// Default options apply if `options` is `None`.
    pub fn is_probably_readerable(&self, options: Option<ReaderableOptions>) -> bool {
        is_probably_readerable_doc(&self.doc, options)
    }

    /// Uses default options for the quick readability heuristic.
    pub fn is_probably_readable(&self) -> bool {
        is_probably_readerable_doc(&self.doc, None)
    }

    /// Uses explicit options for the quick readability heuristic.
    pub fn is_probably_readable_with(&self, options: &ReaderableOptions) -> bool {
        is_probably_readerable_doc(&self.doc, Some(options.clone()))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_readerable_check() {
        let s = "a ".repeat(300);
        assert!(
            Document::parse(&format!("<p>{s}</p>"))
                .unwrap()
                .is_probably_readable()
        );
    }
    #[test]
    fn test_not_readerable() {
        assert!(
            !Document::parse("<p>Short</p>")
                .unwrap()
                .is_probably_readable()
        );
    }

    #[test]
    fn empty_documents_have_no_content() {
        for html in [
            "",
            "<html><head><title>No body content</title></head></html>",
            "<html><body><img src=\"article.jpg\" alt=\"Article image\"></body></html>",
        ] {
            assert!(matches!(
                crate::Extractor::default().extract_document(Document::parse(html).unwrap()),
                Err(crate::Error::NoContent)
            ));
        }
    }
}
