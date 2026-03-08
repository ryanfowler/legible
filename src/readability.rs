//! Main Readability parser implementation.

use std::borrow::Cow;

use crate::cleaning::{
    clean_conditionally, clean_headers, clean_matched_nodes, clean_styles, clean_tags,
    fix_lazy_images, mark_data_tables, prep_document, remove_scripts, simplify_nested_elements,
    unwrap_noscript_images,
};
use crate::constants::{
    ALTER_TO_DIV_EXCEPTIONS, DEFAULT_TAGS_TO_SCORE, UNLIKELY_ROLES, flags::*, regexps,
};
use crate::dom::{
    NodeDataStore, build_match_string, get_tag_name, has_ancestor_tag, node_select_matcher,
};
use crate::error::{Error, Result};
use crate::logging::debug_log;
use crate::metadata::{
    Metadata, get_article_metadata, get_article_title, get_json_ld, text_similarity,
};
use crate::options::Options;
use crate::scoring::{
    compute_initial_readability_data, get_inner_text, get_link_density, get_link_density_cached,
    get_or_compute_stats, has_child_block_element, has_single_tag_inside_element, initialize_node,
    is_element_without_content, is_phrasing_content, is_probably_visible, is_valid_byline,
    wrap_phrasing_content_in_p,
};
use crate::selectors::{SELECTORS, Selectors};
use dom_query::{Document, Node, NodeId};
use hashbrown::{HashMap, HashSet};
use regex::Regex;
use url::Url;

/// The extracted article content.
///
/// This struct contains the main article content extracted from an HTML document,
/// along with metadata like title, author, and publication date.
///
/// # Example
///
/// ```rust
/// use legible::parse;
///
/// let html = "<html><body><article><h1>Title</h1><p>Content...</p></article></body></html>";
///
/// if let Ok(article) = parse(html, None, None) {
///     println!("Title: {}", article.title);
///     println!("Author: {:?}", article.byline);
///     println!("HTML length: {} bytes", article.content.len());
///     println!("Text length: {} chars", article.length);
/// }
/// ```
///
/// # Security
///
/// The [`content`](Article::content) field contains unsanitized HTML extracted from the
/// source document. Before rendering this HTML in a browser or other context where scripts
/// could execute, you should sanitize it using a library like [`ammonia`](https://docs.rs/ammonia):
///
/// ```rust,ignore
/// let safe_html = ammonia::clean(&article.content);
/// ```
#[derive(Debug, Clone)]
pub struct Article {
    /// The article title.
    ///
    /// Extracted from the document's `<title>` tag, `<h1>`, or metadata (JSON-LD, OpenGraph).
    pub title: String,

    /// The author byline.
    ///
    /// Extracted from byline elements, `rel="author"` links, or metadata.
    pub byline: Option<String>,

    /// The text direction (`"ltr"` or `"rtl"`).
    ///
    /// Inherited from the `dir` attribute of ancestor elements.
    pub dir: Option<String>,

    /// The document language (e.g., `"en"`, `"fr"`).
    ///
    /// Extracted from the `lang` attribute of the `<html>` element.
    pub lang: Option<String>,

    /// The article content as HTML.
    ///
    /// This is the cleaned, extracted article content wrapped in a container div.
    /// The HTML structure is simplified and non-content elements are removed.
    ///
    /// **Warning:** This HTML is unsanitized and may contain malicious scripts or other
    /// dangerous content. Always sanitize before rendering (e.g., with the `ammonia` crate).
    pub content: String,

    /// The article content as plain text.
    ///
    /// All HTML tags are stripped, leaving only the text content.
    pub text_content: String,

    /// The length of the text content in characters.
    pub length: usize,

    /// A short excerpt from the article.
    ///
    /// Typically the first paragraph or the meta description.
    pub excerpt: Option<String>,

    /// The site name (e.g., `"The New York Times"`).
    ///
    /// Extracted from OpenGraph `og:site_name` or JSON-LD metadata.
    pub site_name: Option<String>,

    /// The published time as an ISO 8601 string.
    ///
    /// Extracted from `article:published_time` meta tag or JSON-LD metadata.
    pub published_time: Option<String>,
}

/// The Readability parser for extracting article content from HTML.
///
/// This is an internal implementation detail. Use the public [`parse()`](crate::parse)
/// function instead.
pub(crate) struct Readability<'a> {
    doc: Document,
    original_html: &'a str,
    options: Options,
    flags: u32,
    node_data: NodeDataStore,
    article_title: String,
    article_byline: Option<String>,
    article_dir: Option<String>,
    article_lang: Option<String>,
    article_site_name: Option<String>,
    metadata: Metadata,
    base_uri: Option<Url>,
    url_error: Option<url::ParseError>,
    attempts: Vec<AttemptResult>,
}

struct AttemptResult {
    content_html: String,
    text_length: usize,
}

/// Cached node index for O(1) lookups by NodeId.
/// This avoids rebuilding the index multiple times during grab_article.
struct NodeIndex<'a> {
    nodes: Vec<Node<'a>>,
    index: HashMap<NodeId, usize>,
}

impl<'a> NodeIndex<'a> {
    /// Build a new node index from the document.
    fn new(doc: &'a Document) -> Self {
        let nodes: Vec<_> = doc.select("*").nodes().to_vec();
        let index: HashMap<NodeId, usize> =
            nodes.iter().enumerate().map(|(i, n)| (n.id, i)).collect();
        Self { nodes, index }
    }

    /// Get a node by its ID in O(1) time.
    fn get(&self, id: &NodeId) -> Option<Node<'a>> {
        self.index.get(id).map(|&i| self.nodes[i])
    }
}

/// Intermediate article content extracted by grab_article
struct ArticleContent {
    content_html: String,
    text_content: String,
    excerpt: Option<String>,
}

