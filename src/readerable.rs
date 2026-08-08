//! The quick check for readable article content.
#![allow(clippy::collapsible_if)]
use crate::constants::regexps;
use crate::document::Document;
use crate::dom::{Dom, NodeId, Tag, build_match_string};
use crate::options::ReaderableOptions;
use crate::scoring::is_probably_visible;

/// Checks if an HTML document probably contains readable article content.
///
/// This function parses the HTML and runs a quick heuristic. The heuristic scores
/// visible paragraph-like elements by text length. It ignores elements that look like
/// navigation, sidebars, or other unrelated sections.
///
/// A `true` result does not guarantee successful extraction. A `false` result does not
/// prove that the document has no article.
///
/// Use [`Document`](crate::Document) if you want to extract the article after this
/// check. A `Document` prevents a second HTML parse.
///
/// # Parameters
///
/// * `html` is the source HTML.
/// * `options` configures the heuristic. Default options apply if this value is `None`.
///
/// # Example
///
/// ```rust
/// use legible::is_probably_readerable;
///
/// let text = "Article text. ".repeat(30);
/// let html = format!("<article><p>{text}</p></article>");
/// if is_probably_readerable(&html, None) {
///     println!("The document probably contains an article.");
/// }
/// ```
pub fn is_probably_readerable(html: &str, options: Option<ReaderableOptions>) -> bool {
    Document::new(html).is_probably_readerable(options)
}

#[derive(Clone, Copy, Default)]
struct TextStats {
    total_length: usize,
    leading_whitespace: usize,
    trailing_whitespace: usize,
    has_non_whitespace: bool,
}

#[derive(Clone, Copy, Default)]
struct ReaderableNodeState {
    text: TextStats,
    under_list_item: bool,
    seen: bool,
}

/// Per-node state for the heuristic.
///
/// The old implementation computed text by walking every candidate subtree and
/// searched a growing list for BR parents. Keeping this state indexed by NodeId makes
/// both operations linear in the DOM size.
struct ReaderableState {
    entries: Vec<ReaderableNodeState>,
}

impl ReaderableState {
    fn new(dom: &Dom) -> Self {
        let mut entries = vec![ReaderableNodeState::default(); dom.len()];
        let mut nodes = Vec::with_capacity(dom.len());
        nodes.push(dom.root());
        nodes.extend(dom.descendants(dom.root()));

        // A reverse preorder pass is postorder for a tree. Each child state is ready
        // before its parent is combined, so every text node is visited once.
        for &id in nodes.iter().rev() {
            let mut text = dom.text_node(id).map(text_stats).unwrap_or_default();
            for child in dom.children(id) {
                append_text_stats(&mut text, entries[child.index()].text);
            }
            entries[id.index()].text = text;
        }

        // Propagate the list-item exclusion in preorder. This replaces an ancestor
        // walk for every paragraph candidate.
        for &id in &nodes {
            let under_list_item = dom.parent(id).is_some_and(|parent| {
                dom.tag(parent) == Some(Tag::Li) || entries[parent.index()].under_list_item
            });
            entries[id.index()].under_list_item = under_list_item;
        }

        Self { entries }
    }

    #[inline]
    fn text_length(&self, id: NodeId) -> usize {
        let text = self.entries[id.index()].text;
        text.total_length
            .saturating_sub(text.leading_whitespace)
            .saturating_sub(text.trailing_whitespace)
    }

    #[inline]
    fn mark_seen(&mut self, id: NodeId) -> bool {
        let seen = &mut self.entries[id.index()].seen;
        if *seen {
            false
        } else {
            *seen = true;
            true
        }
    }
}

fn text_stats(text: &str) -> TextStats {
    let mut stats = TextStats::default();

    // Most article text is ASCII. Avoid UTF-8 decoding on the common path. Count
    // UTF-16 units to preserve the JavaScript heuristic's length semantics.
    if text.is_ascii() {
        for &byte in text.as_bytes() {
            stats.total_length += 1;
            if byte.is_ascii_whitespace() {
                if !stats.has_non_whitespace {
                    stats.leading_whitespace += 1;
                }
                stats.trailing_whitespace += 1;
            } else {
                stats.has_non_whitespace = true;
                stats.trailing_whitespace = 0;
            }
        }
    } else {
        for c in text.chars() {
            let length = c.len_utf16();
            stats.total_length += length;
            if c.is_whitespace() {
                if !stats.has_non_whitespace {
                    stats.leading_whitespace += length;
                }
                stats.trailing_whitespace += length;
            } else {
                stats.has_non_whitespace = true;
                stats.trailing_whitespace = 0;
            }
        }
    }
    stats
}

fn append_text_stats(a: &mut TextStats, b: TextStats) {
    if !b.has_non_whitespace {
        a.total_length += b.total_length;
        if !a.has_non_whitespace {
            a.leading_whitespace += b.total_length;
        }
        a.trailing_whitespace += b.total_length;
        return;
    }
    if !a.has_non_whitespace {
        a.leading_whitespace += b.leading_whitespace;
    }
    a.total_length += b.total_length;
    a.trailing_whitespace = b.trailing_whitespace;
    a.has_non_whitespace = true;
}

