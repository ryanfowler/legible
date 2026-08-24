//! Source media recognition for semantic document compilation.

use url::Url;

use crate::dom::{AttrName, Dom, NodeId, Tag};

use super::facts::SemanticFacts;
use super::sparse::SparseNodeValues;
use super::{DestinationKind, MediaKind, safe_destination};

pub(crate) struct MediaAnalysis {
    items: SparseNodeValues<RecognizedMedia>,
}

impl MediaAnalysis {
    pub(crate) fn item(&self, node: NodeId) -> Option<&RecognizedMedia> {
        self.items.get(node)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    // Used by the benchmark-only complex storage report.
    #[allow(dead_code)]
    pub(crate) fn storage_bytes(&self) -> usize {
        self.items.allocated_bytes()
    }
}

pub(crate) struct RecognizedMedia {
    pub(crate) kind: MediaKind,
    pub(crate) source: Box<str>,
    pub(crate) title: Box<str>,
    pub(crate) fallback: Option<NodeId>,
}

/// Resolves media sources and fallback links in linear document passes.
pub(crate) fn analyze(dom: &Dom, nodes: &[NodeId], base_url: Option<&Url>) -> MediaAnalysis {
    analyze_inner(dom, nodes, None, base_url)
}

/// Resolves media with visible-text facts from the shared complex-source scan.
pub(crate) fn analyze_with_facts(
    dom: &Dom,
    facts: &SemanticFacts,
    base_url: Option<&Url>,
) -> MediaAnalysis {
    analyze_inner(dom, facts.nodes(), Some(facts), base_url)
}

fn analyze_inner(
    dom: &Dom,
    nodes: &[NodeId],
    facts: Option<&SemanticFacts>,
    base_url: Option<&Url>,
) -> MediaAnalysis {
    if facts.map_or_else(
        || {
            !nodes
                .iter()
                .any(|&node| media_kind(dom.tag(node)).is_some())
        },
        |facts| facts.inventory().media.is_empty(),
    ) {
        return MediaAnalysis {
            items: SparseNodeValues::new(),
        };
    }
    let mut nearest_media = vec![None; dom.len()];
    let mut sources = (0..dom.len()).map(|_| None).collect::<Vec<_>>();
    for &node in nodes {
        let kind = media_kind(dom.tag(node));
        nearest_media[node.index()] = if kind.is_some() {
            Some(node)
        } else {
            dom.parent(node)
                .and_then(|parent| nearest_media[parent.index()])
        };
        if kind.is_some() {
            sources[node.index()] = direct_source(dom, node, base_url);
        } else if dom.tag(node) == Some(Tag::Source)
            && let Some(media) = nearest_media[node.index()]
            && sources[media.index()].is_none()
        {
            sources[media.index()] = direct_source(dom, node, base_url);
        }
    }

    let mut has_text = Vec::new();
    if facts.is_none() {
        has_text = vec![false; dom.len()];
        for &node in nodes.iter().rev() {
            has_text[node.index()] = dom
                .text_node(node)
                .is_some_and(|text| text.chars().any(|character| !character.is_whitespace()))
                || dom.children(node).any(|child| has_text[child.index()]);
        }
    }
    let mut fallbacks = vec![None; dom.len()];
    for &node in nodes {
        let has_visible_text = facts.map_or_else(
            || has_text[node.index()],
            |facts| facts.has_visible_text(node),
        );
        if dom.tag(node) != Some(Tag::A) || !has_visible_text {
            continue;
        }
        let Some(media) = nearest_media[node.index()] else {
            continue;
        };
        if fallbacks[media.index()].is_none()
            && dom.attr(node, AttrName::Href).is_some_and(|destination| {
                safe_destination(destination, base_url, DestinationKind::Resource).is_some()
            })
        {
            fallbacks[media.index()] = Some(node);
        }
    }

    let mut result = SparseNodeValues::with_capacity(
        nodes
            .iter()
            .filter(|&&node| media_kind(dom.tag(node)).is_some())
            .count(),
    );
    for &node in nodes {
        let Some(kind) = media_kind(dom.tag(node)) else {
            continue;
        };
        let Some(mut source) = sources[node.index()].take() else {
            continue;
        };
        let youtube = kind == MediaKind::Embedded && is_youtube(&source);
        if youtube {
            source = canonical_youtube(&source).into();
        }
        let title = [AttrName::AriaLabel, AttrName::Title]
            .into_iter()
            .filter_map(|attribute| dom.attr(node, attribute))
            .find_map(normalize_label)
            .unwrap_or_else(|| match (kind, youtube) {
                (MediaKind::Embedded, true) => "YouTube video".into(),
                (MediaKind::Embedded, false) => "Embedded content".into(),
                (MediaKind::Video, _) => "Video".into(),
                (MediaKind::Audio, _) => "Audio".into(),
            });
        result.push(
            node,
            RecognizedMedia {
                kind,
                source,
                title,
                fallback: fallbacks[node.index()],
            },
        );
    }
    drop(sources);
    result.sort();
    result.build_dense_index_if_dense(dom.len());
    MediaAnalysis { items: result }
}

pub(super) fn cleanup_evidence(dom: &Dom, nodes: &[NodeId]) -> (Vec<bool>, Vec<Option<NodeId>>) {
    let analysis = analyze(dom, nodes, None);
    let mut sources = vec![false; dom.len()];
    let mut fallbacks = vec![None; dom.len()];
    for (node, item) in analysis.items.iter() {
        sources[node.index()] = true;
        fallbacks[node.index()] = item.fallback;
    }
    (sources, fallbacks)
}

pub(super) fn cleanup_evidence_into(
    dom: &Dom,
    nodes: &[NodeId],
    sources: &mut [bool],
    fallbacks: &mut [u32],
) {
    sources.fill(false);
    fallbacks.fill(u32::MAX);
    let analysis = analyze(dom, nodes, None);
    for (node, item) in analysis.items.iter() {
        sources[node.index()] = true;
        if let Some(fallback) = item.fallback {
            fallbacks[node.index()] = fallback.0;
        }
    }
}

fn media_kind(tag: Option<Tag>) -> Option<MediaKind> {
    match tag {
        Some(Tag::Iframe) => Some(MediaKind::Embedded),
        Some(Tag::Video) => Some(MediaKind::Video),
        Some(Tag::Audio) => Some(MediaKind::Audio),
        _ => None,
    }
}

fn direct_source(dom: &Dom, node: NodeId, base_url: Option<&Url>) -> Option<Box<str>> {
    [AttrName::Src, AttrName::DataSrc]
        .into_iter()
        .filter_map(|attribute| dom.attr(node, attribute))
        .filter(|source| {
            !source.trim().eq_ignore_ascii_case("null")
                && !source.trim().eq_ignore_ascii_case("undefined")
        })
        .find_map(|source| safe_destination(source, base_url, DestinationKind::Resource))
}

fn normalize_label(value: &str) -> Option<Box<str>> {
    let value = value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(200)
        .collect::<String>();
    (!value.is_empty()).then(|| value.into_boxed_str())
}

fn canonical_youtube(value: &str) -> String {
    let Some((_, id)) = value.split_once("/embed/") else {
        return value.to_owned();
    };
    let id = id.split(['?', '#']).next().unwrap_or(id);
    format!("https://www.youtube.com/watch?v={id}")
}

fn is_youtube(value: &str) -> bool {
    let absolute;
    let value = if value.starts_with("//") {
        absolute = format!("https:{value}");
        &absolute
    } else {
        value
    };
    let Ok(url) = url::Url::parse(value) else {
        return false;
    };
    url.host_str().is_some_and(|host| {
        let host = host.to_ascii_lowercase();
        host == "youtu.be"
            || host == "youtube.com"
            || host.ends_with(".youtube.com")
            || host == "youtube-nocookie.com"
            || host.ends_with(".youtube-nocookie.com")
    })
}
