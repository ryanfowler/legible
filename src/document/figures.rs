//! Source figure recognition for semantic document compilation.

use crate::dom::{AttrName, Dom, NodeId, Tag};

use super::images::ImageAnalysis;

/// Classifies figure and caption semantics in linear document passes.
pub(crate) fn analyze(
    dom: &Dom,
    nodes: &[NodeId],
    images: &ImageAnalysis,
) -> (Vec<bool>, Vec<bool>) {
    let has_candidates = nodes.iter().any(|&node| {
        matches!(
            dom.tag(node),
            Some(Tag::Img | Tag::Figure | Tag::Figcaption)
        ) || class_is_semantic_evidence(dom, node)
    });
    analyze_inner(dom, nodes, images, has_candidates)
}

pub(crate) fn analyze_with_inventory(
    dom: &Dom,
    nodes: &[NodeId],
    candidates: &[NodeId],
    images: &ImageAnalysis,
) -> (Vec<bool>, Vec<bool>) {
    analyze_inner(dom, nodes, images, !candidates.is_empty())
}

fn analyze_inner(
    dom: &Dom,
    nodes: &[NodeId],
    images: &ImageAnalysis,
    has_candidates: bool,
) -> (Vec<bool>, Vec<bool>) {
    if !has_candidates {
        return (vec![false; dom.len()], vec![false; dom.len()]);
    }
    let mut image_count = vec![0_u8; dom.len()];
    let mut semantic_figure = vec![false; dom.len()];
    let mut caption_evidence = vec![false; dom.len()];
    let mut figures = vec![false; dom.len()];

    for &node in nodes.iter().rev() {
        let mut descendant_images = 0_u8;
        let mut has_figure_descendant = false;
        let mut has_caption = false;
        for child in dom.children(node) {
            descendant_images = descendant_images
                .saturating_add(image_count[child.index()])
                .min(2);
            has_figure_descendant |= semantic_figure[child.index()];
            has_caption |= caption_evidence[child.index()];
        }
        image_count[node.index()] = descendant_images
            .saturating_add(u8::from(
                dom.tag(node) == Some(Tag::Img) || images.is_synthetic(node),
            ))
            .min(2);
        caption_evidence[node.index()] = has_caption
            || dom.tag(node) == Some(Tag::Figcaption)
            || named(dom, node, &["caption", "figcaption", "image-caption"]);
        figures[node.index()] = dom.tag(node) == Some(Tag::Figure)
            || matches!(dom.tag(node), Some(Tag::Div | Tag::P | Tag::Section))
                && named(
                    dom,
                    node,
                    &["figure", "image-with-caption", "media-with-caption"],
                )
                && !has_figure_descendant
                && descendant_images == 1
                && has_caption;
        semantic_figure[node.index()] = has_figure_descendant || figures[node.index()];
    }

    let mut captions = vec![false; dom.len()];
    let mut nearest_figure = vec![None; dom.len()];
    let mut selected_caption = vec![false; dom.len()];
    for &node in nodes {
        nearest_figure[node.index()] = if figures[node.index()] {
            Some(node)
        } else {
            dom.parent(node)
                .and_then(|parent| nearest_figure[parent.index()])
        };
        let caption_evidence = dom.tag(node) == Some(Tag::Figcaption)
            || named(dom, node, &["caption", "figcaption", "image-caption"]);
        if caption_evidence
            && let Some(figure) = nearest_figure[node.index()]
            && !selected_caption[figure.index()]
            && figure != node
        {
            captions[node.index()] = true;
            selected_caption[figure.index()] = true;
        }
    }
    (figures, captions)
}

pub(crate) fn class_is_semantic_evidence(dom: &Dom, node: NodeId) -> bool {
    named(
        dom,
        node,
        &[
            "figure",
            "image-with-caption",
            "media-with-caption",
            "caption",
            "figcaption",
            "image-caption",
        ],
    )
}

fn named(dom: &Dom, node: NodeId, names: &[&str]) -> bool {
    [AttrName::Class, AttrName::Id]
        .into_iter()
        .filter_map(|attribute| dom.attr(node, attribute))
        .flat_map(str::split_whitespace)
        .any(|token| names.iter().any(|name| token.eq_ignore_ascii_case(name)))
}
