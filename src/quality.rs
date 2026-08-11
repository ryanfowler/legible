//! Extraction quality metrics and best-attempt comparison.

use crate::dom::{AttrName, Dom, NodeId, NodeStateStore, Tag};
use crate::scoring::{
    get_link_density_cached, get_or_compute_stats, get_or_compute_stats_excluding,
};

/// Text and structure measured for one DOM region.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ContentMetrics {
    pub(crate) word_count: usize,
    pub(crate) text_chars: usize,
    pub(crate) paragraph_count: usize,
    pub(crate) heading_count: usize,
    pub(crate) structured_block_count: usize,
    pub(crate) link_density: f64,
    has_alphanumeric_text: bool,
}

impl ContentMetrics {
    /// Measures source content after excluding document-level navigation and
    /// chrome. Semantic headers inside a main/article region remain source
    /// content.
    pub(crate) fn measure_source(dom: &Dom, root: NodeId) -> Self {
        let elements = dom.element_descendants_snapshot_with_depth(root);
        let has_primary_region = elements.iter().any(|&(node, _)| {
            matches!(dom.tag(node), Some(Tag::Main | Tag::Article))
                || dom.attr(node, AttrName::Role).is_some_and(is_primary_role)
        });
        let mut in_primary_region = vec![false; dom.len()];
        for &(node, _) in &elements {
            let parent_is_primary = dom
                .parent(node)
                .is_some_and(|parent| in_primary_region[parent.index()]);
            in_primary_region[node.index()] = parent_is_primary
                || matches!(dom.tag(node), Some(Tag::Main | Tag::Article))
                || dom.attr(node, AttrName::Role).is_some_and(is_primary_role);
        }
        let mut excluded = vec![false; dom.len()];
        for &(node, _) in &elements {
            let tag = dom.tag(node);
            let hidden = dom.has_attr(node, AttrName::Hidden)
                || dom.attr(node, AttrName::AriaHidden) == Some("true")
                || dom.attr(node, AttrName::Style).is_some_and(|style| {
                    let compact = style
                        .bytes()
                        .filter(|byte| !byte.is_ascii_whitespace())
                        .map(char::from)
                        .collect::<String>()
                        .to_ascii_lowercase();
                    compact.contains("display:none") || compact.contains("visibility:hidden")
                });
            let hard_non_content = hidden
                || matches!(
                    tag,
                    Some(
                        Tag::Script
                            | Tag::Style
                            | Tag::Template
                            | Tag::Meta
                            | Tag::Link
                            | Tag::Input
                            | Tag::Textarea
                            | Tag::Select
                            | Tag::Button
                            | Tag::Datalist
                            | Tag::Option
                            | Tag::Iframe
                            | Tag::Embed
                            | Tag::Object
                    )
                );
            let role = dom.attr(node, AttrName::Role);
            let document_chrome = matches!(tag, Some(Tag::Header | Tag::Footer | Tag::Nav))
                || role.is_some_and(|roles| {
                    roles.split_whitespace().any(|role| {
                        role.eq_ignore_ascii_case("banner")
                            || role.eq_ignore_ascii_case("navigation")
                    })
                });
            let contextual_sidebar = tag == Some(Tag::Aside)
                || role.is_some_and(|roles| {
                    roles
                        .split_whitespace()
                        .any(|role| role.eq_ignore_ascii_case("complementary"))
                });
            excluded[node.index()] = hard_non_content
                || document_chrome && !in_primary_region[node.index()]
                || contextual_sidebar && has_primary_region && !in_primary_region[node.index()];
        }
        Self::measure_filtered(dom, root, &elements, &excluded)
    }

    pub(crate) fn measure(dom: &Dom, root: NodeId) -> Self {
        let mut store = NodeStateStore::new();
        store.enable_link_lengths();
        let text = get_or_compute_stats(dom, root, &mut store);
        let link_density = get_link_density_cached(dom, root, text.text_length, &mut store);
        let mut metrics = Self::from_text_stats(text, link_density, false);
        for node in std::iter::once(root).chain(dom.descendants(root)) {
            metrics.has_alphanumeric_text |= dom
                .text_node(node)
                .is_some_and(|text| text.chars().any(char::is_alphanumeric));
            metrics.count_structure(dom.tag(node));
        }
        metrics
    }

