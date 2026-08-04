//! Article extraction and its result type.
#![allow(clippy::collapsible_if)]
use crate::cleaning::*;
use crate::constants::{
    flags::*, is_alter_to_div_exception, is_default_tag_to_score, is_unlikely_role, regexps,
};
use crate::dom::{AttrName, Dom, NodeId, NodeStateStore, Tag, build_match_string};
use crate::error::{Error, Result};
use crate::logging::debug_log;
use crate::metadata::{self, Metadata};
use crate::options::Options;
use crate::scoring::*;
use regex::Regex;
use smallvec::SmallVec;
use url::Url;

/// Extracted article content and metadata.
///
/// Legible returns content in HTML, CommonMark, and normalized plain-text formats.
/// Metadata fields are `None` when Legible cannot find the applicable value.
///
/// # Example
///
/// ```rust
/// use legible::parse;
///
/// let html = "<html><body><article><h1>Title</h1><p>Article content.</p></article></body></html>";
///
/// if let Ok(article) = parse(html, None, None) {
///     println!("Title: {}", article.title);
///     println!("Author: {:?}", article.byline);
///     println!("HTML length: {} bytes", article.content.len());
///     println!("Text length: {} characters", article.length);
/// }
/// ```
///
/// # Security
///
/// **Do not render [`content`](Article::content) without sanitizing it.** Legible cleans
/// article content, but it is not an HTML security sanitizer.
///
/// [`markdown_content`](Article::markdown_content) does not contain raw HTML. It removes
/// destinations that have unsupported URI schemes. If you convert the Markdown to HTML,
/// sanitize that HTML according to your application's security policy.
///
/// ```rust,ignore
/// let safe_html = ammonia::clean(&article.content);
/// ```
#[derive(Debug, Clone)]
pub struct Article {
    /// Article title.
    ///
    /// This value can come from the `<title>` element, a heading, or page metadata.
    pub title: String,

    /// Author byline, if found.
    pub byline: Option<String>,

    /// Text direction from the source, such as `"ltr"` or `"rtl"`.
    pub dir: Option<String>,

    /// Document language from the source, such as `"en"` or `"fr"`.
    pub lang: Option<String>,

    /// Extracted article content as an HTML fragment.
    ///
    /// This HTML is not sanitized. It can contain unsafe attributes, URLs, or other
    /// source markup. Apply an HTML sanitizer before you render it.
    pub content: String,

    /// Extracted article content as normalized plain text.
    pub text_content: String,

    /// Extracted article content as CommonMark.
    ///
    /// Legible creates this value from the same document tree as
    /// [`content`](Article::content). It escapes source text and does not include raw
    /// HTML. Links can use HTTP, HTTPS, email, telephone, fragment, and relative
    /// destinations. Images can use HTTP, HTTPS, and relative destinations.
    pub markdown_content: String,

    /// Number of characters in [`text_content`](Article::text_content).
    pub length: usize,

    /// Short article excerpt, if found.
    pub excerpt: Option<String>,

    /// Site name, if found.
    pub site_name: Option<String>,

