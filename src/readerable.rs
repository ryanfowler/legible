//! Functions to determine if a document is probably readerable.

use crate::constants::regexps;
use crate::dom::{build_match_string, has_tag_name};
use crate::options::ReaderableOptions;
use crate::scoring::is_probably_visible;
use dom_query::{Document, Node, NodeId};
use hashbrown::HashSet;

/// Check if a document is probably readerable without parsing the whole thing.
///
/// This is a quick heuristic check to determine if [`parse()`](crate::parse)
/// is likely to succeed. If you want to check readability before parsing, use
/// [`Document`] to avoid parsing the HTML twice.
///
/// The function scores paragraph-like elements based on their text length, ignoring
/// elements that match unlikely candidate patterns (sidebars, navigation, etc.).
///
/// # Arguments
///
/// * `html` - The HTML content to check
/// * `options` - Optional [`ReaderableOptions`] to customize the check
///
/// # Returns
///
/// `true` if the document appears to contain readable article content, `false` otherwise.
///
/// # Example
///
/// ```rust
/// use legible::is_probably_readerable;
///
/// let html = r#"
///     <html><body>
///         <article>
///             <p>This is a substantial article with enough content
///             to be considered readable by the algorithm.</p>
///         </article>
///     </body></html>
/// "#;
///
/// if is_probably_readerable(html, None) {
///     println!("Document is likely readerable");
/// }
/// ```
pub fn is_probably_readerable(html: &str, options: Option<ReaderableOptions>) -> bool {
    let doc = Document::from(html);
    is_probably_readerable_doc(&doc, options)
}

pub(crate) fn is_probably_readerable_doc(
    doc: &Document,
    options: Option<ReaderableOptions>,
) -> bool {
    let options = options.unwrap_or_default();

    let mut score = 0.0;
    let mut seen_div_ids: HashSet<NodeId> = HashSet::new();

    // Reusable buffer for match_string to avoid allocations per node
    let mut match_string_buf = String::with_capacity(128);

    for node in doc.root().descendants_it().filter(|node| node.is_element()) {
        if has_tag_name(&node, "p") || has_tag_name(&node, "pre") || has_tag_name(&node, "article")
        {
            if score_readerable_node(&node, &options, &mut score, &mut match_string_buf) {
                return true;
            }
            continue;
        }

        if has_tag_name(&node, "br")
            && let Some(parent) = node.parent()
            && has_tag_name(&parent, "div")
            && seen_div_ids.insert(parent.id)
            && score_readerable_node(&parent, &options, &mut score, &mut match_string_buf)
        {
            return true;
        }
    }

    false
}

fn score_readerable_node(
    node: &Node<'_>,
    options: &ReaderableOptions,
    score: &mut f64,
    match_string_buf: &mut String,
) -> bool {
    // Check visibility (no match_string needed)
    if !is_probably_visible(node) {
        return false;
    }

    // Build match_string lazily — only needed for regex check
    build_match_string(node, match_string_buf);

    // Use RegexSet for single-pass matching of both patterns
    let candidate_matches = regexps::CANDIDATE_FILTER_SET.matches(match_string_buf);
    if candidate_matches.matched(0) && !candidate_matches.matched(1) {
        return false;
    }

    // Check if li > p (skip list item paragraphs)
    let mut parent = node.parent();
    while let Some(p) = parent {
        if has_tag_name(&p, "li") {
            return false;
        }
        parent = p.parent();
    }

    // Check text content length
    let text_length = node.normalized_char_count();

    if text_length < options.min_content_length {
        return false;
    }

    // Add to score based on content length
    *score += ((text_length - options.min_content_length) as f64).sqrt();

    *score > options.min_score
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_short_content_not_readerable() {
        let html = "<html><body><p>Short</p></body></html>";
        assert!(!is_probably_readerable(html, None));
    }

    #[test]
    fn test_long_content_is_readerable() {
        // Need sqrt(text_len - 140) > 20, so text_len > 540
        let long_text = "a".repeat(600);
        let html = format!("<html><body><p>{}</p></body></html>", long_text);
        assert!(is_probably_readerable(&html, None));
    }

    #[test]
    fn test_unlikely_candidates_ignored() {
        // Even long content is rejected if it has unlikely candidate class
        let long_text = "a".repeat(600);
        let html = format!(
            "<html><body><p class=\"sidebar\">{}</p></body></html>",
            long_text
        );
        assert!(!is_probably_readerable(&html, None));
    }

    #[test]
    fn test_article_tag_helps() {
        // Same scoring rules apply - article tag helps collect nodes but doesn't change scoring
        let text = "a".repeat(600);
        let html = format!("<html><body><article>{}</article></body></html>", text);
        assert!(is_probably_readerable(&html, None));
    }
}
