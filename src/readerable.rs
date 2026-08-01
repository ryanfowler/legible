//! Functions to determine if a document is probably readerable.
#![allow(clippy::collapsible_if)]
use crate::constants::regexps;
use crate::document::Document;
use crate::dom::{Dom, NodeId, Tag, build_match_string};
use crate::options::ReaderableOptions;
use crate::scoring::is_probably_visible;
use std::collections::HashSet;

/// Check if an HTML document probably contains readable article content.
///
/// This quick heuristic scores visible paragraph-like elements by text length. It ignores
/// elements that look like navigation, sidebars, or other non-content sections.
///
/// Use [`Document`](crate::Document) if you plan to extract the article after this check.
/// It avoids parsing the HTML twice.
///
/// # Arguments
///
/// * `html` - The HTML content to check.
/// * `options` - Optional settings for the heuristic.
///
/// # Example
///
/// ```rust
/// use legible::is_probably_readerable;
///
/// let html = "<article><p>This is a substantial article with enough content for the check.</p></article>";
/// if is_probably_readerable(html, None) {
///     println!("Document is likely readerable");
/// }
/// ```
pub fn is_probably_readerable(html: &str, options: Option<ReaderableOptions>) -> bool {
    Document::new(html).is_probably_readerable(options)
}
pub(crate) fn is_probably_readerable_doc(dom: &Dom, options: Option<ReaderableOptions>) -> bool {
    let options = options.unwrap_or_default();
    let mut score = 0.0;
    let mut seen = HashSet::new();
    let mut buf = String::with_capacity(128);
    for id in dom.descendants(dom.root()) {
        if !dom.is_element(id) {
            continue;
        }
        match dom.tag(id) {
            Some(Tag::P | Tag::Pre | Tag::Article) => {
                if score_node(dom, id, &options, &mut score, &mut buf) {
                    return true;
                }
            }
            Some(Tag::Br) => {
                if let Some(p) = dom.parent(id) {
                    if dom.tag(p) == Some(Tag::Div)
                        && seen.insert(p)
                        && score_node(dom, p, &options, &mut score, &mut buf)
                    {
                        return true;
                    }
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
) -> bool {
    if !is_probably_visible(dom, id) {
        return false;
    }
    build_match_string(dom, id, buf);
    let m = regexps::CANDIDATE_FILTER_SET.matches(buf);
    if m.matched(0) && !m.matched(1) {
        return false;
    }
    let mut p = dom.parent(id);
    while let Some(x) = p {
        if dom.tag(x) == Some(Tag::Li) {
            return false;
        }
        p = dom.parent(x)
    }
    let len = dom.normalized_char_count(id);
    if len < o.min_content_length {
        return false;
    }
    *score += ((len - o.min_content_length) as f64).sqrt();
    *score > o.min_score
}