impl<'a> Readability<'a> {
    /// Create a new Readability parser from a pre-parsed document.
    pub(crate) fn from_document(
        doc: Document,
        original_html: &'a str,
        url: Option<&str>,
        options: Option<Options>,
    ) -> Self {
        let options = options.unwrap_or_default();

        let (base_uri, url_error) = match url {
            Some(u) => match Url::parse(u) {
                Ok(parsed) => (Some(parsed), None),
                Err(e) => (None, Some(e)),
            },
            None => (None, None),
        };
        Self {
            doc,
            original_html,
            options,
            flags: FLAG_STRIP_UNLIKELYS | FLAG_WEIGHT_CLASSES | FLAG_CLEAN_CONDITIONALLY,
            node_data: NodeDataStore::new(),
            article_title: String::new(),
            article_byline: None,
            article_dir: None,
            article_lang: None,
            article_site_name: None,
            metadata: Metadata::default(),
            base_uri,
            url_error,
            attempts: Vec::new(),
        }
    }

    /// Parse the document and extract the article content.
    pub(crate) fn parse(mut self) -> Result<Article> {
        // Check for URL parsing error
        if let Some(e) = self.url_error {
            return Err(Error::InvalidUrl(e));
        }

        // Check element count limit
        if self.options.max_elems_to_parse > 0 {
            let count = self.doc.select("*").length();
            if count > self.options.max_elems_to_parse {
                return Err(Error::TooManyElements(
                    count,
                    self.options.max_elems_to_parse,
                ));
            }
        }

        // Unwrap images from noscript tags
        unwrap_noscript_images(&self.doc, &SELECTORS);

        // Get article title early (needed for JSON-LD disambiguation)
        let article_title = get_article_title(&self.doc, &SELECTORS);

        // Extract JSON-LD metadata before removing scripts
        let json_ld = if self.options.disable_json_ld {
            Metadata::default()
        } else {
            get_json_ld(&self.doc, &article_title, &SELECTORS)
        };

        // Remove scripts
        remove_scripts(&self.doc, &SELECTORS);

        // Prepare document
        prep_document(&self.doc, &SELECTORS);

        // Store article title
        self.article_title = article_title;

        // Get metadata
        self.metadata = get_article_metadata(&self.doc, &json_ld, &self.article_title, &SELECTORS);
        if self.metadata.title.is_some() {
            self.article_title = self.metadata.title.take().unwrap_or_default();
        }

        // Grab the article
        let article_content = self.grab_article()?;

        // Get excerpt if not in metadata
        let excerpt = self.metadata.excerpt.take().or(article_content.excerpt);

        let length = article_content.text_content.chars().count();

        Ok(Article {
            title: std::mem::take(&mut self.article_title),
            byline: self
                .metadata
                .byline
                .take()
                .or_else(|| self.article_byline.take()),
            dir: self.article_dir.take(),
            lang: self.article_lang.take(),
            content: article_content.content_html,
            text_content: article_content.text_content,
            length,
            excerpt,
            site_name: self
                .metadata
                .site_name
                .take()
                .or_else(|| self.article_site_name.take()),
            published_time: self.metadata.published_time.take(),
        })
    }

