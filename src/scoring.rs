//! Content scoring and DOM text helpers.
use crate::candidate::{Candidate, CandidateFeatures, CandidateSet};
use crate::constants::is_div_to_p_elem;
use crate::constants::{has_byline, is_phrasing_elem, is_unlikely_role, regexps};
use crate::dom::{AttrName, Dom, NodeId, NodeStateStore, NodeStats, Tag};
use crate::prepared::{SourceAnalysis, SourceEntry};
use crate::tokens::{has_any_token, has_token};
use smallvec::SmallVec;

pub(crate) const TOP_CANDIDATES: usize = 5;

#[derive(Clone, Copy, Debug)]
pub(crate) enum PreparedScoreSeed {
    Node { node: NodeId, parent: NodeId },
    Virtual { parent: NodeId, stats: NodeStats },
}

/// The differences between the immutable source tree and Readability's
/// temporary scoring tree.
///
/// Scoring used to apply these differences to a complete DOM clone. Keep them
/// sparse instead. The source topology and attributes stay shared by normal
/// and relaxed scoring.
pub(crate) struct ScoringView {
    tag_overrides: SmallVec<[(NodeId, Tag); 64]>,
    prepared_seeds: SmallVec<[PreparedScoreSeed; 256]>,
    wrapper_replacements: SmallVec<[(NodeId, NodeId); 32]>,
    parent_overrides: SmallVec<[(NodeId, NodeId); 32]>,
    text_overrides: SmallVec<[(NodeId, NodeStats); 32]>,
    /// Sparse facts for the DIVs that need projection. This replaces a
    /// descendant walk for every prepared DIV without adding a DOM-sized
    /// scoring allocation for each visibility variant.
    div_descendant_facts: SmallVec<[(NodeId, bool); 128]>,
}

impl ScoringView {
    pub(crate) fn build(
        dom: &Dom,
        source: &SourceAnalysis,
        divs: &[NodeId],
        candidates: &CandidateSet,
    ) -> Self {
        let mut view = Self {
            tag_overrides: SmallVec::new(),
            prepared_seeds: SmallVec::new(),
            wrapper_replacements: SmallVec::new(),
            parent_overrides: SmallVec::new(),
            text_overrides: SmallVec::new(),
            div_descendant_facts: SmallVec::new(),
        };
        view.project_aria_structure(dom, source);
        view.sort_and_compact_tags();
        let projected_tag_count = view.tag_overrides.len();
        view.div_descendant_facts =
            view.build_div_descendant_facts(dom, source, divs, projected_tag_count);
        view.prepare_divs(dom, source, divs, candidates, projected_tag_count);
        view.tag_overrides.sort_unstable_by_key(|(node, _)| *node);
        view.wrapper_replacements
            .sort_unstable_by_key(|(wrapper, _)| *wrapper);
        view.parent_overrides
            .sort_unstable_by_key(|(node, _)| *node);
        view.text_overrides.sort_by_key(|(node, _)| *node);
        let mut compacted_text = SmallVec::<[(NodeId, NodeStats); 32]>::new();
        for (node, stats) in view.text_overrides.drain(..) {
            if let Some((last_node, last_stats)) = compacted_text.last_mut()
                && *last_node == node
            {
                *last_stats = stats;
            } else {
                compacted_text.push((node, stats));
            }
        }
        view.text_overrides = compacted_text;
        view
    }

    #[inline]
    pub(crate) fn effective_tag(&self, dom: &Dom, node: NodeId) -> Option<Tag> {
        self.tag_overrides
            .binary_search_by_key(&node, |(node, _)| *node)
            .ok()
            .map(|index| self.tag_overrides[index].1)
            .or_else(|| dom.tag(node))
    }

    pub(crate) fn prepared_seeds(&self) -> &[PreparedScoreSeed] {
        &self.prepared_seeds
    }

    pub(crate) fn ignores_wrapper(&self, node: NodeId) -> bool {
        self.wrapper_replacements
            .binary_search_by_key(&node, |(wrapper, _)| *wrapper)
            .is_ok()
    }

    pub(crate) fn effective_parent(&self, dom: &Dom, node: NodeId) -> Option<NodeId> {
        let mut parent = self
            .parent_overrides
            .binary_search_by_key(&node, |(node, _)| *node)
            .ok()
            .map(|index| self.parent_overrides[index].1)
            .or_else(|| dom.parent(node));
        while parent.is_some_and(|parent| {
            self.ignores_wrapper(parent) && self.projected_node(parent) == parent
        }) {
            parent = parent.and_then(|parent| dom.parent(parent));
        }
        parent
    }

    pub(crate) fn effective_element_children(
        &self,
        dom: &Dom,
        parent: NodeId,
    ) -> SmallVec<[NodeId; 16]> {
        dom.element_children(parent)
            .map(|child| self.projected_node(child))
            .filter(|&child| !(self.ignores_wrapper(child) && self.projected_node(child) == child))
            .collect()
    }

    pub(crate) fn effective_ancestors(&self, dom: &Dom, node: NodeId) -> SmallVec<[NodeId; 16]> {
        let mut ancestors = SmallVec::new();
        let mut current = self.effective_parent(dom, node);
        while let Some(parent) = current {
            ancestors.push(parent);
            current = self.effective_parent(dom, parent);
        }
        ancestors
    }

    pub(crate) fn projected_node(&self, node: NodeId) -> NodeId {
        self.wrapper_replacements
            .binary_search_by_key(&node, |(wrapper, _)| *wrapper)
            .ok()
            .map_or(node, |index| self.wrapper_replacements[index].1)
    }

    pub(crate) fn seed_text_overrides(&self, store: &mut NodeStateStore) {
        for &(node, stats) in &self.text_overrides {
            store.set_stats(node, stats);
        }
    }

    fn projected_tag(&self, dom: &Dom, node: NodeId, projected_tag_count: usize) -> Option<Tag> {
        self.tag_overrides
            .get(..projected_tag_count)
            .and_then(|overrides| {
                overrides
                    .binary_search_by_key(&node, |(node, _)| *node)
                    .ok()
                    .map(|index| overrides[index].1)
            })
            .or_else(|| dom.tag(node))
    }

    fn set_tag(&mut self, node: NodeId, tag: Tag) {
        self.tag_overrides.push((node, tag));
    }

    fn sort_and_compact_tags(&mut self) {
        self.tag_overrides.sort_by_key(|(node, _)| *node);
        let mut compacted = SmallVec::<[(NodeId, Tag); 64]>::new();
        for (node, tag) in self.tag_overrides.drain(..) {
            if let Some((last_node, last_tag)) = compacted.last_mut()
                && *last_node == node
            {
                *last_tag = tag;
            } else {
                compacted.push((node, tag));
            }
        }
        self.tag_overrides = compacted;
    }

    fn project_aria_structure(&mut self, dom: &Dom, source: &SourceAnalysis) {
        for entry in source.elements() {
            let node = entry.node;
            let roles = dom.attr(node, AttrName::Role).unwrap_or_default();
            if has_token(roles, "heading")
                && let Some(tag) = dom
                    .attr_by_local_name(node, "aria-level")
                    .and_then(|value| value.trim().parse::<u8>().ok())
                    .and_then(|level| match level {
                        1 => Some(Tag::H1),
                        2 => Some(Tag::H2),
                        3 => Some(Tag::H3),
                        4 => Some(Tag::H4),
                        5 => Some(Tag::H5),
                        6 => Some(Tag::H6),
                        _ => None,
                    })
            {
                self.set_tag(node, tag);
            }
            if !has_token(roles, "list") {
                continue;
            }
            let items: SmallVec<[NodeId; 16]> = dom
                .element_children(node)
                .filter(|&child| {
                    dom.attr(child, AttrName::Role)
                        .is_some_and(|roles| has_token(roles, "listitem"))
                })
                .collect();
            if items.is_empty() {
                continue;
            }
            if !matches!(dom.tag(node), Some(Tag::Ol | Tag::Ul)) {
                if let Some(markers) = scoring_ordered_markers(dom, &items) {
                    self.set_tag(node, Tag::Ol);
                    for (_, text, replacement) in markers {
                        self.text_overrides
                            .push((text, stats_for_text(replacement)));
                    }
                } else {
                    self.set_tag(node, Tag::Ul);
                }
            }
            for item in items {
                self.set_tag(item, Tag::Li);
            }
        }
    }

