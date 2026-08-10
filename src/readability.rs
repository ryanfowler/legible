//! Readability-derived content extraction.
#![allow(clippy::collapsible_if)]
use crate::candidate::{CandidateSet, CandidateSource};
use crate::cleaning::*;
use crate::constants::{
    flags::*, has_share_element, is_alter_to_div_exception, is_default_tag_to_score,
    is_unlikely_role, regexps,
};
use crate::dom::{AttrName, Dom, NodeId, NodeStateStore, Tag, build_match_string};
use crate::error::{Error, Result};
use crate::extractor::ExtractorConfig;
use crate::logging::debug_log;
use crate::metadata::{self, Metadata, StructuredData};
use crate::page::ExtractedPage;
use crate::scoring::*;
use html5ever::ns;
use regex::Regex;
use smallvec::SmallVec;
use url::Url;

pub(crate) struct Readability<'a> {
    dom: Dom,
    original_html: &'a str,
    options: &'a ExtractorConfig,
    flags: u32,
    node_data: NodeStateStore,
    article_title: String,
    article_byline: Option<String>,
    article_dir: Option<String>,
    article_lang: Option<String>,
    metadata: Metadata,
    structured_data: StructuredData,
    source_uri: Option<Url>,
    base_uri: Option<Url>,
    resolve_fragment_links: bool,
    url_error: Option<url::ParseError>,
    best_attempt: Option<BestAttempt>,
}
struct BestAttempt {
    content: FrozenContent,
    text_len_chars: usize,
    excerpt: Option<String>,
}
struct FrozenContent {
    dom: Dom,
}
struct ArticleContent {
    text_length: usize,
    excerpt: Option<String>,
    /// The node whose children form the output fragment.
    article_root: NodeId,
}

struct CandidateDiscovery {
    candidates: CandidateSet,
    to_score: SmallVec<[NodeId; 256]>,
    divs_to_prepare: SmallVec<[NodeId; 128]>,
    remove_after_scoring: SmallVec<[NodeId; 64]>,
}

