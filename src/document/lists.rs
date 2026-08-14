use crate::dom::{AttrName, Dom, NodeId, Tag};
use smallvec::SmallVec;

use super::{List, ListKind};

/// Semantic list evidence collected without changing the selected DOM.
pub(super) struct ListAnalysis {
    containers: Vec<Option<List>>,
    items: Vec<bool>,
    text_replacements: Vec<Option<Box<str>>>,
}

impl ListAnalysis {
    pub(super) fn analyze(dom: &Dom, nodes: &[NodeId]) -> Self {
        let mut containers = vec![None; dom.len()];
        let mut items = vec![false; dom.len()];
        let mut text_replacements = vec![None; dom.len()];

        for &node in nodes {
            let native = match dom.tag(node) {
                Some(Tag::Ul) => Some(List {
                    kind: ListKind::Unordered,
                    start: None,
                }),
                Some(Tag::Ol) => Some(List {
                    kind: ListKind::Ordered,
                    start: dom
                        .attr(node, AttrName::Start)
                        .and_then(|value| value.parse().ok()),
                }),
                _ => None,
            };
            let has_list_role = has_role(dom, node, "list");
            if native.is_none() && !has_list_role {
                continue;
            }
            let role_items: SmallVec<[NodeId; 16]> = dom
                .element_children(node)
                .filter(|&child| has_role(dom, child, "listitem"))
                .collect();
            for &item in &role_items {
                items[item.index()] = true;
            }
            if let Some(list) = native {
                containers[node.index()] = Some(list);
                continue;
            }
            if role_items.is_empty() {
                continue;
            }
            if let Some(markers) = ordered_markers(dom, &role_items) {
                let start = markers.first().map(|marker| i64::from(marker.number));
                containers[node.index()] = Some(List {
                    kind: ListKind::Ordered,
                    start: start.filter(|start| *start != 1),
                });
                for marker in markers {
                    let value = dom.text_node(marker.text).unwrap_or_default();
                    text_replacements[marker.text.index()] =
                        Some(value[marker.prefix_end..].trim_start().into());
                }
            } else {
                containers[node.index()] = Some(List {
                    kind: ListKind::Unordered,
                    start: None,
                });
            }
        }

        Self {
            containers,
            items,
            text_replacements,
        }
    }

    pub(super) fn container(&self, node: NodeId) -> Option<List> {
        self.containers.get(node.index()).copied().flatten()
    }

    pub(super) fn is_item(&self, node: NodeId) -> bool {
        self.items.get(node.index()).copied().unwrap_or(false)
    }

    pub(super) fn replacement_text(&self, node: NodeId) -> Option<&str> {
        self.text_replacements
            .get(node.index())
            .and_then(Option::as_deref)
    }
}

fn has_role(dom: &Dom, node: NodeId, expected: &str) -> bool {
    dom.attr(node, AttrName::Role).is_some_and(|roles| {
        roles
            .split_ascii_whitespace()
            .any(|role| role.eq_ignore_ascii_case(expected))
    })
}

#[derive(Clone, Copy)]
struct OrderedMarker {
    number: u32,
    text: NodeId,
    prefix_end: usize,
}

fn ordered_markers(dom: &Dom, items: &[NodeId]) -> Option<Vec<OrderedMarker>> {
    if items.len() < 2 {
        return None;
    }
    let markers: Vec<_> = items
        .iter()
        .map(|&item| first_text_marker(dom, item))
        .collect::<Option<_>>()?;
    let first = markers.first()?.number;
    markers
        .iter()
        .enumerate()
        .all(|(index, marker)| marker.number == first.saturating_add(index as u32))
        .then_some(markers)
}

fn first_text_marker(dom: &Dom, item: NodeId) -> Option<OrderedMarker> {
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
    Some(OrderedMarker {
        number: trimmed[..digits].parse().ok()?,
        text,
        prefix_end: leading + digits + 1,
    })
}
