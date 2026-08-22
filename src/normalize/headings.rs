use crate::dom::{AttrName, Dom, NodeId, Tag};
use crate::tokens::has_token;
use smallvec::SmallVec;

/// Removes heading controls and named placeholders that are irrelevant to quality metrics.
#[cfg(test)]
pub(super) fn remove_artifacts(dom: &mut Dom, root: NodeId) {
    let nodes = dom.element_descendants_snapshot_with_depth(root);
    let source_nodes: Vec<_> = std::iter::once(root).chain(dom.descendants(root)).collect();
    remove_artifacts_with_snapshot(dom, root, &nodes, &source_nodes);
}

pub(super) fn remove_artifacts_with_snapshot(
    dom: &mut Dom,
    root: NodeId,
    nodes: &[(NodeId, u32)],
    source_nodes: &[NodeId],
) {
    // The compiler also ignores permalink controls when it compiles an
    // arbitrary source fragment. Remove them here so source-relative result metrics
    // measure the same visible heading content during extraction retries. Use
    // one ancestry index instead of scanning each heading subtree.
    if nodes.iter().any(|&(node, _)| dom.tag(node) == Some(Tag::A)) {
        let heading_permalinks = crate::document::heading_permalink_nodes(dom, source_nodes);
        let mut nearest_heading = vec![None; dom.len()];
        let mut affected_headings = vec![false; dom.len()];
        nearest_heading[root.index()] = heading_level(dom, root).map(|_| root);
        let mut permalinks = SmallVec::<[NodeId; 8]>::new();
        for &(node, _) in nodes {
            nearest_heading[node.index()] = if heading_level(dom, node).is_some() {
                Some(node)
            } else {
                dom.parent(node)
                    .and_then(|parent| nearest_heading[parent.index()])
            };
            if let Some(heading) = nearest_heading[node.index()]
                && heading_permalinks[node.index()]
            {
                affected_headings[heading.index()] = true;
                permalinks.push(node);
            }
        }
        for permalink in permalinks {
            dom.detach(permalink);
        }

        let mut first_text = vec![None; dom.len()];
        let mut last_text = vec![None; dom.len()];
        for node in dom.descendants(root) {
            let Some(parent) = dom.parent(node) else {
                continue;
            };
            let Some(heading) = nearest_heading[parent.index()] else {
                continue;
            };
            if affected_headings[heading.index()] && dom.text_node(node).is_some() {
                first_text[heading.index()].get_or_insert(node);
                last_text[heading.index()] = Some(node);
            }
        }
        for (heading, affected) in affected_headings.into_iter().enumerate() {
            if !affected {
                continue;
            }
            if let Some(node) = first_text[heading] {
                let value = dom
                    .text_node(node)
                    .unwrap_or_default()
                    .trim_start()
                    .to_owned();
                dom.set_text(node, &value);
            }
            if let Some(node) = last_text[heading] {
                let value = dom
                    .text_node(node)
                    .unwrap_or_default()
                    .trim_end()
                    .to_owned();
                dom.set_text(node, &value);
            }
        }
    }

    // A named trailing placeholder is an obvious source artifact. Do not
    // remove an ordinary final heading because index pages can end in one.
    for &(heading, _) in nodes.iter().rev() {
        if dom.parent(heading).is_some()
            && heading_level(dom, heading).is_some()
            && is_trailing_artifact(dom, heading)
        {
            dom.detach(heading);
        }
    }
}

#[cfg(test)]
pub(super) fn normalize_roles(dom: &mut Dom, root: NodeId) {
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

fn has_role(dom: &Dom, node: NodeId, expected: &str) -> bool {
    dom.attr(node, AttrName::Role)
        .is_some_and(|roles| has_token(roles, expected))
}

#[cfg(test)]
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

pub(crate) fn heading_level(dom: &Dom, node: NodeId) -> Option<u8> {
    match dom.tag(node) {
        Some(Tag::H1) => Some(1),
        Some(Tag::H2) => Some(2),
        Some(Tag::H3) => Some(3),
        Some(Tag::H4) => Some(4),
        Some(Tag::H5) => Some(5),
        Some(Tag::H6) => Some(6),
        _ => dom
            .attr(node, AttrName::Role)
            .filter(|roles| has_token(roles, "heading"))
            .and_then(|_| dom.attr_by_local_name(node, "aria-level"))
            .and_then(|level| level.trim().parse::<u8>().ok())
            .filter(|level| (1..=6).contains(level)),
    }
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
        remove_artifacts(&mut dom, root);
        (dom, root)
    }

    #[test]
    fn removes_aria_heading_permalink_without_rewriting_the_role() {
        let (dom, root) = normalized(
            r##"<div role="heading" aria-level="2">Setup <a href="#setup">#</a></div>"##,
        );
        assert!(dom.first_descendant_by_tag(root, Tag::H2).is_none());
        assert_eq!(dom.text(root).trim(), "Setup");
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
    fn nested_heading_cleanup_uses_a_linear_ancestry_index() {
        const DEPTH: usize = 5_000;
        let mut html = r#"<div role="heading" aria-level="2">"#.repeat(DEPTH);
        html.push_str(r##"Title<a href="#title">#</a>"##);
        html.push_str(&"</div>".repeat(DEPTH));
        let (dom, root) = normalized(&html);
        assert_eq!(
            dom.descendants(root)
                .filter(|&node| dom.tag(node) == Some(Tag::A))
                .count(),
            0
        );
    }

    #[test]
    fn removes_only_named_trailing_heading_artifacts() {
        let (dom, root) = normalized(
            r#"<h2>Kept final heading</h2><div role="heading" aria-level="3" class="trailing-heading">Placeholder</div>"#,
        );
        assert_eq!(dom.text(root), "Kept final heading");
    }
}