    /// The main content extraction algorithm.
    fn grab_article(&mut self) -> Result<ArticleContent> {
        let body = self.doc.select_matcher(&SELECTORS.body);
        if body.length() == 0 {
            return Err(Error::NoBody);
        }

        loop {
            debug_log!(self, "Starting grabArticle loop");

            let strip_unlikely_candidates = self.flag_is_active(FLAG_STRIP_UNLIKELYS);

            // First, node prepping
            // Use HashSet for O(1) membership checks (Phase 1.2)
            let mut elements_to_score: HashSet<NodeId> = HashSet::new();

            // Get the HTML element for language
            if let Some(html) = self.doc.select_matcher(&SELECTORS.html).nodes().first()
                && let Some(lang) = html.attr("lang")
            {
                self.article_lang = Some(lang.to_string());
            }

            // Track nodes to remove (use HashSet for efficient lookup)
            let mut nodes_to_remove: HashSet<NodeId> = HashSet::new();
            let mut should_remove_title_header = true;

            // First pass: identify nodes to remove and score
            let all_nodes: Vec<_> = self.doc.select("*").nodes().to_vec();
            let all_nodes_index: HashMap<NodeId, usize> = all_nodes
                .iter()
                .enumerate()
                .map(|(i, n)| (n.id, i))
                .collect();

            // Reusable buffer for building match_string to avoid allocations per node
            let mut match_string_buf = String::with_capacity(128);
            let mut active_removed_root: Option<NodeId> = None;

            for node in &all_nodes {
                // Document order is preorder, so only the most recent removed subtree
                // can affect the current node.
                if let Some(removed_root) = active_removed_root {
                    let mut parent = node.parent();
                    let mut still_in_removed_subtree = false;
                    while let Some(p) = parent {
                        if p.id == removed_root {
                            still_in_removed_subtree = true;
                            break;
                        }
                        parent = p.parent();
                    }

                    if still_in_removed_subtree {
                        continue;
                    }

                    active_removed_root = None;
                }

                if nodes_to_remove.contains(&node.id) {
                    continue;
                }

                let tag_name = get_tag_name(node).unwrap_or_default();
                // Build match_string for regex matching - reuse buffer to avoid allocations
                build_match_string(node, &mut match_string_buf);
                let match_string = &match_string_buf;

                // Check visibility
                if !is_probably_visible(node) {
                    debug_log!(self, "Removing hidden node: {}", match_string);
                    nodes_to_remove.insert(node.id);
                    active_removed_root = Some(node.id);
                    continue;
                }

                // Check aria-modal with role=dialog
                if node
                    .attr("aria-modal")
                    .map(|s| s.as_ref() == "true")
                    .unwrap_or(false)
                    && node
                        .attr("role")
                        .map(|s| s.as_ref() == "dialog")
                        .unwrap_or(false)
                {
                    nodes_to_remove.insert(node.id);
                    active_removed_root = Some(node.id);
                    continue;
                }

                // Check for byline
                if self.article_byline.is_none()
                    && self.metadata.byline.is_none()
                    && is_valid_byline(node, match_string)
                {
                    // Look for itemprop="name" child
                    let itemprop_name = node_select_matcher(node, &SELECTORS.itemprop)
                        .nodes()
                        .first()
                        .cloned();
                    let byline_node = itemprop_name.as_ref().unwrap_or(node);
                    self.article_byline = Some(byline_node.text().trim().to_string());
                    nodes_to_remove.insert(node.id);
                    active_removed_root = Some(node.id);
                    continue;
                }

                // Check for duplicate title header
                if should_remove_title_header && self.header_duplicates_title(node) {
                    debug_log!(
                        self,
                        "Removing header: {} / {}",
                        node.text().trim(),
                        self.article_title.trim()
                    );
                    should_remove_title_header = false;
                    nodes_to_remove.insert(node.id);
                    active_removed_root = Some(node.id);
                    continue;
                }

                // Remove unlikely candidates - check cheap conditions first, then regex
                if strip_unlikely_candidates {
                    // Check tag names first (cheap) before running regex (expensive)
                    if tag_name != "BODY" && tag_name != "A" {
                        let candidate_matches = regexps::CANDIDATE_FILTER_SET.matches(match_string);
                        if candidate_matches.matched(0)  // UNLIKELY_CANDIDATES
                            && !candidate_matches.matched(1)  // OK_MAYBE_ITS_A_CANDIDATE
                            && !has_ancestor_tag(node, "table", 3, None::<fn(&Node) -> bool>)
                            && !has_ancestor_tag(node, "code", 3, None::<fn(&Node) -> bool>)
                        {
                            debug_log!(self, "Removing unlikely candidate: {}", match_string);
                            nodes_to_remove.insert(node.id);
                            active_removed_root = Some(node.id);
                            continue;
                        }
                    }

                    if let Some(role) = node.attr("role")
                        && UNLIKELY_ROLES.contains(role.as_ref())
                    {
                        debug_log!(
                            self,
                            "Removing element with role={}: {}",
                            role,
                            match_string
                        );
                        nodes_to_remove.insert(node.id);
                        active_removed_root = Some(node.id);
                        continue;
                    }
                }

                // Remove empty DIV, SECTION, HEADER, H1-H6
                if matches!(
                    &*tag_name,
                    "DIV" | "SECTION" | "HEADER" | "H1" | "H2" | "H3" | "H4" | "H5" | "H6"
                ) && is_element_without_content(node, &SELECTORS)
                {
                    nodes_to_remove.insert(node.id);
                    active_removed_root = Some(node.id);
                    continue;
                }

                // Add to elements to score (HashSet handles duplicates automatically)
                if DEFAULT_TAGS_TO_SCORE.contains(&*tag_name) {
                    elements_to_score.insert(node.id);
                }

                // Process DIVs - wrap phrasing content in P tags
                if tag_name == "DIV" {
                    // First, wrap any loose phrasing content in P tags
                    wrap_phrasing_content_in_p(node);

                    // Now check if DIV should be converted or scored
                    if has_single_tag_inside_element(node, "P")
                        && get_link_density(node, &SELECTORS) < 0.25
                    {
                        // Sites like http://mobile.slate.com enclose each paragraph with a DIV
                        // element. DIVs with only a P element inside and no text content can be
                        // safely converted into plain P elements to avoid confusing the scoring
                        // algorithm with DIVs with are, in practice, paragraphs.
                        if let Some(p_child) = node.element_children().first() {
                            let p_id = p_child.id;
                            node.replace_with(p_child);
                            elements_to_score.insert(p_id);
                        }
                    } else if !has_child_block_element(node) {
                        node.rename("p");
                        elements_to_score.insert(node.id);
                    } else {
                        // DIV stays as DIV - add any P children to elements_to_score
                        // (these may have been created by wrap_phrasing_content_in_p)
                        // This mimics JS behavior where tree walker visits children
                        for child in node.element_children() {
                            if let Some(child_tag) = get_tag_name(&child)
                                && child_tag == "P"
                            {
                                elements_to_score.insert(child.id);
                            }
                        }
                    }
                }
            }

            // Remove marked nodes using the HashMap index for O(1) lookups
            for node_id in &nodes_to_remove {
                if let Some(&idx) = all_nodes_index.get(node_id) {
                    all_nodes[idx].remove_from_parent();
                }
            }

            // Build node index once after removals - reuse for scoring, candidates, and sibling gathering
            let cached_index = NodeIndex::new(&self.doc);

            // Score elements
            let mut candidates: Vec<NodeId> = Vec::with_capacity(elements_to_score.len());

            for node_id in &elements_to_score {
                let node = match cached_index.get(node_id) {
                    Some(n) => n,
                    None => continue,
                };

                let _parent = match node.parent() {
                    Some(p) if p.is_element() => p,
                    _ => continue,
                };

                // Get or compute cached stats for this node
                let stats = get_or_compute_stats(&node, &mut self.node_data);
                let inner_text_len = stats.text_length;
                if inner_text_len < 25 {
                    continue;
                }

                // Get ancestors (up to 5 levels)
                let ancestors = get_ancestors(&node, 5);
                if ancestors.is_empty() {
                    continue;
                }

                // Calculate content score
                // Note: stats.comma_count is the raw count, add 1 to match JS split().length behavior
                let mut content_score = 1.0;
                content_score += (stats.comma_count + 1) as f64;
                content_score += (inner_text_len / 100).min(3) as f64;

                // Score ancestors
                for (level, ancestor) in ancestors.iter().enumerate() {
                    if !ancestor.is_element() {
                        continue;
                    }

                    if !ancestor.parent().map(|p| p.is_element()).unwrap_or(false) {
                        continue;
                    }

                    {
                        let data = compute_initial_readability_data(ancestor, self.flags);
                        if self.node_data.initialize_if_absent(ancestor.id, data) {
                            candidates.push(ancestor.id);
                        }
                    }

                    let score_divider = if level == 0 {
                        1.0
                    } else if level == 1 {
                        2.0
                    } else {
                        (level * 3) as f64
                    };

                    self.node_data
                        .add_content_score(ancestor.id, content_score / score_divider);
                }
            }

            // Find top candidates
            // Collect all scores first, then sort once at the end (Phase 3.2)
            let mut all_candidate_scores: Vec<(NodeId, f64)> = Vec::with_capacity(candidates.len());

            for candidate_id in &candidates {
                let candidate = match cached_index.get(candidate_id) {
                    Some(c) => c,
                    None => continue,
                };

                let score = self.node_data.get_content_score(candidate_id);
                // Get or compute stats for cached link density calculation
                let stats = get_or_compute_stats(&candidate, &mut self.node_data);
                let link_density = get_link_density_cached(
                    &candidate,
                    stats.text_length,
                    &mut self.node_data,
                    &SELECTORS,
                );
                let final_score = score * (1.0 - link_density);

                if let Some(data) = self.node_data.get_mut(candidate_id) {
                    data.content_score = final_score;
                }

                debug_log!(self, "Candidate with score {:.2}", final_score);

                all_candidate_scores.push((*candidate_id, final_score));
            }

            // Sort by score descending and take top N (Phase 3.2 - O(n log n) vs O(n²))
            all_candidate_scores
                .sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            let top_candidates: Vec<(NodeId, f64)> = all_candidate_scores
                .into_iter()
                .take(self.options.nb_top_candidates)
                .collect();

            // Get top candidate
            // Check if we need to create a synthetic top candidate (when no candidates or top is BODY)
            let body = self.doc.select_matcher(&SELECTORS.body);
            let body_node = body.nodes().first().ok_or(Error::NoBody)?;
            let body_id = body_node.id;

            let needs_synthetic_candidate = top_candidates.is_empty()
                || top_candidates
                    .first()
                    .map(|(id, _)| *id == body_id)
                    .unwrap_or(false);

            let (top_candidate_id, needed_to_create_top_candidate) = if needs_synthetic_candidate {
                // Move all of the page's children into a new DIV
                // (like JS: create DIV, move everything into it, append to page)
                let container = self.doc.tree.new_element("div");
                let container_id = container.id;

                // Collect all body children first to avoid mutation while iterating
                let children: Vec<_> = body_node.children();

                // Move all children (including text nodes) into the container
                for child in children {
                    debug_log!(
                        self,
                        "Moving child out: {}",
                        get_tag_name(&child).unwrap_or_default()
                    );
                    container.append_child(&child);
                }

                // Append the container to body
                body_node.append_child(&container);

                // Initialize the new container node
                initialize_node(&container, &mut self.node_data, self.flags);

                (Some(container_id), true)
            } else {
                let top_id = top_candidates[0].0;
                let mut top_candidate = cached_index.get(&top_id);

                // Alternative candidate ancestors logic:
                // Find a better top candidate node if it contains (at least three) nodes
                // which belong to top_candidates array and whose scores are close to the
                // current top_candidate score.
                if let Some(ref tc) = top_candidate {
                    let top_score = top_candidates[0].1;

                    // Collect ancestor sets for candidates with score >= 75% of top score
                    // Use HashSet for O(1) lookups instead of Vec::contains() O(n)
                    let mut alternative_candidate_ancestors: Vec<HashSet<NodeId>> = Vec::new();
                    for &(candidate_id, candidate_score) in top_candidates.iter().skip(1) {
                        if candidate_score / top_score >= 0.75
                            && let Some(candidate_node) = cached_index.get(&candidate_id)
                        {
                            let ancestors: HashSet<NodeId> = get_ancestors(&candidate_node, 0)
                                .iter()
                                .map(|n| n.id)
                                .collect();
                            alternative_candidate_ancestors.push(ancestors);
                        }
                    }

                    const MINIMUM_TOPCANDIDATES: usize = 3;
                    if alternative_candidate_ancestors.len() >= MINIMUM_TOPCANDIDATES {
                        let mut parent = tc.parent();
                        while let Some(p) = parent {
                            if let Some(ptag) = get_tag_name(&p)
                                && ptag == "BODY"
                            {
                                break;
                            }

                            let mut lists_containing_this_ancestor = 0;
                            for ancestor_list in &alternative_candidate_ancestors {
                                if ancestor_list.contains(&p.id) {
                                    lists_containing_this_ancestor += 1;
                                }
                                // Early exit optimization (matches JS behavior)
                                if lists_containing_this_ancestor >= MINIMUM_TOPCANDIDATES {
                                    break;
                                }
                            }

                            if lists_containing_this_ancestor >= MINIMUM_TOPCANDIDATES {
                                top_candidate = Some(p);
                                break;
                            }

                            parent = p.parent();
                        }
                    }
                }

                // Ensure new top candidate has readability data initialized
                if let Some(ref tc) = top_candidate
                    && !self.node_data.has(&tc.id)
                {
                    initialize_node(tc, &mut self.node_data, self.flags);
                }

                // Walk up the tree looking for a better parent.
                // JavaScript comment: "Because of our bonus system, parents of candidates
                // might have scores themselves. They get half of the node. There won't be
                // nodes with higher scores than our topCandidate, but if we see the score
                // going *up* in the first few steps up the tree, that's a decent sign that
                // there might be more content lurking in other places that we want to unify in."
                if let Some(ref tc) = top_candidate {
                    let mut parent = tc.parent();
                    let top_score = self.node_data.get_content_score(&tc.id);
                    let score_threshold = top_score / 3.0;
                    let mut last_score = top_score;

                    while let Some(p) = parent {
                        if let Some(ptag) = get_tag_name(&p)
                            && ptag == "BODY"
                        {
                            break;
                        }

                        if let Some(parent_data) = self.node_data.get(&p.id) {
                            let parent_score = parent_data.content_score;
                            if parent_score < score_threshold {
                                break;
                            }
                            // If score is increasing, we found a better parent
                            if parent_score > last_score {
                                top_candidate = Some(p);
                                break;
                            }
                            last_score = parent_score;
                        }

                        parent = p.parent();
                    }

                    // If the top candidate is the only child, use parent instead.
                    // This will help sibling joining logic when adjacent content
                    // is actually located in parent's sibling node.
                    if let Some(ref mut tc) = top_candidate {
                        let mut parent_of_tc = tc.parent();
                        while let Some(p) = parent_of_tc {
                            if let Some(ptag) = get_tag_name(&p)
                                && ptag == "BODY"
                            {
                                break;
                            }
                            if p.element_children().len() == 1 {
                                *tc = p;
                                parent_of_tc = tc.parent();
                            } else {
                                break;
                            }
                        }
                    }
                }

                (top_candidate.map(|tc| tc.id), false)
            };

            let top_candidate_id = top_candidate_id.ok_or(Error::NoContent)?;

            // Get the article node ID to use
            let article_node_id = if needed_to_create_top_candidate {
                // No siblings to gather when we created a synthetic candidate
                // Use the container DIV we created (already stored in top_candidate_id)
                Some(top_candidate_id)
            } else {
                // Gather siblings and potentially create container DIV
                // Like JS, we always create a container and move siblings into it
                let sibling_ids = Self::gather_siblings(
                    top_candidate_id,
                    &cached_index,
                    &mut self.node_data,
                    &SELECTORS,
                    self.options.debug,
                );

                if sibling_ids.is_empty() {
                    // No siblings qualified (shouldn't happen since top candidate always included)
                    // Fall back to using top candidate directly
                    Some(top_candidate_id)
                } else {
                    // Create container DIV and move siblings into it
                    // This includes the case of a single sibling - we still need a container
                    // so that the wrapper div is added correctly
                    if let Some(top_candidate) = cached_index.get(&top_candidate_id) {
                        self.create_article_container(&top_candidate, &sibling_ids, &cached_index)
                    } else {
                        Some(top_candidate_id)
                    }
                }
            };

            let article_node_id = article_node_id.ok_or(Error::NoContent)?;

            // Get article node directly by ID - O(1) instead of rebuilding full index
            let article_node = self.doc.tree.get(&article_node_id);

            // Prepare article content
            if let Some(article_node) = article_node {
                // Use reference to avoid cloning the regex
                let video_regex: &Regex = self
                    .options
                    .allowed_video_regex
                    .as_ref()
                    .unwrap_or(&regexps::VIDEOS);

                Self::prep_article(
                    &article_node,
                    &mut self.node_data,
                    self.flags,
                    video_regex,
                    self.options.link_density_modifier,
                    &SELECTORS,
                );

                // Re-fetch the article node after prep_article mutated the DOM - O(1) lookup
                let article_node = self
                    .doc
                    .tree
                    .get(&article_node_id)
                    .ok_or(Error::NoContent)?;

                // Add readability-page-1 wrapper div
                // In JS: if neededToCreateTopCandidate, set id/class on topCandidate
                //        else create wrapper div, move children into it, append to articleContent
                // Since both cases result in articleContent.innerHTML starting with
                // <div id="readability-page-1" class="page">, we create a wrapper in all cases.
                if needed_to_create_top_candidate {
                    // Set id/class directly on the synthetic container
                    article_node.set_attr("id", "readability-page-1");
                    article_node.set_attr("class", "page");
                } else {
                    // Create wrapper div and move all children into it
                    let wrapper = self.doc.tree.new_element("div");
                    wrapper.set_attr("id", "readability-page-1");
                    wrapper.set_attr("class", "page");

                    // Move all children of article_node into wrapper
                    let children: Vec<_> = article_node.children();
                    for child in children {
                        wrapper.append_child(&child);
                    }

                    // Append wrapper to article_node
                    article_node.append_child(&wrapper);
                }

                // Re-fetch the article node after wrapping - O(1) lookup
                let article_node = self
                    .doc
                    .tree
                    .get(&article_node_id)
                    .ok_or(Error::NoContent)?;

                let text_content = get_inner_text(&article_node, true);
                let text_length = text_content.chars().count();

                if text_length < self.options.char_threshold {
                    self.attempts.push(AttemptResult {
                        content_html: article_node.html().to_string(),
                        text_length,
                    });

                    if self.flag_is_active(FLAG_STRIP_UNLIKELYS) {
                        self.remove_flag(FLAG_STRIP_UNLIKELYS);
                    } else if self.flag_is_active(FLAG_WEIGHT_CLASSES) {
                        self.remove_flag(FLAG_WEIGHT_CLASSES);
                    } else if self.flag_is_active(FLAG_CLEAN_CONDITIONALLY) {
                        self.remove_flag(FLAG_CLEAN_CONDITIONALLY);
                    } else {
                        self.attempts
                            .sort_by(|a, b| b.text_length.cmp(&a.text_length));

                        if self.attempts.is_empty() || self.attempts[0].text_length == 0 {
                            return Err(Error::NoContent);
                        }

                        // Use the best attempt - set its content as body and extract text/excerpt
                        let best_attempt = &self.attempts[0];
                        self.doc
                            .select_matcher(&SELECTORS.body)
                            .set_html(best_attempt.content_html.as_str());

                        // Re-fetch to get text and excerpt
                        if let Some(body) = self
                            .doc
                            .select_matcher(&SELECTORS.body)
                            .nodes()
                            .first()
                            .cloned()
                        {
                            let text_content = get_inner_text(&body, true);
                            let excerpt = node_select_matcher(&body, &SELECTORS.p)
                                .nodes()
                                .first()
                                .map(|p| p.text().trim().to_string())
                                .filter(|s| !s.is_empty());
                            let content_html = self.post_process_content_node(&body);

                            return Ok(ArticleContent {
                                content_html,
                                text_content,
                                excerpt,
                            });
                        }

                        return Err(Error::NoContent);
                    }

                    // Reparse document from original HTML for retry
                    self.reparse_and_prepare()?;
                    continue;
                }

                // Find dir attribute from ancestors - O(1) lookup
                if let Some(tc) = self.doc.tree.get(&top_candidate_id) {
                    let ancestors = get_ancestors(&tc, 0);
                    for ancestor in std::iter::once(tc).chain(ancestors) {
                        if let Some(dir) = ancestor.attr("dir") {
                            self.article_dir = Some(dir.to_string());
                            break;
                        }
                    }
                }

                // Extract excerpt from the first paragraph
                let excerpt = node_select_matcher(&article_node, &SELECTORS.p)
                    .nodes()
                    .first()
                    .map(|p| p.text().trim().to_string())
                    .filter(|s| !s.is_empty());

                // Post-process and extract content
                let content_html = self.post_process_content_node(&article_node);

                return Ok(ArticleContent {
                    content_html,
                    text_content,
                    excerpt,
                });
            }

            return Err(Error::NoContent);
        }
    }

