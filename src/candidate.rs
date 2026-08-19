//! Internal content-root candidates.

use crate::constants::split_word_tokens;
use crate::dom::{AttrName, Dom, NodeId, NodeStateStore, Tag};
use crate::prepared::{SourceAnalysis, SourceFlags};
use crate::scoring::has_static_hidden_marker;
use smallvec::SmallVec;
use std::collections::HashSet;

const STRONG_IDS: &[&str] = &["post", "content", "article-content"];
const ARTICLE_TAG_PRIOR: f64 = 0.003;
const MAIN_TAG_PRIOR: f64 = 0.0025;
const ARTICLE_ROLE_PRIOR: f64 = 0.00275;
const ARTICLE_BODY_PRIOR: f64 = 0.0035;
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
const NO_CANDIDATE: u32 = u32::MAX;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CandidateSource {
    Semantic,
    Readability,
    StructuredData,
    Generic,
    CallerHint,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct CandidateSources(u8);

impl CandidateSources {
    fn insert(&mut self, source: CandidateSource) {
        self.0 |= match source {
            CandidateSource::Semantic => 1 << 0,
            CandidateSource::Readability => 1 << 1,
            CandidateSource::StructuredData => 1 << 2,
            CandidateSource::Generic => 1 << 3,
            CandidateSource::CallerHint => 1 << 4,
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
    pub(crate) code_bytes: u32,
    pub(crate) table_count: u32,
    pub(crate) non_empty_table_cell_count: u32,
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
#[derive(Clone, Debug)]
pub(crate) struct CandidateSet {
    candidates: Vec<Candidate>,
    /// Candidate indexes are looked up by source NodeId. Keep this map dense
    /// because callers already have stable node IDs, but use a u32 sentinel
    /// so the map does not double the size of the node index on 64-bit hosts.
    positions: Vec<u32>,
    has_article_body: bool,
}

pub(crate) struct SourceCandidateBuilder {
    candidates: CandidateSet,
    generic_clutter_depth: Option<u32>,
}

impl SourceCandidateBuilder {
    pub(crate) fn new(source_node_count: usize) -> Self {
        Self {
            candidates: CandidateSet::empty(source_node_count),
            generic_clutter_depth: None,
        }
    }

    pub(crate) fn observe(&mut self, dom: &Dom, entry: &crate::prepared::SourceEntry) {
        let node = entry.node;
        let depth = entry.depth;
        if self
            .generic_clutter_depth
            .is_some_and(|root_depth| depth <= root_depth)
        {
            self.generic_clutter_depth = None;
        }
        let in_generic_clutter = self.generic_clutter_depth.is_some();
        if !in_generic_clutter && is_generic_clutter_entry(entry) {
            self.generic_clutter_depth = Some(depth);
        }
        if entry.tag == Some(Tag::Body) {
            self.candidates.add(node, CandidateSource::Generic, 0.0);
        }
        let prior = match entry.tag {
            Some(Tag::Article) => Some(ARTICLE_TAG_PRIOR),
            Some(Tag::Main) => Some(MAIN_TAG_PRIOR),
            _ => None,
        };
        if let Some(prior) = prior {
            self.candidates.add(node, CandidateSource::Semantic, prior);
        }
        if !in_generic_clutter
            && self.generic_clutter_depth != Some(depth)
            && is_generic_candidate(dom, node, entry.tag)
        {
            self.candidates.add(node, CandidateSource::Generic, 0.0);
        }
        if entry.flags.contains(SourceFlags::ARTICLE_ROLE) {
            self.candidates
                .add(node, CandidateSource::Semantic, ARTICLE_ROLE_PRIOR);
        }
        if entry.flags.contains(SourceFlags::MAIN_ROLE) {
            self.candidates
                .add(node, CandidateSource::Semantic, OTHER_SEMANTIC_PRIOR);
        }
        if entry.flags.contains(SourceFlags::ARTICLE_BODY) {
            self.candidates.add_article_body(node);
        }
        if entry.flags.contains(SourceFlags::STRONG_CONTENT_ID)
            || entry.flags.contains(SourceFlags::STRONG_CONTENT_CLASS)
        {
            self.candidates
                .add(node, CandidateSource::Semantic, OTHER_SEMANTIC_PRIOR);
        }
    }

    pub(crate) fn finish(self) -> CandidateSet {
        self.candidates
    }
}

pub(crate) struct CandidateContext {
    // All relationship values are indexed by CandidateSet order. Most source
    // nodes are not candidates, so these must not be DOM-sized arrays.
    readability_in_subtree: Vec<bool>,
    has_authoritative_ancestor: Vec<bool>,
    authoritative_count: Vec<u32>,
    article_peer_count: Vec<u32>,
    article_peer_score: Vec<f64>,
    source_positions: Vec<u32>,
}

impl CandidateContext {
    pub(crate) fn has_readability(&self, candidate: usize) -> bool {
        self.readability_in_subtree[candidate]
    }

    pub(crate) fn has_authoritative_ancestor(&self, candidate: usize) -> bool {
        self.has_authoritative_ancestor[candidate]
    }

    pub(crate) fn has_authoritative_descendant(&self, candidate: usize, own: bool) -> bool {
        self.authoritative_count[candidate] > u32::from(own)
    }

    pub(crate) fn article_peer_summary(&self, candidate: usize) -> (u32, f64) {
        (
            self.article_peer_count[candidate],
            self.article_peer_score[candidate],
        )
    }

    pub(crate) fn source_order(&self, candidate: usize) -> usize {
        let position = self.source_positions[candidate];
        if position != NO_CANDIDATE {
            return position as usize;
        }
        // Detached or synthetic candidates are not present in the snapshot.
        // Keep their order deterministic after attached source nodes.
        usize::MAX
            .saturating_sub(self.source_positions.len())
            .saturating_add(candidate)
    }
}

impl CandidateSet {
    pub(crate) fn empty(source_node_count: usize) -> Self {
        Self {
            candidates: Vec::new(),
            positions: vec![NO_CANDIDATE; source_node_count],
            has_article_body: false,
        }
    }

    #[cfg(test)]
    pub(crate) fn discover_semantic(dom: &Dom) -> Self {
        let snapshot = dom.element_descendants_snapshot_with_depth(dom.root());
        Self::discover_semantic_from_snapshot(dom, &snapshot, dom.body())
    }

    pub(crate) fn discover_semantic_from_snapshot(
        dom: &Dom,
        snapshot: &[(NodeId, u32)],
        body: Option<NodeId>,
    ) -> Self {
        Self::discover_semantic_from_entries(dom, snapshot.iter().copied(), body)
    }

    fn discover_semantic_from_entries(
        dom: &Dom,
        entries: impl IntoIterator<Item = (NodeId, u32)>,
        body: Option<NodeId>,
    ) -> Self {
        let mut candidates = Self {
            candidates: Vec::new(),
            positions: vec![NO_CANDIDATE; dom.len()],
            has_article_body: false,
        };

        if let Some(body) = body {
            candidates.add(body, CandidateSource::Generic, 0.0);
        }

        let mut generic_clutter_depth = None;
        for (node, depth) in entries {
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

            if has_article_body_itemprop(dom, node) {
                candidates.add_article_body(node);
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

    pub(crate) fn add_structured_data(&mut self, node: NodeId) {
        self.add(node, CandidateSource::StructuredData, 40.0);
    }

    pub(crate) fn add_caller_hint(&mut self, node: NodeId) {
        self.add(node, CandidateSource::CallerHint, 2.0);
    }

    pub(crate) fn is_semantic(&self, node: NodeId) -> bool {
        self.get(node)
            .is_some_and(|candidate| candidate.has_source(CandidateSource::Semantic))
    }

    pub(crate) fn has_article_body(&self) -> bool {
        self.has_article_body
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &Candidate> {
        self.candidates.iter()
    }

    pub(crate) fn iter_mut(&mut self) -> impl Iterator<Item = &mut Candidate> {
        self.candidates.iter_mut()
    }

    pub(crate) fn is_authoritative_semantic(&self, dom: &Dom, node: NodeId) -> bool {
        is_authoritative_semantic_node(dom, node)
    }

    pub(crate) fn ranking_context(
        &self,
        dom: &Dom,
        store: &NodeStateStore,
        nodes: &[(NodeId, u32)],
    ) -> CandidateContext {
        let candidate_count = self.candidates.len();
        let mut source_positions = vec![NO_CANDIDATE; candidate_count];
        let mut candidate_parent = vec![NO_CANDIDATE; candidate_count];
        let mut nearest_authoritative_ancestor = vec![NO_CANDIDATE; candidate_count];
        let mut active_candidates: Vec<(u32, u32)> = Vec::new();
        let mut active_authoritative: Vec<(u32, u32)> = Vec::new();

        // Build the candidate containment tree from the existing preorder
        // snapshot. This preserves DOM ancestry while keeping all retained
        // relationship state candidate-sized.
        for (source_position, &(node, depth)) in nodes.iter().enumerate() {
            let Ok(source_position) = u32::try_from(source_position) else {
                break;
            };
            while active_candidates
                .last()
                .is_some_and(|&(active_depth, _)| active_depth >= depth)
            {
                active_candidates.pop();
            }
            while active_authoritative
                .last()
                .is_some_and(|&(active_depth, _)| active_depth >= depth)
            {
                active_authoritative.pop();
            }

            let candidate_index = self.candidate_index(node);
            if let Some(candidate_index) = candidate_index {
                source_positions[candidate_index] = source_position;
                candidate_parent[candidate_index] = active_candidates
                    .last()
                    .map_or(NO_CANDIDATE, |&(_, parent)| parent);
                nearest_authoritative_ancestor[candidate_index] = active_authoritative
                    .last()
                    .map_or(NO_CANDIDATE, |&(_, ancestor)| ancestor);
                active_candidates.push((depth, candidate_index as u32));
            }

            if self.is_authoritative_semantic(dom, node)
                && let Some(candidate_index) = candidate_index
            {
                active_authoritative.push((depth, candidate_index as u32));
            }
        }

        let mut candidate_order: Vec<_> = (0..candidate_count).collect();
        candidate_order.sort_unstable_by_key(|&candidate| source_positions[candidate]);

        let mut readability_in_subtree = vec![false; candidate_count];
        let mut authoritative_count = vec![0_u32; candidate_count];
        for (index, candidate) in self.candidates.iter().enumerate() {
            readability_in_subtree[index] = candidate.has_source(CandidateSource::Readability);
            authoritative_count[index] =
                u32::from(self.is_authoritative_semantic(dom, candidate.node));
        }
        for &child in candidate_order.iter().rev() {
            let parent = candidate_parent[child];
            if parent != NO_CANDIDATE {
                let parent = parent as usize;
                readability_in_subtree[parent] |= readability_in_subtree[child];
                authoritative_count[parent] =
                    authoritative_count[parent].saturating_add(authoritative_count[child]);
            }
        }

        let mut article_peer_count = vec![0_u32; candidate_count];
        let mut article_peer_score = vec![0.0; candidate_count];
        for (candidate_index, candidate) in self.candidates.iter().enumerate() {
            let is_article = dom.tag(candidate.node) == Some(Tag::Article)
                || dom
                    .attr(candidate.node, AttrName::Role)
                    .is_some_and(|role| matches_role(role, "article"));
            let parent = nearest_authoritative_ancestor[candidate_index];
            if parent == NO_CANDIDATE {
                continue;
            }
            let parent = parent as usize;
            if is_article
                && dom.parent(candidate.node).is_some()
                && store
                    .get_stats(candidate.node)
                    .is_some_and(|stats| stats.has_non_whitespace())
            {
                article_peer_count[parent] += 1;
                article_peer_score[parent] += candidate.readability_score;
            }
        }

        CandidateContext {
            readability_in_subtree,
            has_authoritative_ancestor: nearest_authoritative_ancestor
                .iter()
                .map(|&ancestor| ancestor != NO_CANDIDATE)
                .collect(),
            authoritative_count,
            article_peer_count,
            article_peer_score,
            source_positions,
        }
    }

    pub(crate) fn get(&self, node: NodeId) -> Option<&Candidate> {
        self.candidate_index(node)
            .and_then(|position| self.candidates.get(position))
    }

    fn add(&mut self, node: NodeId, source: CandidateSource, value: f64) {
        if node.index() >= self.positions.len() {
            self.positions.resize(node.index() + 1, NO_CANDIDATE);
        }
        let position = self.positions[node.index()];
        if position == NO_CANDIDATE {
            let Ok(position) = u32::try_from(self.candidates.len()) else {
                // NodeId and the lookup sentinel bound the representable
                // candidate count. If that bound is reached, leave the
                // candidate unindexed instead of creating an invalid map
                // entry or panicking on caller-controlled input.
                return;
            };
            if position == NO_CANDIDATE {
                return;
            }
            self.positions[node.index()] = position;
            let mut sources = CandidateSources::default();
            sources.insert(source);
            self.candidates.push(Candidate {
                node,
                sources,
                semantic_prior: if matches!(
                    source,
                    CandidateSource::Semantic
                        | CandidateSource::StructuredData
                        | CandidateSource::CallerHint
                ) {
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

        let candidate = &mut self.candidates[position as usize];
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
            CandidateSource::StructuredData => {
                candidate.semantic_prior = candidate.semantic_prior.max(value)
            }
            CandidateSource::CallerHint => {
                candidate.semantic_prior = candidate.semantic_prior.max(value)
            }
            CandidateSource::Generic => {}
        }
    }

    fn add_article_body(&mut self, node: NodeId) {
        self.has_article_body = true;
        self.add(node, CandidateSource::Semantic, ARTICLE_BODY_PRIOR);
    }

    fn candidate_index(&self, node: NodeId) -> Option<usize> {
        self.positions
            .get(node.index())
            .copied()
            .filter(|&position| position != NO_CANDIDATE)
            .map(|position| position as usize)
    }

    pub(crate) fn index_of(&self, node: NodeId) -> Option<usize> {
        self.candidate_index(node)
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

fn is_generic_clutter_entry(entry: &crate::prepared::SourceEntry) -> bool {
    matches!(
        entry.tag,
        Some(Tag::Aside | Tag::Footer | Tag::Header | Tag::Nav)
    ) || entry.flags.contains(SourceFlags::GENERIC_CLUTTER_ROLE)
}

fn matches_role(roles: &str, expected: &str) -> bool {
    roles
        .split_whitespace()
        .any(|role| role.eq_ignore_ascii_case(expected))
}

pub(crate) fn has_article_body_itemprop(dom: &Dom, node: NodeId) -> bool {
    dom.attr(node, AttrName::ItemProp).is_some_and(|value| {
        value
            .split_ascii_whitespace()
            .any(|item| item.eq_ignore_ascii_case("articleBody"))
    })
}

/// Checks source-relative completeness that remains valid after the selected
/// fragment is cleaned. The final metrics provide the local content floor;
/// this check preserves the strongest-candidate and article-peer safeguards.
pub(crate) fn semantic_root_has_complete_candidate(
    dom: &Dom,
    candidates: &CandidateSet,
    _ranked: &[RankedCandidate],
    selected: NodeId,
    body: NodeId,
) -> bool {
    if !candidates.is_authoritative_semantic(dom, selected) {
        return false;
    }
    let Some(root_features) = candidates.get(selected).map(|candidate| candidate.features) else {
        return false;
    };
    let structured_content = (root_features.code_block_count > 0 && root_features.code_bytes >= 64)
        || (root_features.table_count > 0 && root_features.non_empty_table_cell_count >= 2);
    let coherent_document = root_features.paragraph_count >= 2
        || root_features.heading_count >= 2
        || structured_content;
    if root_features.text_chars < 500
        || root_features.word_count < 30
        || !coherent_document
        || root_features.link_density > 0.45
        || root_features.link_text_chars >= f64::from(root_features.text_chars)
    {
        return false;
    }
    // Compare the root with its strongest contained candidate. A page-level
    // readability winner can be unrelated chrome, so it must not make a
    // complete semantic root look incomplete.
    let covered_strongest = candidates
        .iter()
        .filter(|candidate| {
            candidate.node != selected
                && candidate.node != body
                && is_descendant_of(dom, candidate.node, selected)
        })
        .max_by_key(|candidate| candidate.features.text_chars)
        .is_none_or(|candidate| {
            u64::from(root_features.text_chars).saturating_mul(100)
                >= u64::from(candidate.features.text_chars).saturating_mul(80)
        });
    let main = if is_main_semantic_root(dom, selected) {
        Some(selected)
    } else {
        dom.ancestors(selected).find(|&ancestor| {
            candidates.is_authoritative_semantic(dom, ancestor)
                && is_main_semantic_root(dom, ancestor)
        })
    };
    let has_article_peers = main.is_some_and(|main| article_branch_count(dom, main) >= 2);
    covered_strongest && !has_article_peers
}

pub(crate) fn is_authoritative_semantic_node(dom: &Dom, node: NodeId) -> bool {
    matches!(dom.tag(node), Some(Tag::Article | Tag::Main))
        || dom
            .attr(node, AttrName::Role)
            .is_some_and(|roles| matches_role(roles, "article") || matches_role(roles, "main"))
        || has_article_body_itemprop(dom, node)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RootSelectionReason {
    Ranked,
    SpecificChild,
    SharedParent,
    CompleteAncestor,
    StructuredData,
    ArticleBody,
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

    if reason != RootSelectionReason::StructuredData
        && candidates.has_article_body()
        && let Some(article_body) =
            preferred_article_body_root(dom, candidates, ranked, selected, body)
    {
        selected = article_body;
        reason = RootSelectionReason::ArticleBody;
    }

    // A semantic parent is the correct boundary when several strong branches
    // together form the useful page, such as article cards in a main element.
    // A schema match is already a high-confidence boundary, so do not broaden
    // it with unrelated peer candidates.
    let mut branches = SmallVec::new();
    if reason != RootSelectionReason::StructuredData {
        if let Some(parent) =
            shared_semantic_parent(dom, candidates, ranked, selected, body, first.score)
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
            if let Some((lead, content)) = lead_heading_branches(dom, selected, child)
                && !has_later_content_branch(dom, selected, content)
            {
                branches.extend([lead, content]);
                reason = RootSelectionReason::SharedParent;
            } else {
                selected = child;
                reason = RootSelectionReason::SpecificChild;
            }
        }

        if branches.is_empty()
            && !candidates
                .get(selected)
                .is_some_and(|candidate| candidate.has_source(CandidateSource::StructuredData))
            && let Some((ancestor, lead, content)) = dom
                .ancestors(selected)
                .take(4)
                .filter(|&ancestor| candidates.is_authoritative_semantic(dom, ancestor))
                .filter_map(|ancestor| {
                    lead_heading_branches(dom, ancestor, selected)
                        .map(|(lead, content)| (ancestor, lead, content))
                })
                .find(|&(ancestor, _, content)| !has_later_content_branch(dom, ancestor, content))
        {
            selected = ancestor;
            branches.extend([lead, content]);
            reason = RootSelectionReason::SharedParent;
        }

        if branches.is_empty()
            && !candidates
                .get(selected)
                .is_some_and(|candidate| candidate.has_source(CandidateSource::StructuredData))
            && let Some(boundary) =
                complete_semantic_boundary(dom, candidates, ranked, selected, body)
        {
            selected = boundary;
            branches.clear();
            reason = RootSelectionReason::CompleteAncestor;
        }

        if branches.is_empty()
            && !candidates
                .get(selected)
                .is_some_and(|candidate| candidate.has_source(CandidateSource::StructuredData))
            && let Some(boundary) = balanced_semantic_boundary(dom, candidates, selected)
        {
            reason = if is_descendant_of(dom, boundary, selected) {
                RootSelectionReason::SpecificChild
            } else {
                RootSelectionReason::CompleteAncestor
            };
            selected = boundary;
        }
    }

    // A compact code article can rank only its code wrappers as strong
    // branches. Keep the article when it has one lead heading and no semantic
    // subsection that could contain unrelated content.
    if !branches.is_empty()
        && candidates.is_authoritative_semantic(dom, selected)
        && candidates.get(selected).is_some_and(|candidate| {
            candidate.features.code_block_count > 0
                && candidate.features.heading_count == 1
                && candidate.features.link_density <= 0.2
                && candidate.features.text_chars <= 10_000
        })
        && dom.element_children(selected).next().is_some_and(|child| {
            matches!(
                dom.tag(child),
                Some(Tag::H1 | Tag::H2 | Tag::H3 | Tag::H4 | Tag::H5 | Tag::H6)
            )
        })
        && dom.element_children(selected).all(|child| {
            matches!(
                dom.tag(child),
                Some(Tag::H1 | Tag::H2 | Tag::H3 | Tag::H4 | Tag::H5 | Tag::H6 | Tag::P | Tag::Pre)
            ) || !matches!(
                dom.tag(child),
                Some(Tag::Article | Tag::Aside | Tag::Main | Tag::Nav | Tag::Section)
            ) && dom
                .descendants(child)
                .any(|descendant| dom.tag(descendant) == Some(Tag::Pre))
        })
        && branches.iter().all(|&branch| {
            !std::iter::once(branch)
                .chain(dom.descendants(branch))
                .any(|node| {
                    matches!(
                        dom.tag(node),
                        Some(Tag::H1 | Tag::H2 | Tag::H3 | Tag::H4 | Tag::H5 | Tag::H6)
                    )
                })
        })
    {
        branches.clear();
        reason = RootSelectionReason::CompleteAncestor;
    }

    if has_article_body_itemprop(dom, selected) {
        reason = RootSelectionReason::ArticleBody;
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

fn preferred_article_body_root(
    dom: &Dom,
    candidates: &CandidateSet,
    _ranked: &[RankedCandidate],
    selected: NodeId,
    body: NodeId,
) -> Option<NodeId> {
    let selected_chars = candidates
        .get(selected)
        .map_or(0, |candidate| candidate.features.text_chars);
    candidates
        .iter()
        .filter(|candidate| {
            has_article_body_itemprop(dom, candidate.node)
                && (selected == body
                    || candidate.node == selected
                    || is_descendant_of(dom, selected, candidate.node)
                    || is_descendant_of(dom, candidate.node, selected))
        })
        .filter_map(|candidate| {
            let features = candidate.features;
            let coherent = features.text_chars >= 120
                && (features.sentence_end_count > 0 || features.structured_block_count() > 0)
                && features.link_density <= 0.4;
            let complete_enough = selected_chars == 0
                || u64::from(features.text_chars).saturating_mul(100)
                    >= u64::from(selected_chars).saturating_mul(50);
            (coherent && complete_enough).then_some((candidate.node, features.text_chars))
        })
        .min_by_key(|(_, text_chars)| *text_chars)
        .map(|(node, _)| node)
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
    body: NodeId,
    top_score: f64,
) -> Option<NodeId> {
    dom.ancestors(selected).find(|&parent| {
        parent != body
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

/// Returns a semantic boundary that contains a complete document even when
/// ranking promoted one of its code, table, or prose descendants. The
/// predicate uses the candidate features already calculated for this attempt.
/// It does not scan or serialize the source DOM.
fn complete_semantic_boundary(
    dom: &Dom,
    candidates: &CandidateSet,
    ranked: &[RankedCandidate],
    selected: NodeId,
    body: NodeId,
) -> Option<NodeId> {
    if is_article_semantic_node(dom, selected) {
        return None;
    }
    let body_text_chars = candidates
        .get(body)
        .map_or(0, |candidate| candidate.features.text_chars);
    let roots: SmallVec<[NodeId; 8]> = dom
        .ancestors(selected)
        .take(8)
        .filter(|&root| candidates.is_authoritative_semantic(dom, root))
        .filter(|&root| {
            candidates.get(root).is_some_and(|candidate| {
                let features = candidate.features;
                features.code_block_count >= 2 || features.table_count >= 1
            })
        })
        .collect();
    if roots.is_empty() {
        return None;
    }

    let independent_article_counts: SmallVec<[usize; 8]> = roots
        .iter()
        .map(|&root| {
            if is_main_semantic_root(dom, root) {
                article_branch_count(dom, root)
            } else {
                0
            }
        })
        .collect();

    roots
        .into_iter()
        .enumerate()
        .filter_map(|(root_index, root)| {
            let features = candidates.get(root)?.features;
            let structural_content = features.code_block_count >= 2 || features.table_count >= 1;
            let coherent_document = features.heading_count >= 2
                && (features.paragraph_count >= 2 || structural_content)
                || structural_content && features.heading_count >= 1;
            let meaningful_text = features.text_chars >= 500 && features.word_count >= 30;
            let page_coverage = body_text_chars == 0
                || u64::from(features.text_chars).saturating_mul(100)
                    >= u64::from(body_text_chars).saturating_mul(35);
            let link_share_is_reasonable =
                features.link_density <= 0.45 || structural_content && features.link_density <= 0.6;
            let strongest_candidate_is_covered = ranked
                .first()
                .and_then(|candidate| candidates.get(candidate.node))
                .is_none_or(|candidate| {
                    u64::from(features.text_chars).saturating_mul(100)
                        >= u64::from(candidate.features.text_chars).saturating_mul(80)
                });
            let main_has_article_structure =
                !is_main_semantic_root(dom, root) || independent_article_counts[root_index] > 0;
            (meaningful_text
                && coherent_document
                && page_coverage
                && link_share_is_reasonable
                && strongest_candidate_is_covered
                && main_has_article_structure
                && independent_article_counts[root_index] < 2)
                .then_some(root)
        })
        .next()
}

fn is_main_semantic_root(dom: &Dom, node: NodeId) -> bool {
    dom.tag(node) == Some(Tag::Main)
        || dom
            .attr(node, AttrName::Role)
            .is_some_and(|roles| matches_role(roles, "main"))
}

fn is_article_semantic_node(dom: &Dom, node: NodeId) -> bool {
    dom.tag(node) == Some(Tag::Article)
        || dom
            .attr(node, AttrName::Role)
            .is_some_and(|roles| matches_role(roles, "article"))
}

/// Counts independent article branches under a main-like root. Counting the
/// direct branches, instead of every nested article node, protects listings
/// while allowing an article to contain embedded article-shaped markup.
fn article_branch_count(dom: &Dom, root: NodeId) -> usize {
    dom.element_children(root)
        .filter(|&branch| {
            std::iter::once(branch)
                .chain(dom.descendants(branch))
                .any(|node| is_article_semantic_node(dom, node))
        })
        .take(2)
        .count()
}

/// Corrects local ranking when a semantic boundary has clear completeness or
/// precision evidence. The thresholds are intentionally asymmetric: broadening
/// requires substantial omitted content, while narrowing requires almost all
/// useful text plus structural chrome outside the descendant.
fn balanced_semantic_boundary(
    dom: &Dom,
    candidates: &CandidateSet,
    selected: NodeId,
) -> Option<NodeId> {
    let selected_features = candidates.get(selected)?.features;
    if has_article_body_itemprop(dom, selected) {
        return None;
    }

    for ancestor in dom.ancestors(selected).take(4) {
        if !candidates.is_authoritative_semantic(dom, ancestor)
            || has_article_body_itemprop(dom, ancestor)
        {
            continue;
        }
        let independent_articles = candidates
            .iter()
            .filter(|candidate| {
                candidate.node != ancestor
                    && is_descendant_of(dom, candidate.node, ancestor)
                    && (dom.tag(candidate.node) == Some(Tag::Article)
                        || dom
                            .attr(candidate.node, AttrName::Role)
                            .is_some_and(|roles| {
                                roles
                                    .split_whitespace()
                                    .any(|role| role.eq_ignore_ascii_case("article"))
                            }))
            })
            .count();
        if independent_articles >= 2 {
            continue;
        }
        let ancestor_features = candidates.get(ancestor)?.features;
        let substantially_broader =
            ancestor_features.text_chars >= selected_features.text_chars.saturating_mul(3) / 2;
        let heterogeneous_content = ancestor_features.structured_block_count()
            >= selected_features.structured_block_count().saturating_add(2);
        let omitted_sections = ancestor_features.paragraph_count
            >= selected_features.paragraph_count.saturating_add(2)
            || ancestor_features.heading_count >= selected_features.heading_count.saturating_add(2)
            || heterogeneous_content;
        let nested_in_branch = direct_branch(dom, selected, ancestor) != Some(selected);
        let clean = ancestor_features.link_density <= 0.4
            && ancestor_features.negative_name_score
                <= selected_features.negative_name_score + 25.0;
        if substantially_broader
            && omitted_sections
            && clean
            && (nested_in_branch || heterogeneous_content)
        {
            return Some(ancestor);
        }
    }

    candidates
        .iter()
        .filter(|candidate| {
            candidate.node != selected
                && candidates.is_authoritative_semantic(dom, candidate.node)
                && is_descendant_of(dom, candidate.node, selected)
        })
        .filter_map(|candidate| {
            let outside = selected_features
                .text_chars
                .saturating_sub(candidate.features.text_chars);
            let preserves_content = candidate.features.text_chars.saturating_mul(100)
                >= selected_features.text_chars.saturating_mul(70)
                && outside <= 300;
            (preserves_content
                && has_chrome_outside_descendant(dom, selected, candidate.node)
                && lead_heading_branches(dom, selected, candidate.node).is_none())
            .then_some((candidate.node, candidate.features.text_chars))
        })
        .min_by_key(|(_, chars)| *chars)
        .map(|(node, _)| node)
}

impl CandidateFeatures {
    fn structured_block_count(self) -> u32 {
        self.list_item_count
            .min(1)
            .saturating_add(self.code_block_count)
            .saturating_add(self.table_count)
            .saturating_add(self.figure_count)
    }
}

fn has_chrome_outside_descendant(dom: &Dom, ancestor: NodeId, descendant: NodeId) -> bool {
    let Some(content_branch) = direct_branch(dom, descendant, ancestor) else {
        return false;
    };
    dom.element_children(ancestor).any(|branch| {
        branch != content_branch
            && (matches!(dom.tag(branch), Some(Tag::Nav | Tag::Footer))
                || dom.attr(branch, AttrName::Role).is_some_and(|roles| {
                    roles.split_whitespace().any(|role| {
                        matches!(role.to_ascii_lowercase().as_str(), "navigation" | "status")
                    })
                })
                || node_tokens(dom, branch).iter().any(|token| {
                    matches!(
                        token.as_str(),
                        "breadcrumb" | "breadcrumbs" | "feedback" | "toolbar" | "actions"
                    )
                }))
    })
}

fn lead_heading_branches(
    dom: &Dom,
    ancestor: NodeId,
    descendant: NodeId,
) -> Option<(NodeId, NodeId)> {
    let branch = direct_branch(dom, descendant, ancestor)?;
    if starts_with_heading(dom, branch) {
        return None;
    }
    let previous = previous_substantive_element(dom, branch)?;
    let heading = dom
        .descendants(previous)
        .find(|&node| crate::normalize::heading_level(dom, node).is_some())?;
    let heading_chars = dom.normalized_char_count(heading);
    let branch_chars = dom.normalized_char_count(previous);
    let auxiliary_chars = branch_chars.saturating_sub(heading_chars);
    if dom.parent(previous) != Some(ancestor)
        || heading_chars == 0
        || auxiliary_chars > 80
        || branch_chars > heading_chars.saturating_mul(3).saturating_add(40)
    {
        return None;
    }
    let clutter = matches!(dom.tag(previous), Some(Tag::Nav | Tag::Aside | Tag::Footer))
        || node_tokens(dom, previous).iter().any(|token| {
            matches!(
                token.as_str(),
                "related" | "recommended" | "navigation" | "breadcrumb" | "advertisement"
            )
        })
        || leading_text(dom, previous).is_some_and(|text| {
            ["related", "recommended", "more stories"]
                .iter()
                .any(|prefix| text.starts_with(prefix))
        });
    (!clutter
        && dom
            .descendants(previous)
            .filter(|&node| dom.tag(node) == Some(Tag::A))
            .count()
            <= 1)
        .then_some((previous, branch))
}

fn has_later_content_branch(dom: &Dom, ancestor: NodeId, branch: NodeId) -> bool {
    let mut sibling = dom.next_sibling(branch);
    while let Some(node) = sibling {
        let chrome = matches!(dom.tag(node), Some(Tag::Nav | Tag::Aside | Tag::Footer))
            || node_tokens(dom, node).iter().any(|token| {
                matches!(
                    token.as_str(),
                    "related" | "recommended" | "feedback" | "toolbar"
                )
            });
        let content_structure =
            std::iter::once(node)
                .chain(dom.descendants(node))
                .any(|descendant| {
                    matches!(
                        dom.tag(descendant),
                        Some(
                            Tag::P
                                | Tag::H1
                                | Tag::H2
                                | Tag::H3
                                | Tag::H4
                                | Tag::H5
                                | Tag::H6
                                | Tag::Pre
                                | Tag::Table
                                | Tag::Dl
                                | Tag::Ol
                                | Tag::Ul
                        )
                    )
                });
        if dom.is_element(node)
            && !chrome
            && (content_structure || dom.normalized_char_count(node) >= 80)
        {
            return true;
        }
        sibling = dom.next_sibling(node);
    }
    dom.parent(branch) != Some(ancestor)
}

fn starts_with_heading(dom: &Dom, branch: NodeId) -> bool {
    std::iter::once(branch)
        .chain(dom.descendants(branch))
        .find(|&node| {
            dom.is_element(node)
                && dom.tag(node) != Some(Tag::Div)
                && dom.tag(node) != Some(Tag::Section)
                && dom.tag(node) != Some(Tag::Header)
        })
        .is_some_and(|node| crate::normalize::heading_level(dom, node).is_some())
}

fn previous_substantive_element(dom: &Dom, node: NodeId) -> Option<NodeId> {
    let mut previous = dom.prev_sibling(node);
    while let Some(candidate) = previous {
        if dom.is_element(candidate) {
            return Some(candidate);
        }
        if dom
            .text_node(candidate)
            .is_some_and(|text| !text.trim().is_empty())
        {
            return None;
        }
        previous = dom.prev_sibling(candidate);
    }
    None
}

fn node_tokens(dom: &Dom, node: NodeId) -> Vec<String> {
    [AttrName::Class, AttrName::Id]
        .into_iter()
        .filter_map(|name| dom.attr(node, name))
        .flat_map(|value| value.split(|character: char| !character.is_ascii_alphanumeric()))
        .filter(|token| !token.is_empty())
        .map(str::to_ascii_lowercase)
        .collect()
}

fn leading_text(dom: &Dom, node: NodeId) -> Option<String> {
    let mut text = String::new();
    for descendant in std::iter::once(node).chain(dom.descendants(node)) {
        if let Some(value) = dom.text_node(descendant) {
            text.push_str(value);
            if text.len() >= 64 {
                break;
            }
        }
    }
    let text = text.trim().to_ascii_lowercase();
    (!text.is_empty()).then_some(text)
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

/// Finds a compact DOM root that agrees with schema `articleBody` or `text`.
///
/// The phrase scan limits expensive subtree comparisons to ancestors of
/// visible text that contains the start of a schema value. It can match text
/// split across inline elements.
pub(crate) fn locate_structured_content<'a>(
    dom: &Dom,
    source: &SourceAnalysis,
    texts: impl IntoIterator<Item = &'a str>,
) -> Option<NodeId> {
    let hints: Vec<_> = texts
        .into_iter()
        .filter_map(StructuredHint::new)
        .take(MAX_STRUCTURED_HINTS)
        .collect();
    if hints.is_empty() {
        return None;
    }

    let scan_context = PhraseScanContext::new(dom, source);
    let mut possible = SmallVec::<[NodeId; 32]>::new();
    for hint in &hints {
        let phrase_len = hint.tokens.len().min(6);
        if phrase_len < 5 {
            continue;
        }
        for matched_root in
            visible_phrase_matches(dom, source, &scan_context, &hint.tokens[..phrase_len])
        {
            if is_navigation_region(dom, matched_root) {
                continue;
            }
            for ancestor in std::iter::once(matched_root).chain(dom.ancestors(matched_root)) {
                if dom.is_element(ancestor) && !possible.contains(&ancestor) {
                    possible.push(ancestor);
                }
            }
        }
    }

    possible
        .into_iter()
        .filter(|&node| !is_navigation_region(dom, node))
        .filter_map(|node| {
            let agreement = structured_agreement(dom, node, &hints);
            (agreement >= 0.55).then(|| {
                (
                    node,
                    dom.normalized_char_count_below(node, MAX_CANDIDATE_MATCH_CHARS)
                        .unwrap_or(MAX_CANDIDATE_MATCH_CHARS),
                    agreement,
                )
            })
        })
        .min_by(|left, right| {
            // Prefer the smallest root among similarly strong matches. If one
            // match is substantially better, preserve the better coverage.
            if (left.2 - right.2).abs() <= 0.08 {
                left.1.cmp(&right.1)
            } else {
                right.2.total_cmp(&left.2)
            }
        })
        .map(|(node, _, _)| node)
}

struct PhraseScanContext {
    blocked: Vec<bool>,
    flow_root: Vec<Option<NodeId>>,
}

impl PhraseScanContext {
    fn new(dom: &Dom, source: &SourceAnalysis) -> Self {
        let mut blocked = vec![false; dom.len()];
        let mut flow_root = vec![None; dom.len()];
        for entry in source.elements() {
            let node = entry.node;
            let parent = dom.parent(node);
            let parent_blocked = parent.is_some_and(|parent| blocked[parent.index()]);
            blocked[node.index()] = parent_blocked || is_blocked_structured_region(dom, node);
            flow_root[node.index()] = if is_text_flow_boundary(dom.tag(node)) {
                Some(node)
            } else {
                parent.and_then(|parent| flow_root[parent.index()])
            };
        }
        Self { blocked, flow_root }
    }
}

fn visible_phrase_matches(
    dom: &Dom,
    source: &SourceAnalysis,
    context: &PhraseScanContext,
    phrase: &[String],
) -> SmallVec<[NodeId; 4]> {
    const MAX_MATCHES: usize = 32;
    let mut roots = SmallVec::new();
    let mut matched = 0;
    let mut start = None;
    let mut flow_root = None;
    for entry in source
        .entries_in(source.anchors.root)
        .filter(|entry| dom.is_text(entry.node))
    {
        let node = entry.node;
        let parent = dom.parent(node);
        let current_flow_root = parent.and_then(|parent| context.flow_root[parent.index()]);
        if flow_root.is_some() && current_flow_root != flow_root {
            matched = 0;
            start = None;
        }
        flow_root = current_flow_root;
        if parent.is_some_and(|parent| context.blocked[parent.index()]) {
            matched = 0;
            start = None;
            continue;
        }

        let text = dom.text_node(node).unwrap_or_default().to_lowercase();
        for token in split_word_tokens(&text) {
            if token == phrase[matched] {
                if matched == 0 {
                    start = Some(node);
                }
                matched += 1;
                if matched == phrase.len() {
                    if let Some(start) = start {
                        let end_ancestors: HashSet<_> = dom.ancestors(node).collect();
                        if let Some(root) = dom
                            .ancestors(start)
                            .find(|ancestor| end_ancestors.contains(ancestor))
                            && !roots.contains(&root)
                        {
                            roots.push(root);
                            if roots.len() == MAX_MATCHES {
                                return roots;
                            }
                        }
                    }
                    matched = 0;
                    start = None;
                }
            } else if token == phrase[0] {
                matched = 1;
                start = Some(node);
            } else {
                matched = 0;
                start = None;
            }
        }
    }
    roots
}

fn is_blocked_structured_region(dom: &Dom, node: NodeId) -> bool {
    matches!(
        dom.tag(node),
        Some(Tag::Script | Tag::Style | Tag::Template | Tag::Meta | Tag::Link)
    ) || has_static_hidden_marker(dom, node)
        || dom.attr(node, AttrName::AriaHidden) == Some("true")
}

fn is_text_flow_boundary(tag: Option<Tag>) -> bool {
    matches!(
        tag,
        Some(
            Tag::Address
                | Tag::Article
                | Tag::Blockquote
                | Tag::Body
                | Tag::Details
                | Tag::Div
                | Tag::Figcaption
                | Tag::Figure
                | Tag::Footer
                | Tag::H1
                | Tag::H2
                | Tag::H3
                | Tag::H4
                | Tag::H5
                | Tag::H6
                | Tag::Header
                | Tag::Li
                | Tag::Main
                | Tag::Nav
                | Tag::P
                | Tag::Pre
                | Tag::Section
                | Tag::Summary
                | Tag::Td
                | Tag::Th
        )
    )
}

fn is_navigation_region(dom: &Dom, node: NodeId) -> bool {
    std::iter::once(node)
        .chain(dom.ancestors(node))
        .any(|ancestor| {
            matches!(dom.tag(ancestor), Some(Tag::A | Tag::Nav))
                || dom.attr(ancestor, AttrName::Role).is_some_and(|roles| {
                    roles
                        .split_whitespace()
                        .any(|role| role.eq_ignore_ascii_case("navigation"))
                })
        })
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
    fn ranking_context_relationships_are_candidate_sized() {
        let dom = Dom::parse_document(
            r#"<body><main id="main"><div class="wrapper"><article><p>First article content.</p></article><article><p>Second article content.</p></article></div><p>Not a candidate.</p></main></body>"#,
        )
        .unwrap();
        let main = dom
            .descendants(dom.root())
            .find(|&node| dom.attr(node, AttrName::Id) == Some("main"))
            .unwrap();
        let articles: Vec<_> = dom
            .descendants(dom.root())
            .filter(|&node| dom.tag(node) == Some(Tag::Article))
            .collect();
        let mut candidates = CandidateSet::discover_semantic(&dom);
        for (index, &article) in articles.iter().enumerate() {
            candidates.add_readability(article, 10.0 + index as f64);
        }

        let snapshot = dom.element_descendants_snapshot_with_depth(dom.root());
        let mut store = NodeStateStore::new();
        for candidate in candidates.iter() {
            crate::scoring::get_or_compute_stats(&dom, candidate.node, &mut store);
        }
        let context = candidates.ranking_context(&dom, &store, &snapshot);

        assert_eq!(
            context.readability_in_subtree.len(),
            candidates.candidates.len()
        );
        assert_eq!(
            context.has_authoritative_ancestor.len(),
            candidates.candidates.len()
        );
        assert_eq!(
            context.authoritative_count.len(),
            candidates.candidates.len()
        );
        assert_eq!(
            context.article_peer_count.len(),
            candidates.candidates.len()
        );
        assert_eq!(
            context.article_peer_score.len(),
            candidates.candidates.len()
        );
        assert_eq!(
            std::mem::size_of_val(&candidates.positions[0]),
            std::mem::size_of::<u32>()
        );

        let main_index = candidates.candidate_index(main).unwrap();
        assert!(context.has_readability(main_index));
        assert_eq!(context.article_peer_summary(main_index).0, 2);
        for &article in &articles {
            let article_index = candidates.candidate_index(article).unwrap();
            assert!(context.has_authoritative_ancestor(article_index));
            assert!(!context.has_authoritative_descendant(article_index, true));
        }

        let legacy = legacy_ranking_relationships(&dom, &store, &snapshot, &candidates);
        for (index, candidate) in candidates.candidates.iter().enumerate() {
            let node = candidate.node.index();
            assert_eq!(context.has_readability(index), legacy.0[node]);
            assert_eq!(context.has_authoritative_ancestor(index), legacy.1[node]);
            assert_eq!(
                context.has_authoritative_descendant(index, false),
                legacy.2[node] > 0
            );
            assert_eq!(
                context.article_peer_summary(index),
                (legacy.3[node], legacy.4[node])
            );
        }
    }

    #[test]
    fn equal_candidate_scores_keep_source_order() {
        let dom = Dom::parse_document(
            r#"<body><main><div><section id="first"><p>Equal source text.</p></section></div><div><section id="second"><p>Equal source text.</p></section></div></main></body>"#,
        )
        .unwrap();
        let first = dom
            .descendants(dom.root())
            .find(|&node| dom.attr(node, AttrName::Id) == Some("first"))
            .unwrap();
        let second = dom
            .descendants(dom.root())
            .find(|&node| dom.attr(node, AttrName::Id) == Some("second"))
            .unwrap();
        let mut candidates = CandidateSet::discover_semantic(&dom);
        candidates.add_readability(first, 10.0);
        candidates.add_readability(second, 10.0);

        let first_index = candidates.candidate_index(first).unwrap();
        let second_index = candidates.candidate_index(second).unwrap();
        assert!(first_index < second_index);
        assert_eq!(
            candidates
                .iter()
                .filter(|candidate| candidate.readability_score == 10.0)
                .map(|candidate| candidate.node)
                .collect::<Vec<_>>(),
            [first, second]
        );
    }

    type LegacyRelationships = (Vec<bool>, Vec<bool>, Vec<u32>, Vec<u32>, Vec<f64>);

    fn legacy_ranking_relationships(
        dom: &Dom,
        store: &NodeStateStore,
        nodes: &[(NodeId, u32)],
        candidates: &CandidateSet,
    ) -> LegacyRelationships {
        let mut readability_in_subtree = vec![false; dom.len()];
        let mut authoritative_count = vec![0_u32; dom.len()];
        for candidate in &candidates.candidates {
            readability_in_subtree[candidate.node.index()] =
                candidate.has_source(CandidateSource::Readability);
            if candidates.is_authoritative_semantic(dom, candidate.node) {
                authoritative_count[candidate.node.index()] = 1;
            }
        }
        for &(node, _) in nodes.iter().rev() {
            if let Some(parent) = dom.parent(node) {
                readability_in_subtree[parent.index()] |= readability_in_subtree[node.index()];
                authoritative_count[parent.index()] = authoritative_count[parent.index()]
                    .saturating_add(authoritative_count[node.index()]);
            }
        }

        let mut nearest_authoritative_ancestor = vec![None; dom.len()];
        for &(node, _) in nodes {
            if let Some(parent) = dom.parent(node) {
                nearest_authoritative_ancestor[node.index()] =
                    if candidates.is_authoritative_semantic(dom, parent) {
                        Some(parent)
                    } else {
                        nearest_authoritative_ancestor[parent.index()]
                    };
            }
        }

        let mut article_peer_count = vec![0_u32; dom.len()];
        let mut article_peer_score = vec![0.0; dom.len()];
        for candidate in &candidates.candidates {
            let is_article = dom.tag(candidate.node) == Some(Tag::Article)
                || dom
                    .attr(candidate.node, AttrName::Role)
                    .is_some_and(|role| matches_role(role, "article"));
            let Some(parent) = nearest_authoritative_ancestor[candidate.node.index()] else {
                continue;
            };
            if is_article
                && dom.parent(candidate.node).is_some()
                && store
                    .get_stats(candidate.node)
                    .is_some_and(|stats| stats.has_non_whitespace())
            {
                article_peer_count[parent.index()] += 1;
                article_peer_score[parent.index()] += candidate.readability_score;
            }
        }

        (
            readability_in_subtree,
            nearest_authoritative_ancestor
                .iter()
                .map(Option::is_some)
                .collect(),
            authoritative_count,
            article_peer_count,
            article_peer_score,
        )
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
    fn article_body_itemprop_is_a_token_aware_semantic_root() {
        let html = r#"<body>
            <header><nav>News Reviews Deals Forums</nav></header>
            <main class="content-shell">
                <article>
                    <h1>How to choose a quiet laptop fan</h1>
                    <p class="byline">By Example Writer</p>
                    <section itemprop="author articleBody">
                        <p>A quiet laptop fan reduces noise during long compilation jobs.</p>
                        <p>We tested three fan profiles with the same processor, memory, and room temperature.</p>
                        <h2>What we measured</h2>
                        <p>The balanced profile lowered the measured noise without reducing sustained performance.</p>
                        <p>The result remained consistent after six hours of repeated builds.</p>
                    </section>
                </article>
                <div itemprop="notArticleBody"><p>Unrelated metadata must not become the article.</p></div>
                <aside>Recommended laptops Buying guides Subscribe to our newsletter</aside>
            </main>
            <footer>Privacy Terms Contact</footer>
        </body></html>"#;
        let dom = Dom::parse_document(html).unwrap();
        let body = dom.body().unwrap();
        let article_body = dom
            .descendants(body)
            .find(|&node| dom.attr(node, AttrName::ItemProp) == Some("author articleBody"))
            .unwrap();
        let unrelated = dom
            .descendants(body)
            .find(|&node| dom.attr(node, AttrName::ItemProp) == Some("notArticleBody"))
            .unwrap();
        let candidates = CandidateSet::discover_semantic(&dom);

        assert!(candidates.is_semantic(article_body));
        assert!(candidates.is_authoritative_semantic(&dom, article_body));
        assert!(!candidates.is_semantic(unrelated));

        let markdown = crate::extract(html, None).unwrap().markdown();
        assert!(
            markdown.contains("A quiet laptop fan reduces noise"),
            "{markdown}"
        );
        assert!(
            markdown.contains("six hours of repeated builds"),
            "{markdown}"
        );
        assert!(!markdown.contains("Unrelated metadata"), "{markdown}");
        assert!(!markdown.contains("Recommended laptops"), "{markdown}");
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
    fn root_selection_promotes_a_complete_semantic_ancestor() {
        let dom = Dom::parse_document(
            r#"<body><article id="complete"><section id="focused"><h2>Focused section</h2><p>Polished prose.</p></section><section><h2>Code</h2><pre>one\ntwo</pre></section><section><h2>Data</h2><table><tr><th>Value</th></tr></table></section></article></body>"#,
        )
        .unwrap();
        let complete = dom
            .descendants(dom.root())
            .find(|&node| dom.attr(node, AttrName::Id) == Some("complete"))
            .unwrap();
        let focused = dom
            .descendants(dom.root())
            .find(|&node| dom.attr(node, AttrName::Id) == Some("focused"))
            .unwrap();
        let mut candidates = CandidateSet::discover_semantic(&dom);
        candidates.add_readability(focused, 100.0);
        candidates
            .iter_mut()
            .find(|candidate| candidate.node == complete)
            .unwrap()
            .features = CandidateFeatures {
            text_chars: 900,
            paragraph_count: 6,
            heading_count: 4,
            code_block_count: 1,
            table_count: 1,
            link_density: 0.05,
            ..CandidateFeatures::default()
        };
        candidates
            .iter_mut()
            .find(|candidate| candidate.node == focused)
            .unwrap()
            .features = CandidateFeatures {
            text_chars: 300,
            paragraph_count: 2,
            heading_count: 1,
            link_density: 0.0,
            ..CandidateFeatures::default()
        };
        let selected = select_content_root(
            &dom,
            &candidates,
            &[RankedCandidate {
                node: focused,
                score: 100.0,
                order: 0,
            }],
            dom.body().unwrap(),
            [],
        );
        assert_eq!(selected.node, complete);
    }

    #[test]
    fn completeness_keeps_a_large_technical_root_with_linked_structure() {
        let dom = Dom::parse_document(
            r#"<body><article id="complete"><div id="focused"><h2>Focused API</h2><p>Generated reference details.</p><pre>example()</pre></div></article></body>"#,
        )
        .unwrap();
        let complete = dom
            .descendants(dom.root())
            .find(|&node| dom.attr(node, AttrName::Id) == Some("complete"))
            .unwrap();
        let focused = dom
            .descendants(dom.root())
            .find(|&node| dom.attr(node, AttrName::Id) == Some("focused"))
            .unwrap();
        let body = dom.body().unwrap();
        let mut candidates = CandidateSet::discover_semantic(&dom);
        candidates.add_readability(focused, 100.0);
        candidates
            .iter_mut()
            .find(|candidate| candidate.node == body)
            .unwrap()
            .features
            .text_chars = 3_000;
        candidates
            .iter_mut()
            .find(|candidate| candidate.node == complete)
            .unwrap()
            .features = CandidateFeatures {
            text_chars: 2_000,
            word_count: 500,
            paragraph_count: 4,
            heading_count: 3,
            code_block_count: 2,
            link_density: 0.55,
            ..CandidateFeatures::default()
        };
        candidates
            .iter_mut()
            .find(|candidate| candidate.node == focused)
            .unwrap()
            .features = CandidateFeatures {
            text_chars: 300,
            word_count: 60,
            paragraph_count: 1,
            heading_count: 1,
            link_density: 0.0,
            ..CandidateFeatures::default()
        };

        let selected = select_content_root(
            &dom,
            &candidates,
            &[RankedCandidate {
                node: focused,
                score: 100.0,
                order: 0,
            }],
            body,
            [],
        );
        assert_eq!(selected.node, complete);
        assert_eq!(selected.reason, RootSelectionReason::CompleteAncestor);
    }

    #[test]
    fn completeness_does_not_merge_peer_articles() {
        let dom = Dom::parse_document(
            r#"<body><main id="listing"><div id="focused"><h2>Focused reference</h2><p>Generated reference details.</p><pre>example()</pre></div><article><h2>First card</h2><p>First article summary.</p></article><article><h2>Second card</h2><p>Second article summary.</p></article></main></body>"#,
        )
        .unwrap();
        let listing = dom
            .descendants(dom.root())
            .find(|&node| dom.attr(node, AttrName::Id) == Some("listing"))
            .unwrap();
        let focused = dom
            .descendants(dom.root())
            .find(|&node| dom.attr(node, AttrName::Id) == Some("focused"))
            .unwrap();
        let body = dom.body().unwrap();
        let mut candidates = CandidateSet::discover_semantic(&dom);
        candidates.add_readability(focused, 100.0);
        candidates
            .iter_mut()
            .find(|candidate| candidate.node == body)
            .unwrap()
            .features
            .text_chars = 3_000;
        candidates
            .iter_mut()
            .find(|candidate| candidate.node == listing)
            .unwrap()
            .features = CandidateFeatures {
            text_chars: 2_000,
            word_count: 500,
            paragraph_count: 4,
            heading_count: 3,
            code_block_count: 2,
            link_density: 0.1,
            ..CandidateFeatures::default()
        };
        candidates
            .iter_mut()
            .find(|candidate| candidate.node == focused)
            .unwrap()
            .features = CandidateFeatures {
            text_chars: 300,
            word_count: 60,
            paragraph_count: 1,
            heading_count: 1,
            link_density: 0.0,
            ..CandidateFeatures::default()
        };

        let selected = select_content_root(
            &dom,
            &candidates,
            &[RankedCandidate {
                node: focused,
                score: 100.0,
                order: 0,
            }],
            body,
            [],
        );
        assert_eq!(selected.node, focused);
    }

    #[test]
    fn completeness_does_not_merge_two_article_cards_when_one_wins() {
        let dom = Dom::parse_document(
            r#"<body><main id="listing"><article id="first"><h2>First card</h2><p>First article summary.</p><pre>example()</pre></article><article id="second"><h2>Second card</h2><p>Second article summary.</p></article></main></body>"#,
        )
        .unwrap();
        let listing = dom
            .descendants(dom.root())
            .find(|&node| dom.attr(node, AttrName::Id) == Some("listing"))
            .unwrap();
        let first = dom
            .descendants(dom.root())
            .find(|&node| dom.attr(node, AttrName::Id) == Some("first"))
            .unwrap();
        let body = dom.body().unwrap();
        let mut candidates = CandidateSet::discover_semantic(&dom);
        candidates.add_readability(first, 100.0);
        candidates
            .iter_mut()
            .find(|candidate| candidate.node == body)
            .unwrap()
            .features
            .text_chars = 3_000;
        candidates
            .iter_mut()
            .find(|candidate| candidate.node == listing)
            .unwrap()
            .features = CandidateFeatures {
            text_chars: 2_000,
            word_count: 500,
            paragraph_count: 4,
            heading_count: 3,
            code_block_count: 2,
            link_density: 0.1,
            ..CandidateFeatures::default()
        };
        candidates
            .iter_mut()
            .find(|candidate| candidate.node == first)
            .unwrap()
            .features = CandidateFeatures {
            text_chars: 300,
            word_count: 60,
            paragraph_count: 1,
            heading_count: 1,
            link_density: 0.0,
            ..CandidateFeatures::default()
        };

        let selected = select_content_root(
            &dom,
            &candidates,
            &[RankedCandidate {
                node: first,
                score: 100.0,
                order: 0,
            }],
            body,
            [],
        );
        assert_eq!(selected.node, first);
    }

    #[test]
    fn completeness_does_not_promote_a_chrome_only_main_root() {
        let dom = Dom::parse_document(
            r#"<body><main id="chrome"><p>From Wikipedia, the free encyclopedia.</p><div id="focused"><h2>Focused reference</h2><p>Generated reference details.</p><pre>example()</pre></div><section><h2>Data</h2><table><tr><th>Value</th></tr></table></section></main></body>"#,
        )
        .unwrap();
        let chrome = dom
            .descendants(dom.root())
            .find(|&node| dom.attr(node, AttrName::Id) == Some("chrome"))
            .unwrap();
        let focused = dom
            .descendants(dom.root())
            .find(|&node| dom.attr(node, AttrName::Id) == Some("focused"))
            .unwrap();
        let body = dom.body().unwrap();
        let mut candidates = CandidateSet::discover_semantic(&dom);
        candidates.add_readability(focused, 100.0);
        candidates
            .iter_mut()
            .find(|candidate| candidate.node == body)
            .unwrap()
            .features
            .text_chars = 3_000;
        candidates
            .iter_mut()
            .find(|candidate| candidate.node == chrome)
            .unwrap()
            .features = CandidateFeatures {
            text_chars: 2_000,
            word_count: 500,
            paragraph_count: 4,
            heading_count: 3,
            code_block_count: 2,
            table_count: 1,
            link_density: 0.1,
            ..CandidateFeatures::default()
        };
        candidates
            .iter_mut()
            .find(|candidate| candidate.node == focused)
            .unwrap()
            .features = CandidateFeatures {
            text_chars: 300,
            word_count: 60,
            paragraph_count: 1,
            heading_count: 1,
            code_block_count: 2,
            table_count: 1,
            link_density: 0.0,
            ..CandidateFeatures::default()
        };

        let selected = select_content_root(
            &dom,
            &candidates,
            &[RankedCandidate {
                node: focused,
                score: 100.0,
                order: 0,
            }],
            body,
            [],
        );
        assert_eq!(selected.node, focused);
    }

    #[test]
    fn root_selection_narrows_a_chrome_wrapper_to_its_article() {
        let dom = Dom::parse_document(
            r#"<body><main id="docs"><nav>Collection</nav><article id="answer"><p>Complete documented answer.</p></article><footer class="feedback">Was this useful?</footer></main></body>"#,
        )
        .unwrap();
        let docs = dom
            .descendants(dom.root())
            .find(|&node| dom.attr(node, AttrName::Id) == Some("docs"))
            .unwrap();
        let answer = dom
            .descendants(dom.root())
            .find(|&node| dom.attr(node, AttrName::Id) == Some("answer"))
            .unwrap();
        let mut candidates = CandidateSet::discover_semantic(&dom);
        candidates
            .iter_mut()
            .find(|candidate| candidate.node == docs)
            .unwrap()
            .features = CandidateFeatures {
            text_chars: 100,
            paragraph_count: 1,
            link_density: 0.1,
            ..CandidateFeatures::default()
        };
        candidates
            .iter_mut()
            .find(|candidate| candidate.node == answer)
            .unwrap()
            .features = CandidateFeatures {
            text_chars: 75,
            paragraph_count: 1,
            ..CandidateFeatures::default()
        };
        let selected = select_content_root(
            &dom,
            &candidates,
            &[RankedCandidate {
                node: docs,
                score: 100.0,
                order: 0,
            }],
            dom.body().unwrap(),
            [],
        );
        assert_eq!(selected.node, answer);
    }

    #[test]
    fn root_selection_keeps_an_adjacent_lead_heading() {
        let html = r#"<body><main id="broad"><section><h1>Important label</h1></section><section id="specific"><p>The complete details occupy almost all text in the parent boundary.</p></section></main></body>"#;
        assert_eq!(
            select_test_root(html, &[("broad", 100.0), ("specific", 90.0)], []),
            "broad"
        );
    }

    #[test]
    fn root_selection_keeps_an_adjacent_aria_lead_heading() {
        let html = r#"<body><main id="broad"><section><div role="heading" aria-level="1">Important label</div></section><section id="specific"><p>The complete details occupy almost all text in the parent boundary.</p></section></main></body>"#;
        assert_eq!(
            select_test_root(html, &[("broad", 100.0), ("specific", 90.0)], []),
            "broad"
        );
    }

    #[test]
    fn root_selection_does_not_attach_a_substantial_preceding_section() {
        let dom = Dom::parse_document(
            r#"<body><main id="broad"><section><h1>Earlier article</h1><p>This unrelated article has several complete sentences. It explains another subject in detail and must not be attached to the selected result.</p><p>The earlier conclusion adds more substantial prose outside its heading.</p></section><section id="specific"><p>The selected guide contains the primary answer with enough detail to dominate the parent text.</p></section></main></body>"#,
        )
        .unwrap();
        let broad = dom
            .descendants(dom.root())
            .find(|&node| dom.attr(node, AttrName::Id) == Some("broad"))
            .unwrap();
        let specific = dom
            .descendants(dom.root())
            .find(|&node| dom.attr(node, AttrName::Id) == Some("specific"))
            .unwrap();
        assert_eq!(lead_heading_branches(&dom, broad, specific), None);
    }

    #[test]
    fn lead_heading_boundary_keeps_a_later_conclusion() {
        let html = r#"<body><main id="broad"><section><h1>Guide label</h1></section><section id="specific"><p>The primary section contains detailed instructions, context, examples, and cautions. It explains each step fully so that this branch contains most of the wrapper text and remains the strongest candidate for extraction.</p></section><section><p>Final compatibility note.</p></section></main></body>"#;
        assert_eq!(
            select_test_root(html, &[("broad", 100.0), ("specific", 90.0)], []),
            "broad"
        );
    }

    #[test]
    fn ancestor_completion_does_not_merge_wrapped_articles() {
        let dom = Dom::parse_document(
            r#"<body><main id="listing"><div><article id="first"><p>First article.</p></article></div><div><article><p>Second article.</p></article></div></main></body>"#,
        )
        .unwrap();
        let listing = dom
            .descendants(dom.root())
            .find(|&node| dom.attr(node, AttrName::Id) == Some("listing"))
            .unwrap();
        let first = dom
            .descendants(dom.root())
            .find(|&node| dom.attr(node, AttrName::Id) == Some("first"))
            .unwrap();
        let mut candidates = CandidateSet::discover_semantic(&dom);
        candidates
            .iter_mut()
            .find(|candidate| candidate.node == listing)
            .unwrap()
            .features = CandidateFeatures {
            text_chars: 900,
            paragraph_count: 8,
            heading_count: 3,
            link_density: 0.1,
            ..CandidateFeatures::default()
        };
        candidates
            .iter_mut()
            .find(|candidate| candidate.node == first)
            .unwrap()
            .features = CandidateFeatures {
            text_chars: 300,
            paragraph_count: 2,
            link_density: 0.0,
            ..CandidateFeatures::default()
        };
        assert_eq!(balanced_semantic_boundary(&dom, &candidates, first), None);
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
    fn code_and_table_heavy_semantic_root_remains_complete() {
        let html = r#"<!doctype html>
            <html><body>
                <header><nav>Docs API Blog Community Search</nav></header>
                <main><article>
                    <h1>Working with grouped data</h1>
                    <p>Grouped data lets a report compare several measurements without copying the source rows.</p>
                    <p>The examples below use the same input table and preserve the original column names.</p>
                    <h2>Create a grouped result</h2>
                    <pre><code>result = frame.group_by("team").agg(
    total=pl.col("value").sum(),
    average=pl.col("value").mean(),
)</code></pre>
                    <p>The grouped result contains one row for each team and keeps the aggregate values numeric.</p>
                    <h2>Result columns</h2>
                    <table><thead><tr><th>Column</th><th>Meaning</th></tr></thead>
                        <tbody><tr><td>team</td><td>The grouping key from the source data.</td></tr>
                        <tr><td>total</td><td>The sum of values in the group.</td></tr>
                        <tr><td>average</td><td>The arithmetic mean of values in the group.</td></tr></tbody>
                    </table>
                    <h2>Filter the result</h2>
                    <pre><code>filtered = result.filter(pl.col("total") &gt; 10)</code></pre>
                    <p>Filtering after aggregation keeps the operation readable and avoids changing the input frame.</p>
                </article></main>
                <footer>Related tutorials Pricing Terms</footer>
            </body></html>"#;

        let markdown = crate::extract(html, None).unwrap().markdown();
        assert!(
            markdown.contains("Grouped data lets a report compare"),
            "{markdown}"
        );
        assert!(
            markdown.contains("Filtering after aggregation"),
            "{markdown}"
        );
        assert!(markdown.contains("Column"), "{markdown}");
        assert!(markdown.contains("Meaning"), "{markdown}");
        assert_eq!(markdown.matches("```").count(), 4, "{markdown}");
        assert!(!markdown.contains("Related tutorials"), "{markdown}");
        assert!(!markdown.contains("Pricing Terms"), "{markdown}");
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
    fn structured_location_ignores_json_ld_script_text_and_matches_inline_content() {
        let html = r#"<body>
            <nav><a href="/article">The split article contains exact useful words across inline elements.</a></nav>
            <script type="application/ld+json">{
                "@context":"https://schema.org",
                "@type":"Article",
                "articleBody":"The split article contains exact useful words across inline elements."
            }</script>
            <article id="wanted"><p>The split <strong>article contains</strong> exact useful words <a href="/more">across inline elements.</a></p></article>
        </body>"#;
        let dom = Dom::parse_document(html).unwrap();
        let article = dom
            .first_descendant_by_tag(dom.root(), Tag::Article)
            .unwrap();
        let source = SourceAnalysis::build(&dom);
        let root = locate_structured_content(
            &dom,
            &source,
            ["The split article contains exact useful words across inline elements."],
        )
        .unwrap();

        assert!(root == article || dom.ancestors(root).any(|ancestor| ancestor == article));
        assert_ne!(dom.tag(root), Some(Tag::Script));

        let page = crate::extract(html, Some("https://example.com/page")).unwrap();
        assert!(page.text().contains("split article contains"));
        assert!(!page.html().contains("application/ld+json"));
        assert!(!page.html().contains("<script"));
    }

    #[test]
    fn structured_location_does_not_join_unrelated_blocks() {
        let dom = Dom::parse_document(
            "<body><header>alpha beta gamma</header><article><p>delta epsilon zeta</p></article></body>",
        )
        .unwrap();
        let source = SourceAnalysis::build(&dom);
        assert!(
            locate_structured_content(&dom, &source, ["alpha beta gamma delta epsilon zeta"])
                .is_none()
        );
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
