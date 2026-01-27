//! Functions to determine if a document is probably readerable.

use crate::constants::regexps;
use crate::dom::get_tag_name;
use crate::options::ReaderableOptions;
use crate::scoring::is_probably_visible;
use dom_query::{Document, NodeId};
use std::collections::HashSet;

/// Check if a document is probably readerable without parsing the whole thing.
///
/// This is a quick check to determine if `Readability::parse()` is likely to succeed.
///
/// # Arguments
/// * `html` - The HTML content to check
/// * `options` - Optional configuration for the check
///
/// # Returns
/// `true` if the document appears to be readerable, `false` otherwise
pub fn is_probably_readerable(html: &str, options: Option<ReaderableOptions>) -> bool {
    let options = options.unwrap_or_default();
    let doc = Document::from(html);

    // Get initial nodes: p, pre, article
    let mut node_ids: HashSet<NodeId> = doc
        .select("p, pre, article")
        .nodes()
        .iter()
        .map(|n| n.id)
        .collect();

    // Add parent divs of br elements
    for br in doc.select("div > br").nodes().iter() {
        if let Some(parent) = br.parent() {
            node_ids.insert(parent.id);
        }
    }

    let mut score = 0.0;

    // Iterate only over the nodes we've collected (p, pre, article, and parent divs of br)
    let all_nodes: Vec<_> = doc.select("*").nodes().to_vec();
    for node in all_nodes.iter().filter(|n| node_ids.contains(&n.id)) {
        // Check visibility
        if !is_probably_visible(node) {
            continue;
        }

        // Check class/id against unlikely patterns
        let class = node
            .attr("class")
            .map(|s| s.to_string())
            .unwrap_or_default();
        let id = node.attr("id").map(|s| s.to_string()).unwrap_or_default();
        let match_string = format!("{} {}", class, id);

        if regexps::UNLIKELY_CANDIDATES.is_match(&match_string)
            && !regexps::OK_MAYBE_ITS_A_CANDIDATE.is_match(&match_string)
        {
            continue;
        }

        // Check if li > p (skip list item paragraphs)
        let is_li_p = {
            let mut parent = node.parent();
            let mut result = false;
            while let Some(p) = parent {
                if let Some(tag) = get_tag_name(&p)
                    && tag == "LI"
                {
                    result = true;
                    break;
                }
                parent = p.parent();
            }
            result
        };

        if is_li_p {
            continue;
        }

        // Check text content length
        let text_content = node.text().trim().to_string();
        let text_length = text_content.len();

        if text_length < options.min_content_length {
            continue;
        }

        // Add to score based on content length
        score += ((text_length - options.min_content_length) as f64).sqrt();

        if score > options.min_score {
            return true;
        }
    }

    false
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