impl<'a> Readability<'a> {
    pub(crate) fn from_document(
        dom: Dom,
        original_html: &'a str,
        url: Option<&str>,
        options: &'a ExtractorConfig,
    ) -> Self {
        let (base_uri, url_error) = match url {
            Some(x) => match Url::parse(x) {
                Ok(u) => (Some(u), None),
                Err(e) => (None, Some(e)),
            },
            None => (None, None),
        };
        Self {
            dom,
            original_html,
            options,
            flags: FLAG_STRIP_UNLIKELYS | FLAG_WEIGHT_CLASSES | FLAG_CLEAN_CONDITIONALLY,
            node_data: NodeStateStore::new(),
            article_title: String::new(),
            article_byline: None,
            article_dir: None,
            article_lang: None,
            metadata: Metadata::default(),
            structured_data: StructuredData::default(),
            source_uri: base_uri.clone(),
            base_uri,
            resolve_fragment_links: false,
            url_error,
            best_attempt: None,
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
        let title = metadata::get_article_title(&self.dom);
        if self.options.structured_data {
            self.structured_data = StructuredData::parse(&self.dom);
        }
        self.metadata = metadata::discover(
            &self.dom,
            &self.structured_data,
            &title,
            self.base_uri.as_ref(),
            self.source_uri.as_ref(),
        );
        if self.structured_data.article_texts().next().is_some() {
            debug_log!(self, "Structured data contains a content-location hint");
        }
        unwrap_noscript_images(&mut self.dom);
        prep_document(&mut self.dom);
        self.article_title = self.metadata.title.take().unwrap_or(title);
        let content = self.grab_article()?;
        self.metadata.title = (!self.article_title.is_empty()).then_some(self.article_title);
        if self.metadata.authors.is_empty()
            && let Some(byline) = self.article_byline
        {
            self.metadata.authors.push(byline);
        }
        self.metadata.description = self.metadata.description.or(content.excerpt);
        self.metadata.direction = self.metadata.direction.or(self.article_dir);
        self.metadata.language = self.metadata.language.or(self.article_lang);
        let extracted_dom = self
            .dom
            .copy_children_as_fragment(content.article_root)
            .map_err(|_| Error::NoContent)?;
        let extracted_root = extracted_dom.root();
        Ok(ExtractedPage::new(
            extracted_dom,
            extracted_root,
            self.metadata,
            content.text_length,
        ))
    }
    fn grab_article(&mut self) -> Result<ArticleContent> {
        if self.dom.body().is_none() {
            return Err(Error::NoBody);
        }
        let mut match_buffer = String::new();
        let mut text_buffer = String::new();
        let mut cleaning_nodes = Vec::new();
        loop {
            // Discovery only records nodes. It does not mutate the source DOM.
            // Deferred removals stay attached until all candidate scores and
            // link-density values have been calculated.
            let discovery = self.discover_candidates(&mut match_buffer, &mut text_buffer);

            // Score a prepared working copy. The source tree stays intact until
            // discovery, score propagation, and final ranking are complete.
            let mut scoring_dom = self.dom.clone();
            let mut to_score = discovery.to_score;
            let prepared = Self::prepare_scoring_structure(
                &mut scoring_dom,
                &discovery.divs_to_prepare,
                &discovery.candidates,
            );
            self.node_data.sync_len(scoring_dom.len());
            for id in prepared {
                if self.node_data.mark_score_seen(id) {
                    to_score.push(id)
                }
            }
            let readability_candidates = self.propagate_candidate_scores(&scoring_dom, to_score);
            let top = self.rank_candidates(
                &scoring_dom,
                discovery.candidates,
                readability_candidates,
                &discovery.remove_after_scoring,
            );

            // Apply the already-planned preparation and cleanup only after the
            // non-destructive discovery and scoring phase has selected a root.
            let semantic_candidates = CandidateSet::discover_semantic(&self.dom);
            Self::prepare_scoring_structure(
                &mut self.dom,
                &discovery.divs_to_prepare,
                &semantic_candidates,
            );
            for id in discovery.remove_after_scoring {
                if self.dom.parent(id).is_some() {
                    self.dom.detach(id)
                }
            }
            let body = self.dom.body().ok_or(Error::NoBody)?;
            let (top_id, synthetic) = if top.is_empty() || top[0].0 == body {
                let c = self
                    .dom
                    .create_html_element(Tag::Div)
                    .map_err(|_| Error::NoContent)?;
                let children: SmallVec<[NodeId; 16]> = self.dom.children(body).collect();
                for x in children {
                    self.dom.append_child(c, x)
                }
                self.dom.append_child(body, c);
                initialize_node(&self.dom, c, &mut self.node_data, self.flags);
                (c, true)
            } else {
                let mut tc = top[0].0;
                let top_score = top[0].1;
                let alternatives: SmallVec<[SmallVec<[NodeId; 16]>; 3]> = top
                    .iter()
                    .skip(1)
                    .filter(|(_, score, _)| *score / top_score >= 0.75)
                    .map(|(id, _, _)| self.dom.ancestors(*id).collect())
                    .collect();
                if alternatives.len() >= 3 {
                    let mut p = self.dom.parent(tc);
                    while let Some(x) = p {
                        if x == body {
                            break;
                        }
                        if alternatives.iter().filter(|a| a.contains(&x)).count() >= 3 {
                            tc = x;
                            break;
                        }
                        p = self.dom.parent(x)
                    }
                }
                if !self.node_data.has(tc) {
                    initialize_node(&self.dom, tc, &mut self.node_data, self.flags)
                }
                let mut p = self.dom.parent(tc);
                let threshold = self.node_data.get_content_score(tc) / 3.;
                let mut last = self.node_data.get_content_score(tc);
                while let Some(x) = p {
                    if x == body {
                        break;
                    }
                    if let Some(s) = self.node_data.get(x).map(|e| e.content_score) {
                        if s < threshold {
                            break;
                        }
                        if s > last {
                            tc = x;
                            break;
                        }
                        last = s
                    }
                    p = self.dom.parent(x)
                }
                while let Some(p) = self.dom.parent(tc) {
                    if p == body {
                        break;
                    }
                    let mut ec = self.dom.element_children(p);
                    if ec.next().is_some() && ec.next().is_none() {
                        tc = p
                    } else {
                        break;
                    }
                }
                (tc, false)
            };
            let article_id = if synthetic {
                top_id
            } else {
                let sib = Self::gather_siblings(
                    &self.dom,
                    top_id,
                    &mut self.node_data,
                    self.options.debug,
                );
                self.create_container(top_id, &sib).unwrap_or(top_id)
            };
            let video = regexps::VIDEOS.clone();
            self.prep_article(
                article_id,
                &video,
                &mut match_buffer,
                &mut text_buffer,
                &mut cleaning_nodes,
            );
            if synthetic {
                self.dom
                    .set_attr(article_id, AttrName::Id, "readability-page-1");
                self.dom.set_attr(article_id, AttrName::Class, "page")
            } else {
                let w = self
                    .dom
                    .create_html_element(Tag::Div)
                    .map_err(|_| Error::NoContent)?;
                self.dom.set_attr(w, AttrName::Id, "readability-page-1");
                self.dom.set_attr(w, AttrName::Class, "page");
                let children: SmallVec<[NodeId; 16]> = self.dom.children(article_id).collect();
                for x in children {
                    self.dom.append_child(w, x)
                }
                self.dom.append_child(article_id, w)
            }
            if let Some(len) = self
                .dom
                .normalized_char_count_below(article_id, self.options.char_threshold)
            {
                if self
                    .best_attempt
                    .as_ref()
                    .is_none_or(|best| len > best.text_len_chars)
                {
                    let excerpt = self.article_excerpt(article_id);
                    self.post_process(article_id, &mut cleaning_nodes);
                    let dom = if synthetic {
                        self.dom.copy_subtree_as_fragment(article_id)
                    } else {
                        self.dom.copy_children_as_fragment(article_id)
                    }
                    .map_err(|_| Error::NoContent)?;
                    self.best_attempt = Some(BestAttempt {
                        content: FrozenContent { dom },
                        text_len_chars: len,
                        excerpt,
                    });
                }
                let retry = if self.flags & FLAG_STRIP_UNLIKELYS != 0 {
                    self.flags &= !FLAG_STRIP_UNLIKELYS;
                    true
                } else if self.flags & FLAG_WEIGHT_CLASSES != 0 {
                    self.flags &= !FLAG_WEIGHT_CLASSES;
                    true
                } else if self.flags & FLAG_CLEAN_CONDITIONALLY != 0 {
                    self.flags &= !FLAG_CLEAN_CONDITIONALLY;
                    true
                } else {
                    false
                };
                if retry {
                    self.reparse_prepare()?;
                    continue;
                }
                let best = self.best_attempt.take().ok_or(Error::NoContent)?;
                if best.text_len_chars == 0 {
                    return Err(Error::NoContent);
                }
                self.dom = best.content.dom;
                let root = self.dom.root();
                return Ok(ArticleContent {
                    text_length: best.text_len_chars,
                    excerpt: best.excerpt,
                    article_root: root,
                });
            }
            let mut p = Some(top_id);
            while let Some(x) = p {
                if let Some(d) = self.dom.attr(x, AttrName::Dir) {
                    self.article_dir = Some(d.into());
                    break;
                }
                p = self.dom.parent(x)
            }
            let excerpt = self.article_excerpt(article_id);
            self.post_process(article_id, &mut cleaning_nodes);
            let len = self.dom.normalized_char_count(article_id);
            return Ok(ArticleContent {
                text_length: len,
                excerpt,
                article_root: if synthetic {
                    self.dom.parent(article_id).unwrap_or(article_id)
                } else {
                    article_id
                },
            });
        }
    }
    fn discover_candidates(
        &mut self,
        match_buffer: &mut String,
        text_buffer: &mut String,
    ) -> CandidateDiscovery {
        let strip = self.flags & FLAG_STRIP_UNLIKELYS != 0;
        let candidates = CandidateSet::discover_semantic(&self.dom);
        let mut to_score = SmallVec::<[NodeId; 256]>::new();
        let mut divs_to_prepare = SmallVec::<[NodeId; 128]>::new();
        let mut remove_after_scoring = SmallVec::<[NodeId; 64]>::new();
        if let Some(html) = self.dom.html_element() {
            if let Some(lang) = self.dom.attr(html, AttrName::Lang) {
                self.article_lang = Some(lang.into())
            }
            if let Some(dir) = self.dom.attr(html, AttrName::Dir) {
                self.article_dir = Some(dir.into())
            }
        }
        self.node_data.sync_len(self.dom.len());
        let initial_nodes = self
            .dom
            .element_descendants_snapshot_with_depth(self.dom.root());
        let mut excluded_depth = None;
        let mut remove_title = true;
        for (id, depth) in initial_nodes {
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
            if !is_probably_visible(&self.dom, id)
                || self.dom.attr(id, AttrName::AriaModal) == Some("true")
                    && self.dom.attr(id, AttrName::Role) == Some("dialog")
            {
                remove_after_scoring.push(id);
                excluded_depth = Some(depth);
                continue;
            }
            if self.article_byline.is_none() && !self.metadata.has_source_author {
                build_match_string(&self.dom, id, match_buffer);
                if is_valid_byline(&self.dom, id, match_buffer, text_buffer) {
                    let mut names = Vec::new();
                    self.dom
                        .collect_attr_contains(id, AttrName::ItemProp, "name", &mut names);
                    let name = names.first().copied().unwrap_or(id);
                    self.article_byline =
                        Some(get_inner_text(&self.dom, name, text_buffer).to_owned());
                    remove_after_scoring.push(id);
                    excluded_depth = Some(depth);
                    continue;
                }
            }
            let duplicates_title = if remove_title && matches!(tag, Tag::H1 | Tag::H2) {
                let heading = get_inner_text(&self.dom, id, text_buffer);
                heading_matches_article_title(&self.article_title, heading)
            } else {
                false
            };
            if duplicates_title {
                remove_title = false;
                remove_after_scoring.push(id);
                excluded_depth = Some(depth);
                continue;
            }
            if strip && tag != Tag::Body && tag != Tag::A {
                build_match_string(&self.dom, id, match_buffer);
                let matches = regexps::CANDIDATE_FILTER_SET.matches(match_buffer);
                if matches.matched(0)
                    && !matches.matched(1)
                    && !has_ancestor_tags_any(&self.dom, id, &[Tag::Table, Tag::Code], 3)
                    || self
                        .dom
                        .attr(id, AttrName::Role)
                        .is_some_and(is_unlikely_role)
                {
                    remove_after_scoring.push(id);
                    excluded_depth = Some(depth);
                    continue;
                }
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

    fn prepare_scoring_structure(
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
            } else if has_single_tag_inside_element(dom, id, Tag::P)
                && get_link_density(dom, id) < 0.25
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

    fn propagate_candidate_scores(
        &mut self,
        dom: &Dom,
        to_score: SmallVec<[NodeId; 256]>,
    ) -> SmallVec<[NodeId; 256]> {
        let mut candidates = SmallVec::<[NodeId; 256]>::new();
        for id in to_score {
            let Some(parent) = dom.parent(id).filter(|&x| dom.is_element(x)) else {
                continue;
            };
            let stats = get_or_compute_stats(dom, id, &mut self.node_data);
            if stats.text_length < 25 {
                continue;
            }
            let content_score =
                2.0 + f64::from(stats.comma_count) + f64::from((stats.text_length / 100).min(3));
            let mut ancestor = Some(parent);
            for level in 0..5 {
                let Some(id) = ancestor else { break };
                ancestor = dom.parent(id);
                if !dom.is_element(id) || !ancestor.is_some_and(|parent| dom.is_element(parent)) {
                    continue;
                }
                if Self::initialize_node_once(dom, id, &mut self.node_data, self.flags) {
                    candidates.push(id)
                }
                let divisor = if level == 0 {
                    1.0
                } else if level == 1 {
                    2.0
                } else {
                    (level * 3) as f64
                };
                self.node_data
                    .add_content_score(id, content_score / divisor)
            }
        }
        candidates
    }

    fn rank_candidates(
        &mut self,
        dom: &Dom,
        mut candidates: CandidateSet,
        readability_candidates: SmallVec<[NodeId; 256]>,
        excluded: &[NodeId],
    ) -> SmallVec<[(NodeId, f64, usize); 64]> {
        // Keep the scoring DOM intact through ranking. Calculate final density
        // against a cleanup view so deferred clutter does not change the legacy
        // ranking behavior.
        let mut ranking_dom = dom.clone();
        for &id in excluded {
            if ranking_dom.parent(id).is_some() {
                ranking_dom.detach(id)
            }
        }
        let dom = &ranking_dom;
        self.node_data.clear_stats();

        for id in readability_candidates {
            if Self::is_excluded_candidate(dom, id, excluded) {
                continue;
            }
            let score = self.node_data.get_content_score(id);
            let length = get_or_compute_stats(dom, id, &mut self.node_data).text_length;
            let density = get_link_density_cached(dom, id, length, &mut self.node_data);
            candidates.add_readability(id, score * (1.0 - density));
        }

        let context = candidates.ranking_context(dom);
        let mut scored: SmallVec<[(NodeId, f64, usize); 64]> = candidates
            .iter()
            .enumerate()
            .filter_map(|(order, candidate)| {
                if Self::is_excluded_candidate(dom, candidate.node, excluded) {
                    return None;
                }
                let length =
                    get_or_compute_stats(dom, candidate.node, &mut self.node_data).text_length;
                if length == 0 && !candidate.has_source(CandidateSource::Generic) {
                    return None;
                }
                let is_semantic = candidate.has_source(CandidateSource::Semantic);
                let is_authoritative = candidates.is_authoritative_semantic(dom, candidate.node);
                let has_readability = context.has_readability(candidate.node);
                if is_semantic && !is_authoritative && !has_readability {
                    return None;
                }
                let semantic_content_score = if is_semantic {
                    (f64::from(length) / 1_000_000.0).min(0.001)
                } else {
                    0.0
                };
                let short_semantic_bonus = if is_authoritative
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
                let final_score = candidate.score
                    + candidate.semantic_prior
                    + semantic_content_score
                    + short_semantic_bonus
                    + sibling_content_bonus;
                self.node_data.set_score(candidate.node, final_score);
                Some((candidate.node, final_score, order))
            })
            .collect();
        let top_count = self.options.top_candidates.min(scored.len());
        if top_count < scored.len() {
            scored.select_nth_unstable_by(top_count, |a, b| {
                b.1.partial_cmp(&a.1)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| a.2.cmp(&b.2))
            });
            scored.truncate(top_count);
        }
        scored.sort_unstable_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.2.cmp(&b.2))
        });
        scored
    }