    /// Gather sibling nodes of the top candidate that should be included in the article.
    /// Returns a list of NodeIds that should be included.
    fn gather_siblings(
        top_candidate_id: NodeId,
        node_index: &NodeIndex<'_>,
        node_data: &mut NodeDataStore,
        selectors: &Selectors,
        debug: bool,
    ) -> Vec<NodeId> {
        let top_candidate = match node_index.get(&top_candidate_id) {
            Some(tc) => tc,
            None => return vec![top_candidate_id],
        };

        let parent = match top_candidate.parent() {
            Some(p) => p,
            None => return vec![top_candidate_id],
        };

        // Calculate sibling score threshold
        let top_score = node_data.get_content_score(&top_candidate_id);
        let sibling_score_threshold = (10.0_f64).max(top_score * 0.2);

        // Get top candidate's class for bonus calculation - keep as Cow to avoid allocation
        let top_class = top_candidate.attr("class");

        let mut siblings_to_include = Vec::new();

        // Iterate through parent's element children
        for sibling in parent.element_children() {
            let mut should_append = false;

            if sibling.id == top_candidate_id {
                // Always include the top candidate itself
                should_append = true;
            } else {
                let mut content_bonus = 0.0;

                // Give a bonus if sibling and top candidate have the same non-empty class
                // Compare Cow<str> directly without allocating String
                let sibling_class = sibling.attr("class");
                if let (Some(top), Some(sib)) = (&top_class, &sibling_class)
                    && !sib.is_empty()
                    && sib.as_ref() == top.as_ref()
                {
                    content_bonus = top_score * 0.2;
                }

                // Check if sibling has a readability score that qualifies
                if node_data.has(&sibling.id) {
                    let sibling_score = node_data.get_content_score(&sibling.id);
                    if sibling_score + content_bonus >= sibling_score_threshold {
                        should_append = true;
                    }
                }

                // Special case for P elements without scores
                if !should_append
                    && let Some(tag) = get_tag_name(&sibling)
                    && tag == "P"
                {
                    // Get or compute cached stats for the sibling
                    let stats = get_or_compute_stats(&sibling, node_data);
                    let node_length = stats.text_length;
                    let link_density =
                        get_link_density_cached(&sibling, node_length, node_data, selectors);

                    if (node_length > 80 && link_density < 0.25)
                        || (node_length < 80
                            && node_length > 0
                            && link_density == 0.0
                            && stats.has_sentence_end)
                    {
                        should_append = true;
                    }
                }
            }

            if should_append {
                debug_log!(@bool debug, "Appending sibling node: {:?}", sibling.id);
                siblings_to_include.push(sibling.id);
            }
        }

        siblings_to_include
    }

