//! Content extractor configuration and entry point.

use crate::dom::Dom;
use crate::error::Result;
use crate::extraction::ContentExtractor;
use crate::page::ExtractedPage;

/// A Rust-native content root selector.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContentHint {
    /// Matches one element by its exact `id` value.
    Id(String),
    /// Matches elements that contain this class token.
    Class(String),
    /// Matches elements by a common content tag.
    Tag(ContentTag),
}

/// An HTML tag that can identify a content root.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentTag {
    /// The `article` element.
    Article,
    /// The `main` element.
    Main,
    /// The `section` element.
    Section,
    /// The `div` element.
    Div,
}

/// A reusable HTML content extractor.
#[derive(Debug, Clone)]
pub struct Extractor {
    pub(crate) config: ExtractorConfig,
}

/// Builds an [`Extractor`].
#[derive(Debug, Clone)]
pub struct ExtractorBuilder {
    config: ExtractorConfig,
}

#[derive(Debug, Clone)]
pub(crate) struct ExtractorConfig {
    pub(crate) max_elements: usize,
    pub(crate) structured_data: bool,
    pub(crate) top_candidates: usize,
    pub(crate) debug: bool,
    pub(crate) diagnostics: bool,
    pub(crate) metadata_diagnostics: bool,
    pub(crate) retain_structured_data: bool,
    pub(crate) content_hint: Option<ContentHint>,
    pub(crate) content_root: Option<ContentHint>,
}

impl Default for ExtractorConfig {
    fn default() -> Self {
        Self {
            max_elements: 0,
            structured_data: true,
            top_candidates: 5,
            debug: false,
            diagnostics: false,
            metadata_diagnostics: false,
            retain_structured_data: false,
            content_hint: None,
            content_root: None,
        }
    }
}

impl Extractor {
    /// Returns a builder with the default extraction configuration.
    pub fn builder() -> ExtractorBuilder {
        ExtractorBuilder {
            config: ExtractorConfig::default(),
        }
    }

    /// Extracts relevant content and metadata from an HTML document.
    ///
    /// `url` must be an absolute URL when present. Legible uses it to resolve
    /// relative links and media URLs.
    pub fn extract(&self, html: &str, url: Option<&str>) -> Result<ExtractedPage> {
        let dom = Dom::parse_document(html).expect("HTML DOM node limit exceeded");
        ContentExtractor::from_document(dom, url, &self.config).extract()
    }
}

impl Default for Extractor {
    fn default() -> Self {
        Self::builder().build()
    }
}

impl ExtractorBuilder {
    /// Sets the maximum number of HTML elements. Use `0` for no limit.
    pub fn max_elements(mut self, max: usize) -> Self {
        self.config.max_elements = max;
        self
    }

    /// Controls whether JSON-LD participates in metadata extraction.
    pub fn structured_data(mut self, enabled: bool) -> Self {
        self.config.structured_data = enabled;
        self
    }

    /// Controls whether the extracted page retains decision diagnostics.
    ///
    /// Diagnostics are disabled by default. When disabled, extraction does not
    /// build root descriptions or retain attempt records.
    pub fn diagnostics(mut self, enabled: bool) -> Self {
        self.config.diagnostics = enabled;
        self
    }

    /// Retains metadata candidate provenance on each extracted page.
    pub fn metadata_diagnostics(mut self, enabled: bool) -> Self {
        self.config.metadata_diagnostics = enabled;
        self
    }

    /// Retains parsed JSON-LD values on each extracted page.
    ///
    /// Structured data is not retained by default because it can be large.
    pub fn retain_structured_data(mut self, enabled: bool) -> Self {
        self.config.retain_structured_data = enabled;
        self
    }

    /// Adds strong root-selection evidence while keeping quality validation.
    pub fn content_hint(mut self, hint: ContentHint) -> Self {
        self.config.content_hint = Some(hint);
        self
    }

    /// Extracts only the first subtree that matches `root`.
    ///
    /// Extraction returns [`Error::ContentRootNotFound`](crate::Error::ContentRootNotFound)
    /// when no element matches.
    pub fn content_root(mut self, root: ContentHint) -> Self {
        self.config.content_root = Some(root);
        self
    }

