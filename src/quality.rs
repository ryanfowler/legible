//! Extraction quality metrics and best-attempt comparison.

use crate::diagnostics::{
    SemanticCategoryCoverageInfo, SemanticCoverageCategory, SemanticCoverageInfo,
};
use crate::document::{Document, OperationKind, SemanticItemView as Item};
use crate::dom::{AttrName, Dom, NodeId, NodeStateStore, Tag};
use crate::prepared::{PreparedSource, SourceFlags};
use crate::scoring::{
    get_link_density_cached, get_normalized_inner_text, get_or_compute_stats,
    get_or_compute_stats_excluding,
};
#[cfg(test)]
use crate::scoring::{has_hidden_utility_class_for_discovery, has_static_hidden_marker};
use std::collections::{HashMap, HashSet};

/// Text and structure measured for one DOM region.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ContentMetrics {
    pub(crate) word_count: usize,
    pub(crate) text_chars: usize,
    pub(crate) link_text_chars: usize,
    pub(crate) paragraph_count: usize,
    pub(crate) heading_count: usize,
    pub(crate) list_item_count: usize,
    pub(crate) code_block_count: usize,
    pub(crate) table_count: usize,
    pub(crate) figure_count: usize,
    pub(crate) image_count: usize,
    pub(crate) footnote_reference_count: usize,
    pub(crate) footnote_definition_count: usize,
    pub(crate) math_count: usize,
    pub(crate) structured_block_count: usize,
    pub(crate) link_density: f64,
    has_alphanumeric_text: bool,
    alphabetic_chars: usize,
    digit_chars: usize,
    contextual_structure: bool,
}

impl ContentMetrics {
    /// Measures source content after excluding document-level navigation and
    /// chrome. Semantic headers inside a main/article region remain source
    /// content.
    #[cfg(test)]
    pub(crate) fn measure_source(dom: &Dom, root: NodeId) -> Self {
        Self::measure_source_with_visibility(dom, root, false)
    }

    /// Measures source content while retaining static visibility markers.
    /// ARIA-hidden content and document chrome remain excluded.
    pub(crate) fn measure_source_prepared(
        dom: &Dom,
        source: &PreparedSource,
        root: NodeId,
        relax_static_visibility: bool,
    ) -> Self {
        let Some(range) = source.subtree_range(root) else {
            return Self::default();
        };
        let entries = &source.entries[range];
        let has_primary_region = entries
            .iter()
            .filter(|entry| entry.node != root)
            .any(|entry| entry.is_element() && entry.flags.contains(SourceFlags::PRIMARY_REGION));
        let mut in_primary_region = vec![false; dom.len()];
        for entry in entries
            .iter()
            .filter(|entry| entry.node != root && entry.is_element())
        {
            let parent_is_primary = dom
                .parent(entry.node)
                .is_some_and(|parent| in_primary_region[parent.index()]);
            in_primary_region[entry.node.index()] =
                parent_is_primary || entry.flags.contains(SourceFlags::PRIMARY_REGION);
        }
        let mut excluded = vec![false; dom.len()];
        for entry in entries.iter().filter(|entry| entry.is_element()) {
            let tag = entry.tag;
            let hidden = dom.attr(entry.node, AttrName::AriaHidden) == Some("true")
                || !relax_static_visibility
                    && (entry.flags.contains(SourceFlags::STATIC_HIDDEN)
                        || entry.flags.contains(SourceFlags::UTILITY_HIDDEN))
                || entry.flags.contains(SourceFlags::MODAL_DIALOG)
                || dom.attr(entry.node, AttrName::AriaModal) == Some("true");
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
            let document_chrome = entry.flags.contains(SourceFlags::DOCUMENT_CHROME);
            let contextual_sidebar = tag == Some(Tag::Aside)
                || dom.attr(entry.node, AttrName::Role).is_some_and(|roles| {
                    roles
                        .split_whitespace()
                        .any(|role| role.eq_ignore_ascii_case("complementary"))
                });
            excluded[entry.node.index()] = hard_non_content
                || document_chrome && !in_primary_region[entry.node.index()]
                || contextual_sidebar
                    && has_primary_region
                    && !in_primary_region[entry.node.index()];
        }
        Self::measure_filtered_prepared(dom, root, source, &excluded)
    }