    fn prepare_divs(
        &mut self,
        dom: &Dom,
        source: &SourceAnalysis,
        divs: &[NodeId],
        candidates: &CandidateSet,
        projected_tag_count: usize,
    ) {
        let mut stats = NodeStateStore::new();
        for &div in divs {
            if dom.parent(div).is_none()
                || self.projected_tag(dom, div, projected_tag_count) != Some(Tag::Div)
            {
                continue;
            }
            let children: SmallVec<[NodeId; 8]> = dom.children(div).collect();
            let mut effective = SmallVec::<[PreparedScoreSeed; 8]>::new();
            let mut index = 0;
            while index < children.len() {
                if !is_phrasing_content(dom, children[index]) {
                    effective.push(PreparedScoreSeed::Node {
                        node: children[index],
                        parent: div,
                    });
                    index += 1;
                    continue;
                }
                let start = index;
                let mut has_content = false;
                while index < children.len() && is_phrasing_content(dom, children[index]) {
                    has_content |= dom.is_element(children[index])
                        || dom
                            .text_node(children[index])
                            .is_some_and(|text| !trim_text(text).is_empty());
                    index += 1;
                }
                if !has_content {
                    continue;
                }
                let mut first = start;
                let mut end = index;
                while first < end && is_whitespace(dom, children[first]) {
                    first += 1;
                }
                while first < end && is_whitespace(dom, children[end - 1]) {
                    end -= 1;
                }
                if first < end {
                    let mut combined = NodeStats::default();
                    for &child in &children[first..end] {
                        let child_stats = get_or_compute_stats_from_source(
                            dom,
                            Some(source),
                            child,
                            &mut stats,
                            &[],
                        );
                        append_stats(&mut combined, &child_stats);
                    }
                    effective.push(PreparedScoreSeed::Virtual {
                        parent: div,
                        stats: combined,
                    });
                }
            }

            let paragraph_seed = |seed: PreparedScoreSeed, parent| match seed {
                PreparedScoreSeed::Node { node, .. } if dom.tag(node) == Some(Tag::P) => {
                    Some(PreparedScoreSeed::Node { node, parent })
                }
                PreparedScoreSeed::Virtual { stats, .. } => {
                    Some(PreparedScoreSeed::Virtual { parent, stats })
                }
                _ => None,
            };
            if candidates.is_semantic(div) {
                self.prepared_seeds.extend(
                    effective
                        .into_iter()
                        .filter_map(|seed| paragraph_seed(seed, div)),
                );
            } else if effective.len() == 1
                && paragraph_seed(effective[0], div).is_some()
                && get_link_density(dom, div) < 0.25
            {
                let parent = dom.parent(div).expect("attached div has a parent");
                if let PreparedScoreSeed::Node { node, .. } = effective[0] {
                    self.parent_overrides.push((node, parent));
                }
                self.prepared_seeds
                    .push(paragraph_seed(effective[0], parent).expect("paragraph seed"));
                let promoted = match effective[0] {
                    PreparedScoreSeed::Node { node, .. } => node,
                    PreparedScoreSeed::Virtual { .. } => div,
                };
                // A virtual paragraph has no source NodeId. A self replacement
                // marks its old wrapper as absent while the virtual seed carries
                // the paragraph score at the projected parent.
                self.wrapper_replacements.push((div, promoted));
            } else if !self
                .div_descendant_facts
                .binary_search_by_key(&div, |(node, _)| *node)
                .ok()
                .map(|index| self.div_descendant_facts[index].1)
                .unwrap_or_else(|| {
                    // Synthetic or detached nodes are not part of the prepared
                    // source index. Keep the cold fallback exact for tests and
                    // callers that construct a view over such a tree.
                    dom.descendants(div).any(|node| {
                        dom.is_element(node)
                            && self
                                .projected_tag(dom, node, projected_tag_count)
                                .is_some_and(crate::constants::is_div_to_p_elem)
                    })
                })
            {
                self.set_tag(div, Tag::P);
                self.prepared_seeds.push(PreparedScoreSeed::Node {
                    node: div,
                    parent: dom.parent(div).expect("attached div has a parent"),
                });
            } else {
                self.prepared_seeds.extend(
                    effective
                        .into_iter()
                        .filter_map(|seed| paragraph_seed(seed, div)),
                );
            }
        }
    }

    fn build_div_descendant_facts(
        &self,
        dom: &Dom,
        source: &SourceAnalysis,
        divs: &[NodeId],
        projected_tag_count: usize,
    ) -> SmallVec<[(NodeId, bool); 128]> {
        if divs.is_empty() {
            return SmallVec::new();
        }
        let matching_positions: Vec<_> = source
            .entries
            .iter()
            .enumerate()
            .filter_map(|(position, entry)| {
                (entry.is_element()
                    && self
                        .projected_tag(dom, entry.node, projected_tag_count)
                        .is_some_and(crate::constants::is_div_to_p_elem))
                .then_some(position)
            })
            .collect();
        let mut facts = SmallVec::with_capacity(divs.len());
        for &div in divs {
            let Some(range) = source.subtree_range(div) else {
                continue;
            };
            let first = matching_positions.partition_point(|&position| position <= range.start);
            let contains = matching_positions
                .get(first)
                .is_some_and(|&position| position < range.end);
            facts.push((div, contains));
        }
        facts.sort_unstable_by_key(|(node, _)| *node);
        facts
    }
}

fn scoring_ordered_markers<'a>(
    dom: &'a Dom,
    items: &[NodeId],
) -> Option<Vec<(u32, NodeId, &'a str)>> {
    if items.len() < 2 {
        return None;
    }
    let markers: Vec<_> = items
        .iter()
        .map(|&item| scoring_first_text_marker(dom, item))
        .collect::<Option<_>>()?;
    let first = markers.first()?.0;
    if !markers
        .iter()
        .enumerate()
        .all(|(index, marker)| marker.0 == first.saturating_add(index as u32))
    {
        return None;
    }
    markers
        .into_iter()
        .map(|(number, text, prefix_end)| {
            Some((
                number,
                text,
                dom.text_node(text)?[prefix_end..].trim_start(),
            ))
        })
        .collect()
}

fn scoring_first_text_marker(dom: &Dom, item: NodeId) -> Option<(u32, NodeId, usize)> {
    let text = dom.descendants(item).find(|&node| {
        dom.text_node(node)
            .is_some_and(|value| !value.trim().is_empty())
    })?;
    let value = dom.text_node(text)?;
    let leading = value.len() - value.trim_start().len();
    let trimmed = &value[leading..];
    let digits = trimmed.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 || !matches!(trimmed.as_bytes().get(digits), Some(b'.' | b')')) {
        return None;
    }
    if trimmed
        .as_bytes()
        .get(digits + 1)
        .is_some_and(|byte| !byte.is_ascii_whitespace())
    {
        return None;
    }
    Some((trimmed[..digits].parse().ok()?, text, leading + digits + 1))
}

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