    fn is_excluded_candidate(dom: &Dom, id: NodeId, excluded: &[NodeId]) -> bool {
        excluded.contains(&id)
            || dom
                .ancestors(id)
                .any(|ancestor| excluded.contains(&ancestor))
    }

    fn initialize_node_once(dom: &Dom, id: NodeId, store: &mut NodeStateStore, flags: u32) -> bool {
        let score = compute_initial_readability_data(dom, id, flags);
        store.initialize_if_absent(id, score)
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
        let c = self.dom.create_html_element(Tag::Div).ok()?;
        self.dom.insert_before(first, c);
        for &x in siblings {
            if let Some(t) = self.dom.tag(x) {
                if !is_alter_to_div_exception(t) {
                    self.dom.rename_html(x, Tag::Div)
                }
            }
            self.dom.append_child(c, x)
        }
        Some(c)
    }
    fn prep_article(
        &mut self,
        root: NodeId,
        video: &Regex,
        match_buffer: &mut String,
        text_buffer: &mut String,
        nodes: &mut Vec<NodeId>,
    ) {
        clean_styles(&mut self.dom, root, nodes);
        mark_data_tables(&self.dom, root, &mut self.node_data, nodes);
        fix_lazy_images(&mut self.dom, root, nodes);
        clean_conditionally(
            &mut self.dom,
            root,
            &[Tag::Form],
            Tag::Form,
            self.flags,
            video,
            &mut self.node_data,
            text_buffer,
            nodes,
            self.options.link_density_modifier,
        );
        clean_conditionally(
            &mut self.dom,
            root,
            &[Tag::Fieldset],
            Tag::Fieldset,
            self.flags,
            video,
            &mut self.node_data,
            text_buffer,
            nodes,
            self.options.link_density_modifier,
        );
        clean_tags(
            &mut self.dom,
            root,
            &[Tag::Object, Tag::Embed, Tag::Footer, Tag::Link, Tag::Aside],
            video,
            nodes,
        );
        let threshold = crate::constants::defaults::DEFAULT_CHAR_THRESHOLD;
        let children: SmallVec<[NodeId; 16]> = self.dom.element_children(root).collect();
        for c in children {
            clean_matched_nodes(&mut self.dom, c, nodes, match_buffer, |d, id, m| {
                has_share_element(m) && get_inner_text(d, id, text_buffer).len() < threshold
            })
        }
        clean_tags(
            &mut self.dom,
            root,
            &[
                Tag::Iframe,
                Tag::Input,
                Tag::Textarea,
                Tag::Select,
                Tag::Button,
            ],
            video,
            nodes,
        );
        clean_headers(&mut self.dom, root, self.flags, nodes);
        clean_conditionally(
            &mut self.dom,
            root,
            &[Tag::Table],
            Tag::Table,
            self.flags,
            video,
            &mut self.node_data,
            text_buffer,
            nodes,
            self.options.link_density_modifier,
        );
        clean_conditionally(
            &mut self.dom,
            root,
            &[Tag::Ul, Tag::Ol],
            Tag::Ul,
            self.flags,
            video,
            &mut self.node_data,
            text_buffer,
            nodes,
            self.options.link_density_modifier,
        );
        clean_conditionally(
            &mut self.dom,
            root,
            &[Tag::Div],
            Tag::Div,
            self.flags,
            video,
            &mut self.node_data,
            text_buffer,
            nodes,
            self.options.link_density_modifier,
        );
        let hs: SmallVec<[NodeId; 4]> = self
            .dom
            .descendants(root)
            .filter(|&x| self.dom.tag(x) == Some(Tag::H1))
            .collect();
        for x in hs {
            self.dom.rename_html(x, Tag::H2)
        }
        let ps: SmallVec<[NodeId; 64]> = self
            .dom
            .descendants(root)
            .filter(|&x| self.dom.tag(x) == Some(Tag::P))
            .collect();
        for p in ps {
            let media = self.dom.descendants(p).any(|x| {
                matches!(
                    self.dom.tag(x),
                    Some(Tag::Img | Tag::Embed | Tag::Object | Tag::Iframe)
                )
            });
            if !media && !has_non_empty_inner_text(&self.dom, p) {
                self.dom.detach(p)
            }
        }
        let brs: SmallVec<[NodeId; 32]> = self
            .dom
            .descendants(root)
            .filter(|&x| self.dom.tag(x) == Some(Tag::Br))
            .collect();
        for br in brs {
            if crate::cleaning::next_non_whitespace_sibling(&self.dom, br)
                .is_some_and(|x| self.dom.tag(x) == Some(Tag::P))
            {
                self.dom.detach(br)
            }
        }
        let tables: SmallVec<[NodeId; 16]> = self
            .dom
            .descendants(root)
            .filter(|&x| self.dom.tag(x) == Some(Tag::Table))
            .collect();
        for t in tables {
            let tb = if has_single_tag_inside_element(&self.dom, t, Tag::Tbody) {
                self.dom.element_children(t).next()
            } else {
                Some(t)
            };
            if let Some(tb) = tb {
                if has_single_tag_inside_element(&self.dom, tb, Tag::Tr) {
                    if let Some(row) = self.dom.element_children(tb).next() {
                        if has_single_tag_inside_element(&self.dom, row, Tag::Td) {
                            if let Some(cell) = self.dom.element_children(row).next() {
                                let phr = self
                                    .dom
                                    .children(cell)
                                    .all(|x| is_phrasing_content(&self.dom, x));
                                self.dom
                                    .rename_html(cell, if phr { Tag::P } else { Tag::Div });
                                self.dom.replace_with(t, cell)
                            }
                        }
                    }
                }
            }
        }
    }
    fn article_excerpt(&self, root: NodeId) -> Option<String> {
        self.dom
            .first_descendant_by_tag(root, Tag::P)
            .map(|id| get_inner_text_owned(&self.dom, id))
            .filter(|text| !text.is_empty())
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
        simplify_nested_elements(&mut self.dom, root, nodes);
    }
    fn reparse_prepare(&mut self) -> Result<()> {
        self.dom = Dom::parse_document(self.original_html).map_err(|_| Error::NoContent)?;
        unwrap_noscript_images(&mut self.dom);
        prep_document(&mut self.dom);
        self.article_byline = None;
        self.article_dir = None;
        self.article_lang = None;
        self.node_data.clear();
        if self.dom.body().is_none() {
            Err(Error::NoBody)
        } else {
            Ok(())
        }
    }
}
fn heading_matches_article_title(article_title: &str, heading: &str) -> bool {
    metadata::text_similarity(article_title, heading) > 0.75
        || article_title.strip_prefix(heading).is_some_and(|suffix| {
            let suffix = suffix.trim_start();
            !heading.is_empty()
                && suffix.chars().next().is_some_and(|c| {
                    matches!(c, '|' | '-' | '–' | '—' | '/' | '>' | '»' | '_' | ':')
                })
        })
}