    #[cfg(test)]
    fn measure_source_with_visibility(
        dom: &Dom,
        root: NodeId,
        relax_static_visibility: bool,
    ) -> Self {
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
            let statically_hidden = has_static_hidden_marker(dom, node);
            let utility_hidden = has_hidden_utility_class_for_discovery(dom, node);
            let modal_class = (statically_hidden || utility_hidden)
                && dom.attr(node, AttrName::Class).is_some_and(|classes| {
                    classes.split_whitespace().any(|class| {
                        class.eq_ignore_ascii_case("modal") || class.eq_ignore_ascii_case("dialog")
                    })
                });
            let hidden = dom.attr(node, AttrName::AriaHidden) == Some("true")
                || !relax_static_visibility && (statically_hidden || utility_hidden)
                || dom.attr(node, AttrName::Role).is_some_and(|roles| {
                    roles.split_whitespace().any(|role| {
                        role.eq_ignore_ascii_case("dialog")
                            || role.eq_ignore_ascii_case("alertdialog")
                    })
                })
                || dom.attr(node, AttrName::AriaModal) == Some("true")
                || modal_class;
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

    /// Measures the final semantic result without inspecting source HTML.
    pub(crate) fn measure_document(document: &Document) -> Self {
        let stats = document.stats();
        Self {
            word_count: stats.word_count,
            text_chars: stats.text_length,
            link_text_chars: stats.link_text_length,
            paragraph_count: stats.paragraph_count,
            heading_count: stats.heading_count,
            list_item_count: stats.list_item_count,
            code_block_count: stats.code_block_count,
            table_count: stats.table_count,
            figure_count: stats.figure_count,
            image_count: stats.image_count,
            footnote_reference_count: stats.footnote_reference_count,
            footnote_definition_count: stats.footnote_definition_count,
            math_count: stats.math_count,
            structured_block_count: stats.structured_block_count,
            link_density: stats.link_density,
            has_alphanumeric_text: stats.has_alphanumeric_text,
            alphabetic_chars: stats.alphabetic_chars,
            digit_chars: stats.digit_chars,
            contextual_structure: stats.has_contextual_structure,
        }
    }

    pub(crate) fn measure(dom: &Dom, root: NodeId) -> Self {
        let mut metrics = Self::measure_dom(dom, root);
        let (references, definitions, expressions) =
            crate::document::semantic_normalization_counts(dom, root);
        metrics.footnote_reference_count = references;
        metrics.footnote_definition_count = definitions;
        metrics.math_count = expressions;
        metrics
    }

    /// Measures the structural and text quality signals without semantic
    /// normalization counts. Extraction uses this before it knows whether a
    /// candidate can win. The compiler remains responsible for final semantic
    /// metrics on the selected result.
    pub(crate) fn measure_fast(dom: &Dom, root: NodeId) -> Self {
        let mut metrics = Self::measure_dom(dom, root);
        crate::instrumentation::record_source_full_scan();
        if std::iter::once(root)
            .chain(dom.descendants(root))
            .any(|node| crate::document::semantic_source_evidence(dom, node, None))
        {
            // Preserve enough context to avoid rejecting short semantic
            // content before the compiler can classify it.
            metrics.structured_block_count += 1;
            metrics.contextual_structure = true;
        }
        metrics
    }

    fn measure_dom(dom: &Dom, root: NodeId) -> Self {
        crate::instrumentation::record_source_full_scan();
        let mut store = NodeStateStore::new();
        store.enable_link_lengths();
        let text = get_or_compute_stats(dom, root, &mut store);
        let link_density = get_link_density_cached(dom, root, text.text_length, &mut store);
        let mut metrics = Self::from_text_stats(text, link_density, text.has_alphanumeric());
        for node in std::iter::once(root).chain(dom.descendants(root)) {
            metrics.count_structure(dom.tag(node));
            if dom.tag(node) == Some(Tag::A) {
                metrics.link_text_chars = metrics.link_text_chars.saturating_add(
                    get_or_compute_stats(dom, node, &mut store).text_length as usize,
                );
            }
        }
        metrics
    }

    #[cfg(test)]
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
        let mut metrics = Self::from_text_stats(text, link_density, text.has_alphanumeric());
        let mut included_nodes = Vec::with_capacity(elements.len() + 1);
        if !excluded.get(root.index()).copied().unwrap_or(false) {
            included_nodes.push(root);
        }
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
            included_nodes.push(node);
            metrics.count_structure(dom.tag(node));
            if dom.tag(node) == Some(Tag::A) {
                metrics.link_text_chars = metrics.link_text_chars.saturating_add(
                    get_or_compute_stats_excluding(dom, node, &mut store, excluded).text_length
                        as usize,
                );
            }
        }
        let (references, definitions, expressions) =
            crate::document::semantic_normalization_counts_for_nodes(dom, root, &included_nodes);
        metrics.footnote_reference_count = references;
        metrics.footnote_definition_count = definitions;
        metrics.math_count = expressions;
        metrics
    }

    fn measure_filtered_prepared(
        dom: &Dom,
        root: NodeId,
        source: &PreparedSource,
        excluded: &[bool],
    ) -> Self {
        let mut store = NodeStateStore::new();
        store.enable_link_lengths();
        let text = get_or_compute_stats_excluding(dom, root, &mut store, excluded);
        let link_density = get_link_density_cached(dom, root, text.text_length, &mut store);
        let mut metrics = Self::from_text_stats(text, link_density, text.has_alphanumeric());
        let Some(range) = source.subtree_range(root) else {
            return metrics;
        };
        let entries = &source.entries[range];
        let mut included_nodes = Vec::with_capacity(source.element_count.min(entries.len()) + 1);
        if !excluded.get(root.index()).copied().unwrap_or(false) {
            included_nodes.push(root);
        }
        let mut excluded_depth = None;
        for entry in entries.iter().filter(|entry| entry.is_element()) {
            if entry.node == root {
                continue;
            }
            if let Some(boundary) = excluded_depth {
                if entry.depth > boundary {
                    continue;
                }
                excluded_depth = None;
            }
            if excluded.get(entry.node.index()).copied().unwrap_or(false) {
                excluded_depth = Some(entry.depth);
                continue;
            }
            included_nodes.push(entry.node);
            metrics.count_structure(entry.tag);
            if entry.tag == Some(Tag::A) {
                metrics.link_text_chars = metrics.link_text_chars.saturating_add(
                    get_or_compute_stats_excluding(dom, entry.node, &mut store, excluded)
                        .text_length as usize,
                );
            }
        }
        let (references, definitions, expressions) =
            crate::document::semantic_normalization_counts_for_nodes(dom, root, &included_nodes);
        metrics.footnote_reference_count = references;
        metrics.footnote_definition_count = definitions;
        metrics.math_count = expressions;
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
            link_text_chars: 0,
            link_density,
            has_alphanumeric_text,
            alphabetic_chars: text.alphabetic_chars as usize,
            digit_chars: text.digit_chars as usize,
            ..Self::default()
        }
    }

    fn count_structure(&mut self, tag: Option<Tag>) {
        match tag {
            Some(Tag::P) => self.paragraph_count += 1,
            Some(Tag::H1 | Tag::H2 | Tag::H3 | Tag::H4 | Tag::H5 | Tag::H6) => {
                self.heading_count += 1
            }
            Some(Tag::Li) => self.list_item_count += 1,
            Some(Tag::Pre) => self.code_block_count += 1,
            Some(Tag::Table) => self.table_count += 1,
            Some(Tag::Figure) => self.figure_count += 1,
            Some(Tag::Img) => self.image_count += 1,
            Some(Tag::Math) => self.math_count += 1,
            _ => {}
        }
        if matches!(tag, Some(Tag::Pre | Tag::Th | Tag::Math)) {
            self.contextual_structure = true;
        }
        if matches!(
            tag,
            Some(
                Tag::Pre
                    | Tag::Table
                    | Tag::Figure
                    | Tag::Blockquote
                    | Tag::Details
                    | Tag::Dl
                    | Tag::Math
                    | Tag::Ol
                    | Tag::Ul
            )
        ) {
            self.structured_block_count += 1;
        }
    }

    pub(crate) fn has_meaningful_text(self) -> bool {
        self.has_alphanumeric_text && self.word_count > 0 && self.text_chars > 0
    }
}

