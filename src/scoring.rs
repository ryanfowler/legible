//! Content scoring and DOM text helpers.
use crate::candidate::{Candidate, CandidateFeatures, CandidateSet};
use crate::constants::{has_byline, is_div_to_p_elem, is_phrasing_elem, is_unlikely_role, regexps};
use crate::dom::{AttrName, Dom, NodeId, NodeStateStore, NodeStats, Tag};
use smallvec::SmallVec;

/// A Readability-derived score for a possible content root.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ReadabilityScore {
    pub(crate) node: NodeId,
    pub(crate) score: f64,
}
fn is_hash_url(s: &str) -> bool {
    s.starts_with('#') && s.len() > 1
}

const ASCII_ALPHA: u8 = 1 << 0;
const ASCII_DIGIT: u8 = 1 << 1;
const ASCII_WHITESPACE: u8 = 1 << 2;
const ASCII_COMMA: u8 = 1 << 3;
const ASCII_SENTENCE_END: u8 = 1 << 4;
const ASCII_DOT: u8 = 1 << 5;

const fn ascii_classes() -> [u8; 256] {
    let mut classes = [0; 256];
    let mut value = 0;
    while value < classes.len() {
        let byte = value as u8;
        let mut class = 0;
        if (byte >= b'a' && byte <= b'z') || (byte >= b'A' && byte <= b'Z') {
            class |= ASCII_ALPHA;
        }
        if byte >= b'0' && byte <= b'9' {
            class |= ASCII_DIGIT;
        }
        if byte == b' ' || (byte >= b'\t' && byte <= b'\r') {
            class |= ASCII_WHITESPACE;
        }
        if byte == b',' {
            class |= ASCII_COMMA;
        }
        if byte == b'.' || byte == b'!' || byte == b'?' {
            class |= ASCII_SENTENCE_END;
        }
        if byte == b'.' {
            class |= ASCII_DOT;
        }
        classes[value] = class;
        value += 1;
    }
    classes
}

const ASCII_CLASSES: [u8; 256] = ascii_classes();

fn stats_for_text(text: &str) -> NodeStats {
    let mut s = NodeStats::default();
    s.set_has_text(!text.is_empty());
    let mut prev = true;
    let mut dot = false;
    let mut text_length = 0usize;
    let mut word_count = 0usize;
    let mut comma_count = 0usize;
    let mut sentence_end_count = 0usize;
    let mut has_non_whitespace = false;
    let mut has_alphanumeric = false;
    let mut has_sentence_break = false;

    // Most article text is ASCII. Scan bytes to avoid UTF-8 decoding and
    // Unicode whitespace tables in the hot loop.
    if text.is_ascii() {
        let bytes = text.as_bytes();
        s.set_starts_with_whitespace(bytes.first().is_some_and(u8::is_ascii_whitespace));
        s.set_ends_with_whitespace(bytes.last().is_some_and(u8::is_ascii_whitespace));
        let mut alphabetic_chars = 0_usize;
        let mut digit_chars = 0_usize;
        for &byte in bytes {
            let class = ASCII_CLASSES[usize::from(byte)];
            alphabetic_chars += usize::from(class & ASCII_ALPHA != 0);
            digit_chars += usize::from(class & ASCII_DIGIT != 0);
            comma_count += usize::from(class & ASCII_COMMA != 0);
            sentence_end_count += usize::from(class & ASCII_SENTENCE_END != 0);
            if class & ASCII_WHITESPACE != 0 {
                has_sentence_break |= dot;
                dot = false;
                if !prev {
                    text_length += 1;
                    prev = true
                }
            } else {
                has_non_whitespace = true;
                word_count += usize::from(prev);
                dot = class & ASCII_DOT != 0;
                text_length += 1;
                prev = false
            }
        }
        s.alphabetic_chars = alphabetic_chars.min(u32::MAX as usize) as u32;
        s.digit_chars = digit_chars.min(u32::MAX as usize) as u32;
        has_alphanumeric = alphabetic_chars != 0 || digit_chars != 0;
    } else {
        s.set_starts_with_whitespace(text.starts_with(char::is_whitespace));
        s.set_ends_with_whitespace(text.ends_with(char::is_whitespace));
        for c in text.chars() {
            if c.is_whitespace() {
                has_sentence_break |= dot;
                dot = false;
                if !prev {
                    text_length += 1;
                    prev = true
                }
            } else {
                has_non_whitespace = true;
                if !has_alphanumeric {
                    has_alphanumeric = c.is_alphanumeric();
                }
                s.alphabetic_chars = s
                    .alphabetic_chars
                    .saturating_add(u32::from(c.is_alphabetic()));
                s.digit_chars = s.digit_chars.saturating_add(u32::from(c.is_numeric()));
                word_count += usize::from(prev);
                dot = c == '.';
                comma_count += usize::from(
                    c == ','
                        || matches!(
                            c,
                            '\u{060C}'
                                | '\u{FE50}'
                                | '\u{FE10}'
                                | '\u{FE11}'
                                | '\u{2E41}'
                                | '\u{2E34}'
                                | '\u{2E32}'
                                | '\u{FF0C}'
                        ),
                );
                sentence_end_count += usize::from(matches!(
                    c,
                    '.' | '!' | '?' | '\u{3002}' | '\u{FF01}' | '\u{FF1F}'
                ));
                text_length += 1;
                prev = false
            }
        }
    }
    if prev && text_length > 0 {
        text_length -= 1
    }
    s.text_length = text_length.min(u32::MAX as usize) as u32;
    s.word_count = word_count.min(u32::MAX as usize) as u32;
    s.comma_count = comma_count.min(u32::MAX as usize) as u32;
    s.sentence_end_count = sentence_end_count.min(u32::MAX as usize) as u32;
    s.set_has_non_whitespace(has_non_whitespace);
    s.set_has_alphanumeric(has_alphanumeric);
    s.set_has_sentence_break(has_sentence_break);
    s.set_ends_with_dot(dot);
    s.set_has_sentence_end(has_sentence_break || dot);
    s
}
fn append_stats(a: &mut NodeStats, b: &NodeStats) {
    if !b.has_text() {
        return;
    }
    if !a.has_text() {
        *a = *b;
        return;
    }

    let a_non_whitespace = a.has_non_whitespace();
    let b_non_whitespace = b.has_non_whitespace();
    let a_ends_with_whitespace = a.ends_with_whitespace();
    let b_starts_with_whitespace = b.starts_with_whitespace();
    let sentence_break = a.has_sentence_break()
        || b.has_sentence_break()
        || a.ends_with_dot() && b_starts_with_whitespace;

    if a_non_whitespace && b_non_whitespace && (a_ends_with_whitespace || b_starts_with_whitespace)
    {
        a.text_length = a.text_length.saturating_add(1);
    }
    a.text_length = a.text_length.saturating_add(b.text_length);
    a.word_count = a.word_count.saturating_add(b.word_count);
    if a_non_whitespace && b_non_whitespace && !a_ends_with_whitespace && !b_starts_with_whitespace
    {
        a.word_count = a.word_count.saturating_sub(1);
    }
    a.comma_count = a.comma_count.saturating_add(b.comma_count);
    a.sentence_end_count = a.sentence_end_count.saturating_add(b.sentence_end_count);
    a.alphabetic_chars = a.alphabetic_chars.saturating_add(b.alphabetic_chars);
    a.digit_chars = a.digit_chars.saturating_add(b.digit_chars);
    a.set_has_sentence_break(sentence_break);
    a.set_has_non_whitespace(a_non_whitespace || b_non_whitespace);
    a.set_has_alphanumeric(a.has_alphanumeric() || b.has_alphanumeric());
    a.set_ends_with_whitespace(b.ends_with_whitespace());
    a.set_ends_with_dot(b.ends_with_dot());
    a.set_has_sentence_end(sentence_break || b.ends_with_dot());
}
pub fn get_or_compute_stats(dom: &Dom, id: NodeId, store: &mut NodeStateStore) -> NodeStats {
    get_or_compute_stats_excluding(dom, id, store, &[])
}

