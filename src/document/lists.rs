use crate::dom::{AttrName, Dom, NodeId, Tag};
use crate::tokens::has_token;
use smallvec::SmallVec;

use super::{List, ListKind};

/// Semantic list evidence collected without changing the selected DOM.
pub(super) struct ListAnalysis {
    slots: Vec<u32>,
    entries: Vec<ListFacts>,
}

#[derive(Default)]
struct ListFacts {
    container: Option<List>,
    item: bool,
    replacement: Option<Box<str>>,
}

impl ListAnalysis {
    pub(super) fn analyze(dom: &Dom, candidates: &[NodeId]) -> Self {
        if candidates.is_empty() {
            return Self {
                slots: Vec::new(),
                entries: Vec::new(),
            };
        }
        let mut analysis = Self {
            slots: vec![u32::MAX; dom.len()],
            entries: Vec::new(),
        };

        for &node in candidates {
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
                analysis.entry_mut(item).item = true;
            }
            if let Some(list) = native {
                analysis.entry_mut(node).container = Some(list);
                continue;
            }
            if role_items.is_empty() {
                continue;
            }
            if let Some(markers) = ordered_markers(dom, &role_items) {
                let start = markers.first().map(|marker| i64::from(marker.number));
                analysis.entry_mut(node).container = Some(List {
                    kind: ListKind::Ordered,
                    start: start.filter(|start| *start != 1),
                });
                for marker in markers {
                    let value = dom.text_node(marker.text).unwrap_or_default();
                    analysis.entry_mut(marker.text).replacement =
                        Some(value[marker.prefix_end..].trim_start().into());
                }
            } else {
                analysis.entry_mut(node).container = Some(List {
                    kind: ListKind::Unordered,
                    start: None,
                });
            }
        }

        analysis
    }

    fn entry(&self, node: NodeId) -> Option<&ListFacts> {
        let slot = *self.slots.get(node.index())?;
        (slot != u32::MAX).then(|| &self.entries[slot as usize])
    }

    fn entry_mut(&mut self, node: NodeId) -> &mut ListFacts {
        let slot = self.slots[node.index()];
        let slot = if slot == u32::MAX {
            let slot = self.entries.len() as u32;
            self.entries.push(ListFacts::default());
            self.slots[node.index()] = slot;
            slot
        } else {
            slot
        };
        &mut self.entries[slot as usize]
    }

    pub(super) fn container(&self, node: NodeId) -> Option<List> {
        self.entry(node).and_then(|facts| facts.container)
    }

    pub(super) fn is_item(&self, node: NodeId) -> bool {
        self.entry(node).is_some_and(|facts| facts.item)
    }

    pub(super) fn replacement_text(&self, node: NodeId) -> Option<&str> {
        self.entry(node)
            .and_then(|facts| facts.replacement.as_deref())
    }
}

fn has_role(dom: &Dom, node: NodeId, expected: &str) -> bool {
    dom.attr(node, AttrName::Role)
        .is_some_and(|roles| has_token(roles, expected))
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
