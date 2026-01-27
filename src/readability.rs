//! Main Readability parser implementation.

use crate::cleaning::{
    clean, clean_classes, clean_conditionally, clean_headers, clean_matched_nodes, clean_styles,
    fix_lazy_images, mark_data_tables, prep_document, remove_scripts, simplify_nested_elements,
    unwrap_noscript_images,
};
use crate::constants::{DEFAULT_TAGS_TO_SCORE, UNLIKELY_ROLES, flags::*, regexps};
use crate::dom::{NodeDataStore, get_tag_name, has_ancestor_tag, node_select};
use crate::error::{ReadabilityError, Result};
use crate::metadata::{
    Metadata, get_article_metadata, get_article_title, get_json_ld, text_similarity,
};
use crate::options::Options;
use crate::scoring::{
    get_comma_count, get_inner_text, get_link_density, has_child_block_element,
    has_single_tag_inside_element, initialize_node, is_element_without_content,
    is_phrasing_content, is_probably_visible, is_valid_byline,
};
use dom_query::{Document, Node, NodeId};
use regex::Regex;
use url::Url;

/// The extracted article content.
#[derive(Debug, Clone)]
pub struct Article {
    /// The article title.
    pub title: String,
    /// The author byline.
    pub byline: Option<String>,
    /// The text direction (ltr or rtl).
    pub dir: Option<String>,
    /// The document language.
    pub lang: Option<String>,
    /// The article content as HTML.
    pub content: String,
    /// The article content as plain text.
    pub text_content: String,
    /// The length of the text content.
    pub length: usize,
    /// A short excerpt from the article.
    pub excerpt: Option<String>,
    /// The site name.
    pub site_name: Option<String>,
    /// The published time.
    pub published_time: Option<String>,
}

/// The Readability parser.
pub struct Readability {
    doc: Document,
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
    document_uri: Option<Url>,
    attempts: Vec<AttemptResult>,
}

struct AttemptResult {
    content_html: String,
    text_length: usize,
}

/// Intermediate article content extracted by grab_article
struct ArticleContent {
    content_html: String,
    text_content: String,
    excerpt: Option<String>,
}

impl Readability {
    /// Create a new Readability parser for the given HTML.
    ///
    /// # Arguments
    /// * `html` - The HTML content to parse
    /// * `url` - Optional base URL for resolving relative links
    /// * `options` - Optional configuration options
    pub fn new(html: &str, url: Option<&str>, options: Option<Options>) -> Self {
        let doc = Document::from(html);
        let options = options.unwrap_or_default();

        let base_uri = url.and_then(|u| Url::parse(u).ok());
        let document_uri = base_uri.clone();

        Self {
            doc,
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
            document_uri,
            attempts: Vec::new(),
        }
    }

    /// Parse the document and extract the article content.
    pub fn parse(&mut self) -> Result<Article> {
        // Check element count limit
        if self.options.max_elems_to_parse > 0 {
            let count = self.doc.select("*").length();
            if count > self.options.max_elems_to_parse {
                return Err(ReadabilityError::TooManyElements(
                    count,
                    self.options.max_elems_to_parse,
                ));
            }
        }

        // Unwrap images from noscript tags
        unwrap_noscript_images(&self.doc);

        // Extract JSON-LD metadata before removing scripts
        let json_ld = if self.options.disable_json_ld {
            Metadata::default()
        } else {
            get_json_ld(&self.doc)
        };

        // Remove scripts
        remove_scripts(&self.doc);

        // Prepare document
        prep_document(&self.doc);

        // Get article title
        self.article_title = get_article_title(&self.doc);

        // Get metadata
        self.metadata = get_article_metadata(&self.doc, &json_ld, &self.article_title);
        if self.metadata.title.is_some() {
            self.article_title = self.metadata.title.clone().unwrap_or_default();
        }

        // Grab the article
        let article_content = self.grab_article()?;

        // Get excerpt if not in metadata
        let excerpt = self.metadata.excerpt.clone().or(article_content.excerpt);

        let length = article_content.text_content.len();

        Ok(Article {
            title: self.article_title.clone(),
            byline: self
                .metadata
                .byline
                .clone()
                .or_else(|| self.article_byline.clone()),
            dir: self.article_dir.clone(),
            lang: self.article_lang.clone(),
            content: article_content.content_html,
            text_content: article_content.text_content,
            length,
            excerpt,
            site_name: self
                .metadata
                .site_name
                .clone()
                .or_else(|| self.article_site_name.clone()),
            published_time: self.metadata.published_time.clone(),
        })
    }