/// Computes text statistics while omitting the roots marked in `excluded`.
///
/// The cache must be empty because its entries describe this filtered tree view.
pub(crate) fn get_or_compute_stats_excluding(
    dom: &Dom,
    id: NodeId,
    store: &mut NodeStateStore,
    excluded: &[bool],
) -> NodeStats {
    if let Some(s) = store.get_stats(id) {
        return *s;
    }

    struct StatsFrame {
        node: NodeId,
        next_child: Option<NodeId>,
        stats: NodeStats,
        link_length: f64,
    }

    impl StatsFrame {
        fn new(dom: &Dom, node: NodeId) -> Self {
            Self {
                node,
                next_child: dom.first_child(node),
                stats: dom
                    .text_node(node)
                    .map_or_else(NodeStats::default, stats_for_text),
                link_length: 0.0,
            }
        }
    }

    let cache_links = store.link_lengths_enabled();
    let mut stack = SmallVec::<[StatsFrame; 16]>::new();
    stack.push(StatsFrame::new(dom, id));
    while let Some(frame) = stack.last_mut() {
        if let Some(child) = frame.next_child {
            frame.next_child = dom.next_sibling(child);
            if excluded.get(child.index()).copied().unwrap_or(false) {
                continue;
            }
            if let Some(child_stats) = store.get_stats(child) {
                append_stats(&mut frame.stats, child_stats);
                if cache_links {
                    frame.link_length += store.link_length(child);
                }
            } else {
                stack.push(StatsFrame::new(dom, child));
            }
            continue;
        }

        let mut completed = stack.pop().expect("stats frame exists");
        completed.stats.set_has_sentence_end(
            completed.stats.has_sentence_break() || completed.stats.ends_with_dot(),
        );
        if cache_links {
            if dom.tag(completed.node) == Some(Tag::A) {
                completed.link_length = completed.stats.text_length as f64
                    * if dom
                        .attr(completed.node, AttrName::Href)
                        .is_some_and(is_hash_url)
                    {
                        0.3
                    } else {
                        1.0
                    };
            }
            store.set_link_length(completed.node, completed.link_length);
        }
        store.set_stats(completed.node, completed.stats);
        if let Some(parent) = stack.last_mut() {
            append_stats(&mut parent.stats, &completed.stats);
            if cache_links {
                parent.link_length += completed.link_length;
            }
        }
    }
    store.get_stats(id).copied().unwrap_or_default()
}
/// Structural counts for candidate subtrees.
///
/// Counts are accumulated on the candidate containment tree rather than on
/// every source node. This keeps the feature index proportional to the number
/// of candidates while preserving additive subtree totals.
#[derive(Clone)]
pub(crate) struct CandidateFeatureIndex {
    counts: Vec<StructuralCounts>,
    candidate_nodes: Vec<NodeId>,
    has_links: bool,
}

/// Counts used by structural boundary selection stay exact. The remaining
/// counters are capped because ranking only uses small thresholds or bounded
/// contributions.
#[derive(Clone, Copy, Debug, Default)]
struct StructuralCounts {
    paragraphs: u32,
    headings: u32,
    list_items: u8,
    code_blocks: u32,
    tables: u32,
    figures: u32,
    images: u8,
    protected_blocks: u8,
}

impl StructuralCounts {
    fn add(&mut self, other: Self) {
        self.paragraphs = self.paragraphs.saturating_add(other.paragraphs);
        self.headings = self.headings.saturating_add(other.headings);
        // Eight list items reach the ranking bonus cap.
        self.list_items = self.list_items.saturating_add(other.list_items).min(8);
        self.code_blocks = self.code_blocks.saturating_add(other.code_blocks);
        self.tables = self.tables.saturating_add(other.tables);
        self.figures = self.figures.saturating_add(other.figures);
        self.images = self.images.saturating_add(other.images);
        self.protected_blocks = self.protected_blocks.saturating_add(other.protected_blocks);
    }
}

impl CandidateFeatureIndex {
    pub(crate) fn new(
        dom: &Dom,
        store: &NodeStateStore,
        nodes: &[(NodeId, u32)],
        candidates: &CandidateSet,
    ) -> Self {
        let candidate_nodes: Vec<_> = candidates.iter().map(|candidate| candidate.node).collect();
        let mut counts = vec![StructuralCounts::default(); candidate_nodes.len()];
        let mut candidate_parent = vec![None; candidate_nodes.len()];
        let mut active_candidates = Vec::<(u32, usize)>::new();
        let mut candidate_order = Vec::with_capacity(candidate_nodes.len());
        let mut has_links = false;

        for &(node, depth) in nodes {
            let Some(tag) = dom.tag(node) else { continue };
            if tag == Tag::A {
                has_links = true;
            }

            while active_candidates
                .last()
                .is_some_and(|&(active_depth, _)| active_depth >= depth)
            {
                active_candidates.pop();
            }

            if let Some(candidate_index) = candidates.index_of(node) {
                candidate_parent[candidate_index] =
                    active_candidates.last().map(|&(_, parent)| parent);
                active_candidates.push((depth, candidate_index));
                candidate_order.push(candidate_index);
            }

            let mut own = StructuralCounts::default();
            match tag {
                Tag::P => own.paragraphs = 1,
                Tag::H1 | Tag::H2 | Tag::H3 | Tag::H4 | Tag::H5 | Tag::H6 => own.headings = 1,
                Tag::Li => own.list_items = 1,
                Tag::Pre => own.code_blocks = 1,
                Tag::Table if store.is_data_table(node) == Some(true) => own.tables = 1,
                Tag::Figure => own.figures = 1,
                Tag::Img => own.images = 1,
                Tag::Blockquote | Tag::Details | Tag::Dl | Tag::Math | Tag::Picture => {
                    own.protected_blocks = 1
                }
                _ => {}
            }
            if let Some(&(_, candidate_index)) = active_candidates.last() {
                counts[candidate_index].add(own);
            }
        }

        // The snapshot is in source preorder, so reversing the candidates
        // propagates every nested candidate's total to its candidate parent
        // without revisiting the non-candidate source nodes.
        for &candidate_index in candidate_order.iter().rev() {
            if let Some(parent) = candidate_parent[candidate_index] {
                let child = counts[candidate_index];
                counts[parent].add(child);
            }
        }
        Self {
            counts,
            candidate_nodes,
            has_links,
        }
    }