    fn measure_filtered(
        dom: &Dom,
        root: NodeId,
        elements: &[(NodeId, u32)],
        excluded: &[bool],
    ) -> Self {
        let mut store = NodeStateStore::new();
        store.enable_link_lengths();
        let text = get_or_compute_stats_excluding(dom, root, &mut store, excluded);
        let link_density = get_link_density_cached(dom, root, text.text_length, &mut store);
        let mut inside_excluded = vec![false; dom.len()];
        let has_alphanumeric_text =
            std::iter::once(root)
                .chain(dom.descendants(root))
                .any(|node| {
                    let parent_is_excluded = dom
                        .parent(node)
                        .is_some_and(|parent| inside_excluded[parent.index()]);
                    inside_excluded[node.index()] = excluded[node.index()] || parent_is_excluded;
                    !inside_excluded[node.index()]
                        && dom
                            .text_node(node)
                            .is_some_and(|text| text.chars().any(char::is_alphanumeric))
                });
        let mut metrics = Self::from_text_stats(text, link_density, has_alphanumeric_text);
        let mut excluded_depth = None;
        for &(node, depth) in elements {
            if let Some(boundary) = excluded_depth {
                if depth > boundary {
                    continue;
                }
                excluded_depth = None;
            }
            if excluded[node.index()] {
                excluded_depth = Some(depth);
                continue;
            }
            metrics.count_structure(dom.tag(node));
        }
        metrics
    }

    fn from_text_stats(
        text: crate::dom::NodeStats,
        link_density: f64,
        has_alphanumeric_text: bool,
    ) -> Self {
        Self {
            word_count: text.word_count as usize,
            text_chars: text.text_length as usize,
            link_density,
            has_alphanumeric_text,
            ..Self::default()
        }
    }

    fn count_structure(&mut self, tag: Option<Tag>) {
        match tag {
            Some(Tag::P) => self.paragraph_count += 1,
            Some(Tag::H1 | Tag::H2 | Tag::H3 | Tag::H4 | Tag::H5 | Tag::H6) => {
                self.heading_count += 1
            }
            Some(
                Tag::Pre
                | Tag::Table
                | Tag::Figure
                | Tag::Blockquote
                | Tag::Details
                | Tag::Dl
                | Tag::Math
                | Tag::Ol
                | Tag::Ul,
            ) => self.structured_block_count += 1,
            _ => {}
        }
    }

    pub(crate) fn has_meaningful_text(self) -> bool {
        self.has_alphanumeric_text && self.word_count > 0 && self.text_chars > 0
    }
}

/// Rates an extraction relative to the meaningful source body.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ExtractionQuality {
    pub(crate) word_count: usize,
    pub(crate) text_chars: usize,
    pub(crate) source_word_count: usize,
    pub(crate) source_text_chars: usize,
    pub(crate) coverage: f64,
    pub(crate) paragraph_count: usize,
    pub(crate) heading_count: usize,
    pub(crate) structured_block_count: usize,
    pub(crate) link_density: f64,
    has_alphanumeric_text: bool,
    root_specificity: f64,
}

impl ExtractionQuality {
    pub(crate) fn new(source: ContentMetrics, result: ContentMetrics, specific_root: bool) -> Self {
        let char_coverage = ratio(result.text_chars, source.text_chars);
        Self {
            word_count: result.word_count,
            text_chars: result.text_chars,
            source_word_count: source.word_count,
            source_text_chars: source.text_chars,
            // Character coverage remains stable for languages that do not use
            // spaces between words. Word counts are still useful as absolute
            // quality signals.
            coverage: char_coverage,
            paragraph_count: result.paragraph_count,
            heading_count: result.heading_count,
            structured_block_count: result.structured_block_count,
            link_density: result.link_density,
            has_alphanumeric_text: result.has_alphanumeric_text,
            root_specificity: if specific_root { 1.0 } else { 0.0 },
        }
    }

    /// A short result is good when it retains most of a short source. Longer
    /// results can be good at lower coverage when they retain clear structure.
    pub(crate) fn is_good(self) -> bool {
        if !self.has_alphanumeric_text || self.word_count == 0 || self.text_chars == 0 {
            return false;
        }
        if self.source_word_count <= 60 || self.source_text_chars <= 400 {
            return self.coverage >= 0.45;
        }
        if self.coverage >= 0.6 {
            return true;
        }
        if self.word_count >= 80 && self.coverage >= 0.3 {
            return true;
        }
        if (self.word_count >= 150 || self.text_chars >= 1_000)
            && self.paragraph_count >= 3
            && self.coverage >= 0.05
        {
            return true;
        }
        self.structured_block_count > 0 && self.word_count >= 20 && self.coverage >= 0.25
    }

    pub(crate) fn is_suspiciously_small(self) -> bool {
        !self.has_alphanumeric_text
            || self.word_count == 0
            || self.text_chars == 0
            || self.source_word_count >= 80 && self.word_count < 15
            || self.source_text_chars >= 1_000 && self.coverage < 0.15
    }

