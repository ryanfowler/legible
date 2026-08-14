//! The extracted page and lazy output builders.

use crate::diagnostics::ExtractionDiagnostics;
use crate::dom::{Dom, NodeId};
use crate::metadata::{Metadata, MetadataDiagnostics};
use serde_json::Value;
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy)]
struct ContentStats {
    text_length: usize,
    word_count: usize,
}

/// Relevant page content and metadata.
///
/// Output formats are rendered lazily from one semantic document. Calling a
/// render method more than once produces the same output.
pub struct ExtractedPage {
    metadata: Metadata,
    #[allow(dead_code)] // Retained temporarily for renderer parity.
    dom: Dom,
    #[allow(dead_code)] // Retained temporarily for renderer parity.
    root: NodeId,
    stats: OnceLock<ContentStats>,
    text_length_hint: usize,
    diagnostics: Option<ExtractionDiagnostics>,
    metadata_diagnostics: Option<MetadataDiagnostics>,
    structured_data: Option<Vec<Value>>,
    document: crate::document::Document,
}

impl ExtractedPage {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        dom: Dom,
        root: NodeId,
        metadata: Metadata,
        text_length: usize,
        diagnostics: Option<ExtractionDiagnostics>,
        metadata_diagnostics: Option<MetadataDiagnostics>,
        structured_data: Option<Vec<Value>>,
        _compile_base_url: Option<&url::Url>,
    ) -> crate::Result<Self> {
        let document = crate::document::compile_document(
            &dom,
            root,
            &crate::document::CompileContext::new(_compile_base_url.cloned()),
        )
        .map_err(|_| crate::Error::NoContent)?;
        Ok(Self {
            metadata,
            dom,
            root,
            stats: OnceLock::new(),
            text_length_hint: text_length,
            diagnostics,
            metadata_diagnostics,
            structured_data,
            document,
        })
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
        let stats = self.stats();
        crate::render::text::render_text(
            &self.document,
            stats.text_length,
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
        self.stats().word_count
    }

    /// Returns the number of characters in the normalized extracted text.
    pub fn text_length(&self) -> usize {
        self.stats().text_length
    }

    fn stats(&self) -> ContentStats {
        *self.stats.get_or_init(|| {
            let stats = crate::render::text::measure_text(&self.document);
            ContentStats {
                text_length: stats.text_length,
                word_count: stats.word_count,
            }
        })
    }

    /// Checks retained DOM links for fuzz testing.
    #[doc(hidden)]
    #[cfg(feature = "fuzzing")]
    pub fn validate_dom(&self) -> bool {
        self.dom.validate().is_ok()
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
        crate::render::html::render_html(&self.page.document, self.page.text_length_hint)
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
            self.page.text_length_hint,
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
    fn semantic_renderers_match_legacy_fixture_output() {
        let mut sources = Vec::new();
        for root in ["tests/general", "tests/defuddle", "tests/specialized"] {
            fixture_sources(Path::new(root), &mut sources);
        }
        sources.sort();

        // The semantic compiler removes source-only wrapper boundaries. These
        // fixtures intentionally differ only in redundant blank lines. The
        // escaping fixture also proves that equivalent adjacent text has one
        // deterministic representation after text-node merging.
        let allowed_markdown_differences = [
            "tests/defuddle/footnotes/org-mode-sidenotes/source.html",
            "tests/general/email-contact-form/source.html",
            "tests/general/markdown-escaping/source.html",
            "tests/general/newsletter-like-recovery-form/source.html",
            "tests/general/repeated-navigation-pricing/source.html",
            "tests/general/subscription-settings-form/source.html",
            "tests/specialized/discourse-topic/source.html",
            "tests/specialized/github-issue/source.html",
            "tests/specialized/github-pull-request/source.html",
            "tests/specialized/reddit-static-thread/source.html",
        ];
        let mut markdown_differences = Vec::new();
        let mut text_differences = Vec::new();
        for source in sources {
            if source
                .parent()
                .is_some_and(|directory| directory.join("expected.error").exists())
            {
                continue;
            }
            let html = std::fs::read_to_string(&source).unwrap();
            let Ok(page) = extract(&html, Some("https://example.test/docs/page.html")) else {
                continue;
            };
            let legacy_markdown = crate::markdown::render_markdown(
                &page.dom,
                page.root,
                page.text_length_hint,
                crate::markdown::MarkdownConfig::default(),
            );
            if legacy_markdown != page.markdown()
                && !allowed_markdown_differences
                    .iter()
                    .any(|allowed| source == Path::new(allowed))
            {
                markdown_differences.push(source.clone());
            }
            let legacy_text = crate::text::render_text(
                &page.dom,
                page.root,
                page.text_length_hint,
                &crate::text::TextOptions::default(),
            );
            // Semantic footnote references have no visible marker text. The
            // compiler also inserts a word boundary between adjacent source
            // wrappers when omission would join two words. Both changes remove
            // source implementation details from normalized text.
            let intentional_text_change = source
                .components()
                .any(|component| component.as_os_str() == "footnotes")
                || [
                    "tests/general/inline-boundaries/source.html",
                    "tests/general/job-company-profile/source.html",
                    "tests/general/markdown-escaping/source.html",
                ]
                .iter()
                .any(|allowed| source == Path::new(allowed));
            if legacy_text != page.text() && !intentional_text_change {
                text_differences.push(source);
            }
        }

        assert!(
            markdown_differences.is_empty(),
            "Markdown parity failures: {markdown_differences:?}"
        );
        assert!(
            text_differences.is_empty(),
            "text parity failures: {text_differences:?}"
        );
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
        assert_eq!(page.text_length(), page.text().chars().count());
        assert_eq!(page.word_count(), 2);
    }

    #[test]
    fn markdown_does_not_measure_text_statistics_eagerly() {
        let page = extract("<main><p>Markdown only output.</p></main>", None).unwrap();

        assert!(page.stats.get().is_none());
        assert!(page.markdown().contains("Markdown only output."));
        assert!(page.stats.get().is_none());
        assert_eq!(page.word_count(), 3);
        assert!(page.stats.get().is_some());
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
    fn extracted_page_retains_only_the_selected_fragment() {
        let clutter = "<nav><span>irrelevant</span></nav>".repeat(200);
        let html = format!(
            "<html><body>{clutter}<main><p>This is the relevant page content.</p></main></body></html>"
        );
        let page = extract(&html, None).unwrap();

        assert!(page.dom.len() < 20, "retained {} DOM nodes", page.dom.len());
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
