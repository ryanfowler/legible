use crate::dom::{AttrName, Dom, NodeId, Tag};
use crate::tokens::has_token;
use smallvec::SmallVec;

/// Rewrites ARIA lists only in the scoring DOM.
///
/// The semantic compiler reads these roles directly from selected content.
pub(super) fn normalize_for_scoring(dom: &mut Dom, root: NodeId) {
    let nodes = dom.element_descendants_snapshot_with_depth(root);
    let mut normalized_items = SmallVec::<[NodeId; 32]>::new();

    // Resolve list containers before items. This lets ordered-list detection
    // inspect the original explicit labels and keeps nested ARIA lists local.
    for &(node, _) in &nodes {
        if dom.parent(node).is_none() || !has_role(dom, node, "list") {
            continue;
        }
        let items: SmallVec<[NodeId; 16]> = dom
            .element_children(node)
            .filter(|&child| has_role(dom, child, "listitem"))
            .collect();
        if items.is_empty() {
            continue;
        }
        normalized_items.extend(items.iter().copied());
        if matches!(dom.tag(node), Some(Tag::Ol | Tag::Ul)) {
            continue;
        }
        if let Some(markers) = ordered_markers(dom, &items) {
            dom.rename_html(node, Tag::Ol);
            if let Some((first, _)) = markers.first()
                && *first != 1
            {
                dom.set_attr(node, AttrName::Start, &first.to_string());
            }
            for (_, (text, prefix_end)) in markers {
                let replacement = dom.text_node(text).unwrap_or_default()[prefix_end..]
                    .trim_start()
                    .to_owned();
                dom.set_text(text, &replacement);
            }
        } else {
            dom.rename_html(node, Tag::Ul);
        }
    }

    for node in normalized_items {
        if dom.parent(node).is_some() && has_role(dom, node, "listitem") {
            dom.rename_html(node, Tag::Li);
        }
    }
}

fn has_role(dom: &Dom, node: NodeId, expected: &str) -> bool {
    dom.attr(node, AttrName::Role)
        .is_some_and(|roles| has_token(roles, expected))
}

fn ordered_markers(dom: &Dom, items: &[NodeId]) -> Option<Vec<(u32, (NodeId, usize))>> {
    if items.len() < 2 {
        return None;
    }
    let markers: Vec<_> = items
        .iter()
        .map(|&item| first_text_marker(dom, item))
        .collect::<Option<_>>()?;
    let first = markers.first()?.0;
    markers
        .iter()
        .enumerate()
        .all(|(index, marker)| marker.0 == first.saturating_add(index as u32))
        .then_some(markers)
}

fn first_text_marker(dom: &Dom, item: NodeId) -> Option<(u32, (NodeId, usize))> {
    let text = dom.descendants(item).find(|&node| {
        dom.text_node(node)
            .is_some_and(|value| !value.trim().is_empty())
    })?;
    let value = dom.text_node(text)?;
    let leading = value.len() - value.trim_start().len();
    let trimmed = &value[leading..];
    let digits = trimmed.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 {
        return None;
    }
    let separator = trimmed.as_bytes().get(digits).copied()?;
    if !matches!(separator, b'.' | b')') {
        return None;
    }
    let after = trimmed.as_bytes().get(digits + 1).copied();
    if after.is_some_and(|byte| !byte.is_ascii_whitespace()) {
        return None;
    }
    let number = trimmed[..digits].parse::<u32>().ok()?;
    Some((number, (text, leading + digits + 1)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn semantic_markdown(dom: &Dom, root: NodeId) -> String {
        let document = crate::document::compile_document(
            dom,
            root,
            &crate::document::CompileContext::default(),
            &crate::document::CompileInputs::default(),
        )
        .unwrap();
        crate::render::markdown::render_markdown(
            &document,
            0,
            crate::render::markdown::MarkdownConfig::default(),
        )
    }

    fn normalized(html: &str) -> (Dom, NodeId) {
        let mut dom = Dom::parse_document(html).unwrap();
        let root = dom.body().unwrap();
        normalize_for_scoring(&mut dom, root);
        (dom, root)
    }

    #[test]
    fn converts_nested_aria_lists() {
        let (dom, root) = normalized(
            r#"<div role="list"><div role="listitem">One<div role="list"><div role="listitem">Nested</div></div></div><div role="listitem">Two</div></div>"#,
        );
        assert_eq!(
            dom.descendants(root)
                .filter(|&node| dom.tag(node) == Some(Tag::Ul))
                .count(),
            2
        );
        assert_eq!(
            dom.descendants(root)
                .filter(|&node| dom.tag(node) == Some(Tag::Li))
                .count(),
            3
        );
    }

    #[test]
    fn converts_explicit_ordered_labels() {
        let (dom, root) = normalized(
            r#"<div role="list"><div role="listitem">3. Three</div><div role="listitem">4. Four</div></div>"#,
        );
        let list = dom.first_descendant_by_tag(root, Tag::Ol).unwrap();
        assert_eq!(dom.attr(list, AttrName::Start), Some("3"));
        assert_eq!(semantic_markdown(&dom, root), "3. Three\n1. Four\n");
    }

    #[test]
    fn leaves_repeated_cards_unchanged() {
        let (dom, root) = normalized("<div><div>Card one</div><div>Card two</div></div>");
        assert!(dom.first_descendant_by_tag(root, Tag::Ul).is_none());
        assert!(dom.first_descendant_by_tag(root, Tag::Ol).is_none());
    }

    #[test]
    fn does_not_create_orphan_list_items() {
        let (dom, root) = normalized(
            r#"<div role="listitem">Standalone</div><div role="list"><div><div role="listitem">Indirect</div></div></div>"#,
        );
        assert!(dom.first_descendant_by_tag(root, Tag::Li).is_none());
        assert!(dom.first_descendant_by_tag(root, Tag::Ul).is_none());
    }
}
