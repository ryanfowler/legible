//! Content extractor configuration and entry point.

use crate::budget::ParseBudget;
use crate::dom::Dom;
use crate::error::{ResourceLimitKind, Result};
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
///
/// An extractor stores configuration. It does not store a parsed document.
/// You can use one extractor for many documents. Each extraction creates its
/// own document state.
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
    pub(crate) parse_budget: ParseBudget,
    pub(crate) structured_data: bool,
    pub(crate) diagnostics: bool,
    pub(crate) metadata_diagnostics: bool,
    pub(crate) retain_structured_data: bool,
    pub(crate) content_hint: Option<ContentHint>,
    pub(crate) content_root: Option<ContentHint>,
}

impl Default for ExtractorConfig {
    fn default() -> Self {
        Self {
            parse_budget: ParseBudget::default(),
            structured_data: true,
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
    ///
    /// # Errors
    ///
    /// Returns a parser or resource-limit error when the input cannot be
    /// parsed within the configured budget. It can also return
    /// [`crate::Error::NoBody`], [`crate::Error::NoContent`],
    /// [`crate::Error::InvalidUrl`], or
    /// [`Error::ContentRootNotFound`](crate::Error::ContentRootNotFound).
    pub fn extract(&self, html: &str, url: Option<&str>) -> Result<ExtractedPage> {
        if self.config.parse_budget.max_input_bytes > 0
            && html.len() > self.config.parse_budget.max_input_bytes
        {
            return Err(crate::error::Error::ResourceLimit {
                resource: ResourceLimitKind::InputBytes,
                limit: self.config.parse_budget.max_input_bytes,
            });
        }
        let dom = Dom::parse_document_with_budget(html, &self.config.parse_budget)
            .map_err(crate::error::Error::from_parse_error)?;
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
        self.config.parse_budget.max_elements = max;
        self
    }

    /// Sets all parser and structured-data limits.
    pub fn parse_budget(mut self, budget: ParseBudget) -> Self {
        self.config.parse_budget = budget;
        self
    }

    /// Sets the maximum input size in bytes. Use `0` for no limit.
    pub fn max_input_bytes(mut self, max: usize) -> Self {
        self.config.parse_budget.max_input_bytes = max;
        self
    }

    /// Sets the maximum number of allocated DOM nodes. Use `0` for no limit.
    pub fn max_nodes(mut self, max: usize) -> Self {
        self.config.parse_budget.max_nodes = max;
        self
    }

    /// Sets the maximum number of attributes across the document. Use `0` for no limit.
    pub fn max_total_attributes(mut self, max: usize) -> Self {
        self.config.parse_budget.max_total_attributes = max;
        self
    }

    /// Sets the maximum number of attributes on one element. Use `0` for no limit.
    pub fn max_attributes_per_element(mut self, max: usize) -> Self {
        self.config.parse_budget.max_attributes_per_element = max;
        self
    }

    /// Sets the maximum number of text bytes in the DOM. Use `0` for no limit.
    pub fn max_text_bytes(mut self, max: usize) -> Self {
        self.config.parse_budget.max_text_bytes = max;
        self
    }

    /// Sets the maximum element nesting depth. Use `0` for no limit.
    pub fn max_depth(mut self, max: usize) -> Self {
        self.config.parse_budget.max_depth = max;
        self
    }

    /// Sets the maximum total JSON-LD script size in bytes. Use `0` for no limit.
    pub fn max_json_ld_bytes(mut self, max: usize) -> Self {
        self.config.parse_budget.max_json_ld_bytes = max;
        self
    }

    /// Sets the maximum number of typed JSON-LD items. Use `0` for no limit.
    pub fn max_json_ld_items(mut self, max: usize) -> Self {
        self.config.parse_budget.max_json_ld_items = max;
        self
    }

    /// Sets the maximum JSON-LD nesting depth. Use `0` for the internal safety cap.
    pub fn max_json_ld_depth(mut self, max: usize) -> Self {
        self.config.parse_budget.max_json_ld_depth = max;
        self
    }

    /// Controls whether JSON-LD participates in metadata and root selection.
    ///
    /// This option is enabled by default. It is separate from
    /// [`Self::retain_structured_data`].
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
    /// When retention is disabled, [`ExtractedPage::structured_data`] returns
    /// `None`. When retention is enabled, it returns `Some`, even when no valid
    /// JSON-LD items were found.
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
    use crate::{CandidateSourceInfo, Error, ExtractionStrategyInfo, RootSelectionReasonInfo};

    #[test]
    fn builder_defaults_match_extractor_defaults() {
        let built = Extractor::builder().build();
        let default = Extractor::default();
        assert_eq!(built.config.parse_budget, default.config.parse_budget);
        assert_eq!(built.config.structured_data, default.config.structured_data);
        assert_eq!(built.config.parse_budget.max_elements, 0);
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
        assert_eq!(extractor.config.parse_budget.max_elements, 123);
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
    fn exact_content_root_preserves_metadata_and_semantic_tables() {
        let html = r#"<html><head><title>Article title</title></head><body>
            <main><article id="chosen">
                <h1>Article title</h1>
                <p class="byline">By Ada Lovelace</p>
                <p>This selected article contains enough useful detail for exact-root extraction and metadata checks.</p>
                <table><tr><th>Field</th><th>Value</th></tr><tr><td>Status</td><td>Ready</td></tr></table>
            </article><p>Excluded surrounding content.</p></main>
        </body></html>"#;
        let page = Extractor::builder()
            .content_root(ContentHint::Id("chosen".into()))
            .build()
            .extract(html, None)
            .unwrap();

        assert_eq!(page.metadata().title.as_deref(), Some("Article title"));
        assert!(
            page.metadata()
                .authors
                .iter()
                .any(|author| author.contains("Ada"))
        );
        assert!(!page.text().contains("Article title"));
        assert!(page.markdown().contains("| Field | Value |"));
        assert!(!page.text().contains("Excluded surrounding"));
    }

    #[test]
    fn exact_content_root_reports_one_deterministic_diagnostic_attempt() {
        let page = Extractor::builder()
            .content_root(ContentHint::Tag(ContentTag::Article))
            .diagnostics(true)
            .build()
            .extract(
                "<body><article><p>The requested article has enough useful content for deterministic extraction.</p><p>A second paragraph keeps it meaningful.</p></article><article><p>Excluded content.</p></article></body>",
                None,
            )
            .unwrap();
        let diagnostics = page.diagnostics().unwrap();
        assert_eq!(
            diagnostics.selected_strategy,
            ExtractionStrategyInfo::Normal
        );
        assert_eq!(diagnostics.attempts.len(), 1);
        let attempt = &diagnostics.attempts[0];
        assert!(attempt.accepted);
        assert_eq!(
            attempt.selected_root.selection_reason,
            RootSelectionReasonInfo::SpecificChild
        );
        assert_eq!(
            attempt.selected_root.candidate_sources,
            vec![CandidateSourceInfo::CallerHint]
        );
        assert_eq!(attempt.selected_root.tag.as_deref(), Some("article"));
    }

    #[test]
    fn diagnostics_identify_article_body_root_selection() {
        let page = Extractor::builder()
            .diagnostics(true)
            .build()
            .extract(
                r#"<body><main><article><h1>Quiet laptop fans</h1><section itemprop="articleBody"><p>A quiet laptop fan reduces noise during long compilation jobs.</p><p>The measured result remained consistent after six hours of repeated builds.</p></section></article><div itemprop="notArticleBody">Unrelated metadata.</div></main></body>"#,
                None,
            )
            .unwrap();
        let attempt = page
            .diagnostics()
            .unwrap()
            .attempts
            .iter()
            .find(|attempt| attempt.accepted)
            .unwrap();

        assert_eq!(
            attempt.selected_root.selection_reason,
            RootSelectionReasonInfo::ArticleBody
        );
        assert_eq!(attempt.selected_root.tag.as_deref(), Some("section"));
        assert!(page.text().contains("six hours of repeated builds"));
        assert!(!page.text().contains("Unrelated metadata"));
    }

    #[test]
    fn exact_content_root_rejects_a_root_without_meaningful_text() {
        let error = Extractor::builder()
            .content_root(ContentHint::Id("empty".into()))
            .build()
            .extract(
                "<body><div id='empty'><img src='empty.png'></div><main><p>Other useful content remains outside the requested root.</p></main></body>",
                None,
            );
        assert!(matches!(error, Err(Error::NoContent)));
    }

    #[test]
    fn exact_content_root_normalizes_direct_phrasing_content() {
        let page = Extractor::builder()
            .content_root(ContentHint::Id("root".into()))
            .build()
            .extract(
                "<body><div id='root'>Direct <em>phrasing</em> content.</div><p>Excluded content.</p></body>",
                None,
            )
            .unwrap();

        assert!(
            page.html()
                .contains("<p>Direct <em>phrasing</em> content.</p>")
        );
        assert!(!page.text().contains("Excluded content"));
    }

    #[test]
    fn exact_content_root_preserves_hidden_math_with_tex_annotation() {
        let page = Extractor::builder()
            .content_root(ContentHint::Id("root".into()))
            .build()
            .extract(
                r#"<body><div id="root"><p>The selected explanation includes an equation.</p><math style="opacity:0"><semantics><mrow><mi>E</mi><mo>=</mo><mi>m</mi><msup><mi>c</mi><mn>2</mn></msup></mrow><annotation encoding="application/x-tex">E=mc^2</annotation></semantics></math><p>The explanation continues after the equation.</p></div></body>"#,
                None,
            )
            .unwrap();

        assert!(page.markdown().contains("$E=mc^2$"));
    }

    #[test]
    fn exact_content_root_drops_hidden_and_modal_wrappers() {
        let page = Extractor::builder()
            .content_root(ContentHint::Id("root".into()))
            .build()
            .extract(
                r#"<body><div id="root"><p>Visible selected content remains in the result.</p><div hidden><p>Hidden content must not leak from the requested root.</p></div><div role="dialog"><p>Modal content must not leak from the requested root.</p></div></div></body>"#,
                None,
            )
            .unwrap();

        assert!(page.text().contains("Visible selected content"));
        assert!(!page.text().contains("Hidden content"));
        assert!(!page.text().contains("Modal content"));
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
            Err(Error::TooManyElements { limit: 1, .. })
        ));
    }

    #[test]
    fn parser_budgets_fail_before_extraction_without_panicking() {
        let cases = [
            (
                Extractor::builder().max_input_bytes(4).build(),
                "<p>content</p>",
                ResourceLimitKind::InputBytes,
            ),
            (
                Extractor::builder().max_nodes(1).build(),
                "<p>content</p>",
                ResourceLimitKind::DomNodes,
            ),
            (
                Extractor::builder().max_total_attributes(1).build(),
                "<p id='a' class='b'>content</p>",
                ResourceLimitKind::TotalAttributes,
            ),
            (
                Extractor::builder().max_attributes_per_element(1).build(),
                "<p id='a' class='b'>content</p>",
                ResourceLimitKind::AttributesPerElement,
            ),
            (
                Extractor::builder().max_text_bytes(2).build(),
                "<p>content</p>",
                ResourceLimitKind::TextBytes,
            ),
            (
                Extractor::builder().max_depth(1).build(),
                "<div><div>content</div></div>",
                ResourceLimitKind::ElementDepth,
            ),
        ];

        for (extractor, html, resource) in cases {
            assert!(matches!(
                extractor.extract(html, None),
                Err(Error::ResourceLimit {
                    resource: actual,
                    ..
                }) if actual == resource
            ));
        }
    }

    #[test]
    fn json_ld_budgets_fail_before_structured_data_is_retained() {
        let html = r#"<html><head><script type="application/ld+json">[
            {"@context":"https://schema.org","@type":"Article","headline":"One"},
            {"@context":"https://schema.org","@type":"Article","headline":"Two"}
        ]</script></head><body><main><p>Useful page content.</p></main></body></html>"#;

        assert!(matches!(
            Extractor::builder()
                .max_json_ld_bytes(8)
                .build()
                .extract(html, None),
            Err(Error::ResourceLimit {
                resource: ResourceLimitKind::JsonLdBytes,
                ..
            })
        ));
        assert!(matches!(
            Extractor::builder()
                .max_json_ld_items(1)
                .build()
                .extract(html, None),
            Err(Error::ResourceLimit {
                resource: ResourceLimitKind::JsonLdItems,
                ..
            })
        ));

        let cumulative = r#"<script type="application/ld+json"> {"@type":"Article"} </script>
            <script type="application/ld+json"> {"@type":"Article"} </script>
            <main><p>Useful page content.</p></main>"#;
        assert!(matches!(
            Extractor::builder()
                .max_json_ld_bytes(30)
                .build()
                .extract(cumulative, None),
            Err(Error::ResourceLimit {
                resource: ResourceLimitKind::JsonLdBytes,
                ..
            })
        ));

        let deep = format!(
            "<script type='application/ld+json'>{}{{\"@type\":\"Article\"}}{}</script><main><p>Content</p></main>",
            "[".repeat(4),
            "]".repeat(4)
        );
        assert!(matches!(
            Extractor::builder()
                .max_json_ld_depth(2)
                .build()
                .extract(&deep, None),
            Err(Error::ResourceLimit {
                resource: ResourceLimitKind::JsonLdDepth,
                ..
            })
        ));
    }