    /// The main content extraction algorithm.
    fn grab_article(&mut self) -> Result<ArticleContent> {
        let body = self.doc.select("body");
        if body.length() == 0 {
            return Err(ReadabilityError::NoBody);
        }

        // Store original HTML for retry logic
        let page_cache_html = body.html().to_string();

        loop {
            self.log("Starting grabArticle loop");

            let strip_unlikely_candidates = self.flag_is_active(FLAG_STRIP_UNLIKELYS);

            // First, node prepping
            let mut elements_to_score: Vec<NodeId> = Vec::new();

            // Get the HTML element for language
            if let Some(html) = self.doc.select("html").nodes().first()
                && let Some(lang) = html.attr("lang")
            {
                self.article_lang = Some(lang.to_string());
            }

            // Track nodes to remove
            let mut nodes_to_remove: Vec<NodeId> = Vec::new();
            let mut should_remove_title_header = true;

            // First pass: identify nodes to remove and score
            let all_nodes: Vec<_> = self.doc.select("*").nodes().to_vec();

            for node in &all_nodes {
                if nodes_to_remove.contains(&node.id) {
                    continue;
                }

                let tag_name = get_tag_name(node).unwrap_or_default();
                let class = node
                    .attr("class")
                    .map(|s| s.to_string())
                    .unwrap_or_default();
                let id = node.attr("id").map(|s| s.to_string()).unwrap_or_default();
                let match_string = format!("{} {}", class, id);

                // Check visibility
                if !is_probably_visible(node) {
                    self.log(&format!("Removing hidden node - {}", match_string));
                    nodes_to_remove.push(node.id);
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
                    nodes_to_remove.push(node.id);
                    continue;
                }

                // Check for byline
                if self.article_byline.is_none()
                    && self.metadata.byline.is_none()
                    && is_valid_byline(node, &match_string)
                {
                    // Look for itemprop="name" child
                    let itemprop_name = node_select(node, "[itemprop*='name']")
                        .nodes()
                        .first()
                        .cloned();
                    let byline_node = itemprop_name.as_ref().unwrap_or(node);
                    self.article_byline = Some(byline_node.text().trim().to_string());
                    nodes_to_remove.push(node.id);
                    continue;
                }

                // Check for duplicate title header
                if should_remove_title_header && self.header_duplicates_title(node) {
                    self.log(&format!(
                        "Removing header: {} / {}",
                        node.text().trim(),
                        self.article_title.trim()
                    ));
                    should_remove_title_header = false;
                    nodes_to_remove.push(node.id);
                    continue;
                }

                // Remove unlikely candidates
                if strip_unlikely_candidates {
                    if regexps::UNLIKELY_CANDIDATES.is_match(&match_string)
                        && !regexps::OK_MAYBE_ITS_A_CANDIDATE.is_match(&match_string)
                        && !has_ancestor_tag(node, "table", 3, None::<fn(&Node) -> bool>)
                        && !has_ancestor_tag(node, "code", 3, None::<fn(&Node) -> bool>)
                        && tag_name != "BODY"
                        && tag_name != "A"
                    {
                        self.log(&format!("Removing unlikely candidate - {}", match_string));
                        nodes_to_remove.push(node.id);
                        continue;
                    }

                    if let Some(role) = node.attr("role")
                        && UNLIKELY_ROLES.contains(role.as_ref())
                    {
                        self.log(&format!(
                            "Removing content with role {} - {}",
                            role, match_string
                        ));
                        nodes_to_remove.push(node.id);
                        continue;
                    }
                }

                // Remove empty DIV, SECTION, HEADER, H1-H6
                if matches!(
                    tag_name.as_str(),
                    "DIV" | "SECTION" | "HEADER" | "H1" | "H2" | "H3" | "H4" | "H5" | "H6"
                ) && is_element_without_content(node)
                {
                    nodes_to_remove.push(node.id);
                    continue;
                }

                // Add to elements to score
                if DEFAULT_TAGS_TO_SCORE.contains(tag_name.as_str()) {
                    elements_to_score.push(node.id);
                }

                // Convert DIVs with only phrasing content to P
                if tag_name == "DIV" {
                    if has_single_tag_inside_element(node, "P") && get_link_density(node) < 0.25 {
                        elements_to_score.push(node.id);
                    } else if !has_child_block_element(node) {
                        node.rename("p");
                        elements_to_score.push(node.id);
                    }
                }
            }

            // Remove marked nodes
            {
                let all_nodes: Vec<_> = self.doc.select("*").nodes().to_vec();
                for node_id in &nodes_to_remove {
                    if let Some(node) = all_nodes.iter().find(|n| &n.id == node_id) {
                        node.remove_from_parent();
                    }
                }
            }

            // Score elements
            let mut candidates: Vec<NodeId> = Vec::new();

            for node_id in &elements_to_score {
                let all_nodes: Vec<_> = self.doc.select("*").nodes().to_vec();
                let node = match all_nodes.iter().find(|n| &n.id == node_id) {
                    Some(n) => *n,
                    None => continue,
                };

                let _parent = match node.parent() {
                    Some(p) if p.is_element() => p,
                    _ => continue,
                };

                let inner_text = get_inner_text(&node, true);
                if inner_text.len() < 25 {
                    continue;
                }

                // Get ancestors (up to 5 levels)
                let ancestors = get_ancestors(&node, 5);
                if ancestors.is_empty() {
                    continue;
                }

                // Calculate content score
                let mut content_score = 1.0;
                content_score += get_comma_count(&node) as f64;
                content_score += (inner_text.len() / 100).min(3) as f64;

                // Score ancestors
                for (level, ancestor) in ancestors.iter().enumerate() {
                    if !ancestor.is_element() {
                        continue;
                    }

                    if !ancestor.parent().map(|p| p.is_element()).unwrap_or(false) {
                        continue;
                    }

                    if !self.node_data.has(&ancestor.id) {
                        initialize_node(ancestor, &mut self.node_data, self.flags);
                        candidates.push(ancestor.id);
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
            let mut top_candidates: Vec<(NodeId, f64)> = Vec::new();

            for candidate_id in &candidates {
                let all_nodes: Vec<_> = self.doc.select("*").nodes().to_vec();
                let candidate = match all_nodes.iter().find(|n| &n.id == candidate_id) {
                    Some(c) => *c,
                    None => continue,
                };

                let score = self.node_data.get_content_score(candidate_id);
                let link_density = get_link_density(&candidate);
                let final_score = score * (1.0 - link_density);

                if let Some(data) = self.node_data.get_mut(candidate_id) {
                    data.content_score = final_score;
                }

                self.log(&format!("Candidate with score {:.2}", final_score));

                let mut inserted = false;
                for i in 0..self.options.nb_top_candidates.min(top_candidates.len() + 1) {
                    if i >= top_candidates.len() || final_score > top_candidates[i].1 {
                        top_candidates.insert(i, (*candidate_id, final_score));
                        inserted = true;
                        break;
                    }
                }
                if !inserted && top_candidates.len() < self.options.nb_top_candidates {
                    top_candidates.push((*candidate_id, final_score));
                }
                if top_candidates.len() > self.options.nb_top_candidates {
                    top_candidates.pop();
                }
            }

            // Get top candidate
            let (top_candidate_id, needed_to_create_top_candidate) = if top_candidates.is_empty() {
                let body = self.doc.select("body");
                if let Some(body_node) = body.nodes().first() {
                    initialize_node(body_node, &mut self.node_data, self.flags);
                    (Some(body_node.id), true)
                } else {
                    return Err(ReadabilityError::NoBody);
                }
            } else {
                let top_id = top_candidates[0].0;
                let all_nodes: Vec<_> = self.doc.select("*").nodes().to_vec();
                let mut top_candidate = all_nodes.iter().find(|n| n.id == top_id).cloned();

                if let Some(ref tc) = top_candidate {
                    let mut parent = tc.parent();
                    let top_score = self.node_data.get_content_score(&top_id);
                    let score_threshold = top_score / 3.0;

                    while let Some(p) = parent {
                        if let Some(ptag) = get_tag_name(&p)
                            && ptag == "BODY"
                        {
                            break;
                        }

                        if let Some(parent_data) = self.node_data.get(&p.id) {
                            if parent_data.content_score < score_threshold {
                                break;
                            }
                            if parent_data.content_score > top_score {
                                top_candidate = Some(p);
                                break;
                            }
                        }

                        parent = p.parent();
                    }

                    let mut current = top_candidate;
                    while let Some(c) = current {
                        if let Some(p) = c.parent() {
                            if let Some(ptag) = get_tag_name(&p)
                                && ptag == "BODY"
                            {
                                break;
                            }
                            if p.element_children().len() == 1 {
                                current = Some(p);
                            } else {
                                top_candidate = current;
                                break;
                            }
                        } else {
                            break;
                        }
                    }
                }

                (top_candidate.map(|tc| tc.id), false)
            };

            let top_candidate_id = top_candidate_id.ok_or(ReadabilityError::NoContent)?;

            // Get the article node ID to use
            let article_node_id = if needed_to_create_top_candidate {
                self.doc.select("body").nodes().first().map(|n| n.id)
            } else {
                Some(top_candidate_id)
            };

            let article_node_id = article_node_id.ok_or(ReadabilityError::NoContent)?;

            // Get article node by ID
            let all_nodes: Vec<_> = self.doc.select("*").nodes().to_vec();
            let article_node = all_nodes.into_iter().find(|n| n.id == article_node_id);

            // Prepare article content
            if let Some(article_node) = article_node {
                let video_regex = self
                    .options
                    .allowed_video_regex
                    .clone()
                    .unwrap_or_else(|| regexps::VIDEOS.clone());

                Self::prep_article(
                    &article_node,
                    &mut self.node_data,
                    self.flags,
                    &video_regex,
                    self.options.link_density_modifier,
                );

                // Re-fetch the article node after prep_article mutated the DOM
                let all_nodes_after: Vec<_> = self.doc.select("*").nodes().to_vec();
                let article_node = all_nodes_after
                    .into_iter()
                    .find(|n| n.id == article_node_id)
                    .ok_or(ReadabilityError::NoContent)?;

                let text_content = get_inner_text(&article_node, true);
                let text_length = text_content.len();

                if text_length < self.options.char_threshold {
                    self.attempts.push(AttemptResult {
                        content_html: article_node.html().to_string(),
                        text_length,
                    });

                    self.doc.select("body").set_html(page_cache_html.as_str());

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
                            return Err(ReadabilityError::NoContent);
                        }

                        // Use the best attempt - set its content as body and extract text/excerpt
                        let best_attempt = &self.attempts[0];
                        self.doc
                            .select("body")
                            .set_html(best_attempt.content_html.as_str());

                        // Re-fetch to get text and excerpt
                        if let Some(body) = self.doc.select("body").nodes().first().cloned() {
                            let text_content = get_inner_text(&body, true);
                            let excerpt = node_select(&body, "p")
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

                        return Err(ReadabilityError::NoContent);
                    }

                    self.node_data.clear();
                    continue;
                }

                // Find dir attribute from ancestors
                {
                    let all_nodes: Vec<_> = self.doc.select("*").nodes().to_vec();
                    if let Some(tc) = all_nodes.iter().find(|n| n.id == top_candidate_id) {
                        let ancestors = get_ancestors(tc, 0);
                        for ancestor in std::iter::once(*tc).chain(ancestors) {
                            if let Some(dir) = ancestor.attr("dir") {
                                self.article_dir = Some(dir.to_string());
                                break;
                            }
                        }
                    }
                }

                // Extract excerpt BEFORE post-processing modifies the DOM
                let excerpt = node_select(&article_node, "p")
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

            return Err(ReadabilityError::NoContent);
        }
    }

    /// Prepare the article for display.
    fn prep_article(
        article_content: &Node<'_>,
        node_data: &mut NodeDataStore,
        flags: u32,
        video_regex: &Regex,
        link_density_modifier: f64,
    ) {
        clean_styles(article_content);

        mark_data_tables(article_content, node_data);

        fix_lazy_images(article_content);

        clean_conditionally(
            article_content,
            "form",
            flags,
            video_regex,
            node_data,
            link_density_modifier,
        );
        clean_conditionally(
            article_content,
            "fieldset",
            flags,
            video_regex,
            node_data,
            link_density_modifier,
        );
        clean(article_content, "object", video_regex);
        clean(article_content, "embed", video_regex);
        clean(article_content, "footer", video_regex);
        clean(article_content, "link", video_regex);
        clean(article_content, "aside", video_regex);

        let share_threshold = crate::constants::defaults::DEFAULT_CHAR_THRESHOLD;
        for child in article_content.element_children() {
            clean_matched_nodes(&child, |node, match_string| {
                regexps::SHARE_ELEMENTS.is_match(match_string)
                    && node.text().len() < share_threshold
            });
        }

        clean(article_content, "iframe", video_regex);
        clean(article_content, "input", video_regex);
        clean(article_content, "textarea", video_regex);
        clean(article_content, "select", video_regex);
        clean(article_content, "button", video_regex);

        clean_headers(article_content, flags);

        clean_conditionally(
            article_content,
            "table",
            flags,
            video_regex,
            node_data,
            link_density_modifier,
        );
        clean_conditionally(
            article_content,
            "ul",
            flags,
            video_regex,
            node_data,
            link_density_modifier,
        );
        clean_conditionally(
            article_content,
            "div",
            flags,
            video_regex,
            node_data,
            link_density_modifier,
        );

        for h1 in node_select(article_content, "h1").nodes().iter() {
            h1.rename("h2");
        }

        let empty_ps: Vec<_> = node_select(article_content, "p")
            .nodes()
            .iter()
            .filter(|p| {
                let has_media = node_select(p, "img, embed, object, iframe").length() > 0;
                let has_text = !get_inner_text(p, false).is_empty();
                !has_media && !has_text
            })
            .map(|p| p.id)
            .collect();

        for p in node_select(article_content, "p").nodes().iter() {
            if empty_ps.contains(&p.id) {
                p.remove_from_parent();
            }
        }

        for br in node_select(article_content, "br").nodes().iter() {
            if let Some(next) = br.next_sibling()
                && next.is_element()
                && let Some(tag) = get_tag_name(&next)
                && tag == "P"
            {
                br.remove_from_parent();
            }
        }

        let tables: Vec<_> = node_select(article_content, "table").nodes().to_vec();
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
                let cell_html = cell.inner_html();
                table.set_html(cell_html.as_ref());
            }
        }
    }

    /// Post-process the extracted content from a Node.
    fn post_process_content_node(&self, node: &Node<'_>) -> String {
        self.fix_relative_uris(node);

        simplify_nested_elements(node);

        if !self.options.keep_classes {
            clean_classes(node, &self.options.classes_to_preserve);
        }

        node.inner_html().to_string()
    }

    /// Convert relative URIs to absolute.
    fn fix_relative_uris(&self, article_content: &Node<'_>) {
        let base_uri = match &self.base_uri {
            Some(u) => u,
            None => return,
        };

        for link in node_select(article_content, "a").nodes().iter() {
            if let Some(href) = link.attr("href") {
                if href.starts_with('#') && self.base_uri == self.document_uri {
                    continue;
                }

                if href.starts_with("javascript:") {
                    let text = link.text();
                    let escaped = html_escape(&text);
                    link.set_html(escaped.as_str());
                    link.rename("span");
                    continue;
                }

                if let Ok(absolute) = base_uri.join(href.as_ref()) {
                    link.set_attr("href", absolute.as_str());
                }
            }
        }

        for media in node_select(
            article_content,
            "img, picture, figure, video, audio, source",
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

    fn log(&self, msg: &str) {
        if self.options.debug {
            eprintln!("Reader: (Readability) {}", msg);
        }
    }
}

/// Get ancestors of a node up to max_depth (0 = unlimited).
fn get_ancestors<'a>(node: &Node<'a>, max_depth: usize) -> Vec<Node<'a>> {
    let mut ancestors = Vec::new();
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

/// Escape HTML special characters.
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