/// Useful structures measured from one semantic document in one linear walk.
#[derive(Clone, Debug, Default)]
pub(crate) struct SemanticStructureCounts {
    code_blocks: usize,
    data_tables: usize,
    list_items: HashMap<Box<str>, usize>,
    substantial_list_items: HashMap<Box<str>, usize>,
    visuals: usize,
    headings: usize,
    referenced_footnotes: HashSet<Box<str>>,
    math_expressions: usize,
}

impl SemanticStructureCounts {
    pub(crate) fn measure(document: &Document) -> Self {
        let mut counts = Self::default();
        struct Frame {
            opening: usize,
            kind: OperationKind,
            in_figure: bool,
            list_items: Vec<Box<str>>,
        }

        let mut stack = Vec::<Frame>::new();
        for (index, operation) in document.operations().iter().copied().enumerate() {
            if operation.is_close() {
                let opening = document.operation_opening_index(operation);
                if let Some(frame) = stack.pop() {
                    debug_assert_eq!(frame.opening, opening);
                    if frame.kind == OperationKind::List {
                        for item in &frame.list_items {
                            *counts.list_items.entry(item.clone()).or_default() += 1;
                        }
                        if frame.list_items.len() >= 3 {
                            for item in frame.list_items {
                                *counts.substantial_list_items.entry(item).or_default() += 1;
                            }
                        }
                    }
                }
                continue;
            }

            let kind = operation.kind();
            let in_figure = stack.last().is_some_and(|frame| frame.in_figure);
            if let Some(node) = document.operation_view(index) {
                match node {
                    Item::CodeBlock(_) => counts.code_blocks += 1,
                    Item::Table(_) => counts.data_tables += 1,
                    Item::ListItem
                        if stack
                            .last()
                            .is_some_and(|frame| frame.kind == OperationKind::List) =>
                    {
                        let item = list_item_signature(document, index);
                        if let Some(list) = stack.last_mut() {
                            list.list_items.push(item);
                        }
                    }
                    Item::Figure => counts.visuals += 1,
                    Item::Image(_) if !in_figure => counts.visuals += 1,
                    Item::Heading { .. } => counts.headings += 1,
                    Item::FootnoteReference(id) => {
                        if let Some(definition) = document.footnote_record(id) {
                            counts.referenced_footnotes.insert(definition.label.clone());
                        }
                    }
                    Item::InlineMath(_) | Item::DisplayMath(_) => {
                        counts.math_expressions += 1;
                    }
                    _ => {}
                }
            }

            if kind.is_container() {
                stack.push(Frame {
                    opening: index,
                    kind,
                    in_figure: in_figure || kind == OperationKind::Figure,
                    list_items: Vec::new(),
                });
            }
        }
        counts
    }
}