    /// Create a container DIV and move the gathered siblings into it.
    /// Returns the NodeId of the new container.
    fn create_article_container(
        &self,
        top_candidate: &Node<'_>,
        sibling_ids: &[NodeId],
        node_index: &NodeIndex<'_>,
    ) -> Option<NodeId> {
        let parent = top_candidate.parent()?;

        // Create a new DIV element as container
        let container = self.doc.tree.new_element("div");
        let container_id = container.id;

        // Find the first sibling to insert before - O(1) lookup
        let first_sibling = sibling_ids.first().and_then(|id| node_index.get(id))?;

        // Insert container before the first sibling
        first_sibling.insert_before(&container);

        // Move each qualifying sibling into the container
        for sibling_id in sibling_ids {
            if let Some(sibling) = node_index.get(sibling_id) {
                // Convert tag to DIV if not in ALTER_TO_DIV_EXCEPTIONS
                if let Some(tag) = get_tag_name(&sibling)
                    && !ALTER_TO_DIV_EXCEPTIONS.contains(&*tag)
                {
                    debug_log!(self, "Altering sibling {} to div", tag);
                    sibling.rename("div");
                }

                // Append to container
                container.append_child(&sibling);
            }
        }

        // Verify we added the container to the parent
        if parent
            .element_children()
            .iter()
            .any(|c| c.id == container_id)
        {
            Some(container_id)
        } else {
            // Container insertion failed, return top candidate
            Some(top_candidate.id)
        }
    }

