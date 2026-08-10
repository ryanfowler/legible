//! Content scoring and DOM text helpers.
use crate::candidate::{Candidate, CandidateFeatures, CandidateSet};
use crate::constants::{
    flags::*, has_byline, is_div_to_p_elem, is_phrasing_elem, is_unlikely_role, regexps,
};
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
fn stats_for_text(text: &str) -> NodeStats {
    let mut s = NodeStats {
        has_text: !text.is_empty(),
        starts_with_whitespace: text.starts_with(char::is_whitespace),
        ends_with_whitespace: text.ends_with(char::is_whitespace),
        ..Default::default()
    };
    let mut prev = true;
    let mut dot = false;
    let mut text_length = 0usize;
    let mut word_count = 0usize;
    let mut comma_count = 0usize;
    let mut sentence_end_count = 0usize;

    // Most article text is ASCII. Scan bytes to avoid UTF-8 decoding and
    // Unicode whitespace tables in the hot loop.
    if text.is_ascii() {
        for &byte in text.as_bytes() {
            if byte.is_ascii_whitespace() {
                s.has_sentence_break |= dot;
                dot = false;
                if !prev {
                    text_length += 1;
                    prev = true
                }
            } else {
                s.has_non_whitespace = true;
                word_count += usize::from(prev);
                dot = byte == b'.';
                comma_count += usize::from(byte == b',');
                sentence_end_count += usize::from(matches!(byte, b'.' | b'!' | b'?'));
                text_length += 1;
                prev = false
            }
        }
    } else {
        for c in text.chars() {
            if c.is_whitespace() {
                s.has_sentence_break |= dot;
                dot = false;
                if !prev {
                    text_length += 1;
                    prev = true
                }
            } else {
                s.has_non_whitespace = true;
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
    s.ends_with_dot = dot;
    s.has_sentence_end = s.has_sentence_break || dot;
    s
}
fn append_stats(a: &mut NodeStats, b: &NodeStats) {
    if !b.has_text {
        return;
    }
    if !a.has_text {
        *a = *b;
        return;
    }
    a.has_sentence_break |= b.has_sentence_break || (a.ends_with_dot && b.starts_with_whitespace);
    if a.has_non_whitespace
        && b.has_non_whitespace
        && (a.ends_with_whitespace || b.starts_with_whitespace)
    {
        a.text_length = a.text_length.saturating_add(1)
    }
    a.text_length = a.text_length.saturating_add(b.text_length);
    a.word_count = a.word_count.saturating_add(b.word_count);
    if a.has_non_whitespace
        && b.has_non_whitespace
        && !a.ends_with_whitespace
        && !b.starts_with_whitespace
    {
        a.word_count = a.word_count.saturating_sub(1);
    }
    a.comma_count = a.comma_count.saturating_add(b.comma_count);
    a.sentence_end_count = a.sentence_end_count.saturating_add(b.sentence_end_count);
    a.has_non_whitespace |= b.has_non_whitespace;
    a.ends_with_whitespace = b.ends_with_whitespace;
    a.ends_with_dot = b.ends_with_dot;
    a.has_sentence_end = a.has_sentence_break || a.ends_with_dot
}
pub fn get_or_compute_stats(dom: &Dom, id: NodeId, store: &mut NodeStateStore) -> NodeStats {
    if let Some(s) = store.get_stats(id) {
        return *s;
    }

    let mut stack = SmallVec::<[(NodeId, bool); 16]>::new();
    stack.push((id, false));
    while let Some((n, expanded)) = stack.pop() {
        if store.get_stats(n).is_some() {
            continue;
        }
        if !expanded {
            stack.push((n, true));
            for c in dom.children_rev(n) {
                if store.get_stats(c).is_none() {
                    stack.push((c, false))
                }
            }
            continue;
        }
        let mut s = match dom.text_node(n) {
            Some(t) => stats_for_text(t),
            None => NodeStats::default(),
        };
        let cache_links = store.link_lengths_enabled();
        let mut link_length = 0.0;
        for c in dom.children(n) {
            if let Some(cs) = store.get_stats(c) {
                append_stats(&mut s, cs)
            }
            if cache_links {
                link_length += store.link_length(c);
            }
        }
        if cache_links {
            if dom.tag(n) == Some(Tag::A) {
                link_length = s.text_length as f64
                    * if dom.attr(n, AttrName::Href).is_some_and(is_hash_url) {
                        0.3
                    } else {
                        1.0
                    };
            }
            store.set_link_length(n, link_length);
        }
        s.has_sentence_end = s.has_sentence_break || s.ends_with_dot;
        store.set_stats(n, s)
    }
    store.get_stats(id).copied().unwrap_or_default()
}
/// Structural counts for all attached subtrees.
///
/// This index makes feature calculation linear in DOM size, even when many
/// nested nodes become candidates.
pub(crate) struct CandidateFeatureIndex {
    counts: Vec<StructuralCounts>,
    has_links: bool,
}

#[derive(Clone, Copy, Debug, Default)]
struct StructuralCounts {
    paragraphs: u32,
    headings: u32,
    list_items: u32,
    code_blocks: u32,
    tables: u32,
    figures: u32,
    images: u32,
    protected_blocks: u32,
}

impl StructuralCounts {
    fn add(&mut self, other: Self) {
        self.paragraphs = self.paragraphs.saturating_add(other.paragraphs);
        self.headings = self.headings.saturating_add(other.headings);
        self.list_items = self.list_items.saturating_add(other.list_items);
        self.code_blocks = self.code_blocks.saturating_add(other.code_blocks);
        self.tables = self.tables.saturating_add(other.tables);
        self.figures = self.figures.saturating_add(other.figures);
        self.images = self.images.saturating_add(other.images);
        self.protected_blocks = self.protected_blocks.saturating_add(other.protected_blocks);
    }
}

impl CandidateFeatureIndex {
    pub(crate) fn new(dom: &Dom, store: &NodeStateStore) -> Self {
        let mut counts = vec![StructuralCounts::default(); dom.len()];
        let mut nodes = Vec::with_capacity(dom.len());
        nodes.push(dom.root());
        nodes.extend(dom.descendants(dom.root()));
        let mut has_links = false;

        for &node in &nodes {
            let Some(tag) = dom.tag(node) else { continue };
            let own = &mut counts[node.index()];
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
                Tag::A => has_links = true,
                _ => {}
            }
        }
        for &node in nodes.iter().rev() {
            if let Some(parent) = dom.parent(node) {
                let child = counts[node.index()];
                counts[parent.index()].add(child);
            }
        }
        Self { counts, has_links }
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
        candidate: Candidate,
        store: &mut NodeStateStore,
        flags: u32,
    ) -> CandidateFeatures {
        let text = get_or_compute_stats(dom, candidate.node, store);
        let counts = self.counts[candidate.node.index()];
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
        let (positive_name_score, negative_name_score) = if flags & FLAG_WEIGHT_CLASSES != 0 {
            name_signals(dom, candidate.node)
        } else {
            (0.0, 0.0)
        };

        CandidateFeatures {
            text_chars: text.text_length,
            word_count: text.word_count,
            paragraph_count: counts.paragraphs,
            heading_count: counts.headings,
            list_item_count: counts.list_items,
            code_block_count: counts.code_blocks,
            table_count: counts.tables,
            figure_count: counts.figures,
            image_count: counts.images,
            link_text_chars,
            link_density,
            sentence_end_count: text.sentence_end_count,
            comma_count: text.comma_count,
            protected_block_count: counts.protected_blocks,
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

pub fn compute_initial_readability_data(dom: &Dom, id: NodeId, flags: u32) -> f64 {
    let score = match dom.tag(id) {
        Some(Tag::Div) => 5.,
        Some(Tag::Pre | Tag::Td | Tag::Blockquote) => 3.,
        Some(
            Tag::Address | Tag::Ol | Tag::Ul | Tag::Dl | Tag::Dd | Tag::Dt | Tag::Li | Tag::Form,
        ) => -3.,
        Some(Tag::H1 | Tag::H2 | Tag::H3 | Tag::H4 | Tag::H5 | Tag::H6 | Tag::Th) => -5.,
        _ => 0.,
    };
    score + get_class_weight(dom, id, flags) as f64
}
pub fn initialize_node(dom: &Dom, id: NodeId, store: &mut NodeStateStore, flags: u32) {
    store.initialize_if_absent(id, compute_initial_readability_data(dom, id, flags));
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
    let mut to_score = SmallVec::new();
    for &id in divs {
        if dom.parent(id).is_none() {
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
/// `scoring_dom` contains the scoring-only structural preparation.
/// `density_dom` is the same tree with deferred clutter detached. Stable node
/// IDs let this function reuse one [`NodeStateStore`] for both views.
pub(crate) fn compute_readability_scores(
    scoring_dom: &Dom,
    density_dom: &Dom,
    to_score: impl IntoIterator<Item = NodeId>,
    excluded: &[NodeId],
    store: &mut NodeStateStore,
    flags: u32,
) -> SmallVec<[ReadabilityScore; 64]> {
    let mut discovered = SmallVec::<[NodeId; 256]>::new();
    for node in to_score {
        let Some(parent) = scoring_dom
            .parent(node)
            .filter(|&parent| scoring_dom.is_element(parent))
        else {
            continue;
        };
        let stats = get_or_compute_stats(scoring_dom, node, store);
        if stats.text_length < 25 {
            continue;
        }
        let content_score =
            2.0 + f64::from(stats.comma_count) + f64::from((stats.text_length / 100).min(3));
        let mut ancestor = Some(parent);
        for level in 0..5 {
            let Some(node) = ancestor else { break };
            ancestor = scoring_dom.parent(node);
            if !scoring_dom.is_element(node)
                || !ancestor.is_some_and(|parent| scoring_dom.is_element(parent))
            {
                continue;
            }
            let initial = compute_initial_readability_data(scoring_dom, node, flags);
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

    // Structural preparation and deferred cleanup produce different text and
    // link totals. Keep propagated scores, but recompute cached statistics.
    store.clear_stats();
    let mut scores = SmallVec::new();
    for node in discovered {
        if is_excluded_candidate(density_dom, node, excluded) {
            continue;
        }
        let content_score = store.get_content_score(node);
        let length = get_or_compute_stats(density_dom, node, store).text_length;
        let density = get_link_density_cached(density_dom, node, length, store);
        scores.push(ReadabilityScore {
            node,
            score: content_score * (1.0 - density),
        });
    }
    scores
}

fn is_excluded_candidate(dom: &Dom, node: NodeId, excluded: &[NodeId]) -> bool {
    excluded.contains(&node)
        || dom
            .ancestors(node)
            .any(|ancestor| excluded.contains(&ancestor))
}
pub fn get_class_weight(dom: &Dom, id: NodeId, flags: u32) -> i32 {
    if flags & FLAG_WEIGHT_CLASSES == 0 {
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
    if dom.attr(id, AttrName::Style).is_some_and(has_hidden_style)
        || dom.has_attr(id, AttrName::Hidden)
    {
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
pub fn is_valid_byline(dom: &Dom, id: NodeId, ms: &str, text_buffer: &mut String) -> bool {
    let ok = dom.attr(id, AttrName::Rel) == Some("author")
        || dom
            .attr(id, AttrName::ItemProp)
            .is_some_and(|x| x.contains("author"))
        || has_byline(ms);
    if !ok {
        return false;
    }
    let t = get_inner_text(dom, id, text_buffer);
    !t.is_empty() && t.len() < 400 && t.chars().count() < 100
}
fn has_hidden_style(style: &str) -> bool {
    let style = style.as_bytes();
    (0..style.len()).any(|start| {
        matches_style_declaration(style, start, b"display", b"none")
            || matches_style_declaration(style, start, b"visibility", b"hidden")
    })
}

fn matches_style_declaration(style: &[u8], start: usize, property: &[u8], value: &[u8]) -> bool {
    let property_end = start + property.len();
    if property_end > style.len() || !style[start..property_end].eq_ignore_ascii_case(property) {
        return false;
    }
    let mut cursor = property_end;
    while style.get(cursor).is_some_and(u8::is_ascii_whitespace) {
        cursor += 1;
    }
    if style.get(cursor) != Some(&b':') {
        return false;
    }
    cursor += 1;
    while style.get(cursor).is_some_and(u8::is_ascii_whitespace) {
        cursor += 1;
    }
    let value_end = cursor + value.len();
    value_end <= style.len() && style[cursor..value_end].eq_ignore_ascii_case(value)
}

#[cfg(test)]
mod tests {
    use super::{
        CandidateFeatureIndex, compute_readability_scores, get_link_density,
        get_link_density_cached, stats_for_text,
    };
    use crate::candidate::CandidateSet;
    use crate::constants::flags::{FLAG_CLEAN_CONDITIONALLY, FLAG_WEIGHT_CLASSES};
    use crate::dom::{Dom, NodeStateStore, Tag};

    #[test]
    fn text_stats_match_for_ascii_and_unicode_paths() {
        let ascii = stats_for_text(" a,\t b. c ");
        assert_eq!(ascii.text_length, 7);
        assert_eq!(ascii.word_count, 3);
        assert_eq!(ascii.comma_count, 1);
        assert_eq!(ascii.sentence_end_count, 1);
        assert!(ascii.starts_with_whitespace);
        assert!(ascii.ends_with_whitespace);
        assert!(ascii.has_sentence_break);
        assert!(ascii.has_sentence_end);

        let unicode = stats_for_text("\u{3000}甲， 乙.\u{a0}");
        assert_eq!(unicode.text_length, 5);
        assert_eq!(unicode.word_count, 2);
        assert_eq!(unicode.comma_count, 1);
        assert_eq!(unicode.sentence_end_count, 1);
        assert!(unicode.starts_with_whitespace);
        assert!(unicode.ends_with_whitespace);
        assert!(unicode.has_sentence_end);
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
        let mut store = NodeStateStore::new();
        let mut table_nodes = Vec::new();
        crate::cleaning::mark_data_tables(&dom, dom.root(), &mut store, &mut table_nodes);
        let index = CandidateFeatureIndex::new(&dom, &store);
        index.prepare_text_cache(&mut store);
        let features = index.features(&dom, candidate, &mut store, FLAG_WEIGHT_CLASSES);

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

        let unweighted = index.features(&dom, candidate, &mut store, 0);
        assert_eq!(unweighted.positive_name_score, 0.0);
        assert_eq!(unweighted.negative_name_score, 0.0);
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
        let dom = Dom::parse_document(
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
        let flags = FLAG_WEIGHT_CLASSES | FLAG_CLEAN_CONDITIONALLY;

        let scores = compute_readability_scores(&dom, &dom, paragraphs, &[], &mut store, flags);

        let article_score = scores
            .iter()
            .find(|candidate| candidate.node == article)
            .map(|candidate| candidate.score)
            .unwrap();
        assert!(article_score > 2.0);
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
