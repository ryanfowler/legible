//! General content extraction and strategy-based retry orchestration.
#![allow(clippy::collapsible_if)]
use crate::candidate::{
    CandidateSet, CandidateSource, DocumentEvidence, RankedCandidate, RootSelection,
    RootSelectionReason, locate_structured_content, select_content_root,
    semantic_root_has_complete_candidate,
};
use crate::cleaning::*;
use crate::constants::{
    is_alter_to_div_exception, is_default_tag_to_score, is_phrasing_elem, regexps,
};
use crate::diagnostics::{
    AcceptanceExceptionInfo, AttemptRejectionReason, CandidateSourceInfo, CleanupActionInfo,
    CleanupActionKind, ContentMetricsInfo, ExtractionAttempt, ExtractionDiagnostics,
    ExtractionStrategyInfo, NormalizationCountsInfo, QualityInfo, RepresentationMetricsInfo,
    RootInfo, RootSelectionReasonInfo,
};
use crate::dom::{AttrName, DocumentAnchors, Dom, NodeId, NodeStateStore, ScoreStore, Tag};
use crate::error::{Error, ResourceLimitKind, Result};
use crate::extractor::{ContentHint, ContentTag, ExtractorConfig};
use crate::instrumentation::{Phase, PhaseGuard};
use crate::logging::debug_log;
use crate::metadata::{self, Metadata, MetadataDiagnostics, StructuredData};
use crate::normalize::{
    accessible_math_nodes, adjacent_lead_media, adopt_external_footnotes,
    cleanup_selected_content_in_workspace, collect_external_footnotes,
    has_primary_heading_semantics, normalize_svg_before_scoring,
    prepare_media_before_cleanup_in_workspace, remove_decorative_media_before_cleanup_in_workspace,
    remove_empty_content_with_source_facts,
};
use crate::page::ExtractedPage;
use crate::page_kind::PageKind;
use crate::prepared::{SourceAnalysis, SourceElements, SourceEntry, SourceFlags};
use crate::quality::{
    CleanedFragmentAnalysis, ContentMetrics, ExtractionQuality, SemanticStructureCounts,
    interactive_shell_evidence, is_access_barrier, is_access_barrier_prepared,
    is_application_shell_notice, is_incoherent_short_result, is_interactive_shell,
    is_link_only_semantic_root, semantic_coverage,
};
use crate::scoring::*;
use crate::specialized::{self, DocumentContext};
use crate::tokens::{has_any_token, has_token};
use regex::Regex;
use smallvec::SmallVec;
use std::collections::HashSet;
use url::Url;

pub(crate) struct ContentExtractor<'a> {
    dom: Dom,
    source_dom_nodes: usize,
    options: &'a ExtractorConfig,
    strategy: ExtractionStrategy,
    #[cfg(test)]
    node_data: NodeStateStore,
    page_title: String,
    structured_title: String,
    page_byline: Option<String>,
    page_direction: Option<String>,
    page_language: Option<String>,
    metadata: Metadata,
    metadata_diagnostics: Option<MetadataDiagnostics>,
    structured_data: StructuredData,
    source_uri: Option<Url>,
    base_uri: Option<Url>,
    url_error: Option<url::ParseError>,
    best_attempt: Option<BestAttempt>,
    diagnostic_attempts: Option<Vec<ExtractionAttempt>>,
    diagnostic_cleanup_actions: Vec<CleanupActionInfo>,
    diagnostic_normalization: NormalizationCountsInfo,
    specialized_root: Option<NodeId>,
    specialized_identity: Option<&'static str>,
    page_kind: PageKind,
    metadata_fallback_text: Option<String>,
    metadata_fallback_source_metrics: Option<ContentMetrics>,
    metadata_fallback_source_barrier: Option<bool>,
}

#[derive(Clone, Copy)]
enum CommonTableWrapper {
    Rows,
    Cells,
    Sections,
    Columns,
}

impl CommonTableWrapper {
    fn node_count(self) -> usize {
        match self {
            Self::Rows | Self::Columns => 2,
            Self::Cells => 3,
            Self::Sections => 1,
        }
    }
}

fn common_table_wrapper(tags: &[Tag], sibling_count: usize) -> Option<CommonTableWrapper> {
    if sibling_count == 0 || tags.len() != sibling_count {
        return None;
    }
    if tags.iter().all(|&tag| tag == Tag::Tr) {
        Some(CommonTableWrapper::Rows)
    } else if tags.iter().all(|tag| matches!(tag, Tag::Td | Tag::Th)) {
        Some(CommonTableWrapper::Cells)
    } else if tags.iter().all(|tag| {
        matches!(
            tag,
            Tag::Caption | Tag::Colgroup | Tag::Tbody | Tag::Tfoot | Tag::Thead
        )
    }) {
        Some(CommonTableWrapper::Sections)
    } else if tags.iter().all(|&tag| tag == Tag::Col) {
        Some(CommonTableWrapper::Columns)
    } else {
        None
    }
}

fn table_wrapper_count(tag: Tag) -> usize {
    match tag {
        Tag::Tr | Tag::Col => 2,
        Tag::Td | Tag::Th => 3,
        Tag::Caption | Tag::Colgroup | Tag::Tbody | Tag::Tfoot | Tag::Thead => 1,
        _ => 0,
    }
}

fn table_wrapper_plan(tags: &[Tag], sibling_count: usize) -> (Option<CommonTableWrapper>, usize) {
    let common = common_table_wrapper(tags, sibling_count);
    let node_count = common.map_or_else(
        || {
            tags.iter().fold(0usize, |count, &tag| {
                count.saturating_add(table_wrapper_count(tag))
            })
        },
        CommonTableWrapper::node_count,
    );
    (common, node_count)
}

#[derive(Default)]
struct TitleHeadingPlan {
    preferred: Option<NodeId>,
    brand_headings: SmallVec<[NodeId; 2]>,
}

struct PlanContext<'a> {
    prepared_source: &'a SourceAnalysis,
    accessible_math: &'a HashSet<NodeId>,
    title_plan: &'a TitleHeadingPlan,
    base_candidates: &'a CandidateSet,
    content_hint_targets: &'a [NodeId],
    source_anchors: DocumentAnchors,
    document_evidence: DocumentEvidence,
    structured_texts: &'a [&'a str],
    structured_root: Option<NodeId>,
    short_source_access_barrier: bool,
}

fn remove_title_brand_headings(dom: &mut Dom, root: NodeId, plan: &TitleHeadingPlan) {
    let Some(preferred) = plan.preferred else {
        return;
    };
    if preferred != root && !dom.ancestors(preferred).any(|ancestor| ancestor == root) {
        return;
    }
    let headings: SmallVec<[NodeId; 2]> = dom
        .element_descendants_snapshot_with_depth(root)
        .into_iter()
        .map(|(node, _)| node)
        .filter(|node| plan.brand_headings.contains(node))
        .collect();
    for heading in headings {
        dom.detach(heading);
    }
}

fn exact_is_phrasing_content(dom: &Dom, node: NodeId, depth: u32) -> bool {
    if dom.is_text(node) || dom.is_comment(node) {
        return true;
    }
    let Some(tag) = dom.tag(node) else {
        return false;
    };
    is_phrasing_elem(tag)
        || matches!(tag, Tag::A | Tag::Del | Tag::Ins)
            && depth < 10
            && dom
                .children(node)
                .all(|child| exact_is_phrasing_content(dom, child, depth + 1))
}

fn exact_is_whitespace(dom: &Dom, node: NodeId) -> bool {
    dom.text_node(node)
        .is_some_and(|text| text.trim().is_empty())
        || dom.tag(node) == Some(Tag::Br)
}

/// Wrap direct phrasing content in the caller-selected root.
///
/// This keeps the exact-root path's source semantics without invoking the
/// generic readability preparation pass.
fn wrap_exact_phrasing_content_in_p(dom: &mut Dom, root: NodeId) {
    let children: SmallVec<[NodeId; 8]> = dom.children(root).collect();
    if children.is_empty()
        || !children
            .iter()
            .all(|&child| exact_is_phrasing_content(dom, child, 0))
    {
        return;
    }
    let mut start = 0;
    let mut end = children.len();
    while start < end && exact_is_whitespace(dom, children[start]) {
        start += 1;
    }
    while end > start && exact_is_whitespace(dom, children[end - 1]) {
        end -= 1;
    }
    if start == end {
        return;
    }
    let paragraph = dom.create_html_element(Tag::P).expect("DOM node limit");
    dom.insert_before(children[start], paragraph);
    for &child in &children[start..end] {
        dom.append_child(paragraph, child);
    }
    for &child in children[..start].iter().chain(children[end..].iter()) {
        dom.detach(child);
    }
}

fn has_exact_single_paragraph_child(dom: &Dom, node: NodeId) -> bool {
    let mut found = false;
    for child in dom.children(node) {
        if dom.is_element(child) {
            if found || dom.tag(child) != Some(Tag::P) {
                return false;
            }
            found = true;
        } else if dom.is_text(child)
            && dom
                .text_node(child)
                .is_some_and(|text| text.ends_with(|character: char| !character.is_whitespace()))
        {
            return false;
        }
    }
    found
}

struct BestAttempt {
    physical_attempt: PhysicalAttemptId,
    quality: ExtractionQuality,
    excerpt: Option<String>,
    direction: Option<String>,
    strategy: ExtractionStrategy,
    byline: Option<String>,
    diagnostic_index: Option<usize>,
}

struct AttemptVerdict {
    ignores_visible_source_barrier: bool,
    link_only_semantic_root: bool,
    valid_result: bool,
    visibility_improves: bool,
    deferred_for_visibility: bool,
    acceptance_exception: bool,
    accepted: bool,
}

struct AttemptPolicyInput<'a> {
    strategy: ExtractionStrategy,
    structured_root: Option<NodeId>,
    selection_node: NodeId,
    quality: ExtractionQuality,
    metrics: ContentMetrics,
    best: Option<&'a BestAttempt>,
    has_relaxable_hidden_content: bool,
    visibility_recovery_needed: bool,
    short_source_access_barrier: bool,
    root_in_document_chrome: bool,
    access_barrier: bool,
    interactive_shell: bool,
    incoherent_short: bool,
    visibility_root_semantic: bool,
    semantic_root_complete_candidate: bool,
    semantic_root_boilerplate: bool,
    rejected_link_only_semantic_root: &'a mut bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ExactRootOrigin {
    Caller,
    Specialized,
}

struct FrozenContent {
    dom: Dom,
    source_facts: Option<crate::document::SemanticSourceFacts>,
    source_evidence: crate::document::SourceEvidence,
    retained_stream: Option<crate::document::RetainedStream>,
    ordinary_plan: Option<crate::document::OrdinarySourcePlan>,
    ordinary_checked: bool,
}

/// Immutable state shared by all physical extraction attempts.
///
/// The source DOM is owned by `ContentExtractor`, but this view is the only
/// source state an attempt needs. An attempt must never take ownership of it or
/// mutate it.
struct SourceSession<'a, 'b> {
    extractor: &'a ContentExtractor<'b>,
}

/// Mutable state for one physical fragment execution.
///
/// Dropping this value drops a rejected fragment. It does not require source
/// DOM restoration or attempt-state reset on the extraction coordinator.
struct AttemptRunner<'a, 'b> {
    source: SourceSession<'a, 'b>,
    dom: Dom,
    scratch: AttemptScratch,
    cleanup_actions: Vec<CleanupActionInfo>,
    normalization: NormalizationCountsInfo,
}

#[derive(Default)]
struct AttemptScratch {
    node_data: NodeStateStore,
    workspace: FragmentWorkspace,
    cleaning_nodes: Vec<NodeId>,
}

impl<'a, 'b> AttemptRunner<'a, 'b> {
    fn new(source: &'a ContentExtractor<'b>, dom: Dom, scratch: AttemptScratch) -> Self {
        Self {
            source: SourceSession { extractor: source },
            dom,
            scratch,
            cleanup_actions: Vec::new(),
            normalization: NormalizationCountsInfo::default(),
        }
    }
}

#[derive(Clone, Debug)]
struct AttemptPlan {
    strategy: ExtractionStrategy,
    visibility: VisibilityVariant,
    analysis_index: usize,
    selection: RootSelection,
    physical_attempt: PhysicalAttemptId,
    source_direction: Option<String>,
    root_info: Option<RootInfo>,
    root_in_document_chrome: bool,
    visibility_root_semantic: bool,
    semantic_root_complete_candidate: bool,
    semantic_root_boilerplate: bool,
    byline: Option<String>,
}