    pub(crate) fn matches_candidates(&self, candidates: &CandidateSet) -> bool {
        self.candidate_nodes.len() == candidates.iter().count()
            && self
                .candidate_nodes
                .iter()
                .zip(candidates.iter())
                .all(|(&node, candidate)| node == candidate.node)
    }

    pub(crate) fn prepare_text_cache(&self, store: &mut NodeStateStore) {
        if self.has_links && !store.link_lengths_enabled() {
            store.enable_link_lengths();
            store.clear_stats();
        }
    }

    pub(crate) fn features(
        &self,
        dom: &Dom,
        candidate_index: usize,
        candidate: Candidate,
        store: &mut NodeStateStore,
        weight_classes: bool,
    ) -> CandidateFeatures {
        let text = get_or_compute_stats(dom, candidate.node, store);
        let counts = self
            .counts
            .get(candidate_index)
            .copied()
            .unwrap_or_default();
        let link_text_chars = if store.link_lengths_enabled() {
            store.link_length(candidate.node)
        } else {
            0.0
        };
        let link_density = if text.text_length == 0 {
            0.0
        } else {
            (link_text_chars / f64::from(text.text_length)).clamp(0.0, 1.0)
        };
        let (positive_name_score, negative_name_score) = if weight_classes {
            name_signals(dom, candidate.node)
        } else {
            (0.0, 0.0)
        };

        CandidateFeatures {
            text_chars: text.text_length,
            word_count: text.word_count,
            paragraph_count: counts.paragraphs,
            heading_count: counts.headings,
            list_item_count: u32::from(counts.list_items),
            code_block_count: counts.code_blocks,
            table_count: counts.tables,
            figure_count: counts.figures,
            image_count: u32::from(counts.images),
            link_text_chars,
            link_density,
            sentence_end_count: text.sentence_end_count,
            comma_count: text.comma_count,
            protected_block_count: u32::from(counts.protected_blocks),
            readability_score: candidate.readability_score,
            semantic_prior: candidate.semantic_prior,
            positive_name_score,
            negative_name_score,
        }
    }
}

impl CandidateFeatures {
    /// Combines prose, semantic, and structured-content evidence.
    /// Link density is a bounded penalty because useful indexes can be links.
    pub(crate) fn ranking_score(self) -> f64 {
        // Readability already represents prose well. Keep duplicate prose
        // evidence small so it only breaks close ties.
        let text_evidence = (f64::from(self.word_count) / 100_000.0).min(0.001)
            + (f64::from(self.text_chars) / 1_000_000.0).min(0.001);
        let prose_evidence = (f64::from(self.paragraph_count) / 100_000.0).min(0.001)
            + (f64::from(self.sentence_end_count) / 100_000.0).min(0.001)
            + (f64::from(self.comma_count) / 100_000.0).min(0.001);

        // Give large bonuses only when structure is the main content signal.
        // This avoids promoting a broad article wrapper because it contains an
        // incidental table, list, or media block.
        let sparse_prose = self.paragraph_count <= 2;
        let list_evidence =
            if self.list_item_count >= 3 && self.paragraph_count <= 1 && self.link_density >= 0.5 {
                (f64::from(self.list_item_count) * 1.5).min(12.0)
            } else {
                0.0
            };
        let structure_evidence = list_evidence
            + if sparse_prose {
                (f64::from(self.code_block_count) * 4.0).min(12.0)
                    + (f64::from(self.table_count) * 4.0).min(12.0)
            } else {
                0.0
            }
            + (f64::from(self.heading_count) / 100_000.0).min(0.001)
            + (f64::from(self.figure_count) / 100_000.0).min(0.001)
            + (f64::from(self.image_count) / 100_000.0).min(0.001)
            + (f64::from(self.protected_block_count) / 100_000.0).min(0.001);
        let link_penalty = (self.link_density * 0.2).min(0.1);
        let link_volume_evidence = (self.link_text_chars / 1_000_000.0).min(0.001);
        let negative_name_penalty = if self.positive_name_score > 0.0 {
            // Conflicting names are uncertain. Let content evidence decide.
            self.negative_name_score * 0.1
        } else {
            // Preserve a substantial candidate even when its name looks
            // suspicious. The penalty scales with, but cannot erase, its
            // strongest content signal.
            self.negative_name_score * (1.0 + self.readability_score.max(0.0) * 0.6).min(125.0)
        };
        let name_evidence = self.positive_name_score * 0.001 - negative_name_penalty;

        self.readability_score
            + self.semantic_prior
            + text_evidence
            + prose_evidence
            + structure_evidence
            + link_volume_evidence
            + name_evidence
            - link_penalty
    }
}

fn name_signals(dom: &Dom, node: NodeId) -> (f64, f64) {
    fn signals_for_node(dom: &Dom, node: NodeId) -> (bool, bool) {
        let mut positive = false;
        let mut negative = false;
        for name in [AttrName::Class, AttrName::Id] {
            let Some(value) = dom.attr(node, name).filter(|value| !value.is_empty()) else {
                continue;
            };
            let matches = regexps::CLASS_WEIGHT_SET.matches(value);
            positive |= matches.matched(1);
            negative |= matches.matched(0) || regexps::UNLIKELY_CANDIDATES.is_match(value);
        }
        let role = dom.attr(node, AttrName::Role);
        positive |= role.is_some_and(|roles| {
            roles.split_whitespace().any(|role| {
                role.eq_ignore_ascii_case("article") || role.eq_ignore_ascii_case("main")
            })
        });
        negative |= role.is_some_and(is_unlikely_role);
        (positive, negative)
    }

    let (positive, mut negative) = signals_for_node(dom, node);
    for ancestor in dom.ancestors(node).take(3) {
        let (ancestor_positive, ancestor_negative) = signals_for_node(dom, ancestor);
        negative |= ancestor_negative && !ancestor_positive;
    }
    (f64::from(positive), f64::from(negative))
}

pub fn compute_initial_readability_data(dom: &Dom, id: NodeId, weight_classes: bool) -> f64 {
    let score = match dom.tag(id) {
        Some(Tag::Div) => 5.,
        Some(Tag::Pre | Tag::Td | Tag::Blockquote) => 3.,
        Some(
            Tag::Address | Tag::Ol | Tag::Ul | Tag::Dl | Tag::Dd | Tag::Dt | Tag::Li | Tag::Form,
        ) => -3.,
        Some(Tag::H1 | Tag::H2 | Tag::H3 | Tag::H4 | Tag::H5 | Tag::H6 | Tag::Th) => -5.,
        _ => 0.,
    };
    score + get_class_weight(dom, id, weight_classes) as f64
}
pub fn initialize_node(dom: &Dom, id: NodeId, store: &mut NodeStateStore, weight_classes: bool) {
    store.initialize_if_absent(
        id,
        compute_initial_readability_data(dom, id, weight_classes),
    );
}

