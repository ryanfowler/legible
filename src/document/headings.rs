//! Source heading-control recognition for semantic compilation and quality cleanup.

use crate::dom::{AttrName, Dom, NodeId, Tag};

use super::{facts::SemanticFacts, images::ImageAnalysis};

#[derive(Default)]
struct HeadingState {
    first_text: Option<NodeId>,
    last_text: Option<NodeId>,
    previous_character: Option<char>,
    next_character: Option<char>,
    meaningful: bool,
}

/// Adds heading and permalink decisions to the shared complex-source facts.
pub(super) fn analyze_complex(dom: &Dom, facts: &mut SemanticFacts, images: &ImageAnalysis) {
    if facts.inventory().headings.is_empty() {
        return;
    }

    if facts.inventory().fragment_links.is_empty() {
        let headings = facts.inventory().headings.clone();
        for heading in headings {
            facts.mark_heading_content(heading, facts.has_visible_text(heading));
        }
        let image_nodes = facts.inventory().images.clone();
        for image in image_nodes {
            if dom.tag(image) != Some(Tag::Img) || images.source(image).is_none() {
                continue;
            }
            if let Some(heading) = dom
                .ancestors(image)
                .find(|&ancestor| facts.heading_level(ancestor).is_some())
            {
                facts.mark_heading_content(heading, true);
            }
        }
        return;
    }

    let mut permalink_candidate = vec![false; dom.len()];
    let fragment_links = facts.inventory().fragment_links.clone();
    for node in fragment_links {
        let named = [AttrName::AriaLabel, AttrName::Title, AttrName::Class]
            .into_iter()
            .filter_map(|name| dom.attr(node, name))
            .any(|value| {
                let value = value.to_ascii_lowercase();
                value.contains("permalink")
                    || value.contains("anchor-link")
                    || value.contains("heading-anchor")
            });
        if facts.has_visible_text(node) && facts.glyph_only(node)
            || named && !facts.has_visible_text(node)
        {
            permalink_candidate[node.index()] = true;
        }
    }

    let headings = facts.inventory().headings.clone();
    let mut heading_slots = vec![u32::MAX; dom.len()];
    for (slot, &heading) in headings.iter().enumerate() {
        heading_slots[heading.index()] = slot as u32;
    }
    let mut owner = vec![u32::MAX; dom.len()];
    let mut inside_permalink = vec![false; dom.len()];
    let mut previous_word = vec![false; dom.len()];
    let nodes = facts.nodes().to_vec();
    let mut states = (0..headings.len())
        .map(|_| HeadingState::default())
        .collect::<Vec<_>>();

    for &node in &nodes {
        let own_heading = heading_slots[node.index()];
        owner[node.index()] = if own_heading != u32::MAX {
            own_heading
        } else {
            dom.parent(node)
                .map_or(u32::MAX, |parent| owner[parent.index()])
        };
        inside_permalink[node.index()] = permalink_candidate[node.index()]
            || dom
                .parent(node)
                .is_some_and(|parent| inside_permalink[parent.index()]);
        let slot = owner[node.index()];
        if slot == u32::MAX {
            continue;
        }
        let heading = headings[slot as usize];
        let state = &mut states[slot as usize];
        if inside_permalink[node.index()] {
            facts.mark_heading_has_permalink(heading);
            if permalink_candidate[node.index()] {
                facts.mark_heading_permalink(node);
                previous_word[node.index()] =
                    state.previous_character.is_some_and(char::is_alphanumeric);
            }
            continue;
        }
        if let Some(text) = dom.text_node(node) {
            state.first_text.get_or_insert(node);
            state.last_text = Some(node);
            state.meaningful |= facts.has_visible_text(node);
            if let Some(character) = text
                .chars()
                .rev()
                .find(|character| !character.is_whitespace())
            {
                state.previous_character = Some(character);
            }
        } else if dom.tag(node) == Some(Tag::Img) && images.source(node).is_some() {
            state.meaningful = true;
        }
    }

    for &node in nodes.iter().rev() {
        let slot = owner[node.index()];
        if slot == u32::MAX {
            continue;
        }
        let state = &mut states[slot as usize];
        if inside_permalink[node.index()] {
            if permalink_candidate[node.index()]
                && previous_word[node.index()]
                && state.next_character.is_some_and(char::is_alphanumeric)
            {
                facts.mark_permalink_separator(node);
            }
            continue;
        }
        if let Some(text) = dom.text_node(node)
            && let Some(character) = text.chars().find(|character| !character.is_whitespace())
        {
            state.next_character = Some(character);
        }
    }

    for (heading, state) in headings.into_iter().zip(states) {
        facts.mark_heading_content(heading, state.meaningful);
        if facts.heading_has_permalink(heading) {
            if let Some(first) = state.first_text {
                facts.mark_heading_trim_start(first);
            }
            if let Some(last) = state.last_text {
                facts.mark_heading_trim_end(last);
            }
        }
    }
}

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