pub(crate) fn is_probably_readerable_doc(dom: &Dom, options: Option<ReaderableOptions>) -> bool {
    let options = options.unwrap_or_default();
    let mut state = ReaderableState::new(dom);
    let mut score = 0.0;
    let mut buf = String::with_capacity(128);
    for id in dom.descendants(dom.root()) {
        if !dom.is_element(id) {
            continue;
        }
        match dom.tag(id) {
            Some(Tag::P | Tag::Pre | Tag::Article) => {
                if score_node(dom, id, &options, &mut score, &mut buf, &state) {
                    return true;
                }
            }
            Some(Tag::Br) => {
                if let Some(p) = dom.parent(id)
                    && dom.tag(p) == Some(Tag::Div)
                    && state.mark_seen(p)
                    && score_node(dom, p, &options, &mut score, &mut buf, &state)
                {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

fn score_node(
    dom: &Dom,
    id: NodeId,
    o: &ReaderableOptions,
    score: &mut f64,
    buf: &mut String,
    state: &ReaderableState,
) -> bool {
    if !is_probably_visible(dom, id) {
        return false;
    }
    build_match_string(dom, id, buf);
    let m = regexps::CANDIDATE_FILTER_SET.matches(buf);
    if m.matched(0) && !m.matched(1) {
        return false;
    }
    if dom.tag(id) == Some(Tag::P) && state.entries[id.index()].under_list_item {
        return false;
    }
    let len = state.text_length(id);
    if len < o.min_content_length {
        return false;
    }
    *score += ((len - o.min_content_length) as f64).sqrt();
    *score > o.min_score
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reference_trimmed_text_utf16_len(dom: &Dom, root: NodeId) -> usize {
        let mut count = 0;
        let mut pending_whitespace = 0;
        for id in std::iter::once(root).chain(dom.descendants(root)) {
            let Some(text) = dom.text_node(id) else {
                continue;
            };
            if text.is_ascii() {
                for &byte in text.as_bytes() {
                    if byte.is_ascii_whitespace() {
                        if count > 0 {
                            pending_whitespace += 1;
                        }
                    } else {
                        count += pending_whitespace + 1;
                        pending_whitespace = 0;
                    }
                }
            } else {
                for c in text.chars() {
                    if c.is_whitespace() {
                        if count > 0 {
                            pending_whitespace += c.len_utf16();
                        }
                    } else {
                        count += pending_whitespace + c.len_utf16();
                        pending_whitespace = 0;
                    }
                }
            }
        }
        count
    }

    #[test]
    fn counts_surrogate_pairs_as_two_readerable_characters() {
        let options = ReaderableOptions::new()
            .min_content_length(2)
            .min_score(-0.1);
        assert!(is_probably_readerable("<p>😀</p>", Some(options)));
    }

    #[test]
    fn caches_trimmed_text_across_nested_candidates() {
        let dom = Dom::parse_document("<article>  <div>one</div>  </article>").unwrap();
        let state = ReaderableState::new(&dom);
        let article = dom
            .descendants(dom.root())
            .find(|&id| dom.tag(id) == Some(Tag::Article))
            .unwrap();
        assert_eq!(state.text_length(article), 3);
    }

    #[test]
    fn aggregates_whitespace_across_sibling_text_nodes() {
        let dom = Dom::parse_document(
            "<article> \n<div>\u{2003}one</div>\t<span>two\u{a0}</span> </article>",
        )
        .unwrap();
        let state = ReaderableState::new(&dom);
        let article = dom
            .descendants(dom.root())
            .find(|&id| dom.tag(id) == Some(Tag::Article))
            .unwrap();
        assert_eq!(state.text_length(article), 7);
    }

    #[test]
    fn cached_text_lengths_match_the_subtree_scan() {
        for html in [
            "<article> leading <div>nested</div><!-- comment --> trailing</article>",
            "<article>one<div>two</article> three",
            "<article>\u{2003}one<span>😀</span>\u{a0}</article>",
        ] {
            let dom = Dom::parse_document(html).unwrap();
            let state = ReaderableState::new(&dom);
            let ids: Vec<_> = std::iter::once(dom.root())
                .chain(dom.descendants(dom.root()))
                .collect();
            for id in ids {
                assert_eq!(
                    state.text_length(id),
                    reference_trimmed_text_utf16_len(&dom, id),
                    "mismatch for {html:?} at {id:?}",
                );
            }
        }
    }

    #[test]
    fn excludes_paragraphs_under_list_items_without_an_ancestor_scan() {
        let dom = Dom::parse_document("<li><div><p>text</p></div></li>").unwrap();
        let state = ReaderableState::new(&dom);
        let paragraph = dom
            .descendants(dom.root())
            .find(|&id| dom.tag(id) == Some(Tag::P))
            .unwrap();
        assert!(state.entries[paragraph.index()].under_list_item);
    }
}