/// The complete identity of one physical fragment execution.
///
/// Two logical strategies may share an execution only when these fields are
/// equal. Logical metadata such as direction, byline, root diagnostics, and
/// acceptance policy is intentionally not part of this value.
#[derive(Clone, Debug, Eq, PartialEq)]
struct PhysicalPlan {
    source_roots: SmallVec<[NodeId; 16]>,
    selection_node: NodeId,
    top_id: NodeId,
    synthetic: bool,
    visibility: VisibilityVariant,
    conditional_cleanup: bool,
    body_fallback: bool,
    rename_top: bool,
    lead_media: Option<NodeId>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PhysicalAttemptId(usize);

struct PhysicalAttempt {
    plan: PhysicalPlan,
    cached: Option<CachedPhysicalAttempt>,
}

struct CachedPhysicalAttempt {
    result_metrics: ContentMetrics,
    semantic_coverage: Option<crate::diagnostics::SemanticCoverageInfo>,
    representation: Option<RepresentationMetricsInfo>,
    cleanup_actions: Vec<CleanupActionInfo>,
    normalization: NormalizationCountsInfo,
    access_barrier: bool,
    interactive_shell: bool,
    incoherent_short: bool,
    excerpt: Option<String>,
    content: Option<FrozenContent>,
}

impl CachedPhysicalAttempt {
    #[allow(clippy::too_many_arguments)]
    fn from_result(
        result_metrics: ContentMetrics,
        semantic_coverage: Option<crate::diagnostics::SemanticCoverageInfo>,
        representation: Option<RepresentationMetricsInfo>,
        cleanup_actions: Vec<CleanupActionInfo>,
        normalization: NormalizationCountsInfo,
        access_barrier: bool,
        interactive_shell: bool,
        incoherent_short: bool,
    ) -> Self {
        Self {
            result_metrics,
            semantic_coverage,
            representation,
            cleanup_actions,
            normalization,
            access_barrier,
            interactive_shell,
            incoherent_short,
            excerpt: None,
            content: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
enum ExtractionStrategy {
    Normal,
    RelaxedCleanup,
    BroadContent,
    StructuredDataHint,
    RelaxedVisibility,
    BodyFallback,
    MetadataFallback,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum VisibilityVariant {
    Normal,
    Relaxed,
}

impl ExtractionStrategy {
    fn visibility_variant(self) -> VisibilityVariant {
        if self == Self::RelaxedVisibility {
            VisibilityVariant::Relaxed
        } else {
            VisibilityVariant::Normal
        }
    }
}

#[cfg(test)]
std::thread_local! {
    static GENERIC_SCORING_CALLS: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
fn record_generic_scoring_call() {
    GENERIC_SCORING_CALLS.with(|calls| calls.set(calls.get() + 1));
}

#[cfg(test)]
fn reset_generic_scoring_calls() {
    GENERIC_SCORING_CALLS.with(|calls| calls.set(0));
}

#[cfg(test)]
fn generic_scoring_calls() -> u32 {
    GENERIC_SCORING_CALLS.with(std::cell::Cell::get)
}

impl From<ExtractionStrategy> for ExtractionStrategyInfo {
    fn from(value: ExtractionStrategy) -> Self {
        match value {
            ExtractionStrategy::Normal => Self::Normal,
            ExtractionStrategy::RelaxedCleanup => Self::RelaxedCleanup,
            ExtractionStrategy::BroadContent => Self::BroadContent,
            ExtractionStrategy::StructuredDataHint => Self::StructuredDataHint,
            ExtractionStrategy::RelaxedVisibility => Self::RelaxedVisibility,
            ExtractionStrategy::BodyFallback => Self::BodyFallback,
            ExtractionStrategy::MetadataFallback => Self::MetadataFallback,
        }
    }
}

impl From<RootSelectionReason> for RootSelectionReasonInfo {
    fn from(value: RootSelectionReason) -> Self {
        match value {
            RootSelectionReason::Ranked => Self::Ranked,
            RootSelectionReason::SpecificChild => Self::SpecificChild,
            RootSelectionReason::SharedParent => Self::SharedParent,
            RootSelectionReason::CompleteAncestor => Self::CompleteAncestor,
            RootSelectionReason::StructuredData => Self::StructuredData,
            RootSelectionReason::ArticleBody => Self::ArticleBody,
            RootSelectionReason::BodyFallback => Self::BodyFallback,
        }
    }
}

impl ExtractionStrategy {
    const ORDER: [Self; 6] = [
        Self::Normal,
        Self::RelaxedCleanup,
        Self::BroadContent,
        Self::StructuredDataHint,
        Self::RelaxedVisibility,
        Self::BodyFallback,
    ];

    fn weight_classes(self) -> bool {
        !matches!(self, Self::BroadContent | Self::BodyFallback)
    }

    fn conditional_cleanup(self) -> bool {
        matches!(
            self,
            Self::Normal | Self::StructuredDataHint | Self::RelaxedVisibility
        )
    }

    fn should_plan(
        self,
        source_metrics: ContentMetrics,
        structured_root: Option<NodeId>,
        has_relaxable_hidden_content: bool,
    ) -> bool {
        if self == Self::StructuredDataHint && structured_root.is_none() {
            return false;
        }
        if self == Self::RelaxedVisibility && !has_relaxable_hidden_content {
            return false;
        }
        self == Self::RelaxedVisibility || source_metrics.has_meaningful_text()
    }
}
struct ExtractedContent {
    excerpt: Option<String>,
    /// The compiled semantic result.
    document: crate::document::Document,
}

#[derive(Clone)]
struct CandidateDiscovery {
    candidates: CandidateSet,
    to_score: SmallVec<[NodeId; 256]>,
    divs_to_prepare: SmallVec<[NodeId; 128]>,
    remove_after_scoring: SmallVec<[NodeId; 64]>,
    byline: Option<String>,
    has_links: bool,
}

struct ScoredVariant {
    scores: ScoreStore,
    candidates: CandidateSet,
    ranked: SmallVec<[RankedCandidate; 64]>,
}

struct ScoringAnalysis {
    view: ScoringView,
    discovery: CandidateDiscovery,
    weighted: ScoredVariant,
    unweighted: Option<ScoredVariant>,
    shared_facts: NodeStateStore,
    working_snapshot: Vec<(NodeId, u32)>,
    excluded_mask: Vec<bool>,
    feature_index: CandidateFeatureIndex,
    body: NodeId,
}

impl ScoringAnalysis {
    fn variant(&self, weight_classes: bool) -> Option<&ScoredVariant> {
        if weight_classes {
            Some(&self.weighted)
        } else {
            self.unweighted.as_ref()
        }
    }

    fn variant_mut(&mut self, weight_classes: bool) -> Option<&mut ScoredVariant> {
        if weight_classes {
            Some(&mut self.weighted)
        } else {
            self.unweighted.as_mut()
        }
    }

    fn take_scores(&mut self, weight_classes: bool) -> Option<ScoreStore> {
        self.variant_mut(weight_classes)
            .map(|variant| std::mem::take(&mut variant.scores))
    }

    fn restore_scores(&mut self, weight_classes: bool, scores: ScoreStore) {
        if let Some(variant) = self.variant_mut(weight_classes) {
            variant.scores = scores;
        }
    }
}

#[derive(Default)]
struct AnalysisCache {
    variants: Vec<(VisibilityVariant, ScoringAnalysis)>,
}

impl AnalysisCache {
    fn find(&self, visibility: VisibilityVariant) -> Option<usize> {
        self.variants
            .iter()
            .position(|(cached_visibility, _)| *cached_visibility == visibility)
    }
}

fn content_hint_has_value(target: &ContentHint) -> bool {
    !matches!(target, ContentHint::Id(value) | ContentHint::Class(value) if value.trim().is_empty())
}

fn content_target_matches(dom: &Dom, node: NodeId, target: &ContentHint) -> bool {
    if !dom.is_element(node) {
        return false;
    }
    match target {
        ContentHint::Id(value) => dom.attr(node, AttrName::Id) == Some(value.as_str()),
        ContentHint::Class(value) => dom
            .attr(node, AttrName::Class)
            .is_some_and(|classes| classes.split_whitespace().any(|class| class == value)),
        ContentHint::Tag(tag) => {
            let expected = match tag {
                ContentTag::Article => Tag::Article,
                ContentTag::Main => Tag::Main,
                ContentTag::Section => Tag::Section,
                ContentTag::Div => Tag::Div,
            };
            dom.tag(node) == Some(expected)
        }
    }
}

fn find_content_targets_from_prepared(
    dom: &Dom,
    source: &SourceAnalysis,
    target: &ContentHint,
) -> Vec<NodeId> {
    if !content_hint_has_value(target) {
        return Vec::new();
    }
    source
        .elements()
        .map(|entry| entry.node)
        .filter(|&node| content_target_matches(dom, node, target))
        .collect()
}

impl<'a> ContentExtractor<'a> {
    pub(crate) fn from_document(dom: Dom, url: Option<&str>, options: &'a ExtractorConfig) -> Self {
        let source_dom_nodes = dom.len();
        let (base_uri, url_error) = match url {
            Some(x) => match Url::parse(x) {
                Ok(u) => (Some(u), None),
                Err(e) => (None, Some(e)),
            },
            None => (None, None),
        };
        Self {
            dom,
            source_dom_nodes,
            options,
            strategy: ExtractionStrategy::Normal,
            #[cfg(test)]
            node_data: NodeStateStore::new(),
            page_title: String::new(),
            structured_title: String::new(),
            page_byline: None,
            page_direction: None,
            page_language: None,
            metadata: Metadata::default(),
            metadata_diagnostics: None,
            structured_data: StructuredData::default(),
            source_uri: base_uri.clone(),
            base_uri,
            url_error,
            best_attempt: None,
            diagnostic_attempts: options.diagnostics.then(Vec::new),
            diagnostic_cleanup_actions: Vec::new(),
            diagnostic_normalization: NormalizationCountsInfo::default(),
            specialized_root: None,
            specialized_identity: None,
            page_kind: PageKind::Unknown,
            metadata_fallback_text: None,
            metadata_fallback_source_metrics: None,
            metadata_fallback_source_barrier: None,
        }
    }
    pub(crate) fn extract(mut self) -> Result<ExtractedPage> {
        if let Some(e) = self.url_error {
            return Err(Error::InvalidUrl(e));
        }
        let _metadata_phase = PhaseGuard::new(Phase::Metadata);
        let preparation_anchors = self.dom.document_anchors();
        if let Some(base) = preparation_anchors.first_base_with_href
            && let Some(href) = self.dom.attr(base, AttrName::Href)
        {
            let base_uri = self
                .base_uri
                .as_ref()
                .map_or_else(|| Url::parse(href), |document_uri| document_uri.join(href));
            if let Ok(base_uri) = base_uri {
                self.base_uri = Some(base_uri);
            }
        }
        // Metadata must inspect the parsed source before preparation removes or
        // rewrites any nodes. Image preparation happens afterwards because it
        // can replace placeholder and noscript subtrees.
        let title = metadata::get_page_title(&self.dom);
        self.structured_title = metadata::content_identity_title(&self.dom, &title);
        if self.options.structured_data {
            self.structured_data = StructuredData::parse(&self.dom, &self.options.parse_budget)
                .map_err(|error| match error {
                    metadata::StructuredDataError::Bytes { limit } => {
                        Error::resource_limit(ResourceLimitKind::JsonLdBytes, limit)
                    }
                    metadata::StructuredDataError::Items { limit } => {
                        Error::resource_limit(ResourceLimitKind::JsonLdItems, limit)
                    }
                    metadata::StructuredDataError::Depth { limit } => {
                        Error::resource_limit(ResourceLimitKind::JsonLdDepth, limit)
                    }
                })?;
        }
        (self.metadata, self.metadata_diagnostics) = metadata::discover_with_diagnostics(
            &self.dom,
            &self.structured_data,
            &title,
            self.base_uri.as_ref(),
            self.source_uri.as_ref(),
            self.options.metadata_diagnostics,
        );
        if self.options.diagnostics {
            self.metadata_fallback_text = metadata::metadata_backed_content(
                &self.dom,
                &self.structured_data,
                &self.metadata,
                &self.structured_title,
                self.base_uri.as_ref(),
                self.source_uri.as_ref(),
            );
            self.metadata_fallback_source_barrier = preparation_anchors
                .body
                .map(|body| is_access_barrier(&self.dom, body));
        }
        if self
            .structured_data
            .primary_texts(&self.structured_title, self.source_uri.as_ref())
            .next()
            .is_some()
        {
            debug_log!("Structured data contains a content-location hint");
        }
        #[cfg(feature = "bench-instrumentation")]
        drop(_metadata_phase);
        let _preparation_phase = PhaseGuard::new(Phase::Preparation);
        unwrap_noscript_images(&mut self.dom);
        prep_document_with_body(&mut self.dom, preparation_anchors.body);
        let document_root = self.dom.root();
        normalize_svg_before_scoring(&mut self.dom, document_root);
        if self.options.content_root.is_none() {
            let specialized = specialized::extract(&DocumentContext {
                dom: &self.dom,
                source_uri: self.source_uri.as_ref(),
            });
            if let Some(result) = specialized {
                self.dom = result.dom;
                self.specialized_root = Some(result.root);
                self.specialized_identity = Some(result.identity);
                self.page_kind = result.kind;
            }
        }
        if self.page_kind == PageKind::Unknown {
            self.page_kind = PageKind::detect(&self.dom);
        }
        #[cfg(feature = "bench-instrumentation")]
        drop(_preparation_phase);
        self.page_title = self
            .metadata
            .title
            .take()
            .or_else(|| metadata::normalize_title(&title))
            .unwrap_or_default();
        let content = match self.extract_content() {
            Ok(content) => content,
            Err(Error::NoContent)
                if self.options.content_root.is_none() && self.specialized_root.is_none() =>
            {
                self.extract_metadata_fallback()?
            }
            Err(Error::NoContent) => {
                return Err(Error::NoContent);
            }
            Err(error) => return Err(error),
        };
        self.metadata.title = (!self.page_title.is_empty()).then_some(self.page_title);
        if self.metadata.authors.is_empty()
            && let Some(byline) = self
                .page_byline
                .as_deref()
                .and_then(metadata::normalize_person)
        {
            self.metadata.authors.push(byline);
        }
        self.metadata.description = self.metadata.description.or(content.excerpt);
        self.metadata.direction = self.metadata.direction.or_else(|| {
            self.page_direction
                .as_deref()
                .and_then(metadata::normalize_direction)
        });
        self.metadata.language = self.metadata.language.or_else(|| {
            self.page_language
                .as_deref()
                .and_then(metadata::normalize_language)
        });
        if let Some(diagnostics) = &mut self.metadata_diagnostics {
            diagnostics.complete_with_fallbacks(&self.metadata);
        }
        // Normal extraction compiles only the selected candidate. Diagnostic
        // extraction compiles every attempt so it can report semantic metrics.
        let diagnostics = self
            .diagnostic_attempts
            .take()
            .map(|attempts| ExtractionDiagnostics {
                selected_strategy: self.strategy.into(),
                specialized_extractor: self.specialized_identity.map(str::to_owned),
                attempts,
            });
        let retained_structured_data = self
            .options
            .retain_structured_data
            .then(|| self.structured_data.retained_items());
        Ok(ExtractedPage::new(
            content.document,
            self.metadata,
            diagnostics,
            self.metadata_diagnostics,
            retained_structured_data,
        ))
    }

    #[cold]
    #[inline(never)]
    fn extract_metadata_fallback(&mut self) -> Result<ExtractedContent> {
        let text = self
            .metadata_fallback_text
            .take()
            .or_else(|| {
                metadata::metadata_backed_content(
                    &self.dom,
                    &self.structured_data,
                    &self.metadata,
                    &self.structured_title,
                    self.base_uri.as_ref(),
                    self.source_uri.as_ref(),
                )
            })
            .ok_or(Error::NoContent)?;
        let source_barrier = self.metadata_fallback_source_barrier.unwrap_or_else(|| {
            self.dom
                .body()
                .is_some_and(|body| is_access_barrier(&self.dom, body))
        });
        if source_barrier {
            return Err(Error::NoContent);
        }

        let (mut dom, root) = specialized::new_output().ok_or(Error::NoContent)?;
        let paragraph =
            specialized::create_element(&mut dom, root, Tag::P).ok_or(Error::NoContent)?;
        if !specialized::append_text(&mut dom, paragraph, &text) {
            return Err(Error::NoContent);
        }
        let final_dom_nodes = dom.len();
        let source_evidence = crate::document::SourceEvidence::default();
        let compile_context =
            crate::document::CompileContext::new(self.base_uri.clone(), self.source_uri.as_ref());
        self.strategy = ExtractionStrategy::MetadataFallback;
        crate::instrumentation::record_strategy(ExtractionStrategy::MetadataFallback as u8);
        let document = crate::document::compile_document_owned(
            dom,
            root,
            &compile_context,
            crate::document::CompileInputs {
                source_evidence: Some(&source_evidence),
                ..Default::default()
            },
        )
        .map_err(|_| Error::NoContent)?;
        if self.diagnostic_attempts.is_some() {
            let result_metrics = ContentMetrics::measure_document(&document);
            let source_metrics = self
                .metadata_fallback_source_metrics
                .unwrap_or(result_metrics);
            let quality = ExtractionQuality::new(source_metrics, result_metrics, false);
            self.record_attempt(
                ExtractionStrategy::MetadataFallback,
                Some(RootInfo {
                    tag: Some("main".to_owned()),
                    id: None,
                    classes: Vec::new(),
                    selection_reason: RootSelectionReasonInfo::MetadataFallback,
                    candidate_sources: Vec::new(),
                }),
                source_metrics,
                result_metrics,
                quality,
                None,
                Some(RepresentationMetricsInfo {
                    source_dom_nodes: self.source_dom_nodes,
                    final_dom_nodes,
                    document_nodes: document.len(),
                    estimated_document_bytes: document.retained_bytes_estimate(),
                }),
                true,
                false,
                None,
            );
        }
        Ok(ExtractedContent {
            excerpt: None,
            document,
        })
    }

    fn extract_content(&mut self) -> Result<ExtractedContent> {
        let _candidate_preflight_phase = PhaseGuard::new(Phase::CandidateDiscovery);
        let prepared_source =
            SourceAnalysis::build_with_semantic_counts(&self.dom, self.options.diagnostics);
        let exact_root = if let Some(target) = &self.options.content_root {
            Some(
                find_content_targets_from_prepared(&self.dom, &prepared_source, target)
                    .into_iter()
                    .next()
                    .ok_or(Error::ContentRootNotFound)?,
            )
        } else {
            self.specialized_root
        };
        if let Some(root) = exact_root {
            let origin = if self.options.content_root.is_some() {
                ExactRootOrigin::Caller
            } else {
                ExactRootOrigin::Specialized
            };
            let compile_context = crate::document::CompileContext::new(
                self.base_uri.clone(),
                self.source_uri.as_ref(),
            );
            #[cfg(feature = "bench-instrumentation")]
            drop(_candidate_preflight_phase);
            return self.extract_exact_root(
                root,
                origin,
                &compile_context,
                prepared_source.anchors,
            );
        }
        // External definitions can only be adopted when the source contains
        // a reference target. Avoid the full-document definition scan for the
        // common article path without footnote links.
        let footnote_definitions = prepared_source.has_possible_footnote_reference().then(|| {
            crate::instrumentation::record_external_footnote_scan();
            collect_external_footnotes(&self.dom)
        });
        let source_anchors = prepared_source.anchors;
        let body = source_anchors.body.ok_or(Error::NoBody)?;
        let source_metrics = prepared_source.source_metrics;
        if self.options.diagnostics {
            self.metadata_fallback_source_metrics = Some(source_metrics);
        }
        let has_relaxable_hidden_content = prepared_source.has_relaxable_hidden_content(body);
        let relaxed_source_metrics = prepared_source.relaxed_metrics.unwrap_or(source_metrics);
        if !source_metrics.has_meaningful_text() && !relaxed_source_metrics.has_meaningful_text() {
            return Err(Error::NoContent);
        }
        let short_source_access_barrier = (source_metrics.word_count <= 60
            || source_metrics.text_chars <= 400)
            && is_access_barrier_prepared(&self.dom, &prepared_source, body);
        let substantial_hidden_gain = relaxed_source_metrics.text_chars
            >= source_metrics.text_chars.saturating_mul(2)
            && relaxed_source_metrics.text_chars >= source_metrics.text_chars.saturating_add(1_000);
        let visibility_recovery_needed = has_relaxable_hidden_content
            && (source_metrics.word_count <= 30
                || source_metrics.text_chars <= 200
                || substantial_hidden_gain)
            && relaxed_source_metrics.text_chars >= source_metrics.text_chars.saturating_mul(2)
            && relaxed_source_metrics.text_chars >= source_metrics.text_chars.saturating_add(100);
        let structured_texts: Vec<_> = self
            .structured_data
            .primary_texts(&self.structured_title, self.source_uri.as_ref())
            .map(|text| text.chars().take(4_096).collect::<String>())
            .collect();
        let structured_text_refs: Vec<_> = structured_texts.iter().map(String::as_str).collect();
        let structured_root = locate_structured_content(
            &self.dom,
            &prepared_source,
            structured_text_refs.iter().copied(),
        );
        let document_evidence = DocumentEvidence {
            title_chars: u16::try_from(self.page_title.chars().count().min(usize::from(u16::MAX)))
                .unwrap_or(u16::MAX),
            description_chars: self
                .metadata
                .description
                .as_deref()
                .map_or(0, |description| {
                    u16::try_from(description.chars().count().min(usize::from(u16::MAX)))
                        .unwrap_or(u16::MAX)
                }),
            structured_items: u8::try_from(
                self.structured_data
                    .document_evidence_count(&self.structured_title, self.source_uri.as_ref())
                    .min(usize::from(u8::MAX)),
            )
            .unwrap_or(u8::MAX),
            has_hidden_content: has_relaxable_hidden_content,
        };
        let mut text_buffer = String::new();
        let mut attempt_scratch = AttemptScratch::default();
        let accessible_math = prepared_source.accessible_math_nodes(&self.dom);
        let base_candidates = prepared_source.candidates().clone();
        let content_hint_targets =
            self.options
                .content_hint
                .as_ref()
                .map_or_else(Vec::new, |hint| {
                    if content_hint_has_value(hint) {
                        crate::instrumentation::record_content_hint_scan();
                    }
                    find_content_targets_from_prepared(&self.dom, &prepared_source, hint)
                });
        let title_plan = title_heading_plan(
            &self.dom,
            SourceElements::Prepared(&prepared_source),
            &self.page_title,
            &self.structured_title,
            self.metadata.site_name.as_deref(),
            self.source_uri.as_ref(),
        );
        #[cfg(feature = "bench-instrumentation")]
        drop(_candidate_preflight_phase);
        if let Some(html) = source_anchors.html {
            if let Some(lang) = self.dom.attr(html, AttrName::Lang) {
                self.page_language = Some(lang.into())
            }
            if let Some(dir) = self.dom.attr(html, AttrName::Dir) {
                self.page_direction = Some(dir.into())
            }
        }
        let compile_context =
            crate::document::CompileContext::new(self.base_uri.clone(), self.source_uri.as_ref());
        let ctx = PlanContext {
            prepared_source: &prepared_source,
            accessible_math: &accessible_math,
            title_plan: &title_plan,
            base_candidates: &base_candidates,
            content_hint_targets: &content_hint_targets,
            source_anchors,
            document_evidence,
            structured_texts: &structured_text_refs,
            structured_root,
            short_source_access_barrier,
        };
        let mut analysis_cache = AnalysisCache::default();
        let mut physical_attempts = Vec::new();
        let mut rejected_link_only_semantic_root = false;
        for strategy in ExtractionStrategy::ORDER {
            if !strategy.should_plan(
                source_metrics,
                structured_root,
                has_relaxable_hidden_content,
            ) {
                continue;
            }
            self.strategy = strategy;
            crate::instrumentation::record_strategy(strategy as u8);
            crate::instrumentation::record_logical_attempt_plan();
            let (plan, physical_plan) =
                self.build_attempt_plan(&ctx, strategy, &mut text_buffer, &mut analysis_cache)?;
            let plan = Self::intern_physical_plan(plan, physical_plan, &mut physical_attempts);
            let strategy = plan.strategy;
            debug_assert_eq!(plan.visibility, strategy.visibility_variant());
            let physical_attempt = plan.physical_attempt;
            let physical_plan = physical_attempts[physical_attempt.0].plan.clone();
            let analysis_index = plan.analysis_index;
            self.strategy = strategy;
            self.diagnostic_cleanup_actions.clear();
            self.diagnostic_normalization = NormalizationCountsInfo::default();
            let analysis = &analysis_cache.variants[analysis_index].1;
            let body = analysis.body;
            self.page_byline = plan.byline.clone();
            let selection = plan.selection.clone();
            let source_siblings = physical_plan.source_roots.clone();
            let top_id = physical_plan.top_id;
            let synthetic = physical_plan.synthetic;
            let rename_top = physical_plan.rename_top;
            let lead_media = physical_plan.lead_media;
            let source_direction = plan.source_direction.clone();
            let root_info = plan.root_info.clone();
            let root_in_document_chrome = plan.root_in_document_chrome;
            let visibility_root_semantic = plan.visibility_root_semantic;
            let semantic_root_complete_candidate = plan.semantic_root_complete_candidate;
            let semantic_root_boilerplate = plan.semantic_root_boilerplate;
            let cached_attempt = physical_attempts[physical_attempt.0].cached.take();
            if let Some(mut cached) = cached_attempt {
                crate::instrumentation::record_deduplicated_attempt();
                self.diagnostic_cleanup_actions = cached.cleanup_actions.clone();
                self.diagnostic_normalization = cached.normalization;
                let attempt_source_metrics = if strategy == ExtractionStrategy::RelaxedVisibility {
                    relaxed_source_metrics
                } else {
                    source_metrics
                };
                let result_metrics = cached.result_metrics;
                let quality = ExtractionQuality::new(
                    attempt_source_metrics,
                    result_metrics,
                    selection.node != body && strategy != ExtractionStrategy::BodyFallback,
                );
                let verdict = Self::evaluate_attempt(AttemptPolicyInput {
                    strategy,
                    structured_root,
                    selection_node: selection.node,
                    quality,
                    metrics: result_metrics,
                    best: self.best_attempt.as_ref(),
                    has_relaxable_hidden_content,
                    visibility_recovery_needed,
                    short_source_access_barrier,
                    root_in_document_chrome,
                    access_barrier: cached.access_barrier,
                    interactive_shell: cached.interactive_shell,
                    incoherent_short: cached.incoherent_short,
                    visibility_root_semantic,
                    semantic_root_complete_candidate,
                    semantic_root_boilerplate,
                    rejected_link_only_semantic_root: &mut rejected_link_only_semantic_root,
                });
                if verdict.accepted {
                    let excerpt = cached.excerpt.take();
                    self.page_direction = source_direction.clone();
                    self.page_byline = plan.byline.clone();
                    self.record_attempt(
                        strategy,
                        root_info,
                        attempt_source_metrics,
                        result_metrics,
                        quality,
                        cached.semantic_coverage.clone(),
                        cached.representation,
                        true,
                        verdict.acceptance_exception,
                        None,
                    );
                    let content = cached.content.take().ok_or(Error::NoContent)?;
                    let FrozenContent {
                        dom,
                        source_facts,
                        source_evidence,
                        retained_stream,
                        ordinary_plan,
                        ordinary_checked,
                    } = content;
                    let root = dom.root();
                    let document = crate::document::compile_document_owned(
                        dom,
                        root,
                        &compile_context,
                        crate::document::CompileInputs {
                            source_facts: source_facts.as_ref(),
                            source_evidence: Some(&source_evidence),
                            retained_stream: retained_stream.as_ref(),
                            ordinary_plan: ordinary_plan.as_ref(),
                            ordinary_checked,
                        },
                    )
                    .map_err(|_| Error::NoContent)?;
                    return Ok(ExtractedContent { excerpt, document });
                }
                let rejection = Self::attempt_rejection_reason(
                    root_in_document_chrome,
                    cached.access_barrier,
                    short_source_access_barrier && !verdict.ignores_visible_source_barrier,
                    verdict.link_only_semantic_root,
                    cached.interactive_shell,
                    cached.incoherent_short,
                    verdict.visibility_improves,
                    verdict.deferred_for_visibility,
                );
                let diagnostic_index = self.record_attempt(
                    strategy,
                    root_info,
                    attempt_source_metrics,
                    result_metrics,
                    quality,
                    cached.semantic_coverage.clone(),
                    cached.representation,
                    false,
                    false,
                    Some(rejection),
                );
                let can_be_best = verdict.valid_result
                    && verdict.visibility_improves
                    && self.best_attempt.as_ref().is_none_or(|best| {
                        quality.best_attempt_score() > best.quality.best_attempt_score()
                    });
                if can_be_best {
                    if let Some(previous) = self
                        .best_attempt
                        .as_ref()
                        .and_then(|best| best.diagnostic_index)
                        && let Some(attempts) = self.diagnostic_attempts.as_mut()
                    {
                        attempts[previous].rejection_reason =
                            Some(AttemptRejectionReason::Superseded);
                    }
                    self.best_attempt = Some(BestAttempt {
                        physical_attempt,
                        quality,
                        excerpt: cached.excerpt.take(),
                        direction: source_direction.clone(),
                        strategy,
                        byline: plan.byline.clone(),
                        diagnostic_index,
                    });
                }
                physical_attempts[physical_attempt.0].cached = Some(cached);
                continue;
            }
            crate::instrumentation::record_physical_attempt_execution();
            let working_dom = &self.dom;
            // Copy only the selected source roots. The scoring view remains
            // available for later strategies, so a rejected attempt does not
            // require another complete DOM clone.
            let fragment = {
                let _phase = PhaseGuard::new(Phase::FragmentCopy);
                analysis
                    .view
                    .copy_projected_subtrees_as_fragment_excluding(
                        working_dom,
                        &source_siblings,
                        &analysis.excluded_mask,
                    )
                    .map_err(|_| Error::NoContent)?
            };
            let copied_siblings: SmallVec<[NodeId; 16]> =
                fragment.children(fragment.root()).collect();
            let copied_top = if synthetic {
                copied_siblings[0]
            } else {
                let position = source_siblings
                    .iter()
                    .position(|&node| node == top_id)
                    .ok_or(Error::NoContent)?;
                copied_siblings[position]
            };

            // The attempt owns its fragment. The prepared source remains
            // available only through the runner's immutable source session.
            let mut runner =
                AttemptRunner::new(self, fragment, std::mem::take(&mut attempt_scratch));
            runner.scratch.workspace.reset();
            runner.scratch.node_data.clear();
            runner.scratch.cleaning_nodes.clear();
            let (_top_id, content_id) = if selection.node == body {
                Self::prune_body_fallback_chrome(&mut runner.dom, copied_top);
                runner.dom.reserve_additional_nodes_exact(2);
                let container = runner
                    .dom
                    .create_html_element(Tag::Div)
                    .map_err(|_| Error::NoContent)?;
                let children: SmallVec<[NodeId; 16]> = runner.dom.children(copied_top).collect();
                for child in children {
                    runner.dom.append_child(container, child)
                }
                runner.dom.append_child(copied_top, container);
                let fragment_root = runner.dom.root();
                runner.dom.append_child(fragment_root, container);
                runner.dom.detach(copied_top);
                (container, container)
            } else if !selection.branches.is_empty() {
                let container =
                    Self::create_container(&mut runner.dom, copied_siblings[0], &copied_siblings)
                        .ok_or(Error::NoContent)?;
                (container, container)
            } else {
                if rename_top
                    && matches!(runner.dom.tag(copied_top), Some(Tag::Article | Tag::Main))
                {
                    runner.dom.rename_html(copied_top, Tag::Div);
                    runner.dom.remove_attr(copied_top, AttrName::ItemProp);
                }
                let content_id =
                    Self::create_container(&mut runner.dom, copied_top, &copied_siblings)
                        .unwrap_or(copied_top);
                (copied_top, content_id)
            };
            let synthetic = selection.node == body || !selection.branches.is_empty();
            if let Some(lead_media) = lead_media
                && !source_siblings.contains(&lead_media)
                && let Some(first_child) = runner.dom.first_child(content_id)
            {
                let copied_lead = runner
                    .dom
                    .import_subtree(&runner.source.extractor.dom, lead_media)
                    .map_err(|_| Error::NoContent)?;
                let fragment_root = runner.dom.root();
                runner.dom.append_child(fragment_root, copied_lead);
                runner.dom.insert_before(first_child, copied_lead);
            }
            let fragment_title_snapshot = runner
                .dom
                .element_descendants_snapshot_with_depth(content_id);
            let fragment_title_plan = title_heading_plan(
                &runner.dom,
                SourceElements::Snapshot(&fragment_title_snapshot),
                &runner.source.extractor.page_title,
                &runner.source.extractor.structured_title,
                runner.source.extractor.metadata.site_name.as_deref(),
                runner.source.extractor.source_uri.as_ref(),
            );
            remove_title_brand_headings(&mut runner.dom, content_id, &fragment_title_plan);

            // Cleanup owns a compact copy of the selected region. The source
            // DOM remains available for a retry and is never affected by an
            // earlier attempt's mutations.
            if let Some(footnote_definitions) = footnote_definitions.as_ref() {
                adopt_external_footnotes(
                    footnote_definitions,
                    &runner.source.extractor.dom,
                    &mut runner.dom,
                    content_id,
                );
            }
            runner.scratch.node_data.clear();
            runner.scratch.node_data.enable_link_lengths();

            let _cleanup_phase = PhaseGuard::new(Phase::Cleanup);
            let shell_evidence = interactive_shell_evidence(&runner.dom, content_id);
            let video = regexps::VIDEOS.clone();
            let (candidate_semantic_metrics, source_evidence) =
                runner.source.extractor.prep_article(
                    &mut runner.dom,
                    content_id,
                    selection.node != body && strategy != ExtractionStrategy::BodyFallback,
                    &compile_context,
                    &video,
                    &mut text_buffer,
                    &mut runner.scratch.cleaning_nodes,
                    &mut runner.scratch.node_data,
                    &mut runner.scratch.workspace,
                    &mut runner.cleanup_actions,
                );
            if synthetic {
                runner
                    .dom
                    .set_attr(content_id, AttrName::Id, "legible-content");
                runner.dom.set_attr(content_id, AttrName::Class, "page")
            } else {
                let w = runner
                    .dom
                    .create_html_element(Tag::Div)
                    .map_err(|_| Error::NoContent)?;
                runner.dom.set_attr(w, AttrName::Id, "legible-content");
                runner.dom.set_attr(w, AttrName::Class, "page");
                let children: SmallVec<[NodeId; 16]> = runner.dom.children(content_id).collect();
                for x in children {
                    runner.dom.append_child(w, x)
                }
                runner.dom.append_child(content_id, w)
            }
            let access_barrier = is_access_barrier(&runner.dom, content_id);
            let (mut source_facts, mut retained_stream) = runner.source.extractor.final_cleanup(
                &mut runner.dom,
                content_id,
                &source_evidence,
                &mut runner.scratch.cleaning_nodes,
                &mut runner.cleanup_actions,
            );
            crate::instrumentation::record_cleaned_nodes(runner.dom.len());
            runner.scratch.workspace.invalidate();
            runner.source.extractor.capture_normalization_counts(
                &runner.dom,
                content_id,
                &mut runner.scratch.workspace,
                &mut runner.normalization,
            );
            #[cfg(feature = "bench-instrumentation")]
            drop(_cleanup_phase);
            if synthetic
                && content_id != runner.dom.root()
                && let Some(retained_stream) = retained_stream.as_mut()
            {
                retained_stream.prepend_root(content_id);
            }
            // The selected region is already a compact fragment. Remove the
            // internal selection boundary when the output contract excludes
            // it. This makes the fragment itself the final compiler input and
            // avoids copying it immediately before semantic compilation.
            if !synthetic {
                let fragment_root = runner.dom.root();
                runner.dom.move_children(content_id, fragment_root);
                runner.dom.detach(content_id);
            }
            let result_root = runner.dom.root();
            if let Some(source_facts) = source_facts.as_mut() {
                source_facts.rebase_root(&runner.dom, result_root);
            }
            let cleaned_analysis = retained_stream
                .take()
                .map(|stream| {
                    CleanedFragmentAnalysis::from_retained_stream(
                        &runner.dom,
                        result_root,
                        stream,
                        Some(&source_evidence),
                    )
                })
                .ok_or(Error::NoContent)?;
            // Normal extraction only needs the semantic document for a candidate
            // that can win. Diagnostics still compile every attempt so that they
            // retain complete semantic metrics.
            let result_document = if self.diagnostic_attempts.is_some() {
                Some(
                    crate::document::compile_document(
                        &runner.dom,
                        result_root,
                        &compile_context,
                        &crate::document::CompileInputs {
                            source_facts: source_facts.as_ref(),
                            source_evidence: Some(&source_evidence),
                            retained_stream: Some(&cleaned_analysis.retained_stream),
                            ordinary_plan: cleaned_analysis.ordinary_plan.as_ref(),
                            ordinary_checked: cleaned_analysis.ordinary_checked,
                        },
                    )
                    .map_err(|_| Error::NoContent)?,
                )
            } else {
                None
            };
            let representation =
                result_document
                    .as_ref()
                    .map(|document| RepresentationMetricsInfo {
                        source_dom_nodes: self.source_dom_nodes,
                        final_dom_nodes: {
                            crate::instrumentation::record_final_dom_node_scan();
                            1 + runner.dom.descendants(result_root).count()
                        },
                        document_nodes: document.len(),
                        estimated_document_bytes: document.retained_bytes_estimate(),
                    });
            let result_metrics = result_document.as_ref().map_or_else(
                || cleaned_analysis.metrics,
                ContentMetrics::measure_document,
            );
            let result_semantic_counts = result_document.as_ref().and_then(|document| {
                candidate_semantic_metrics
                    .as_ref()
                    .map(|_| SemanticStructureCounts::measure(document))
            });
            let semantic_coverage = candidate_semantic_metrics.as_ref().and_then(|source| {
                result_semantic_counts
                    .as_ref()
                    .and_then(|result| semantic_coverage(source, result))
            });
            let interactive_shell = is_interactive_shell(result_metrics, shell_evidence)
                || is_application_shell_notice(&runner.dom, result_root, result_metrics);
            let incoherent_short = is_incoherent_short_result(result_metrics);
            let AttemptRunner {
                dom: attempt_dom,
                scratch,
                cleanup_actions,
                normalization,
                ..
            } = runner;
            attempt_scratch = scratch;
            self.diagnostic_cleanup_actions = cleanup_actions.clone();
            self.diagnostic_normalization = normalization;
            let attempt_source_metrics = if strategy == ExtractionStrategy::RelaxedVisibility {
                relaxed_source_metrics
            } else {
                source_metrics
            };
            let quality = ExtractionQuality::new(
                attempt_source_metrics,
                result_metrics,
                selection.node != body && strategy != ExtractionStrategy::BodyFallback,
            );
            debug_log!(
                "Extraction strategy {:?}: words={}, coverage={:.3}, links={:.3}",
                strategy,
                quality.word_count,
                quality.coverage,
                quality.link_density
            );

            let verdict = Self::evaluate_attempt(AttemptPolicyInput {
                strategy,
                structured_root,
                selection_node: selection.node,
                quality,
                metrics: result_metrics,
                best: self.best_attempt.as_ref(),
                has_relaxable_hidden_content,
                visibility_recovery_needed,
                short_source_access_barrier,
                root_in_document_chrome,
                access_barrier,
                interactive_shell,
                incoherent_short,
                visibility_root_semantic,
                semantic_root_complete_candidate,
                semantic_root_boilerplate,
                rejected_link_only_semantic_root: &mut rejected_link_only_semantic_root,
            });
            if verdict.accepted {
                self.page_direction = source_direction.clone();
                self.page_byline = plan.byline.clone();
                let excerpt = self.content_excerpt_if_needed(&attempt_dom, result_root);
                self.record_attempt(
                    strategy,
                    root_info,
                    attempt_source_metrics,
                    result_metrics,
                    quality,
                    semantic_coverage,
                    representation,
                    true,
                    verdict.acceptance_exception,
                    None,
                );
                let document = if let Some(document) = result_document {
                    document
                } else {
                    crate::document::compile_document_owned(
                        attempt_dom,
                        result_root,
                        &compile_context,
                        crate::document::CompileInputs {
                            source_facts: source_facts.as_ref(),
                            source_evidence: Some(&source_evidence),
                            retained_stream: Some(&cleaned_analysis.retained_stream),
                            ordinary_plan: cleaned_analysis.ordinary_plan.as_ref(),
                            ordinary_checked: cleaned_analysis.ordinary_checked,
                        },
                    )
                    .map_err(|_| Error::NoContent)?
                };
                return Ok(ExtractedContent { excerpt, document });
            }

            let rejection = Self::attempt_rejection_reason(
                root_in_document_chrome,
                access_barrier,
                short_source_access_barrier && !verdict.ignores_visible_source_barrier,
                verdict.link_only_semantic_root,
                interactive_shell,
                incoherent_short,
                verdict.visibility_improves,
                verdict.deferred_for_visibility,
            );
            let diagnostic_index = self.record_attempt(
                strategy,
                root_info,
                attempt_source_metrics,
                result_metrics,
                quality,
                semantic_coverage.clone(),
                representation,
                false,
                false,
                Some(rejection),
            );
            let mut cached = CachedPhysicalAttempt::from_result(
                result_metrics,
                semantic_coverage.clone(),
                representation,
                cleanup_actions,
                normalization,
                access_barrier,
                interactive_shell,
                incoherent_short,
            );
            let can_be_best = verdict.valid_result
                && verdict.visibility_improves
                && self.best_attempt.as_ref().is_none_or(|best| {
                    quality.best_attempt_score() > best.quality.best_attempt_score()
                });
            cached.excerpt = self.content_excerpt_if_needed(&attempt_dom, result_root);
            let CleanedFragmentAnalysis {
                retained_stream,
                ordinary_plan,
                ordinary_checked,
                ..
            } = cleaned_analysis;
            cached.content = Some(FrozenContent {
                dom: attempt_dom,
                source_facts,
                source_evidence,
                retained_stream: Some(retained_stream),
                ordinary_plan,
                ordinary_checked,
            });
            let excerpt = cached.excerpt.clone();
            physical_attempts[physical_attempt.0].cached = Some(cached);
            if can_be_best {
                if let Some(previous) = self
                    .best_attempt
                    .as_ref()
                    .and_then(|best| best.diagnostic_index)
                    && let Some(attempts) = self.diagnostic_attempts.as_mut()
                {
                    attempts[previous].rejection_reason = Some(AttemptRejectionReason::Superseded);
                }
                self.best_attempt = Some(BestAttempt {
                    physical_attempt,
                    quality,
                    excerpt,
                    direction: source_direction.clone(),
                    strategy,
                    byline: plan.byline.clone(),
                    diagnostic_index,
                });
            }
        }

        let best = self.best_attempt.take().ok_or(Error::NoContent)?;
        if !best.quality.is_good() && best.quality.is_suspiciously_small() {
            return Err(Error::NoContent);
        }
        self.page_direction = best.direction;
        self.page_byline = best.byline;
        self.strategy = best.strategy;
        if let Some(index) = best.diagnostic_index
            && let Some(attempts) = self.diagnostic_attempts.as_mut()
        {
            attempts[index].accepted = true;
            attempts[index].rejection_reason = None;
        }
        let FrozenContent {
            dom,
            source_facts,
            source_evidence,
            retained_stream,
            ordinary_plan,
            ordinary_checked,
        } = physical_attempts[best.physical_attempt.0]
            .cached
            .take()
            .and_then(|cached| cached.content)
            .ok_or(Error::NoContent)?;
        let root = dom.root();
        let document = crate::document::compile_document_owned(
            dom,
            root,
            &compile_context,
            crate::document::CompileInputs {
                source_facts: source_facts.as_ref(),
                source_evidence: Some(&source_evidence),
                retained_stream: retained_stream.as_ref(),
                ordinary_plan: ordinary_plan.as_ref(),
                ordinary_checked,
            },
        )
        .map_err(|_| Error::NoContent)?;
        Ok(ExtractedContent {
            excerpt: best.excerpt,
            document,
        })
    }

    fn intern_physical_plan(
        mut plan: AttemptPlan,
        physical_plan: PhysicalPlan,
        physical_attempts: &mut Vec<PhysicalAttempt>,
    ) -> AttemptPlan {
        let physical_attempt = Self::physical_attempt_id(&physical_plan, physical_attempts);
        plan.physical_attempt = physical_attempt;
        plan
    }

    fn physical_attempt_id(
        physical_plan: &PhysicalPlan,
        physical_attempts: &mut Vec<PhysicalAttempt>,
    ) -> PhysicalAttemptId {
        if let Some(index) = physical_attempts
            .iter()
            .position(|attempt| attempt.plan == *physical_plan)
        {
            return PhysicalAttemptId(index);
        }
        let id = PhysicalAttemptId(physical_attempts.len());
        crate::instrumentation::record_unique_attempt_plan(id.0 as u8);
        physical_attempts.push(PhysicalAttempt {
            plan: physical_plan.clone(),
            cached: None,
        });
        id
    }

    fn build_attempt_plan(
        &mut self,
        ctx: &PlanContext<'_>,
        strategy: ExtractionStrategy,
        text_buffer: &mut String,
        analysis_cache: &mut AnalysisCache,
    ) -> Result<(AttemptPlan, PhysicalPlan)> {
        let visibility = strategy.visibility_variant();
        let analysis_index = if let Some(index) = analysis_cache.find(visibility) {
            index
        } else {
            #[cfg(test)]
            record_generic_scoring_call();
            let analysis = self.build_scoring_analysis(ctx, visibility, text_buffer)?;
            analysis_cache.variants.push((visibility, analysis));
            analysis_cache.variants.len() - 1
        };
        if !strategy.weight_classes() {
            let analysis = &mut analysis_cache.variants[analysis_index].1;
            if analysis.unweighted.is_none() {
                self.prepare_unweighted_scoring(
                    analysis,
                    ctx.prepared_source,
                    ctx.content_hint_targets,
                    ctx.structured_root,
                );
            }
        }
        let scores = analysis_cache.variants[analysis_index]
            .1
            .take_scores(strategy.weight_classes())
            .ok_or(Error::NoContent)?;
        let analysis = &mut analysis_cache.variants[analysis_index].1;
        let discovery = &analysis.discovery;
        let variant = analysis
            .variant(strategy.weight_classes())
            .ok_or(Error::NoContent)?;
        let candidates = &variant.candidates;
        let ranked = &variant.ranked;
        let body = analysis.body;
        let mut scores = scores;
        let working_dom = &self.dom;
        let _root_selection_phase = PhaseGuard::new(Phase::RootSelection);
        let mut selection = select_content_root(
            working_dom,
            candidates,
            ranked,
            body,
            ctx.structured_texts.iter().copied(),
        );
        selection = self.selection_for_strategy(
            strategy,
            working_dom,
            body,
            selection,
            ctx.structured_root,
        );
        if strategy == ExtractionStrategy::RelaxedVisibility
            && ctx.short_source_access_barrier
            && let Some(hidden) = ranked
                .iter()
                .find(|candidate| self.is_inside_static_hidden(candidate.node))
        {
            selection = RootSelection {
                node: hidden.node,
                reason: RootSelectionReason::SpecificChild,
                branches: SmallVec::new(),
            };
        }
        let visibility_root_semantic =
            candidates.is_authoritative_semantic(working_dom, selection.node);
        let semantic_root_complete_candidate = semantic_root_has_complete_candidate(
            working_dom,
            candidates,
            ranked,
            selection.node,
            body,
        );
        let semantic_root_boilerplate = is_boilerplate_root_node(working_dom, selection.node);
        let root_info = self.root_info(
            working_dom,
            candidates,
            &selection,
            ranked.first().map(|candidate| candidate.node),
        );
        let root_in_document_chrome =
            Self::is_document_chrome_root(working_dom, selection.node, body);
        if selection.node == body {
            selection.branches.clear();
        }

        let mut top_id = selection.node;
        let rename_top = selection.reason == RootSelectionReason::CompleteAncestor
            && matches!(working_dom.tag(top_id), Some(Tag::Article | Tag::Main));
        if !selection.branches.is_empty() || selection.node == body {
            top_id = selection.branches.first().copied().unwrap_or(body);
        } else {
            if selection.reason == RootSelectionReason::Ranked {
                let top_score = ranked[0].score;
                let alternatives: SmallVec<[SmallVec<[NodeId; 16]>; 3]> = ranked
                    .iter()
                    .skip(1)
                    .filter(|candidate| candidate.score / top_score >= 0.75)
                    .filter(|candidate| {
                        candidates.get(candidate.node).is_some_and(|candidate| {
                            candidate.has_source(CandidateSource::Readability)
                                || candidate.has_source(CandidateSource::Semantic)
                        })
                    })
                    .map(|candidate| {
                        analysis
                            .view
                            .effective_ancestors(working_dom, candidate.node)
                    })
                    .collect();
                if alternatives.len() >= 3 {
                    let mut parent = analysis.view.effective_parent(working_dom, top_id);
                    while let Some(node) = parent {
                        if node == body {
                            break;
                        }
                        if alternatives
                            .iter()
                            .filter(|ancestors| ancestors.contains(&node))
                            .count()
                            >= 3
                        {
                            top_id = node;
                            break;
                        }
                        parent = analysis.view.effective_parent(working_dom, node)
                    }
                }
                if !scores.has(top_id) {
                    initialize_score_node(
                        working_dom,
                        top_id,
                        &mut scores,
                        self.strategy.weight_classes(),
                    )
                }
                let threshold = scores.get(top_id) / 3.0;
                let mut last = scores.get(top_id);
                let mut parent = analysis.view.effective_parent(working_dom, top_id);
                while let Some(node) = parent {
                    if node == body {
                        break;
                    }
                    if let Some(score) = scores.get_if_initialized(node) {
                        if score < threshold {
                            break;
                        }
                        if score > last {
                            top_id = node;
                            break;
                        }
                        last = score;
                    }
                    parent = analysis.view.effective_parent(working_dom, node)
                }
                while let Some(parent) = analysis.view.effective_parent(working_dom, top_id) {
                    if parent == body {
                        break;
                    }
                    let children = analysis
                        .view
                        .effective_element_children(working_dom, parent);
                    if children.len() == 1 {
                        top_id = parent;
                    } else {
                        break;
                    }
                }
            }
            if selection.reason == RootSelectionReason::Ranked
                && (working_dom.tag(top_id) == Some(Tag::Pre)
                    || working_dom
                        .descendants(top_id)
                        .any(|node| working_dom.tag(node) == Some(Tag::Pre)))
                && let Some(article) = analysis
                    .view
                    .effective_ancestors(working_dom, top_id)
                    .into_iter()
                    .take(8)
                    .find(|&ancestor| {
                        matches!(working_dom.tag(ancestor), Some(Tag::Article | Tag::Main))
                            && working_dom.normalized_char_count(ancestor) <= 10_000
                    })
                && has_compact_code_page_structure(working_dom, article)
                && (has_compact_code_lead(working_dom, article, top_id)
                    || has_line_number_table_marker(working_dom, top_id))
            {
                top_id = article;
            }
            if !scores.has(top_id) {
                initialize_score_node(
                    working_dom,
                    top_id,
                    &mut scores,
                    self.strategy.weight_classes(),
                )
            }
        }

        let synthetic = !selection.branches.is_empty() || selection.node == body;
        let lead_media = (!synthetic && selection.reason != RootSelectionReason::ArticleBody)
            .then(|| adjacent_lead_media(working_dom, top_id))
            .flatten();
        let direction_root = selection
            .branches
            .first()
            .and_then(|&branch| working_dom.parent(branch))
            .unwrap_or(top_id);
        let source_direction = std::iter::once(direction_root)
            .chain(
                analysis
                    .view
                    .effective_ancestors(working_dom, direction_root),
            )
            .find_map(|node| working_dom.attr(node, AttrName::Dir))
            .map(str::to_owned);
        let mut source_siblings: SmallVec<[NodeId; 16]> = if !synthetic {
            if selection.reason == RootSelectionReason::Ranked {
                Self::gather_siblings(
                    working_dom,
                    &analysis.view,
                    top_id,
                    &mut analysis.shared_facts,
                    &mut scores,
                )
                .into_iter()
                .collect()
            } else {
                SmallVec::from_slice(&[top_id])
            }
        } else if !selection.branches.is_empty() {
            selection.branches.iter().copied().collect()
        } else {
            SmallVec::from_slice(&[body])
        };
        source_siblings.retain(|node| {
            !analysis
                .excluded_mask
                .get(node.index())
                .copied()
                .unwrap_or(false)
        });
        let physical_plan = PhysicalPlan {
            source_roots: source_siblings.clone(),
            selection_node: selection.node,
            top_id,
            synthetic,
            visibility,
            conditional_cleanup: strategy.conditional_cleanup(),
            body_fallback: strategy == ExtractionStrategy::BodyFallback,
            rename_top,
            lead_media,
        };
        let plan = AttemptPlan {
            strategy,
            visibility,
            analysis_index,
            selection,
            physical_attempt: PhysicalAttemptId(usize::MAX),
            source_direction,
            root_info,
            root_in_document_chrome,
            visibility_root_semantic,
            semantic_root_complete_candidate,
            semantic_root_boilerplate,
            byline: discovery.byline.clone(),
        };
        analysis_cache.variants[analysis_index]
            .1
            .restore_scores(strategy.weight_classes(), scores);
        Ok((plan, physical_plan))
    }

    fn prepare_unweighted_scoring(
        &self,
        analysis: &mut ScoringAnalysis,
        prepared_source: &SourceAnalysis,
        content_hint_targets: &[NodeId],
        structured_root: Option<NodeId>,
    ) {
        let _scoring_phase = PhaseGuard::new(Phase::Scoring);
        analysis.shared_facts.enable_source_stats();
        let (unweighted, _, _) = self.score_candidates(
            &self.dom,
            prepared_source,
            &analysis.view,
            &analysis.discovery,
            &analysis.excluded_mask,
            false,
            content_hint_targets,
            structured_root,
            analysis.weighted.candidates.document_evidence(),
            analysis.body,
            Some(&analysis.feature_index),
            Some(&analysis.working_snapshot),
            Some(&analysis.weighted.candidates),
            &mut analysis.shared_facts,
        );
        analysis.unweighted = Some(unweighted);
    }

    fn build_scoring_analysis(
        &mut self,
        ctx: &PlanContext<'_>,
        visibility: VisibilityVariant,
        text_buffer: &mut String,
    ) -> Result<ScoringAnalysis> {
        // Discovery is source-only and therefore shared by every strategy with
        // the same visibility policy. Keep its side effects out of the retry
        // loop so a rejected attempt cannot alter the next plan.
        let mut discovery = {
            let _phase = PhaseGuard::new(Phase::CandidateDiscovery);
            self.discover_candidates_with_indexes(
                text_buffer,
                ctx.prepared_source,
                ctx.accessible_math,
                ctx.title_plan,
                ctx.base_candidates,
                visibility,
            )
        };
        if let Some(root) = self.specialized_root {
            // Specialized extraction has already separated content from page
            // chrome. Keep its canonical root and metadata boundary intact.
            discovery.remove_after_scoring.retain(|node| {
                *node != root && !self.dom.ancestors(*node).any(|ancestor| ancestor == root)
            });
            discovery.byline = None;
        }

        let _scoring_phase = PhaseGuard::new(Phase::Scoring);
        let working_dom = &self.dom;
        let working_root = working_dom.root();
        let excluded_mask = build_exclusion_mask_with_source(
            working_dom,
            ctx.prepared_source,
            &discovery.remove_after_scoring,
        );
        let view = ScoringView::build_with_exclusions(
            working_dom,
            ctx.prepared_source,
            &discovery.divs_to_prepare,
            &discovery.candidates,
            &excluded_mask,
        );
        let body = ctx
            .source_anchors
            .body
            .filter(|&node| {
                working_dom.tag(node) == Some(Tag::Body)
                    && (node == working_root || working_dom.parent(node).is_some())
            })
            .ok_or(Error::NoBody)?;

        // Class weighting changes only score inputs. Keep the normal weighted
        // ranking on the shared scoring view. The unweighted variant is built
        // lazily only if a later broad or body-fallback strategy needs it.
        let mut shared_facts = NodeStateStore::new();
        let (weighted, feature_index, working_snapshot) = self.score_candidates(
            working_dom,
            ctx.prepared_source,
            &view,
            &discovery,
            &excluded_mask,
            true,
            ctx.content_hint_targets,
            ctx.structured_root,
            ctx.document_evidence,
            body,
            None,
            None,
            None,
            &mut shared_facts,
        );
        crate::instrumentation::record_scoring_nodes(working_snapshot.len());

        Ok(ScoringAnalysis {
            view,
            discovery,
            weighted,
            unweighted: None,
            shared_facts,
            working_snapshot,
            excluded_mask,
            feature_index,
            body,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn score_candidates(
        &self,
        dom: &Dom,
        prepared_source: &SourceAnalysis,
        view: &ScoringView,
        discovery: &CandidateDiscovery,
        excluded_mask: &[bool],
        weight_classes: bool,
        content_hint_targets: &[NodeId],
        structured_root: Option<NodeId>,
        document_evidence: DocumentEvidence,
        body: NodeId,
        shared_feature_index: Option<&CandidateFeatureIndex>,
        working_snapshot: Option<&[(NodeId, u32)]>,
        base_candidates: Option<&CandidateSet>,
        shared_facts: &mut NodeStateStore,
    ) -> (ScoredVariant, CandidateFeatureIndex, Vec<(NodeId, u32)>) {
        let mut scores = ScoreStore::new();
        if discovery.has_links {
            shared_facts.enable_link_lengths();
        }
        let mut to_score = SmallVec::<[NodeId; 256]>::new();
        for &node in &discovery.to_score {
            if scores.mark_seen(node) {
                to_score.push(node);
            }
        }
        let readability_scores = compute_readability_scores_in_view(
            dom,
            prepared_source,
            view,
            to_score,
            excluded_mask,
            shared_facts,
            &mut scores,
            weight_classes,
        );
        let mut computed_snapshot = None;
        let working_snapshot = if let Some(snapshot) = working_snapshot {
            snapshot
        } else {
            computed_snapshot = Some(
                prepared_source
                    .elements()
                    .filter(|entry| {
                        !excluded_mask
                            .get(entry.node.index())
                            .copied()
                            .unwrap_or(false)
                            && !view.ignores_wrapper(entry.node)
                    })
                    .map(|entry| (entry.node, entry.depth))
                    .collect(),
            );
            computed_snapshot
                .as_deref()
                .unwrap_or(&[] as &[(NodeId, u32)])
        };

        let working_root = dom.root();
        let mut candidates = base_candidates.unwrap_or(&discovery.candidates).clone();
        if base_candidates.is_some() {
            candidates.reset_variant_state();
        }
        candidates.set_document_evidence(document_evidence);
        for &node in content_hint_targets {
            let attached = node == working_root
                || dom.ancestors(node).any(|ancestor| ancestor == working_root);
            if attached
                && self
                    .options
                    .content_hint
                    .as_ref()
                    .is_some_and(|hint| content_target_matches(dom, node, hint))
            {
                candidates.add_caller_hint(node);
            }
        }
        if let Some(root) = structured_root {
            candidates.add_structured_data(root);
        }

        let (ranked, feature_index) = Self::rank_candidates_with_snapshot_and_scores(
            dom,
            Some(prepared_source),
            Some(view),
            body,
            working_snapshot,
            &mut candidates,
            readability_scores,
            excluded_mask,
            shared_facts,
            &mut scores,
            weight_classes,
            TOP_CANDIDATES,
            shared_feature_index,
        );
        (
            ScoredVariant {
                scores,
                candidates,
                ranked,
            },
            feature_index,
            computed_snapshot.unwrap_or_default(),
        )
    }

    fn extract_exact_root(
        &mut self,
        root: NodeId,
        origin: ExactRootOrigin,
        compile_context: &crate::document::CompileContext,
        anchors: DocumentAnchors,
    ) -> Result<ExtractedContent> {
        let _root_selection_phase = PhaseGuard::new(Phase::RootSelection);
        self.strategy = ExtractionStrategy::Normal;
        crate::instrumentation::record_strategy(ExtractionStrategy::Normal as u8);
        self.diagnostic_cleanup_actions.clear();
        self.diagnostic_normalization = NormalizationCountsInfo::default();

        let body = anchors
            .body
            .filter(|&node| {
                self.dom.tag(node) == Some(Tag::Body) && self.dom.parent(node).is_some()
            })
            .ok_or(Error::NoBody)?;
        let mut ancestor = Some(root);
        let mut root_attached = false;
        while let Some(node) = ancestor {
            if node == self.dom.root() {
                root_attached = true;
                break;
            }
            ancestor = self.dom.parent(node);
        }
        if !self.dom.is_element(root) || !root_attached {
            return Err(match origin {
                ExactRootOrigin::Caller => Error::ContentRootNotFound,
                ExactRootOrigin::Specialized => Error::NoContent,
            });
        }

        let source_metrics = ContentMetrics::measure(&self.dom, root);
        if !source_metrics.has_meaningful_text() {
            return Err(Error::NoContent);
        }
        if let Some(html) = anchors.html.filter(|&node| {
            self.dom.tag(node) == Some(Tag::Html) && self.dom.parent(node).is_some()
        }) {
            if let Some(lang) = self.dom.attr(html, AttrName::Lang) {
                self.page_language = Some(lang.into());
            }
            if let Some(dir) = self.dom.attr(html, AttrName::Dir) {
                self.page_direction = Some(dir.into());
            }
        }

        let root_info = self.exact_root_info(&self.dom, root, origin);
        #[cfg(feature = "bench-instrumentation")]
        drop(_root_selection_phase);

        let mut fragment = {
            let _phase = PhaseGuard::new(Phase::FragmentCopy);
            self.dom
                .copy_subtree_as_fragment(root)
                .map_err(|_| Error::NoContent)?
        };
        let copied_root = fragment
            .first_child(fragment.root())
            .ok_or(Error::NoContent)?;
        Self::normalize_exact_root_structure(&mut fragment, copied_root, origin);
        let synthetic = root == body;
        let (_top_id, content_id) = if synthetic {
            Self::prune_body_fallback_chrome(&mut fragment, copied_root);
            fragment.reserve_additional_nodes_exact(1);
            let container = fragment
                .create_html_element(Tag::Div)
                .map_err(|_| Error::NoContent)?;
            let children: SmallVec<[NodeId; 16]> = fragment.children(copied_root).collect();
            for child in children {
                fragment.append_child(container, child);
            }
            fragment.detach(copied_root);
            fragment.append_child(fragment.root(), container);
            (container, container)
        } else {
            let content_id = Self::create_container(&mut fragment, copied_root, &[copied_root])
                .unwrap_or(copied_root);
            (copied_root, content_id)
        };
        let title_snapshot = fragment.element_descendants_snapshot_with_depth(fragment.root());
        let title_plan = title_heading_plan(
            &fragment,
            SourceElements::Snapshot(&title_snapshot),
            &self.page_title,
            &self.structured_title,
            self.metadata.site_name.as_deref(),
            self.source_uri.as_ref(),
        );
        let mut text_buffer = String::new();
        let byline = self.prepare_exact_fragment(
            &mut fragment,
            content_id,
            origin,
            &title_plan,
            &mut text_buffer,
        );
        remove_title_brand_headings(&mut fragment, content_id, &title_plan);
        let mut runner = AttemptRunner::new(self, fragment, AttemptScratch::default());
        runner.scratch.workspace.reset();
        runner.scratch.node_data.enable_link_lengths();

        let _cleanup_phase = PhaseGuard::new(Phase::Cleanup);
        let shell_evidence = interactive_shell_evidence(&runner.dom, content_id);
        let video = regexps::VIDEOS.clone();
        let (candidate_semantic_metrics, source_evidence) = runner.source.extractor.prep_article(
            &mut runner.dom,
            content_id,
            root != body,
            compile_context,
            &video,
            &mut text_buffer,
            &mut runner.scratch.cleaning_nodes,
            &mut runner.scratch.node_data,
            &mut runner.scratch.workspace,
            &mut runner.cleanup_actions,
        );
        if synthetic {
            runner
                .dom
                .set_attr(content_id, AttrName::Id, "legible-content");
            runner.dom.set_attr(content_id, AttrName::Class, "page");
        } else {
            let wrapper = runner
                .dom
                .create_html_element(Tag::Div)
                .map_err(|_| Error::NoContent)?;
            runner
                .dom
                .set_attr(wrapper, AttrName::Id, "legible-content");
            runner.dom.set_attr(wrapper, AttrName::Class, "page");
            let children: SmallVec<[NodeId; 16]> = runner.dom.children(content_id).collect();
            for child in children {
                runner.dom.append_child(wrapper, child);
            }
            runner.dom.append_child(content_id, wrapper);
        }

        let access_barrier = is_access_barrier(&runner.dom, content_id);
        let (mut source_facts, mut retained_stream) = runner.source.extractor.final_cleanup(
            &mut runner.dom,
            content_id,
            &source_evidence,
            &mut runner.scratch.cleaning_nodes,
            &mut runner.cleanup_actions,
        );
        crate::instrumentation::record_cleaned_nodes(runner.dom.len());
        runner.scratch.workspace.invalidate();
        runner.source.extractor.capture_normalization_counts(
            &runner.dom,
            content_id,
            &mut runner.scratch.workspace,
            &mut runner.normalization,
        );
        #[cfg(feature = "bench-instrumentation")]
        drop(_cleanup_phase);
        if synthetic
            && content_id != runner.dom.root()
            && let Some(retained_stream) = retained_stream.as_mut()
        {
            retained_stream.prepend_root(content_id);
        }
        if !synthetic {
            let fragment_root = runner.dom.root();
            runner.dom.move_children(content_id, fragment_root);
            runner.dom.detach(content_id);
        }
        let result_root = runner.dom.root();
        if let Some(source_facts) = source_facts.as_mut() {
            source_facts.rebase_root(&runner.dom, result_root);
        }
        let cleaned_analysis = retained_stream
            .take()
            .map(|stream| {
                CleanedFragmentAnalysis::from_retained_stream(
                    &runner.dom,
                    result_root,
                    stream,
                    Some(&source_evidence),
                )
            })
            .ok_or(Error::NoContent)?;
        let ordinary_plan = cleaned_analysis.ordinary_plan.as_ref();

        let result_document = if self.diagnostic_attempts.is_some() {
            Some(
                crate::document::compile_document(
                    &runner.dom,
                    result_root,
                    compile_context,
                    &crate::document::CompileInputs {
                        source_facts: source_facts.as_ref(),
                        source_evidence: Some(&source_evidence),
                        retained_stream: Some(&cleaned_analysis.retained_stream),
                        ordinary_plan,
                        ordinary_checked: cleaned_analysis.ordinary_checked,
                    },
                )
                .map_err(|_| Error::NoContent)?,
            )
        } else {
            None
        };
        let representation = result_document
            .as_ref()
            .map(|document| RepresentationMetricsInfo {
                source_dom_nodes: self.source_dom_nodes,
                final_dom_nodes: {
                    crate::instrumentation::record_final_dom_node_scan();
                    1 + runner.dom.descendants(result_root).count()
                },
                document_nodes: document.len(),
                estimated_document_bytes: document.retained_bytes_estimate(),
            });
        let result_metrics = result_document.as_ref().map_or_else(
            || cleaned_analysis.metrics,
            ContentMetrics::measure_document,
        );
        let result_semantic_counts = result_document.as_ref().and_then(|document| {
            candidate_semantic_metrics
                .as_ref()
                .map(|_| SemanticStructureCounts::measure(document))
        });
        let semantic_coverage = candidate_semantic_metrics.as_ref().and_then(|source| {
            result_semantic_counts
                .as_ref()
                .and_then(|result| semantic_coverage(source, result))
        });
        let quality = ExtractionQuality::new(source_metrics, result_metrics, root != body);
        let root_in_document_chrome = false;
        let interactive_shell = is_interactive_shell(result_metrics, shell_evidence)
            || is_application_shell_notice(&runner.dom, result_root, result_metrics);
        let incoherent_short = is_incoherent_short_result(result_metrics);
        let AttemptRunner {
            dom: attempt_dom,
            scratch: _,
            cleanup_actions,
            normalization,
            ..
        } = runner;
        self.diagnostic_cleanup_actions = cleanup_actions;
        self.diagnostic_normalization = normalization;
        let valid_result = result_metrics.has_meaningful_text();
        if !valid_result {
            self.record_attempt(
                ExtractionStrategy::Normal,
                root_info,
                source_metrics,
                result_metrics,
                quality,
                semantic_coverage,
                representation,
                false,
                false,
                Some(Self::attempt_rejection_reason(
                    root_in_document_chrome,
                    access_barrier,
                    false,
                    false,
                    interactive_shell,
                    incoherent_short,
                    false,
                    false,
                )),
            );
            return Err(Error::NoContent);
        }

        self.page_byline = byline;
        let excerpt = self.content_excerpt_if_needed(&attempt_dom, result_root);
        self.record_attempt(
            ExtractionStrategy::Normal,
            root_info,
            source_metrics,
            result_metrics,
            quality,
            semantic_coverage,
            representation,
            true,
            false,
            None,
        );
        let document = if let Some(document) = result_document {
            document
        } else {
            crate::document::compile_document(
                &attempt_dom,
                result_root,
                compile_context,
                &crate::document::CompileInputs {
                    source_facts: source_facts.as_ref(),
                    source_evidence: Some(&source_evidence),
                    retained_stream: Some(&cleaned_analysis.retained_stream),
                    ordinary_plan,
                    ordinary_checked: cleaned_analysis.ordinary_checked,
                },
            )
            .map_err(|_| Error::NoContent)?
        };
        Ok(ExtractedContent { excerpt, document })
    }

    fn prepare_exact_fragment(
        &self,
        dom: &mut Dom,
        root: NodeId,
        origin: ExactRootOrigin,
        title_plan: &TitleHeadingPlan,
        text_buffer: &mut String,
    ) -> Option<String> {
        if origin != ExactRootOrigin::Caller {
            return None;
        }
        let snapshot = dom.element_descendants_snapshot_with_depth(root);
        let accessible_math = accessible_math_nodes(dom, &snapshot);
        let heading_limit = heading_text_limit(&self.page_title, &self.structured_title);
        let retain_preferred_title =
            title_plan.preferred.is_some() && !title_plan.brand_headings.is_empty();
        let mut remove_title = !retain_preferred_title;
        let mut byline = None;
        let mut excluded_depth = None;
        for &(node, depth) in &snapshot {
            if let Some(root_depth) = excluded_depth {
                if depth > root_depth {
                    continue;
                }
                excluded_depth = None;
            }
            if (!accessible_math.contains(&node) && !is_probably_visible(dom, node))
                || Self::is_modal_or_dialog_in(dom, node)
            {
                dom.detach(node);
                excluded_depth = Some(depth);
                continue;
            }
            if byline.is_none() && !self.metadata.has_source_author {
                if is_valid_byline(dom, node, text_buffer) {
                    byline = Some(
                        metadata::byline_name(dom, node)
                            .unwrap_or_else(|| get_inner_text(dom, node, text_buffer).to_owned()),
                    );
                    dom.detach(node);
                    excluded_depth = Some(depth);
                    continue;
                }
            }
            let duplicates_title = if has_primary_heading_semantics(dom, node) {
                let heading = get_inner_text_limited(dom, node, text_buffer, heading_limit);
                let matches_title = heading_matches_page_title(&self.page_title, heading)
                    && (!self.metadata.title_from_content_heading
                        || heading_matches_page_title(&self.structured_title, heading));
                matches_title
                    && if retain_preferred_title {
                        title_plan.preferred != Some(node)
                    } else {
                        remove_title
                    }
            } else {
                false
            };
            if duplicates_title {
                remove_title = false;
                dom.detach(node);
                excluded_depth = Some(depth);
            }
        }
        byline
    }

    fn normalize_exact_root_structure(dom: &mut Dom, root: NodeId, origin: ExactRootOrigin) {
        match origin {
            ExactRootOrigin::Caller => {
                if dom.tag(root) == Some(Tag::Div) {
                    wrap_exact_phrasing_content_in_p(dom, root);
                }
            }
            ExactRootOrigin::Specialized => {
                let nodes = dom.element_descendants_snapshot_with_depth(root);
                for &(node, _) in &nodes {
                    if node == root
                        || dom.parent(node).is_none()
                        || dom.tag(node) != Some(Tag::Div)
                        || !is_probably_visible(dom, node)
                        || Self::is_modal_or_dialog_in(dom, node)
                        || !has_exact_single_paragraph_child(dom, node)
                    {
                        continue;
                    }
                    let Some(paragraph) = dom.element_children(node).next() else {
                        continue;
                    };
                    dom.replace_with(node, paragraph);
                }
            }
        }
    }

    fn exact_root_info(
        &self,
        dom: &Dom,
        root: NodeId,
        origin: ExactRootOrigin,
    ) -> Option<RootInfo> {
        self.diagnostic_attempts.as_ref()?;
        Some(RootInfo {
            tag: dom.qual_name(root).map(|name| name.local.to_string()),
            id: dom.attr(root, AttrName::Id).map(str::to_owned),
            classes: dom
                .attr(root, AttrName::Class)
                .map(|classes| classes.split_whitespace().map(str::to_owned).collect())
                .unwrap_or_default(),
            selection_reason: RootSelectionReason::SpecificChild.into(),
            candidate_sources: match origin {
                ExactRootOrigin::Caller => vec![CandidateSourceInfo::CallerHint],
                ExactRootOrigin::Specialized => Vec::new(),
            },
        })
    }

    fn root_info(
        &self,
        dom: &Dom,
        candidates: &CandidateSet,
        selection: &RootSelection,
        ranking_winner: Option<NodeId>,
    ) -> Option<RootInfo> {
        self.diagnostic_attempts.as_ref()?;
        let candidate = candidates.get(selection.node);
        let mut candidate_sources = Vec::new();
        for (internal, public) in [
            (CandidateSource::Semantic, CandidateSourceInfo::Semantic),
            (
                CandidateSource::Readability,
                CandidateSourceInfo::Readability,
            ),
            (
                CandidateSource::StructuredData,
                CandidateSourceInfo::StructuredData,
            ),
            (CandidateSource::Generic, CandidateSourceInfo::Generic),
            (CandidateSource::CallerHint, CandidateSourceInfo::CallerHint),
        ] {
            let present = if internal == CandidateSource::CallerHint {
                self.options.content_root.is_none()
                    && ranking_winner.is_some_and(|winner| {
                        candidates
                            .get(winner)
                            .is_some_and(|candidate| candidate.has_source(internal))
                            && (winner == selection.node
                                || dom
                                    .ancestors(winner)
                                    .any(|ancestor| ancestor == selection.node)
                                || dom
                                    .ancestors(selection.node)
                                    .any(|ancestor| ancestor == winner))
                    })
            } else {
                candidate.is_some_and(|candidate| candidate.has_source(internal))
            };
            if present {
                candidate_sources.push(public);
            }
        }
        Some(RootInfo {
            tag: dom
                .qual_name(selection.node)
                .map(|name| name.local.to_string()),
            id: dom.attr(selection.node, AttrName::Id).map(str::to_owned),
            classes: dom
                .attr(selection.node, AttrName::Class)
                .map(|classes| classes.split_whitespace().map(str::to_owned).collect())
                .unwrap_or_default(),
            selection_reason: selection.reason.into(),
            candidate_sources,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn record_attempt(
        &mut self,
        strategy: ExtractionStrategy,
        root: Option<RootInfo>,
        source: ContentMetrics,
        result: ContentMetrics,
        quality: ExtractionQuality,
        semantic_coverage: Option<crate::diagnostics::SemanticCoverageInfo>,
        representation: Option<RepresentationMetricsInfo>,
        accepted: bool,
        acceptance_exception: bool,
        rejection_reason: Option<AttemptRejectionReason>,
    ) -> Option<usize> {
        let attempts = self.diagnostic_attempts.as_mut()?;
        let index = attempts.len();
        attempts.push(ExtractionAttempt {
            strategy: strategy.into(),
            selected_root: root.expect("diagnostic root is built when diagnostics are enabled"),
            source: Self::metrics_info(source),
            result: Self::metrics_info(result),
            quality: QualityInfo {
                coverage: quality.coverage,
                best_attempt_score: quality.best_attempt_score(),
                good: quality.is_good(),
                suspiciously_small: quality.is_suspiciously_small(),
            },
            semantic_coverage,
            cleanup_actions: self.diagnostic_cleanup_actions.clone(),
            normalization: self.diagnostic_normalization,
            representation: representation
                .expect("representation metrics are built when diagnostics are enabled"),
            accepted,
            acceptance_exception: acceptance_exception
                .then_some(AcceptanceExceptionInfo::TrustedSemanticRoot),
            rejection_reason,
        });
        Some(index)
    }

    fn metrics_info(metrics: ContentMetrics) -> ContentMetricsInfo {
        ContentMetricsInfo {
            word_count: metrics.word_count,
            text_chars: metrics.text_chars,
            link_text_chars: metrics.link_text_chars,
            paragraph_count: metrics.paragraph_count,
            heading_count: metrics.heading_count,
            list_item_count: metrics.list_item_count,
            code_block_count: metrics.code_block_count,
            table_count: metrics.table_count,
            figure_count: metrics.figure_count,
            image_count: metrics.image_count,
            footnote_reference_count: metrics.footnote_reference_count,
            footnote_definition_count: metrics.footnote_definition_count,
            math_count: metrics.math_count,
            structured_block_count: metrics.structured_block_count,
            link_density: metrics.link_density,
        }
    }

    fn evaluate_attempt(input: AttemptPolicyInput<'_>) -> AttemptVerdict {
        let schema_match = input.structured_root == Some(input.selection_node)
            && !input.quality.is_suspiciously_small()
            && (input.quality.coverage >= 0.2 || input.quality.text_chars >= 500);
        let ignores_visible_source_barrier = input.strategy
            == ExtractionStrategy::RelaxedVisibility
            && input.has_relaxable_hidden_content;
        let selected_link_only_semantic_root = input.strategy
            != ExtractionStrategy::RelaxedVisibility
            && input.visibility_root_semantic
            && is_link_only_semantic_root(input.metrics);
        let link_only_semantic_root = selected_link_only_semantic_root
            || input.strategy == ExtractionStrategy::BodyFallback
                && *input.rejected_link_only_semantic_root
                && is_link_only_semantic_root(input.metrics);
        *input.rejected_link_only_semantic_root |= selected_link_only_semantic_root;
        let valid_result = !input.root_in_document_chrome
            && input.metrics.has_meaningful_text()
            && !input.access_barrier
            && (!input.short_source_access_barrier || ignores_visible_source_barrier)
            && !input.interactive_shell
            && !input.incoherent_short
            && !link_only_semantic_root;
        let visibility_candidate_coherent = input.strategy != ExtractionStrategy::RelaxedVisibility
            || input.metrics.paragraph_count >= 2
            || input.visibility_root_semantic && input.metrics.structured_block_count > 0;
        let visibility_improves = visibility_candidate_coherent
            && (input.strategy != ExtractionStrategy::RelaxedVisibility
                || input.best.is_none_or(|best| {
                    input.quality.text_chars >= best.quality.text_chars.saturating_mul(2)
                        || input.quality.text_chars > best.quality.text_chars
                            && input.quality.coverage >= best.quality.coverage + 0.2
                }));
        let deferred_for_visibility = input.visibility_recovery_needed
            && input.strategy != ExtractionStrategy::RelaxedVisibility;
        let acceptance_exception = !input.quality.is_good()
            && !schema_match
            && valid_result
            && visibility_improves
            && !deferred_for_visibility
            && !input.semantic_root_boilerplate
            && Self::trusted_semantic_root(
                input.visibility_root_semantic,
                input.semantic_root_complete_candidate,
                input.metrics,
                valid_result,
            );
        let accepted = valid_result
            && visibility_improves
            && !deferred_for_visibility
            && (input.quality.is_good() || schema_match || acceptance_exception);
        AttemptVerdict {
            ignores_visible_source_barrier,
            link_only_semantic_root,
            valid_result,
            visibility_improves,
            deferred_for_visibility,
            acceptance_exception,
            accepted,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn attempt_rejection_reason(
        document_chrome: bool,
        access_barrier: bool,
        source_access_barrier: bool,
        link_only_semantic_root: bool,
        interactive_shell: bool,
        incoherent_short: bool,
        visibility_improves: bool,
        deferred_for_visibility: bool,
    ) -> AttemptRejectionReason {
        if document_chrome {
            AttemptRejectionReason::DocumentChrome
        } else if access_barrier {
            AttemptRejectionReason::AccessBarrier
        } else if source_access_barrier {
            AttemptRejectionReason::SourceAccessBarrier
        } else if link_only_semantic_root {
            AttemptRejectionReason::LinkOnlySemanticRoot
        } else if interactive_shell {
            AttemptRejectionReason::InteractiveShell
        } else if incoherent_short {
            AttemptRejectionReason::IncoherentShortResult
        } else if deferred_for_visibility {
            AttemptRejectionReason::PotentialHiddenContent
        } else if !visibility_improves {
            AttemptRejectionReason::InsufficientImprovement
        } else {
            AttemptRejectionReason::LowQuality
        }
    }

    fn trusted_semantic_root(
        semantic_root: bool,
        complete_candidate: bool,
        result: ContentMetrics,
        valid_result: bool,
    ) -> bool {
        semantic_root
            && complete_candidate
            && valid_result
            && result.has_meaningful_text()
            && semantic_root_is_complete(result)
    }

    fn prune_body_fallback_chrome(dom: &mut Dom, body: NodeId) {
        let elements = dom.element_descendants_snapshot_with_depth(body);
        let has_primary_region = elements.iter().any(|&(node, _)| {
            matches!(dom.tag(node), Some(Tag::Main | Tag::Article))
                || dom
                    .attr(node, AttrName::Role)
                    .is_some_and(|roles| has_any_token(roles, &["main", "article"]))
        });
        let mut in_primary_region = vec![false; dom.len()];
        let mut remove = SmallVec::<[NodeId; 16]>::new();
        for (node, _) in elements {
            let parent_is_primary = dom
                .parent(node)
                .is_some_and(|parent| in_primary_region[parent.index()]);
            in_primary_region[node.index()] = parent_is_primary
                || matches!(dom.tag(node), Some(Tag::Main | Tag::Article))
                || dom
                    .attr(node, AttrName::Role)
                    .is_some_and(|roles| has_any_token(roles, &["main", "article"]));
            let role = dom.attr(node, AttrName::Role);
            let document_chrome =
                matches!(dom.tag(node), Some(Tag::Header | Tag::Footer | Tag::Nav))
                    || role.is_some_and(|roles| has_any_token(roles, &["banner", "navigation"]));
            let contextual_sidebar = dom.tag(node) == Some(Tag::Aside)
                || role.is_some_and(|roles| has_token(roles, "complementary"));
            if (document_chrome || contextual_sidebar && has_primary_region)
                && !in_primary_region[node.index()]
            {
                remove.push(node);
            }
        }
        for node in remove {
            dom.detach(node);
        }
    }

    fn is_document_chrome_root(dom: &Dom, node: NodeId, body: NodeId) -> bool {
        let protected = std::iter::once(node)
            .chain(dom.ancestors(node))
            .take_while(|&ancestor| ancestor != body)
            .any(|ancestor| {
                matches!(dom.tag(ancestor), Some(Tag::Main | Tag::Article))
                    || dom
                        .attr(ancestor, AttrName::Role)
                        .is_some_and(|roles| has_any_token(roles, &["main", "article"]))
            });
        !protected
            && std::iter::once(node)
                .chain(dom.ancestors(node))
                .take_while(|&ancestor| ancestor != body)
                .any(|ancestor| {
                    matches!(
                        dom.tag(ancestor),
                        Some(Tag::Aside | Tag::Header | Tag::Footer | Tag::Nav)
                    ) || dom.attr(ancestor, AttrName::Role).is_some_and(|roles| {
                        has_any_token(roles, &["banner", "complementary", "navigation"])
                    })
                })
    }

    fn selection_for_strategy(
        &self,
        strategy: ExtractionStrategy,
        dom: &Dom,
        body: NodeId,
        normal: RootSelection,
        structured_root: Option<NodeId>,
    ) -> RootSelection {
        match strategy {
            ExtractionStrategy::Normal
            | ExtractionStrategy::RelaxedCleanup
            | ExtractionStrategy::RelaxedVisibility => normal,
            ExtractionStrategy::StructuredDataHint => RootSelection {
                node: structured_root.unwrap_or(normal.node),
                reason: RootSelectionReason::StructuredData,
                branches: SmallVec::new(),
            },
            ExtractionStrategy::BroadContent => {
                let node = std::iter::once(normal.node)
                    .chain(dom.ancestors(normal.node))
                    .find(|&node| {
                        dom.tag(node) == Some(Tag::Main)
                            || dom
                                .attr(node, AttrName::Role)
                                .is_some_and(|roles| has_token(roles, "main"))
                    })
                    .or_else(|| {
                        std::iter::once(normal.node)
                            .chain(dom.ancestors(normal.node))
                            .find(|&node| dom.tag(node) == Some(Tag::Article))
                    })
                    .unwrap_or(normal.node);
                RootSelection {
                    node,
                    reason: RootSelectionReason::SharedParent,
                    branches: SmallVec::new(),
                }
            }
            ExtractionStrategy::BodyFallback | ExtractionStrategy::MetadataFallback => {
                RootSelection {
                    node: body,
                    reason: RootSelectionReason::BodyFallback,
                    branches: SmallVec::new(),
                }
            }
        }
    }

    fn is_static_hidden_marker(&self, node: NodeId) -> bool {
        self.dom.attr(node, AttrName::AriaHidden) != Some("true")
            && (!is_probably_visible(&self.dom, node)
                || has_hidden_utility_class_for_discovery(&self.dom, node))
    }

    fn is_inside_static_hidden(&self, node: NodeId) -> bool {
        std::iter::once(node)
            .chain(self.dom.ancestors(node))
            .any(|ancestor| {
                self.is_static_hidden_marker(ancestor) && !self.is_modal_or_dialog(ancestor)
            })
    }

    /// Marks hidden roots that have semantic or repeated structural evidence.
    /// Reverse preorder aggregates each subtree without rescanning descendants.
    fn relaxed_hidden_roots(&self, prepared_source: &SourceAnalysis) -> Vec<bool> {
        let mut paragraphs = vec![0_u8; self.dom.len()];
        let mut structured = vec![false; self.dom.len()];
        let mut allowed = vec![false; self.dom.len()];
        for entry in prepared_source.elements().rev() {
            let node = entry.node;
            let tag = self.dom.tag(node);
            paragraphs[node.index()] = u8::from(tag == Some(Tag::P));
            structured[node.index()] = matches!(
                tag,
                Some(Tag::Dl | Tag::Figure | Tag::Ol | Tag::Pre | Tag::Table | Tag::Ul)
            );
            for child in self.dom.element_children(node) {
                paragraphs[node.index()] = paragraphs[node.index()]
                    .saturating_add(paragraphs[child.index()])
                    .min(2);
                structured[node.index()] |= structured[child.index()];
            }
            let authoritative = entry.flags.contains(SourceFlags::PRIMARY_REGION);
            allowed[node.index()] =
                authoritative || paragraphs[node.index()] >= 2 || structured[node.index()];
        }
        allowed
    }

    fn is_duplicate_hidden_variant(&self, node: NodeId) -> bool {
        if !self.is_static_hidden_marker(node) {
            return false;
        }
        let mut sibling = self.dom.prev_sibling(node);
        while sibling.is_some_and(|candidate| !self.dom.is_element(candidate)) {
            sibling = sibling.and_then(|candidate| self.dom.prev_sibling(candidate));
        }
        let sibling = sibling.or_else(|| {
            let mut candidate = self.dom.next_sibling(node);
            while candidate.is_some_and(|candidate| !self.dom.is_element(candidate)) {
                candidate = candidate.and_then(|candidate| self.dom.next_sibling(candidate));
            }
            candidate
        });
        let Some(sibling) = sibling.filter(|&sibling| !self.is_static_hidden_marker(sibling))
        else {
            return false;
        };
        if self.dom.tag(node) != self.dom.tag(sibling) {
            return false;
        }
        let mut node_buffer = String::new();
        let mut sibling_buffer = String::new();
        self.dom.append_text(node, &mut node_buffer);
        let node_text = node_buffer.trim();
        if node_text.chars().count() < 100 {
            return false;
        }
        self.dom.append_text(sibling, &mut sibling_buffer);
        node_text == sibling_buffer.trim()
    }

    fn is_visible_for_strategy(
        &self,
        entry: &SourceEntry,
        accessible_math: &HashSet<NodeId>,
        visibility: VisibilityVariant,
    ) -> bool {
        let node = entry.node;
        if accessible_math.contains(&node) {
            return true;
        }
        let fallback_image = entry.flags.contains(SourceFlags::FALLBACK_IMAGE);
        if entry.flags.contains(SourceFlags::ARIA_HIDDEN) && !fallback_image {
            return false;
        }
        if visibility == VisibilityVariant::Relaxed {
            true
        } else {
            (!entry.flags.contains(SourceFlags::STATIC_HIDDEN)
                && !entry.flags.contains(SourceFlags::UTILITY_HIDDEN))
                || fallback_image
        }
    }

    fn is_modal_or_dialog(&self, node: NodeId) -> bool {
        Self::is_modal_or_dialog_in(&self.dom, node)
    }

    fn is_modal_or_dialog_in(dom: &Dom, node: NodeId) -> bool {
        dom.attr(node, AttrName::AriaModal) == Some("true")
            || dom
                .attr(node, AttrName::Role)
                .is_some_and(|roles| has_any_token(roles, &["dialog", "alertdialog"]))
            || (!is_probably_visible(dom, node)
                || has_hidden_utility_class_for_discovery(dom, node))
                && dom
                    .attr(node, AttrName::Class)
                    .is_some_and(|classes| has_any_token(classes, &["modal", "dialog"]))
    }

    #[cfg(test)]
    fn discover_candidates(&mut self, text_buffer: &mut String) -> CandidateDiscovery {
        let prepared_source = SourceAnalysis::build(&self.dom);
        let accessible_math = prepared_source.accessible_math_nodes(&self.dom);
        let title_plan = title_heading_plan(
            &self.dom,
            SourceElements::Prepared(&prepared_source),
            &self.page_title,
            &self.structured_title,
            self.metadata.site_name.as_deref(),
            self.source_uri.as_ref(),
        );
        let base_candidates = prepared_source.candidates().clone();
        self.discover_candidates_with_indexes(
            text_buffer,
            &prepared_source,
            &accessible_math,
            &title_plan,
            &base_candidates,
            VisibilityVariant::Normal,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn discover_candidates_with_indexes(
        &mut self,
        text_buffer: &mut String,
        prepared_source: &SourceAnalysis,
        accessible_math: &HashSet<NodeId>,
        title_plan: &TitleHeadingPlan,
        base_candidates: &CandidateSet,
        visibility: VisibilityVariant,
    ) -> CandidateDiscovery {
        let candidates = base_candidates.clone();
        let relaxed_hidden = (visibility == VisibilityVariant::Relaxed)
            .then(|| self.relaxed_hidden_roots(prepared_source));
        let mut to_score = SmallVec::<[NodeId; 256]>::new();
        let mut divs_to_prepare = SmallVec::<[NodeId; 128]>::new();
        let mut remove_after_scoring = SmallVec::<[NodeId; 64]>::new();
        let mut has_links = false;
        let heading_limit = heading_text_limit(&self.page_title, &self.structured_title);
        let mut excluded_depth = None;
        let retain_preferred_title =
            title_plan.preferred.is_some() && !title_plan.brand_headings.is_empty();
        let mut remove_title = !retain_preferred_title;
        let mut byline = self.page_byline.clone();
        for entry in prepared_source.elements() {
            let id = entry.node;
            let depth = entry.depth;
            if let Some(root_depth) = excluded_depth {
                if depth > root_depth {
                    continue;
                }
                excluded_depth = None
            }
            let tag = self
                .dom
                .tag(id)
                .expect("element snapshot must contain only elements");
            has_links |= tag == Tag::A;
            let unsupported_hidden = relaxed_hidden.as_ref().is_some_and(|allowed| {
                entry.flags.contains(SourceFlags::STATIC_HIDDEN)
                    && (!allowed[id.index()] || self.is_duplicate_hidden_variant(id))
            });
            if !self.is_visible_for_strategy(entry, accessible_math, visibility)
                || entry.flags.contains(SourceFlags::MODAL_DIALOG)
                || unsupported_hidden
            {
                remove_after_scoring.push(id);
                excluded_depth = Some(depth);
                continue;
            }
            if byline.is_none() && !self.metadata.has_source_author {
                if is_valid_byline(&self.dom, id, text_buffer) {
                    byline =
                        Some(metadata::byline_name(&self.dom, id).unwrap_or_else(|| {
                            get_inner_text(&self.dom, id, text_buffer).to_owned()
                        }));
                    remove_after_scoring.push(id);
                    excluded_depth = Some(depth);
                    continue;
                }
            }
            let duplicates_title = if has_primary_heading_semantics(&self.dom, id) {
                let heading = get_inner_text_limited(&self.dom, id, text_buffer, heading_limit);
                let matches_title = heading_matches_page_title(&self.page_title, heading)
                    && (!self.metadata.title_from_content_heading
                        || heading_matches_page_title(&self.structured_title, heading));
                matches_title
                    && if retain_preferred_title {
                        title_plan.preferred != Some(id)
                    } else {
                        remove_title
                    }
            } else {
                false
            };
            if duplicates_title {
                remove_title = false;
                remove_after_scoring.push(id);
                excluded_depth = Some(depth);
                continue;
            }
            if matches!(
                tag,
                Tag::Div
                    | Tag::Section
                    | Tag::Header
                    | Tag::H1
                    | Tag::H2
                    | Tag::H3
                    | Tag::H4
                    | Tag::H5
                    | Tag::H6
            ) && !entry.flags.contains(SourceFlags::HAS_NON_WHITESPACE_TEXT)
                && self
                    .dom
                    .element_children(id)
                    .all(|child| matches!(self.dom.tag(child), Some(Tag::Br | Tag::Hr)))
            {
                remove_after_scoring.push(id);
                excluded_depth = Some(depth);
                continue;
            }
            if is_default_tag_to_score(tag) {
                to_score.push(id)
            }
            if tag == Tag::Div {
                divs_to_prepare.push(id)
            }
        }
        CandidateDiscovery {
            candidates,
            to_score,
            divs_to_prepare,
            remove_after_scoring,
            byline,
            has_links,
        }
    }

    #[cfg(test)]
    fn rank_candidates(
        &mut self,
        dom: &Dom,
        candidates: &mut CandidateSet,
        readability_scores: SmallVec<[ReadabilityScore; 64]>,
        excluded: &[bool],
    ) -> SmallVec<[RankedCandidate; 64]> {
        let snapshot = dom.element_descendants_snapshot_with_depth(dom.root());
        let body = dom.body().unwrap_or(dom.root());
        let mut node_data = NodeStateStore::new();
        Self::rank_candidates_with_snapshot(
            dom,
            None,
            None,
            body,
            &snapshot,
            candidates,
            readability_scores,
            excluded,
            &mut node_data,
            self.strategy.weight_classes(),
            TOP_CANDIDATES,
            None,
        )
        .0
    }

    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    fn rank_candidates_with_snapshot(
        dom: &Dom,
        source: Option<&SourceAnalysis>,
        scoring_view: Option<&ScoringView>,
        body: NodeId,
        snapshot: &[(NodeId, u32)],
        candidates: &mut CandidateSet,
        readability_scores: SmallVec<[ReadabilityScore; 64]>,
        excluded: &[bool],
        facts: &mut NodeStateStore,
        weight_classes: bool,
        top_candidates: usize,
        shared_feature_index: Option<&CandidateFeatureIndex>,
    ) -> (SmallVec<[RankedCandidate; 64]>, CandidateFeatureIndex) {
        let mut scores = ScoreStore::new();
        Self::rank_candidates_with_snapshot_and_scores(
            dom,
            source,
            scoring_view,
            body,
            snapshot,
            candidates,
            readability_scores,
            excluded,
            facts,
            &mut scores,
            weight_classes,
            top_candidates,
            shared_feature_index,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn rank_candidates_with_snapshot_and_scores(
        dom: &Dom,
        source: Option<&SourceAnalysis>,
        scoring_view: Option<&ScoringView>,
        body: NodeId,
        snapshot: &[(NodeId, u32)],
        candidates: &mut CandidateSet,
        readability_scores: SmallVec<[ReadabilityScore; 64]>,
        excluded: &[bool],
        facts: &mut NodeStateStore,
        scores: &mut ScoreStore,
        weight_classes: bool,
        top_candidates: usize,
        shared_feature_index: Option<&CandidateFeatureIndex>,
    ) -> (SmallVec<[RankedCandidate; 64]>, CandidateFeatureIndex) {
        for readability in readability_scores {
            candidates.add_readability(readability.node, readability.score);
        }

        // Readability scoring selectively invalidates ancestors after it
        // detaches deferred clutter. Reuse those refreshed statistics and the
        // unaffected leaf cache. Feature calculation uses the same tree and
        // would otherwise repeat a full postorder text scan.
        let feature_index = if let Some(feature_index) = shared_feature_index
            .filter(|feature_index| feature_index.matches_candidates(candidates))
        {
            feature_index.clone()
        } else {
            let mut table_nodes = Vec::new();
            mark_data_tables_from_snapshot(dom, dom.root(), snapshot, facts, &mut table_nodes);
            CandidateFeatureIndex::new(dom, facts, source, snapshot, candidates, scoring_view)
        };
        feature_index.prepare_text_cache(facts);
        if let Some(scoring_view) = scoring_view {
            scoring_view.seed_text_overrides(facts);
        }
        for (candidate_index, candidate) in candidates.iter_mut().enumerate() {
            candidate.features = feature_index.features(
                dom,
                source,
                candidate_index,
                *candidate,
                facts,
                weight_classes,
                excluded,
            );
        }
        let has_substantial_authoritative_root = candidates.iter().any(|candidate| {
            candidate.node != body
                && candidates.is_authoritative_semantic(dom, candidate.node)
                && candidate.features.word_count >= 20
        });
        let context = candidates.ranking_context(dom, facts, snapshot);
        let mut scored: SmallVec<[RankedCandidate; 64]> = candidates
            .iter()
            .enumerate()
            .filter_map(|(candidate_index, candidate)| {
                if excluded
                    .get(candidate.node.index())
                    .copied()
                    .unwrap_or(false)
                {
                    return None;
                }
                let length = match source {
                    Some(source) => {
                        get_or_compute_stats_from_source_excluding(
                            dom,
                            source,
                            candidate.node,
                            facts,
                            excluded,
                        )
                        .text_length
                    }
                    None => {
                        get_or_compute_stats_excluding(dom, candidate.node, facts, excluded)
                            .text_length
                    }
                };
                if length == 0 && candidate.node != body {
                    return None;
                }
                let is_semantic = candidate.has_source(CandidateSource::Semantic);
                let is_authoritative = candidates.is_authoritative_semantic(dom, candidate.node);
                let has_readability = context.has_readability(candidate_index);
                let has_meaningful_content = has_readability
                    || candidate.features.code_block_count > 0
                    || candidate.features.table_count > 0
                    || candidate.features.list_item_count >= 3
                    || candidate.features.figure_count > 0
                    || candidate.features.paragraph_count > 0
                        && candidate.features.word_count >= 3
                        && candidate.features.sentence_end_count > 0;
                if is_semantic && !is_authoritative && !has_readability && !has_meaningful_content {
                    return None;
                }
                let is_generic_only = candidate.has_source(CandidateSource::Generic)
                    && !is_semantic
                    && !candidate.has_source(CandidateSource::Readability);
                let has_distinct_structural_content = candidate.features.code_block_count > 0
                    || candidate.features.table_count > 0
                    || candidate.features.list_item_count >= 3
                    || candidate.features.figure_count > 0
                    || dom.tag(candidate.node) == Some(Tag::Td) && has_meaningful_content;
                if is_generic_only && !has_distinct_structural_content {
                    return None;
                }
                let short_semantic_bonus = if is_authoritative
                    && has_meaningful_content
                    && !context.has_authoritative_ancestor(candidate_index)
                    && !context.has_authoritative_descendant(candidate_index, is_authoritative)
                {
                    25.0 * (1.0 - (f64::from(length.min(100)) / 100.0))
                } else {
                    0.0
                };
                let is_main = dom.tag(candidate.node) == Some(Tag::Main)
                    || dom
                        .attr(candidate.node, AttrName::Role)
                        .is_some_and(|role| has_token(role, "main"));
                let (article_peer_count, article_peer_score) = if is_main {
                    context.article_peer_summary(candidate_index)
                } else {
                    (0, 0.0)
                };
                let sibling_content_bonus = if length <= 1_000 && article_peer_count >= 2 {
                    article_peer_score + 0.1
                } else {
                    0.0
                };
                // A small boundary bonus lets a focused generic container beat
                // a broad ancestor with the same structural evidence. It is too
                // small to override a Readability prose score.
                let generic_boundary_bonus =
                    if candidate.node != body && is_generic_only && has_distinct_structural_content
                    {
                        0.01
                    } else {
                        0.0
                    };
                let is_hidden_semantic_root = is_authoritative
                    && std::iter::once(candidate.node)
                        .chain(dom.ancestors(candidate.node))
                        .any(|ancestor| {
                            has_static_hidden_marker(dom, ancestor)
                                || has_hidden_utility_class(dom, ancestor)
                        });
                let complete_root_bonus = if candidate.node == body {
                    if has_substantial_authoritative_root {
                        0.0
                    } else {
                        candidates
                            .document_evidence()
                            .complete_root_bonus(candidate.features)
                    }
                } else if is_authoritative && is_hidden_semantic_root {
                    candidates
                        .document_evidence()
                        .complete_root_bonus(candidate.features)
                } else {
                    0.0
                };
                let final_score = candidate.features.ranking_score()
                    + short_semantic_bonus
                    + sibling_content_bonus
                    + generic_boundary_bonus
                    + complete_root_bonus;
                scores.set(candidate.node, final_score);
                Some(RankedCandidate {
                    node: candidate.node,
                    score: final_score,
                    order: context.source_order(candidate_index),
                })
            })
            .collect();
        let top_count = top_candidates.min(scored.len());
        if top_count < scored.len() {
            scored.select_nth_unstable_by(top_count, |a, b| {
                b.score
                    .partial_cmp(&a.score)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| a.order.cmp(&b.order))
            });
            scored.truncate(top_count);
        }
        scored.sort_unstable_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.order.cmp(&b.order))
        });
        (scored, feature_index)
    }

    fn gather_siblings(
        dom: &Dom,
        scoring_view: &ScoringView,
        top: NodeId,
        facts: &mut NodeStateStore,
        scores: &mut ScoreStore,
    ) -> SmallVec<[NodeId; 8]> {
        let Some(parent) = scoring_view.effective_parent(dom, top) else {
            let mut out = SmallVec::new();
            out.push(top);
            return out;
        };
        let threshold = 10f64.max(scores.get(top) * 0.2);
        let class = dom.attr(top, AttrName::Class);
        let mut out = SmallVec::<[NodeId; 8]>::new();
        for x in scoring_view.effective_element_children(dom, parent) {
            let mut yes = x == top;
            if !yes {
                let bonus = if class.is_some() && dom.attr(x, AttrName::Class) == class {
                    scores.get(top) * 0.2
                } else {
                    0.
                };
                if scores.has(x) && scores.get(x) + bonus >= threshold {
                    yes = true
                }
                if !yes && scoring_view.effective_tag(dom, x) == Some(Tag::P) {
                    let s = get_or_compute_stats(dom, x, facts);
                    let d = get_link_density_cached(dom, x, s.text_length, facts);
                    yes = (s.text_length > 80 && d < 0.25)
                        || (s.text_length < 80
                            && s.text_length > 0
                            && d == 0.0
                            && s.has_sentence_end())
                }
                if !yes
                    && is_near_preceding_sibling_in_view(dom, scoring_view, x, top)
                    && matches!(
                        scoring_view.effective_tag(dom, x),
                        Some(Tag::H2 | Tag::H3 | Tag::H4)
                    )
                    && [AttrName::Class, AttrName::Id]
                        .into_iter()
                        .filter_map(|attribute| dom.attr(x, attribute))
                        .flat_map(|value| {
                            value.split(|character: char| !character.is_ascii_alphanumeric())
                        })
                        .any(|token| {
                            matches!(
                                token.to_ascii_lowercase().as_str(),
                                "subtitle" | "dek" | "standfirst" | "summary"
                            )
                        })
                {
                    let stats = get_or_compute_stats(dom, x, facts);
                    yes = (30..=400).contains(&(stats.text_length as usize))
                        && get_link_density_cached(dom, x, stats.text_length, facts) == 0.0;
                }
            }
            if yes {
                debug_log!("Appending sibling node: {:?}", x);
                out.push(x)
            }
        }
        out
    }
    fn create_container(dom: &mut Dom, _top: NodeId, siblings: &[NodeId]) -> Option<NodeId> {
        let first = *siblings.first()?;
        let tags: SmallVec<[Tag; 8]> = siblings.iter().filter_map(|&node| dom.tag(node)).collect();
        let (common_table_wrapper, wrapper_count) = table_wrapper_plan(&tags, siblings.len());

        // Copied fragments are often exactly at capacity. Reserve only the
        // container and the table wrappers that this selection needs.
        dom.reserve_additional_nodes_exact(1usize.saturating_add(wrapper_count));
        let container = dom.create_html_element(Tag::Div).ok()?;
        dom.insert_before(first, container);

        // A synthetic content boundary must not break the HTML table content
        // model. Keep one common wrapper when the selected siblings are rows,
        // cells, or table sections. This also prevents a row from being
        // renamed to a div while it still contains cells.
        let table_parent = match common_table_wrapper {
            Some(CommonTableWrapper::Rows) => {
                let table = dom.create_html_element(Tag::Table).ok()?;
                let body = dom.create_html_element(Tag::Tbody).ok()?;
                dom.append_child(container, table);
                dom.append_child(table, body);
                Some(body)
            }
            Some(CommonTableWrapper::Cells) => {
                let table = dom.create_html_element(Tag::Table).ok()?;
                let body = dom.create_html_element(Tag::Tbody).ok()?;
                let row = dom.create_html_element(Tag::Tr).ok()?;
                dom.append_child(container, table);
                dom.append_child(table, body);
                dom.append_child(body, row);
                Some(row)
            }
            Some(CommonTableWrapper::Sections) => {
                let table = dom.create_html_element(Tag::Table).ok()?;
                dom.append_child(container, table);
                Some(table)
            }
            Some(CommonTableWrapper::Columns) => {
                let table = dom.create_html_element(Tag::Table).ok()?;
                let group = dom.create_html_element(Tag::Colgroup).ok()?;
                dom.append_child(container, table);
                dom.append_child(table, group);
                Some(group)
            }
            None => None,
        };

        for &node in siblings {
            if let Some(parent) = table_parent {
                dom.append_child(parent, node);
                continue;
            }
            if let Some(tag) = dom.tag(node) {
                let wrapper = match tag {
                    Tag::Tr => {
                        let table = dom.create_html_element(Tag::Table).ok()?;
                        let body = dom.create_html_element(Tag::Tbody).ok()?;
                        dom.append_child(container, table);
                        dom.append_child(table, body);
                        Some(body)
                    }
                    Tag::Td | Tag::Th => {
                        let table = dom.create_html_element(Tag::Table).ok()?;
                        let body = dom.create_html_element(Tag::Tbody).ok()?;
                        let row = dom.create_html_element(Tag::Tr).ok()?;
                        dom.append_child(container, table);
                        dom.append_child(table, body);
                        dom.append_child(body, row);
                        Some(row)
                    }
                    Tag::Caption | Tag::Colgroup | Tag::Tbody | Tag::Tfoot | Tag::Thead => {
                        let table = dom.create_html_element(Tag::Table).ok()?;
                        dom.append_child(container, table);
                        Some(table)
                    }
                    Tag::Col => {
                        let table = dom.create_html_element(Tag::Table).ok()?;
                        let group = dom.create_html_element(Tag::Colgroup).ok()?;
                        dom.append_child(container, table);
                        dom.append_child(table, group);
                        Some(group)
                    }
                    _ => None,
                };
                if let Some(wrapper) = wrapper {
                    dom.append_child(wrapper, node);
                    continue;
                }
                if !is_alter_to_div_exception(tag) && tag != Tag::Table {
                    dom.rename_html(node, Tag::Div)
                }
            }
            dom.append_child(container, node)
        }
        Some(container)
    }
    #[allow(clippy::too_many_arguments)]
    fn prep_article(
        &self,
        dom: &mut Dom,
        root: NodeId,
        credible_semantic_candidate: bool,
        compile_context: &crate::document::CompileContext,
        video: &Regex,
        text_buffer: &mut String,
        nodes: &mut Vec<NodeId>,
        node_data: &mut NodeStateStore,
        workspace: &mut FragmentWorkspace,
        cleanup_actions: &mut Vec<CleanupActionInfo>,
    ) -> (
        Option<SemanticStructureCounts>,
        crate::document::SourceEvidence,
    ) {
        // Cleanup mutates only the compact selected fragment. Hard cleanup
        // removes executable and interactive markup. Heuristic cleanup needs
        // several agreeing clutter signals before it removes a subtree.
        prepare_media_before_cleanup_in_workspace(dom, root, workspace);
        let before = self.diagnostic_element_count(dom, root);
        remove_decorative_media_before_cleanup_in_workspace(dom, root, workspace);
        self.record_cleanup_delta(
            dom,
            cleanup_actions,
            CleanupActionKind::DecorativeMedia,
            before,
            root,
        );
        let mut semantic_gate = crate::document::SemanticGate::default();
        clean_styles_with_semantic_gate_in_workspace(
            dom,
            root,
            nodes,
            &mut semantic_gate,
            workspace,
        );
        mark_data_tables_in_workspace(dom, root, node_data, nodes, workspace);
        for &node in nodes.iter() {
            if node_data.is_data_table(node) == Some(true) {
                semantic_gate.add_data_table_node(node);
            }
        }
        let source_evidence = crate::document::SourceEvidence::analyze_with_gate_and_snapshot(
            dom,
            root,
            node_data,
            semantic_gate,
            workspace.preorder(),
            workspace.elements_with_depth(),
        );
        let before = self.diagnostic_element_count(dom, root);
        hard_cleanup_in_workspace(
            dom,
            root,
            video,
            self.strategy == ExtractionStrategy::RelaxedVisibility,
            &source_evidence,
            nodes,
            workspace,
        );
        self.record_cleanup_delta(
            dom,
            cleanup_actions,
            CleanupActionKind::HardCleanup,
            before,
            root,
        );
        // Semantic coverage starts after high-confidence removal of active,
        // hidden, and decorative source structures. It then measures what the
        // relevance cleanup retains from this credible selected candidate.
        let candidate_semantic_metrics = self
            .diagnostic_attempts
            .as_ref()
            .filter(|_| credible_semantic_candidate)
            .and_then(|_| {
                crate::document::compile_document(
                    dom,
                    root,
                    compile_context,
                    &crate::document::CompileInputs {
                        source_evidence: Some(&source_evidence),
                        ..Default::default()
                    },
                )
                .ok()
                .map(|document| SemanticStructureCounts::measure(&document))
            });
        if self.page_kind.uses_article_cleanup() {
            if self.strategy.conditional_cleanup() {
                let before = self.diagnostic_element_count(dom, root);
                heuristic_cleanup_in_workspace(
                    dom,
                    root,
                    self.page_kind,
                    node_data,
                    &source_evidence,
                    text_buffer,
                    nodes,
                    workspace,
                );
                self.record_cleanup_delta(
                    dom,
                    cleanup_actions,
                    CleanupActionKind::HeuristicCleanup,
                    before,
                    root,
                );
            }
            // Global chrome is high-confidence cleanup. Apply it to every
            // extraction strategy, including broad and fallback attempts.
            let before = self.diagnostic_element_count(dom, root);
            remove_global_chrome_in_workspace(dom, root, node_data, &source_evidence, workspace);
            self.record_cleanup_delta(
                dom,
                cleanup_actions,
                CleanupActionKind::HeuristicCleanup,
                before,
                root,
            );
            let before = self.diagnostic_element_count(dom, root);
            remove_inline_chrome_controls_in_workspace(
                dom,
                root,
                node_data,
                &source_evidence,
                workspace,
            );
            self.record_cleanup_delta(
                dom,
                cleanup_actions,
                CleanupActionKind::HeuristicCleanup,
                before,
                root,
            );
            let before = self.diagnostic_element_count(dom, root);
            remove_repeated_and_discussion_content_in_workspace(
                dom,
                root,
                self.page_kind,
                node_data,
                &source_evidence,
                workspace,
            );
            self.record_cleanup_delta(
                dom,
                cleanup_actions,
                CleanupActionKind::HeuristicCleanup,
                before,
                root,
            );
        }

        // Remove duplicate media and named placeholders. Keep all output
        // semantics in source form for the document compiler.
        cleanup_selected_content_in_workspace(dom, root, nodes, self.base_uri.is_some(), workspace);

        // Single traversal collects both paragraphs and line breaks,
        // replacing two separate filters over `descendants`.
        let mut paragraphs = SmallVec::<[NodeId; 64]>::new();
        let mut breaks = SmallVec::<[NodeId; 32]>::new();
        for id in dom.descendants(root) {
            match dom.tag(id) {
                Some(Tag::P) => paragraphs.push(id),
                Some(Tag::Br) => breaks.push(id),
                _ => {}
            }
        }
        for paragraph in paragraphs {
            let media = dom.descendants(paragraph).any(|node| {
                matches!(
                    dom.tag(node),
                    Some(Tag::Img | Tag::Embed | Tag::Object | Tag::Iframe)
                ) || source_evidence.math(node)
                    || source_evidence.accessible_math(node)
            });
            if !media && !has_non_empty_inner_text(dom, paragraph) {
                dom.detach(paragraph);
            }
        }
        for line_break in breaks {
            if crate::cleaning::next_non_whitespace_sibling(dom, line_break)
                .is_some_and(|node| dom.tag(node) == Some(Tag::P))
            {
                dom.detach(line_break);
            }
        }
        (candidate_semantic_metrics, source_evidence)
    }
    fn content_excerpt(&self, dom: &Dom, root: NodeId) -> Option<String> {
        crate::instrumentation::record_content_excerpt_scan();
        let mut buffer = String::new();
        dom.descendants(root)
            .filter(|&node| dom.tag(node) == Some(Tag::P))
            .filter(|&node| {
                !dom.ancestors(node)
                    .take_while(|&ancestor| ancestor != root)
                    .any(|ancestor| {
                        matches!(dom.tag(ancestor), Some(Tag::Aside | Tag::Nav))
                            || [AttrName::Class, AttrName::Id]
                                .into_iter()
                                .filter_map(|name| dom.attr(ancestor, name))
                                .any(|value| {
                                    let value = value.to_ascii_lowercase();
                                    value.contains("hatnote")
                                        || value.contains("dablink")
                                        || value.contains("shortdescription")
                                })
                    })
            })
            .find_map(|node| {
                buffer.clear();
                dom.append_text(node, &mut buffer);
                let trimmed = buffer.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed.to_owned())
                }
            })
    }

    fn content_excerpt_if_needed(&self, dom: &Dom, root: NodeId) -> Option<String> {
        if self.metadata.description.is_some() {
            return None;
        }
        // Final cleanup only detaches empty blocks, which this scan already
        // ignores. The retained root therefore preserves the excerpt source
        // point while letting rejected attempts skip the subtree scan.
        self.content_excerpt(dom, root)
    }
    fn final_cleanup(
        &self,
        dom: &mut Dom,
        root: NodeId,
        evidence: &crate::document::SourceEvidence,
        nodes: &mut Vec<NodeId>,
        cleanup_actions: &mut Vec<CleanupActionInfo>,
    ) -> (
        Option<crate::document::SemanticSourceFacts>,
        Option<crate::document::RetainedStream>,
    ) {
        // The compiler resolves URLs, drops source attributes, ignores comments,
        // and collapses transparent wrappers. Only relevance cleanup mutates the
        // selected DOM at this stage.
        let before = self.diagnostic_element_count(dom, root);
        let mut source_facts = None;
        let retained_stream =
            remove_empty_content_with_source_facts(dom, root, nodes, &mut source_facts, evidence);
        self.record_cleanup_delta(
            dom,
            cleanup_actions,
            CleanupActionKind::FinalCleanup,
            before,
            root,
        );
        (source_facts, retained_stream)
    }

    fn diagnostic_element_count(&self, dom: &Dom, root: NodeId) -> Option<usize> {
        self.diagnostic_attempts.as_ref()?;
        Some(
            dom.descendants(root)
                .filter(|&node| dom.is_element(node))
                .count(),
        )
    }

    fn record_cleanup_delta(
        &self,
        dom: &Dom,
        actions: &mut Vec<CleanupActionInfo>,
        kind: CleanupActionKind,
        before: Option<usize>,
        root: NodeId,
    ) {
        let Some(before) = before else {
            return;
        };
        let removed_elements = before.saturating_sub(
            dom.descendants(root)
                .filter(|&node| dom.is_element(node))
                .count(),
        );
        if removed_elements > 0 {
            actions.push(CleanupActionInfo {
                kind,
                removed_elements,
            });
        }
    }

    fn capture_normalization_counts(
        &self,
        dom: &Dom,
        root: NodeId,
        workspace: &mut FragmentWorkspace,
        normalization: &mut NormalizationCountsInfo,
    ) {
        if self.diagnostic_attempts.is_none() {
            return;
        }
        workspace.ensure_snapshot(
            dom,
            root,
            crate::instrumentation::SnapshotKind::FinalNormalization,
        );
        let source_nodes = workspace.preorder();
        let (flattened_layout_tables, semantic_tables) =
            crate::document::table_normalization_counts_for_nodes(dom, root, source_nodes);
        let (footnote_references, footnote_definitions, math_expressions) =
            crate::document::semantic_normalization_counts_for_nodes(dom, root, source_nodes);
        *normalization = NormalizationCountsInfo {
            code_blocks: crate::document::source_code_block_count_for_nodes(
                dom,
                root,
                source_nodes,
            ),
            flattened_layout_tables,
            tables: semantic_tables,
            footnote_references,
            footnote_definitions,
            math_expressions,
            ..NormalizationCountsInfo::default()
        };
        for &node in source_nodes.iter().skip(1) {
            if dom.tag(node) == Some(Tag::Img) {
                normalization.images += 1;
            }
        }
    }
}
fn has_line_number_table_marker(dom: &Dom, root: NodeId) -> bool {
    std::iter::once(root)
        .chain(dom.descendants(root))
        .any(|node| {
            dom.tag(node) == Some(Tag::Table)
                && [AttrName::Class, AttrName::Id]
                    .into_iter()
                    .filter_map(|attribute| dom.attr(node, attribute))
                    .flat_map(str::split_whitespace)
                    .any(|token| {
                        matches!(
                            token.to_ascii_lowercase().as_str(),
                            "lntable" | "highlighttable" | "rouge-table" | "rouge-line-table"
                        )
                    })
        })
}

fn has_compact_code_page_structure(dom: &Dom, ancestor: NodeId) -> bool {
    let mut headings = std::iter::once(ancestor)
        .chain(dom.descendants(ancestor))
        .filter(|&node| {
            matches!(
                dom.tag(node),
                Some(Tag::H1 | Tag::H2 | Tag::H3 | Tag::H4 | Tag::H5 | Tag::H6)
            )
        });
    if headings.next().is_none() || headings.next().is_some() {
        return false;
    }
    if !dom.element_children(ancestor).next().is_some_and(|child| {
        matches!(
            dom.tag(child),
            Some(Tag::H1 | Tag::H2 | Tag::H3 | Tag::H4 | Tag::H5 | Tag::H6)
        )
    }) {
        return false;
    }
    if !dom.element_children(ancestor).all(|child| {
        matches!(
            dom.tag(child),
            Some(Tag::H1 | Tag::H2 | Tag::H3 | Tag::H4 | Tag::H5 | Tag::H6 | Tag::P | Tag::Pre)
        ) || !matches!(
            dom.tag(child),
            Some(Tag::Article | Tag::Aside | Tag::Main | Tag::Nav | Tag::Section)
        ) && dom
            .descendants(child)
            .any(|descendant| dom.tag(descendant) == Some(Tag::Pre))
    }) {
        return false;
    }

    let text_chars = dom.normalized_char_count(ancestor).max(1);
    let link_chars: usize = dom
        .descendants(ancestor)
        .filter(|&node| dom.tag(node) == Some(Tag::A))
        .map(|node| dom.normalized_char_count(node))
        .sum();
    link_chars.saturating_mul(5) <= text_chars
}

fn has_compact_code_lead(dom: &Dom, ancestor: NodeId, selected: NodeId) -> bool {
    let mut branch = selected;
    for parent in dom.ancestors(selected) {
        if parent == ancestor {
            break;
        }
        branch = parent;
    }

    let mut previous = dom.prev_sibling(branch);
    let mut substantive = SmallVec::<[NodeId; 2]>::new();
    while let Some(node) = previous {
        if dom.is_element(node) {
            substantive.push(node);
            if substantive.len() > 2 {
                return false;
            }
        } else if dom
            .text_node(node)
            .is_some_and(|text| !text.trim().is_empty())
        {
            return false;
        }
        previous = dom.prev_sibling(node);
    }
    substantive.reverse();
    matches!(
        substantive.as_slice(),
        [heading]
            if matches!(dom.tag(*heading), Some(Tag::H1 | Tag::H2 | Tag::H3 | Tag::H4))
    ) || matches!(
        substantive.as_slice(),
        [heading, lead]
            if matches!(dom.tag(*heading), Some(Tag::H1 | Tag::H2 | Tag::H3 | Tag::H4))
                && dom.tag(*lead) == Some(Tag::P)
                && dom.normalized_char_count(*lead) <= 240
    )
}

fn is_near_preceding_sibling_in_view(
    dom: &Dom,
    scoring_view: &ScoringView,
    candidate: NodeId,
    target: NodeId,
) -> bool {
    let mut sibling = dom.next_sibling(candidate);
    let mut intervening_elements = 0_u8;
    while let Some(node) = sibling {
        if scoring_view.projected_node(node) == target {
            return true;
        }
        if dom.is_element(node) {
            intervening_elements += 1;
            if intervening_elements > 1 {
                return false;
            }
        } else if dom
            .text_node(node)
            .is_some_and(|text| !text.trim().is_empty())
        {
            return false;
        }
        sibling = dom.next_sibling(node);
    }
    false
}

#[cfg(test)]
fn is_near_preceding_sibling(dom: &Dom, candidate: NodeId, target: NodeId) -> bool {
    let mut sibling = dom.next_sibling(candidate);
    let mut intervening_elements = 0_u8;
    while let Some(node) = sibling {
        if node == target {
            return true;
        }
        if dom.is_element(node) {
            intervening_elements += 1;
            if intervening_elements > 1 {
                return false;
            }
        } else if dom
            .text_node(node)
            .is_some_and(|text| !text.trim().is_empty())
        {
            return false;
        }
        sibling = dom.next_sibling(node);
    }
    false
}

fn title_heading_plan(
    dom: &Dom,
    elements: SourceElements<'_>,
    page_title: &str,
    structured_title: &str,
    site_name: Option<&str>,
    source_uri: Option<&Url>,
) -> TitleHeadingPlan {
    let root = dom.root();

    // A heading can be deeply nested in repaired HTML. Compute the score of
    // each node's nearest marked ancestor path once instead of walking that
    // path again for every matching heading. Keep the cheaper direct walk for
    // ordinary documents.
    let deeply_nested = elements.iter().any(|(_, depth)| depth > 64);
    let context_scores = deeply_nested.then(|| {
        let mut scores = vec![0_i32; dom.len()];
        let root_score = title_heading_context_score(dom, root, 0);
        for (node, _) in elements.iter() {
            let parent_score = dom.parent(node).map_or(0, |parent| {
                if parent == root {
                    root_score
                } else {
                    scores[parent.index()]
                }
            });
            scores[node.index()] = title_heading_context_score(dom, node, parent_score);
        }
        scores
    });

    let headings: Vec<_> = elements
        .iter()
        .map(|(node, _)| node)
        .filter(|&node| has_primary_heading_semantics(dom, node))
        .filter(|&node| is_probably_visible(dom, node))
        .collect();
    let has_link = elements
        .iter()
        .any(|(node, _)| dom.tag(node) == Some(Tag::A));
    let brand_headings: SmallVec<[NodeId; 2]> = if has_link {
        // Resolve linked descendants and ancestors in two linear passes for a
        // deep tree. A descendant scan per heading becomes quadratic after
        // HTML repair nests many unclosed headings. Keep direct scans for
        // ordinary shallow documents to avoid two dense temporary arrays.
        let linked_contexts = deeply_nested.then(|| {
            let mut descendant_links = vec![None; dom.len()];
            for (node, _) in elements.iter().rev() {
                let subtree_link = if dom.tag(node) == Some(Tag::A) {
                    Some(node)
                } else {
                    descendant_links[node.index()]
                };
                if let Some(link) = subtree_link
                    && let Some(parent) = dom.parent(node)
                {
                    descendant_links[parent.index()] = Some(link);
                }
            }
            let mut ancestor_links = vec![None; dom.len()];
            let mut in_document_chrome = vec![false; dom.len()];
            for (node, _) in elements.iter() {
                if let Some(parent) = dom.parent(node) {
                    ancestor_links[node.index()] = if dom.tag(parent) == Some(Tag::A) {
                        Some(parent)
                    } else {
                        ancestor_links[parent.index()]
                    };
                    in_document_chrome[node.index()] = in_document_chrome[parent.index()]
                        || matches!(
                            dom.tag(parent),
                            Some(Tag::Header | Tag::Footer | Tag::Nav | Tag::Aside)
                        );
                }
            }
            (descendant_links, ancestor_links, in_document_chrome)
        });
        headings
            .iter()
            .copied()
            .filter(|&heading| {
                let (link, in_document_chrome) = linked_contexts.as_ref().map_or_else(
                    || (linked_heading_context(dom, heading), None),
                    |(descendants, ancestors, chrome)| {
                        (
                            descendants[heading.index()].or(ancestors[heading.index()]),
                            Some(chrome[heading.index()]),
                        )
                    },
                );
                is_linked_site_brand_heading(
                    dom,
                    heading,
                    link,
                    in_document_chrome,
                    site_name,
                    source_uri,
                )
            })
            .collect()
    } else {
        SmallVec::new()
    };
    let text_limit = heading_text_limit(page_title, structured_title);
    let preferred = headings
        .iter()
        .copied()
        .filter(|heading| !brand_headings.contains(heading))
        .filter_map(|heading| {
            let text = get_inner_text_owned_limited(dom, heading, text_limit);
            let matches_page_title = heading_matches_page_title(page_title, &text);
            let matches_structured_title = heading_matches_page_title(structured_title, &text);
            (matches_page_title || matches_structured_title).then_some((
                heading,
                i32::from(matches_page_title) * 40
                    + i32::from(matches_structured_title) * 20
                    + i32::from(dom.tag(heading) == Some(Tag::H1)) * 8
                    + context_scores.as_ref().map_or_else(
                        || title_heading_score(dom, heading),
                        |scores| scores[heading.index()],
                    ),
            ))
        })
        .max_by_key(|(_, score)| *score)
        .map(|(heading, _)| heading);

    TitleHeadingPlan {
        preferred,
        brand_headings,
    }
}

fn title_heading_score(dom: &Dom, heading: NodeId) -> i32 {
    let mut score = 0;
    let mut current = Some(heading);
    while let Some(node) = current {
        score += match dom.tag(node) {
            Some(Tag::Article) => 32,
            Some(Tag::Main) => 24,
            _ => 0,
        };
        if has_primary_content_marker(dom, node) {
            score += 28;
            break;
        }
        current = dom.parent(node);
    }
    score
}

fn title_heading_context_score(dom: &Dom, node: NodeId, parent_score: i32) -> i32 {
    let own_score = match dom.tag(node) {
        Some(Tag::Article) => 32,
        Some(Tag::Main) => 24,
        _ => 0,
    };
    if has_primary_content_marker(dom, node) {
        own_score + 28
    } else {
        parent_score + own_score
    }
}

fn linked_heading_context(dom: &Dom, heading: NodeId) -> Option<NodeId> {
    dom.descendants(heading)
        .find(|&node| dom.tag(node) == Some(Tag::A))
        .or_else(|| {
            dom.ancestors(heading)
                .find(|&node| dom.tag(node) == Some(Tag::A))
        })
}

fn is_linked_site_brand_heading(
    dom: &Dom,
    heading: NodeId,
    link: Option<NodeId>,
    in_document_chrome: Option<bool>,
    site_name: Option<&str>,
    source_uri: Option<&Url>,
) -> bool {
    let Some(link) = link else { return false };
    let has_brand_token = [heading, link]
        .into_iter()
        .chain(dom.ancestors(heading).take(4))
        .filter_map(|node| {
            [AttrName::Class, AttrName::Id]
                .into_iter()
                .find_map(|attribute| dom.attr(node, attribute))
        })
        .flat_map(|value| value.split(|character: char| !character.is_ascii_alphanumeric()))
        .any(|token| {
            matches!(
                token.to_ascii_lowercase().as_str(),
                "brand" | "branding" | "logo" | "masthead" | "wordmark" | "sitetitle"
            )
        });
    if in_document_chrome.unwrap_or_else(|| {
        dom.ancestors(heading).any(|node| {
            matches!(
                dom.tag(node),
                Some(Tag::Header | Tag::Footer | Tag::Nav | Tag::Aside)
            )
        })
    }) {
        return false;
    }
    if has_brand_token {
        return true;
    }
    let Some(site_name) = site_name else {
        return false;
    };
    let heading_text =
        get_inner_text_owned_limited(dom, heading, heading_text_limit(site_name, ""));
    let matches_site_name = metadata::text_similarity(site_name, &heading_text) > 0.9
        && metadata::text_similarity(&heading_text, site_name) > 0.9;
    let root_link = source_uri.is_some_and(|source| {
        dom.attr(link, AttrName::Href)
            .is_some_and(|href| is_site_root_link(source, href))
    });
    matches_site_name && root_link
}

fn is_site_root_link(source: &Url, href: &str) -> bool {
    let Ok(target) = source.join(href) else {
        return false;
    };
    if source.scheme() != target.scheme()
        || source.host_str() != target.host_str()
        || source.port_or_known_default() != target.port_or_known_default()
        || target.query().is_some()
        || target.fragment().is_some()
    {
        return false;
    }
    let current_path = source.path().trim_end_matches('/');
    let target_path = target.path().trim_end_matches('/');
    target_path != current_path
        && (target_path.is_empty()
            || target_path == "/"
            || current_path
                .strip_prefix(target_path)
                .is_some_and(|remainder| remainder.starts_with('/')))
}

fn has_primary_content_marker(dom: &Dom, node: NodeId) -> bool {
    [
        dom.attr(node, AttrName::Class),
        dom.attr(node, AttrName::Id),
    ]
    .into_iter()
    .flatten()
    .flat_map(|value| value.split(|character: char| !character.is_ascii_alphanumeric()))
    .any(|token| {
        matches!(
            token.to_ascii_lowercase().as_str(),
            "articlecontent"
                | "articlebody"
                | "entrycontent"
                | "econtent"
                | "maincontent"
                | "postcontent"
                | "postbody"
        )
    })
}

fn heading_text_limit(page_title: &str, structured_title: &str) -> usize {
    page_title
        .chars()
        .count()
        .max(structured_title.chars().count())
        .saturating_add(128)
        .clamp(256, 4_096)
}

fn heading_matches_page_title(page_title: &str, heading: &str) -> bool {
    metadata::text_similarity(page_title, heading) > 0.75
        || page_title.strip_prefix(heading).is_some_and(|suffix| {
            let suffix = suffix.trim_start();
            !heading.is_empty()
                && suffix.chars().next().is_some_and(|c| {
                    matches!(c, '|' | '-' | '–' | '—' | '/' | '>' | '»' | '_' | ':')
                })
        })
}

/// Returns true when a cleaned semantic root contains enough independent
/// document structure to stand on its own when page-level coverage is low.
fn semantic_root_is_complete(metrics: ContentMetrics) -> bool {
    const MIN_TEXT_CHARS: usize = 500;
    const MIN_WORDS: usize = 30;

    let structured_content = (metrics.code_block_count > 0 && metrics.code_bytes >= 64)
        || (metrics.table_count > 0 && metrics.non_empty_table_cell_count >= 2);
    let coherent_document = metrics.paragraph_count >= 2
        || metrics.heading_count >= 2
        || structured_content
        || metrics.figure_count > 0;
    metrics.text_chars >= MIN_TEXT_CHARS
        && (metrics.word_count >= MIN_WORDS || structured_content && metrics.word_count >= 10)
        && coherent_document
        && metrics.link_density <= 0.45
        && metrics.link_text_chars < metrics.text_chars
}

fn is_boilerplate_root_node(dom: &Dom, node: NodeId) -> bool {
    const BOILERPLATE_NAMES: &[&str] = &[
        "accessdenied",
        "applicationshell",
        "captcha",
        "consent",
        "cookie",
        "forbidden",
        "login",
        "maintenance",
        "newsletter",
        "notfound",
        "paywall",
        "signin",
        "subscribe",
        "subscription",
        "verify",
        "verification",
    ];
    [AttrName::Id, AttrName::Class]
        .into_iter()
        .filter_map(|attribute| dom.attr(node, attribute))
        .map(|value| {
            value
                .chars()
                .filter(|character| character.is_ascii_alphanumeric())
                .flat_map(char::to_lowercase)
                .collect::<String>()
        })
        .any(|value| BOILERPLATE_NAMES.iter().any(|name| value == *name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "bench-instrumentation")]
    #[test]
    fn generic_scoring_does_not_clone_the_source_dom() {
        crate::instrumentation::reset();
        crate::extract(
            r#"<body><div class="content"><p>This generic document contains enough prose, punctuation, and detail to require normal readability scoring.</p><p>A second paragraph confirms the selected content and keeps the extraction result substantial.</p></div></body>"#,
            None,
        )
        .unwrap();
        assert_eq!(crate::instrumentation::snapshot().counters.dom_clones, 0);

        crate::instrumentation::reset();
        crate::extract(
            r#"<body><main class="d-none"><p>This hidden document contains enough useful prose for relaxed visibility scoring and extraction.</p><p>A second paragraph makes the hidden result coherent and substantial.</p></main></body>"#,
            None,
        )
        .unwrap();
        assert_eq!(crate::instrumentation::snapshot().counters.dom_clones, 0);
    }

    #[test]
    fn accepted_extraction_copies_the_selected_region_once() {
        let html = r#"<body><article><h1>Direct compilation</h1>
            <p>This complete article has enough useful text to select and accept its content.</p>
            <p>The semantic compiler consumes the cleaned fragment without a second deep copy.</p>
        </article></body>"#;
        Dom::reset_fragment_copy_count();

        let page = crate::extract(html, None).unwrap();

        assert!(page.markdown().contains("semantic compiler consumes"));
        assert_eq!(Dom::fragment_copy_count(), 1);
    }

    #[test]
    fn exact_and_specialized_roots_skip_generic_scoring() {
        reset_generic_scoring_calls();
        crate::Extractor::builder()
            .content_root(crate::ContentHint::Id("chosen".into()))
            .build()
            .extract(
                "<body><main id='chosen'><p>The requested root contains enough useful text for extraction.</p><p>A second paragraph keeps the requested content substantive.</p></main><aside>Unrelated page content.</aside></body>",
                None,
            )
            .unwrap();
        assert_eq!(generic_scoring_calls(), 0);

        reset_generic_scoring_calls();
        crate::Extractor::builder()
            .build()
            .extract(
                include_str!("../tests/specialized/hacker-news-listing/source.html"),
                Some("https://news.ycombinator.com/"),
            )
            .unwrap();
        assert_eq!(generic_scoring_calls(), 0);
    }

    #[test]
    fn retry_strategies_share_visibility_and_weighted_analysis() {
        reset_generic_scoring_calls();
        crate::Extractor::builder()
            .diagnostics(true)
            .build()
            .extract(
                include_str!("../benches/fixtures/strategies/relaxed-cleanup/source.html"),
                None,
            )
            .unwrap();
        assert_eq!(generic_scoring_calls(), 1);

        reset_generic_scoring_calls();
        crate::Extractor::builder()
            .diagnostics(true)
            .build()
            .extract(
                include_str!("../benches/fixtures/strategies/relaxed-visibility/source.html"),
                None,
            )
            .unwrap();
        assert_eq!(generic_scoring_calls(), 2);
    }

    #[test]
    fn physical_plans_include_cleanup_visibility_and_boundaries() {
        let base = PhysicalPlan {
            source_roots: SmallVec::from_slice(&[NodeId(1), NodeId(2)]),
            selection_node: NodeId(1),
            top_id: NodeId(1),
            synthetic: false,
            visibility: VisibilityVariant::Normal,
            conditional_cleanup: true,
            body_fallback: false,
            rename_top: false,
            lead_media: None,
        };

        let mut physical_attempts = Vec::new();
        let first = ContentExtractor::physical_attempt_id(&base, &mut physical_attempts);
        let equivalent = ContentExtractor::physical_attempt_id(&base, &mut physical_attempts);
        assert_eq!(first, equivalent);
        assert_eq!(physical_attempts.len(), 1);

        let mut cleanup = base.clone();
        cleanup.conditional_cleanup = false;
        assert_ne!(base, cleanup);

        let mut visibility = base.clone();
        visibility.visibility = VisibilityVariant::Relaxed;
        assert_ne!(base, visibility);

        let mut boundary = base.clone();
        boundary.synthetic = true;
        assert_ne!(base, boundary);
    }

    #[test]
    fn logical_strategy_order_keeps_fallbacks_declarative() {
        assert_eq!(
            ExtractionStrategy::ORDER,
            [
                ExtractionStrategy::Normal,
                ExtractionStrategy::RelaxedCleanup,
                ExtractionStrategy::BroadContent,
                ExtractionStrategy::StructuredDataHint,
                ExtractionStrategy::RelaxedVisibility,
                ExtractionStrategy::BodyFallback,
            ]
        );
    }

    #[test]
    fn retained_stream_keeps_the_non_synthetic_boundary_for_both_compilers() {
        let ordinary = crate::extract(
            r#"<body><main><article><h1>Boundary preservation</h1><p>This ordinary article contains enough complete prose to select the article as the content boundary and keep its semantic wrapper in canonical output.</p><p>A second paragraph confirms that the retained source stream preserves the same outer structure.</p></article></main></body>"#,
            None,
        )
        .unwrap();
        assert!(ordinary.html().starts_with("<div>"));

        let complex = crate::extract(
            r#"<body><main><article><h1>Complex boundary preservation</h1><p>This article contains enough complete prose to select the article and a mathematical expression <math><mi>x</mi></math> that requires complex semantic lowering.</p><p>A second paragraph keeps the extracted result substantial.</p></article></main></body>"#,
            None,
        )
        .unwrap();
        assert!(complex.html().starts_with("<div>"));
    }

    #[test]
    fn retained_stream_keeps_the_synthetic_body_boundary() {
        let ordinary = crate::extract(
            "<body><h1>Old page</h1>Useful text<br>Second useful line</body>",
            None,
        )
        .unwrap();
        assert!(ordinary.html().starts_with("<div>"));
        assert!(ordinary.text().contains("Second useful line"));

        let complex = crate::extract(
            r#"<body><h1>Formula page</h1><p>This body fallback has enough complete prose to remain meaningful and includes <math><mi>x</mi></math> for complex lowering.</p></body>"#,
            None,
        )
        .unwrap();
        assert!(complex.html().starts_with("<div>"));
        assert!(complex.text().contains("body fallback"));
    }

    #[test]
    fn subtitle_context_must_precede_and_stay_close_to_content() {
        let dom = Dom::parse_fragment(
            r#"<h3 id="near" class="subtitle">A useful article summary with enough sentence-like text.</h3><div></div><article id="content"><p>Article text.</p></article><h3 id="footer" class="footer-banner__subtitle">An unrelated promotional summary with enough text.</h3>"#,
            Tag::Div,
        )
        .unwrap();
        let content = dom
            .descendants(dom.root())
            .find(|&node| dom.attr(node, AttrName::Id) == Some("content"))
            .unwrap();
        let near = dom
            .descendants(dom.root())
            .find(|&node| dom.attr(node, AttrName::Id) == Some("near"))
            .unwrap();
        let footer = dom
            .descendants(dom.root())
            .find(|&node| dom.attr(node, AttrName::Id) == Some("footer"))
            .unwrap();
        assert!(is_near_preceding_sibling(&dom, near, content));
        assert!(!is_near_preceding_sibling(&dom, footer, content));
    }

    fn ranked_winner_id(html: &str) -> String {
        let dom = Dom::parse_document(html).unwrap();
        let config = ExtractorConfig::default();
        let mut readability = ContentExtractor::from_document(dom, None, &config);
        let mut text_buffer = String::new();
        let discovery = readability.discover_candidates(&mut text_buffer);
        let mut scoring_dom = readability.dom.clone();
        let mut to_score = discovery.to_score;
        let prepared = prepare_readability_structure(
            &mut scoring_dom,
            &discovery.divs_to_prepare,
            &discovery.candidates,
        );
        for node in prepared {
            if readability.node_data.mark_score_seen(node) {
                to_score.push(node);
            }
        }
        let excluded_mask = build_exclusion_mask(&scoring_dom, &discovery.remove_after_scoring);
        let scores = compute_readability_scores(
            &mut scoring_dom,
            None,
            to_score,
            &discovery.remove_after_scoring,
            &excluded_mask,
            &mut readability.node_data,
            readability.strategy.weight_classes(),
        );
        let mut candidates = discovery.candidates;
        let ranked =
            readability.rank_candidates(&scoring_dom, &mut candidates, scores, &excluded_mask);
        scoring_dom
            .attr(ranked[0].node, AttrName::Id)
            .expect("winner must have a test ID")
            .to_owned()
    }

    #[test]
    fn ranking_ties_keep_source_order_at_the_candidate_cutoff() {
        let dom = Dom::parse_document(
            r#"<body><p id="first">Same source sentence.</p><p id="second">Same source sentence.</p><p id="third">Same source sentence.</p></body>"#,
        )
        .unwrap();
        let body = dom.body().unwrap();
        let snapshot = dom.element_descendants_snapshot_with_depth(dom.root());
        let mut candidates = CandidateSet::discover_semantic(&dom);
        // Add candidates in a different order than their source positions.
        // Equal scores must still use the snapshot's source order at the
        // top-candidate cutoff.
        for id in ["third", "second", "first"] {
            let node = dom
                .descendants(dom.root())
                .find(|&node| dom.attr(node, AttrName::Id) == Some(id))
                .unwrap();
            candidates.add_readability(node, 10.0);
        }
        let mut store = NodeStateStore::new();
        let mut excluded = vec![false; dom.len()];
        excluded[body.index()] = true;

        let (ranked, _) = ContentExtractor::rank_candidates_with_snapshot(
            &dom,
            None,
            None,
            body,
            &snapshot,
            &mut candidates,
            SmallVec::new(),
            &excluded,
            &mut store,
            true,
            2,
            None,
        );
        let ranked_ids: Vec<_> = ranked
            .iter()
            .map(|candidate| dom.attr(candidate.node, AttrName::Id).unwrap())
            .collect();
        assert_eq!(ranked_ids, ["first", "second"]);
    }

    #[test]
    fn synthetic_container_preserves_table_ancestry() {
        let dom = Dom::parse_document(
            "<body><table><tr><td>First</td><td>Second</td></tr></table></body>",
        )
        .unwrap();
        let row = dom
            .descendants(dom.root())
            .find(|&node| dom.tag(node) == Some(Tag::Tr))
            .unwrap();
        let config = ExtractorConfig::default();
        let mut extractor = ContentExtractor::from_document(dom, None, &config);
        let container =
            ContentExtractor::create_container(&mut extractor.dom, row, &[row]).unwrap();

        for cell in extractor
            .dom
            .descendants(container)
            .filter(|&node| matches!(extractor.dom.tag(node), Some(Tag::Td | Tag::Th)))
        {
            assert_eq!(
                extractor
                    .dom
                    .ancestors(cell)
                    .find_map(|node| match extractor.dom.tag(node) {
                        Some(Tag::Tr | Tag::Table) => extractor.dom.tag(node),
                        _ => None,
                    }),
                Some(Tag::Tr)
            );
            assert!(
                extractor
                    .dom
                    .ancestors(cell)
                    .any(|node| extractor.dom.tag(node) == Some(Tag::Table))
            );
        }
    }

    #[test]
    fn synthetic_container_counts_only_required_table_wrappers() {
        assert_eq!(table_wrapper_plan(&[Tag::P; 64], 64).1, 0);
        assert_eq!(table_wrapper_plan(&[Tag::Tr; 64], 64).1, 2);
        assert_eq!(table_wrapper_plan(&[Tag::Td, Tag::Th], 2).1, 3);
        assert_eq!(
            table_wrapper_plan(&[Tag::Thead, Tag::Tbody, Tag::Tfoot], 3).1,
            1
        );
        assert_eq!(table_wrapper_plan(&[Tag::Col; 64], 64).1, 2);
        assert_eq!(table_wrapper_plan(&[Tag::Tr, Tag::P, Tag::Td], 3).1, 5);

        // A non-element sibling prevents one shared wrapper chain. Count the
        // wrappers for the element siblings that still need protection.
        assert_eq!(table_wrapper_plan(&[Tag::Tr], 2).1, 2);
    }

    #[test]
    fn ranks_article_and_non_article_content() {
        let fixtures = [
            (
                r#"<body><aside><p>Brief promotion with ordinary prose.</p></aside><article id="wanted"><p>A normal article has complete sentences, useful detail, commas, and enough text for paragraph scoring.</p></article></body>"#,
                "wanted",
            ),
            (
                r#"<body><div id="other" class="summary"><p>A misleading prose summary has many words, clauses, and punctuation.</p></div><main id="wanted"><h1>Build reference</h1><pre><code>cargo build --release
cargo test</code></pre><p>Run these commands.</p></main></body>"#,
                "wanted",
            ),
            (
                r#"<body><div id="other" class="summary"><p>A competing prose summary has complete sentences, commas, and article-like words.</p></div><main id="wanted"><h1>Status codes</h1><table><tr><th>Code</th><th>Meaning</th></tr><tr><td>200</td><td>Success</td></tr></table></main></body>"#,
                "wanted",
            ),
            (
                r#"<body><div id="other" class="post-content sidebar"><p>This sidebar has polished prose, but describes an unrelated promotion.</p></div><main id="wanted"><h1>API index</h1><ul><li><a href="/a">Alpha reference</a></li><li><a href="/b">Beta reference</a></li><li><a href="/c">Gamma reference</a></li><li><a href="/d">Delta reference</a></li></ul></main></body>"#,
                "wanted",
            ),
            (
                r#"<body><div id="other" class="post-content sidebar"><p>Misleading positive text in a sidebar with several ordinary words.</p></div><article id="wanted"><p>The actual article has useful prose, multiple clauses, commas, and stronger content evidence for ranking.</p><p>Its second paragraph adds relevant facts and a complete explanation.</p><p>Its third paragraph continues the primary discussion with useful detail.</p><p>Its final paragraph gives a clear conclusion for the reader.</p></article></body>"#,
                "wanted",
            ),
            (
                r#"<body><div id="other" class="post-content"><p>Short teaser.</p></div><main id="wanted" class="sidebar article-content"><p>This substantial guide remains useful although one class token is negative. It has complete sentences and enough detail.</p><p>A second paragraph confirms the content evidence.</p></main></body>"#,
                "wanted",
            ),
            (
                r#"<body><main id="wanted"><article><h2>First card</h2><p>First useful summary.</p></article><article><h2>Second card</h2><p>Second useful summary.</p></article><article><h2>Third card</h2><p>Third useful summary.</p></article></main></body>"#,
                "wanted",
            ),
        ];

        for (html, expected) in fixtures {
            assert_eq!(ranked_winner_id(html), expected, "{html}");
        }
    }

    #[test]
    fn tiny_semantic_placeholder_does_not_override_substantive_content() {
        let html = r#"<body><main id="placeholder">Loading...</main><section id="wanted"><p>The substantive section contains complete article prose, useful details, commas, and enough text for normal paragraph scoring.</p></section></body>"#;

        let text = crate::extract(html, None).unwrap().text();

        assert!(text.contains("substantive section"), "{text}");
    }

    #[test]
    fn conflicting_class_tokens_do_not_delete_substantial_content() {
        let html = r#"<body>
            <div class="sidebar article-content">
                <h1>Retained guide</h1>
                <p>This substantial article explains the subject with complete sentences and useful detail.</p>
                <pre><code>cargo test</code></pre>
                <table><thead><tr><th>Command</th><th>Purpose</th></tr></thead><tbody><tr><td>test</td><td>Validate changes</td></tr></tbody></table>
                <figure><img src="diagram.png" alt="Flow"><figcaption>Extraction flow</figcaption></figure>
            </div>
            <div class="article-content"><p>Short competing teaser.</p></div>
        </body>"#;

        let markdown = crate::extract(html, None).unwrap().markdown();

        assert!(markdown.contains("substantial article"), "{markdown}");
        assert!(markdown.contains("cargo test"), "{markdown}");
        assert!(markdown.contains("Command"), "{markdown}");
        assert!(markdown.contains("Extraction flow"), "{markdown}");
        assert!(!markdown.contains("competing teaser"), "{markdown}");
    }

    #[test]
    fn cleanup_protects_semantic_content_inside_negative_wrappers() {
        let html = r#"<body><main>
            <div class="sidebar">
                <h2 class="related">Unrelated recommendations</h2>
                <ul>
                    <li><a href="/one">Unrelated linked item one</a></li>
                    <li><a href="/two">Unrelated linked item two</a></li>
                    <li><a href="/three">Unrelated linked item three</a></li>
                </ul>
                <pre><code>cargo test --all-features</code></pre>
                <figure><img src="flow.png" alt="Flow"><figcaption>Validation flow</figcaption></figure>
            </div>
            <div><p>This sibling has enough prose to keep the main root selected while cleanup inspects the negative wrapper. It explains the primary topic in detail, provides careful context, describes the expected behavior, and gives readers a complete answer. The next part adds implementation constraints, practical examples, validation steps, and recovery guidance. Another part documents the inputs, outputs, edge cases, compatibility requirements, and important tradeoffs. The final part summarizes the recommended approach, explains why it works, and identifies the checks that confirm a correct result.</p></div>
        </main></body>"#;

        let markdown = crate::extract(html, None).unwrap().markdown();

        assert!(markdown.contains("cargo test --all-features"), "{markdown}");
        assert!(markdown.contains("Validation flow"), "{markdown}");
        assert!(!markdown.contains("Unrelated linked item"), "{markdown}");
    }

    #[test]
    fn cleanup_removes_related_sections_by_heading_and_links() {
        let html = r#"<body><main><article>
            <h1>Primary guide</h1>
            <p>This article explains the primary topic with complete sentences, useful context, practical details, and a clear conclusion. It gives readers enough information to answer the question without relying on the links below.</p>
            <div class="wp-block-group alignright">
                <h3>Related</h3>
                <div><a href="/one">A related story about the same topic</a></div>
                <div><a href="/two">Another related story with more detail</a></div>
            </div>
        </article></main></body>"#;

        let markdown = crate::extract(html, None).unwrap().markdown();

        assert!(markdown.contains("primary topic"), "{markdown}");
        assert!(!markdown.contains("A related story"), "{markdown}");
        assert!(!markdown.contains("Another related story"), "{markdown}");
    }

    #[test]
    fn cleanup_keeps_substantial_further_reading_content() {
        let substantial_section = "This section explains the related research in detail, including its methods, limitations, and practical implications. ".repeat(14);
        let html = format!(
            r#"<body><main><article>
                <h1>Primary guide</h1>
                <p>The primary guide contains complete context and a clear conclusion for the reader.</p>
                <section><h2>Further reading</h2><p>{substantial_section}</p><a href="/one">First reference</a><a href="/two">Second reference</a></section>
            </article></main></body>"#
        );

        let markdown = crate::extract(&html, None).unwrap().markdown();

        assert!(
            markdown.contains("related research in detail"),
            "{markdown}"
        );
    }

    #[test]
    fn unlikely_roles_are_evidence_instead_of_deletion_rules() {
        let html = r#"<body><div role="complementary" class="article-content">
            <p>This complete guide remains available despite its conflicting semantic role.</p>
            <p>Strong content evidence keeps the useful extraction candidate.</p>
        </div></body>"#;

        let markdown = crate::extract(html, None).unwrap().markdown();

        assert!(markdown.contains("complete guide"), "{markdown}");
        assert!(markdown.contains("Strong content evidence"), "{markdown}");
    }

    #[test]
    fn retries_restore_the_source_before_relaxed_cleanup() {
        let html = r#"<body><main dir="rtl">
            <p>A short stable introduction.</p>
            <div class="sidebar"><p><a href="/recovered">Useful linked reference recovered by the relaxed attempt.</a></p></div>
        </main></body>"#;

        let page = crate::extract(html, Some("https://example.com/docs/page")).unwrap();
        let output = page.html();

        // The normal conditional pass removes the link-heavy negative wrapper.
        // The final retry disables that pass and must start from the source.
        assert!(output.contains("Useful linked reference"), "{output}");
        assert!(
            output.contains("href=\"https://example.com/recovered\""),
            "{output}"
        );
        assert_eq!(page.metadata().direction.as_deref(), Some("rtl"));
    }

    #[test]
    fn email_addresses_are_not_treated_as_byline_timestamp_separators() {
        let html = r#"<body><main>
            <div class="byline">Contact editor@example.com <a href="/team">Editorial team</a></div>
            <p>This article has enough complete prose to produce useful extracted content for the test.</p>
        </main></body>"#;

        let page = crate::extract(html, None).unwrap();

        assert_eq!(
            page.metadata().authors,
            ["Contact editor@example.com Editorial team"]
        );
    }

    #[test]
    fn broad_and_body_fallbacks_retain_non_article_pages() {
        let listing = r#"<body><header>Site navigation</header><main>
            <h1>Package index</h1>
            <ul>
                <li><a href="/alpha">Alpha API reference</a></li>
                <li><a href="/beta">Beta API reference</a></li>
                <li><a href="/gamma">Gamma API reference</a></li>
                <li><a href="/delta">Delta API reference</a></li>
            </ul>
        </main><footer>Legal links</footer></body>"#;
        let markdown = crate::extract(listing, Some("https://example.com/index"))
            .unwrap()
            .markdown();
        assert!(markdown.contains("Alpha API reference"), "{markdown}");
        assert!(markdown.contains("Delta API reference"), "{markdown}");
        assert!(!markdown.contains("Site navigation"), "{markdown}");

        let old_page = crate::extract(
            "<body><h1>Old page</h1>Useful text<br>Second useful line</body>",
            None,
        )
        .unwrap()
        .text();
        assert!(old_page.contains("Useful text"), "{old_page}");
        assert!(old_page.contains("Second useful line"), "{old_page}");
    }

    #[test]
    fn empty_and_executable_only_pages_have_no_content() {
        assert!(matches!(
            crate::extract("<html><body></body></html>", None),
            Err(Error::NoContent)
        ));
        assert!(matches!(
            crate::extract("<body>... --- !!!</body>", None),
            Err(Error::NoContent)
        ));
        assert!(matches!(
            crate::extract("<html><head><title>Head only</title></head></html>", None),
            Err(Error::NoContent)
        ));
        assert!(matches!(
            crate::extract(
                "<body><img src='photo.jpg' alt='A useful photo'></body>",
                None
            ),
            Err(Error::NoContent)
        ));
        assert!(matches!(
            crate::extract(
                "<html><body><script>visible = false</script><style>body{}</style></body></html>",
                None,
            ),
            Err(Error::NoContent)
        ));

        let hidden = "hidden navigation words ".repeat(500);
        let html = format!(
            "<body><div hidden>{hidden}</div><nav>{hidden}</nav><main><p>Short visible answer.</p></main></body>"
        );
        let page = crate::extract(&html, None).unwrap();
        assert_eq!(page.text(), "Short visible answer.");
    }

    #[test]
    fn short_schema_teaser_does_not_truncate_a_long_page() {
        let prose = "The full guide explains configuration, validation, recovery, compatibility, and deployment with practical details. ".repeat(20);
        let html = format!(
            r#"<body><script type="application/ld+json">{{"@context":"https://schema.org","@type":"Article","articleBody":"Brief schema teaser appears here"}}</script>
            <div><p>Brief schema teaser appears here</p></div>
            <main><h1>Full guide</h1><p>{prose}</p></main></body>"#
        );
        let text = crate::extract(&html, None).unwrap().text();
        assert!(
            text.contains("The full guide explains configuration"),
            "{text}"
        );
        assert!(text.contains("deployment with practical details"), "{text}");
    }

    #[test]
    fn document_chrome_cannot_replace_short_main_content() {
        let html = r#"<body>
            <header id="content"><p>This polished header text has enough words, punctuation, and a strong identifier to compete as content.</p></header>
            <aside><p>Unrelated sidebar details and links.</p></aside>
            <div role="complementary"><p>Another unrelated panel.</p></div>
            <div role="main"><p>Short visible answer.</p></div>
        </body>"#;
        let text = crate::extract(html, None).unwrap().text();
        assert_eq!(text, "Short visible answer.");
    }

    #[test]
    fn strategies_choose_broad_main_and_final_body_boundaries() {
        let dom = Dom::parse_document(
            "<body><main><article><p>Useful result.</p></article></main><aside>Note</aside></body>",
        )
        .unwrap();
        let body = dom.body().unwrap();
        let article = dom.first_descendant_by_tag(body, Tag::Article).unwrap();
        let config = ExtractorConfig::default();
        let readability = ContentExtractor::from_document(dom.clone(), None, &config);
        let normal = RootSelection {
            node: article,
            reason: RootSelectionReason::Ranked,
            branches: SmallVec::new(),
        };

        let broad = readability.selection_for_strategy(
            ExtractionStrategy::BroadContent,
            &dom,
            body,
            normal.clone(),
            None,
        );
        assert_eq!(dom.tag(broad.node), Some(Tag::Main));
        let fallback = readability.selection_for_strategy(
            ExtractionStrategy::BodyFallback,
            &dom,
            body,
            normal,
            None,
        );
        assert_eq!(fallback.node, body);
        assert!(!ExtractionStrategy::BodyFallback.weight_classes());
        assert!(!ExtractionStrategy::BodyFallback.conditional_cleanup());
    }

    #[test]
    fn diagnostics_are_opt_in_and_report_the_selected_attempt() {
        let html = r#"<body><main id="guide" class="docs content"><p>This useful guide has a complete sentence.</p></main></body>"#;
        let default_page = crate::extract(html, None).unwrap();
        assert!(default_page.diagnostics().is_none());

        let page = crate::Extractor::builder()
            .diagnostics(true)
            .build()
            .extract(html, None)
            .unwrap();
        let diagnostics = page.diagnostics().unwrap();
        assert_eq!(
            diagnostics.selected_strategy,
            ExtractionStrategyInfo::Normal
        );
        assert_eq!(diagnostics.attempts.len(), 1);
        assert!(diagnostics.attempts[0].accepted);
        assert_eq!(
            diagnostics.attempts[0].selected_root.tag.as_deref(),
            Some("main")
        );
        assert_eq!(
            diagnostics.attempts[0].selected_root.id.as_deref(),
            Some("guide")
        );
        assert_eq!(
            diagnostics.attempts[0].selected_root.classes,
            ["docs", "content"]
        );
        assert!(diagnostics.attempts[0].semantic_coverage.is_none());
    }

    #[test]
    fn trusted_semantic_root_can_accept_complete_low_coverage_content() {
        let shell = "Dashboard toolbar workspace settings notifications ".repeat(240);
        let html = format!(
            "<body><main><aside class='shell'>{shell}</aside><article><h1>Reliable batch processing</h1><p>Batch processing is reliable when each job records its input, output, and retry state for later diagnosis.</p><p>This guide describes the worker boundary and the retry policy needed to diagnose a failed run across several independent stages.</p><p>Each retry records the failure category, preserves the original input, and reports the final outcome so operators can recover a failed nightly report without guessing.</p><p>Operators can compare duration, retry count, output rows, and the age of the oldest pending job to find the correct recovery action quickly.</p></article></main></body>"
        );
        let page = crate::Extractor::builder()
            .diagnostics(true)
            .build()
            .extract(&html, None)
            .unwrap();
        let diagnostics = page.diagnostics().unwrap();
        let attempt = diagnostics
            .attempts
            .iter()
            .find(|attempt| attempt.accepted)
            .unwrap();

        assert_eq!(attempt.selected_root.tag.as_deref(), Some("article"));
        assert_eq!(
            attempt.acceptance_exception,
            Some(AcceptanceExceptionInfo::TrustedSemanticRoot)
        );
        assert!(!attempt.quality.good);
        assert!(page.text().contains("retry policy"));
        assert!(!page.text().contains("Dashboard toolbar"));
    }

    #[test]
    fn trusted_semantic_root_does_not_bypass_safety_gates() {
        let shell = "Dashboard toolbar workspace settings notifications ".repeat(240);
        for html in [
            "<body><main><h1>Access denied</h1><p>You do not have permission to access this resource.</p></main></body>",
            "<body><div id='root'></div><script src='/assets/application.js'></script></body>",
        ] {
            assert!(
                crate::Extractor::builder()
                    .build()
                    .extract(html, None)
                    .is_err(),
                "{html}"
            );
        }
        let paywall = format!(
            "<body><main><aside>{shell}</aside><article class='paywall'><h1>Subscription required</h1><p>Subscribe to unlock this article. The complete report is available after the account subscription is confirmed by the service.</p><p>Members can review the full investigation, supporting evidence, and recovery details after the subscription is confirmed for the account.</p><p>The report explains the measured result, the background conditions, and the evidence collected during the investigation for authorized readers.</p><p>Readers can compare the documented findings and follow the recovery procedure after access has been granted by the account service.</p></article></main></body>"
        );
        let application = format!(
            "<body><main><aside>{shell}</aside><article class='application-shell'><h1>Dashboard application</h1><p>Enable JavaScript to continue. This application shell is not a document and provides no static report content for the reader.</p><p>Use the controls below to open the workspace, continue the setup process, and configure notifications for the account.</p><p>The client application loads the remaining screen after the browser initializes the required interactive modules.</p><p>These controls are placeholders for a client-rendered workspace and do not contain a complete article or reference.</p><button>Open dashboard</button><button>Continue</button><button>Configure notifications</button></article></main></body>"
        );
        for html in [paywall, application] {
            let result = crate::Extractor::builder()
                .diagnostics(true)
                .build()
                .extract(&html, None);
            if let Ok(page) = result {
                assert!(
                    page.diagnostics().is_some_and(|diagnostics| diagnostics
                        .attempts
                        .iter()
                        .all(|attempt| attempt.acceptance_exception.is_none())),
                    "{html}"
                );
            }
        }
    }

    #[test]
    fn itemprop_article_body_is_promoted_even_when_nested_content_ranks_higher() {
        let prose = "The article explains the patent history, the technical scope, and the practical effect for Linux users with verified detail. ".repeat(8);
        let shell = "Navigation account recommendations newsletter settings ".repeat(80);
        let html = format!(
            "<body><header>{shell}</header><main><article><h1>Patent history</h1><section id='article-body' itemprop='articleBody'><p>{prose}</p><h2>Background</h2><p>{prose}</p></section></article></main></body>"
        );
        let page = crate::Extractor::builder()
            .diagnostics(true)
            .build()
            .extract(&html, None)
            .unwrap();
        let diagnostics = page.diagnostics().unwrap();
        let attempt = diagnostics
            .attempts
            .iter()
            .find(|attempt| attempt.accepted)
            .unwrap();

        assert_eq!(attempt.selected_root.id.as_deref(), Some("article-body"));
        assert_eq!(
            attempt.selected_root.selection_reason,
            crate::RootSelectionReasonInfo::ArticleBody
        );
        assert!(page.text().contains("patent history"));
        assert!(!page.text().contains("Navigation account recommendations"));
    }

    #[test]
    fn semantic_root_completeness_keeps_code_and_table_documents() {
        let shell = "Dashboard navigation recommendations account settings ".repeat(140);
        let code = "df = df.filter(pl.col(\"value\") > 10).select([\"name\", \"value\"])\n";
        let html = format!(
            "<body><aside>{shell}</aside><main><article><h1>DataFrame reference</h1><p>This reference explains the complete workflow for loading, transforming, and validating a DataFrame.</p><h2>Filtering</h2><p>Use the following expression to keep rows that match the required value threshold.</p><pre><code>{}{}</code></pre><h2>Options</h2><table><thead><tr><th>Name</th><th>Description</th></tr></thead><tbody><tr><td>value</td><td>Numeric filter value</td></tr><tr><td>name</td><td>Output column name</td></tr></tbody></table></article></main></body>",
            code,
            code.repeat(3)
        );
        let page = crate::Extractor::builder()
            .diagnostics(true)
            .build()
            .extract(&html, None)
            .unwrap();
        let diagnostics = page.diagnostics().unwrap();
        let attempt = diagnostics
            .attempts
            .iter()
            .find(|attempt| attempt.accepted)
            .unwrap();

        assert_eq!(attempt.selected_root.tag.as_deref(), Some("article"));
        assert!(attempt.result.code_block_count > 0);
        assert!(attempt.result.table_count > 0);
        assert!(page.text().contains("complete workflow"));
        assert!(page.text().contains("Numeric filter value"));
        assert!(!page.text().contains("Dashboard navigation recommendations"));
    }

    #[test]
    fn semantic_root_completeness_rejects_link_only_content() {
        let complete_text = "Complete document content has enough words and meaningful detail for the measured result and the reason the procedure matters. ".repeat(8);
        let complete_dom = Dom::parse_fragment(
            &format!("<div><p>{complete_text}</p><p>{complete_text}</p></div>"),
            Tag::Div,
        )
        .unwrap();
        let complete = ContentMetrics::measure(&complete_dom, complete_dom.root());
        assert!(semantic_root_is_complete(complete));

        let linked_text = "linked words ".repeat(60);
        let links_html = format!(
            "<div><p><a href='/one'>{linked_text}</a></p><p><a href='/two'>{linked_text}</a></p></div>"
        );
        let links_dom = Dom::parse_fragment(&links_html, Tag::Div).unwrap();
        let links = ContentMetrics::measure(&links_dom, links_dom.root());
        assert!(!semantic_root_is_complete(links));
    }

    #[test]
    fn diagnostics_report_full_semantic_coverage_for_a_technical_document() {
        let html = r#"<body><main><h2>Install</h2><p>This guide explains the complete installation procedure.</p><pre><code>cargo install sample</code></pre><h2>Options</h2><ul><li>First option</li><li>Second option</li><li>Third option</li></ul><h2>Results</h2><table><tr><th>Name</th><th>Value</th></tr><tr><td>status</td><td>ready</td></tr></table></main></body>"#;
        let page = crate::Extractor::builder()
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
        let coverage = attempt.semantic_coverage.as_ref().unwrap();

        assert_eq!(coverage.score, 1.0);
        assert_eq!(coverage.categories.len(), 4);
        assert_eq!(
            coverage
                .categories
                .iter()
                .map(|category| category.category)
                .collect::<Vec<_>>(),
            [
                crate::diagnostics::SemanticCoverageCategory::CodeBlocks,
                crate::diagnostics::SemanticCoverageCategory::DataTables,
                crate::diagnostics::SemanticCoverageCategory::SubstantialListItems,
                crate::diagnostics::SemanticCoverageCategory::Headings,
            ]
        );
    }

    #[test]
    fn diagnostics_report_retries_and_the_actual_winner() {
        let linked_detail = "Useful linked reference with descriptive context, practical guidance, examples, and details. ";
        let html = format!(
            r#"<body><main><p>A short stable introduction.</p><aside class="related"><h2>Related reference</h2><a href="/one">{linked_detail}</a><a href="/two">{linked_detail}</a></aside></main></body>"#
        );
        let page = crate::Extractor::builder()
            .diagnostics(true)
            .build()
            .extract(&html, None)
            .unwrap();
        let diagnostics = page.diagnostics().unwrap();
        assert!(diagnostics.attempts.len() >= 2);
        let winner = diagnostics
            .attempts
            .iter()
            .find(|attempt| attempt.accepted)
            .unwrap();
        assert_eq!(diagnostics.selected_strategy, winner.strategy);
        assert!(
            diagnostics
                .attempts
                .iter()
                .filter(|attempt| attempt.accepted)
                .count()
                == 1
        );
        assert!(diagnostics.attempts.iter().all(|attempt| {
            attempt.normalization.code_blocks == 0
                && attempt.normalization.footnote_references == 0
                && attempt.normalization.footnote_definitions == 0
                && attempt.normalization.math_expressions == 0
                && attempt.normalization.images == 0
                && attempt.normalization.tables == 0
                && attempt.normalization.flattened_layout_tables == 0
        }));
    }

    #[cfg(feature = "bench-instrumentation")]
    #[test]
    fn deferred_attempt_work_is_visible_in_counters() {
        let hidden_detail = "The recovered section explains configuration, validation, compatibility, and deployment with practical detail. ".repeat(4);
        let html = format!(
            r#"<body><main><p>Visible summary.</p><article hidden><h2>Complete guide</h2><p>{hidden_detail}</p></article></main></body>"#
        );
        crate::instrumentation::reset();
        let page = crate::Extractor::builder()
            .diagnostics(true)
            .build()
            .extract(&html, None)
            .unwrap();
        let deferred = crate::instrumentation::deferred_work_snapshot();

        let attempts = page.diagnostics().unwrap().attempts.len();
        assert!(attempts >= 2);
        assert!(deferred.content_excerpt_scans < attempts as u64);
        assert!(deferred.final_dom_node_scans > 0);
        assert_eq!(
            crate::instrumentation::snapshot()
                .counters
                .prepared_source_builds,
            1
        );
    }

    #[cfg(feature = "bench-instrumentation")]
    #[test]
    fn metadata_description_skips_excerpt_and_normal_extraction_skips_final_count() {
        crate::instrumentation::reset();
        let page = crate::extract(
            r#"<html><head><meta name="description" content="The source description."></head><body><main><p>The article has enough useful text to be selected and rendered.</p><p>A second paragraph keeps the result substantive.</p></main></body></html>"#,
            None,
        )
        .unwrap();
        let deferred = crate::instrumentation::deferred_work_snapshot();

        assert_eq!(
            page.metadata().description.as_deref(),
            Some("The source description.")
        );
        assert_eq!(deferred.content_excerpt_scans, 0);
        assert_eq!(deferred.final_dom_node_scans, 0);

        crate::instrumentation::reset();
        crate::Extractor::builder()
            .diagnostics(true)
            .build()
            .extract(
                r#"<body><main><p>The diagnostic result has enough useful text to be selected and measured.</p><p>A second paragraph keeps the result substantive.</p></main></body>"#,
                None,
            )
            .unwrap();
        assert!(crate::instrumentation::deferred_work_snapshot().final_dom_node_scans > 0);
    }

    #[cfg(feature = "bench-instrumentation")]
    #[test]
    fn exact_root_skips_external_footnote_collection_and_content_hints_scan_once() {
        crate::instrumentation::reset();
        crate::Extractor::builder()
            .content_root(crate::ContentHint::Id("chosen".into()))
            .build()
            .extract(
                r##"<body><main id="chosen"><p>The requested root contains enough useful text for extraction.</p><p>A second paragraph keeps the root substantive.<sup><a href="#external-note">1</a></sup></p></main><aside id="external-note" role="doc-footnote">This definition is outside the requested root.</aside></body>"##,
                None,
            )
            .unwrap();
        let deferred = crate::instrumentation::deferred_work_snapshot();
        assert_eq!(deferred.external_footnote_scans, 0);

        crate::instrumentation::reset();
        let specialized_page = crate::Extractor::builder()
            .diagnostics(true)
            .build()
            .extract(
                include_str!("../tests/specialized/hacker-news-listing/source.html"),
                Some("https://news.ycombinator.com/"),
            )
            .unwrap();
        assert!(
            specialized_page
                .diagnostics()
                .and_then(|diagnostics| diagnostics.specialized_extractor.as_deref())
                .is_some()
        );
        assert_eq!(
            crate::instrumentation::deferred_work_snapshot().external_footnote_scans,
            0
        );

        crate::instrumentation::reset();
        crate::Extractor::builder()
            .content_hint(crate::ContentHint::Id("preferred".into()))
            .build()
            .extract(
                r#"<body><main><div id="preferred"><p>The caller selected this useful content with enough detail for extraction.</p><p>A second paragraph provides more useful context.</p></div></main></body>"#,
                None,
            )
            .unwrap();
        assert_eq!(
            crate::instrumentation::deferred_work_snapshot().content_hint_scans,
            1
        );
    }

    #[test]
    fn cached_content_hints_skip_detached_scoring_nodes() {
        let page = crate::Extractor::builder()
            .content_hint(crate::ContentHint::Tag(crate::ContentTag::Div))
            .diagnostics(true)
            .build()
            .extract(
                r#"<body><main><div id="preferred"><p>Short hinted text.</p></div><p>The surrounding document contains enough substantive content to remain a valid extraction result with practical detail and useful context.</p><p>A second paragraph keeps the selected region coherent.</p></main></body>"#,
                None,
            )
            .unwrap();

        assert!(page.text().contains("surrounding document"));
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
    fn streamed_dashboard_prefers_the_complete_document_root() {
        let html = r#"
            <html>
              <head>
                <title>Server-rendered model dashboard | Example</title>
                <meta name="description" content="A model dashboard with provider pricing, performance, uptime, benchmarks, and an API description for the selected model." />
                <script type="application/ld+json">{"@context":"https://schema.org","@type":"SoftwareApplication","name":"Example Model","description":"A model dashboard with provider pricing and performance details."}</script>
              </head>
              <body>
                <nav><a href="/models">Models</a><a href="/docs">Docs</a></nav>
                <div hidden id="stream-title">
                  <h1>Example Model</h1>
                  <p>The selected model supports long-context reasoning, coding, and agent workflows.</p>
                  <p>It accepts text and images and returns reliable text output for production systems.</p>
                </div>
                <div hidden id="stream-sections">
                  <section id="providers"><h2>Providers</h2><p>Several providers host this model and provide automatic failover when an endpoint is unavailable.</p><table><tr><th>Provider</th><th>Latency</th><th>Uptime</th></tr><tr><td>Primary</td><td>3.08s</td><td>99.40%</td></tr></table></section>
                  <section id="pricing"><h2>Pricing</h2><p>Customers pay a lower effective price when caching and discounts apply to their requests.</p><p>Input tokens cost $2.50 per million and output tokens cost $15 per million.</p></section>
                  <section id="performance"><h2>Performance</h2><p>Throughput measures how fast the model writes tokens, while latency measures the full round trip.</p><p>Independent evaluations measure reasoning and tool use across several tasks.</p></section>
                  <section id="uptime"><h2>Uptime</h2><p>The service monitors provider responses and routes requests to a healthy endpoint when needed.</p><div class="chart"><p>Availability 99.90%.</p><p>Availability 99.97%.</p><p>Availability 99.95%.</p><p>Availability 99.80%.</p><p>Availability 99.89%.</p><p>Availability 99.77%.</p><p>Availability 99.88%.</p><p>Availability 99.93%.</p><p>Availability 99.65%.</p><p>Availability 99.54%.</p><p>Availability 99.75%.</p><p>Availability 99.30%.</p></div></section>
                </div>
              </body>
            </html>
        "#;
        let page = crate::Extractor::builder()
            .diagnostics(true)
            .structured_data(true)
            .build()
            .extract(html, Some("https://example.test/models/example"))
            .unwrap();
        let diagnostics = page.diagnostics().unwrap();
        let accepted = diagnostics
            .attempts
            .iter()
            .find(|attempt| attempt.accepted)
            .unwrap();
        assert_eq!(
            diagnostics.selected_strategy,
            ExtractionStrategyInfo::RelaxedVisibility
        );
        assert_eq!(accepted.selected_root.tag.as_deref(), Some("body"));
        assert!(page.text().contains("Several providers host this model"));
        assert!(page.text().contains("Input tokens cost $2.50"));
        assert!(page.text().contains("Independent evaluations measure"));
        assert!(
            page.text()
                .contains("The service monitors provider responses")
        );
        assert!(
            !page
                .markdown()
                .contains("Server-rendered model dashboard | Example")
        );
    }

    #[test]
    fn complete_semantic_root_competes_with_data_heavy_siblings() {
        let html = r#"
            <html>
              <head>
                <title>Model reference</title>
                <meta name="description" content="A complete reference with provider, pricing, performance, and usage information for this model." />
                <script type="application/ld+json">{"@type":"SoftwareApplication","name":"Reference model","description":"Provider and pricing reference."}</script>
              </head>
              <body>
                <main hidden id="complete-document">
                  <h1>Reference model</h1>
                  <section><h2>Providers</h2><p>Several providers serve this model with failover and regional capacity.</p><p>Provider status and latency are monitored throughout the day.</p></section>
                  <section><h2>Pricing</h2><p>Input and output token prices depend on the selected provider and cache policy.</p><p>Customers can compare current rates before sending production requests.</p></section>
                  <section><h2>Performance</h2><p>Performance tests measure throughput, latency, reasoning, and tool use.</p><p>The reference records results across representative workloads.</p></section>
                  <section><h2>Usage</h2><p>The API accepts text and images and returns structured text for applications.</p><p>Request limits and response formats are documented for each provider.</p></section>
                </main>
                <div hidden class="metric-chart">
                  <p>Availability 99.90%</p><p>Availability 99.91%</p><p>Availability 99.92%</p><p>Availability 99.93%</p><p>Availability 99.94%</p><p>Availability 99.95%</p><p>Availability 99.96%</p><p>Availability 99.97%</p><p>Availability 99.98%</p><p>Availability 99.99%</p><p>Availability 99.89%</p><p>Availability 99.88%</p><p>Availability 99.87%</p><p>Availability 99.86%</p><p>Availability 99.85%</p><p>Availability 99.84%</p><p>Availability 99.83%</p><p>Availability 99.82%</p><p>Availability 99.81%</p><p>Availability 99.80%</p>
                </div>
              </body>
            </html>
        "#;
        let page = crate::Extractor::builder()
            .diagnostics(true)
            .structured_data(true)
            .build()
            .extract(html, Some("https://example.test/reference/model"))
            .unwrap();
        let diagnostics = page.diagnostics().unwrap();
        let accepted = diagnostics
            .attempts
            .iter()
            .find(|attempt| attempt.accepted)
            .unwrap();
        assert_eq!(
            accepted.selected_root.id.as_deref(),
            Some("complete-document")
        );
        assert!(page.text().contains("Several providers serve this model"));
        assert!(page.text().contains("The API accepts text and images"));
    }

    #[test]
    fn diagnostics_report_visibility_recovery_attempts() {
        let hidden_detail = "The recovered section explains configuration, validation, compatibility, and deployment with practical detail. ".repeat(4);
        let html = format!(
            r#"<body><main><p>Visible summary.</p><article hidden><h2>Complete guide</h2><p>{hidden_detail}</p></article></main></body>"#
        );
        let page = crate::Extractor::builder()
            .diagnostics(true)
            .build()
            .extract(&html, None)
            .unwrap();
        let diagnostics = page.diagnostics().unwrap();
        assert!(diagnostics.attempts.len() >= 2);
        assert_eq!(
            diagnostics.selected_strategy,
            ExtractionStrategyInfo::RelaxedVisibility
        );
        assert!(diagnostics.attempts.iter().any(|attempt| {
            attempt.rejection_reason == Some(AttemptRejectionReason::PotentialHiddenContent)
        }));
        assert!(page.text().contains("recovered section"));
    }

    #[test]
    fn relaxed_visibility_recovers_a_large_streamed_fragment() {
        let hidden_detail = "The streamed article explains configuration, validation, compatibility, deployment, and recovery with practical detail. ".repeat(30);
        let visible_detail = "The page includes a visible summary with enough metadata and introductory context for normal extraction, but the complete article is still being streamed into the hidden fragment. ".repeat(3);
        let html = format!(
            r##"<body><main><header><h1>Streamed article</h1><p>{visible_detail}</p></header><div hidden id="S:0"><p>{hidden_detail} Equation <span data-legible-math="inline" data-latex="x^2">x 2</span><sup><a role="doc-noteref" href="#fn1">1</a></sup></p><aside id="fn1" role="doc-footnote">The equation note remains useful.</aside><h2>Implementation</h2><p>The second streamed paragraph gives the final implementation details and conclusion.</p></div></main></body>"##
        );
        let page = crate::Extractor::builder()
            .diagnostics(true)
            .build()
            .extract(&html, None)
            .unwrap();

        assert!(page.text().contains("streamed article explains"));
        assert!(page.text().contains("Implementation"));
        let markdown = page.markdown();
        assert!(markdown.contains("$x^2$"), "{markdown}");
        assert!(markdown.contains("[^fn1]"), "{markdown}");
        assert_eq!(
            page.diagnostics().unwrap().selected_strategy,
            ExtractionStrategyInfo::RelaxedVisibility
        );
    }

    #[test]
    fn relaxed_visibility_recovers_hidden_content_without_selecting_hidden_junk() {
        let hidden_article = r#"<body><article style="display: none"><h1>Recovered guide</h1><p>The hidden server-rendered guide contains coherent content, practical details, and a complete explanation.</p><p>A second paragraph confirms that this is the primary document content.</p></article></body>"#;
        let page = crate::Extractor::builder()
            .diagnostics(true)
            .build()
            .extract(hidden_article, None)
            .unwrap();
        assert!(page.text().contains("hidden server-rendered guide"));
        let recovered_html = page.html();
        assert!(!recovered_html.contains(" hidden="), "{recovered_html}");
        assert!(
            !recovered_html.contains("class=\"hidden"),
            "{recovered_html}"
        );
        assert!(
            !recovered_html.contains("display: none"),
            "{recovered_html}"
        );
        assert_eq!(
            page.diagnostics().unwrap().selected_strategy,
            ExtractionStrategyInfo::RelaxedVisibility
        );

        let visible_article = r#"<body><div hidden><nav><a href="/1">Hidden navigation one</a><a href="/2">Hidden navigation two</a><a href="/3">Hidden navigation three</a></nav></div><main><p>The visible article is the correct primary content.</p></main></body>"#;
        let page = crate::Extractor::builder()
            .diagnostics(true)
            .build()
            .extract(visible_article, None)
            .unwrap();
        assert_eq!(
            page.text(),
            "The visible article is the correct primary content."
        );
        assert_eq!(
            page.diagnostics().unwrap().selected_strategy,
            ExtractionStrategyInfo::Normal
        );

        let barrier_and_article = r#"<body><main class="paywall"><h1>Subscribe to unlock this article</h1><p>Choose a plan and sign in to continue. $9 per month. $90 annual.</p></main><article hidden><h1>Recovered report</h1><p>The complete hidden report provides verified facts, careful analysis, and useful context for every reader.</p><p>Its second paragraph gives implementation detail and a clear conclusion.</p></article></body>"#;
        let page = crate::Extractor::builder()
            .diagnostics(true)
            .build()
            .extract(barrier_and_article, None)
            .unwrap();
        assert!(page.text().contains("complete hidden report"));
        assert_eq!(
            page.diagnostics().unwrap().selected_strategy,
            ExtractionStrategyInfo::RelaxedVisibility
        );

        let visible_dialog = r#"<body><main class="dialog"><p>This visible dialog transcript is legitimate document content and must remain available.</p><p>A second paragraph provides useful context for the reader.</p></main></body>"#;
        let page = crate::extract(visible_dialog, None).unwrap();
        assert!(page.text().contains("legitimate document content"));

        let hidden_modal = r#"<body><div class="modal" style="display:none"><h1>Long promotion</h1><p>This modal contains a long promotional message with many polished words and repeated details that must never replace the document.</p></div><main><p>Short visible answer.</p></main></body>"#;
        let page = crate::extract(hidden_modal, None).unwrap();
        assert_eq!(page.text(), "Short visible answer.");
    }

    #[test]
    fn relaxed_visibility_handles_hidden_indexes_and_duplicate_variants() {
        let index = r#"<body><main class="d-none"><h1>Reference index</h1><ul><li><a href="/a">Alpha reference guide</a></li><li><a href="/b">Beta reference guide</a></li><li><a href="/c">Gamma reference guide</a></li><li><a href="/d">Delta reference guide</a></li></ul></main></body>"#;
        let page = crate::Extractor::builder()
            .diagnostics(true)
            .build()
            .extract(index, None)
            .unwrap();
        assert!(page.text().contains("Alpha reference guide"));
        assert_eq!(
            page.diagnostics().unwrap().selected_strategy,
            ExtractionStrategyInfo::RelaxedVisibility
        );

        let duplicate = r#"<body><article class="desktop"><p>The responsive article has useful visible content, a complete explanation, practical implementation details, and careful validation guidance for readers.</p></article><article class="d-none mobile"><p>The responsive article has useful visible content, a complete explanation, practical implementation details, and careful validation guidance for readers.</p></article></body>"#;
        let page = crate::Extractor::builder()
            .diagnostics(true)
            .build()
            .extract(duplicate, None)
            .unwrap();
        assert_eq!(
            page.diagnostics().unwrap().selected_strategy,
            ExtractionStrategyInfo::Normal
        );
        assert_eq!(page.text().matches("responsive article").count(), 1);

        let hidden_paragraph = r#"<body><p class="d-none">Utility-hidden paragraph must not leak into the result.</p><main><p>Visible short answer.</p></main></body>"#;
        let page = crate::extract(hidden_paragraph, None).unwrap();
        assert_eq!(page.text(), "Visible short answer.");
    }

    #[test]
    fn short_multibyte_hidden_variant_is_not_duplicate() {
        let text = "界".repeat(34);
        let html = format!(
            "<body><article><p>{text}</p></article><article class=\"d-none\"><p>{text}</p></article></body>"
        );
        let dom = Dom::parse_document(&html).unwrap();
        let config = ExtractorConfig::default();
        let extractor = ContentExtractor::from_document(dom, None, &config);
        let hidden = extractor
            .dom
            .descendants(extractor.dom.root())
            .find(|&node| extractor.dom.attr(node, AttrName::Class) == Some("d-none"))
            .unwrap();

        assert!(!extractor.is_duplicate_hidden_variant(hidden));
    }

    #[test]
    fn long_multibyte_hidden_variant_compares_trimmed_text() {
        let text = "界".repeat(100);
        let html = format!(
            "<body><article><p> {text} </p></article><article class=\"d-none\">\n<p> {text} </p>\n</article></body>"
        );
        let dom = Dom::parse_document(&html).unwrap();
        let config = ExtractorConfig::default();
        let extractor = ContentExtractor::from_document(dom, None, &config);
        let hidden = extractor
            .dom
            .descendants(extractor.dom.root())
            .find(|&node| extractor.dom.attr(node, AttrName::Class) == Some("d-none"))
            .unwrap();

        assert!(extractor.is_duplicate_hidden_variant(hidden));
    }

    #[test]
    fn recognizes_title_prefix_with_whitespace_before_separator() {
        assert!(heading_matches_page_title("Article | Example", "Article"));
        assert!(!heading_matches_page_title("Different title", "Article"));
    }

    #[test]
    fn finds_linked_brand_headings_without_subtree_rescans() {
        let wrappers = "<div>".repeat(70);
        let closing = "</div>".repeat(70);
        let html = format!(
            r#"{wrappers}<header><h1 class="brand"><a href="/">Header brand</a></h1></header><h1 class="brand"><span><a href="/">Example</a></span></h1><main><h1><span>Article title</span></h1></main>{closing}"#,
        );
        let dom = Dom::parse_document(&html).unwrap();
        let source = Url::parse("https://example.com/article").unwrap();
        let snapshot = dom.element_descendants_snapshot_with_depth(dom.root());
        let plan = title_heading_plan(
            &dom,
            SourceElements::Snapshot(&snapshot),
            "Article title | Example",
            "Article title",
            Some("Example"),
            Some(&source),
        );

        assert_eq!(plan.brand_headings.len(), 1);
        assert_eq!(
            dom.attr(plan.brand_headings[0], AttrName::Class),
            Some("brand")
        );
        assert!(plan.preferred.is_some());
    }

    #[test]
    fn removes_a_duplicate_aria_page_title_heading() {
        let page = crate::extract(
            r#"<html><head><title>Article title</title></head><body><main><div role="heading" aria-level="1">Article title</div><p>The article contains enough useful text to select this main region and retain its complete explanation.</p><p>A second paragraph confirms that the semantic heading does not duplicate the resolved page title.</p></main></body></html>"#,
            None,
        )
        .unwrap();

        assert_eq!(page.metadata().title.as_deref(), Some("Article title"));
        assert!(!page.text().contains("Article title"));
        assert!(page.text().contains("complete explanation"));
    }

    #[test]
    fn readability_discovery_adds_and_selects_a_non_semantic_candidate() {
        let html = r#"<body><blockquote><p>
            Traditional article prose has enough detail, punctuation, and length to identify this container.
        </p></blockquote><footer>Page footer</footer></body>"#;
        let dom = Dom::parse_document(html).unwrap();
        let content = dom
            .first_descendant_by_tag(dom.root(), Tag::Blockquote)
            .unwrap();
        let config = ExtractorConfig::default();
        let mut readability = ContentExtractor::from_document(dom, None, &config);
        let mut text_buffer = String::new();
        let discovery = readability.discover_candidates(&mut text_buffer);
        assert!(!discovery.candidates.is_semantic(content));
        let mut scoring_dom = readability.dom.clone();
        let mut to_score = discovery.to_score;
        let prepared = prepare_readability_structure(
            &mut scoring_dom,
            &discovery.divs_to_prepare,
            &discovery.candidates,
        );
        for node in prepared {
            if readability.node_data.mark_score_seen(node) {
                to_score.push(node);
            }
        }
        let excluded_mask = build_exclusion_mask(&scoring_dom, &discovery.remove_after_scoring);
        let scores = compute_readability_scores(
            &mut scoring_dom,
            None,
            to_score,
            &discovery.remove_after_scoring,
            &excluded_mask,
            &mut readability.node_data,
            readability.strategy.weight_classes(),
        );
        assert!(scores.iter().any(|score| score.node == content));

        let mut candidates = discovery.candidates;
        let ranked =
            readability.rank_candidates(&scoring_dom, &mut candidates, scores, &excluded_mask);
        assert_eq!(ranked[0].node, content);
    }

    #[test]
    fn semantic_ranking_can_override_the_readability_winner() {
        let html = r#"<body><main><h2>Semantic context</h2><blockquote>
            <p>Focused sentence has enough text.</p>
        </blockquote></main></body>"#;
        let dom = Dom::parse_document(html).unwrap();
        let main = dom.first_descendant_by_tag(dom.root(), Tag::Main).unwrap();
        let blockquote = dom
            .first_descendant_by_tag(dom.root(), Tag::Blockquote)
            .unwrap();
        let config = ExtractorConfig::default();
        let mut readability = ContentExtractor::from_document(dom, None, &config);
        let mut text_buffer = String::new();
        let discovery = readability.discover_candidates(&mut text_buffer);
        let mut scoring_dom = readability.dom.clone();
        let mut to_score = discovery.to_score;
        let prepared = prepare_readability_structure(
            &mut scoring_dom,
            &discovery.divs_to_prepare,
            &discovery.candidates,
        );
        for node in prepared {
            if readability.node_data.mark_score_seen(node) {
                to_score.push(node);
            }
        }
        let excluded_mask = build_exclusion_mask(&scoring_dom, &discovery.remove_after_scoring);
        let scores = compute_readability_scores(
            &mut scoring_dom,
            None,
            to_score,
            &discovery.remove_after_scoring,
            &excluded_mask,
            &mut readability.node_data,
            readability.strategy.weight_classes(),
        );

        let raw_winner = scores
            .iter()
            .max_by(|left, right| left.score.total_cmp(&right.score))
            .unwrap();
        assert_eq!(raw_winner.node, blockquote);
        let mut candidates = discovery.candidates;
        let ranked =
            readability.rank_candidates(&scoring_dom, &mut candidates, scores, &excluded_mask);
        assert_eq!(ranked[0].node, main);
    }

    #[test]
    fn ranking_uses_statistics_from_the_current_tree_view() {
        let html = r#"<body><main id="wanted"><p>Visible answer.</p><div id="excluded"><a href="/ad">Excluded linked promotion with many extra words.</a></div></main></body>"#;
        let source = Dom::parse_document(html).unwrap();
        let main = source
            .first_descendant_by_tag(source.root(), Tag::Main)
            .unwrap();
        let excluded = source
            .descendants(source.root())
            .find(|&node| source.attr(node, AttrName::Id) == Some("excluded"))
            .unwrap();
        let config = ExtractorConfig::default();
        let mut readability = ContentExtractor::from_document(source.clone(), None, &config);
        readability.node_data.enable_link_lengths();
        let stale = get_or_compute_stats(&source, main, &mut readability.node_data);
        assert!(stale.text_length > 50);
        assert!(readability.node_data.link_length(main) > 0.0);

        let mut ranking_dom = source;
        ranking_dom.detach(excluded);
        // The extraction pipeline invalidates cached statistics immediately
        // after it mutates the scoring tree. Ranking can then reuse them.
        readability.node_data.clear_stats();
        let mut candidates = CandidateSet::discover_semantic(&ranking_dom);
        readability.rank_candidates(&ranking_dom, &mut candidates, SmallVec::new(), &[]);

        let fresh = get_or_compute_stats(&ranking_dom, main, &mut readability.node_data);
        assert_eq!(fresh.text_length, 15);
        assert_eq!(readability.node_data.link_length(main), 0.0);
    }

    #[test]
    fn discovery_preserves_unlikely_subtrees_for_scoring() {
        let html = r#"<body>
            <div class="sidebar" id="unlikely"><p>This sidebar text is long enough to inspect.</p></div>
            <main><p>This primary content is long enough to score as a candidate.</p></main>
        </body>"#;
        let dom = Dom::parse_document(html).unwrap();
        let config = ExtractorConfig::default();
        let mut readability = ContentExtractor::from_document(dom, None, &config);
        let unlikely = readability
            .dom
            .descendants(readability.dom.root())
            .find(|&id| readability.dom.attr(id, AttrName::Id) == Some("unlikely"))
            .unwrap();
        let parent = readability.dom.parent(unlikely);
        let dom_len = readability.dom.len();
        let mut text_buffer = String::new();

        let discovery = readability.discover_candidates(&mut text_buffer);
        let normal = readability
            .dom
            .descendants(readability.dom.root())
            .find(|&id| readability.dom.tag(id) == Some(Tag::Main))
            .unwrap();
        let normal_parent = readability.dom.parent(normal);
        let normal_tag = readability.dom.tag(normal);
        let normal_attrs = readability.dom.attrs(normal).to_vec();
        let mut scoring_dom = readability.dom.clone();
        let prepared = prepare_readability_structure(
            &mut scoring_dom,
            &discovery.divs_to_prepare,
            &discovery.candidates,
        );
        let mut to_score = discovery.to_score.clone();
        for id in prepared {
            if readability.node_data.mark_score_seen(id) {
                to_score.push(id)
            }
        }
        let excluded_mask = build_exclusion_mask(&scoring_dom, &discovery.remove_after_scoring);
        let scores = compute_readability_scores(
            &mut scoring_dom,
            None,
            to_score,
            &discovery.remove_after_scoring,
            &excluded_mask,
            &mut readability.node_data,
            readability.strategy.weight_classes(),
        );
        let mut candidates = discovery.candidates;
        let _ = readability.rank_candidates(&scoring_dom, &mut candidates, scores, &excluded_mask);

        // Scoring can replace simple wrappers in its private copy. The source
        // subtree remains intact and contributes the same visible text.
        assert!(
            scoring_dom
                .text(scoring_dom.root())
                .contains("sidebar text")
        );
        assert_eq!(readability.dom.parent(unlikely), parent);
        assert_eq!(readability.dom.parent(normal), normal_parent);
        assert_eq!(readability.dom.tag(normal), normal_tag);
        assert_eq!(readability.dom.attrs(normal), normal_attrs);
        assert_eq!(readability.dom.len(), dom_len);
        assert!(!discovery.remove_after_scoring.contains(&unlikely));
    }
}