/// Measures bounded semantic preservation inside one credible source candidate.
/// Categories with weak source evidence do not affect the score.
pub(crate) fn semantic_coverage(
    source: &SemanticStructureCounts,
    result: &SemanticStructureCounts,
) -> Option<SemanticCoverageInfo> {
    let mut categories = Vec::with_capacity(7);
    push_semantic_coverage(
        &mut categories,
        SemanticCoverageCategory::CodeBlocks,
        source.code_blocks,
        result.code_blocks,
        1,
    );
    push_semantic_coverage(
        &mut categories,
        SemanticCoverageCategory::DataTables,
        source.data_tables,
        result.data_tables,
        1,
    );
    push_semantic_coverage(
        &mut categories,
        SemanticCoverageCategory::SubstantialListItems,
        multiset_size(&source.substantial_list_items),
        multiset_overlap(&source.substantial_list_items, &result.list_items),
        1,
    );
    push_semantic_coverage(
        &mut categories,
        SemanticCoverageCategory::Visuals,
        source.visuals,
        result.visuals,
        1,
    );
    push_semantic_coverage(
        &mut categories,
        SemanticCoverageCategory::Headings,
        source.headings,
        result.headings,
        3,
    );
    push_semantic_coverage(
        &mut categories,
        SemanticCoverageCategory::FootnoteDefinitions,
        source.referenced_footnotes.len(),
        source
            .referenced_footnotes
            .intersection(&result.referenced_footnotes)
            .count(),
        1,
    );
    push_semantic_coverage(
        &mut categories,
        SemanticCoverageCategory::MathExpressions,
        source.math_expressions,
        result.math_expressions,
        1,
    );
    if categories.is_empty() {
        return None;
    }
    let score = categories
        .iter()
        .map(|category| category.coverage)
        .sum::<f64>()
        / categories.len() as f64;
    Some(SemanticCoverageInfo { score, categories })
}

fn list_item_signature(document: &Document, root: usize) -> Box<str> {
    let mut parts = Vec::<Box<str>>::new();
    let end = document.operation_end(root).saturating_add(1);
    let mut index = root;
    while index < end {
        let Some(operation) = document.operations().get(index).copied() else {
            break;
        };
        if operation.is_close() {
            index += 1;
            continue;
        }
        let Some(node) = document.operation_view(index) else {
            index += 1;
            continue;
        };
        if index != root && matches!(node, Item::List(_)) {
            index = document.operation_end(index).saturating_add(1);
            continue;
        }
        let text = match node {
            Item::Text(text) | Item::InlineCode(text) => Some(text),
            Item::CodeBlock(code) => Some(code.text()),
            Item::Image(image) => Some(if image.alt().is_empty() {
                image.source()
            } else {
                image.alt()
            }),
            Item::TaskMarker(marker) => marker.fallback_label(),
            Item::InlineMath(math) | Item::DisplayMath(math) => {
                Some(math.fallback_text().unwrap_or_else(|| math.source()))
            }
            Item::Media(media) => Some(media.title().unwrap_or_else(|| media.source())),
            _ => None,
        };
        if let Some(text) = text {
            let text = crate::constants::normalize_whitespace(text);
            if !text.is_empty() {
                parts.push(text.into_boxed_str());
            }
        }
        index += 1;
    }
    parts.join(" ").into_boxed_str()
}

fn multiset_size(values: &HashMap<Box<str>, usize>) -> usize {
    values.values().copied().sum()
}

fn multiset_overlap(source: &HashMap<Box<str>, usize>, result: &HashMap<Box<str>, usize>) -> usize {
    source
        .iter()
        .map(|(item, &count)| count.min(result.get(item).copied().unwrap_or_default()))
        .sum()
}