/// Prepares a scoring-only DOM and returns paragraphs created by the pass.
///
/// The caller must pass a copy of the source DOM. This function can wrap
/// phrasing content, replace simple wrappers, and rename leaf `div` elements.
pub(crate) fn prepare_readability_structure(
    dom: &mut Dom,
    divs: &[NodeId],
    candidates: &CandidateSet,
) -> SmallVec<[NodeId; 256]> {
    // Most prepared divs create at most one scoring paragraph. The parsed DOM
    // often ends at its vector capacity, so reserve before the first wrapper.
    // Exact growth avoids doubling the entire scoring arena for this temporary
    // set of nodes.
    dom.reserve_additional_nodes_exact(divs.len());
    let mut to_score = SmallVec::new();
    for &id in divs {
        if dom.parent(id).is_none() || dom.tag(id) != Some(Tag::Div) {
            continue;
        }
        wrap_phrasing_content_in_p(dom, id);
        if candidates.is_semantic(id) {
            to_score.extend(
                dom.element_children(id)
                    .filter(|&child| dom.tag(child) == Some(Tag::P)),
            );
        } else if has_single_tag_inside_element(dom, id, Tag::P) && get_link_density(dom, id) < 0.25
        {
            if let Some(paragraph) = dom.element_children(id).next() {
                dom.replace_with(id, paragraph);
                to_score.push(paragraph)
            }
        } else if !has_child_block_element(dom, id) {
            dom.rename_html(id, Tag::P);
            to_score.push(id)
        } else {
            to_score.extend(
                dom.element_children(id)
                    .filter(|&child| dom.tag(child) == Some(Tag::P)),
            );
        }
    }
    to_score
}

/// Computes Readability paragraph-propagation scores without selecting a root.
///
/// `dom` contains the scoring-only structural preparation. This function first
/// propagates scores. It then detaches deferred clutter and calculates link
/// density. This order avoids a second full DOM copy.
pub(crate) fn compute_readability_scores(
    dom: &mut Dom,
    to_score: impl IntoIterator<Item = NodeId>,
    excluded: &[NodeId],
    excluded_mask: &[bool],
    store: &mut NodeStateStore,
    weight_classes: bool,
) -> SmallVec<[ReadabilityScore; 64]> {
    let mut discovered = SmallVec::<[NodeId; 256]>::new();
    for node in to_score {
        let Some(parent) = dom.parent(node).filter(|&parent| dom.is_element(parent)) else {
            continue;
        };
        let stats = get_or_compute_stats(dom, node, store);
        if stats.text_length < 25 {
            continue;
        }
        let content_score =
            2.0 + f64::from(stats.comma_count) + f64::from((stats.text_length / 100).min(3));
        let mut ancestor = Some(parent);
        for level in 0..5 {
            let Some(node) = ancestor else { break };
            ancestor = dom.parent(node);
            if !dom.is_element(node) || !ancestor.is_some_and(|parent| dom.is_element(parent)) {
                continue;
            }
            let initial = compute_initial_readability_data(dom, node, weight_classes);
            if store.initialize_if_absent(node, initial) {
                discovered.push(node)
            }
            let divisor = match level {
                0 => 1.0,
                1 => 2.0,
                _ => (level * 3) as f64,
            };
            store.add_content_score(node, content_score / divisor);
        }
    }

    // Detach excluded nodes and invalidate cached stats for their ancestors.
    // Leaf-level paragraph text is unchanged, so their cached stats remain valid.
    // This avoids a full text re-scan in the second pass.
    let mut ancestors_to_invalidate = SmallVec::<[NodeId; 64]>::new();
    for &node in excluded {
        if dom.parent(node).is_some() {
            // Collect ancestors before detaching
            for ancestor in dom.ancestors(node) {
                ancestors_to_invalidate.push(ancestor);
            }
            dom.detach(node);
        }
    }
    for &node in &ancestors_to_invalidate {
        store.invalidate_stats(node);
        if store.link_lengths_enabled() {
            store.set_link_length(node, 0.0);
        }
    }
    let mut scores = SmallVec::new();
    for node in discovered {
        if excluded_mask.get(node.index()).copied().unwrap_or(false) {
            continue;
        }
        let content_score = store.get_content_score(node);
        let length = get_or_compute_stats(dom, node, store).text_length;
        let density = get_link_density_cached(dom, node, length, store);
        scores.push(ReadabilityScore {
            node,
            score: content_score * (1.0 - density),
        });
    }
    scores
}

/// Marks excluded roots and their descendants before the scoring tree is mutated.
///
/// A boolean index avoids repeatedly walking candidate ancestor chains. This is
/// important for malformed documents, where HTML tree repair can create deep
/// nesting and many candidates.
pub(crate) fn build_exclusion_mask(dom: &Dom, excluded: &[NodeId]) -> Vec<bool> {
    if excluded.is_empty() {
        return Vec::new();
    }
    let mut mask = vec![false; dom.len()];
    for &root in excluded {
        if root.index() >= mask.len() {
            continue;
        }
        mask[root.index()] = true;
        for node in dom.descendants(root) {
            mask[node.index()] = true;
        }
    }
    mask
}
pub fn get_class_weight(dom: &Dom, id: NodeId, weight_classes: bool) -> i32 {
    if !weight_classes {
        return 0;
    }
    let mut w = 0;
    for a in dom.attrs(id) {
        if !matches!(a.name.local.as_ref(), "class" | "id") || a.value.is_empty() {
            continue;
        }
        let m = regexps::CLASS_WEIGHT_SET.matches(a.value.as_ref());
        if m.matched(0) {
            w -= 25
        }
        if m.matched(1) {
            w += 25
        }
    }
    w
}
pub fn has_non_empty_inner_text(dom: &Dom, id: NodeId) -> bool {
    dom.has_non_whitespace_text(id)
}
pub fn get_inner_text<'a>(dom: &Dom, id: NodeId, out: &'a mut String) -> &'a str {
    out.clear();
    dom.append_text(id, out);
    out.trim()
}

pub fn get_inner_text_limited<'a>(
    dom: &Dom,
    id: NodeId,
    out: &'a mut String,
    limit: usize,
) -> &'a str {
    out.clear();
    if limit == usize::MAX {
        dom.append_text(id, out);
    } else {
        dom.append_text_limited(id, out, limit);
    }
    out.trim()
}
pub fn get_normalized_inner_text<'a>(dom: &Dom, id: NodeId, out: &'a mut String) -> &'a str {
    out.clear();
    dom.append_normalized_text(id, out);
    out
}
pub fn get_inner_text_owned(dom: &Dom, id: NodeId) -> String {
    let mut out = String::new();
    dom.append_text(id, &mut out);
    let start = out.len() - out.trim_start().len();
    let end = out.trim_end().len();
    if end == 0 {
        out.clear();
    } else {
        out.truncate(end);
        if start != 0 {
            out.drain(..start);
        }
    }
    out
}