pub(crate) fn stats_for_text(text: &str) -> NodeStats {
    let mut s = NodeStats::default();
    s.set_has_text(!text.is_empty());
    if let [byte] = text.as_bytes()
        && byte.is_ascii()
    {
        let class = ASCII_CLASSES[usize::from(*byte)];
        s.set_starts_with_whitespace(class & ASCII_WHITESPACE != 0);
        s.set_ends_with_whitespace(class & ASCII_WHITESPACE != 0);
        if class & ASCII_WHITESPACE == 0 {
            s.text_length = 1;
            s.word_count = 1;
            s.alphabetic_chars = u32::from(class & ASCII_ALPHA != 0);
            s.digit_chars = u32::from(class & ASCII_DIGIT != 0);
            s.comma_count = u32::from(class & ASCII_COMMA != 0);
            s.sentence_end_count = u32::from(class & ASCII_SENTENCE_END != 0);
            s.set_has_non_whitespace(true);
            s.set_has_alphanumeric(class & (ASCII_ALPHA | ASCII_DIGIT) != 0);
            s.set_ends_with_dot(class & ASCII_DOT != 0);
            s.set_has_sentence_end(class & ASCII_SENTENCE_END != 0);
        }
        return s;
    }
    // Source HTML has many short text leaves. For these leaves, combine the
    // ASCII check with the statistics scan instead of walking the bytes once
    // in `is_ascii` and once again to count them.
    if text.len() <= 64
        && let Some(stats) = short_ascii_stats(text.as_bytes())
    {
        return stats;
    }
    let mut prev = true;
    let mut dot = false;
    let mut text_length = 0usize;
    let mut word_count = 0usize;
    let mut comma_count = 0usize;
    let mut sentence_end_count = 0usize;
    let mut has_non_whitespace = false;
    let mut has_sentence_break = false;
    // Most article text is ASCII. Scan bytes to avoid UTF-8 decoding and
    // Unicode whitespace tables in the hot loop.
    let has_alphanumeric = if text.is_ascii() {
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
        alphabetic_chars != 0 || digit_chars != 0
    } else {
        s.set_starts_with_whitespace(text.starts_with(char::is_whitespace));
        s.set_ends_with_whitespace(text.ends_with(char::is_whitespace));
        let mut alphabetic_chars = 0_usize;
        let mut digit_chars = 0_usize;
        // Mixed-content pages are mostly ASCII with isolated non-ASCII
        // characters. Handle ASCII bytes through the class table and decode
        // only real non-ASCII characters, so common text avoids the Unicode
        // property tables.
        let bytes = text.as_bytes();
        let mut index = 0;
        while index < bytes.len() {
            let byte = bytes[index];
            if byte.is_ascii() {
                index += 1;
                let class = ASCII_CLASSES[usize::from(byte)];
                if class & ASCII_WHITESPACE != 0 {
                    has_sentence_break |= dot;
                    dot = false;
                    if !prev {
                        text_length += 1;
                        prev = true;
                    }
                } else {
                    has_non_whitespace = true;
                    word_count += usize::from(prev);
                    dot = class & ASCII_DOT != 0;
                    comma_count += usize::from(class & ASCII_COMMA != 0);
                    sentence_end_count += usize::from(class & ASCII_SENTENCE_END != 0);
                    alphabetic_chars += usize::from(class & ASCII_ALPHA != 0);
                    digit_chars += usize::from(class & ASCII_DIGIT != 0);
                    text_length += 1;
                    prev = false;
                }
                continue;
            }
            let Some(c) = text[index..].chars().next() else {
                break;
            };
            index += c.len_utf8();
            if c.is_whitespace() {
                has_sentence_break |= dot;
                dot = false;
                if !prev {
                    text_length += 1;
                    prev = true;
                }
            } else {
                has_non_whitespace = true;
                alphabetic_chars += usize::from(c.is_alphabetic());
                digit_chars += usize::from(c.is_numeric());
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
                prev = false;
            }
        }
        s.alphabetic_chars = alphabetic_chars.min(u32::MAX as usize) as u32;
        s.digit_chars = digit_chars.min(u32::MAX as usize) as u32;
        alphabetic_chars != 0 || digit_chars != 0
    };
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

fn short_ascii_stats(bytes: &[u8]) -> Option<NodeStats> {
    let mut stats = NodeStats::default();
    stats.set_has_text(!bytes.is_empty());
    stats.set_starts_with_whitespace(
        bytes
            .first()
            .is_some_and(|byte| ASCII_CLASSES[usize::from(*byte)] & ASCII_WHITESPACE != 0),
    );
    stats.set_ends_with_whitespace(
        bytes
            .last()
            .is_some_and(|byte| ASCII_CLASSES[usize::from(*byte)] & ASCII_WHITESPACE != 0),
    );

    let mut previous_whitespace = true;
    let mut ends_with_dot = false;
    let mut has_sentence_break = false;
    let mut text_length = 0usize;
    let mut word_count = 0usize;
    let mut alphabetic_chars = 0usize;
    let mut digit_chars = 0usize;
    let mut comma_count = 0usize;
    let mut sentence_end_count = 0usize;
    for &byte in bytes {
        if !byte.is_ascii() {
            return None;
        }
        let class = ASCII_CLASSES[usize::from(byte)];
        alphabetic_chars += usize::from(class & ASCII_ALPHA != 0);
        digit_chars += usize::from(class & ASCII_DIGIT != 0);
        comma_count += usize::from(class & ASCII_COMMA != 0);
        sentence_end_count += usize::from(class & ASCII_SENTENCE_END != 0);
        if class & ASCII_WHITESPACE != 0 {
            has_sentence_break |= ends_with_dot;
            ends_with_dot = false;
            if !previous_whitespace {
                text_length += 1;
                previous_whitespace = true;
            }
        } else {
            word_count += usize::from(previous_whitespace);
            ends_with_dot = class & ASCII_DOT != 0;
            text_length += 1;
            previous_whitespace = false;
        }
    }
    if previous_whitespace && text_length > 0 {
        text_length -= 1;
    }
    stats.text_length = text_length as u32;
    stats.word_count = word_count as u32;
    stats.alphabetic_chars = alphabetic_chars as u32;
    stats.digit_chars = digit_chars as u32;
    stats.comma_count = comma_count as u32;
    stats.sentence_end_count = sentence_end_count as u32;
    stats.set_has_non_whitespace(word_count != 0);
    stats.set_has_alphanumeric(alphabetic_chars != 0 || digit_chars != 0);
    stats.set_has_sentence_break(has_sentence_break);
    stats.set_ends_with_dot(ends_with_dot);
    stats.set_has_sentence_end(has_sentence_break || ends_with_dot);
    Some(stats)
}

pub(crate) fn append_stats(a: &mut NodeStats, b: &NodeStats) {
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

#[inline]
fn append_content_stats(a: &mut NodeStats, b: &NodeStats) {
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
    a.alphabetic_chars = a.alphabetic_chars.saturating_add(b.alphabetic_chars);
    a.digit_chars = a.digit_chars.saturating_add(b.digit_chars);
    a.set_has_non_whitespace(a_non_whitespace || b_non_whitespace);
    a.set_has_alphanumeric(a.has_alphanumeric() || b.has_alphanumeric());
    a.set_ends_with_whitespace(b.ends_with_whitespace());
}
pub fn get_or_compute_stats(dom: &Dom, id: NodeId, store: &mut NodeStateStore) -> NodeStats {
    get_or_compute_stats_excluding(dom, id, store, &[])
}

fn get_or_compute_stats_from_source(
    dom: &Dom,
    source: Option<&SourceAnalysis>,
    id: NodeId,
    store: &mut NodeStateStore,
    excluded: &[bool],
) -> NodeStats {
    get_or_compute_stats_excluding_impl(dom, source, id, store, excluded)
}

pub(crate) fn get_or_compute_stats_from_source_excluding(
    dom: &Dom,
    source: &SourceAnalysis,
    id: NodeId,
    store: &mut NodeStateStore,
    excluded: &[bool],
) -> NodeStats {
    get_or_compute_stats_excluding_impl(dom, Some(source), id, store, excluded)
}

pub(crate) type SourceTextMetrics = (NodeStats, f64, usize);

/// Computes normal and relaxed source metrics during one source-order walk.
/// Text leaves use the lexical facts that `SourceAnalysis` collected while it
/// built the source index.
pub(crate) fn stats_from_analysis_entries_pair(
    dom: &Dom,
    source: &SourceAnalysis,
    entries: &[SourceEntry],
    normal_excluded: &[bool],
    relaxed_excluded: Option<&[bool]>,
) -> (SourceTextMetrics, Option<SourceTextMetrics>) {
    #[derive(Clone, Copy)]
    struct Frame {
        node: NodeId,
        depth: u32,
        tag: Option<Tag>,
        stats: NodeStats,
        link_length: f64,
        link_text_length: u32,
    }

    struct Accumulator<'a> {
        dom: &'a Dom,
        excluded: &'a [bool],
        excluded_depth: Option<u32>,
        stack: SmallVec<[Frame; 32]>,
        result: NodeStats,
        link_length: f64,
        link_text_length: u32,
    }

    impl Accumulator<'_> {
        fn close_top(&mut self) {
            let mut finished = self.stack.pop().expect("source stats frame exists");
            if finished.tag == Some(Tag::A) {
                finished.link_text_length = finished.stats.text_length;
                finished.link_length = finished.stats.text_length as f64
                    * if self
                        .dom
                        .attr(finished.node, AttrName::Href)
                        .is_some_and(is_hash_url)
                    {
                        0.3
                    } else {
                        1.0
                    };
            }
            if let Some(parent) = self.stack.last_mut() {
                append_content_stats(&mut parent.stats, &finished.stats);
                parent.link_length += finished.link_length;
                parent.link_text_length = parent
                    .link_text_length
                    .saturating_add(finished.link_text_length);
            } else {
                self.result = finished.stats;
                self.link_length = finished.link_length;
                self.link_text_length = finished.link_text_length;
            }
        }

        fn observe(&mut self, entry: &SourceEntry, text: Option<NodeStats>) {
            if self.excluded_depth.is_some_and(|depth| entry.depth > depth) {
                return;
            }
            self.excluded_depth = None;
            if self
                .excluded
                .get(entry.node.index())
                .copied()
                .unwrap_or(false)
            {
                self.excluded_depth = Some(entry.depth);
                return;
            }
            while self
                .stack
                .last()
                .is_some_and(|frame| frame.depth >= entry.depth)
            {
                self.close_top();
            }
            if entry.is_element() {
                self.stack.push(Frame {
                    node: entry.node,
                    depth: entry.depth,
                    tag: entry.tag,
                    stats: NodeStats::default(),
                    link_length: 0.0,
                    link_text_length: 0,
                });
            } else if let Some(stats) = text {
                if let Some(parent) = self.stack.last_mut() {
                    append_content_stats(&mut parent.stats, &stats);
                } else {
                    append_content_stats(&mut self.result, &stats);
                }
            }
        }

        fn finish(mut self) -> (NodeStats, f64, usize) {
            while !self.stack.is_empty() {
                self.close_top();
            }
            (
                self.result,
                self.link_length,
                self.link_text_length as usize,
            )
        }
    }

    let mut normal = Accumulator {
        dom,
        excluded: normal_excluded,
        excluded_depth: None,
        stack: SmallVec::new(),
        result: NodeStats::default(),
        link_length: 0.0,
        link_text_length: 0,
    };
    let mut relaxed = relaxed_excluded.map(|excluded| Accumulator {
        dom,
        excluded,
        excluded_depth: None,
        stack: SmallVec::new(),
        result: NodeStats::default(),
        link_length: 0.0,
        link_text_length: 0,
    });
    for entry in entries {
        let text = source.text_stats_for_entry(entry);
        normal.observe(entry, text);
        if let Some(relaxed) = &mut relaxed {
            relaxed.observe(entry, text);
        }
    }
    (normal.finish(), relaxed.map(Accumulator::finish))
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
    get_or_compute_stats_excluding_impl(dom, None, id, store, excluded)
}

