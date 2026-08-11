//! The extracted page and lazy output builders.

use crate::diagnostics::ExtractionDiagnostics;
use crate::dom::{Dom, NodeId};
use crate::metadata::Metadata;

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
    stats: ContentStats,
    diagnostics: Option<ExtractionDiagnostics>,
}

impl ExtractedPage {
    pub(crate) fn new(
        dom: Dom,
        root: NodeId,
        metadata: Metadata,
        _text_length: usize,
        diagnostics: Option<ExtractionDiagnostics>,
    ) -> Self {
        let (text_length, word_count) = crate::text::measure_text(&dom, root);
        Self {
            metadata,
            dom,
            root,
            stats: ContentStats {
                text_length,
                word_count,
            },
            diagnostics,
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
        crate::text::render_text(
            &self.dom,
            self.root,
            self.stats.text_length,
            &crate::text::TextOptions::default(),
        )
    }

    /// Renders the extracted content as an HTML fragment.
    ///
    /// This HTML is not sanitized. Apply a sanitizer before you insert it into
    /// an untrusted page.
    pub fn html(&self) -> String {
        crate::dom::render_html(&self.dom, self.root, self.stats.text_length)
    }

    /// Returns the number of words in the normalized extracted text.
    pub fn word_count(&self) -> usize {
        self.stats.word_count
    }

    /// Returns the number of characters in the normalized extracted text.
    pub fn text_length(&self) -> usize {
        self.stats.text_length
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
            self.page.stats.text_length,
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
}