    /// Prepare the article for display.
    fn prep_article(
        article_content: &Node<'_>,
        node_data: &mut NodeDataStore,
        flags: u32,
        video_regex: &Regex,
        link_density_modifier: f64,
        selectors: &Selectors,
    ) {
        clean_styles(article_content);

        mark_data_tables(article_content, node_data, selectors);

        fix_lazy_images(article_content, selectors);

        clean_conditionally(
            article_content,
            "form",
            flags,
            video_regex,
            node_data,
            link_density_modifier,
            selectors,
        );
        clean_conditionally(
            article_content,
            "fieldset",
            flags,
            video_regex,
            node_data,
            link_density_modifier,
            selectors,
        );
        clean_tags(
            article_content,
            &["object", "embed", "footer", "link", "aside"],
            video_regex,
        );

        let share_threshold = crate::constants::defaults::DEFAULT_CHAR_THRESHOLD;
        for child in article_content.element_children() {
            clean_matched_nodes(&child, |node, match_string| {
                regexps::SHARE_ELEMENTS.is_match(match_string)
                    && node.text().len() < share_threshold
            });
        }

        clean_tags(
            article_content,
            &["iframe", "input", "textarea", "select", "button"],
            video_regex,
        );

        clean_headers(article_content, flags, selectors);

        clean_conditionally(
            article_content,
            "table",
            flags,
            video_regex,
            node_data,
            link_density_modifier,
            selectors,
        );
        clean_conditionally(
            article_content,
            "ul",
            flags,
            video_regex,
            node_data,
            link_density_modifier,
            selectors,
        );
        clean_conditionally(
            article_content,
            "div",
            flags,
            video_regex,
            node_data,
            link_density_modifier,
            selectors,
        );

        for h1 in node_select_matcher(article_content, &selectors.h1)
            .nodes()
            .iter()
        {
            h1.rename("h2");
        }

        // Query P elements once and remove empty ones directly
        let ps: Vec<_> = node_select_matcher(article_content, &selectors.p)
            .nodes()
            .to_vec();
        for p in ps {
            let has_media =
                node_select_matcher(&p, &selectors.img_embed_object_iframe).length() > 0;
            let has_text = !get_inner_text(&p, false).is_empty();
            if !has_media && !has_text {
                p.remove_from_parent();
            }
        }

        for br in node_select_matcher(article_content, &selectors.br)
            .nodes()
            .iter()
        {
            if let Some(next) = br.next_sibling()
                && next.is_element()
                && let Some(tag) = get_tag_name(&next)
                && tag == "P"
            {
                br.remove_from_parent();
            }
        }

        let tables: Vec<_> = node_select_matcher(article_content, &selectors.table)
            .nodes()
            .to_vec();
        for table in tables {
            let tbody = if has_single_tag_inside_element(&table, "TBODY") {
                table.element_children().first().cloned()
            } else {
                Some(table)
            };

            if let Some(tbody) = tbody
                && has_single_tag_inside_element(&tbody, "TR")
                && let Some(row) = tbody.element_children().first()
                && has_single_tag_inside_element(row, "TD")
                && let Some(cell) = row.element_children().first()
            {
                let all_phrasing = cell.children().iter().all(|c| is_phrasing_content(c));
                let new_tag = if all_phrasing { "p" } else { "div" };
                cell.rename(new_tag);
                // Move the cell (now renamed) to replace the table directly
                // This avoids the serialize/deserialize cycle of inner_html + set_html
                table.replace_with(cell);
            }
        }
    }

