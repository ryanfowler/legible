//! General content extraction and strategy-based retry orchestration.
#![allow(clippy::collapsible_if)]
use crate::candidate::{
    CandidateSet, CandidateSource, RankedCandidate, RootSelection, RootSelectionReason,
    locate_structured_content, select_content_root,
};
use crate::cleaning::*;
use crate::constants::{is_alter_to_div_exception, is_default_tag_to_score, regexps};
use crate::diagnostics::{
    AttemptRejectionReason, CandidateSourceInfo, CleanupActionInfo, CleanupActionKind,
    ContentMetricsInfo, ExtractionAttempt, ExtractionDiagnostics, ExtractionStrategyInfo,
    NormalizationCountsInfo, QualityInfo, RootInfo, RootSelectionReasonInfo,
};
use crate::dom::{AttrName, Dom, NodeId, NodeStateStore, Tag, build_match_string};
use crate::error::{Error, Result};
use crate::extractor::{ContentHint, ContentTag, ExtractorConfig};
use crate::logging::debug_log;
use crate::metadata::{self, Metadata, MetadataDiagnostics, StructuredData};
use crate::normalize::{
    accessible_math_nodes, adjacent_lead_media, adopt_external_footnotes,
    collect_external_footnotes, finish_normalization, has_primary_heading_semantics,
    normalize_after_cleanup, normalize_scoring_structure, normalize_svg_before_scoring,
    preserve_semantics_before_cleanup, remove_decorative_media_before_cleanup,
};
use crate::page::ExtractedPage;
use crate::page_kind::PageKind;
use crate::quality::{
    ContentMetrics, ExtractionQuality, is_access_barrier, is_incoherent_short_result,
    is_interactive_shell,
};
use crate::scoring::*;
use crate::specialized::{self, DocumentContext};
use html5ever::ns;
use regex::Regex;
use smallvec::SmallVec;
use url::Url;

pub(crate) struct ContentExtractor<'a> {
    dom: Dom,
    options: &'a ExtractorConfig,
    strategy: ExtractionStrategy,
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
    resolve_fragment_links: bool,
    url_error: Option<url::ParseError>,
    best_attempt: Option<BestAttempt>,
    diagnostic_attempts: Option<Vec<ExtractionAttempt>>,
    diagnostic_cleanup_actions: Vec<CleanupActionInfo>,
    diagnostic_normalization: NormalizationCountsInfo,
    specialized_root: Option<NodeId>,
    specialized_identity: Option<&'static str>,
    page_kind: PageKind,
}