fn get_or_compute_stats_excluding_impl(
    dom: &Dom,
    source: Option<&SourceAnalysis>,
    id: NodeId,
    store: &mut NodeStateStore,
    excluded: &[bool],
) -> NodeStats {
    if let Some(s) = store.get_stats(id) {
        return *s;
    }

    // Most small content elements are linear wrapper chains around one text
    // leaf, such as p > span > text. Avoid a stack frame for every wrapper.
    let mut chain = SmallVec::<[NodeId; 8]>::new();
    if let Some(text) = dom.text_node(id) {
        let stats = source
            .and_then(|source| source.text_stats(id))
            .unwrap_or_else(|| stats_for_text(text));
        store.set_stats(id, stats);
        return stats;
    }
    let mut current = id;
    let leaf = loop {
        if let Some(text) = dom.text_node(current) {
            break Some((text, None));
        }
        let Some(child) = dom.first_child(current) else {
            break Some(("", None));
        };
        if dom.next_sibling(child).is_some()
            || excluded.get(child.index()).copied().unwrap_or(false)
        {
            break None;
        }
        chain.push(current);
        if let Some(text) = dom.text_node(child) {
            break Some((text, Some(child)));
        }
        current = child;
    };
    if let Some((text, child)) = leaf
        && !chain.is_empty()
    {
        let stats = child
            .and_then(|child| store.get_stats(child).copied())
            .or_else(|| child.and_then(|child| source.and_then(|source| source.text_stats(child))))
            .unwrap_or_else(|| stats_for_text(text));
        let cache_links = store.link_lengths_enabled();
        for &node in chain.iter().rev() {
            if cache_links {
                let link_length = if dom.tag(node) == Some(Tag::A) {
                    stats.text_length as f64
                        * if dom.attr(node, AttrName::Href).is_some_and(is_hash_url) {
                            0.3
                        } else {
                            1.0
                        }
                } else {
                    0.0
                };
                store.set_link_length(node, link_length);
            }
            store.set_stats(node, stats);
        }
        return stats;
    }

    struct StatsFrame {
        node: NodeId,
        next_child: Option<NodeId>,
        stats: NodeStats,
        link_length: f64,
    }

    impl StatsFrame {
        #[inline(always)]
        fn new(dom: &Dom, source: Option<&SourceAnalysis>, node: NodeId) -> Self {
            Self {
                node,
                next_child: dom.first_child(node),
                stats: match dom.text_node(node) {
                    Some(text) => source
                        .and_then(|source| source.text_stats(node))
                        .unwrap_or_else(|| stats_for_text(text)),
                    None => NodeStats::default(),
                },
                link_length: 0.0,
            }
        }
    }

    let cache_links = store.link_lengths_enabled();
    let mut stack = SmallVec::<[StatsFrame; 16]>::new();
    stack.push(StatsFrame::new(dom, source, id));
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
                stack.push(StatsFrame::new(dom, source, child));
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
    code_bytes: u32,
    tables: u32,
    non_empty_table_cells: u32,
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
        self.code_bytes = self.code_bytes.saturating_add(other.code_bytes);
        self.tables = self.tables.saturating_add(other.tables);
        self.non_empty_table_cells = self
            .non_empty_table_cells
            .saturating_add(other.non_empty_table_cells);
        self.figures = self.figures.saturating_add(other.figures);
        self.images = self.images.saturating_add(other.images);
        self.protected_blocks = self.protected_blocks.saturating_add(other.protected_blocks);
    }
}

