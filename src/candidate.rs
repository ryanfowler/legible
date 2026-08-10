//! Internal content-root candidates.

use crate::constants::split_word_tokens;
use crate::dom::{AttrName, Dom, NodeId, Tag};
use smallvec::SmallVec;
use std::collections::HashSet;

const STRONG_IDS: &[&str] = &["post", "content", "article-content"];
const ARTICLE_TAG_PRIOR: f64 = 0.003;
const MAIN_TAG_PRIOR: f64 = 0.0025;
const ARTICLE_ROLE_PRIOR: f64 = 0.00275;
const OTHER_SEMANTIC_PRIOR: f64 = 0.0025;
const ADDITIONAL_SIGNAL_BONUS: f64 = 0.0005;
const MAX_SEMANTIC_PRIOR: f64 = 0.004;
const STRONG_CLASSES: &[&str] = &[
    "post-content",
    "post-body",
    "article-content",
    "article-body",
    "entry-content",
    "content-article",
    "markdown-body",
    "post",
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CandidateSource {
    Semantic,
    Readability,
    Generic,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct CandidateSources(u8);

impl CandidateSources {
    fn insert(&mut self, source: CandidateSource) {
        self.0 |= match source {
            CandidateSource::Semantic => 1 << 0,
            CandidateSource::Readability => 1 << 1,
            CandidateSource::Generic => 1 << 2,
        };
    }

    fn contains(self, source: CandidateSource) -> bool {
        let mut source_only = Self::default();
        source_only.insert(source);
        self.0 & source_only.0 != 0
    }
}

/// Content evidence calculated for one possible extraction root.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct CandidateFeatures {
    pub(crate) text_chars: u32,
    pub(crate) word_count: u32,
    pub(crate) paragraph_count: u32,
    pub(crate) heading_count: u32,
    pub(crate) list_item_count: u32,
    pub(crate) code_block_count: u32,
    pub(crate) table_count: u32,
    pub(crate) figure_count: u32,
    pub(crate) image_count: u32,
    pub(crate) link_text_chars: f64,
    pub(crate) link_density: f64,
    pub(crate) sentence_end_count: u32,
    pub(crate) comma_count: u32,
    pub(crate) protected_block_count: u32,
    pub(crate) readability_score: f64,
    pub(crate) semantic_prior: f64,
    pub(crate) positive_name_score: f64,
    pub(crate) negative_name_score: f64,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Candidate {
    pub(crate) node: NodeId,
    sources: CandidateSources,
    pub(crate) semantic_prior: f64,
    pub(crate) readability_score: f64,
    pub(crate) features: CandidateFeatures,
}

/// One candidate after general feature ranking.
#[derive(Clone, Copy, Debug)]
pub(crate) struct RankedCandidate {
    pub(crate) node: NodeId,
    pub(crate) score: f64,
    pub(crate) order: usize,
}

impl Candidate {
    pub(crate) fn has_source(self, source: CandidateSource) -> bool {
        self.sources.contains(source)
    }
}

/// A deduplicated candidate collection indexed by stable DOM node ID.
#[derive(Debug)]
pub(crate) struct CandidateSet {
    candidates: Vec<Candidate>,
    positions: Vec<usize>,
}

pub(crate) struct CandidateContext {
    readability_in_subtree: Vec<bool>,
    has_authoritative_ancestor: Vec<bool>,
    authoritative_count: Vec<u32>,
    article_peer_count: Vec<u32>,
    article_peer_score: Vec<f64>,
}

impl CandidateContext {
    pub(crate) fn has_readability(&self, node: NodeId) -> bool {
        self.readability_in_subtree[node.index()]
    }

    pub(crate) fn has_authoritative_ancestor(&self, node: NodeId) -> bool {
        self.has_authoritative_ancestor[node.index()]
    }

    pub(crate) fn has_authoritative_descendant(&self, node: NodeId, own: bool) -> bool {
        self.authoritative_count[node.index()] > u32::from(own)
    }

    pub(crate) fn article_peer_summary(&self, node: NodeId) -> (u32, f64) {
        (
            self.article_peer_count[node.index()],
            self.article_peer_score[node.index()],
        )
    }
}

impl CandidateSet {
    pub(crate) fn discover_semantic(dom: &Dom) -> Self {
        let mut candidates = Self {
            candidates: Vec::new(),
            positions: vec![usize::MAX; dom.len()],
        };

        if let Some(body) = dom.body() {
            candidates.add(body, CandidateSource::Generic, 0.0);
        }

        let mut generic_clutter_depth = None;
        for (node, depth) in dom.element_descendants_snapshot_with_depth(dom.root()) {
            if generic_clutter_depth.is_some_and(|root_depth| depth <= root_depth) {
                generic_clutter_depth = None;
            }
            let tag = dom.tag(node);
            let in_generic_clutter = generic_clutter_depth.is_some();
            if !in_generic_clutter && is_generic_clutter_container(dom, node) {
                generic_clutter_depth = Some(depth);
            }

            let tag_prior = match tag {
                Some(Tag::Article) => Some(ARTICLE_TAG_PRIOR),
                Some(Tag::Main) => Some(MAIN_TAG_PRIOR),
                _ => None,
            };
            if let Some(prior) = tag_prior {
                candidates.add(node, CandidateSource::Semantic, prior);
            }

            if !in_generic_clutter
                && generic_clutter_depth != Some(depth)
                && is_generic_candidate(dom, node, tag)
            {
                candidates.add(node, CandidateSource::Generic, 0.0);
            }

            if let Some(role) = dom.attr(node, AttrName::Role) {
                if matches_role(role, "article") {
                    candidates.add(node, CandidateSource::Semantic, ARTICLE_ROLE_PRIOR);
                }
                if matches_role(role, "main") {
                    candidates.add(node, CandidateSource::Semantic, OTHER_SEMANTIC_PRIOR);
                }
            }

            if dom.attr(node, AttrName::Id).is_some_and(|id| {
                STRONG_IDS
                    .iter()
                    .any(|pattern| id.eq_ignore_ascii_case(pattern))
            }) {
                candidates.add(node, CandidateSource::Semantic, OTHER_SEMANTIC_PRIOR);
            }

            if dom.attr(node, AttrName::Class).is_some_and(|class| {
                class.split_whitespace().any(|token| {
                    STRONG_CLASSES
                        .iter()
                        .any(|pattern| token.eq_ignore_ascii_case(pattern))
                })
            }) {
                candidates.add(node, CandidateSource::Semantic, OTHER_SEMANTIC_PRIOR);
            }
        }

        candidates
    }

    pub(crate) fn add_readability(&mut self, node: NodeId, score: f64) {
        self.add(node, CandidateSource::Readability, score);
    }

    pub(crate) fn is_semantic(&self, node: NodeId) -> bool {
        self.get(node)
            .is_some_and(|candidate| candidate.has_source(CandidateSource::Semantic))
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &Candidate> {
        self.candidates.iter()
    }

    pub(crate) fn iter_mut(&mut self) -> impl Iterator<Item = &mut Candidate> {
        self.candidates.iter_mut()
    }

    pub(crate) fn is_authoritative_semantic(&self, dom: &Dom, node: NodeId) -> bool {
        matches!(dom.tag(node), Some(Tag::Article | Tag::Main))
            || dom
                .attr(node, AttrName::Role)
                .is_some_and(|roles| matches_role(roles, "article") || matches_role(roles, "main"))
    }

    pub(crate) fn ranking_context(&self, dom: &Dom) -> CandidateContext {
        let mut readability_in_subtree = vec![false; dom.len()];
        let mut authoritative_count = vec![0_u32; dom.len()];
        let mut has_text = vec![false; dom.len()];
        for candidate in &self.candidates {
            readability_in_subtree[candidate.node.index()] =
                candidate.has_source(CandidateSource::Readability);
            if self.is_authoritative_semantic(dom, candidate.node) {
                authoritative_count[candidate.node.index()] = 1;
            }
        }

        let nodes: Vec<_> = dom.descendants(dom.root()).collect();
        for &node in &nodes {
            has_text[node.index()] = dom
                .text_node(node)
                .is_some_and(|text| text.chars().any(|character| !character.is_whitespace()));
        }
        for &node in nodes.iter().rev() {
            if let Some(parent) = dom.parent(node) {
                readability_in_subtree[parent.index()] |= readability_in_subtree[node.index()];
                authoritative_count[parent.index()] = authoritative_count[parent.index()]
                    .saturating_add(authoritative_count[node.index()]);
                has_text[parent.index()] |= has_text[node.index()];
            }
        }

        let mut nearest_authoritative_ancestor = vec![None; dom.len()];
        for (node, _) in dom.element_descendants_snapshot_with_depth(dom.root()) {
            if let Some(parent) = dom.parent(node) {
                nearest_authoritative_ancestor[node.index()] =
                    if self.is_authoritative_semantic(dom, parent) {
                        Some(parent)
                    } else {
                        nearest_authoritative_ancestor[parent.index()]
                    };
            }
        }

        let mut article_peer_count = vec![0_u32; dom.len()];
        let mut article_peer_score = vec![0.0; dom.len()];
        for candidate in &self.candidates {
            let is_article = dom.tag(candidate.node) == Some(Tag::Article)
                || dom
                    .attr(candidate.node, AttrName::Role)
                    .is_some_and(|role| matches_role(role, "article"));
            let Some(parent) = nearest_authoritative_ancestor[candidate.node.index()] else {
                continue;
            };
            if is_article && has_text[candidate.node.index()] {
                article_peer_count[parent.index()] += 1;
                article_peer_score[parent.index()] += candidate.readability_score;
            }
        }

        CandidateContext {
            readability_in_subtree,
            has_authoritative_ancestor: nearest_authoritative_ancestor
                .into_iter()
                .map(|ancestor| ancestor.is_some())
                .collect(),
            authoritative_count,
            article_peer_count,
            article_peer_score,
        }
    }

    pub(crate) fn get(&self, node: NodeId) -> Option<&Candidate> {
        let position = self.positions.get(node.index()).copied()?;
        (position != usize::MAX).then(|| &self.candidates[position])
    }

    fn add(&mut self, node: NodeId, source: CandidateSource, value: f64) {
        if node.index() >= self.positions.len() {
            self.positions.resize(node.index() + 1, usize::MAX);
        }
        let position = self.positions[node.index()];
        if position == usize::MAX {
            self.positions[node.index()] = self.candidates.len();
            let mut sources = CandidateSources::default();
            sources.insert(source);
            self.candidates.push(Candidate {
                node,
                sources,
                semantic_prior: if source == CandidateSource::Semantic {
                    value
                } else {
                    0.0
                },
                readability_score: if source == CandidateSource::Readability {
                    value
                } else {
                    0.0
                },
                features: CandidateFeatures::default(),
            });
            return;
        }

        let candidate = &mut self.candidates[position];
        let already_had_source = candidate.sources.contains(source);
        candidate.sources.insert(source);
        match source {
            CandidateSource::Semantic => {
                // Independent semantic signals increase confidence, but a node
                // cannot gain an unbounded score from redundant attributes.
                candidate.semantic_prior = candidate.semantic_prior.max(value);
                if already_had_source {
                    candidate.semantic_prior = (candidate.semantic_prior + ADDITIONAL_SIGNAL_BONUS)
                        .min(MAX_SEMANTIC_PRIOR);
                }
            }
            CandidateSource::Readability => {
                candidate.readability_score = candidate.readability_score.max(value)
            }
            CandidateSource::Generic => {}
        }
    }
}

fn is_generic_candidate(dom: &Dom, node: NodeId, tag: Option<Tag>) -> bool {
    match tag {
        Some(Tag::Section | Tag::Td) => true,
        Some(Tag::Div) => dom.children(node).any(|child| {
            matches!(
                dom.tag(child),
                Some(Tag::Dl | Tag::Figure | Tag::Ol | Tag::Pre | Tag::Table | Tag::Ul)
            )
        }),
        _ => false,
    }
}

fn is_generic_clutter_container(dom: &Dom, node: NodeId) -> bool {
    matches!(
        dom.tag(node),
        Some(Tag::Aside | Tag::Footer | Tag::Header | Tag::Nav)
    ) || dom.attr(node, AttrName::Role).is_some_and(|roles| {
        ["banner", "complementary", "dialog", "navigation"]
            .into_iter()
            .any(|role| matches_role(roles, role))
    })
}

fn matches_role(roles: &str, expected: &str) -> bool {
    roles
        .split_whitespace()
        .any(|role| role.eq_ignore_ascii_case(expected))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RootSelectionReason {
    Ranked,
    SpecificChild,
    SharedParent,
    StructuredData,
    BodyFallback,
}

#[derive(Clone, Debug)]
pub(crate) struct RootSelection {
    pub(crate) node: NodeId,
    pub(crate) reason: RootSelectionReason,
    /// Direct branches to copy when the common parent also contains clutter.
    pub(crate) branches: SmallVec<[NodeId; 4]>,
}

/// Selects the content boundary after feature ranking.
///
/// Ranking identifies strong individual nodes. This pass handles tree
/// relationships that are difficult to express as independent scores. It can
/// narrow a broad winner, promote a shared semantic parent, and use schema
/// text to resolve close results.
pub(crate) fn select_content_root<'a>(
    dom: &Dom,
    candidates: &CandidateSet,
    ranked: &[RankedCandidate],
    body: NodeId,
    structured_texts: impl IntoIterator<Item = &'a str>,
) -> RootSelection {
    let Some(first) = ranked.first() else {
        return RootSelection {
            node: body,
            reason: RootSelectionReason::BodyFallback,
            branches: SmallVec::new(),
        };
    };
    let mut selected = first.node;
    let mut reason = RootSelectionReason::Ranked;

    let hints: Vec<_> = structured_texts
        .into_iter()
        .filter_map(StructuredHint::new)
        .take(MAX_STRUCTURED_HINTS)
        .collect();
    if !hints.is_empty() {
        let selected_agreement = structured_agreement(dom, selected, &hints);
        if let Some((node, agreement)) = ranked
            .iter()
            .filter(|candidate| {
                candidate.node != body && structured_tie_score(candidate.score, first.score)
            })
            .map(|candidate| {
                (
                    candidate.node,
                    structured_agreement(dom, candidate.node, &hints),
                )
            })
            .max_by(|left, right| left.1.total_cmp(&right.1))
            && agreement >= 0.6
            && agreement > selected_agreement + 0.1
        {
            selected = node;
            reason = RootSelectionReason::StructuredData;
        }
    }

    // A semantic parent is the correct boundary when several strong branches
    // together form the useful page, such as article cards in a main element.
    // A schema match is already a high-confidence boundary, so do not broaden
    // it with unrelated peer candidates.
    let mut branches = SmallVec::new();
    if reason != RootSelectionReason::StructuredData {
        if let Some(parent) = shared_semantic_parent(dom, candidates, ranked, selected, first.score)
        {
            selected = parent;
            reason = RootSelectionReason::SharedParent;
        } else if let Some((parent, shared_branches)) =
            shared_generic_parent(dom, candidates, ranked, selected, body, first.score)
        {
            selected = parent;
            branches = shared_branches;
            reason = RootSelectionReason::SharedParent;
        } else if let Some(child) =
            more_specific_candidate(dom, candidates, ranked, selected, first.score)
        {
            selected = child;
            reason = RootSelectionReason::SpecificChild;
        }
    }

    RootSelection {
        node: selected,
        reason,
        branches,
    }
}

fn competitive_score(score: f64, top_score: f64) -> bool {
    score >= top_score - (top_score.abs() * 0.25).max(5.0)
}

fn structured_tie_score(score: f64, top_score: f64) -> bool {
    score >= top_score - (top_score.abs() * 0.03).max(1.0)
}

fn is_descendant_of(dom: &Dom, node: NodeId, ancestor: NodeId) -> bool {
    dom.ancestors(node).any(|parent| parent == ancestor)
}

fn direct_branch(dom: &Dom, node: NodeId, ancestor: NodeId) -> Option<NodeId> {
    if node == ancestor {
        return None;
    }
    let mut branch = node;
    for parent in dom.ancestors(node) {
        if parent == ancestor {
            return Some(branch);
        }
        branch = parent;
    }
    None
}

fn shared_semantic_parent(
    dom: &Dom,
    candidates: &CandidateSet,
    ranked: &[RankedCandidate],
    selected: NodeId,
    top_score: f64,
) -> Option<NodeId> {
    dom.ancestors(selected).find(|&parent| {
        parent != dom.body().unwrap_or(parent)
            && candidates.is_authoritative_semantic(dom, parent)
            && ranked.iter().any(|candidate| {
                candidate.node == parent && competitive_score(candidate.score, top_score)
            })
            && {
                let mut branches = SmallBranchSet::default();
                for candidate in ranked.iter().filter(|candidate| {
                    competitive_score(candidate.score, top_score)
                        && (candidate.node == selected
                            || is_descendant_of(dom, candidate.node, parent))
                }) {
                    if let Some(branch) = direct_branch(dom, candidate.node, parent) {
                        branches.insert(branch);
                    }
                }
                branches.len() >= 2
            }
    })
}

fn shared_generic_parent(
    dom: &Dom,
    candidates: &CandidateSet,
    ranked: &[RankedCandidate],
    selected: NodeId,
    body: NodeId,
    top_score: f64,
) -> Option<(NodeId, SmallVec<[NodeId; 4]>)> {
    let anchor = ranked.iter().find(|candidate| {
        competitive_score(candidate.score, top_score)
            && candidates.get(candidate.node).is_some_and(|candidate| {
                candidate.has_source(CandidateSource::Generic) && candidate.node != body
            })
    })?;
    let expand_wrapped_branches = !dom
        .ancestors(anchor.node)
        .any(|node| candidates.is_authoritative_semantic(dom, node));

    dom.ancestors(anchor.node).find_map(|parent| {
        if is_generic_clutter_container(dom, parent)
            || !(selected == body || selected == parent || is_descendant_of(dom, selected, parent))
        {
            return None;
        }

        let mut branch_set = SmallBranchSet::default();
        for candidate in ranked.iter().filter(|candidate| {
            competitive_score(candidate.score, top_score)
                && candidates
                    .get(candidate.node)
                    .is_some_and(|candidate| candidate.has_source(CandidateSource::Generic))
                && is_descendant_of(dom, candidate.node, parent)
        }) {
            if let Some(branch) = direct_branch(dom, candidate.node, parent)
                && (expand_wrapped_branches || branch == candidate.node)
            {
                branch_set.insert(branch);
            }
        }
        if branch_set.len() < 2 {
            return None;
        }

        // Keep document order. Ranking order can differ from tree order.
        let branches = dom
            .children(parent)
            .filter(|child| branch_set.contains(*child))
            .collect();
        Some((parent, branches))
    })
}

#[derive(Default)]
struct SmallBranchSet(SmallVec<[NodeId; 4]>);

impl SmallBranchSet {
    fn insert(&mut self, node: NodeId) {
        if !self.0.contains(&node) {
            self.0.push(node);
        }
    }

    fn contains(&self, node: NodeId) -> bool {
        self.0.contains(&node)
    }

    fn len(&self) -> usize {
        self.0.len()
    }
}

fn more_specific_candidate(
    dom: &Dom,
    candidates: &CandidateSet,
    ranked: &[RankedCandidate],
    selected: NodeId,
    top_score: f64,
) -> Option<NodeId> {
    let selected_chars = candidates.get(selected)?.features.text_chars.max(1);
    ranked
        .iter()
        .filter(|candidate| {
            candidate.node != selected
                && competitive_score(candidate.score, top_score)
                && is_descendant_of(dom, candidate.node, selected)
                && candidates
                    .get(candidate.node)
                    .is_some_and(|value| value.has_source(CandidateSource::Semantic))
        })
        .filter_map(|candidate| {
            let features = candidates.get(candidate.node)?.features;
            let coverage = f64::from(features.text_chars) / f64::from(selected_chars);
            (coverage >= 0.8).then_some((candidate.node, features.text_chars))
        })
        .min_by_key(|(_, chars)| *chars)
        .map(|(node, _)| node)
}

const MAX_STRUCTURED_HINTS: usize = 8;
const MAX_STRUCTURED_HINT_CHARS: usize = 4_096;
const MAX_CANDIDATE_MATCH_CHARS: usize = 16_384;

struct StructuredHint {
    tokens: Vec<String>,
    token_set: HashSet<String>,
    text_chars: usize,
}

impl StructuredHint {
    fn new(text: &str) -> Option<Self> {
        let prefix = char_prefix(text, MAX_STRUCTURED_HINT_CHARS).to_lowercase();
        let tokens: Vec<_> = split_word_tokens(&prefix).map(str::to_owned).collect();
        if tokens.len() < 5 {
            return None;
        }
        let text_chars = tokens
            .iter()
            .map(|token| token.chars().count())
            .sum::<usize>()
            + tokens.len().saturating_sub(1);
        let token_set = tokens.iter().cloned().collect();
        Some(Self {
            tokens,
            token_set,
            text_chars,
        })
    }
}

fn char_prefix(text: &str, limit: usize) -> &str {
    text.char_indices()
        .nth(limit)
        .map_or(text, |(end, _)| &text[..end])
}

fn structured_agreement(dom: &Dom, node: NodeId, hints: &[StructuredHint]) -> f64 {
    let candidate_text =
        bounded_normalized_text(dom, node, MAX_CANDIDATE_MATCH_CHARS).to_lowercase();
    let candidate_tokens: Vec<_> = split_word_tokens(&candidate_text).collect();
    if candidate_tokens.is_empty() {
        return 0.0;
    }
    let candidate_set: HashSet<_> = candidate_tokens.iter().copied().collect();
    hints
        .iter()
        .map(|hint| {
            let (unique_count, unique_chars) = hint
                .tokens
                .iter()
                .filter(|token| !candidate_set.contains(token.as_str()))
                .fold((0_usize, 0_usize), |(count, chars), token| {
                    (count + 1, chars + token.chars().count())
                });
            let unique_chars = unique_chars + unique_count.saturating_sub(1);
            let coverage = 1.0 - unique_chars as f64 / hint.text_chars as f64;
            let precision = candidate_tokens
                .iter()
                .filter(|token| hint.token_set.contains(**token))
                .count() as f64
                / candidate_tokens.len() as f64;
            if coverage + precision == 0.0 {
                0.0
            } else {
                2.0 * coverage * precision / (coverage + precision)
            }
        })
        .fold(0.0, f64::max)
}

fn bounded_normalized_text(dom: &Dom, root: NodeId, limit: usize) -> String {
    let mut out = String::with_capacity(limit.min(1_024));
    let mut character_count = 0;
    let mut pending_whitespace = false;
    'nodes: for node in std::iter::once(root).chain(dom.descendants(root)) {
        let Some(text) = dom.text_node(node) else {
            continue;
        };
        for character in text.chars() {
            if character.is_whitespace() {
                pending_whitespace |= !out.is_empty();
                continue;
            }
            if pending_whitespace {
                if character_count == limit {
                    break 'nodes;
                }
                out.push(' ');
                character_count += 1;
                pending_whitespace = false;
            }
            if character_count == limit {
                break 'nodes;
            }
            out.push(character);
            character_count += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovers_and_merges_semantic_signals() {
        let dom = Dom::parse_document(
            r#"<body><main id="content" role="main"><div class="markdown-body"></div></main></body>"#,
        )
        .unwrap();
        let main = dom.first_descendant_by_tag(dom.root(), Tag::Main).unwrap();
        let markdown = dom.first_descendant_by_tag(dom.root(), Tag::Div).unwrap();
        let candidates = CandidateSet::discover_semantic(&dom);

        let main_candidate = candidates.get(main).unwrap();
        assert!(main_candidate.has_source(CandidateSource::Semantic));
        assert_eq!(
            candidates
                .iter()
                .filter(|candidate| candidate.node == main)
                .count(),
            1
        );
        assert_eq!(main_candidate.semantic_prior, 0.0035);
        assert!(candidates.is_semantic(markdown));
        assert!(
            candidates
                .iter()
                .any(|candidate| candidate.has_source(CandidateSource::Generic))
        );
    }

    #[test]
    fn discovers_generic_structural_roots() {
        let dom = Dom::parse_document(
            "<body><section><div><pre>code</pre></div><table><tr><td>value</td></tr></table></section></body>",
        )
        .unwrap();
        let candidates = CandidateSet::discover_semantic(&dom);

        for tag in [Tag::Section, Tag::Div, Tag::Td] {
            let node = dom.first_descendant_by_tag(dom.root(), tag).unwrap();
            assert!(
                candidates
                    .get(node)
                    .is_some_and(|candidate| candidate.has_source(CandidateSource::Generic)),
                "missing generic {tag:?} candidate"
            );
        }
    }

    #[test]
    fn generic_discovery_skips_clutter_regions() {
        let dom = Dom::parse_document(
            "<body><div role=banner><div id=nav></div></div><section id=content></section></body>",
        )
        .unwrap();
        let candidates = CandidateSet::discover_semantic(&dom);
        let nav = dom
            .descendants(dom.root())
            .find(|&node| dom.attr(node, AttrName::Id) == Some("nav"))
            .unwrap();
        let content = dom
            .descendants(dom.root())
            .find(|&node| dom.attr(node, AttrName::Id) == Some("content"))
            .unwrap();

        assert!(candidates.get(nav).is_none());
        assert!(
            candidates
                .get(content)
                .is_some_and(|candidate| candidate.has_source(CandidateSource::Generic))
        );
    }

    #[test]
    fn strong_names_match_complete_tokens_only() {
        let dom = Dom::parse_document(
            r#"<body><div class="postscript"></div><div class="post"></div></body>"#,
        )
        .unwrap();
        let divs: Vec<_> = dom
            .element_descendants_snapshot_with_depth(dom.root())
            .into_iter()
            .map(|(node, _)| node)
            .filter(|&node| dom.tag(node) == Some(Tag::Div))
            .collect();
        let candidates = CandidateSet::discover_semantic(&dom);

        assert!(!candidates.is_semantic(divs[0]));
        assert!(candidates.is_semantic(divs[1]));
    }

    #[test]
    fn semantic_roots_drive_extraction() {
        for opening in [
            "<main>",
            "<article>",
            "<div class=markdown-body>",
            "<div class=entry-content>",
            "<div id=content>",
            "<div role=main>",
            "<div role=article>",
        ] {
            let closing = if opening.starts_with("<main") {
                "</main>"
            } else if opening.starts_with("<article") {
                "</article>"
            } else {
                "</div>"
            };
            let html = format!(
                "<body><p>Outside clutter</p>{opening}<p>Chosen semantic content has enough text to preserve.</p>{closing}</body>"
            );
            let markdown = crate::extract(&html, None).unwrap().markdown();
            assert!(markdown.contains("Chosen semantic content"), "{opening}");
            assert!(!markdown.contains("Outside clutter"), "{opening}");
        }
    }

    #[test]
    fn short_structural_semantic_roots_exclude_clutter() {
        for content in [
            "<pre>cargo test</pre>",
            "<ul><li>Build</li><li>Test</li><li>Ship</li></ul>",
        ] {
            let html = format!(
                "<body><header>Header clutter</header><div class=markdown-body>{content}</div><footer>Footer clutter</footer></body>"
            );
            let markdown = crate::extract(&html, None).unwrap().markdown();

            assert!(!markdown.contains("Header clutter"), "{markdown}");
            assert!(!markdown.contains("Footer clutter"), "{markdown}");
        }
    }

    #[test]
    fn generic_structural_root_excludes_header_clutter() {
        let html =
            "<body><header>Header clutter</header><section><pre>cargo test</pre></section></body>";
        let markdown = crate::extract(html, None).unwrap().markdown();

        assert!(markdown.contains("cargo test"), "{markdown}");
        assert!(!markdown.contains("Header clutter"), "{markdown}");
    }

    #[test]
    fn generic_sibling_roots_are_consolidated() {
        let html = r#"<body>
            <header>Header clutter</header>
            <section><ul><li>One</li><li>Two</li><li>Three</li></ul></section>
            <section><ul><li>Four</li><li>Five</li><li>Six</li></ul></section>
        </body>"#;
        let markdown = crate::extract(html, None).unwrap().markdown();

        for item in ["One", "Two", "Three", "Four", "Five", "Six"] {
            assert!(markdown.contains(item), "missing {item}: {markdown}");
        }
        assert!(!markdown.contains("Header clutter"), "{markdown}");
    }

    #[test]
    fn wrapped_generic_sibling_roots_are_consolidated() {
        let html = r#"<body>
            <header>Header clutter</header>
            <div>
                <div><section><ul><li>One</li><li>Two</li><li>Three</li></ul></section></div>
                <div><section><ul><li>Four</li><li>Five</li><li>Six</li></ul></section></div>
            </div>
        </body>"#;
        let markdown = crate::extract(html, None).unwrap().markdown();

        for item in ["One", "Two", "Three", "Four", "Five", "Six"] {
            assert!(markdown.contains(item), "missing {item}: {markdown}");
        }
        assert!(!markdown.contains("Header clutter"), "{markdown}");
    }

    #[test]
    fn linked_generic_sibling_roots_exclude_common_parent_clutter() {
        let html = r#"<body>
            <header>Header clutter</header>
            <section><ul><li><a href="/1">One</a></li><li><a href="/2">Two</a></li><li><a href="/3">Three</a></li></ul></section>
            <section><ul><li><a href="/4">Four</a></li><li><a href="/5">Five</a></li><li><a href="/6">Six</a></li></ul></section>
        </body>"#;
        let markdown = crate::extract(html, Some("https://example.com"))
            .unwrap()
            .markdown();

        assert!(
            markdown.contains("[One](https://example.com/1)"),
            "{markdown}"
        );
        assert!(
            markdown.contains("[Six](https://example.com/6)"),
            "{markdown}"
        );
        assert!(!markdown.contains("Header clutter"), "{markdown}");
    }

    #[test]
    fn semantic_candidate_can_override_readability_winner() {
        let html = r#"<body><main><h2>Semantic context</h2><blockquote>
            <p>Focused sentence has enough text.</p>
        </blockquote></main></body>"#;
        let markdown = crate::extract(html, None).unwrap().markdown();

        // Readability gives the nested blockquote a larger propagated score.
        // The chooser can still retain the short authoritative semantic root.
        assert!(markdown.contains("Semantic context"), "{markdown}");
        assert!(markdown.contains("Focused sentence"), "{markdown}");
    }

    #[test]
    fn nested_article_is_more_specific_than_main() {
        let html = r#"<body><main><p>Main introduction</p><article>
            <p>The nested article is the selected specific content root.</p>
        </article></main></body>"#;
        let markdown = crate::extract(html, None).unwrap().markdown();

        assert!(markdown.contains("nested article"));
        assert!(!markdown.contains("Main introduction"));
    }

    #[test]
    fn main_keeps_multiple_article_cards() {
        let html = r#"<body><main>
            <article><h2>First card</h2><p>First useful summary. It has much more prose, several clauses, and enough detail to outscore the other card.</p></article>
            <article><h2>Second card</h2><p>Second useful summary.</p></article>
        </main></body>"#;
        let markdown = crate::extract(html, None).unwrap().markdown();

        assert!(markdown.contains("First card"), "{markdown}");
        assert!(markdown.contains("Second card"), "{markdown}");
    }

    #[test]
    fn short_main_beats_long_header_boilerplate() {
        for answer in ["Short useful answer.", "This is a short useful answer."] {
            let html = format!(
                r#"<body>
                    <header><p>This long site header explains navigation, account controls, subscriptions, promotions, and other boilerplate.</p></header>
                    <main><p>{answer}</p></main>
                </body>"#
            );
            let markdown = crate::extract(&html, None).unwrap().markdown();

            assert!(markdown.contains(answer), "{answer}: {markdown}");
            assert!(
                !markdown.contains("long site header"),
                "{answer}: {markdown}"
            );
        }
    }

    #[test]
    fn empty_article_does_not_promote_main() {
        let html = r#"<body><main><p>Unrelated intro</p>
            <article><p>This substantive nested article is the selected content root.</p></article>
            <article></article>
        </main></body>"#;
        let markdown = crate::extract(html, None).unwrap().markdown();

        assert!(markdown.contains("substantive nested article"));
        assert!(!markdown.contains("Unrelated intro"));
    }

    #[test]
    fn weak_wrappers_do_not_hide_authoritative_semantics() {
        let short_main = r#"<body>
            <header><p>This long site header explains navigation, account controls, subscriptions, promotions, and other boilerplate.</p></header>
            <div id="content"><main><p>Short wrapped answer.</p></main></div>
        </body>"#;
        let markdown = crate::extract(short_main, None).unwrap().markdown();
        assert!(markdown.contains("Short wrapped answer"));
        assert!(!markdown.contains("long site header"));

        let wrapped_cards = r#"<body><main>
            <div class="post"><article><h2>First wrapped card</h2><p>First summary.</p></article></div>
            <div class="post"><article><h2>Second wrapped card</h2><p>Second summary.</p></article></div>
        </main></body>"#;
        let markdown = crate::extract(wrapped_cards, None).unwrap().markdown();
        assert!(markdown.contains("First wrapped card"));
        assert!(markdown.contains("Second wrapped card"));
    }

    #[test]
    fn weak_semantic_name_does_not_override_prose() {
        for weak in [r#"class="post""#, r#"id="content""#] {
            let html = format!(
                r#"<body><div {weak}>Related post</div><section><p>Actual useful answer.</p></section></body>"#
            );
            let markdown = crate::extract(&html, None).unwrap().markdown();

            assert!(
                markdown.contains("Actual useful answer"),
                "{weak}: {markdown}"
            );
        }
    }

    fn select_test_root<'a>(
        html: &str,
        ranking: &[(&str, f64)],
        hints: impl IntoIterator<Item = &'a str>,
    ) -> String {
        let dom = Dom::parse_document(html).unwrap();
        let mut candidates = CandidateSet::discover_semantic(&dom);
        let mut ranked = SmallVec::<[RankedCandidate; 8]>::new();
        for (order, &(id, score)) in ranking.iter().enumerate() {
            let node = dom
                .descendants(dom.root())
                .find(|&node| dom.attr(node, AttrName::Id) == Some(id))
                .unwrap();
            candidates.add_readability(node, score);
            ranked.push(RankedCandidate { node, score, order });
        }
        for candidate in candidates.iter_mut() {
            candidate.features.text_chars = dom
                .normalized_char_count(candidate.node)
                .min(u32::MAX as usize) as u32;
        }
        let body = dom.body().unwrap();
        let selected = select_content_root(&dom, &candidates, &ranked, body, hints);
        dom.attr(selected.node, AttrName::Id)
            .unwrap_or("body")
            .to_owned()
    }

    #[test]
    fn root_selection_prefers_a_specific_equivalent_child() {
        let html = r#"<body><main id="broad"><article id="specific"><p>The same substantial content appears inside this precise semantic child, with enough text to represent almost all of its parent.</p></article></main></body>"#;

        assert_eq!(
            select_test_root(html, &[("broad", 100.0), ("specific", 90.0)], []),
            "specific"
        );
    }

    #[test]
    fn root_selection_rejects_a_tiny_nested_candidate() {
        let html = r#"<body><main id="broad"><p>The broad document contains a long useful introduction and several important facts that belong in the result.</p><article id="tiny">Tiny card.</article><p>It also contains a useful conclusion outside the card.</p></main></body>"#;

        assert_eq!(
            select_test_root(html, &[("broad", 100.0), ("tiny", 99.0)], []),
            "broad"
        );
    }

    #[test]
    fn root_selection_promotes_the_parent_of_strong_siblings() {
        let html = r#"<body><main id="listing"><article id="first">First useful card with a complete summary.</article><article id="second">Second useful card with another complete summary.</article></main></body>"#;

        assert_eq!(
            select_test_root(
                html,
                &[("first", 100.0), ("second", 90.0), ("listing", 85.0)],
                [],
            ),
            "listing"
        );
    }

    #[test]
    fn root_selection_keeps_a_structured_content_winner() {
        let html = r#"<body><div id="prose"><p>A prose summary competes with the reference.</p></div><main id="reference"><h1>Commands</h1><pre><code>cargo test --all-features</code></pre><table><tr><th>Flag</th><th>Meaning</th></tr></table></main></body>"#;

        assert_eq!(
            select_test_root(html, &[("reference", 100.0), ("prose", 98.0)], [],),
            "reference"
        );
    }

    #[test]
    fn root_selection_uses_structured_text_to_break_a_close_result() {
        let html = r#"<body><div id="other"><p>An unrelated candidate has polished prose and enough detail to rank first.</p></div><article id="matching"><p>The schema text identifies this exact article body and its important conclusion.</p></article></body>"#;
        let hint =
            "The schema text identifies this exact article body and its important conclusion.";

        assert_eq!(
            select_test_root(html, &[("other", 100.0), ("matching", 98.0)], [hint]),
            "matching"
        );
    }

    #[test]
    fn structured_text_does_not_select_the_broad_body() {
        let html = r#"<body id="body"><aside>Unrelated navigation, promotions, account controls, and other page furniture.</aside><article id="matching"><p>The schema text identifies this exact article body and its important conclusion.</p></article></body>"#;
        let hint =
            "The schema text identifies this exact article body and its important conclusion.";

        assert_eq!(
            select_test_root(html, &[("body", 100.0), ("matching", 99.0)], [hint]),
            "matching"
        );
    }

    #[test]
    fn root_selection_falls_back_to_body() {
        assert_eq!(
            select_test_root("<body id=body><p>Visible fallback.</p></body>", &[], []),
            "body"
        );
    }

    #[test]
    fn structured_root_selection_excludes_unrelated_siblings_end_to_end() {
        let html = r#"<body>
            <script type="application/ld+json">{
                "@context":"https://schema.org",
                "@type":"Article",
                "articleBody":"The schema-selected report explains the exact result with several specific details and a final conclusion."
            }</script>
            <article><p>An unrelated report explains a different result with several polished details and a separate conclusion.</p></article>
            <article><p>The schema-selected report explains the exact result with several specific details and a final conclusion.</p></article>
        </body>"#;

        let markdown = crate::extract(html, None).unwrap().markdown();

        assert!(markdown.contains("schema-selected report"), "{markdown}");
        assert!(!markdown.contains("unrelated report"), "{markdown}");
    }

    #[test]
    fn structured_root_selection_ignores_related_json_ld_items() {
        let html = r#"<body><h1>Primary report</h1>
            <script type="application/ld+json">[
                {"@context":"https://schema.org","@type":"Article","headline":"Related report","articleBody":"The related card contains a separate schema body with enough words to look plausible."},
                {"@context":"https://schema.org","@type":"Article","headline":"Primary report","articleBody":"The primary report contains the selected schema body with exact useful details."}
            ]</script>
            <article><p>The related card contains a separate schema body with enough words to look plausible.</p></article>
            <article><p>The primary report contains the selected schema body with exact useful details.</p></article>
        </body>"#;

        let markdown = crate::extract(html, None).unwrap().markdown();

        assert!(markdown.contains("primary report"), "{markdown}");
        assert!(!markdown.contains("related card"), "{markdown}");
    }
}