fn has_ancestor_tags_any(dom: &Dom, id: NodeId, tags: &[Tag], max: usize) -> bool {
    dom.ancestors(id)
        .take(max)
        .any(|x| dom.tag(x).is_some_and(|t| tags.contains(&t)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_title_prefix_with_whitespace_before_separator() {
        assert!(heading_matches_article_title(
            "Article | Example",
            "Article"
        ));
        assert!(!heading_matches_article_title("Different title", "Article"));
    }

    #[test]
    fn discovery_and_scoring_preserve_unlikely_subtrees() {
        let html = r#"<body>
            <div class="sidebar" id="unlikely"><p>This sidebar text is long enough to inspect.</p></div>
            <main><p>This primary content is long enough to score as a candidate.</p></main>
        </body>"#;
        let dom = Dom::parse_document(html).unwrap();
        let config = ExtractorConfig::default();
        let mut readability = Readability::from_document(dom, html, None, &config);
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
        let prepared = Readability::prepare_scoring_structure(
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
        let candidates = readability.propagate_candidate_scores(&scoring_dom, to_score);
        let _ = readability.rank_candidates(
            &scoring_dom,
            discovery.candidates,
            candidates,
            &discovery.remove_after_scoring,
        );

        assert!(scoring_dom.parent(unlikely).is_some());
        assert_eq!(readability.dom.parent(unlikely), parent);
        assert_eq!(readability.dom.parent(normal), normal_parent);
        assert_eq!(readability.dom.tag(normal), normal_tag);
        assert_eq!(readability.dom.attrs(normal), normal_attrs);
        assert_eq!(readability.dom.len(), dom_len);
        assert!(discovery.remove_after_scoring.contains(&unlikely));
    }
}