    /// Post-process the extracted content from a Node.
    /// Combines multiple traversals into optimized passes for better performance.
    fn post_process_content_node(&self, node: &Node<'_>) -> String {
        // These use targeted selectors, so they remain separate
        self.fix_relative_uris(node);

        // This may modify tree structure, so it runs before the combined pass
        simplify_nested_elements(node, &SELECTORS);

        // Combined single-pass traversal for:
        // - clean_classes (if not keeping classes)
        // - remove_comments
        // - escape_attribute_values
        self.post_process_single_pass(node);

        node.inner_html().to_string()
    }

    /// Single-pass post-processing that handles multiple cleanup operations per node.
    /// Combines clean_classes, remove_comments, and escape_attribute_values into one traversal.
    fn post_process_single_pass(&self, node: &Node<'_>) {
        // Collect all descendants first to avoid mutation issues during traversal
        let descendants: Vec<_> = node.descendants_it().collect();

        // Track comment nodes to remove (can't remove during iteration)
        let mut comments_to_remove = Vec::new();

        for descendant in descendants {
            if descendant.is_element() {
                // Clean classes (unless keeping them)
                if !self.options.keep_classes
                    && let Some(class_attr) = descendant.attr("class")
                {
                    // Build preserved classes string directly without intermediate Vec
                    let mut preserved = String::new();
                    for class in class_attr.split_whitespace() {
                        if self.options.classes_to_preserve.iter().any(|p| p == class) {
                            if !preserved.is_empty() {
                                preserved.push(' ');
                            }
                            preserved.push_str(class);
                        }
                    }

                    if preserved.is_empty() {
                        descendant.remove_attr("class");
                    } else {
                        descendant.set_attr("class", &preserved);
                    }
                }

                // Escape < and > in attribute values
                // Check on borrowed value first, only allocate if escaping is needed
                let attrs_to_fix: Vec<_> = descendant
                    .attrs()
                    .iter()
                    .filter_map(|attr| {
                        let value_ref = attr.value.as_ref();
                        if value_ref.contains('<') || value_ref.contains('>') {
                            Some((attr.name.local.to_string(), value_ref.to_string()))
                        } else {
                            None
                        }
                    })
                    .collect();

                for (name, value) in attrs_to_fix {
                    let escaped = escape_angle_brackets(&value);
                    descendant.set_attr(&name, escaped.as_ref());
                }
            } else if !descendant.is_text() {
                // It's a comment node - mark for removal
                comments_to_remove.push(descendant);
            }
        }

        // Remove comment nodes after traversal
        for comment in comments_to_remove {
            comment.remove_from_parent();
        }
    }