fn push_semantic_coverage(
    categories: &mut Vec<SemanticCategoryCoverageInfo>,
    category: SemanticCoverageCategory,
    source_count: usize,
    result_count: usize,
    minimum_source_count: usize,
) {
    if source_count < minimum_source_count {
        return;
    }
    categories.push(SemanticCategoryCoverageInfo {
        category,
        source_count,
        result_count,
        coverage: ratio(result_count, source_count),
    });
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

/// Detects a dominant access gate. A match needs structural and textual
/// evidence, except for explicit machine-generated denial text.
pub(crate) fn is_access_barrier(dom: &Dom, root: NodeId) -> bool {
    is_access_barrier_impl(dom, root, None)
}

pub(crate) fn is_access_barrier_prepared(dom: &Dom, source: &PreparedSource, root: NodeId) -> bool {
    is_access_barrier_impl(dom, root, Some(source))
}

fn is_access_barrier_impl(dom: &Dom, root: NodeId, source: Option<&PreparedSource>) -> bool {
    crate::instrumentation::record_source_full_scan();
    let mut buffer = String::new();
    let text = normalize_barrier_text(get_normalized_inner_text(dom, root, &mut buffer));
    if text.is_empty() {
        return false;
    }
    let heading = source
        .map(|source| {
            source
                .entries_in(root)
                .map(|entry| entry.node)
                .find(|&node| {
                    matches!(dom.tag(node), Some(Tag::H1 | Tag::H2 | Tag::H3))
                        && dom.has_non_whitespace_text(node)
                })
        })
        .unwrap_or_else(|| {
            std::iter::once(root)
                .chain(dom.descendants(root))
                .find(|&node| {
                    matches!(dom.tag(node), Some(Tag::H1 | Tag::H2 | Tag::H3))
                        && dom.has_non_whitespace_text(node)
                })
        })
        .map(|node| normalize_barrier_text(get_normalized_inner_text(dom, node, &mut buffer)))
        .unwrap_or_default();
    let strong_denial_heading = matches!(
        heading.trim_matches(
            |character: char| character.is_ascii_punctuation() || character.is_whitespace()
        ),
        "access denied"
            | "request blocked"
            | "verify you are human"
            | "acces refuse"
            | "acces restreint"
            | "requete bloquee"
    );
    let exact_gate_heading = strong_denial_heading
        || matches!(
            heading.trim_matches(
                |character: char| character.is_ascii_punctuation() || character.is_whitespace()
            ),
            "subscription required"
                | "subscribe to unlock"
                | "subscribe to unlock this article"
                | "content locked"
                | "article unavailable"
        );
    let heading_gate = [
        "access denied",
        "access restricted",
        "request blocked",
        "verify you are human",
        "subscription required",
        "subscribe to unlock",
        "content locked",
        "article unavailable",
        "acces refuse",
        "acces restreint",
        "requete bloquee",
        "contenu indisponible",
    ]
    .iter()
    .any(|phrase| heading.starts_with(phrase));
    let structural_gate = source
        .map(|source| {
            source.entries_in(root).any(|entry| {
                [AttrName::Class, AttrName::Id]
                    .into_iter()
                    .filter_map(|name| dom.attr(entry.node, name))
                    .flat_map(|value| {
                        value.split(|character: char| !character.is_ascii_alphanumeric())
                    })
                    .any(|token| {
                        matches!(
                            token.to_ascii_lowercase().as_str(),
                            "paywall" | "barrier" | "gate" | "subscribe"
                        )
                    })
            })
        })
        .unwrap_or_else(|| {
            std::iter::once(root)
                .chain(dom.descendants(root))
                .any(|node| {
                    [AttrName::Class, AttrName::Id]
                        .into_iter()
                        .filter_map(|name| dom.attr(node, name))
                        .flat_map(|value| {
                            value.split(|character: char| !character.is_ascii_alphanumeric())
                        })
                        .any(|token| {
                            matches!(
                                token.to_ascii_lowercase().as_str(),
                                "paywall" | "barrier" | "gate" | "subscribe"
                            )
                        })
                })
        });
    let action = [
        "sign in to continue",
        "log in to continue",
        "subscribe to continue",
        "subscribe to unlock",
        "choose a plan",
        "start your trial",
        "verify you are human",
        "enable cookies",
        "try again later",
        "connectez-vous pour continuer",
        "abonnez-vous pour continuer",
        "verifiez que vous etes humain",
        "obtenir une autorisation",
        "autorisation d'acces",
    ]
    .iter()
    .filter(|phrase| text.contains(**phrase))
    .count();
    let automated = text.contains("automated traffic")
        || text.contains("identified as automated")
        || text.contains("bot detection")
        || text.contains("trafic a ete identifie comme automatise")
        || text.contains("activite de bot")
        || text.contains("trafic automatise");
    let request_identifier = text.contains("request id")
        || text.contains("client ip")
        || text.contains("incident id")
        || text.contains("identifiant de requete")
        || text.contains("adresse ip")
        || text.contains("identifiant d'incident");
    let machine_denial = automated
        && (request_identifier
            || text.contains("access denied")
            || text.contains("acces refuse")
            || text.contains("acces restreint")
            || text.contains("verify you are human"));
    let direct_automation_notice = text.contains("your traffic was identified as automated")
        || text.contains("your traffic has been identified as automated")
        || text.contains("votre trafic a ete identifie comme automatise");
    let explicit_machine_denial = (automated
        && (strong_denial_heading || denial_permission_text(&text))
        && (request_identifier || action > 0))
        || direct_automation_notice && request_identifier && action > 0;
    let denial_support = denial_permission_text(&text);
    let offer = [" per month", "/month", "monthly", "annual", "free trial"]
        .iter()
        .filter(|term| text.contains(**term))
        .count()
        + usize::from(text.contains('$') || text.contains('€') || text.contains('£'));

    machine_denial && strong_denial_heading
        || explicit_machine_denial
        || strong_denial_heading && denial_support
        || exact_gate_heading && action > 0
        || heading_gate && structural_gate
        || structural_gate && action > 0 && offer >= 2
}

/// Evidence for a control-dominated application shell.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct InteractiveShellEvidence {
    controls: usize,
    data_structure: bool,
}

/// Collects source structure used to recognize an application shell.
/// Extraction does not execute the client code that would populate such a page.
pub(crate) fn interactive_shell_evidence(dom: &Dom, root: NodeId) -> InteractiveShellEvidence {
    crate::instrumentation::record_source_full_scan();
    let controls = std::iter::once(root)
        .chain(dom.descendants(root))
        .filter(|&node| {
            matches!(
                dom.tag(node),
                Some(
                    Tag::Button
                        | Tag::Input
                        | Tag::Select
                        | Tag::Textarea
                        | Tag::Form
                        | Tag::Datalist
                )
            )
        })
        .count();
    let data_structure = dom.descendants(root).any(|node| {
        matches!(
            dom.tag(node),
            Some(Tag::Table | Tag::Pre | Tag::Dl | Tag::Ol | Tag::Ul)
        )
    });
    InteractiveShellEvidence {
        controls,
        data_structure,
    }
}

