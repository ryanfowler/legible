//! The extracted page and lazy output builders.

use crate::diagnostics::ExtractionDiagnostics;
use crate::document::Document;
use crate::metadata::{Metadata, MetadataDiagnostics};
use serde_json::Value;

/// Relevant page content and metadata.
///
/// Output formats are rendered lazily from one semantic document. Calling a
/// render method more than once produces the same output.
pub struct ExtractedPage {
    metadata: Metadata,
    document: Document,
    diagnostics: Option<ExtractionDiagnostics>,
    metadata_diagnostics: Option<MetadataDiagnostics>,
    structured_data: Option<Vec<Value>>,
}

impl ExtractedPage {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        document: Document,
        metadata: Metadata,
        diagnostics: Option<ExtractionDiagnostics>,
        metadata_diagnostics: Option<MetadataDiagnostics>,
        structured_data: Option<Vec<Value>>,
    ) -> Self {
        Self {
            metadata,
            document,
            diagnostics,
            metadata_diagnostics,
            structured_data,
        }
    }

    /// Returns the extracted semantic document.
    ///
    /// The document omits site chrome and source HTML implementation details.
    /// Use it when you need structured content instead of a rendered format.
    pub fn document(&self) -> &Document {
        &self.document
    }

    /// Returns discovered page metadata.
    pub fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    /// Returns extraction diagnostics when the extractor enabled them.
    pub fn diagnostics(&self) -> Option<&ExtractionDiagnostics> {
        self.diagnostics.as_ref()
    }

    /// Returns metadata provenance when the extractor enabled it.
    pub fn metadata_diagnostics(&self) -> Option<&MetadataDiagnostics> {
        self.metadata_diagnostics.as_ref()
    }

    /// Returns parsed JSON-LD items when the extractor retained them.
    pub fn structured_data(&self) -> Option<&[Value]> {
        self.structured_data.as_deref()
    }

    /// Renders the extracted content as CommonMark.
    pub fn markdown(&self) -> String {
        self.markdown_builder().render()
    }

    /// Returns a Markdown output builder.
    pub fn markdown_builder(&self) -> MarkdownBuilder<'_> {
        MarkdownBuilder {
            page: self,
            links: true,
            images: true,
        }
    }

    /// Renders the extracted content as normalized plain text.
    pub fn text(&self) -> String {
        crate::render::text::render_text(
            &self.document,
            self.document.text_length(),
            &crate::render::text::TextOptions::default(),
        )
    }

    /// Renders the extracted content as canonical semantic HTML.
    ///
    /// The semantic document cannot contain active source elements, arbitrary
    /// attributes, or unsupported URI schemes.
    pub fn html(&self) -> String {
        self.html_builder().render()
    }

    /// Renders canonical semantic HTML.
    ///
    /// This method is an alias for [`Self::html`].
    pub fn safe_html(&self) -> String {
        self.html_builder().sanitize(true).render()
    }

    /// Returns an HTML output builder.
    pub fn html_builder(&self) -> HtmlBuilder<'_> {
        HtmlBuilder {
            page: self,
            sanitize: false,
        }
    }

    /// Returns the number of words in the normalized extracted text.
    pub fn word_count(&self) -> usize {
        self.document.word_count()
    }

    /// Returns the number of characters in the normalized extracted text.
    pub fn text_length(&self) -> usize {
        self.document.text_length()
    }

    /// Checks semantic document invariants for fuzz testing.
    #[doc(hidden)]
    #[cfg(feature = "fuzzing")]
    pub fn validate_document(&self) -> bool {
        self.document.validate().is_ok()
    }
}

/// Configures HTML rendering for an [`ExtractedPage`].
pub struct HtmlBuilder<'a> {
    page: &'a ExtractedPage,
    sanitize: bool,
}

impl HtmlBuilder<'_> {
    /// Retained for compatibility. Canonical semantic HTML is always safe by construction.
    pub fn sanitize(mut self, enabled: bool) -> Self {
        self.sanitize = enabled;
        self
    }

    /// Renders the configured HTML output.
    pub fn render(self) -> String {
        let _ = self.sanitize;
        crate::render::html::render_html(&self.page.document, self.page.document.text_length())
    }
}

/// Configures Markdown rendering for an [`ExtractedPage`].
pub struct MarkdownBuilder<'a> {
    page: &'a ExtractedPage,
    links: bool,
    images: bool,
}