    /// Convert relative URIs to absolute.
    fn fix_relative_uris(&self, article_content: &Node<'_>) {
        let base_uri = match &self.base_uri {
            Some(u) => u,
            None => return,
        };

        for link in node_select_matcher(article_content, &SELECTORS.a)
            .nodes()
            .iter()
        {
            if let Some(href) = link.attr("href") {
                if href.starts_with('#') {
                    continue;
                }

                if href.starts_with("javascript:") {
                    let text = link.text();
                    let escaped = html_escape(&text);
                    link.set_html(escaped.as_ref());
                    link.rename("span");
                    continue;
                }

                if let Ok(absolute) = base_uri.join(href.as_ref()) {
                    link.set_attr("href", absolute.as_str());
                }
            }
        }

        for media in node_select_matcher(
            article_content,
            &SELECTORS.img_picture_figure_video_audio_source,
        )
        .nodes()
        .iter()
        {
            if let Some(src) = media.attr("src")
                && let Ok(absolute) = base_uri.join(src.as_ref())
            {
                media.set_attr("src", absolute.as_str());
            }

            if let Some(poster) = media.attr("poster")
                && let Ok(absolute) = base_uri.join(poster.as_ref())
            {
                media.set_attr("poster", absolute.as_str());
            }

            if let Some(srcset) = media.attr("srcset") {
                let new_srcset =
                    regexps::SRCSET_URL.replace_all(srcset.as_ref(), |caps: &regex::Captures| {
                        let url = &caps[1];
                        let descriptor = caps.get(2).map(|m| m.as_str()).unwrap_or("");
                        let comma = &caps[3];

                        if let Ok(absolute) = base_uri.join(url) {
                            format!("{}{}{}", absolute.as_str(), descriptor, comma)
                        } else {
                            caps[0].to_string()
                        }
                    });
                media.set_attr("srcset", &new_srcset);
            }
        }
    }

    /// Check if a header duplicates the article title.
    fn header_duplicates_title(&self, node: &Node<'_>) -> bool {
        let tag = get_tag_name(node).unwrap_or_default();
        if tag != "H1" && tag != "H2" {
            return false;
        }

        let heading = get_inner_text(node, false);
        text_similarity(&self.article_title, &heading) > 0.75
    }

    fn flag_is_active(&self, flag: u32) -> bool {
        (self.flags & flag) > 0
    }

    fn remove_flag(&mut self, flag: u32) {
        self.flags &= !flag;
    }

    /// Reparse the document from original HTML and re-run preparation steps.
    /// Used during retry logic to restore document to prepared state.
    fn reparse_and_prepare(&mut self) -> Result<()> {
        self.doc = Document::from(self.original_html);

        // Re-run preparation (but NOT metadata extraction - already stored)
        unwrap_noscript_images(&self.doc, &SELECTORS);
        remove_scripts(&self.doc, &SELECTORS);
        prep_document(&self.doc, &SELECTORS);

        // Verify body exists
        if self.doc.select_matcher(&SELECTORS.body).length() == 0 {
            return Err(Error::NoBody);
        }

        // Reset state that could have been set during failed attempt
        self.article_byline = None;
        self.article_dir = None;
        self.article_lang = None;

        self.node_data.clear();
        Ok(())
    }
}

/// Get ancestors of a node up to max_depth (0 = unlimited).
fn get_ancestors<'a>(node: &Node<'a>, max_depth: usize) -> Vec<Node<'a>> {
    let mut ancestors = if max_depth > 0 {
        Vec::with_capacity(max_depth)
    } else {
        Vec::new()
    };
    let mut current = node.parent();
    let mut depth = 0;

    while let Some(parent) = current {
        ancestors.push(parent);
        depth += 1;
        if max_depth > 0 && depth >= max_depth {
            break;
        }
        current = parent.parent();
    }

    ancestors
}

/// Escape HTML special characters using single-pass with pre-allocated buffer.
fn html_escape<'a>(s: &'a str) -> Cow<'a, str> {
    if !s.contains(['&', '<', '>', '"', '\'']) {
        return Cow::Borrowed(s);
    }
    let mut result = String::with_capacity(s.len() + 16);
    for c in s.chars() {
        match c {
            '&' => result.push_str("&amp;"),
            '<' => result.push_str("&lt;"),
            '>' => result.push_str("&gt;"),
            '"' => result.push_str("&quot;"),
            '\'' => result.push_str("&#39;"),
            _ => result.push(c),
        }
    }
    Cow::Owned(result)
}

/// Escape only angle brackets using single-pass to avoid intermediate string allocations.
fn escape_angle_brackets<'a>(s: &'a str) -> Cow<'a, str> {
    if !s.contains(['<', '>']) {
        return Cow::Borrowed(s);
    }
    let mut result = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '<' => result.push_str("&lt;"),
            '>' => result.push_str("&gt;"),
            _ => result.push(c),
        }
    }
    Cow::Owned(result)
}
