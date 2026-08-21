use crate::dom::{AttrName, Dom, NodeId, Tag};

use super::sparse::SparseNodeValues;

#[derive(Clone, Debug)]
pub(crate) struct RecognizedCallout {
    pub(crate) kind: &'static str,
    pub(crate) title: Box<str>,
    pub(crate) title_node: Option<NodeId>,
    pub(crate) title_is_strong: bool,
}

pub(crate) struct CalloutAnalysis {
    values: SparseNodeValues<RecognizedCallout>,
    #[allow(dead_code)]
    bounded_text_bytes: usize,
}

impl CalloutAnalysis {
    pub(crate) fn analyze(dom: &Dom, nodes: &[NodeId], candidates: &[NodeId]) -> Self {
        if candidates.is_empty() {
            return Self {
                values: SparseNodeValues::new(),
                bounded_text_bytes: 0,
            };
        }
        let mut values = SparseNodeValues::with_capacity(candidates.len());
        let labels = bounded_subtree_text(dom, nodes);
        let bounded_text_bytes = labels
            .capacity()
            .saturating_mul(std::mem::size_of::<Option<Box<str>>>())
            .saturating_add(
                labels
                    .iter()
                    .filter_map(Option::as_ref)
                    .map(|value| value.len())
                    .sum::<usize>(),
            );
        for &node in candidates {
            let (structural, candidate_kind) = callout_evidence(dom, node);
            let Some(kind) = candidate_kind else {
                continue;
            };
            let title_node = dom.element_children(node).next();
            let explicit_name = title_node.is_some_and(|child| explicitly_named(dom, child));
            let title_text = title_node.and_then(|child| labels[child.index()].as_deref());
            let label = title_text
                .map(|text| text.trim().trim_end_matches(':'))
                .filter(|text| explicit_name || text.len() <= 16);
            if !structural && label.is_none_or(|label| canonical_kind(label) != Some(kind)) {
                continue;
            }
            let explicit_title = title_node
                .filter(|_| label.is_some_and(|label| canonical_kind(label) == Some(kind)));
            let title = explicit_title
                .and(title_text)
                .map(|title| title.trim().trim_end_matches(':').to_owned())
                .unwrap_or_else(|| title_case(kind));
            let title_is_strong = explicit_title.is_some_and(|title| {
                dom.element_children(title)
                    .any(|child| matches!(dom.tag(child), Some(Tag::Strong | Tag::B)))
            });
            values.push(
                node,
                RecognizedCallout {
                    kind,
                    title: title.into(),
                    title_node: explicit_title,
                    title_is_strong,
                },
            );
        }
        values.sort();
        values.build_dense_index_if_dense(dom.len());
        Self {
            values,
            bounded_text_bytes,
        }
    }

    pub(crate) fn value(&self, node: NodeId) -> Option<&RecognizedCallout> {
        self.values.get(node)
    }

    #[allow(dead_code)]
    pub(crate) fn storage_bytes(&self) -> usize {
        self.values
            .allocated_bytes()
            .saturating_add(self.bounded_text_bytes)
    }
}

/// Returns source and candidate callout evidence from one bounded check.
pub(crate) fn source_evidence(dom: &Dom, node: NodeId) -> (bool, bool) {
    if !matches!(dom.tag(node), Some(Tag::Aside | Tag::Div | Tag::Section)) {
        return (false, false);
    }
    let (structural, kind) = callout_evidence(dom, node);
    (structural && kind.is_some(), kind.is_some())
}

/// Returns true when cleanup must retain source evidence for compilation.
pub(crate) fn is_source_evidence(dom: &Dom, node: NodeId) -> bool {
    source_evidence(dom, node).0
}

pub(crate) fn class_is_semantic_evidence(dom: &Dom, node: NodeId) -> bool {
    source_evidence(dom, node).1
}