pub fn get_inner_text_owned_limited(dom: &Dom, id: NodeId, limit: usize) -> String {
    let mut out = String::new();
    if limit == usize::MAX {
        dom.append_text(id, &mut out);
    } else {
        dom.append_text_limited(id, &mut out, limit);
    }
    let start = out.len() - out.trim_start().len();
    let end = out.trim_end().len();
    if end == 0 {
        out.clear();
    } else {
        out.truncate(end);
        if start != 0 {
            out.drain(..start);
        }
    }
    out
}
pub fn get_link_density_with_text(
    dom: &Dom,
    id: NodeId,
    text: Option<&str>,
    mut store: Option<&mut NodeStateStore>,
) -> f64 {
    let total = text.map_or_else(|| dom.normalized_char_count(id), |x| x.chars().count());
    if total == 0 {
        return 0.;
    }
    let mut links = 0.;
    for x in dom.descendants(id) {
        if dom.tag(x) != Some(Tag::A) {
            continue;
        }
        let len = if let Some(st) = store.as_deref_mut() {
            get_or_compute_stats(dom, x, st).text_length as usize
        } else {
            dom.normalized_char_count(x)
        };
        links += len as f64
            * if dom.attr(x, AttrName::Href).is_some_and(is_hash_url) {
                0.3
            } else {
                1.
            }
    }
    links / total as f64
}
pub fn get_link_density(dom: &Dom, id: NodeId) -> f64 {
    get_link_density_with_text(dom, id, None, None)
}
pub fn get_link_density_cached(dom: &Dom, id: NodeId, len: u32, store: &mut NodeStateStore) -> f64 {
    if len == 0 {
        return 0.;
    }
    get_or_compute_stats(dom, id, store);
    if dom.tag(id) == Some(Tag::A) {
        // Link density excludes the root itself. This case is not part of the
        // normal candidate path, but preserve the helper's original behavior.
        let links = dom
            .children(id)
            .map(|child| store.link_length(child))
            .sum::<f64>();
        links / len as f64
    } else {
        store.link_length(id) / len as f64
    }
}
pub fn is_whitespace(dom: &Dom, id: NodeId) -> bool {
    dom.text_node(id).is_some_and(|t| t.trim().is_empty()) || dom.tag(id) == Some(Tag::Br)
}
pub fn is_phrasing_content(dom: &Dom, id: NodeId) -> bool {
    fn go(d: &Dom, n: NodeId, depth: u32) -> bool {
        if d.is_text(n) || d.is_comment(n) {
            return true;
        }
        let Some(t) = d.tag(n) else { return false };
        if is_phrasing_elem(t) {
            return true;
        }
        matches!(t, Tag::A | Tag::Del | Tag::Ins)
            && depth < 10
            && d.children(n).all(|c| go(d, c, depth + 1))
    }
    go(dom, id, 0)
}
pub fn wrap_phrasing_content_in_p(dom: &mut Dom, div: NodeId) {
    let children: SmallVec<[NodeId; 8]> = dom.children(div).collect();
    let mut i = 0;
    while i < children.len() {
        if !is_phrasing_content(dom, children[i]) {
            i += 1;
            continue;
        }
        let mut j = i;
        let mut content = false;
        while j < children.len() && is_phrasing_content(dom, children[j]) {
            content |= dom.is_element(children[j])
                || dom
                    .text_node(children[j])
                    .is_some_and(|t| !t.trim().is_empty());
            j += 1
        }
        if content {
            let mut a = i;
            let mut b = j;
            while a < b && is_whitespace(dom, children[a]) {
                a += 1
            }
            while a < b && is_whitespace(dom, children[b - 1]) {
                b -= 1
            }
            if a < b {
                let p = dom.create_html_element(Tag::P).expect("DOM node limit");
                dom.insert_before(children[a], p);
                for &x in &children[a..b] {
                    dom.append_child(p, x)
                }
                for &x in children[i..a].iter().chain(children[b..j].iter()) {
                    dom.detach(x)
                }
            }
        }
        i = j
    }
}
pub fn is_element_without_content(dom: &Dom, id: NodeId) -> bool {
    dom.is_element(id)
        && dom
            .element_children(id)
            .all(|c| matches!(dom.tag(c), Some(Tag::Br | Tag::Hr)))
        && !dom.has_non_whitespace_text(id)
}
pub fn has_single_tag_inside_element(dom: &Dom, id: NodeId, tag: Tag) -> bool {
    let mut found = false;
    for c in dom.children(id) {
        if dom.is_element(c) {
            if found || dom.tag(c) != Some(tag) {
                return false;
            }
            found = true
        } else if dom.is_text(c)
            && dom
                .text_node(c)
                .is_some_and(|t| t.ends_with(|x: char| !x.is_whitespace()))
        {
            return false;
        }
    }
    found
}
pub fn has_child_block_element(dom: &Dom, id: NodeId) -> bool {
    dom.descendants(id)
        .any(|x| dom.is_element(x) && dom.tag(x).is_some_and(is_div_to_p_elem))
}
pub fn is_probably_visible(dom: &Dom, id: NodeId) -> bool {
    if has_static_hidden_marker(dom, id) {
        return false;
    }
    if dom.attr(id, AttrName::AriaHidden) == Some("true")
        && !dom
            .attr(id, AttrName::Class)
            .is_some_and(|x| x.contains("fallback-image"))
    {
        return false;
    }
    true
}
pub fn is_valid_byline(dom: &Dom, id: NodeId, text_buffer: &mut String) -> bool {
    let ok = dom.attr(id, AttrName::Rel) == Some("author")
        || dom
            .attr(id, AttrName::ItemProp)
            .is_some_and(|x| x.contains("author"))
        || dom.attr(id, AttrName::Class).is_some_and(has_byline)
        || dom.attr(id, AttrName::Id).is_some_and(has_byline);
    if !ok {
        return false;
    }
    let t = get_inner_text(dom, id, text_buffer);
    !t.is_empty() && t.len() < 400 && t.chars().count() < 100
}
pub(crate) fn has_static_hidden_marker(dom: &Dom, id: NodeId) -> bool {
    dom.has_attr(id, AttrName::Hidden)
        || dom.attr(id, AttrName::Style).is_some_and(has_hidden_style)
}

pub(crate) fn has_hidden_utility_class(dom: &Dom, id: NodeId) -> bool {
    dom.attr(id, AttrName::Class).is_some_and(|classes| {
        let display_show = classes
            .split_ascii_whitespace()
            .any(is_responsive_display_show);
        let visibility_show = classes.split_ascii_whitespace().any(|class| {
            class
                .to_ascii_lowercase()
                .split_once(':')
                .is_some_and(|(variant, value)| {
                    is_responsive_breakpoint(variant) && value == "visible"
                })
        });
        let accessibility_show = classes.split_ascii_whitespace().any(|class| {
            class
                .to_ascii_lowercase()
                .split_once(':')
                .is_some_and(|(variant, value)| {
                    is_responsive_breakpoint(variant) && value == "not-sr-only"
                })
        });
        classes.split_ascii_whitespace().any(|class| {
            if ["hidden", "d-none", "display-none", "u-hidden"]
                .iter()
                .any(|expected| class.eq_ignore_ascii_case(expected))
            {
                !display_show
            } else if class.eq_ignore_ascii_case("invisible") {
                !visibility_show
            } else if class.eq_ignore_ascii_case("visually-hidden")
                || class.eq_ignore_ascii_case("sr-only")
            {
                !accessibility_show
            } else {
                false
            }
        })
    })
}