impl CandidateFeatureIndex {
    pub(crate) fn new(
        dom: &Dom,
        store: &mut NodeStateStore,
        source: Option<&SourceAnalysis>,
        nodes: &[(NodeId, u32)],
        candidates: &CandidateSet,
        scoring_view: Option<&ScoringView>,
    ) -> Self {
        let candidate_nodes: Vec<_> = candidates.iter().map(|candidate| candidate.node).collect();
        let mut counts = vec![StructuralCounts::default(); candidate_nodes.len()];
        let mut candidate_parent = vec![None; candidate_nodes.len()];
        let mut active_candidates = Vec::<(u32, usize)>::new();
        let mut candidate_order = Vec::with_capacity(candidate_nodes.len());
        let mut has_links = false;
        let mut virtual_paragraphs = Vec::<(NodeId, u32)>::new();
        if let Some(scoring_view) = scoring_view {
            for seed in scoring_view.prepared_seeds() {
                let PreparedScoreSeed::Virtual { parent, .. } = *seed else {
                    continue;
                };
                virtual_paragraphs.push((parent, 1));
            }
            virtual_paragraphs.sort_unstable_by_key(|(node, _)| *node);
            let mut compacted = Vec::<(NodeId, u32)>::with_capacity(virtual_paragraphs.len());
            for (parent, count) in virtual_paragraphs.drain(..) {
                if let Some((last_parent, last_count)) = compacted.last_mut()
                    && *last_parent == parent
                {
                    *last_count = last_count.saturating_add(count);
                } else {
                    compacted.push((parent, count));
                }
            }
            virtual_paragraphs = compacted;
        }

        for &(node, depth) in nodes {
            let Some(tag) = scoring_view
                .and_then(|view| view.effective_tag(dom, node))
                .or_else(|| dom.tag(node))
            else {
                continue;
            };
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

            if let Ok(index) = virtual_paragraphs.binary_search_by_key(&node, |(parent, _)| *parent)
                && let Some(&(_, candidate_index)) = active_candidates.last()
            {
                counts[candidate_index].paragraphs = counts[candidate_index]
                    .paragraphs
                    .saturating_add(virtual_paragraphs[index].1);
            }

            let mut own = StructuralCounts::default();
            match tag {
                Tag::P => own.paragraphs = 1,
                Tag::H1 | Tag::H2 | Tag::H3 | Tag::H4 | Tag::H5 | Tag::H6 => own.headings = 1,
                Tag::Li => own.list_items = 1,
                Tag::Pre => {
                    own.code_blocks = 1;
                    own.code_bytes = source.and_then(|source| source.entry(node)).map_or_else(
                        || u32::try_from(dom.text(node).len()).unwrap_or(u32::MAX),
                        |entry| entry.subtree_text_bytes,
                    );
                }
                Tag::Table if store.is_data_table(node) == Some(true) => own.tables = 1,
                Tag::Td | Tag::Th
                    if get_or_compute_stats(dom, node, store).has_non_whitespace() =>
                {
                    own.non_empty_table_cells = 1;
                }
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

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn features(
        &self,
        dom: &Dom,
        source: Option<&SourceAnalysis>,
        candidate_index: usize,
        candidate: Candidate,
        store: &mut NodeStateStore,
        weight_classes: bool,
        excluded: &[bool],
    ) -> CandidateFeatures {
        let text = match source {
            Some(source) => get_or_compute_stats_from_source_excluding(
                dom,
                source,
                candidate.node,
                store,
                excluded,
            ),
            None => get_or_compute_stats_excluding(dom, candidate.node, store, excluded),
        };
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
            name_signals(dom, source, candidate.node)
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
            code_bytes: counts.code_bytes,
            table_count: counts.tables,
            non_empty_table_cell_count: counts.non_empty_table_cells,
            figure_count: counts.figures,
            image_count: u32::from(counts.images),
            link_text_chars,
            link_density,
            digit_chars: text.digit_chars,
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

        // Dashboard charts and metric cards often expose every value as a
        // paragraph-like wrapper. They can outscore a complete document on
        // readability even though they have no prose or section structure.
        // Treat this shape as weak document evidence. A real document with
        // headings, sentences, or tables remains unaffected.
        let numeric_metric_shape = self.digit_chars >= self.word_count / 5;
        let repeated_metric_shape = self.sentence_end_count
            >= self
                .paragraph_count
                .saturating_add(self.paragraph_count / 2);
        let repeated_metric_penalty = if self.paragraph_count >= 40
            && self.heading_count == 0
            && self.comma_count <= 2
            && self.word_count <= self.paragraph_count.saturating_mul(4)
            && self.digit_chars > 0
            && (numeric_metric_shape || repeated_metric_shape)
            && self.table_count == 0
            && self.code_block_count == 0
        {
            (f64::from(self.paragraph_count) * 2.0).min(160.0)
        } else {
            0.0
        };

        self.readability_score
            + self.semantic_prior
            + text_evidence
            + prose_evidence
            + structure_evidence
            + link_volume_evidence
            + name_evidence
            - link_penalty
            - repeated_metric_penalty
    }
}

fn name_signals(dom: &Dom, source: Option<&SourceAnalysis>, node: NodeId) -> (f64, f64) {
    fn signals_for_node(dom: &Dom, source: Option<&SourceAnalysis>, node: NodeId) -> (bool, bool) {
        if let Some(entry) = source.and_then(|source| source.entry(node)) {
            return (
                entry
                    .flags
                    .contains(crate::prepared::SourceFlags::POSITIVE_NAME),
                entry
                    .flags
                    .contains(crate::prepared::SourceFlags::NEGATIVE_NAME),
            );
        }
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
        positive |= role.is_some_and(|roles| has_any_token(roles, &["article", "main"]));
        negative |= role.is_some_and(is_unlikely_role);
        (positive, negative)
    }

    let (positive, mut negative) = signals_for_node(dom, source, node);
    for ancestor in dom.ancestors(node).take(3) {
        let (ancestor_positive, ancestor_negative) = signals_for_node(dom, source, ancestor);
        negative |= ancestor_negative && !ancestor_positive;
    }
    (f64::from(positive), f64::from(negative))
}

pub fn compute_initial_readability_data(dom: &Dom, id: NodeId, weight_classes: bool) -> f64 {
    compute_initial_readability_data_from_source(dom, None, id, weight_classes)
}

pub(crate) fn compute_initial_readability_data_from_source(
    dom: &Dom,
    source: Option<&SourceAnalysis>,
    id: NodeId,
    weight_classes: bool,
) -> f64 {
    let score = match dom.tag(id) {
        Some(Tag::Div) => 5.,
        Some(Tag::Pre | Tag::Td | Tag::Blockquote) => 3.,
        Some(
            Tag::Address | Tag::Ol | Tag::Ul | Tag::Dl | Tag::Dd | Tag::Dt | Tag::Li | Tag::Form,
        ) => -3.,
        Some(Tag::H1 | Tag::H2 | Tag::H3 | Tag::H4 | Tag::H5 | Tag::H6 | Tag::Th) => -5.,
        _ => 0.,
    };
    let class_weight = if weight_classes {
        source
            .and_then(|source| source.class_weight(id))
            .unwrap_or_else(|| get_class_weight(dom, id, true))
    } else {
        0
    };
    score + f64::from(class_weight)
}
pub fn initialize_node(dom: &Dom, id: NodeId, store: &mut NodeStateStore, weight_classes: bool) {
    store.initialize_if_absent(
        id,
        compute_initial_readability_data(dom, id, weight_classes),
    );
}

/// Prepares a scoring-only DOM and returns paragraphs created by the pass.
///
/// The caller must pass a selected fragment or a test-only source copy. This
/// function can wrap phrasing content, replace simple wrappers, and rename
/// leaf `div` elements.
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
#[cfg(test)]
pub(crate) fn compute_readability_scores(
    dom: &mut Dom,
    source: Option<&SourceAnalysis>,
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
        let stats = get_or_compute_stats_from_source(dom, source, node, store, &[]);
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
            let initial =
                compute_initial_readability_data_from_source(dom, source, node, weight_classes);
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
        let length = get_or_compute_stats_from_source(dom, source, node, store, &[]).text_length;
        let density = get_link_density_cached(dom, node, length, store);
        scores.push(ReadabilityScore {
            node,
            score: content_score * (1.0 - density),
        });
    }
    scores
}

pub(crate) fn compute_readability_scores_in_view(
    dom: &Dom,
    source: &SourceAnalysis,
    view: &ScoringView,
    to_score: impl IntoIterator<Item = NodeId>,
    excluded_mask: &[bool],
    store: &mut NodeStateStore,
    weight_classes: bool,
) -> SmallVec<[ReadabilityScore; 64]> {
    let mut discovered = SmallVec::<[NodeId; 256]>::new();
    view.seed_text_overrides(store);
    let mut score_seed = |parent: NodeId, stats: NodeStats, store: &mut NodeStateStore| {
        if stats.text_length < 25 {
            return;
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
            let score = initial_readability_for_tag(
                view.effective_tag(dom, node),
                source.class_weight(node),
                weight_classes,
            );
            if store.initialize_if_absent(node, score) {
                discovered.push(node);
            }
            let divisor = match level {
                0 => 1.0,
                1 => 2.0,
                _ => (level * 3) as f64,
            };
            store.add_content_score(node, content_score / divisor);
        }
    };

    for node in to_score {
        let Some(parent) = view
            .effective_parent(dom, node)
            .filter(|&parent| dom.is_element(parent))
        else {
            continue;
        };
        let stats = get_or_compute_stats_from_source(dom, Some(source), node, store, &[]);
        score_seed(parent, stats, store);
    }
    for seed in view.prepared_seeds() {
        match *seed {
            PreparedScoreSeed::Node { node, parent } => {
                if !store.mark_score_seen(node) {
                    continue;
                }
                let stats = get_or_compute_stats_from_source(dom, Some(source), node, store, &[]);
                score_seed(parent, stats, store);
            }
            PreparedScoreSeed::Virtual { parent, stats } => score_seed(parent, stats, store),
        }
    }

    // Paragraph propagation uses the unfiltered source, just as the mutable
    // implementation did before detach. Candidate metrics use the retained
    // scoring view, so discard only cached text and link facts now.
    store.clear_stats();
    view.seed_text_overrides(store);
    let mut scores = SmallVec::new();
    for node in discovered {
        if excluded_mask.get(node.index()).copied().unwrap_or(false) || view.ignores_wrapper(node) {
            continue;
        }
        let content_score = store.get_content_score(node);
        let length =
            get_or_compute_stats_from_source_excluding(dom, source, node, store, excluded_mask)
                .text_length;
        let density = get_link_density_cached(dom, node, length, store);
        scores.push(ReadabilityScore {
            node,
            score: content_score * (1.0 - density),
        });
    }
    scores
}

fn initial_readability_for_tag(
    tag: Option<Tag>,
    class_weight: Option<i32>,
    weight_classes: bool,
) -> f64 {
    let score = match tag {
        Some(Tag::Div) => 5.0,
        Some(Tag::Pre | Tag::Td | Tag::Blockquote) => 3.0,
        Some(
            Tag::Address | Tag::Ol | Tag::Ul | Tag::Dl | Tag::Dd | Tag::Dt | Tag::Li | Tag::Form,
        ) => -3.0,
        Some(Tag::H1 | Tag::H2 | Tag::H3 | Tag::H4 | Tag::H5 | Tag::H6 | Tag::Th) => -5.0,
        _ => 0.0,
    };
    score
        + if weight_classes {
            f64::from(class_weight.unwrap_or_default())
        } else {
            0.0
        }
}

/// Marks excluded roots and their descendants before the scoring tree is mutated.
///
/// A boolean index avoids repeatedly walking candidate ancestor chains. This is
/// important for malformed documents, where HTML tree repair can create deep
/// nesting and many candidates.
#[cfg(test)]
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

/// Builds an exclusion mask from prepared source intervals.
///
/// The normal source path converts each excluded root to one preorder range,
/// merges overlapping ranges, and fills the mask in one source-order pass.
/// Detached or synthetic roots use the legacy descendant walk as a cold
/// fallback because they have no entry in `SourceAnalysis`.
pub(crate) fn build_exclusion_mask_with_source(
    dom: &Dom,
    source: &SourceAnalysis,
    excluded: &[NodeId],
) -> Vec<bool> {
    if excluded.is_empty() {
        return Vec::new();
    }
    let mut mask = vec![false; dom.len()];
    let mut ranges = Vec::with_capacity(excluded.len());
    let mut detached = Vec::new();
    for &root in excluded {
        if let Some(range) = source.subtree_range(root) {
            ranges.push(range);
        } else {
            detached.push(root);
        }
    }
    ranges.sort_unstable_by_key(|range| (range.start, range.end));
    let mut merged: Vec<std::ops::Range<usize>> = Vec::with_capacity(ranges.len());
    for range in ranges {
        if let Some(last) = merged.last_mut()
            && range.start <= last.end
        {
            last.end = last.end.max(range.end);
        } else {
            merged.push(range);
        }
    }
    let mut range_index = 0;
    for (position, entry) in source.entries.iter().enumerate() {
        while range_index < merged.len() && position >= merged[range_index].end {
            range_index += 1;
        }
        if range_index < merged.len()
            && position >= merged[range_index].start
            && let Some(slot) = mask.get_mut(entry.node.index())
        {
            *slot = true;
        }
    }
    for root in detached {
        if let Some(slot) = mask.get_mut(root.index()) {
            *slot = true;
            for node in dom.descendants(root) {
                if let Some(slot) = mask.get_mut(node.index()) {
                    *slot = true;
                }
            }
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
    trim_text(out)
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
    trim_text(out)
}

/// Trims HTML text without invoking the Unicode whitespace iterator for the
/// overwhelmingly common ASCII source path.
#[inline]
pub(crate) fn trim_text(value: &str) -> &str {
    if value.is_ascii() {
        let bytes = value.as_bytes();
        let mut start = 0;
        while start < bytes.len()
            && (bytes[start] == b' ' || (b'\t'..=b'\r').contains(&bytes[start]))
        {
            start += 1;
        }
        let mut end = bytes.len();
        while end > start && (bytes[end - 1] == b' ' || (b'\t'..=b'\r').contains(&bytes[end - 1])) {
            end -= 1;
        }
        &value[start..end]
    } else {
        value.trim()
    }
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
    dom.text_node(id).is_some_and(|t| trim_text(t).is_empty()) || dom.tag(id) == Some(Tag::Br)
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
                    .is_some_and(|t| !trim_text(t).is_empty());
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
        let mut display_show = false;
        let mut visibility_show = false;
        let mut accessibility_show = false;
        let mut hidden = false;
        let mut invisible = false;
        let mut accessibility_hidden = false;
        for class in classes.split_ascii_whitespace() {
            display_show |= is_responsive_display_show(class);
            if let Some((variant, value)) = class.split_once(':')
                && is_responsive_breakpoint(variant)
            {
                visibility_show |= value.eq_ignore_ascii_case("visible");
                accessibility_show |= value.eq_ignore_ascii_case("not-sr-only");
            }
            hidden |= has_any_token(class, &["hidden", "d-none", "display-none", "u-hidden"]);
            invisible |= has_token(class, "invisible");
            accessibility_hidden |= has_any_token(class, &["visually-hidden", "sr-only"]);
        }
        hidden && !display_show
            || invisible && !visibility_show
            || accessibility_hidden && !accessibility_show
    })
}

fn is_responsive_breakpoint(value: &str) -> bool {
    ["sm", "md", "lg", "xl", "xxl", "2xl"]
        .iter()
        .any(|expected| value.eq_ignore_ascii_case(expected))
}

fn is_responsive_display_show(class: &str) -> bool {
    let tailwind = class.split_once(':').is_some_and(|(variant, display)| {
        is_responsive_breakpoint(variant) && is_visible_display_utility(display)
    });
    let bootstrap = class
        .split_once('-')
        .filter(|(prefix, _)| prefix.eq_ignore_ascii_case("d"))
        .and_then(|(_, class)| class.split_once('-'))
        .is_some_and(|(breakpoint, display)| {
            is_responsive_breakpoint(breakpoint) && is_visible_display_utility(display)
        });
    tailwind || bootstrap
}

fn is_visible_display_utility(value: &str) -> bool {
    [
        "block",
        "inline",
        "inline-block",
        "flex",
        "inline-flex",
        "grid",
        "inline-grid",
        "table",
        "contents",
    ]
    .iter()
    .any(|expected| value.eq_ignore_ascii_case(expected))
}

pub(crate) fn has_hidden_utility_class_for_discovery(dom: &Dom, id: NodeId) -> bool {
    // Hidden skip links and page anchors must not change candidate boundaries.
    if dom.tag(id) == Some(Tag::A) || !has_hidden_utility_class(dom, id) {
        return false;
    }
    let authoritative_root = matches!(dom.tag(id), Some(Tag::Article | Tag::Main))
        || dom
            .attr(id, AttrName::Role)
            .is_some_and(|roles| has_any_token(roles, &["article", "main"]));
    authoritative_root
        || dom.attr(id, AttrName::Class).is_some_and(|classes| {
            has_any_token(
                classes,
                &["invisible", "d-none", "display-none", "u-hidden"],
            )
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
    if value.eq_ignore_ascii_case("none") {
        return Some(true);
    }
    [
        "initial",
        "inherit",
        "unset",
        "revert",
        "revert-layer",
        "block",
        "inline",
        "inline-block",
        "flow-root",
        "run-in",
        "list-item",
        "flex",
        "inline-flex",
        "grid",
        "inline-grid",
        "table",
        "inline-table",
        "table-row",
        "table-cell",
        "table-caption",
        "table-row-group",
        "table-header-group",
        "table-footer-group",
        "table-column",
        "table-column-group",
        "contents",
        "ruby",
        "ruby-base",
        "ruby-text",
        "ruby-base-container",
        "ruby-text-container",
        "-webkit-box",
    ]
    .iter()
    .any(|expected| value.eq_ignore_ascii_case(expected))
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
    #[test]
    fn hybrid_matches_reference_implementation() {
        // Reference: straightforward per-char implementation.
        fn reference(
            text: &str,
        ) -> (
            usize,
            usize,
            usize,
            usize,
            bool,
            bool,
            bool,
            bool,
            bool,
            u32,
            u32,
        ) {
            let mut prev = true;
            let mut dot = false;
            let mut text_length = 0usize;
            let mut word_count = 0usize;
            let mut comma_count = 0usize;
            let mut sentence_end_count = 0usize;
            let mut has_non_whitespace = false;
            let mut has_alphanumeric = false;
            let mut has_sentence_break = false;
            let mut alphabetic_chars = 0u32;
            let mut digit_chars = 0u32;
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
                    alphabetic_chars =
                        alphabetic_chars.saturating_add(u32::from(c.is_alphabetic()));
                    digit_chars = digit_chars.saturating_add(u32::from(c.is_numeric()));
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
            if prev && text_length > 0 {
                text_length -= 1
            }
            (
                text_length,
                word_count,
                comma_count,
                sentence_end_count,
                has_non_whitespace,
                has_alphanumeric,
                has_sentence_break,
                dot,
                has_sentence_break || dot,
                alphabetic_chars,
                digit_chars,
            )
        }

        let samples = [
            "AI hasn\u{2019}t meaningfully changed anything in cybersecurity so far. Deep fake phishing is still rare, L",
            "café naïve 日本語 text with mixed 漢字 content!",
            "\u{3000}甲， 乙.\u{a0}",
            "plain ascii words. two sentences!",
            "tabs\tand\nnewlines\r\nmixed   spaces ",
            "no trailing newline",
            "trailing space ",
            " leading tab\tthen words, more.",
            "emoji \u{1F600} and accents éàü",
        ];
        for sample in samples {
            let s = stats_for_text(sample);
            let r = reference(sample);
            assert_eq!(s.text_length as usize, r.0, "text_length {sample:?}");
            assert_eq!(s.word_count as usize, r.1, "word_count {sample:?}");
            assert_eq!(s.comma_count as usize, r.2, "comma_count {sample:?}");
            assert_eq!(
                s.sentence_end_count as usize, r.3,
                "sentence_end {sample:?}"
            );
            assert_eq!(s.has_non_whitespace(), r.4, "has_non_ws {sample:?}");
            assert_eq!(s.has_alphanumeric(), r.5, "has_alnum {sample:?}");
            assert_eq!(s.has_sentence_break(), r.6, "sentence_break {sample:?}");
            assert_eq!(s.ends_with_dot(), r.7, "ends_dot {sample:?}");
            assert_eq!(s.has_sentence_end(), r.8, "sentence_end_flag {sample:?}");
            assert_eq!(s.alphabetic_chars, r.9, "alpha_chars {sample:?}");
            assert_eq!(s.digit_chars, r.10, "digit_chars {sample:?}");
        }
    }

    use super::{
        CandidateFeatureIndex, PreparedScoreSeed, ScoringView, build_exclusion_mask,
        compute_readability_scores, compute_readability_scores_in_view, get_link_density,
        get_link_density_cached, get_or_compute_stats, has_hidden_utility_class,
        has_hidden_utility_class_for_discovery, is_probably_visible, prepare_readability_structure,
        stats_for_text,
    };
    use crate::candidate::CandidateSet;
    use crate::dom::{AttrName, Dom, NodeStateStore, Tag};
    use crate::prepared::SourceAnalysis;

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
        let index =
            CandidateFeatureIndex::new(&dom, &mut store, None, &snapshot, &candidates, None);
        index.prepare_text_cache(&mut store);
        let features = index.features(
            &dom,
            None,
            candidate_index,
            candidate,
            &mut store,
            true,
            &[],
        );

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

        let unweighted = index.features(
            &dom,
            None,
            candidate_index,
            candidate,
            &mut store,
            false,
            &[],
        );
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
        let index =
            CandidateFeatureIndex::new(&dom, &mut store, None, &snapshot, &candidates, None);
        let features = index.features(
            &dom,
            None,
            candidate_index,
            candidate,
            &mut store,
            false,
            &[],
        );

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
        let index =
            CandidateFeatureIndex::new(&dom, &mut store, None, &snapshot, &candidates, None);

        assert_eq!(index.counts.len(), candidates.iter().count());
        let outer_features = index.features(
            &dom,
            None,
            candidates.index_of(outer).unwrap(),
            *candidates.get(outer).unwrap(),
            &mut store,
            false,
            &[],
        );
        let inner_features = index.features(
            &dom,
            None,
            candidates.index_of(inner).unwrap(),
            *candidates.get(inner).unwrap(),
            &mut store,
            false,
            &[],
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
    fn repeated_metric_shape_does_not_penalize_long_prose() {
        let prose = crate::candidate::CandidateFeatures {
            text_chars: 1_200,
            word_count: 400,
            paragraph_count: 40,
            comma_count: 18,
            sentence_end_count: 20,
            ..Default::default()
        };
        let metrics = crate::candidate::CandidateFeatures {
            text_chars: 400,
            word_count: 80,
            paragraph_count: 40,
            digit_chars: 24,
            ..Default::default()
        };

        assert!(metrics.ranking_score() + 20.0 < prose.ranking_score());
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

        let scores =
            compute_readability_scores(&mut dom, None, paragraphs, &[], &[], &mut store, true);

        let article_score = scores
            .iter()
            .find(|candidate| candidate.node == article)
            .map(|candidate| candidate.score)
            .unwrap();
        assert!(article_score > 2.0);
    }

    #[test]
    fn immutable_view_matches_div_preparation_scores() {
        let source_dom = Dom::parse_document(
            r#"<body><main><div>Long inline prose contains enough detail, commas, and useful explanation <span>for stable scoring.</span></div><div><p>A second paragraph contains enough useful words, punctuation, and detail for propagation.</p></div></main></body>"#,
        )
        .unwrap();
        let source = SourceAnalysis::build(&source_dom);
        let candidates = CandidateSet::discover_semantic(&source_dom);
        let divs: Vec<_> = source_dom
            .descendants(source_dom.root())
            .filter(|&node| source_dom.tag(node) == Some(Tag::Div))
            .collect();
        let paragraphs: Vec<_> = source_dom
            .descendants(source_dom.root())
            .filter(|&node| source_dom.tag(node) == Some(Tag::P))
            .collect();

        let view = ScoringView::build(&source_dom, &source, &divs, &candidates);
        let mut view_store = NodeStateStore::new();
        for &paragraph in &paragraphs {
            view_store.mark_score_seen(paragraph);
        }
        let view_scores = compute_readability_scores_in_view(
            &source_dom,
            &source,
            &view,
            paragraphs.clone(),
            &[],
            &mut view_store,
            true,
        );

        let original_len = source_dom.len();
        let mut legacy = source_dom.clone();
        let prepared = prepare_readability_structure(&mut legacy, &divs, &candidates);
        let mut legacy_store = NodeStateStore::new();
        let mut legacy_seeds = paragraphs;
        legacy_seeds.extend(prepared);
        legacy_seeds.sort_unstable();
        legacy_seeds.dedup();
        let legacy_scores = compute_readability_scores(
            &mut legacy,
            None,
            legacy_seeds,
            &[],
            &[],
            &mut legacy_store,
            true,
        );

        assert_eq!(source_dom.len(), original_len);
        for expected in legacy_scores {
            let actual = view_scores
                .iter()
                .find(|score| score.node == expected.node)
                .expect("view preserves each legacy candidate");
            assert!(
                (actual.score - expected.score).abs() < f64::EPSILON,
                "node {:?}: view={}, legacy={}",
                expected.node,
                actual.score,
                expected.score
            );
        }
    }

    #[test]
    fn aria_projections_bypass_div_paragraph_preparation() {
        let dom = Dom::parse_document(
            r#"<body><main><div role="heading" aria-level="2">Heading</div><div role="list"><div role="listitem">First item</div><div role="listitem">Second item</div></div></main></body>"#,
        )
        .unwrap();
        let source = SourceAnalysis::build(&dom);
        let candidates = CandidateSet::discover_semantic(&dom);
        let divs: Vec<_> = dom
            .descendants(dom.root())
            .filter(|&node| dom.tag(node) == Some(Tag::Div))
            .collect();
        let view = ScoringView::build(&dom, &source, &divs, &candidates);
        let heading = divs[0];
        let list = divs[1];

        assert_eq!(view.effective_tag(&dom, heading), Some(Tag::H2));
        assert_eq!(view.effective_tag(&dom, list), Some(Tag::Ul));
        assert!(view.prepared_seeds().is_empty());
        assert!(divs.iter().all(|&node| dom.tag(node) == Some(Tag::Div)));
    }

    #[test]
    fn ignored_wrapper_projects_parent_and_sibling_topology() {
        let dom = Dom::parse_document(
            r#"<body><div><p>Wrapped paragraph contains enough useful prose for scoring.</p></div><p>Adjacent paragraph remains a sibling.</p></body>"#,
        )
        .unwrap();
        let source = SourceAnalysis::build(&dom);
        let candidates = CandidateSet::discover_semantic(&dom);
        let wrapper = dom.first_descendant_by_tag(dom.root(), Tag::Div).unwrap();
        let wrapped = dom.first_descendant_by_tag(wrapper, Tag::P).unwrap();
        let body = dom.body().unwrap();
        let view = ScoringView::build(&dom, &source, &[wrapper], &candidates);

        assert_eq!(view.effective_parent(&dom, wrapped), Some(body));
        let children = view.effective_element_children(&dom, body);
        assert_eq!(children.first(), Some(&wrapped));
        assert_eq!(children.len(), 2);
    }

    #[test]
    fn virtual_paragraph_removes_its_source_wrapper_from_topology() {
        let dom = Dom::parse_document(
            r#"<body><div>Useful phrasing <strong>continues here</strong>.</div><p>Adjacent paragraph remains visible.</p></body>"#,
        )
        .unwrap();
        let source = SourceAnalysis::build(&dom);
        let candidates = CandidateSet::discover_semantic(&dom);
        let wrapper = dom.first_descendant_by_tag(dom.root(), Tag::Div).unwrap();
        let strong = dom.first_descendant_by_tag(wrapper, Tag::Strong).unwrap();
        let body = dom.body().unwrap();
        let view = ScoringView::build(&dom, &source, &[wrapper], &candidates);

        assert!(view.ignores_wrapper(wrapper));
        assert_eq!(view.effective_parent(&dom, strong), Some(body));
        assert!(
            !view
                .effective_element_children(&dom, body)
                .contains(&wrapper)
        );
        assert!(view.prepared_seeds().iter().any(|seed| {
            matches!(seed, PreparedScoreSeed::Virtual { parent, .. } if *parent == body)
        }));
    }

    #[test]
    fn ordered_aria_list_projects_marker_text_stats() {
        let dom = Dom::parse_document(
            r#"<body><main><div role="list"><div role="listitem">3. Three</div><div role="listitem">4. Four</div></div></main></body>"#,
        )
        .unwrap();
        let source = SourceAnalysis::build(&dom);
        let candidates = CandidateSet::discover_semantic(&dom);
        let divs: Vec<_> = dom
            .descendants(dom.root())
            .filter(|&node| dom.tag(node) == Some(Tag::Div))
            .collect();
        let list = divs[0];
        let first_item = divs[1];
        let view = ScoringView::build(&dom, &source, &divs, &candidates);
        let mut view_store = NodeStateStore::new();
        view.seed_text_overrides(&mut view_store);
        let view_stats = get_or_compute_stats(&dom, first_item, &mut view_store);

        let mut legacy = dom.clone();
        crate::normalize::materialize_scoring_structure(&mut legacy);
        let mut legacy_store = NodeStateStore::new();
        let legacy_stats = get_or_compute_stats(&legacy, first_item, &mut legacy_store);

        assert_eq!(view.effective_tag(&dom, list), Some(Tag::Ol));
        assert_eq!(view_stats.text_length, legacy_stats.text_length);
        assert_eq!(view_stats.word_count, legacy_stats.word_count);
        assert_eq!(view_stats.alphabetic_chars, legacy_stats.alphabetic_chars);
        assert_eq!(view_stats.digit_chars, legacy_stats.digit_chars);
        assert_eq!(
            dom.text_node(dom.first_child(first_item).unwrap()),
            Some("3. Three")
        );
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
            None,
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
    fn source_interval_exclusion_matches_descendant_walk() {
        let dom = Dom::parse_document(
            r#"<body><main><div id="outer"><p>Keep</p><div id="inner"><p>Drop</p></div><p>Keep too</p></div><aside><p>Other</p></aside></main></body>"#,
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
        let source = SourceAnalysis::build(&dom);
        let legacy = build_exclusion_mask(&dom, &[outer, inner]);
        let indexed = super::build_exclusion_mask_with_source(&dom, &source, &[outer, inner]);
        assert_eq!(indexed, legacy);
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