    /// Scores attempts without treating the longest result as automatically
    /// best. Specific roots and useful structure offset a moderate loss of
    /// source coverage. Link-heavy results receive only a bounded penalty.
    pub(crate) fn best_attempt_score(self) -> f64 {
        let structure = (self.structured_block_count as f64 * 2.0).min(12.0)
            + (self.paragraph_count as f64 * 0.25).min(4.0)
            + (self.heading_count as f64 * 0.5).min(4.0);
        self.coverage * 100.0 + self.root_specificity * 12.0 + structure
            - (self.link_density * 12.0).min(10.0)
    }
}

fn is_primary_role(roles: &str) -> bool {
    roles
        .split_whitespace()
        .any(|role| role.eq_ignore_ascii_case("main") || role.eq_ignore_ascii_case("article"))
}

fn ratio(value: usize, total: usize) -> f64 {
    if total == 0 {
        f64::from(value == 0)
    } else {
        (value as f64 / total as f64).clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metrics(words: usize, chars: usize, blocks: usize, links: f64) -> ContentMetrics {
        ContentMetrics {
            word_count: words,
            text_chars: chars,
            structured_block_count: blocks,
            link_density: links,
            has_alphanumeric_text: true,
            ..ContentMetrics::default()
        }
    }

    #[test]
    fn source_metrics_filter_nested_hidden_and_chrome_regions() {
        let source = Dom::parse_document(
            r#"<body><header><a href="/home">Home nav words</a></header><main><header><h1>Article title</h1></header><p>First visible paragraph.</p><div hidden><p>Hidden outer <span>nested words.</span></p><div><a href="/hidden">Hidden link words</a></div></div><p>Second <a href="/guide">visible guide</a>.</p><footer><p>Article footer note.</p></footer></main><aside role="complementary"><p>Outside sidebar content.</p></aside><footer><p>Global footer content.</p></footer></body>"#,
        )
        .unwrap();
        let expected = Dom::parse_document(
            r#"<body><main><header><h1>Article title</h1></header><p>First visible paragraph.</p><p>Second <a href="/guide">visible guide</a>.</p><footer><p>Article footer note.</p></footer></main></body>"#,
        )
        .unwrap();
        let source_body = source.body().unwrap();
        let expected_main = expected
            .first_descendant_by_tag(expected.root(), Tag::Main)
            .unwrap();
        let actual = ContentMetrics::measure_source(&source, source_body);
        let expected = ContentMetrics::measure(&expected, expected_main);

        assert_eq!(actual.word_count, expected.word_count);
        assert_eq!(actual.text_chars, expected.text_chars);
        assert_eq!(actual.paragraph_count, expected.paragraph_count);
        assert_eq!(actual.heading_count, expected.heading_count);
        assert_eq!(
            actual.structured_block_count,
            expected.structured_block_count
        );
        assert_eq!(actual.link_density, expected.link_density);
        assert_eq!(actual.has_alphanumeric_text, expected.has_alphanumeric_text);
    }

    #[test]
    fn distinguishes_short_valid_and_large_source_tiny_results() {
        let short =
            ExtractionQuality::new(metrics(35, 180, 0, 0.0), metrics(30, 160, 0, 0.0), true);
        assert!(short.is_good());
        assert!(!short.is_suspiciously_small());

        let tiny = ExtractionQuality::new(
            metrics(4_000, 24_000, 0, 0.0),
            metrics(30, 180, 0, 0.0),
            true,
        );
        assert!(!tiny.is_good());
        assert!(tiny.is_suspiciously_small());
    }

    #[test]
    fn accepts_meaningful_link_and_structured_results() {
        let links =
            ExtractionQuality::new(metrics(100, 700, 1, 0.9), metrics(85, 600, 1, 0.9), true);
        assert!(links.is_good());

        let code =
            ExtractionQuality::new(metrics(100, 700, 2, 0.0), metrics(30, 250, 2, 0.0), true);
        assert!(code.is_good());
    }

    #[test]
    fn punctuation_only_result_is_not_good() {
        let source = metrics(20, 120, 0, 0.0);
        let mut punctuation = metrics(20, 120, 0, 0.0);
        punctuation.has_alphanumeric_text = false;
        let quality = ExtractionQuality::new(source, punctuation, true);
        assert!(!quality.is_good());
        assert!(quality.is_suspiciously_small());
    }

    #[test]
    fn best_attempt_is_not_selected_only_by_length() {
        let focused =
            ExtractionQuality::new(metrics(100, 700, 2, 0.2), metrics(75, 520, 2, 0.0), true);
        let broad_links =
            ExtractionQuality::new(metrics(100, 700, 2, 0.2), metrics(85, 600, 0, 0.95), false);
        assert!(focused.best_attempt_score() > broad_links.best_attempt_score());
    }
}