    /// Publication time from page metadata, if found.
    ///
    /// Legible does not validate or change the source format.
    pub published_time: Option<String>,
}
pub(crate) struct Readability<'a> {
    dom: Dom,
    original_html: &'a str,
    options: Options,
    flags: u32,
    node_data: NodeStateStore,
    article_title: String,
    article_byline: Option<String>,
    article_dir: Option<String>,
    article_lang: Option<String>,
    metadata: Metadata,
    base_uri: Option<Url>,
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
    content_html: String,
    text_content: String,
    text_length: usize,
    excerpt: Option<String>,
    /// The node whose children produce the serialized content.
    /// Valid in `self.dom` after `grab_article` returns.
    article_root: NodeId,
}
impl<'a> Readability<'a> {
    pub(crate) fn from_document(
        dom: Dom,
        original_html: &'a str,
        url: Option<&str>,
        options: Option<Options>,
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
            options: options.unwrap_or_default(),
            flags: FLAG_STRIP_UNLIKELYS | FLAG_WEIGHT_CLASSES | FLAG_CLEAN_CONDITIONALLY,
            node_data: NodeStateStore::new(),
            article_title: String::new(),
            article_byline: None,
            article_dir: None,
            article_lang: None,
            metadata: Metadata::default(),
            base_uri,
            url_error,
            best_attempt: None,
        }
    }
    pub(crate) fn parse(mut self) -> Result<Article> {
        if let Some(e) = self.url_error {
            return Err(Error::InvalidUrl(e));
        }
        if self.options.max_elems_to_parse > 0 {
            let n = self
                .dom
                .descendants(self.dom.root())
                .filter(|&x| self.dom.is_element(x))
                .count();
            if n > self.options.max_elems_to_parse {
                return Err(Error::TooManyElements(n, self.options.max_elems_to_parse));
            }
        }
        unwrap_noscript_images(&mut self.dom);
        let title = metadata::get_article_title(&self.dom);
        let json = if self.options.disable_json_ld {
            Metadata::default()
        } else {
            metadata::get_json_ld(&self.dom, &title)
        };
        remove_scripts(&mut self.dom);
        prep_document(&mut self.dom);
        self.article_title = title;
        self.metadata = metadata::get_article_metadata(&self.dom, &json, &self.article_title);
        if let Some(t) = self.metadata.title.take() {
            self.article_title = t
        }
        let content = self.grab_article()?;
        let excerpt = self.metadata.excerpt.take().or(content.excerpt);
        let markdown_content = crate::markdown::dom_to_markdown(
            &self.dom,
            content.article_root,
            content.text_content.len(),
        );
        Ok(Article {
            title: std::mem::take(&mut self.article_title),
            byline: self.metadata.byline.take().or(self.article_byline.take()),
            dir: self.article_dir.take(),
            lang: self.article_lang.take(),
            content: content.content_html,
            markdown_content,
            text_content: content.text_content,
            length: content.text_length,
            excerpt,
            site_name: self.metadata.site_name.take(),
            published_time: self.metadata.published_time.take(),
        })
    }
    fn grab_article(&mut self) -> Result<ArticleContent> {
        if self.dom.body().is_none() {
            return Err(Error::NoBody);
        }
        loop {
            let strip = self.flags & FLAG_STRIP_UNLIKELYS != 0;
            let mut to_score = SmallVec::<[NodeId; 256]>::new();
            if let Some(html) = self.dom.html_element() {
                if let Some(lang) = self.dom.attr(html, AttrName::Lang) {
                    self.article_lang = Some(lang.into())
                }
                if let Some(dir) = self.dom.attr(html, AttrName::Dir) {
                    self.article_dir = Some(dir.into())
                }
            }
            let mut remove = SmallVec::<[NodeId; 64]>::new();
            // Allocate dense state once. Scoring IDs follow document order and
            // would otherwise grow this vector several times during the pass.
            self.node_data.sync_len(self.dom.len());
            // Tree repair can make arena order differ from document order. Record
            // only attached elements in preorder before this pass starts mutating.
            // Snapshot depths identify removed subtrees without ancestor walks.
            let initial_nodes = self
                .dom
                .element_descendants_snapshot_with_depth(self.dom.root());
            let mut buf = String::new();
            let mut removed_depth = None;
            let mut remove_title = true;
            for (id, depth) in initial_nodes {
                if let Some(root_depth) = removed_depth {
                    if depth > root_depth {
                        continue;
                    }
                    removed_depth = None
                }
                let tag = self
                    .dom
                    .tag(id)
                    .expect("element snapshot must contain only elements");
                if !is_probably_visible(&self.dom, id) {
                    remove.push(id);
                    removed_depth = Some(depth);
                    continue;
                }
                if self.dom.attr(id, AttrName::AriaModal) == Some("true")
                    && self.dom.attr(id, AttrName::Role) == Some("dialog")
                {
                    remove.push(id);
                    removed_depth = Some(depth);
                    continue;
                }
                if self.article_byline.is_none() && self.metadata.byline.is_none() {
                    build_match_string(&self.dom, id, &mut buf);
                    if is_valid_byline(&self.dom, id, &buf) {
                        let mut names = Vec::new();
                        self.dom
                            .collect_attr_contains(id, AttrName::ItemProp, "name", &mut names);
                        let n = names.first().copied().unwrap_or(id);
                        self.article_byline =
                            Some(get_inner_text(&self.dom, n, false).trim().into());
                        remove.push(id);
                        removed_depth = Some(depth);
                        continue;
                    }
                }
                if remove_title
                    && matches!(tag, Tag::H1 | Tag::H2)
                    && metadata::text_similarity(
                        &self.article_title,
                        &get_inner_text(&self.dom, id, false),
                    ) > 0.75
                {
                    remove_title = false;
                    remove.push(id);
                    removed_depth = Some(depth);
                    continue;
                }
                if strip && tag != Tag::Body && tag != Tag::A {
                    build_match_string(&self.dom, id, &mut buf);
                    let m = regexps::CANDIDATE_FILTER_SET.matches(&buf);
                    if m.matched(0)
                        && !m.matched(1)
                        && !has_ancestor_tags_any(&self.dom, id, &[Tag::Table, Tag::Code], 3)
                    {
                        remove.push(id);
                        removed_depth = Some(depth);
                        continue;
                    }
                    if self
                        .dom
                        .attr(id, AttrName::Role)
                        .is_some_and(is_unlikely_role)
                    {
                        remove.push(id);
                        removed_depth = Some(depth);
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
                    remove.push(id);
                    removed_depth = Some(depth);
                    continue;
                }
                if is_default_tag_to_score(tag) && self.node_data.mark_score_seen(id) {
                    to_score.push(id)
                }
                if tag == Tag::Div {
                    wrap_phrasing_content_in_p(&mut self.dom, id);
                    if has_single_tag_inside_element(&self.dom, id, Tag::P)
                        && get_link_density(&self.dom, id) < 0.25
                    {
                        if let Some(p) = self.dom.element_children(id).next() {
                            let pid = p;
                            self.dom.replace_with(id, pid);
                            if self.node_data.mark_score_seen(pid) {
                                to_score.push(pid)
                            }
                        }
                    } else if !has_child_block_element(&self.dom, id) {
                        self.dom.rename_html(id, Tag::P);
                        if self.node_data.mark_score_seen(id) {
                            to_score.push(id)
                        }
                    } else {
                        for p in self
                            .dom
                            .element_children(id)
                            .filter(|&x| self.dom.tag(x) == Some(Tag::P))
                        {
                            if self.node_data.mark_score_seen(p) {
                                to_score.push(p)
                            }
                        }
                    }
                }
            }
            for id in remove {
                if self.dom.parent(id).is_some() {
                    self.dom.detach(id)
                }
            }
            self.node_data.sync_len(self.dom.len());
            let mut candidates = SmallVec::<[NodeId; 256]>::new();
            for id in to_score {
                let Some(parent) = self.dom.parent(id).filter(|&x| self.dom.is_element(x)) else {
                    continue;
                };
                let stats = get_or_compute_stats(&self.dom, id, &mut self.node_data);
                if stats.text_length < 25 {
                    continue;
                }
                let cs =
                    1.0 + (stats.comma_count + 1) as f64 + (stats.text_length / 100).min(3) as f64;
                let mut a = Some(parent);
                for level in 0..5 {
                    let Some(x) = a else { break };
                    a = self.dom.parent(x);
                    if !self.dom.is_element(x) || !a.is_some_and(|z| self.dom.is_element(z)) {
                        continue;
                    }
                    if Self::initialize_node_once(&self.dom, x, &mut self.node_data, self.flags) {
                        candidates.push(x)
                    }
                    let div = if level == 0 {
                        1.0
                    } else if level == 1 {
                        2.0
                    } else {
                        (level * 3) as f64
                    };
                    self.node_data.add_content_score(x, cs / div)
                }
            }
            let mut scored: SmallVec<[(NodeId, f64, usize); 64]> = candidates
                .iter()
                .enumerate()
                .map(|(order, &id)| {
                    let s = self.node_data.get_content_score(id);
                    let len = get_or_compute_stats(&self.dom, id, &mut self.node_data).text_length;
                    let d = get_link_density_cached(&self.dom, id, len, &mut self.node_data);
                    let f = s * (1.0 - d);
                    self.node_data.set_score(id, f);
                    (id, f, order)
                })
                .collect();
            let top_count = self.options.nb_top_candidates.min(scored.len());
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
            let top = scored;
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
            let video = self
                .options
                .allowed_video_regex
                .clone()
                .unwrap_or_else(|| regexps::VIDEOS.clone());
            self.prep_article(article_id, &video);
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
                    self.post_process(article_id);
                    let dom = self
                        .dom
                        .copy_subtree_as_fragment(article_id)
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
                let (text, _) = self.dom.normalized_text(root, best.text_len_chars);
                let mut html = String::new();
                self.dom
                    .serialize_children(root, &mut html)
                    .map_err(|_| Error::NoContent)?;
                return Ok(ArticleContent {
                    content_html: html,
                    text_content: text,
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
            self.post_process(article_id);
            let (text, len) = self
                .dom
                .normalized_text(article_id, self.options.char_threshold);
            let mut html = String::new();
            self.dom
                .serialize_children(article_id, &mut html)
                .map_err(|_| Error::NoContent)?;
            return Ok(ArticleContent {
                content_html: html,
                text_content: text,
                text_length: len,
                excerpt,
                article_root: article_id,
            });
        }
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
    fn prep_article(&mut self, root: NodeId, video: &Regex) {
        clean_styles(&mut self.dom, root);
        mark_data_tables(&self.dom, root, &mut self.node_data);
        fix_lazy_images(&mut self.dom, root);
        clean_conditionally(
            &mut self.dom,
            root,
            &[Tag::Form],
            Tag::Form,
            self.flags,
            video,
            &mut self.node_data,
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
            self.options.link_density_modifier,
        );
        clean_tags(
            &mut self.dom,
            root,
            &[Tag::Object, Tag::Embed, Tag::Footer, Tag::Link, Tag::Aside],
            video,
        );
        let threshold = crate::constants::defaults::DEFAULT_CHAR_THRESHOLD;
        let children: SmallVec<[NodeId; 16]> = self.dom.element_children(root).collect();
        for c in children {
            clean_matched_nodes(&mut self.dom, c, |d, id, m| {
                m.as_bytes()
                    .windows(5)
                    .any(|w| w.eq_ignore_ascii_case(b"share"))
                    && regexps::SHARE_ELEMENTS.is_match(m)
                    && get_inner_text(d, id, false).len() < threshold
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
        );
        clean_headers(&mut self.dom, root, self.flags);
        clean_conditionally(
            &mut self.dom,
            root,
            &[Tag::Table],
            Tag::Table,
            self.flags,
            video,
            &mut self.node_data,
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
            if self
                .dom
                .next_sibling(br)
                .is_some_and(|x| self.dom.is_element(x) && self.dom.tag(x) == Some(Tag::P))
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
            .map(|id| get_inner_text(&self.dom, id, false))
            .filter(|text| !text.is_empty())
    }
    fn post_process(&mut self, root: NodeId) {
        self.fix_relative_uris(root);
        simplify_nested_elements(&mut self.dom, root);
        let ids: Vec<_> = self.dom.descendants(root).collect();
        let mut comments = SmallVec::<[NodeId; 32]>::new();
        for id in ids {
            if self.dom.is_element(id) {
                if !self.options.keep_classes {
                    if let Some(c) = self.dom.attr(id, AttrName::Class) {
                        let keep = c
                            .split_whitespace()
                            .filter(|x| self.options.classes_to_preserve.iter().any(|p| p == x))
                            .collect::<Vec<_>>()
                            .join(" ");
                        if keep.is_empty() {
                            self.dom.remove_attr(id, AttrName::Class)
                        } else {
                            self.dom.set_attr(id, AttrName::Class, &keep)
                        }
                    }
                }
            } else if self.dom.is_comment(id) {
                comments.push(id)
            }
        }
        for x in comments {
            self.dom.detach(x)
        }
    }
    fn fix_relative_uris(&mut self, root: NodeId) {
        let Some(base) = self.base_uri.clone() else {
            return;
        };
        let links: Vec<_> = self
            .dom
            .descendants(root)
            .filter(|&x| self.dom.tag(x) == Some(Tag::A))
            .collect();
        for x in links {
            if let Some(h) = self.dom.attr(x, AttrName::Href).map(str::to_string) {
                if h.starts_with('#') {
                    continue;
                }
                if h.starts_with("javascript:") {
                    let text = self.dom.text(x);
                    self.dom.detach_children(x);
                    if let Ok(text_node) = self.dom.create_text(&text) {
                        self.dom.append_child(x, text_node)
                    }
                    self.dom.rename_html(x, Tag::Span)
                } else if let Ok(u) = base.join(&h) {
                    self.dom.set_attr(x, AttrName::Href, u.as_str())
                }
            }
        }
        let media: Vec<_> = self
            .dom
            .descendants(root)
            .filter(|&x| {
                matches!(
                    self.dom.tag(x),
                    Some(
                        Tag::Img
                            | Tag::Picture
                            | Tag::Figure
                            | Tag::Video
                            | Tag::Audio
                            | Tag::Source
                    )
                )
            })
            .collect();
        for x in media {
            for a in [AttrName::Src, AttrName::Poster] {
                if let Some(v) = self.dom.attr(x, a).map(str::to_string) {
                    if let Ok(u) = base.join(&v) {
                        self.dom.set_attr(x, a, u.as_str())
                    }
                }
            }
            if let Some(v) = self.dom.attr(x, AttrName::Srcset).map(str::to_string) {
                let n = regexps::SRCSET_URL.replace_all(&v, |c: &regex::Captures| {
                    let u = base
                        .join(&c[1])
                        .map(|x| x.to_string())
                        .unwrap_or_else(|_| c[1].into());
                    format!(
                        "{}{}{}",
                        u,
                        c.get(2).map_or("", |x| x.as_str()),
                        c.get(3).map_or("", |x| x.as_str())
                    )
                });
                self.dom.set_attr(x, AttrName::Srcset, &n)
            }
        }
    }
    fn reparse_prepare(&mut self) -> Result<()> {
        self.dom = Dom::parse_document(self.original_html).map_err(|_| Error::NoContent)?;
        unwrap_noscript_images(&mut self.dom);
        remove_scripts(&mut self.dom);
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
fn has_ancestor_tags_any(dom: &Dom, id: NodeId, tags: &[Tag], max: usize) -> bool {
    dom.ancestors(id)
        .take(max)
        .any(|x| dom.tag(x).is_some_and(|t| tags.contains(&t)))
}
