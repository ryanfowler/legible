//! The extracted page and lazy output builders.

use crate::diagnostics::ExtractionDiagnostics;
use crate::document::Document;
use crate::metadata::{Metadata, MetadataDiagnostics};
use serde_json::Value;

/// Relevant page content and metadata.
///
/// Output formats are rendered lazily from one private semantic representation.
/// Calling a render method more than once produces the same output.
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
        let _phase =
            crate::instrumentation::PhaseGuard::new(crate::instrumentation::Phase::Rendering);
        self.document.text()
    }

    /// Renders the extracted content as canonical semantic HTML.
    ///
    /// The private semantic representation cannot contain active source elements,
    /// arbitrary attributes, or unsupported URI schemes.
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

    /// Returns the number of characters contributed by link content.
    pub fn link_text_length(&self) -> usize {
        self.document.link_text_length()
    }

    /// Returns the fraction of normalized text contributed by links.
    pub fn link_density(&self) -> f64 {
        self.document.link_density()
    }

    /// Returns the number of semantic paragraphs.
    pub fn paragraph_count(&self) -> usize {
        self.document.paragraph_count()
    }

    /// Returns the number of semantic headings.
    pub fn heading_count(&self) -> usize {
        self.document.heading_count()
    }

    /// Returns the number of semantic list items.
    pub fn list_item_count(&self) -> usize {
        self.document.list_item_count()
    }

    /// Returns the number of semantic code blocks.
    pub fn code_block_count(&self) -> usize {
        self.document.code_block_count()
    }

    /// Returns the number of semantic data tables.
    pub fn table_count(&self) -> usize {
        self.document.table_count()
    }

    /// Returns the number of semantic figures.
    pub fn figure_count(&self) -> usize {
        self.document.figure_count()
    }

    /// Returns the number of semantic images.
    pub fn image_count(&self) -> usize {
        self.document.image_count()
    }

    /// Returns the number of footnote references.
    pub fn footnote_reference_count(&self) -> usize {
        self.document.footnote_reference_count()
    }

    /// Returns the number of footnote definitions.
    pub fn footnote_definition_count(&self) -> usize {
        self.document.footnote_definition_count()
    }

    /// Returns the number of math expressions.
    pub fn math_count(&self) -> usize {
        self.document.math_count()
    }

    /// Returns the number of blocks with useful structural evidence.
    pub fn structured_block_count(&self) -> usize {
        self.document.structured_block_count()
    }

    /// Returns whether normalized text contains an alphanumeric character.
    pub fn has_alphanumeric_text(&self) -> bool {
        self.document.stats().has_alphanumeric_text
    }

    /// Returns the number of alphabetic characters in normalized text.
    pub fn alphabetic_chars(&self) -> usize {
        self.document.stats().alphabetic_chars
    }

    /// Returns the number of numeric characters in normalized text.
    pub fn digit_chars(&self) -> usize {
        self.document.stats().digit_chars
    }

    /// Returns whether the result contains contextual semantic structure.
    pub fn has_contextual_structure(&self) -> bool {
        self.document.has_contextual_structure()
    }

    #[cfg(test)]
    pub(crate) fn semantic_node_count(&self) -> usize {
        self.document.len()
    }

    #[cfg(test)]
    pub(crate) fn semantic_retained_bytes(&self) -> usize {
        self.document.retained_bytes_estimate()
    }

    /// Checks private semantic representation invariants for fuzz testing.
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
        let _phase =
            crate::instrumentation::PhaseGuard::new(crate::instrumentation::Phase::Rendering);
        let _ = self.sanitize;
        crate::render::html::render_html(
            &self.page.document,
            self.page.document.output_capacity_hint(),
        )
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
        let _phase =
            crate::instrumentation::PhaseGuard::new(crate::instrumentation::Phase::Rendering);
        crate::render::markdown::render_markdown(
            &self.page.document,
            self.page.document.output_capacity_hint(),
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
        assert_eq!(page.image_count(), 1);
    }

    #[test]
    fn first_markdown_render_does_not_initialize_stats() {
        let page = extract("<main><p>Markdown output.</p></main>", None).unwrap();

        assert!(!page.document.stats_initialized());
        assert_eq!(page.markdown(), "Markdown output.\n");
        assert!(!page.document.stats_initialized());
    }

    #[test]
    fn first_html_render_does_not_initialize_stats() {
        let page = extract("<main><p>HTML output.</p></main>", None).unwrap();

        assert!(!page.document.stats_initialized());
        assert_eq!(page.html(), "<div><p>HTML output.</p></div>");
        assert!(!page.document.stats_initialized());
    }

    #[test]
    fn first_text_render_initializes_stats_during_the_text_walk() {
        let page = extract("<main><p>Text output.</p></main>", None).unwrap();

        assert!(!page.document.stats_initialized());
        assert_eq!(page.text(), "Text output.");
        assert!(page.document.stats_initialized());
        assert_eq!(page.text_length(), 12);
        assert_eq!(page.word_count(), 2);
    }

    #[test]
    fn cached_stats_keep_text_rendering_deterministic() {
        let page = extract("<main><p>Cached text.</p></main>", None).unwrap();

        assert_eq!(page.word_count(), 2);
        assert_eq!(page.text(), "Cached text.");
        assert_eq!(page.text(), "Cached text.");
    }

    #[test]
    fn semantic_metrics_are_cached_on_the_document() {
        let page = extract("<main><p>Markdown only output.</p></main>", None).unwrap();

        let before = page.document.stats();
        assert!(page.markdown().contains("Markdown only output."));
        assert_eq!(page.document.stats(), before);
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
    fn disabled_images_do_not_make_an_image_only_heading_visible() {
        let page = extract(
            "<main><h2><img src='diagram.png' alt='Diagram'></h2><p>Content.</p></main>",
            None,
        )
        .unwrap();
        let markdown = page.markdown_builder().images(false).render();

        assert_eq!(markdown, "Content.\n");
    }

    #[test]
    fn extracted_page_retains_only_the_private_semantic_representation() {
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
