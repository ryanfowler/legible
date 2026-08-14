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
/// Output formats are rendered lazily from the extracted DOM. Calling a render
/// method more than once produces the same output.
pub struct ExtractedPage {
    metadata: Metadata,
    dom: Dom,
    root: NodeId,
    stats: OnceLock<ContentStats>,
    text_length_hint: usize,
    diagnostics: Option<ExtractionDiagnostics>,
    metadata_diagnostics: Option<MetadataDiagnostics>,
    structured_data: Option<Vec<Value>>,
    #[cfg(test)]
    _diagnostic_document: crate::document::Document,
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
    ) -> Self {
        #[cfg(test)]
        let diagnostic_document = crate::document::compile_document(
            &dom,
            root,
            &crate::document::CompileContext::new(_compile_base_url.cloned()),
        )
        .expect("the normalized extraction DOM must compile to a valid semantic document");
        Self {
            metadata,
            dom,
            root,
            stats: OnceLock::new(),
            text_length_hint: text_length,
            diagnostics,
            metadata_diagnostics,
            structured_data,
            #[cfg(test)]
            _diagnostic_document: diagnostic_document,
        }
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
        crate::text::render_text(
            &self.dom,
            self.root,
            stats.text_length,
            &crate::text::TextOptions::default(),
        )
    }

    /// Renders the extracted content as an HTML fragment.
    ///
    /// This HTML is not sanitized. Apply a sanitizer before you insert it into
    /// an untrusted page.
    pub fn html(&self) -> String {
        self.html_builder().render()
    }

    /// Renders a sanitized HTML fragment for normal downstream display.
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
            let (text_length, word_count) = crate::text::measure_text(&self.dom, self.root);
            ContentStats {
                text_length,
                word_count,
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
    /// Controls whether unsafe elements, attributes, and URLs are removed.
    pub fn sanitize(mut self, enabled: bool) -> Self {
        self.sanitize = enabled;
        self
    }

    /// Renders the configured HTML output.
    pub fn render(self) -> String {
        if self.sanitize {
            crate::html::render_safe_html(
                &self.page.dom,
                self.page.root,
                self.page.text_length_hint,
            )
        } else {
            crate::dom::render_html(&self.page.dom, self.page.root, self.page.text_length_hint)
        }
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
        crate::markdown::render_markdown(
            &self.page.dom,
            self.page.root,
            self.page.text_length_hint,
            crate::markdown::MarkdownConfig {
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
    fn safe_html_removes_active_content_without_changing_raw_html() {
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
        assert!(raw.contains("onclick="));
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