fn is_responsive_breakpoint(value: &str) -> bool {
    matches!(value, "sm" | "md" | "lg" | "xl" | "xxl" | "2xl")
}

fn is_responsive_display_show(class: &str) -> bool {
    let class = class.to_ascii_lowercase();
    let tailwind = class.split_once(':').is_some_and(|(variant, display)| {
        is_responsive_breakpoint(variant) && is_visible_display_utility(display)
    });
    let bootstrap = class
        .strip_prefix("d-")
        .and_then(|class| class.split_once('-'))
        .is_some_and(|(breakpoint, display)| {
            is_responsive_breakpoint(breakpoint) && is_visible_display_utility(display)
        });
    tailwind || bootstrap
}

fn is_visible_display_utility(value: &str) -> bool {
    matches!(
        value,
        "block"
            | "inline"
            | "inline-block"
            | "flex"
            | "inline-flex"
            | "grid"
            | "inline-grid"
            | "table"
            | "contents"
    )
}

pub(crate) fn has_hidden_utility_class_for_discovery(dom: &Dom, id: NodeId) -> bool {
    // Hidden skip links and page anchors must not change candidate boundaries.
    if dom.tag(id) == Some(Tag::A) || !has_hidden_utility_class(dom, id) {
        return false;
    }
    let authoritative_root = matches!(dom.tag(id), Some(Tag::Article | Tag::Main))
        || dom.attr(id, AttrName::Role).is_some_and(|roles| {
            roles.split_ascii_whitespace().any(|role| {
                role.eq_ignore_ascii_case("article") || role.eq_ignore_ascii_case("main")
            })
        });
    authoritative_root
        || dom.attr(id, AttrName::Class).is_some_and(|classes| {
            classes.split_ascii_whitespace().any(|class| {
                ["invisible", "d-none", "display-none", "u-hidden"]
                    .iter()
                    .any(|expected| class.eq_ignore_ascii_case(expected))
            })
        })
}

pub(crate) fn is_hidden_utility_class(class: &str) -> bool {
    // Match complete, unconditional utility names only. Responsive variants
    // such as `sm:hidden` can be visible at other viewport widths.
    [
        "hidden",
        "invisible",
        "visually-hidden",
        "sr-only",
        "d-none",
        "display-none",
        "u-hidden",
    ]
    .iter()
    .any(|expected| class.eq_ignore_ascii_case(expected))
}

fn has_hidden_style(style: &str) -> bool {
    // Store (hidden, important) for each property. A later declaration wins
    // unless an earlier declaration used !important.
    let mut display = None;
    let mut visibility = None;
    let mut opacity = None;
    for declaration in style.split(';') {
        let Some((property, raw_value)) = declaration.split_once(':') else {
            continue;
        };
        let (value, important) = raw_value.rsplit_once('!').map_or_else(
            || (raw_value.trim(), false),
            |(value, priority)| {
                if priority.trim().eq_ignore_ascii_case("important") {
                    (value.trim(), true)
                } else {
                    (raw_value.trim(), false)
                }
            },
        );
        let state = if property.trim().eq_ignore_ascii_case("display") {
            valid_display_visibility(value).map(|hidden| (&mut display, hidden))
        } else if property.trim().eq_ignore_ascii_case("visibility") {
            valid_visibility(value).map(|hidden| (&mut visibility, hidden))
        } else if property.trim().eq_ignore_ascii_case("opacity") {
            valid_opacity(value).map(|hidden| (&mut opacity, hidden))
        } else {
            None
        };
        if let Some((state, hidden)) = state
            && !state.is_some_and(|(_, previous_important)| previous_important && !important)
        {
            *state = Some((hidden, important));
        }
    }
    [display, visibility, opacity]
        .into_iter()
        .flatten()
        .any(|(hidden, _)| hidden)
}

fn valid_display_visibility(value: &str) -> Option<bool> {
    let value = value.to_ascii_lowercase();
    if value == "none" {
        return Some(true);
    }
    matches!(
        value.as_str(),
        "initial"
            | "inherit"
            | "unset"
            | "revert"
            | "revert-layer"
            | "block"
            | "inline"
            | "inline-block"
            | "flow-root"
            | "run-in"
            | "list-item"
            | "flex"
            | "inline-flex"
            | "grid"
            | "inline-grid"
            | "table"
            | "inline-table"
            | "table-row"
            | "table-cell"
            | "table-caption"
            | "table-row-group"
            | "table-header-group"
            | "table-footer-group"
            | "table-column"
            | "table-column-group"
            | "contents"
            | "ruby"
            | "ruby-base"
            | "ruby-text"
            | "ruby-base-container"
            | "ruby-text-container"
            | "-webkit-box"
    )
    .then_some(false)
}

fn valid_visibility(value: &str) -> Option<bool> {
    if value.eq_ignore_ascii_case("hidden") || value.eq_ignore_ascii_case("collapse") {
        Some(true)
    } else if [
        "visible",
        "initial",
        "inherit",
        "unset",
        "revert",
        "revert-layer",
    ]
    .iter()
    .any(|expected| value.eq_ignore_ascii_case(expected))
    {
        Some(false)
    } else {
        None
    }
}

fn valid_opacity(value: &str) -> Option<bool> {
    if ["initial", "inherit", "unset", "revert", "revert-layer"]
        .iter()
        .any(|expected| value.eq_ignore_ascii_case(expected))
    {
        return Some(false);
    }
    let opacity = value
        .strip_suffix('%')
        .map_or_else(
            || value.parse::<f64>(),
            |value| value.parse::<f64>().map(|value| value / 100.0),
        )
        .ok()?;
    (0.0..=1.0).contains(&opacity).then_some(opacity == 0.0)
}

#[cfg(test)]
mod tests {
    use super::{
        CandidateFeatureIndex, build_exclusion_mask, compute_readability_scores, get_link_density,
        get_link_density_cached, get_or_compute_stats, has_hidden_utility_class,
        has_hidden_utility_class_for_discovery, is_probably_visible, stats_for_text,
    };
    use crate::candidate::CandidateSet;
    use crate::dom::{AttrName, Dom, NodeStateStore, Tag};

