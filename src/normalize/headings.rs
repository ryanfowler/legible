use crate::dom::{AttrName, Dom, NodeId, Tag};
use smallvec::SmallVec;

pub(super) fn normalize(dom: &mut Dom, root: NodeId) {
    normalize_roles(dom, root);
    let nodes = dom.element_descendants_snapshot_with_depth(root);

    // Permalink controls are presentation. A normal link in a heading remains
    // content, including a fragment link whose label is not a permalink glyph.
    // Skip the per-heading subtree walk when the document has no anchors.
    let has_anchor = dom
        .descendants(root)
        .any(|node| dom.tag(node) == Some(Tag::A));
    if has_anchor {
        for &(heading, _) in &nodes {
            if dom.parent(heading).is_none() || !is_heading(dom.tag(heading)) {
                continue;
            }
            let links: SmallVec<[NodeId; 4]> = dom
                .descendants(heading)
                .filter(|&node| dom.tag(node) == Some(Tag::A) && is_permalink(dom, node))
                .collect();
            let removed_permalink = !links.is_empty();
            for link in links {
                dom.detach(link);
            }
            if removed_permalink {
                trim_heading_edges(dom, heading);
            }
        }
    }

    // A named trailing placeholder is an obvious source artifact. Do not
    // remove an ordinary final heading because index pages can end in one.
    for &(heading, _) in nodes.iter().rev() {
        if dom.parent(heading).is_some()
            && is_heading(dom.tag(heading))
            && is_trailing_artifact(dom, heading)
        {
            dom.detach(heading);
        }
    }
}

pub(super) fn normalize_roles(dom: &mut Dom, root: NodeId) {
    // Capture ARIA heading semantics before class and role cleanup. This small
    // pass is also safe to run on the scoring-only DOM.
    let nodes = dom.element_descendants_snapshot_with_depth(root);
    for &(node, _) in &nodes {
        if dom.parent(node).is_none() || !has_role(dom, node, "heading") {
            continue;
        }
        let Some(level) = dom
            .attr_by_local_name(node, "aria-level")
            .and_then(|value| value.trim().parse::<u8>().ok())
            .and_then(heading_tag)
        else {
            continue;
        };
        dom.rename_html(node, level);
    }
}

pub(super) fn has_primary_role(dom: &Dom, node: NodeId) -> bool {
    has_role(dom, node, "heading")
        && dom
            .attr_by_local_name(node, "aria-level")
            .and_then(|value| value.trim().parse::<u8>().ok())
            .is_some_and(|level| matches!(level, 1 | 2))
}

fn trim_heading_edges(dom: &mut Dom, heading: NodeId) {
    let text_nodes: SmallVec<[NodeId; 4]> = dom
        .descendants(heading)
        .filter(|&node| dom.text_node(node).is_some())
        .collect();
    let Some(&first) = text_nodes.first() else {
        return;
    };
    let first_value = dom
        .text_node(first)
        .unwrap_or_default()
        .trim_start()
        .to_owned();
    dom.set_text(first, &first_value);
    if let Some(&last) = text_nodes.last() {
        let last_value = dom
            .text_node(last)
            .unwrap_or_default()
            .trim_end()
            .to_owned();
        dom.set_text(last, &last_value);
    }
}

fn has_role(dom: &Dom, node: NodeId, expected: &str) -> bool {
    dom.attr(node, AttrName::Role).is_some_and(|roles| {
        roles
            .split_ascii_whitespace()
            .any(|role| role.eq_ignore_ascii_case(expected))
    })
}

fn heading_tag(level: u8) -> Option<Tag> {
    match level {
        1 => Some(Tag::H1),
        2 => Some(Tag::H2),
        3 => Some(Tag::H3),
        4 => Some(Tag::H4),
        5 => Some(Tag::H5),
        6 => Some(Tag::H6),
        _ => None,
    }
}

fn is_heading(tag: Option<Tag>) -> bool {
    matches!(
        tag,
        Some(Tag::H1 | Tag::H2 | Tag::H3 | Tag::H4 | Tag::H5 | Tag::H6)
    )
}

fn is_permalink(dom: &Dom, link: NodeId) -> bool {
    if !dom
        .attr(link, AttrName::Href)
        .is_some_and(|href| href.trim().starts_with('#'))
    {
        return false;
    }
    let label = dom.text(link);
    let glyph_only = !label.trim().is_empty()
        && label
            .trim()
            .chars()
            .all(|character| matches!(character, '#' | '¶' | '§' | '🔗' | ' '));
    let named_as_permalink = [AttrName::AriaLabel, AttrName::Title, AttrName::Class]
        .into_iter()
        .filter_map(|name| dom.attr(link, name))
        .any(|value| {
            let value = value.to_ascii_lowercase();
            value.contains("permalink")
                || value.contains("anchor-link")
                || value.contains("heading-anchor")
        });
    glyph_only || named_as_permalink && label.trim().is_empty()
}

fn is_trailing_artifact(dom: &Dom, heading: NodeId) -> bool {
    let named_as_artifact = [AttrName::Class, AttrName::Id]
        .into_iter()
        .filter_map(|name| dom.attr(heading, name))
        .flat_map(str::split_ascii_whitespace)
        .any(|token| {
            matches!(
                token.to_ascii_lowercase().as_str(),
                "empty-heading" | "heading-placeholder" | "trailing-heading"
            )
        });
    named_as_artifact && !has_meaningful_following_content(dom, heading)
}

fn has_meaningful_following_content(dom: &Dom, heading: NodeId) -> bool {
    let mut node = heading;
    while let Some(parent) = dom.parent(node) {
        let mut sibling = dom.next_sibling(node);
        while let Some(candidate) = sibling {
            if dom.has_non_whitespace_text(candidate)
                || dom.descendants(candidate).any(|descendant| {
                    matches!(dom.tag(descendant), Some(Tag::Img | Tag::Table | Tag::Pre))
                })
            {
                return true;
            }
            sibling = dom.next_sibling(candidate);
        }
        node = parent;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn normalized(html: &str) -> (Dom, NodeId) {
        let mut dom = Dom::parse_document(html).unwrap();
        let root = dom.body().unwrap();
        normalize(&mut dom, root);
        (dom, root)
    }

    #[test]
    fn converts_heading_roles_and_removes_permalinks() {
        let (dom, root) = normalized(
            r##"<div role="heading" aria-level="2">Setup <a href="#setup">#</a></div>"##,
        );
        let heading = dom.first_descendant_by_tag(root, Tag::H2).unwrap();
        assert_eq!(dom.text(heading).trim(), "Setup");
        assert!(dom.first_descendant_by_tag(root, Tag::A).is_none());
    }

    #[test]
    fn keeps_legitimate_heading_links() {
        for html in [
            r#"<h2><a href="/guide">Read the guide</a></h2>"#,
            r##"<h2><a class="heading-anchor" href="#guide">Read the guide</a></h2>"##,
        ] {
            let (dom, root) = normalized(html);
            assert_eq!(dom.text(root), "Read the guide");
            assert!(dom.first_descendant_by_tag(root, Tag::A).is_some());
        }
    }

    #[test]
    fn removes_only_named_trailing_heading_artifacts() {
        let (dom, root) = normalized(
            r#"<h2>Kept final heading</h2><h3 class="trailing-heading">Placeholder</h3>"#,
        );
        assert_eq!(dom.text(root), "Kept final heading");
    }
}