fn callout_evidence(dom: &Dom, node: NodeId) -> (bool, Option<&'static str>) {
    let mut structural = false;
    let mut kind = None;
    if dom.attr(node, AttrName::Role).is_some_and(|roles| {
        roles
            .split_whitespace()
            .any(|role| role.eq_ignore_ascii_case("note"))
    }) {
        structural = true;
        kind = Some("note");
    }
    for token in [AttrName::Class, AttrName::Id]
        .into_iter()
        .filter_map(|name| dom.attr(node, name))
        .flat_map(str::split_whitespace)
    {
        let token = token
            .trim_matches(|ch: char| !ch.is_ascii_alphanumeric())
            .to_ascii_lowercase();
        structural |= matches!(token.as_str(), "admonition" | "callout" | "alert");
        if let Some(value) = canonical_kind(&token) {
            kind = Some(value);
        }
    }
    (structural, kind)
}

fn canonical_kind(value: &str) -> Option<&'static str> {
    match value
        .trim()
        .trim_end_matches(':')
        .to_ascii_lowercase()
        .as_str()
    {
        "note" => Some("note"),
        "warning" => Some("warning"),
        "tip" => Some("tip"),
        "important" => Some("important"),
        "caution" => Some("caution"),
        "info" | "information" => Some("info"),
        _ => None,
    }
}

fn explicitly_named(dom: &Dom, node: NodeId) -> bool {
    [AttrName::Class, AttrName::Id]
        .into_iter()
        .filter_map(|name| dom.attr(node, name))
        .flat_map(str::split_whitespace)
        .any(|token| {
            matches!(
                token.to_ascii_lowercase().as_str(),
                "title" | "admonition-title" | "callout-title" | "label"
            )
        })
}

fn title_case(kind: &str) -> String {
    let mut title = kind.to_owned();
    if let Some(first) = title.get_mut(0..1) {
        first.make_ascii_uppercase();
    }
    title
}

/// Computes bounded normalized text prefixes for every subtree in one pass.
fn bounded_subtree_text(dom: &Dom, nodes: &[NodeId]) -> Vec<Option<Box<str>>> {
    const LIMIT: usize = 32;
    let mut values = (0..dom.len()).map(|_| None).collect::<Vec<_>>();
    for &node in nodes.iter().rev() {
        let mut value = String::new();
        if let Some(text) = dom.text_node(node) {
            append_normalized(&mut value, text, LIMIT);
        } else {
            for child in dom.children(node) {
                let Some(text) = values[child.index()].as_deref() else {
                    continue;
                };
                append_normalized(&mut value, text, LIMIT);
                if value.len() >= LIMIT {
                    break;
                }
            }
        }
        if !value.is_empty() {
            values[node.index()] = Some(value.into());
        }
    }
    values
}

fn append_normalized(output: &mut String, input: &str, limit: usize) {
    if input.is_ascii() {
        for word in input.split_ascii_whitespace() {
            if !output.is_empty() {
                if output.len() == limit {
                    break;
                }
                output.push(' ');
            }
            let remaining = limit.saturating_sub(output.len());
            let end = word.len().min(remaining);
            output.push_str(&word[..end]);
            if output.len() == limit {
                break;
            }
        }
    } else {
        for word in input.split_whitespace() {
            if !output.is_empty() {
                if output.len() == limit {
                    break;
                }
                output.push(' ');
            }
            let remaining = limit.saturating_sub(output.len());
            for character in word.chars().take(remaining) {
                output.push(character);
            }
            if output.len() == limit {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_an_admonition_but_not_a_card() {
        let dom = Dom::parse_fragment(r#"<div class="admonition warning"><p class="admonition-title">Warning</p><p>Take care.</p></div><div class="card warning"><p>Release notes</p></div>"#, Tag::Div).unwrap();
        let nodes = std::iter::once(dom.root())
            .chain(dom.descendants(dom.root()))
            .collect::<Vec<_>>();
        let candidates = nodes
            .iter()
            .copied()
            .filter(|&node| class_is_semantic_evidence(&dom, node))
            .collect::<Vec<_>>();
        let analysis = CalloutAnalysis::analyze(&dom, &nodes, &candidates);
        let mut divs = dom
            .descendants(dom.root())
            .filter(|&node| dom.tag(node) == Some(Tag::Div));
        let callout = analysis.value(divs.next().unwrap()).unwrap();
        assert_eq!(callout.kind, "warning");
        assert!(callout.title_node.is_some());
        assert_eq!(&*callout.title, "Warning");
        assert!(analysis.value(divs.next().unwrap()).is_none());
    }
}