    #[test]
    fn static_visibility_handles_inline_css_declarations() {
        let dom = Dom::parse_fragment(
            r#"<i style="DISPLAY : none !important"></i><i style="visibility: HIDDEN"></i><i style="opacity: 0.0"></i><i style="opacity: 0.2; display: block"></i><i style="not-display:none"></i><i style="display:none; display:block"></i><i style="visibility:hidden!important; visibility:visible"></i><i style="opacity:0!important; opacity:1!important"></i><i style="display:none; display:"></i><i style="opacity:0; opacity:invalid"></i><i style="display:none; display:initial"></i><i style="visibility:hidden; visibility:initial"></i><i style="opacity:0; opacity:initial"></i><i style="display:none; display:inherit"></i><i style="visibility:hidden; visibility:unset"></i><i style="opacity:0; opacity:inherit"></i>"#,
            Tag::Div,
        )
        .unwrap();
        let nodes = dom.element_children(dom.root()).collect::<Vec<_>>();
        assert!(!is_probably_visible(&dom, nodes[0]));
        assert!(!is_probably_visible(&dom, nodes[1]));
        assert!(!is_probably_visible(&dom, nodes[2]));
        assert!(is_probably_visible(&dom, nodes[3]));
        assert!(is_probably_visible(&dom, nodes[4]));
        assert!(is_probably_visible(&dom, nodes[5]));
        assert!(!is_probably_visible(&dom, nodes[6]));
        assert!(is_probably_visible(&dom, nodes[7]));
        assert!(!is_probably_visible(&dom, nodes[8]));
        assert!(!is_probably_visible(&dom, nodes[9]));
        assert!(is_probably_visible(&dom, nodes[10]));
        assert!(is_probably_visible(&dom, nodes[11]));
        assert!(is_probably_visible(&dom, nodes[12]));
        assert!(is_probably_visible(&dom, nodes[13]));
        assert!(is_probably_visible(&dom, nodes[14]));
        assert!(is_probably_visible(&dom, nodes[15]));
    }

    #[test]
    fn hidden_utilities_do_not_match_responsive_variants() {
        let dom = Dom::parse_fragment(
            r#"<i class="hidden"></i><i class="invisible"></i><i class="visually-hidden"></i><i class="sr-only"></i><i class="sm:hidden md:invisible not-sr-only"></i><i class="hidden md:block"></i><a class="hidden"></a><i class="d-none d-md-block"></i><i class="invisible md:block"></i><i class="invisible md:visible"></i><i class="sr-only md:not-sr-only"></i>"#,
            Tag::Div,
        )
        .unwrap();
        let nodes = dom.element_children(dom.root()).collect::<Vec<_>>();
        assert!(
            nodes[..4]
                .iter()
                .all(|&node| has_hidden_utility_class(&dom, node))
        );
        assert!(!has_hidden_utility_class(&dom, nodes[4]));
        assert!(!has_hidden_utility_class(&dom, nodes[5]));
        assert!(has_hidden_utility_class(&dom, nodes[6]));
        assert!(!has_hidden_utility_class_for_discovery(&dom, nodes[6]));
        assert!(!has_hidden_utility_class(&dom, nodes[7]));
        assert!(has_hidden_utility_class(&dom, nodes[8]));
        assert!(!has_hidden_utility_class(&dom, nodes[9]));
        assert!(!has_hidden_utility_class(&dom, nodes[10]));
    }

    #[test]
    fn text_stats_match_for_ascii_and_unicode_paths() {
        let ascii = stats_for_text(" a,\t b. c ");
        assert_eq!(ascii.text_length, 7);
        assert_eq!(ascii.word_count, 3);
        assert_eq!(ascii.comma_count, 1);
        assert_eq!(ascii.sentence_end_count, 1);
        assert!(ascii.starts_with_whitespace());
        assert!(ascii.ends_with_whitespace());
        assert!(ascii.has_sentence_break());
        assert!(ascii.has_sentence_end());

        let unicode = stats_for_text("\u{3000}甲， 乙.\u{a0}");
        assert_eq!(unicode.text_length, 5);
        assert_eq!(unicode.word_count, 2);
        assert_eq!(unicode.comma_count, 1);
        assert_eq!(unicode.sentence_end_count, 1);
        assert!(unicode.starts_with_whitespace());
        assert!(unicode.ends_with_whitespace());
        assert!(unicode.has_sentence_end());

        let spaces_only = stats_for_text("   ");
        assert_eq!(spaces_only.text_length, 0);
        assert_eq!(spaces_only.word_count, 0);
        assert!(!spaces_only.has_non_whitespace());

        let space_separated = stats_for_text("  Alpha  beta.  ");
        assert_eq!(space_separated.text_length, 11);
        assert_eq!(space_separated.word_count, 2);
        assert!(space_separated.has_sentence_break());
        assert!(!space_separated.ends_with_dot());
    }

    #[test]
    fn general_features_include_text_structure_links_and_names() {
        let dom = Dom::parse_document(
            r#"<body><main class="article-content sidebar"><h1>Reference</h1>
            <p>Read the <a href="/guide">complete guide</a>. Then continue!</p>
            <ul><li>First</li><li>Second</li></ul><pre><code>cargo test</code></pre>
            <table><thead><tr><th>Key</th><th>Value</th></tr></thead></table>
            <table role="presentation"><tr><td>Left</td><td>Right</td></tr></table>
            <figure><img src="plot.png"><figcaption>Plot</figcaption></figure>
            <details><summary>Notes</summary><p>More detail.</p></details></main></body>"#,
        )
        .unwrap();
        let main = dom.first_descendant_by_tag(dom.root(), Tag::Main).unwrap();
        let candidates = CandidateSet::discover_semantic(&dom);
        let candidate = *candidates
            .iter()
            .find(|candidate| candidate.node == main)
            .unwrap();
        let candidate_index = candidates.index_of(main).unwrap();
        let mut store = NodeStateStore::new();
        let mut table_nodes = Vec::new();
        let snapshot = dom.element_descendants_snapshot_with_depth(dom.root());
        crate::cleaning::mark_data_tables_from_snapshot(
            &dom,
            dom.root(),
            &snapshot,
            &mut store,
            &mut table_nodes,
        );
        let index = CandidateFeatureIndex::new(&dom, &store, &snapshot, &candidates);
        index.prepare_text_cache(&mut store);
        let features = index.features(&dom, candidate_index, candidate, &mut store, true);

        assert!(features.word_count >= 12);
        assert_eq!(features.paragraph_count, 2);
        assert_eq!(features.heading_count, 1);
        assert_eq!(features.list_item_count, 2);
        assert_eq!(features.code_block_count, 1);
        assert_eq!(features.table_count, 1);
        assert_eq!(features.figure_count, 1);
        assert_eq!(features.image_count, 1);
        assert!(features.protected_block_count >= 1);
        assert!(features.link_text_chars > 0.0);
        assert!(features.link_density > 0.0 && features.link_density < 1.0);
        assert!(features.positive_name_score > 0.0);
        assert!(features.negative_name_score > 0.0);

        let unweighted = index.features(&dom, candidate_index, candidate, &mut store, false);
        assert_eq!(unweighted.positive_name_score, 0.0);
        assert_eq!(unweighted.negative_name_score, 0.0);
    }

