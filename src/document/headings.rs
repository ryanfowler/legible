//! Source heading-control recognition for semantic compilation and quality cleanup.

use crate::dom::{AttrName, Dom, NodeId, Tag};

/// Identifies heading permalink controls with one reverse subtree analysis.
pub(crate) fn permalink_nodes(dom: &Dom, nodes: &[NodeId]) -> Vec<bool> {
    if !nodes.iter().any(|&node| {
        dom.tag(node) == Some(Tag::A)
            && dom
                .attr(node, AttrName::Href)
                .is_some_and(|href| href.trim().starts_with('#'))
    }) {
        return vec![false; dom.len()];
    }
    let mut has_visible_text = vec![false; dom.len()];
    let mut glyph_only = vec![true; dom.len()];
    for &node in nodes.iter().rev() {
        if let Some(text) = dom.text_node(node) {
            for character in text.chars().filter(|character| !character.is_whitespace()) {
                has_visible_text[node.index()] = true;
                glyph_only[node.index()] &= matches!(character, '#' | '¶' | '§' | '🔗');
            }
        } else {
            for child in dom.children(node) {
                if has_visible_text[child.index()] {
                    has_visible_text[node.index()] = true;
                    glyph_only[node.index()] &= glyph_only[child.index()];
                }
            }
        }
    }

    let mut permalinks = vec![false; dom.len()];
    for &node in nodes {
        if dom.tag(node) != Some(Tag::A)
            || !dom
                .attr(node, AttrName::Href)
                .is_some_and(|href| href.trim().starts_with('#'))
        {
            continue;
        }
        let named = [AttrName::AriaLabel, AttrName::Title, AttrName::Class]
            .into_iter()
            .filter_map(|name| dom.attr(node, name))
            .any(|value| {
                let value = value.to_ascii_lowercase();
                value.contains("permalink")
                    || value.contains("anchor-link")
                    || value.contains("heading-anchor")
            });
        permalinks[node.index()] = has_visible_text[node.index()] && glyph_only[node.index()]
            || named && !has_visible_text[node.index()];
    }
    permalinks
}