impl MarkdownBuilder<'_> {
    /// Controls whether links are rendered as links or plain text.
    pub fn links(mut self, enabled: bool) -> Self {
        self.links = enabled;
        self
    }

    /// Controls whether images are rendered.
    pub fn images(mut self, enabled: bool) -> Self {
        self.images = enabled;
        self
    }

    /// Renders the configured Markdown output.
    pub fn render(self) -> String {
        crate::render::markdown::render_markdown(
            &self.page.document,
            self.page.document.text_length(),
            crate::render::markdown::MarkdownConfig {
                links: self.links,
                images: self.images,
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use crate::extract;
    use std::path::{Path, PathBuf};

    fn fixture_sources(root: &Path, sources: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(root) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                fixture_sources(&path, sources);
            } else if path.file_name().is_some_and(|name| name == "source.html") {
                sources.push(path);
            }
        }
    }

    #[test]
    fn all_extraction_fixtures_compile_to_valid_documents() {
        let mut sources = Vec::new();
        fixture_sources(Path::new("tests"), &mut sources);
        sources.sort();
        assert!(!sources.is_empty());

        for source in sources {
            let html = std::fs::read_to_string(&source).unwrap();
            let result = extract(&html, Some("https://example.test/docs/page.html"));
            let expects_error = source
                .parent()
                .is_some_and(|directory| directory.join("expected.error").exists());
            assert!(
                result.is_ok() || expects_error,
                "{} did not extract: {:?}",
                source.display(),
                result.err()
            );
        }
    }

    #[test]
    fn outputs_are_lazy_and_deterministic() {
        let page = extract(
            "<main><p>Hello <a href='/world'>world</a>.</p><img src='image.png' alt='Image'></main>",
            Some("https://example.com/page"),
        )
        .unwrap();

        assert_eq!(page.markdown(), page.markdown());
        assert_eq!(page.text(), page.text());
        assert_eq!(page.html(), page.html());
        assert_eq!(page.document().text(), page.text());
        assert_eq!(page.document().text_length(), page.text_length());
        assert_eq!(page.document().word_count(), page.word_count());
        assert_eq!(page.text_length(), page.text().chars().count());
        assert_eq!(page.word_count(), 2);
    }

    #[test]
    fn semantic_metrics_are_cached_on_the_document() {
        let page = extract("<main><p>Markdown only output.</p></main>", None).unwrap();

        let before = page.document().stats();
        assert!(page.markdown().contains("Markdown only output."));
        assert_eq!(page.document().stats(), before);
        assert_eq!(page.word_count(), 3);
    }

    #[test]
    fn text_statistics_include_structural_boundaries() {
        let page = extract(
            "<main><p>Hello</p><p>world</p><table><tr><td>one</td><td>two</td></tr></table><p>before<br>after</p></main>",
            None,
        )
        .unwrap();

        assert_eq!(page.text(), "Hello world one two before after");
        assert_eq!(page.text_length(), page.text().chars().count());
        assert_eq!(page.word_count(), 6);
    }

    #[test]
    fn markdown_builder_controls_links_and_images() {
        let page = extract(
            "<main><p>Hello <a href='/world'>world</a>.</p><img src='image.png' alt='Image'></main>",
            Some("https://example.com/page"),
        )
        .unwrap();
        let markdown = page.markdown_builder().links(false).images(false).render();

        assert!(markdown.contains("Hello world."));
        assert!(!markdown.contains("]("));
        assert!(!markdown.contains("!["));
    }

    #[test]
    fn extracted_page_retains_only_the_semantic_document() {
        let clutter = "<nav><span>irrelevant</span></nav>".repeat(200);
        let html = format!(
            "<html><body>{clutter}<main><p>This is the relevant page content.</p></main></body></html>"
        );
        let page = extract(&html, None).unwrap();

        assert!(
            page.document.len() < 10,
            "retained {} semantic nodes",
            page.document.len()
        );
        assert!(page.text().contains("relevant page content"));
    }

    #[test]
    fn canonical_html_cannot_include_active_source_content() {
        let page = extract(
            r##"<main><p onclick="alert(1)"><a href="java&#x0A;script:alert(1)" onfocus="x">bad</a>
            <a href="/safe">safe</a><img src="data:text/html,bad" onerror="x">
            <svg><script>alert(1)</script><circle onload="x"></circle></svg>
            <svg><a id="target"><text>click</text></a><animate href="#target" attributeName="href" values="javascript:alert(1)" fill="freeze"></animate></svg>
            <iframe srcdoc="<script>x</script>"></iframe></p></main>"##,
            Some("https://example.com/page"),
        )
        .unwrap();

        let raw = page.html();
        let safe = page.safe_html();
        assert_eq!(raw, safe);
        assert!(!raw.contains("onclick="));
        assert!(!safe.to_ascii_lowercase().contains("javascript:"));
        assert!(!safe.contains("onclick="));
        assert!(!safe.contains("onfocus="));
        assert!(!safe.contains("onerror="));
        assert!(!safe.contains("onload="));
        assert!(!safe.contains("<script"));
        assert!(!safe.contains("<animate"));
        assert!(!safe.contains("attributeName="));
        assert!(!safe.contains("values="));
        assert!(!safe.contains("<iframe"));
        assert!(!safe.contains("srcdoc="));
        assert!(!safe.contains("data:text/html"));
        assert!(safe.contains("href=\"https://example.com/safe\""));
        assert_eq!(raw, page.html());
        assert_eq!(safe, page.html_builder().sanitize(true).render());
    }
}