    #[test]
    fn structural_counts_remain_exact_for_boundary_selection() {
        let table = "<table><tr><td>A</td><td>B</td></tr>".to_owned()
            + &"<tr><td>C</td><td>D</td></tr>".repeat(5)
            + "</table>";
        let html = format!(
            "<body><main>{}{}{}{}{}</main></body>",
            "<p>Text</p>".repeat(300),
            "<h2>Heading</h2>".repeat(300),
            "<pre>Code</pre>".repeat(300),
            table.repeat(300),
            "<figure><img src=\"image.png\"></figure>".repeat(300),
        );
        let dom = Dom::parse_document(&html).unwrap();
        let main = dom.first_descendant_by_tag(dom.root(), Tag::Main).unwrap();
        let candidates = CandidateSet::discover_semantic(&dom);
        let candidate = *candidates
            .iter()
            .find(|candidate| candidate.node == main)
            .unwrap();
        let candidate_index = candidates.index_of(main).unwrap();
        let mut store = NodeStateStore::new();
        let mut table_nodes = Vec::new();
        let snapshot = dom.element_descendants_snapshot_with_depth(dom.root());
        crate::cleaning::mark_data_tables_from_snapshot(
            &dom,
            dom.root(),
            &snapshot,
            &mut store,
            &mut table_nodes,
        );
        let index = CandidateFeatureIndex::new(&dom, &store, &snapshot, &candidates);
        let features = index.features(&dom, candidate_index, candidate, &mut store, false);

        assert_eq!(features.paragraph_count, 300);
        assert_eq!(features.heading_count, 300);
        assert_eq!(features.code_block_count, 300);
        assert_eq!(features.table_count, 300);
        assert_eq!(features.figure_count, 300);
    }

    #[test]
    fn structural_counts_aggregate_through_nested_candidates() {
        let dom = Dom::parse_document(
            r#"<body><main><section id="outer"><p>Before.</p><section id="inner"><h2>Inside</h2><p>Inner.</p><figure><img src="plot.png"></figure></section><p>After.</p></section></main></body>"#,
        )
        .unwrap();
        let outer = dom
            .descendants(dom.root())
            .find(|&node| dom.attr(node, AttrName::Id) == Some("outer"))
            .unwrap();
        let inner = dom
            .descendants(dom.root())
            .find(|&node| dom.attr(node, AttrName::Id) == Some("inner"))
            .unwrap();
        let candidates = CandidateSet::discover_semantic(&dom);
        let snapshot = dom.element_descendants_snapshot_with_depth(dom.root());
        let mut store = NodeStateStore::new();
        let mut table_nodes = Vec::new();
        crate::cleaning::mark_data_tables_from_snapshot(
            &dom,
            dom.root(),
            &snapshot,
            &mut store,
            &mut table_nodes,
        );
        let index = CandidateFeatureIndex::new(&dom, &store, &snapshot, &candidates);

        assert_eq!(index.counts.len(), candidates.iter().count());
        let outer_features = index.features(
            &dom,
            candidates.index_of(outer).unwrap(),
            *candidates.get(outer).unwrap(),
            &mut store,
            false,
        );
        let inner_features = index.features(
            &dom,
            candidates.index_of(inner).unwrap(),
            *candidates.get(inner).unwrap(),
            &mut store,
            false,
        );

        assert_eq!(inner_features.paragraph_count, 1);
        assert_eq!(inner_features.heading_count, 1);
        assert_eq!(inner_features.figure_count, 1);
        assert_eq!(inner_features.image_count, 1);
        assert_eq!(outer_features.paragraph_count, 3);
        assert_eq!(outer_features.heading_count, 1);
        assert_eq!(outer_features.figure_count, 1);
        assert_eq!(outer_features.image_count, 1);
    }

    #[test]
    fn link_density_penalty_is_bounded() {
        let sparse = crate::candidate::CandidateFeatures {
            text_chars: 1_000,
            word_count: 100,
            ..Default::default()
        };
        let linked = crate::candidate::CandidateFeatures {
            link_text_chars: 1_000.0,
            link_density: 1.0,
            ..sparse
        };

        assert!(sparse.ranking_score() - linked.ranking_score() <= 4.0);
    }

    #[test]
    fn substantial_content_can_overcome_negative_name_evidence() {
        let substantial_negative = crate::candidate::CandidateFeatures {
            readability_score: 100.0,
            negative_name_score: 1.0,
            ..Default::default()
        };
        let ordinary = crate::candidate::CandidateFeatures {
            readability_score: 20.0,
            ..Default::default()
        };

        assert!(substantial_negative.ranking_score() > ordinary.ranking_score());

        let zero_score_negative = crate::candidate::CandidateFeatures {
            negative_name_score: 1.0,
            ..Default::default()
        };
        assert!(zero_score_negative.ranking_score() < 0.0);
    }

    #[test]
    fn traditional_article_produces_a_readability_candidate() {
        let mut dom = Dom::parse_document(
            r#"<body><article><p>This traditional article paragraph contains enough prose, commas, and detail to contribute a strong content score.</p></article><aside><p>Short note.</p></aside></body>"#,
        )
        .unwrap();
        let article = dom
            .first_descendant_by_tag(dom.root(), Tag::Article)
            .unwrap();
        let paragraphs: Vec<_> = dom
            .descendants(dom.root())
            .filter(|&node| dom.tag(node) == Some(Tag::P))
            .collect();
        let mut store = NodeStateStore::new();

        let scores = compute_readability_scores(&mut dom, paragraphs, &[], &[], &mut store, true);

        let article_score = scores
            .iter()
            .find(|candidate| candidate.node == article)
            .map(|candidate| candidate.score)
            .unwrap();
        assert!(article_score > 2.0);
    }

    #[test]
    fn invalidating_excluded_descendant_refreshes_ancestor_stats_and_links() {
        let mut dom = Dom::parse_document(
            r#"<body><main><p>Visible answer contains enough useful words for scoring.</p><div id="excluded"><a href="/ad">Excluded linked promotion with many extra words.</a></div></main></body>"#,
        )
        .unwrap();
        let main = dom.first_descendant_by_tag(dom.root(), Tag::Main).unwrap();
        let visible = dom
            .descendants(main)
            .find(|&node| dom.tag(node) == Some(Tag::P))
            .unwrap();
        let excluded = dom
            .descendants(main)
            .find(|&node| dom.attr(node, crate::dom::AttrName::Id) == Some("excluded"))
            .unwrap();
        let mut store = NodeStateStore::new();
        store.enable_link_lengths();
        let stale = get_or_compute_stats(&dom, main, &mut store);
        assert!(stale.text_length > 80);
        assert!(store.link_length(main) > 0.0);

        let excluded_mask = build_exclusion_mask(&dom, &[excluded]);
        let scores = compute_readability_scores(
            &mut dom,
            [visible],
            &[excluded],
            &excluded_mask,
            &mut store,
            true,
        );
        assert!(scores.iter().any(|score| score.node == main));

        let fresh = get_or_compute_stats(&dom, main, &mut store);
        assert!(fresh.text_length < stale.text_length);
        assert_eq!(store.link_length(main), 0.0);
    }

    #[test]
    fn cached_link_density_matches_structural_scan() {
        let dom = Dom::parse_fragment(
            r##"plain <a href="/full">full</a> <a href="#hash">hash</a>"##,
            Tag::Div,
        )
        .unwrap();
        let root = dom.root();
        let mut store = NodeStateStore::new();
        store.enable_link_lengths();
        let len = dom.normalized_char_count(root);

        let expected = get_link_density(&dom, root);
        let actual = get_link_density_cached(&dom, root, len as u32, &mut store);
        assert!((actual - expected).abs() < f64::EPSILON);
    }
}