/// Detects a control-dominated application shell with no explanatory content.
/// The text and semantic structure come from the compiled result.
pub(crate) fn is_interactive_shell(
    metrics: ContentMetrics,
    evidence: InteractiveShellEvidence,
) -> bool {
    if metrics.word_count > 20 || metrics.paragraph_count > 0 || metrics.heading_count > 0 {
        return false;
    }
    evidence.controls >= 2 && !evidence.data_structure
}

/// Rejects only very short fragments that contain values but no lexical or
/// structural context.
pub(crate) fn is_incoherent_short_result(metrics: ContentMetrics) -> bool {
    if metrics.text_chars > 200 || metrics.word_count > 20 {
        return false;
    }
    let has_lexical_text = metrics.alphabetic_chars > 0;
    let unlabeled_values = metrics.alphabetic_chars <= 16
        && metrics.digit_chars >= metrics.alphabetic_chars.saturating_mul(2).max(4)
        && metrics.structured_block_count == 0;
    (!has_lexical_text || unlabeled_values) && !metrics.contextual_structure
}

fn denial_permission_text(text: &str) -> bool {
    [
        "permission to access",
        "do not have permission",
        "not authorized",
        "authorization required",
        "forbidden",
        "autorisation d'acces",
        "obtenir une autorisation",
        "acces non autorise",
        "vous n'etes pas autorise",
    ]
    .iter()
    .any(|phrase| text.contains(phrase))
}

fn normalize_barrier_text(text: &str) -> String {
    if text.is_ascii() {
        return text.to_ascii_lowercase();
    }
    text.chars()
        .flat_map(char::to_lowercase)
        .map(|character| match character {
            'à' | 'á' | 'â' | 'ä' | 'ã' | 'å' => 'a',
            'ç' => 'c',
            'è' | 'é' | 'ê' | 'ë' => 'e',
            'ì' | 'í' | 'î' | 'ï' => 'i',
            'ñ' => 'n',
            'ò' | 'ó' | 'ô' | 'ö' | 'õ' => 'o',
            'ù' | 'ú' | 'û' | 'ü' => 'u',
            'ý' | 'ÿ' => 'y',
            other => other,
        })
        .collect()
}