#[derive(Default)]
struct TitleHeadingPlan {
    preferred: Option<NodeId>,
    brand_headings: SmallVec<[NodeId; 2]>,
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

struct BestAttempt {
    content: FrozenContent,
    quality: ExtractionQuality,
    excerpt: Option<String>,
    direction: Option<String>,
    strategy: ExtractionStrategy,
    diagnostic_index: Option<usize>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ExtractionStrategy {
    Normal,
    RelaxedCleanup,
    BroadContent,
    StructuredDataHint,
    RelaxedVisibility,
    BodyFallback,
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
}
struct FrozenContent {
    dom: Dom,
}
struct ExtractedContent {
    text_length: usize,
    excerpt: Option<String>,
    /// The node whose children form the output fragment.
    content_root: NodeId,
}

struct CandidateDiscovery {
    candidates: CandidateSet,
    to_score: SmallVec<[NodeId; 256]>,
    divs_to_prepare: SmallVec<[NodeId; 128]>,
    remove_after_scoring: SmallVec<[NodeId; 64]>,
}

fn find_content_targets(dom: &Dom, target: &ContentHint) -> Vec<NodeId> {
    if matches!(target, ContentHint::Id(value) | ContentHint::Class(value) if value.trim().is_empty())
    {
        return Vec::new();
    }
    dom.descendants(dom.root())
        .filter(|&node| {
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
        })
        .collect()
}

impl<'a> ContentExtractor<'a> {
    pub(crate) fn from_document(dom: Dom, url: Option<&str>, options: &'a ExtractorConfig) -> Self {
        let (base_uri, url_error) = match url {
            Some(x) => match Url::parse(x) {
                Ok(u) => (Some(u), None),
                Err(e) => (None, Some(e)),
            },
            None => (None, None),
        };
        Self {
            dom,
            options,
            strategy: ExtractionStrategy::Normal,
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
            resolve_fragment_links: false,
            url_error,
            best_attempt: None,
            diagnostic_attempts: options.diagnostics.then(Vec::new),
            diagnostic_cleanup_actions: Vec::new(),
            diagnostic_normalization: NormalizationCountsInfo::default(),
            specialized_root: None,
            specialized_identity: None,
            page_kind: PageKind::Unknown,
        }
    }
    pub(crate) fn extract(mut self) -> Result<ExtractedPage> {
        if let Some(e) = self.url_error {
            return Err(Error::InvalidUrl(e));
        }
        if self.options.max_elements > 0 {
            let n = self
                .dom
                .descendants(self.dom.root())
                .filter(|&x| self.dom.is_element(x))
                .count();
            if n > self.options.max_elements {
                return Err(Error::TooManyElements(n, self.options.max_elements));
            }
        }
        if let Some(base) = self.dom.descendants(self.dom.root()).find(|&id| {
            self.dom
                .qual_name(id)
                .is_some_and(|name| name.ns == ns!(html) && name.local.as_ref() == "base")
                && self.dom.attr(id, AttrName::Href).is_some()
        }) && let Some(href) = self.dom.attr(base, AttrName::Href)
        {
            let base_uri = self
                .base_uri
                .as_ref()
                .map_or_else(|| Url::parse(href), |document_uri| document_uri.join(href));
            if let Ok(base_uri) = base_uri {
                self.resolve_fragment_links = self
                    .source_uri
                    .as_ref()
                    .is_some_and(|document_uri| base_uri != *document_uri);
                self.base_uri = Some(base_uri);
            }
        }
        // Metadata must inspect the parsed source before preparation removes or
        // rewrites any nodes. Image preparation happens afterwards because it
        // can replace placeholder and noscript subtrees.
        let title = metadata::get_page_title(&self.dom);
        self.structured_title = metadata::content_identity_title(&self.dom, &title);
        if self.options.structured_data {
            self.structured_data = StructuredData::parse(&self.dom);
        }
        (self.metadata, self.metadata_diagnostics) = metadata::discover_with_diagnostics(
            &self.dom,
            &self.structured_data,
            &title,
            self.base_uri.as_ref(),
            self.source_uri.as_ref(),
            self.options.metadata_diagnostics,
        );
        if self
            .structured_data
            .primary_texts(&self.structured_title, self.source_uri.as_ref())
            .next()
            .is_some()
        {
            debug_log!(self, "Structured data contains a content-location hint");
        }
        unwrap_noscript_images(&mut self.dom);
        prep_document(&mut self.dom);
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
        self.page_title = self
            .metadata
            .title
            .take()
            .or_else(|| metadata::normalize_title(&title))
            .unwrap_or_default();
        let content = self.extract_content()?;
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
        // The extraction result already owns a compact fragment. Keep that
        // fragment instead of copying every retained node a second time.
        let extracted_root = content.content_root;
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
            self.dom,
            extracted_root,
            self.metadata,
            content.text_length,
            diagnostics,
            self.metadata_diagnostics,
            retained_structured_data,
            self.base_uri.as_ref(),
        ))
    }
    fn extract_content(&mut self) -> Result<ExtractedContent> {
        let body = self.dom.body().ok_or(Error::NoBody)?;
        let exact_root = if let Some(target) = &self.options.content_root {
            Some(
                find_content_targets(&self.dom, target)
                    .into_iter()
                    .next()
                    .ok_or(Error::ContentRootNotFound)?,
            )
        } else {
            self.specialized_root
        };
        let footnote_definitions = collect_external_footnotes(&self.dom);
        let source_metrics = exact_root.map_or_else(
            || ContentMetrics::measure_source(&self.dom, body),
            |root| ContentMetrics::measure(&self.dom, root),
        );
        let has_relaxable_hidden_content = self.has_relaxable_hidden_content(body);
        let relaxed_source_metrics = if has_relaxable_hidden_content {
            ContentMetrics::measure_source_relaxed_visibility(&self.dom, body)
        } else {
            source_metrics
        };
        if !source_metrics.has_meaningful_text() && !relaxed_source_metrics.has_meaningful_text() {
            return Err(Error::NoContent);
        }
        let short_source_access_barrier = (source_metrics.word_count <= 60
            || source_metrics.text_chars <= 400)
            && is_access_barrier(&self.dom, body);
        let substantial_hidden_gain = relaxed_source_metrics.text_chars
            >= source_metrics.text_chars.saturating_mul(2)
            && relaxed_source_metrics.text_chars >= source_metrics.text_chars.saturating_add(1_000);
        let visibility_recovery_needed = has_relaxable_hidden_content
            && (source_metrics.word_count <= 30
                || source_metrics.text_chars <= 200
                || substantial_hidden_gain)
            && relaxed_source_metrics.text_chars >= source_metrics.text_chars.saturating_mul(2)
            && relaxed_source_metrics.text_chars >= source_metrics.text_chars.saturating_add(100);
        let structured_root = locate_structured_content(
            &self.dom,
            self.structured_data
                .primary_texts(&self.structured_title, self.source_uri.as_ref()),
        );
        let mut match_buffer = String::new();
        let mut text_buffer = String::new();
        let mut cleaning_nodes = Vec::new();
        // These indexes depend only on the prepared source. Reuse them across
        // strategy retries instead of rebuilding the same preorder snapshot
        // and title plan for every attempt.
        let initial_nodes = self
            .dom
            .element_descendants_snapshot_with_depth(self.dom.root());
        let accessible_math = accessible_math_nodes(&self.dom, &initial_nodes);
        let base_candidates =
            CandidateSet::discover_semantic_from_snapshot(&self.dom, &initial_nodes);
        let title_plan = title_heading_plan(
            &self.dom,
            &self.page_title,
            &self.structured_title,
            self.metadata.site_name.as_deref(),
            self.source_uri.as_ref(),
        );
        if let Some(html) = self.dom.html_element() {
            if let Some(lang) = self.dom.attr(html, AttrName::Lang) {
                self.page_language = Some(lang.into())
            }
            if let Some(dir) = self.dom.attr(html, AttrName::Dir) {
                self.page_direction = Some(dir.into())
            }
        }
        for strategy in ExtractionStrategy::ORDER {
            if strategy == ExtractionStrategy::StructuredDataHint && structured_root.is_none() {
                continue;
            }
            if strategy == ExtractionStrategy::RelaxedVisibility && !has_relaxable_hidden_content {
                continue;
            }
            if strategy != ExtractionStrategy::RelaxedVisibility
                && !source_metrics.has_meaningful_text()
            {
                continue;
            }
            self.strategy = strategy;
            self.diagnostic_cleanup_actions.clear();
            self.diagnostic_normalization = NormalizationCountsInfo::default();
            // Discovery only records nodes. It does not mutate the source DOM.
            // Deferred removals stay attached until all candidate scores and
            // link-density values have been calculated.
            let mut discovery = self.discover_candidates_with_indexes(
                &mut match_buffer,
                &mut text_buffer,
                &initial_nodes,
                &accessible_math,
                &title_plan,
                &base_candidates,
            );
            if let Some(root) = self.specialized_root {
                // A specialized extractor has already separated content from
                // page chrome. Generic discovery can still provide ranking
                // data, but it must not remove canonical titles or bylines.
                discovery.remove_after_scoring.retain(|node| {
                    *node != root && !self.dom.ancestors(*node).any(|ancestor| ancestor == root)
                });
                self.page_byline = None;
            }

            // Prepare and score one working copy. Score propagation runs before
            // deferred clutter is detached. The prepared source stays intact
            // for retries.
            let mut working_dom = self.dom.clone();
            let working_root = working_dom.root();
            normalize_scoring_structure(&mut working_dom, working_root);
            let mut to_score = discovery.to_score;
            let prepared = prepare_readability_structure(
                &mut working_dom,
                &discovery.divs_to_prepare,
                &discovery.candidates,
            );
            self.node_data.sync_len(working_dom.len());
            for id in prepared {
                if self.node_data.mark_score_seen(id) {
                    to_score.push(id)
                }
            }
            let excluded_mask = build_exclusion_mask(&working_dom, &discovery.remove_after_scoring);
            let readability_scores = compute_readability_scores(
                &mut working_dom,
                to_score,
                &discovery.remove_after_scoring,
                &excluded_mask,
                &mut self.node_data,
                self.strategy.weight_classes(),
            );
            let mut candidates = discovery.candidates;
            if let Some(hint) = &self.options.content_hint {
                for node in find_content_targets(&working_dom, hint) {
                    candidates.add_caller_hint(node);
                }
            }
            if let Some(root) = structured_root {
                candidates.add_structured_data(root);
            }
            let ranked = self.rank_candidates(
                &working_dom,
                &mut candidates,
                readability_scores,
                &excluded_mask,
            );
            let body = working_dom.body().ok_or(Error::NoBody)?;
            let mut selection = select_content_root(
                &working_dom,
                &candidates,
                &ranked,
                body,
                self.structured_data
                    .primary_texts(&self.structured_title, self.source_uri.as_ref()),
            );
            selection = self.selection_for_strategy(
                strategy,
                &working_dom,
                body,
                selection,
                structured_root,
            );
            if strategy == ExtractionStrategy::RelaxedVisibility
                && short_source_access_barrier
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
            if let Some(node) = exact_root {
                selection = RootSelection {
                    node,
                    reason: RootSelectionReason::SpecificChild,
                    branches: SmallVec::new(),
                };
            }
            let visibility_root_semantic = matches!(
                working_dom.tag(selection.node),
                Some(Tag::Article | Tag::Main)
            ) || working_dom
                .attr(selection.node, AttrName::Role)
                .is_some_and(Self::has_primary_role);
            let root_info = self.root_info(
                &working_dom,
                &candidates,
                &selection,
                ranked.first().map(|candidate| candidate.node),
            );
            let root_in_document_chrome =
                Self::is_document_chrome_root(&working_dom, selection.node, body);
            if selection.node == body {
                Self::prune_body_fallback_chrome(&mut working_dom, body);
                selection.branches.clear();
            }

            // Move the selected working tree into the extraction path. Keep the
            // prepared source as the immutable input for a possible retry.
            let source_dom = std::mem::replace(&mut self.dom, working_dom);
            let body = self.dom.body().ok_or(Error::NoBody)?;
            let (top_id, synthetic) = if !selection.branches.is_empty() {
                let container = self
                    .create_container(selection.branches[0], &selection.branches)
                    .ok_or(Error::NoContent)?;
                initialize_node(
                    &self.dom,
                    container,
                    &mut self.node_data,
                    self.strategy.weight_classes(),
                );
                (container, true)
            } else if selection.node == body {
                let container = self
                    .dom
                    .create_html_element(Tag::Div)
                    .map_err(|_| Error::NoContent)?;
                let children: SmallVec<[NodeId; 16]> = self.dom.children(body).collect();
                for child in children {
                    self.dom.append_child(container, child)
                }
                self.dom.append_child(body, container);
                initialize_node(
                    &self.dom,
                    container,
                    &mut self.node_data,
                    self.strategy.weight_classes(),
                );
                (container, true)
            } else {
                let mut top_id = selection.node;
                // Ancestor validation changes the boundary, not the output
                // contract. Keep the established generic content wrapper.
                if selection.reason == RootSelectionReason::CompleteAncestor
                    && matches!(self.dom.tag(top_id), Some(Tag::Article | Tag::Main))
                {
                    self.dom.rename_html(top_id, Tag::Div);
                    self.dom.remove_attr(top_id, AttrName::ItemProp);
                }
                // Preserve useful boundary expansion from the prose scoring algorithm when the
                // structural selector accepts the raw ranking winner. Explicit
                // child, parent, or schema choices are already final.
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
                        .map(|candidate| self.dom.ancestors(candidate.node).collect())
                        .collect();
                    if alternatives.len() >= 3 {
                        let mut parent = self.dom.parent(top_id);
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
                            parent = self.dom.parent(node)
                        }
                    }
                    if !self.node_data.has(top_id) {
                        initialize_node(
                            &self.dom,
                            top_id,
                            &mut self.node_data,
                            self.strategy.weight_classes(),
                        )
                    }
                    let threshold = self.node_data.get_content_score(top_id) / 3.0;
                    let mut last = self.node_data.get_content_score(top_id);
                    let mut parent = self.dom.parent(top_id);
                    while let Some(node) = parent {
                        if node == body {
                            break;
                        }
                        if let Some(score) = self.node_data.get(node).map(|data| data.content_score)
                        {
                            if score < threshold {
                                break;
                            }
                            if score > last {
                                top_id = node;
                                break;
                            }
                            last = score;
                        }
                        parent = self.dom.parent(node)
                    }
                    while let Some(parent) = self.dom.parent(top_id) {
                        if parent == body {
                            break;
                        }
                        let mut children = self.dom.element_children(parent);
                        if children.next().is_some() && children.next().is_none() {
                            top_id = parent;
                        } else {
                            break;
                        }
                    }
                }
                // A compact semantic article can lose its lead heading when a
                // syntax-highlighted wrapper wins readability scoring. Promote
                // to that boundary only when it is small, link-light, and the
                // omitted prefix is a heading with at most one lead paragraph.
                if selection.reason == RootSelectionReason::Ranked
                    && (self.dom.tag(top_id) == Some(Tag::Pre)
                        || self
                            .dom
                            .descendants(top_id)
                            .any(|node| self.dom.tag(node) == Some(Tag::Pre)))
                    && let Some(article) = self.dom.ancestors(top_id).take(8).find(|&ancestor| {
                        matches!(self.dom.tag(ancestor), Some(Tag::Article | Tag::Main))
                            && self.dom.normalized_char_count(ancestor) <= 10_000
                    })
                    && has_compact_code_page_structure(&self.dom, article)
                    && (has_compact_code_lead(&self.dom, article, top_id)
                        || has_line_number_table_marker(&self.dom, top_id))
                {
                    top_id = article;
                }
                if !self.node_data.has(top_id) {
                    initialize_node(
                        &self.dom,
                        top_id,
                        &mut self.node_data,
                        self.strategy.weight_classes(),
                    )
                }
                (top_id, false)
            };
            let lead_media = (!synthetic && exact_root.is_none())
                .then(|| adjacent_lead_media(&self.dom, top_id))
                .flatten();
            let content_id = if synthetic {
                top_id
            } else {
                let siblings = if selection.reason == RootSelectionReason::Ranked {
                    Self::gather_siblings(
                        &self.dom,
                        top_id,
                        &mut self.node_data,
                        self.options.debug,
                    )
                } else {
                    SmallVec::from_slice(&[top_id])
                };
                self.create_container(top_id, &siblings).unwrap_or(top_id)
            };
            remove_title_brand_headings(&mut self.dom, content_id, &title_plan);
            if let Some(lead_media) = lead_media
                && lead_media != content_id
                && self.dom.parent(lead_media).is_some()
                && let Some(first_child) = self.dom.first_child(content_id)
            {
                self.dom.insert_before(first_child, lead_media);
            }