    #[test]
    fn parser_budget_survives_repair_callbacks_after_poisoning() {
        let error = Extractor::builder()
            .max_nodes(4)
            .build()
            .extract("<table><tr><td><p>content</p></td></tr></table>", None);
        assert!(matches!(error, Err(Error::ResourceLimit { .. })));

        let error = Extractor::builder()
            .max_nodes(1)
            .build()
            .extract("<div></div>", None);
        assert!(matches!(error, Err(Error::ResourceLimit { .. })));
    }

    #[test]
    fn template_contents_obey_element_depth_budget() {
        let error = Extractor::builder().max_depth(1).build().extract(
            "<template><div>content</div></template><main><p>page</p></main>",
            None,
        );
        assert!(matches!(
            error,
            Err(Error::ResourceLimit {
                resource: ResourceLimitKind::ElementDepth,
                ..
            })
        ));
    }

    #[test]
    fn internal_json_ld_depth_cap_is_iterative_and_non_panicking() {
        let depth = 600;
        let deep = format!(
            "<script type='application/ld+json'>{}{{\"@type\":\"Article\"}}{}</script><main><p>Content</p></main>",
            "[".repeat(depth),
            "]".repeat(depth)
        );
        assert!(matches!(
            Extractor::default().extract(&deep, None),
            Err(Error::ResourceLimit {
                resource: ResourceLimitKind::JsonLdDepth,
                ..
            })
        ));
        assert!(matches!(
            Extractor::builder()
                .max_json_ld_depth(1_000)
                .build()
                .extract(&deep, None),
            Err(Error::ResourceLimit {
                resource: ResourceLimitKind::JsonLdDepth,
                ..
            })
        ));
    }

    #[test]
    fn zero_parser_budgets_preserve_default_behavior() {
        let html = "<main><p>Content remains available with unlimited budgets.</p></main>";
        let default = Extractor::default().extract(html, None).unwrap();
        let explicit = Extractor::builder()
            .parse_budget(ParseBudget::default())
            .build()
            .extract(html, None)
            .unwrap();
        assert_eq!(default.markdown(), explicit.markdown());
        assert_eq!(default.metadata().title, explicit.metadata().title);
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