#[cfg(test)]
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
        let prepared = crate::prepared::PreparedSource::build(&source);
        let prepared_actual = prepared.source_metrics;
        let expected = ContentMetrics::measure(&expected, expected_main);

        assert_eq!(actual.word_count, expected.word_count);
        assert_eq!(actual.text_chars, expected.text_chars);
        assert_eq!(actual.paragraph_count, expected.paragraph_count);
        assert_eq!(actual.heading_count, expected.heading_count);
        assert_eq!(
            actual.structured_block_count,
            expected.structured_block_count
        );
        assert_eq!(actual.link_text_chars, expected.link_text_chars);
        assert_eq!(actual.link_density, expected.link_density);
        assert_eq!(actual.has_alphanumeric_text, expected.has_alphanumeric_text);
        assert_eq!(actual.alphabetic_chars, expected.alphabetic_chars);
        assert_eq!(actual.digit_chars, expected.digit_chars);
        assert_eq!(prepared_actual.word_count, actual.word_count);
        assert_eq!(prepared_actual.text_chars, actual.text_chars);
        assert_eq!(prepared_actual.paragraph_count, actual.paragraph_count);
        assert_eq!(prepared_actual.heading_count, actual.heading_count);
        assert_eq!(
            prepared_actual.structured_block_count,
            actual.structured_block_count
        );
        assert_eq!(prepared_actual.link_text_chars, actual.link_text_chars);
        assert_eq!(prepared_actual.link_density, actual.link_density);
        assert_eq!(
            prepared_actual.has_alphanumeric_text,
            actual.has_alphanumeric_text
        );
    }

    #[test]
    fn semantic_result_metrics_match_dom_metrics_for_common_shapes() {
        let dom = Dom::parse_fragment(
            r#"<div><h1>Heading</h1><p>First <a href="/guide">linked words</a>.</p><p>Second paragraph.</p><ul><li>One</li><li>Two</li></ul><pre><code>let x = 1;</code></pre><figure><img src="image.png" alt="Image"><figcaption>Figure caption</figcaption></figure><table><tr><th>Name</th><th>Value</th></tr><tr><td>A</td><td>1</td></tr></table><math><mi>x</mi></math><p>Reference <sup data-legible-footnote-ref="n1">1</sup></p><ol><li data-legible-footnote="n1"><p>Note text.</p></li></ol></div>"#,
            Tag::Div,
        )
        .unwrap();
        let old = ContentMetrics::measure(&dom, dom.root());
        let document = crate::document::compile_document(
            &dom,
            dom.root(),
            &crate::document::CompileContext::default(),
        )
        .unwrap();
        let semantic = ContentMetrics::measure_document(&document);

        // Structural result metrics have the same meaning on both sides of
        // the migration. Text metrics use the canonical semantic rendering.
        assert!(semantic.word_count >= old.word_count);
        assert!(semantic.text_chars >= old.text_chars);
        assert_eq!(semantic.text_chars, document.text().chars().count());
        assert_eq!(old.paragraph_count, semantic.paragraph_count);
        assert_eq!(old.heading_count, semantic.heading_count);
        assert!(old.list_item_count >= semantic.list_item_count);
        assert!(semantic.list_item_count > 0);
        assert_eq!(old.code_block_count, semantic.code_block_count);
        assert_eq!(old.table_count, semantic.table_count);
        assert_eq!(old.figure_count, semantic.figure_count);
        assert_eq!(old.image_count, semantic.image_count);
        assert_eq!(
            old.footnote_reference_count,
            semantic.footnote_reference_count
        );
        assert_eq!(
            old.footnote_definition_count,
            semantic.footnote_definition_count
        );
        assert!(old.math_count >= semantic.math_count);
        assert!(semantic.math_count > 0);
        assert!(old.structured_block_count >= semantic.structured_block_count);
        assert!(semantic.structured_block_count > 0);
        assert_eq!(old.link_text_chars, semantic.link_text_chars);
        assert!((old.link_density - semantic.link_density).abs() < 0.05);
        assert_eq!(old.has_alphanumeric_text, semantic.has_alphanumeric_text);
        assert_eq!(old.alphabetic_chars, semantic.alphabetic_chars);
        assert!(old.digit_chars >= semantic.digit_chars);
        assert_eq!(old.contextual_structure, semantic.contextual_structure);
        assert_eq!(old.has_meaningful_text(), semantic.has_meaningful_text());
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
    fn semantic_coverage_reports_missing_code_and_table_structure() {
        let source = SemanticStructureCounts {
            code_blocks: 4,
            data_tables: 2,
            headings: 2,
            ..SemanticStructureCounts::default()
        };
        let result = SemanticStructureCounts {
            code_blocks: 1,
            data_tables: 0,
            headings: 0,
            ..SemanticStructureCounts::default()
        };

        let coverage = semantic_coverage(&source, &result).unwrap();

        assert_eq!(coverage.categories.len(), 2);
        assert_eq!(coverage.score, 0.125);
        assert_eq!(
            coverage.categories[0],
            SemanticCategoryCoverageInfo {
                category: SemanticCoverageCategory::CodeBlocks,
                source_count: 4,
                result_count: 1,
                coverage: 0.25,
            }
        );
        assert_eq!(
            coverage.categories[1].category,
            SemanticCoverageCategory::DataTables
        );
        assert_eq!(coverage.categories[1].coverage, 0.0);
    }

    #[test]
    fn semantic_coverage_ignores_weak_list_heading_and_footnote_evidence() {
        let source = SemanticStructureCounts {
            headings: 1,
            ..SemanticStructureCounts::default()
        };

        assert!(semantic_coverage(&source, &SemanticStructureCounts::default()).is_none());
    }

    #[test]
    fn semantic_coverage_reports_partial_retention_of_a_substantial_list() {
        let source = SemanticStructureCounts {
            list_items: HashMap::from([
                (Box::<str>::from("a"), 1),
                (Box::<str>::from("b"), 1),
                (Box::<str>::from("c"), 1),
            ]),
            substantial_list_items: HashMap::from([
                (Box::<str>::from("a"), 1),
                (Box::<str>::from("b"), 1),
                (Box::<str>::from("c"), 1),
            ]),
            ..SemanticStructureCounts::default()
        };
        let result = SemanticStructureCounts {
            list_items: HashMap::from([
                (Box::<str>::from("a"), 1),
                (Box::<str>::from("b"), 1),
                (Box::<str>::from("unrelated"), 3),
            ]),
            ..SemanticStructureCounts::default()
        };

        let coverage = semantic_coverage(&source, &result).unwrap();

        assert_eq!(coverage.categories.len(), 1);
        assert_eq!(
            coverage.categories[0].category,
            SemanticCoverageCategory::SubstantialListItems
        );
        assert_eq!(coverage.categories[0].coverage, 2.0 / 3.0);
        assert_eq!(coverage.score, 2.0 / 3.0);

        let unrelated = SemanticStructureCounts {
            list_items: HashMap::from([(Box::<str>::from("unrelated"), 3)]),
            ..SemanticStructureCounts::default()
        };
        assert_eq!(semantic_coverage(&source, &unrelated).unwrap().score, 0.0);
    }

    #[test]
    fn semantic_structure_counts_measure_lists_visuals_and_resolved_footnotes() {
        let dom = Dom::parse_fragment(
            r##"<ul><li>one</li></ul><ul><li>two</li></ul><ul><li>three</li></ul><ol><li>a</li><li>b</li><li>c</li></ol><figure><img src="one.png"><img src="two.png"></figure><figure><figcaption>Diagram</figcaption></figure><img src="standalone.png"><p><sup data-legible-footnote-ref="n1">1</sup><sup data-legible-footnote-ref="n1">1</sup></p><ol><li data-legible-footnote="n1">Used note</li><li data-legible-footnote="n2">Unused note</li></ol>"##,
            Tag::Div,
        )
        .unwrap();
        let document = crate::document::compile_document(
            &dom,
            dom.root(),
            &crate::document::CompileContext::default(),
        )
        .unwrap();

        let counts = SemanticStructureCounts::measure(&document);

        assert_eq!(multiset_size(&counts.substantial_list_items), 3);
        assert_eq!(multiset_size(&counts.list_items), 6);
        assert_eq!(counts.visuals, 3);
        assert_eq!(counts.referenced_footnotes.len(), 1);
        assert!(counts.referenced_footnotes.contains("n1"));
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
    fn classifies_access_gates_without_rejecting_discussion() {
        let denied = Dom::parse_document(
            r#"<body><main class="challenge"><h1>Access denied</h1><p>Automated traffic was detected. Verify you are human.</p><p>Request ID: 123</p></main></body>"#,
        )
        .unwrap();
        assert!(is_access_barrier(&denied, denied.body().unwrap()));

        let wall = Dom::parse_document(
            r#"<body><main class="paywall"><h1>Subscribe to unlock this article</h1><p>Choose a plan and start your trial.</p><p>$9 per month. $90 annual.</p></main></body>"#,
        )
        .unwrap();
        assert!(is_access_barrier(&wall, wall.body().unwrap()));

        let french = Dom::parse_document(
            r#"<html lang="fr"><body><main><h1>Accès restreint</h1><p>Votre trafic a été identifié comme automatisé (bot). Si vous souhaitez obtenir une autorisation d’accès à ce contenu, contactez-nous.</p><p>Adresse IP : 192.0.2.1. Identifiant de requête : abc.</p></main></body></html>"#,
        )
        .unwrap();
        assert!(is_access_barrier(&french, french.body().unwrap()));

        let generic_heading = Dom::parse_document(
            r#"<body><main><h1>Something went wrong</h1><p>Your traffic was identified as automated. Verify you are human.</p><p>Request ID: 123.</p></main></body>"#,
        )
        .unwrap();
        assert!(is_access_barrier(
            &generic_heading,
            generic_heading.body().unwrap()
        ));

        let discussion = Dom::parse_document(
            r#"<body><main class="challenge"><article><h1>How bot detection works</h1><p>This article explains automated traffic and request IDs without blocking the reader.</p><p>A sample plan costs $9 per month.</p></article></main></body>"#,
        )
        .unwrap();
        assert!(!is_access_barrier(&discussion, discussion.body().unwrap()));

        let troubleshooting = Dom::parse_document(
            r#"<body><main class="challenge"><article><h1>Access denied troubleshooting</h1><p>Bot detection systems can classify automated traffic. Support engineers use a request ID to find the relevant diagnostic record.</p><p>This guide explains the policy and its recovery design for application developers.</p></article></main></body>"#,
        )
        .unwrap();
        assert!(!is_access_barrier(
            &troubleshooting,
            troubleshooting.body().unwrap()
        ));

        let recovery_guide = Dom::parse_document(
            r#"<body><article class="barrier"><h1>Human verification recovery</h1><p>Bot detection can associate automated traffic with a request ID. If the prompt says verify you are human, follow the documented recovery procedure.</p><p>The rest of this support article explains diagnosis, accessibility, and account recovery in detail.</p></article></body>"#,
        )
        .unwrap();
        assert!(!is_access_barrier(
            &recovery_guide,
            recovery_guide.body().unwrap()
        ));

        let short_guide = Dom::parse_document(
            r#"<body><article><h1>Access denied troubleshooting</h1><p>If a stale browser session causes this message, enable cookies and reload the application.</p><p>This short guide explains the recovery steps.</p></article></body>"#,
        )
        .unwrap();
        assert!(!is_access_barrier(
            &short_guide,
            short_guide.body().unwrap()
        ));

        let forbidden = Dom::parse_document(
            r#"<body><main><h1>Access denied</h1><p>You do not have permission to access this resource.</p></main></body>"#,
        )
        .unwrap();
        assert!(is_access_barrier(&forbidden, forbidden.body().unwrap()));
    }

    #[test]
    fn short_coherence_uses_lexical_and_structural_context() {
        let ruler = Dom::parse_fragment("<div>11.1×10¹⁹ 2.2×10¹⁹</div>", Tag::Div).unwrap();
        let root = ruler.root();
        assert!(is_incoherent_short_result(ContentMetrics::measure(
            &ruler, root
        )));

        for html in [
            "<p>Status: 200 OK</p>",
            "<table><tr><th>Value</th></tr><tr><td>42</td></tr></table>",
            "<pre><code>42</code></pre>",
            "<math><mn>42</mn></math>",
        ] {
            let dom = Dom::parse_fragment(html, Tag::Div).unwrap();
            let root = dom.root();
            assert!(
                !is_incoherent_short_result(ContentMetrics::measure(&dom, root)),
                "{html}"
            );
        }

        for html in [
            r#"<div class="warning"><p>42</p></div>"#,
            r#"<p><span data-legible-math="inline" data-latex="42">42</span></p>"#,
        ] {
            let dom = Dom::parse_fragment(html, Tag::Div).unwrap();
            let root = dom.root();
            assert!(
                !is_incoherent_short_result(ContentMetrics::measure_fast(&dom, root)),
                "{html}"
            );
        }
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