    /// Builds the extractor.
    pub fn build(self) -> Extractor {
        Extractor {
            config: self.config,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Error;

    #[test]
    fn builder_defaults_match_extractor_defaults() {
        let built = Extractor::builder().build();
        let default = Extractor::default();
        assert_eq!(built.config.max_elements, default.config.max_elements);
        assert_eq!(built.config.structured_data, default.config.structured_data);
        assert_eq!(built.config.max_elements, 0);
        assert!(built.config.structured_data);
        assert!(!built.config.diagnostics);
    }

    #[test]
    fn builder_sets_public_configuration() {
        let extractor = Extractor::builder()
            .max_elements(123)
            .structured_data(false)
            .diagnostics(true)
            .build();
        assert_eq!(extractor.config.max_elements, 123);
        assert!(!extractor.config.structured_data);
        assert!(extractor.config.diagnostics);
    }

    #[test]
    fn exact_content_root_is_required_and_limits_output() {
        let html = "<main><article id='first'><p>First selected content.</p></article><article id='second'><p>Second excluded content.</p></article></main>";
        let page = Extractor::builder()
            .content_root(ContentHint::Id("first".into()))
            .build()
            .extract(html, None)
            .unwrap();
        assert!(page.text().contains("First selected"));
        assert!(!page.text().contains("Second excluded"));

        let error = Extractor::builder()
            .content_root(ContentHint::Id("missing".into()))
            .build()
            .extract(html, None);
        assert!(matches!(error, Err(Error::ContentRootNotFound)));

        let chrome = Extractor::builder()
            .content_root(ContentHint::Id("chosen-nav".into()))
            .build()
            .extract(
                "<nav id='chosen-nav'><p>Requested navigation details.</p></nav><main><p>Automatic main content.</p></main>",
                None,
            )
            .unwrap();
        assert!(chrome.text().contains("Requested navigation details"));
        assert!(!chrome.text().contains("Automatic main content"));
    }

    #[test]
    fn svg_chart_replacement_preserves_id_and_class_hints() {
        let html = r#"<main>
            <svg id="chosen-chart" class="benchmark-chart">
                <title>Release scores</title>
                <g><text>Alpha</text><text>10</text></g>
                <g><text>Beta</text><text>20</text></g>
            </svg>
            <p>Unrelated content outside the chart.</p>
        </main>"#;
        let by_id = Extractor::builder()
            .content_root(ContentHint::Id("chosen-chart".into()))
            .build()
            .extract(html, None)
            .unwrap();
        assert!(by_id.text().contains("Release scores"));
        assert!(!by_id.text().contains("Unrelated content"));

        let by_class = Extractor::builder()
            .content_root(ContentHint::Class("benchmark-chart".into()))
            .build()
            .extract(html, None)
            .unwrap();
        assert!(by_class.text().contains("Release scores"));
        assert!(!by_class.text().contains("Unrelated content"));
    }

    #[test]
    fn duplicate_class_hints_keep_automatic_quality_selection() {
        let html = "<main><div class='entry'><p>Short note.</p></div><div class='entry'><p>This is the substantial article. It contains enough useful detail to make the preferred content clear. The second paragraph continues the explanation with practical context.</p><p>More relevant details complete the article.</p></div></main>";
        let page = Extractor::builder()
            .content_hint(ContentHint::Class("entry".into()))
            .build()
            .extract(html, None)
            .unwrap();
        assert!(page.text().contains("substantial article"));
    }

    #[test]
    fn diagnostics_report_caller_hint_evidence() {
        let html = "<main><div id='preferred'><p>The caller identified this useful content. It has enough detail for extraction and a clear sentence.</p><p>A second paragraph provides more useful context.</p></div></main>";
        let page = Extractor::builder()
            .content_hint(ContentHint::Id("preferred".into()))
            .diagnostics(true)
            .build()
            .extract(html, None)
            .unwrap();
        let accepted = page
            .diagnostics()
            .unwrap()
            .attempts
            .iter()
            .find(|attempt| attempt.accepted)
            .unwrap();
        assert!(
            accepted
                .selected_root
                .candidate_sources
                .contains(&crate::diagnostics::CandidateSourceInfo::CallerHint)
        );
    }

    #[test]
    fn diagnostics_report_cleanup_and_normalized_structures() {
        let html = r#"<main><article><h1>Technical guide</h1><p>This guide explains a complete process with enough useful detail for extraction. It provides stable context and a clear result.</p><pre><code class='language-rust'>fn main() {}</code></pre><p>More substantial prose completes the guide and keeps the selected article coherent.</p><aside class='newsletter'><p>Join our newsletter</p><form><input type='email'><button>Subscribe</button></form></aside></article></main>"#;
        let page = Extractor::builder()
            .diagnostics(true)
            .build()
            .extract(html, None)
            .unwrap();
        let attempt = page
            .diagnostics()
            .unwrap()
            .attempts
            .iter()
            .find(|attempt| attempt.accepted)
            .unwrap();

        assert_eq!(attempt.normalization.code_blocks, 1);
        assert_eq!(
            attempt.representation.source_dom_nodes,
            crate::dom::Dom::parse_document(html).unwrap().len()
        );
        assert!(attempt.representation.final_dom_nodes < attempt.representation.source_dom_nodes);
        assert_eq!(
            attempt.representation.document_nodes,
            page.semantic_node_count()
        );
        assert_eq!(
            attempt.representation.estimated_document_bytes,
            page.semantic_retained_bytes()
        );
        assert!(attempt.cleanup_actions.iter().any(|action| {
            action.kind == crate::diagnostics::CleanupActionKind::HeuristicCleanup
                && action.removed_elements > 0
        }));
    }

    #[test]
    fn diagnostics_count_normalized_semantics() {
        let html = r##"<main><article><h1>Reference guide</h1><p>This guide explains the semantic examples with enough useful context for stable extraction.<a href="#note" role="doc-noteref">1</a></p><pre><code>fn main() {}</code></pre><math><mi>x</mi></math><figure><img src="diagram.png" alt="Diagram"></figure><table><tr><th>Name</th><th>Value</th></tr><tr><td>A</td><td>1</td></tr></table><table role="presentation"><tr><td><p>Layout prose remains readable.</p></td></tr></table><aside id="note" role="doc-footnote">A useful note.</aside><p>A final paragraph provides more substantial content and a clear conclusion.</p></article></main>"##;
        let page = Extractor::builder()
            .diagnostics(true)
            .build()
            .extract(html, None)
            .unwrap();
        let counts = page
            .diagnostics()
            .unwrap()
            .attempts
            .iter()
            .find(|attempt| attempt.accepted)
            .unwrap()
            .normalization;

        assert_eq!(counts.code_blocks, 1);
        assert_eq!(counts.footnote_references, 1);
        assert_eq!(counts.footnote_definitions, 1);
        assert_eq!(counts.math_expressions, 1);
        assert_eq!(counts.images, 1);
        assert_eq!(counts.tables, 1);
        assert_eq!(counts.flattened_layout_tables, 1);
    }

    #[test]
    fn diagnostics_count_all_compiler_code_shapes() {
        let html = r#"<main><article><h1>Code reference</h1><p>This guide contains several code forms with enough useful explanation for stable extraction and diagnostics.</p><pre>plain preformatted text</pre><div><code>orphan
multiline</code></div><table role="presentation" class="highlighttable"><tr><td class="linenos"><pre>1</pre></td><td><pre><code>gutter source</code></pre></td></tr></table><p>A final paragraph explains the examples and provides a complete conclusion for the reader.</p></article></main>"#;
        let page = Extractor::builder()
            .diagnostics(true)
            .build()
            .extract(html, None)
            .unwrap();
        let counts = page
            .diagnostics()
            .unwrap()
            .attempts
            .iter()
            .find(|attempt| attempt.accepted)
            .unwrap()
            .normalization;

        assert_eq!(counts.code_blocks, 3);
        assert_eq!(counts.tables, 0);
    }

    #[test]
    fn diagnostics_report_specialized_extractor_identity() {
        let page = Extractor::builder()
            .diagnostics(true)
            .build()
            .extract(
                include_str!("../tests/specialized/hacker-news-listing/source.html"),
                Some("https://news.ycombinator.com/"),
            )
            .unwrap();

        assert_eq!(
            page.diagnostics().unwrap().specialized_extractor.as_deref(),
            Some("hacker-news")
        );
    }

    #[test]
    fn poor_hint_does_not_override_better_automatic_content() {
        let html = "<body><aside id='poor-hint'><p>Brief sidebar note.</p></aside><article id='article'><h1>Substantial article</h1><p>This article explains the main subject in detail. It gives readers the context that they need to understand the result, and it contains several complete sentences.</p><p>The second paragraph adds practical evidence, examples, and a clear conclusion. This is the useful page content that automatic extraction should retain.</p></article></body>";
        let page = Extractor::builder()
            .content_hint(ContentHint::Id("poor-hint".into()))
            .diagnostics(true)
            .build()
            .extract(html, None)
            .unwrap();
        assert!(page.text().contains("article explains the main subject"));
        assert!(!page.text().contains("Brief sidebar note"));
        let accepted = page
            .diagnostics()
            .unwrap()
            .attempts
            .iter()
            .find(|attempt| attempt.accepted)
            .unwrap();
        assert!(
            !accepted
                .selected_root
                .candidate_sources
                .contains(&crate::diagnostics::CandidateSourceInfo::CallerHint)
        );
    }

    #[test]
    fn exact_root_does_not_adopt_external_footnotes() {
        let html = "<main><article id='chosen'><p>Selected text.<sup><a href='#note-1' role='doc-noteref'>1</a></sup></p></article><section class='footnotes'><ol><li id='note-1' role='doc-footnote'>Outside definition must stay outside.</li></ol></section></main>";
        let page = Extractor::builder()
            .content_root(ContentHint::Id("chosen".into()))
            .build()
            .extract(html, None)
            .unwrap();
        assert!(page.text().contains("Selected text"));
        assert!(!page.text().contains("Outside definition"));
    }

    #[test]
    fn metadata_retention_is_opt_in() {
        let html = r#"<html><head><title>HTML title</title><meta property="og:title" content="OpenGraph title"><script type="application/ld+json">{"@context":"https://schema.org","@type":"Article","headline":"Schema title"}</script></head><body><main><p>Useful content for this page.</p></main></body></html>"#;
        let default = Extractor::default().extract(html, None).unwrap();
        assert!(default.metadata_diagnostics().is_none());
        assert!(default.structured_data().is_none());

        let retained = Extractor::builder()
            .metadata_diagnostics(true)
            .retain_structured_data(true)
            .build()
            .extract(html, None)
            .unwrap();
        let title = &retained.metadata_diagnostics().unwrap().title;
        assert!(title.selected.is_some());
        assert!(!title.alternatives.is_empty());
        assert!(
            retained
                .structured_data()
                .is_some_and(|items| !items.is_empty())
        );

        let disabled = Extractor::builder()
            .structured_data(false)
            .metadata_diagnostics(true)
            .retain_structured_data(true)
            .build()
            .extract(html, None)
            .unwrap();
        assert!(
            disabled
                .metadata_diagnostics()
                .unwrap()
                .title
                .selected
                .as_ref()
                .is_none_or(|value| value.source != crate::metadata::MetadataSource::JsonLd)
        );
        assert!(disabled.structured_data().unwrap().is_empty());
    }

    #[test]
    fn metadata_diagnostics_report_the_highest_confidence_duplicate() {
        let html = r#"<html><head><meta property="og:title" content="Shared title"><meta name="dc:title" content="Shared title"><meta name="author" content="Ada"><meta name="dc:creator" content="Ada"></head><body><main><p>Useful page content for metadata selection.</p></main></body></html>"#;
        let page = Extractor::builder()
            .metadata_diagnostics(true)
            .build()
            .extract(html, None)
            .unwrap();
        let diagnostics = page.metadata_diagnostics().unwrap();
        assert_eq!(
            diagnostics.title.selected.as_ref().unwrap().source,
            crate::metadata::MetadataSource::DublinCore
        );
        assert_eq!(
            diagnostics.authors.selected[0].source,
            crate::metadata::MetadataSource::DublinCore
        );
    }

    #[test]
    fn max_elements_is_enforced() {
        let extractor = Extractor::builder().max_elements(1).build();
        assert!(matches!(
            extractor.extract("<main><p>Content</p></main>", None),
            Err(Error::TooManyElements(_, 1))
        ));
    }

    #[test]
    fn structured_data_can_be_disabled() {
        let html = r#"<html><head><title>Page</title>
            <script type="application/ld+json">
            {"@context":"https://schema.org","@type":"Article","author":[{"name":"Ada"},{"name":"Grace"}]}
            </script></head><body><main><p>Useful page content.</p></main></body></html>"#;
        let enabled = Extractor::default().extract(html, None).unwrap();
        let disabled = Extractor::builder()
            .structured_data(false)
            .build()
            .extract(html, None)
            .unwrap();

        assert_eq!(enabled.metadata().authors, ["Ada", "Grace"]);
        assert!(disabled.metadata().authors.is_empty());
    }
}