            if let Some(direction) = std::iter::once(top_id)
                .chain(self.dom.ancestors(top_id))
                .find_map(|node| self.dom.attr(node, AttrName::Dir))
            {
                self.page_direction = Some(direction.to_owned());
            }

            // Cleanup owns a compact copy of the selected region. The source
            // DOM remains available for a retry and is never affected by an
            // earlier attempt's mutations.
            let mut fragment = self
                .dom
                .copy_subtree_as_fragment(content_id)
                .map_err(|_| Error::NoContent)?;
            let content_id = fragment
                .first_child(fragment.root())
                .ok_or(Error::NoContent)?;
            if exact_root.is_none() {
                adopt_external_footnotes(&footnote_definitions, &mut fragment, content_id);
            }
            self.dom = fragment;
            self.node_data.clear();
            self.node_data.enable_link_lengths();

            let interactive_shell = is_interactive_shell(&self.dom, content_id);
            let video = regexps::VIDEOS.clone();
            self.prep_article(
                content_id,
                &video,
                &mut match_buffer,
                &mut text_buffer,
                &mut cleaning_nodes,
            );
            if synthetic {
                self.dom
                    .set_attr(content_id, AttrName::Id, "legible-content");
                self.dom.set_attr(content_id, AttrName::Class, "page")
            } else {
                let w = self
                    .dom
                    .create_html_element(Tag::Div)
                    .map_err(|_| Error::NoContent)?;
                self.dom.set_attr(w, AttrName::Id, "legible-content");
                self.dom.set_attr(w, AttrName::Class, "page");
                let children: SmallVec<[NodeId; 16]> = self.dom.children(content_id).collect();
                for x in children {
                    self.dom.append_child(w, x)
                }
                self.dom.append_child(content_id, w)
            }
            let excerpt = self.content_excerpt(content_id);
            let access_barrier = is_access_barrier(&self.dom, content_id);
            self.post_process(content_id, &mut cleaning_nodes);
            self.capture_normalization_counts(content_id);
            let result_dom = if synthetic {
                self.dom.copy_subtree_as_fragment(content_id)
            } else {
                self.dom.copy_children_as_fragment(content_id)
            }
            .map_err(|_| Error::NoContent)?;
            let result_metrics = ContentMetrics::measure(&result_dom, result_dom.root());
            let incoherent_short =
                is_incoherent_short_result(&result_dom, result_dom.root(), result_metrics);
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
                self,
                "Extraction strategy {:?}: words={}, coverage={:.3}, links={:.3}",
                strategy,
                quality.word_count,
                quality.coverage,
                quality.link_density
            );

            let schema_match = structured_root == Some(selection.node)
                && !quality.is_suspiciously_small()
                && (quality.coverage >= 0.2 || quality.text_chars >= 500);
            let ignores_visible_source_barrier =
                strategy == ExtractionStrategy::RelaxedVisibility && has_relaxable_hidden_content;
            let valid_result = if exact_root.is_some() {
                result_metrics.has_meaningful_text()
            } else {
                !root_in_document_chrome
                    && !access_barrier
                    && !(short_source_access_barrier && !ignores_visible_source_barrier)
                    && !interactive_shell
                    && !incoherent_short
            };
            let visibility_candidate_coherent = strategy != ExtractionStrategy::RelaxedVisibility
                || result_metrics.paragraph_count >= 2
                || visibility_root_semantic && result_metrics.structured_block_count > 0;
            let visibility_improves = visibility_candidate_coherent
                && (strategy != ExtractionStrategy::RelaxedVisibility
                    || self.best_attempt.as_ref().is_none_or(|best| {
                        quality.text_chars >= best.quality.text_chars.saturating_mul(2)
                            || quality.text_chars > best.quality.text_chars
                                && quality.coverage >= best.quality.coverage + 0.2
                    }));
            let deferred_for_visibility =
                visibility_recovery_needed && strategy != ExtractionStrategy::RelaxedVisibility;
            let accepted = valid_result
                && (exact_root.is_some()
                    || visibility_improves
                        && !deferred_for_visibility
                        && (quality.is_good() || schema_match));
            if accepted {
                self.record_attempt(
                    strategy,
                    root_info,
                    attempt_source_metrics,
                    result_metrics,
                    quality,
                    true,
                    None,
                );
                self.dom = result_dom;
                let root = self.dom.root();
                return Ok(ExtractedContent {
                    text_length: quality.text_chars,
                    excerpt,
                    content_root: root,
                });
            }

            let rejection = Self::attempt_rejection_reason(
                root_in_document_chrome,
                access_barrier,
                short_source_access_barrier && !ignores_visible_source_barrier,
                interactive_shell,
                incoherent_short,
                visibility_improves,
                deferred_for_visibility,
            );
            let diagnostic_index = self.record_attempt(
                strategy,
                root_info,
                attempt_source_metrics,
                result_metrics,
                quality,
                false,
                Some(rejection),
            );
            if valid_result
                && visibility_improves
                && self.best_attempt.as_ref().is_none_or(|best| {
                    quality.best_attempt_score() > best.quality.best_attempt_score()
                })
            {
                if let Some(previous) = self
                    .best_attempt
                    .as_ref()
                    .and_then(|best| best.diagnostic_index)
                    && let Some(attempts) = self.diagnostic_attempts.as_mut()
                {
                    attempts[previous].rejection_reason = Some(AttemptRejectionReason::Superseded);
                }
                self.best_attempt = Some(BestAttempt {
                    content: FrozenContent { dom: result_dom },
                    quality,
                    excerpt,
                    direction: self.page_direction.clone(),
                    strategy,
                    diagnostic_index,
                });
            }
            self.restore_source(source_dom);
        }

        let best = self.best_attempt.take().ok_or(Error::NoContent)?;
        if !best.quality.is_good() && best.quality.is_suspiciously_small() {
            return Err(Error::NoContent);
        }
        self.dom = best.content.dom;
        self.page_direction = best.direction;
        self.strategy = best.strategy;
        if let Some(index) = best.diagnostic_index
            && let Some(attempts) = self.diagnostic_attempts.as_mut()
        {
            attempts[index].accepted = true;
            attempts[index].rejection_reason = None;
        }
        let root = self.dom.root();
        Ok(ExtractedContent {
            text_length: best.quality.text_chars,
            excerpt: best.excerpt,
            content_root: root,
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
        accepted: bool,
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
            cleanup_actions: self.diagnostic_cleanup_actions.clone(),
            normalization: self.diagnostic_normalization,
            accepted,
            rejection_reason,
        });
        Some(index)
    }

    fn metrics_info(metrics: ContentMetrics) -> ContentMetricsInfo {
        ContentMetricsInfo {
            word_count: metrics.word_count,
            text_chars: metrics.text_chars,
            paragraph_count: metrics.paragraph_count,
            heading_count: metrics.heading_count,
            structured_block_count: metrics.structured_block_count,
            link_density: metrics.link_density,
        }
    }

    fn attempt_rejection_reason(
        document_chrome: bool,
        access_barrier: bool,
        source_access_barrier: bool,
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

    fn prune_body_fallback_chrome(dom: &mut Dom, body: NodeId) {
        let elements = dom.element_descendants_snapshot_with_depth(body);
        let has_primary_region = elements.iter().any(|&(node, _)| {
            matches!(dom.tag(node), Some(Tag::Main | Tag::Article))
                || dom
                    .attr(node, AttrName::Role)
                    .is_some_and(Self::has_primary_role)
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
                    .is_some_and(Self::has_primary_role);
            let role = dom.attr(node, AttrName::Role);
            let document_chrome =
                matches!(dom.tag(node), Some(Tag::Header | Tag::Footer | Tag::Nav))
                    || role.is_some_and(|roles| {
                        roles.split_whitespace().any(|role| {
                            role.eq_ignore_ascii_case("banner")
                                || role.eq_ignore_ascii_case("navigation")
                        })
                    });
            let contextual_sidebar = dom.tag(node) == Some(Tag::Aside)
                || role.is_some_and(|roles| {
                    roles
                        .split_whitespace()
                        .any(|role| role.eq_ignore_ascii_case("complementary"))
                });
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

    fn has_primary_role(roles: &str) -> bool {
        roles
            .split_whitespace()
            .any(|role| role.eq_ignore_ascii_case("main") || role.eq_ignore_ascii_case("article"))
    }

    fn is_document_chrome_root(dom: &Dom, node: NodeId, body: NodeId) -> bool {
        let protected = std::iter::once(node)
            .chain(dom.ancestors(node))
            .take_while(|&ancestor| ancestor != body)
            .any(|ancestor| {
                matches!(dom.tag(ancestor), Some(Tag::Main | Tag::Article))
                    || dom
                        .attr(ancestor, AttrName::Role)
                        .is_some_and(Self::has_primary_role)
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
                        roles.split_whitespace().any(|role| {
                            role.eq_ignore_ascii_case("banner")
                                || role.eq_ignore_ascii_case("complementary")
                                || role.eq_ignore_ascii_case("navigation")
                        })
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
                            || dom.attr(node, AttrName::Role).is_some_and(|roles| {
                                roles
                                    .split_whitespace()
                                    .any(|role| role.eq_ignore_ascii_case("main"))
                            })
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
            ExtractionStrategy::BodyFallback => RootSelection {
                node: body,
                reason: RootSelectionReason::BodyFallback,
                branches: SmallVec::new(),
            },
        }
    }

    fn is_visibility_recovery_container(&self, node: NodeId) -> bool {
        matches!(
            self.dom.tag(node),
            Some(Tag::Article | Tag::Aside | Tag::Div | Tag::Main | Tag::Nav | Tag::Section)
        )
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

    fn has_relaxable_hidden_content(&self, root: NodeId) -> bool {
        self.dom.descendants(root).any(|node| {
            self.is_visibility_recovery_container(node)
                && !self.is_modal_or_dialog(node)
                && self.is_static_hidden_marker(node)
        })
    }

    /// Marks hidden roots that have semantic or repeated structural evidence.
    /// Reverse preorder aggregates each subtree without rescanning descendants.
    fn relaxed_hidden_roots(&self) -> Vec<bool> {
        let nodes = self
            .dom
            .element_descendants_snapshot_with_depth(self.dom.root());
        let mut paragraphs = vec![0_u8; self.dom.len()];
        let mut structured = vec![false; self.dom.len()];
        let mut allowed = vec![false; self.dom.len()];
        for &(node, _) in nodes.iter().rev() {
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
            let authoritative = matches!(tag, Some(Tag::Article | Tag::Main))
                || self
                    .dom
                    .attr(node, AttrName::Role)
                    .is_some_and(Self::has_primary_role);
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

    fn is_visible_for_strategy(&self, node: NodeId, accessible_math: &[bool]) -> bool {
        if accessible_math.get(node.index()).copied().unwrap_or(false) {
            return true;
        }
        let utility_hidden = has_hidden_utility_class_for_discovery(&self.dom, node);
        if self.strategy == ExtractionStrategy::RelaxedVisibility {
            self.dom.attr(node, AttrName::AriaHidden) != Some("true")
                || self
                    .dom
                    .attr(node, AttrName::Class)
                    .is_some_and(|class| class.contains("fallback-image"))
        } else {
            is_probably_visible(&self.dom, node) && !utility_hidden
        }
    }

    fn is_modal_or_dialog(&self, node: NodeId) -> bool {
        self.dom.attr(node, AttrName::AriaModal) == Some("true")
            || self.dom.attr(node, AttrName::Role).is_some_and(|roles| {
                roles.split_whitespace().any(|role| {
                    role.eq_ignore_ascii_case("dialog") || role.eq_ignore_ascii_case("alertdialog")
                })
            })
            || (!is_probably_visible(&self.dom, node)
                || has_hidden_utility_class_for_discovery(&self.dom, node))
                && self.dom.attr(node, AttrName::Class).is_some_and(|classes| {
                    classes.split_whitespace().any(|class| {
                        class.eq_ignore_ascii_case("modal") || class.eq_ignore_ascii_case("dialog")
                    })
                })
    }

    #[cfg(test)]
    fn discover_candidates(
        &mut self,
        match_buffer: &mut String,
        text_buffer: &mut String,
    ) -> CandidateDiscovery {
        let initial_nodes = self
            .dom
            .element_descendants_snapshot_with_depth(self.dom.root());
        let accessible_math = accessible_math_nodes(&self.dom, &initial_nodes);
        let title_plan = title_heading_plan(
            &self.dom,
            &self.page_title,
            &self.structured_title,
            self.metadata.site_name.as_deref(),
            self.source_uri.as_ref(),
        );
        let base_candidates =
            CandidateSet::discover_semantic_from_snapshot(&self.dom, &initial_nodes);
        self.discover_candidates_with_indexes(
            match_buffer,
            text_buffer,
            &initial_nodes,
            &accessible_math,
            &title_plan,
            &base_candidates,
        )
    }

    fn discover_candidates_with_indexes(
        &mut self,
        match_buffer: &mut String,
        text_buffer: &mut String,
        initial_nodes: &[(NodeId, u32)],
        accessible_math: &[bool],
        title_plan: &TitleHeadingPlan,
        base_candidates: &CandidateSet,
    ) -> CandidateDiscovery {
        let candidates = base_candidates.clone();
        let relaxed_hidden = (self.strategy == ExtractionStrategy::RelaxedVisibility)
            .then(|| self.relaxed_hidden_roots());
        let mut to_score = SmallVec::<[NodeId; 256]>::new();
        let mut divs_to_prepare = SmallVec::<[NodeId; 128]>::new();
        let mut remove_after_scoring = SmallVec::<[NodeId; 64]>::new();
        self.node_data.sync_len(self.dom.len());
        let heading_limit = heading_text_limit(&self.page_title, &self.structured_title);
        let mut excluded_depth = None;
        let retain_preferred_title =
            title_plan.preferred.is_some() && !title_plan.brand_headings.is_empty();
        let mut remove_title = !retain_preferred_title;
        for &(id, depth) in initial_nodes {
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
            if tag == Tag::A {
                self.node_data.enable_link_lengths();
            }
            let unsupported_hidden = relaxed_hidden.as_ref().is_some_and(|allowed| {
                self.is_static_hidden_marker(id)
                    && (!allowed[id.index()] || self.is_duplicate_hidden_variant(id))
            });
            if !self.is_visible_for_strategy(id, accessible_math)
                || self.is_modal_or_dialog(id)
                || unsupported_hidden
            {
                remove_after_scoring.push(id);
                excluded_depth = Some(depth);
                continue;
            }
            if self.page_byline.is_none() && !self.metadata.has_source_author {
                build_match_string(&self.dom, id, match_buffer);
                if is_valid_byline(&self.dom, id, match_buffer, text_buffer) {
                    self.page_byline =
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
            ) && is_element_without_content(&self.dom, id)
            {
                remove_after_scoring.push(id);
                excluded_depth = Some(depth);
                continue;
            }
            if is_default_tag_to_score(tag) && self.node_data.mark_score_seen(id) {
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
        }
    }

    fn rank_candidates(
        &mut self,
        dom: &Dom,
        candidates: &mut CandidateSet,
        readability_scores: SmallVec<[ReadabilityScore; 64]>,
        excluded: &[bool],
    ) -> SmallVec<[RankedCandidate; 64]> {
        for readability in readability_scores {
            candidates.add_readability(readability.node, readability.score);
        }

        // Readability scoring selectively invalidates ancestors after it
        // detaches deferred clutter. Reuse those refreshed statistics and the
        // unaffected leaf cache. Feature calculation uses the same tree and
        // would otherwise repeat a full postorder text scan.
        let mut table_nodes = Vec::new();
        mark_data_tables(dom, dom.root(), &mut self.node_data, &mut table_nodes);
        let feature_index = CandidateFeatureIndex::new(dom, &self.node_data);
        feature_index.prepare_text_cache(&mut self.node_data);
        for candidate in candidates.iter_mut() {
            candidate.features = feature_index.features(
                dom,
                *candidate,
                &mut self.node_data,
                self.strategy.weight_classes(),
            );
        }

        let context = candidates.ranking_context(dom, &self.node_data);
        let mut scored: SmallVec<[RankedCandidate; 64]> = candidates
            .iter()
            .enumerate()
            .filter_map(|(order, candidate)| {
                if excluded
                    .get(candidate.node.index())
                    .copied()
                    .unwrap_or(false)
                {
                    return None;
                }
                let length =
                    get_or_compute_stats(dom, candidate.node, &mut self.node_data).text_length;
                if length == 0 && Some(candidate.node) != dom.body() {
                    return None;
                }
                let is_semantic = candidate.has_source(CandidateSource::Semantic);
                let is_authoritative = candidates.is_authoritative_semantic(dom, candidate.node);
                let has_readability = context.has_readability(candidate.node);
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
                    && !context.has_authoritative_ancestor(candidate.node)
                    && !context.has_authoritative_descendant(candidate.node, is_authoritative)
                {
                    25.0 * (1.0 - (f64::from(length.min(100)) / 100.0))
                } else {
                    0.0
                };
                let is_main = dom.tag(candidate.node) == Some(Tag::Main)
                    || dom
                        .attr(candidate.node, AttrName::Role)
                        .is_some_and(|role| {
                            role.split_whitespace()
                                .any(|value| value.eq_ignore_ascii_case("main"))
                        });
                let (article_peer_count, article_peer_score) = if is_main {
                    context.article_peer_summary(candidate.node)
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
                let generic_boundary_bonus = if candidate.node
                    != dom.body().unwrap_or(candidate.node)
                    && is_generic_only
                    && has_distinct_structural_content
                {
                    0.01
                } else {
                    0.0
                };
                let final_score = candidate.features.ranking_score()
                    + short_semantic_bonus
                    + sibling_content_bonus
                    + generic_boundary_bonus;
                self.node_data.set_score(candidate.node, final_score);
                Some(RankedCandidate {
                    node: candidate.node,
                    score: final_score,
                    order,
                })
            })
            .collect();
        let top_count = self.options.top_candidates.min(scored.len());
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
        scored
    }

    fn gather_siblings(
        dom: &Dom,
        top: NodeId,
        store: &mut NodeStateStore,
        debug: bool,
    ) -> SmallVec<[NodeId; 8]> {
        let Some(parent) = dom.parent(top) else {
            let mut out = SmallVec::new();
            out.push(top);
            return out;
        };
        let threshold = 10f64.max(store.get_content_score(top) * 0.2);
        let class = dom.attr(top, AttrName::Class);
        let mut out = SmallVec::<[NodeId; 8]>::new();
        for x in dom.element_children(parent) {
            let mut yes = x == top;
            if !yes {
                let bonus = if class.is_some() && dom.attr(x, AttrName::Class) == class {
                    store.get_content_score(top) * 0.2
                } else {
                    0.
                };
                if store.has(x) && store.get_content_score(x) + bonus >= threshold {
                    yes = true
                }
                if !yes && dom.tag(x) == Some(Tag::P) {
                    let s = get_or_compute_stats(dom, x, store);
                    let d = get_link_density_cached(dom, x, s.text_length, store);
                    yes = (s.text_length > 80 && d < 0.25)
                        || (s.text_length < 80
                            && s.text_length > 0
                            && d == 0.0
                            && s.has_sentence_end)
                }
                if !yes
                    && is_near_preceding_sibling(dom, x, top)
                    && matches!(dom.tag(x), Some(Tag::H2 | Tag::H3 | Tag::H4))
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
                    let stats = get_or_compute_stats(dom, x, store);
                    yes = (30..=400).contains(&(stats.text_length as usize))
                        && get_link_density_cached(dom, x, stats.text_length, store) == 0.0;
                }
            }
            if yes {
                debug_log!(@bool debug,"Appending sibling node: {:?}",x);
                out.push(x)
            }
        }
        out
    }
    fn create_container(&mut self, _top: NodeId, siblings: &[NodeId]) -> Option<NodeId> {
        let first = *siblings.first()?;
        let container = self.dom.create_html_element(Tag::Div).ok()?;
        self.dom.insert_before(first, container);

        // A synthetic content boundary must not break the HTML table content
        // model. Keep one common wrapper when the selected siblings are rows,
        // cells, or table sections. This also prevents a row from being
        // renamed to a div while it still contains cells.
        let tags: SmallVec<[Tag; 8]> = siblings
            .iter()
            .filter_map(|&node| self.dom.tag(node))
            .collect();
        let table_parent = if tags.len() == siblings.len() && tags.iter().all(|tag| *tag == Tag::Tr)
        {
            let table = self.dom.create_html_element(Tag::Table).ok()?;
            let body = self.dom.create_html_element(Tag::Tbody).ok()?;
            self.dom.append_child(container, table);
            self.dom.append_child(table, body);
            Some(body)
        } else if tags.len() == siblings.len()
            && tags.iter().all(|tag| matches!(tag, Tag::Td | Tag::Th))
        {
            let table = self.dom.create_html_element(Tag::Table).ok()?;
            let body = self.dom.create_html_element(Tag::Tbody).ok()?;
            let row = self.dom.create_html_element(Tag::Tr).ok()?;
            self.dom.append_child(container, table);
            self.dom.append_child(table, body);
            self.dom.append_child(body, row);
            Some(row)
        } else if tags.len() == siblings.len()
            && tags.iter().all(|tag| {
                matches!(
                    tag,
                    Tag::Caption | Tag::Colgroup | Tag::Tbody | Tag::Tfoot | Tag::Thead
                )
            })
        {
            let table = self.dom.create_html_element(Tag::Table).ok()?;
            self.dom.append_child(container, table);
            Some(table)
        } else if tags.len() == siblings.len() && tags.iter().all(|tag| *tag == Tag::Col) {
            let table = self.dom.create_html_element(Tag::Table).ok()?;
            let group = self.dom.create_html_element(Tag::Colgroup).ok()?;
            self.dom.append_child(container, table);
            self.dom.append_child(table, group);
            Some(group)
        } else {
            None
        };

        for &node in siblings {
            if let Some(parent) = table_parent {
                self.dom.append_child(parent, node);
                continue;
            }
            if let Some(tag) = self.dom.tag(node) {
                let wrapper = match tag {
                    Tag::Tr => {
                        let table = self.dom.create_html_element(Tag::Table).ok()?;
                        let body = self.dom.create_html_element(Tag::Tbody).ok()?;
                        self.dom.append_child(container, table);
                        self.dom.append_child(table, body);
                        Some(body)
                    }
                    Tag::Td | Tag::Th => {
                        let table = self.dom.create_html_element(Tag::Table).ok()?;
                        let body = self.dom.create_html_element(Tag::Tbody).ok()?;
                        let row = self.dom.create_html_element(Tag::Tr).ok()?;
                        self.dom.append_child(container, table);
                        self.dom.append_child(table, body);
                        self.dom.append_child(body, row);
                        Some(row)
                    }
                    Tag::Caption | Tag::Colgroup | Tag::Tbody | Tag::Tfoot | Tag::Thead => {
                        let table = self.dom.create_html_element(Tag::Table).ok()?;
                        self.dom.append_child(container, table);
                        Some(table)
                    }
                    Tag::Col => {
                        let table = self.dom.create_html_element(Tag::Table).ok()?;
                        let group = self.dom.create_html_element(Tag::Colgroup).ok()?;
                        self.dom.append_child(container, table);
                        self.dom.append_child(table, group);
                        Some(group)
                    }
                    _ => None,
                };
                if let Some(wrapper) = wrapper {
                    self.dom.append_child(wrapper, node);
                    continue;
                }
                if !is_alter_to_div_exception(tag) && tag != Tag::Table {
                    self.dom.rename_html(node, Tag::Div)
                }
            }
            self.dom.append_child(container, node)
        }
        Some(container)
    }
    fn prep_article(
        &mut self,
        root: NodeId,
        video: &Regex,
        _match_buffer: &mut String,
        text_buffer: &mut String,
        nodes: &mut Vec<NodeId>,
    ) {
        // Cleanup mutates only the compact selected fragment. Hard cleanup
        // removes executable and interactive markup. Heuristic cleanup needs
        // several agreeing clutter signals before it removes a subtree.
        preserve_semantics_before_cleanup(&mut self.dom, root);
        let before = self.diagnostic_element_count(root);
        remove_decorative_media_before_cleanup(&mut self.dom, root);
        self.record_cleanup_delta(CleanupActionKind::DecorativeMedia, before, root);
        clean_styles(&mut self.dom, root, nodes);
        let before = self.diagnostic_element_count(root);
        hard_cleanup(
            &mut self.dom,
            root,
            video,
            self.strategy == ExtractionStrategy::RelaxedVisibility,
            nodes,
        );
        self.record_cleanup_delta(CleanupActionKind::HardCleanup, before, root);
        if self.page_kind.uses_article_cleanup() {
            if self.strategy.conditional_cleanup() {
                let before = self.diagnostic_element_count(root);
                heuristic_cleanup(
                    &mut self.dom,
                    root,
                    self.page_kind,
                    &mut self.node_data,
                    text_buffer,
                    nodes,
                );
                self.record_cleanup_delta(CleanupActionKind::HeuristicCleanup, before, root);
            }
            // Global chrome is high-confidence cleanup. Apply it to every
            // extraction strategy, including broad and fallback attempts.
            let before = self.diagnostic_element_count(root);
            remove_global_chrome(&mut self.dom, root, &mut self.node_data);
            self.record_cleanup_delta(CleanupActionKind::HeuristicCleanup, before, root);
        }

        // Normalization is separate from relevance cleanup. Serializers receive
        // stable code, figure, image, footnote, and table structures.
        let normalization = normalize_after_cleanup(&mut self.dom, root, nodes);
        if self.diagnostic_attempts.is_some() {
            self.diagnostic_normalization.flattened_layout_tables =
                normalization.flattened_layout_tables;
        }

        // Single traversal collects both paragraphs and line breaks,
        // replacing two separate filters over `descendants`.
        let mut paragraphs = SmallVec::<[NodeId; 64]>::new();
        let mut breaks = SmallVec::<[NodeId; 32]>::new();
        for id in self.dom.descendants(root) {
            match self.dom.tag(id) {
                Some(Tag::P) => paragraphs.push(id),
                Some(Tag::Br) => breaks.push(id),
                _ => {}
            }
        }
        for paragraph in paragraphs {
            let media = self.dom.descendants(paragraph).any(|node| {
                matches!(
                    self.dom.tag(node),
                    Some(Tag::Img | Tag::Embed | Tag::Object | Tag::Iframe)
                ) || self.dom.attr(node, AttrName::DataMath).is_some()
            });
            if !media && !has_non_empty_inner_text(&self.dom, paragraph) {
                self.dom.detach(paragraph);
            }
        }
        for line_break in breaks {
            if crate::cleaning::next_non_whitespace_sibling(&self.dom, line_break)
                .is_some_and(|node| self.dom.tag(node) == Some(Tag::P))
            {
                self.dom.detach(line_break);
            }
        }
    }
    fn content_excerpt(&self, root: NodeId) -> Option<String> {
        let mut buffer = String::new();
        self.dom
            .descendants(root)
            .filter(|&node| self.dom.tag(node) == Some(Tag::P))
            .filter(|&node| {
                !self
                    .dom
                    .ancestors(node)
                    .take_while(|&ancestor| ancestor != root)
                    .any(|ancestor| {
                        matches!(self.dom.tag(ancestor), Some(Tag::Aside | Tag::Nav))
                            || [AttrName::Class, AttrName::Id]
                                .into_iter()
                                .filter_map(|name| self.dom.attr(ancestor, name))
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
                self.dom.append_text(node, &mut buffer);
                let trimmed = buffer.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed.to_owned())
                }
            })
    }
    fn post_process(&mut self, root: NodeId, nodes: &mut Vec<NodeId>) {
        // URI repair, class cleanup, and comment removal share one stable
        // preorder snapshot. The snapshot also permits structural link changes.
        nodes.clear();
        nodes.extend(self.dom.descendants(root));
        let mut class_buffer = String::new();
        for &id in nodes.iter() {
            if self.dom.parent(id).is_none() {
                continue;
            }
            if self.dom.is_comment(id) {
                self.dom.detach(id);
                continue;
            }
            let Some(tag) = self.dom.tag(id) else {
                continue;
            };

            if let Some(base) = self.base_uri.as_ref() {
                if tag == Tag::A {
                    if let Some(href) = self.dom.attr(id, AttrName::Href) {
                        if href.starts_with('#') && !self.resolve_fragment_links {
                            // Fragment links do not need URI resolution when the
                            // document does not override its base URL.
                        } else if href.starts_with("javascript:") {
                            let replacement = if self.dom.first_child(id) == self.dom.last_child(id)
                                && self
                                    .dom
                                    .first_child(id)
                                    .is_some_and(|child| self.dom.is_text(child))
                            {
                                self.dom.create_text(&self.dom.text(id))
                            } else {
                                self.dom.create_html_element(Tag::Span).inspect(|&span| {
                                    self.dom.move_children(id, span);
                                })
                            };
                            if let Ok(replacement) = replacement {
                                self.dom.replace_with(id, replacement)
                            }
                        } else if let Ok(url) = base.join(href) {
                            self.dom.set_attr(id, AttrName::Href, url.as_str())
                        }
                    }
                } else if matches!(
                    tag,
                    Tag::Img | Tag::Picture | Tag::Figure | Tag::Video | Tag::Audio | Tag::Source
                ) {
                    for attr in [AttrName::Src, AttrName::Poster] {
                        if let Some(value) = self.dom.attr(id, attr)
                            && let Ok(url) = base.join(value)
                        {
                            self.dom.set_attr(id, attr, url.as_str())
                        }
                    }
                    if let Some(value) = self.dom.attr(id, AttrName::Srcset) {
                        let replacement =
                            regexps::SRCSET_URL.replace_all(value, |captures: &regex::Captures| {
                                let url = base
                                    .join(&captures[1])
                                    .map(|url| url.to_string())
                                    .unwrap_or_else(|_| captures[1].into());
                                format!(
                                    "{}{}{}",
                                    url,
                                    captures.get(2).map_or("", |value| value.as_str()),
                                    captures.get(3).map_or("", |value| value.as_str())
                                )
                            });
                        if let std::borrow::Cow::Owned(replacement) = replacement {
                            self.dom
                                .set_attr(id, AttrName::Srcset, replacement.as_str())
                        }
                    }
                }
            }

            if !self.options.keep_classes
                && let Some(classes) = self.dom.attr(id, AttrName::Class)
            {
                class_buffer.clear();
                for class in classes.split_whitespace().filter(|class| {
                    self.options
                        .classes_to_preserve
                        .iter()
                        .any(|preserved| preserved == class)
                }) {
                    if !class_buffer.is_empty() {
                        class_buffer.push(' ')
                    }
                    class_buffer.push_str(class)
                }
                if class_buffer.is_empty() {
                    self.dom.remove_attr(id, AttrName::Class)
                } else if class_buffer != classes {
                    self.dom
                        .set_attr(id, AttrName::Class, class_buffer.as_str())
                }
            }
        }
        let before = self.diagnostic_element_count(root);
        finish_normalization(&mut self.dom, root, nodes);
        self.record_cleanup_delta(CleanupActionKind::FinalCleanup, before, root);
    }

    fn diagnostic_element_count(&self, root: NodeId) -> Option<usize> {
        self.diagnostic_attempts.as_ref()?;
        Some(
            self.dom
                .descendants(root)
                .filter(|&node| self.dom.is_element(node))
                .count(),
        )
    }

    fn record_cleanup_delta(
        &mut self,
        kind: CleanupActionKind,
        before: Option<usize>,
        root: NodeId,
    ) {
        let Some(before) = before else {
            return;
        };
        let removed_elements = before.saturating_sub(
            self.dom
                .descendants(root)
                .filter(|&node| self.dom.is_element(node))
                .count(),
        );
        if removed_elements > 0 {
            self.diagnostic_cleanup_actions.push(CleanupActionInfo {
                kind,
                removed_elements,
            });
        }
    }

    fn capture_normalization_counts(&mut self, root: NodeId) {
        if self.diagnostic_attempts.is_none() {
            return;
        }
        let flattened_layout_tables = self.diagnostic_normalization.flattened_layout_tables;
        let mut counts = NormalizationCountsInfo {
            flattened_layout_tables,
            ..NormalizationCountsInfo::default()
        };
        for node in self.dom.descendants(root) {
            match self.dom.tag(node) {
                Some(Tag::Pre)
                    if self
                        .dom
                        .element_children(node)
                        .any(|child| self.dom.tag(child) == Some(Tag::Code)) =>
                {
                    counts.code_blocks += 1;
                }
                Some(Tag::Img) => counts.images += 1,
                Some(Tag::Table) => counts.tables += 1,
                _ => {}
            }
            counts.footnote_references +=
                usize::from(self.dom.attr(node, AttrName::DataFootnoteRef).is_some());
            counts.footnote_definitions +=
                usize::from(self.dom.attr(node, AttrName::DataFootnote).is_some());
            counts.math_expressions +=
                usize::from(self.dom.attr(node, AttrName::DataMath).is_some());
        }
        self.diagnostic_normalization = counts;
    }

    fn restore_source(&mut self, source: Dom) {
        self.dom = source;
        self.page_byline = None;
        self.page_direction = None;
        self.page_language = None;
        self.node_data.clear();
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
    page_title: &str,
    structured_title: &str,
    site_name: Option<&str>,
    source_uri: Option<&Url>,
) -> TitleHeadingPlan {
    let root = dom.root();
    let snapshot = dom.element_descendants_snapshot_with_depth(root);

    // A heading can be deeply nested in repaired HTML. Compute the score of
    // each node's nearest marked ancestor path once instead of walking that
    // path again for every matching heading. Keep the cheaper direct walk for
    // ordinary documents.
    let deeply_nested = snapshot.iter().any(|&(_, depth)| depth > 64);
    let context_scores = deeply_nested.then(|| {
        let mut scores = vec![0_i32; dom.len()];
        let root_score = title_heading_context_score(dom, root, 0);
        for &(node, _) in &snapshot {
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

    let headings: Vec<_> = snapshot
        .iter()
        .map(|&(node, _)| node)
        .filter(|&node| has_primary_heading_semantics(dom, node))
        .filter(|&node| is_probably_visible(dom, node))
        .collect();
    let has_link = dom
        .descendants(root)
        .any(|node| dom.tag(node) == Some(Tag::A));
    let brand_headings: SmallVec<[NodeId; 2]> = if has_link {
        // Resolve linked descendants and ancestors in two linear passes for a
        // deep tree. A descendant scan per heading becomes quadratic after
        // HTML repair nests many unclosed headings. Keep direct scans for
        // ordinary shallow documents to avoid two dense temporary arrays.
        let linked_contexts = deeply_nested.then(|| {
            let mut descendant_links = vec![None; dom.len()];
            for &(node, _) in snapshot.iter().rev() {
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
            for &(node, _) in &snapshot {
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

#[cfg(test)]
mod tests {
    use super::*;

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
        let mut match_buffer = String::new();
        let mut text_buffer = String::new();
        let discovery = readability.discover_candidates(&mut match_buffer, &mut text_buffer);
        let mut scoring_dom = readability.dom.clone();
        let mut to_score = discovery.to_score;
        let prepared = prepare_readability_structure(
            &mut scoring_dom,
            &discovery.divs_to_prepare,
            &discovery.candidates,
        );
        readability.node_data.sync_len(scoring_dom.len());
        for node in prepared {
            if readability.node_data.mark_score_seen(node) {
                to_score.push(node);
            }
        }
        let excluded_mask = build_exclusion_mask(&scoring_dom, &discovery.remove_after_scoring);
        let scores = compute_readability_scores(
            &mut scoring_dom,
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
        let container = extractor.create_container(row, &[row]).unwrap();

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
            r#"<body><main><header><h1>Streamed article</h1><p>{visible_detail}</p></header><div hidden id="S:0"><p>{hidden_detail}</p><h2>Implementation</h2><p>The second streamed paragraph gives the final implementation details and conclusion.</p></div></main></body>"#
        );
        let page = crate::Extractor::builder()
            .diagnostics(true)
            .build()
            .extract(&html, None)
            .unwrap();

        assert!(page.text().contains("streamed article explains"));
        assert!(page.text().contains("Implementation"));
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
        let plan = title_heading_plan(
            &dom,
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
        let mut match_buffer = String::new();
        let mut text_buffer = String::new();
        let discovery = readability.discover_candidates(&mut match_buffer, &mut text_buffer);
        assert!(!discovery.candidates.is_semantic(content));
        let mut scoring_dom = readability.dom.clone();
        let mut to_score = discovery.to_score;
        let prepared = prepare_readability_structure(
            &mut scoring_dom,
            &discovery.divs_to_prepare,
            &discovery.candidates,
        );
        readability.node_data.sync_len(scoring_dom.len());
        for node in prepared {
            if readability.node_data.mark_score_seen(node) {
                to_score.push(node);
            }
        }
        let excluded_mask = build_exclusion_mask(&scoring_dom, &discovery.remove_after_scoring);
        let scores = compute_readability_scores(
            &mut scoring_dom,
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
        let mut match_buffer = String::new();
        let mut text_buffer = String::new();
        let discovery = readability.discover_candidates(&mut match_buffer, &mut text_buffer);
        let mut scoring_dom = readability.dom.clone();
        let mut to_score = discovery.to_score;
        let prepared = prepare_readability_structure(
            &mut scoring_dom,
            &discovery.divs_to_prepare,
            &discovery.candidates,
        );
        readability.node_data.sync_len(scoring_dom.len());
        for node in prepared {
            if readability.node_data.mark_score_seen(node) {
                to_score.push(node);
            }
        }
        let excluded_mask = build_exclusion_mask(&scoring_dom, &discovery.remove_after_scoring);
        let scores = compute_readability_scores(
            &mut scoring_dom,
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
        let mut match_buffer = String::new();
        let mut text_buffer = String::new();

        let discovery = readability.discover_candidates(&mut match_buffer, &mut text_buffer);
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
        readability.node_data.sync_len(scoring_dom.len());
        for id in prepared {
            if readability.node_data.mark_score_seen(id) {
                to_score.push(id)
            }
        }
        let excluded_mask = build_exclusion_mask(&scoring_dom, &discovery.remove_after_scoring);
        let scores = compute_readability_scores(
            &mut scoring_dom,
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
