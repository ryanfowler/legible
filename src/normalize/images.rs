use crate::cleaning::fix_lazy_images;
use crate::dom::{AttrName, Dom, NodeId, Tag};
use smallvec::SmallVec;

#[derive(Clone, Copy, Debug, PartialEq)]
enum SrcsetDescriptor {
    Width(u32),
    Density(f32),
    None,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct SrcsetCandidate<'a> {
    url: &'a str,
    descriptor: SrcsetDescriptor,
}

pub(super) fn normalize(dom: &mut Dom, root: NodeId, nodes: &mut Vec<NodeId>) {
    fix_lazy_images(dom, root, nodes);

    // Lazy loaders also place responsive values on `picture` sources. Promote
    // those attributes without discarding the source element or its metadata.
    let responsive_nodes: SmallVec<[NodeId; 32]> = dom
        .descendants(root)
        .filter(|&node| matches!(dom.tag(node), Some(Tag::Img | Tag::Source)))
        .collect();
    for node in responsive_nodes {
        if valid_image_attribute(dom.attr(node, AttrName::Srcset)).is_none()
            && let Some(value) =
                valid_image_attribute(dom.attr(node, AttrName::DataSrcset)).map(str::to_owned)
        {
            dom.set_attr(node, AttrName::Srcset, &value);
        }
        if valid_image_attribute(dom.attr(node, AttrName::Src)).is_none()
            && let Some(value) =
                valid_image_attribute(dom.attr(node, AttrName::DataSrc)).map(str::to_owned)
        {
            dom.set_attr(node, AttrName::Src, &value);
        }
    }
    nodes.clear();
    nodes.extend(
        dom.descendants(root)
            .filter(|&node| dom.tag(node) == Some(Tag::Img)),
    );

    // Resolve one useful concrete URL for Markdown. Keep every original
    // `srcset` and the complete `picture` structure for HTML rendering.
    for &image in nodes.iter() {
        if let Some(source) = best_responsive_source(dom, image) {
            dom.set_attr(image, AttrName::Src, &source);
        }
    }

    deduplicate(dom, nodes);

    // An unresolved placeholder has no visual content. Accessible text or a
    // sole figure caption can still make the image meaningful.
    for &image in nodes.iter() {
        if dom.parent(image).is_some()
            && image_has_orphan_placeholder_source(dom, image)
            && !has_meaningful_image_description(dom, image)
        {
            dom.detach(image);
        }
    }
}

fn best_responsive_source(dom: &Dom, image: NodeId) -> Option<String> {
    if let Some(picture) = dom
        .ancestors(image)
        .find(|&ancestor| dom.tag(ancestor) == Some(Tag::Picture))
    {
        for node in std::iter::once(picture).chain(
            dom.descendants(picture)
                .filter(|&node| dom.tag(node) == Some(Tag::Source)),
        ) {
            if let Some(candidate) = valid_image_attribute(
                dom.attr(node, AttrName::DataSrcset)
                    .or_else(|| dom.attr(node, AttrName::Srcset)),
            )
            .and_then(best_srcset_candidate)
            {
                return Some(candidate.url.to_owned());
            }
            if let Some(src) = valid_image_attribute(
                dom.attr(node, AttrName::DataSrc)
                    .or_else(|| dom.attr(node, AttrName::Src)),
            ) {
                return Some(src.to_owned());
            }
        }
    }

    dom.attr(image, AttrName::Srcset)
        .and_then(best_srcset_candidate)
        .map(|candidate| candidate.url.to_owned())
        .or_else(|| {
            let replace = dom.attr(image, AttrName::Src).is_none()
                || image_has_placeholder_source(dom, image);
            replace
                .then(|| {
                    valid_image_attribute(dom.attr(image, AttrName::DataSrc)).map(str::to_owned)
                })
                .flatten()
        })
}

fn parse_srcset(srcset: &str) -> Vec<SrcsetCandidate<'_>> {
    let bytes = srcset.as_bytes();
    let mut candidates = Vec::new();
    let mut position = 0;
    while position < bytes.len() {
        while position < bytes.len()
            && (bytes[position].is_ascii_whitespace() || bytes[position] == b',')
        {
            position += 1;
        }
        if position == bytes.len() {
            break;
        }

        let url_start = position;
        while position < bytes.len() && !bytes[position].is_ascii_whitespace() {
            position += 1;
        }
        let mut url_end = position;
        while url_end > url_start && bytes[url_end - 1] == b',' {
            url_end -= 1;
        }
        let ended_with_comma = url_end != position;
        let url = &srcset[url_start..url_end];

        let descriptor = if ended_with_comma {
            Some(SrcsetDescriptor::None)
        } else {
            let descriptor_start = position;
            while position < bytes.len() && bytes[position] != b',' {
                position += 1;
            }
            parse_descriptor(srcset[descriptor_start..position].trim())
        };
        if !url.is_empty() && descriptor.is_some() {
            candidates.push(SrcsetCandidate {
                url,
                descriptor: descriptor.unwrap_or(SrcsetDescriptor::None),
            });
        }
        if position < bytes.len() {
            position += 1;
        }
    }
    candidates
}

fn parse_descriptor(value: &str) -> Option<SrcsetDescriptor> {
    if value.is_empty() {
        return Some(SrcsetDescriptor::None);
    }
    if value.split_ascii_whitespace().count() != 1 {
        return None;
    }
    if let Some(width) = value.strip_suffix('w') {
        return width
            .parse::<u32>()
            .ok()
            .filter(|width| *width > 0)
            .map(SrcsetDescriptor::Width);
    }
    if let Some(density) = value.strip_suffix('x') {
        return density
            .parse::<f32>()
            .ok()
            .filter(|density| density.is_finite() && *density > 0.0)
            .map(SrcsetDescriptor::Density);
    }
    None
}

fn best_srcset_candidate(srcset: &str) -> Option<SrcsetCandidate<'_>> {
    parse_srcset(srcset).into_iter().reduce(|best, candidate| {
        if candidate_is_better(&candidate, &best) {
            candidate
        } else {
            best
        }
    })
}

fn candidate_is_better(candidate: &SrcsetCandidate<'_>, current: &SrcsetCandidate<'_>) -> bool {
    match (candidate.descriptor, current.descriptor) {
        (SrcsetDescriptor::Width(left), SrcsetDescriptor::Width(right)) => left > right,
        (SrcsetDescriptor::Width(_), _) => true,
        (_, SrcsetDescriptor::Width(_)) => false,
        (SrcsetDescriptor::Density(left), SrcsetDescriptor::Density(right)) => left > right,
        (SrcsetDescriptor::Density(_), SrcsetDescriptor::None) => true,
        _ => false,
    }
}

fn deduplicate(dom: &mut Dom, nodes: &[NodeId]) {
    // Adjacent wrappers are strong hydration evidence. The pair must also
    // share a source, a meaningful description plus quality evidence, or a
    // lightbox destination.
    for &image in nodes {
        if dom.parent(image).is_none() {
            continue;
        }
        let current_container = image_container(dom, image);
        let Some(previous_container) = previous_element_sibling(dom, current_container) else {
            continue;
        };
        let Some(previous_image) = single_image(dom, previous_container) else {
            continue;
        };
        let previous_removal = if is_image_only_wrapper(dom, previous_container, previous_image) {
            previous_container
        } else {
            previous_image
        };
        if !likely_duplicate(dom, previous_image, image) {
            continue;
        }
        let remove = if lightbox_links_to(dom, previous_image, image) {
            previous_removal
        } else if lightbox_links_to(dom, image, previous_image) {
            current_container
        } else if image_quality(dom, image) > image_quality(dom, previous_image) {
            previous_removal
        } else {
            current_container
        };
        dom.detach(remove);
    }

    // A figure is another strong local boundary. Do not compare images across
    // figures or deduplicate repeated images across article paragraphs.
    let figures: SmallVec<[NodeId; 8]> = nodes
        .iter()
        .filter_map(|&image| {
            dom.ancestors(image)
                .find(|&ancestor| dom.tag(ancestor) == Some(Tag::Figure))
        })
        .collect();
    for figure in figures {
        let images: SmallVec<[NodeId; 4]> = dom
            .descendants(figure)
            .filter(|&node| dom.tag(node) == Some(Tag::Img) && dom.parent(node).is_some())
            .collect();
        if images.len() != 2 || !likely_duplicate(dom, images[0], images[1]) {
            continue;
        }
        let remove = if image_quality(dom, images[1]) > image_quality(dom, images[0]) {
            image_container_within(dom, images[0], figure)
        } else {
            image_container_within(dom, images[1], figure)
        };
        dom.detach(remove);
    }
}

fn likely_duplicate(dom: &Dom, first: NodeId, second: NodeId) -> bool {
    if same_image_url(dom, first, second) {
        return true;
    }
    if lightbox_links_to(dom, first, second) || lightbox_links_to(dom, second, first) {
        return true;
    }
    same_meaningful_alt(dom, first, second)
        && (is_hydration_placeholder(dom, first)
            || is_hydration_placeholder(dom, second)
            || responsive_quality(dom, first) != responsive_quality(dom, second))
}

fn image_quality(dom: &Dom, image: NodeId) -> (bool, Option<(u8, u32)>, u64) {
    let real = !is_hydration_placeholder(dom, image);
    let responsive = responsive_quality(dom, image);
    let declared_width = dom
        .attr(image, AttrName::Width)
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);
    let declared_height = dom
        .attr(image, AttrName::Height)
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);
    (real, responsive, declared_width * declared_height)
}

fn responsive_quality(dom: &Dom, image: NodeId) -> Option<(u8, u32)> {
    dom.attr(image, AttrName::Srcset)
        .and_then(best_srcset_candidate)
        .map(|candidate| match candidate.descriptor {
            SrcsetDescriptor::Width(width) => (2, width),
            SrcsetDescriptor::Density(density) => (1, (density * 1_000.0) as u32),
            SrcsetDescriptor::None => (0, 0),
        })
}

fn image_container(dom: &Dom, image: NodeId) -> NodeId {
    let mut container = image;
    for ancestor in dom.ancestors(image).take(3) {
        if is_image_only_wrapper(dom, ancestor, image) {
            container = ancestor;
        } else {
            break;
        }
    }
    container
}

fn image_container_within(dom: &Dom, image: NodeId, boundary: NodeId) -> NodeId {
    let mut container = image;
    for ancestor in dom.ancestors(image) {
        if ancestor == boundary {
            break;
        }
        if is_image_only_wrapper(dom, ancestor, image) {
            container = ancestor;
        } else {
            break;
        }
    }
    container
}

fn is_image_only_wrapper(dom: &Dom, wrapper: NodeId, image: NodeId) -> bool {
    matches!(
        dom.tag(wrapper),
        Some(Tag::A | Tag::Picture | Tag::Span | Tag::Div)
    ) && single_image(dom, wrapper) == Some(image)
        && std::iter::once(wrapper)
            .chain(dom.descendants(wrapper))
            .all(|node| {
                dom.text_node(node)
                    .is_some_and(|text| text.trim().is_empty())
                    || dom.is_comment(node)
                    || matches!(
                        dom.tag(node),
                        Some(Tag::A | Tag::Picture | Tag::Source | Tag::Span | Tag::Div | Tag::Img)
                    )
            })
}

fn lightbox_links_to(dom: &Dom, thumbnail: NodeId, full: NodeId) -> bool {
    let full_src = dom.attr(full, AttrName::Src);
    dom.ancestors(thumbnail)
        .find(|&ancestor| dom.tag(ancestor) == Some(Tag::A))
        .and_then(|anchor| dom.attr(anchor, AttrName::Href))
        .is_some_and(|href| Some(href) == full_src)
}

fn same_meaningful_alt(dom: &Dom, first: NodeId, second: NodeId) -> bool {
    let Some(first) = dom.attr_by_local_name(first, "alt").map(str::trim) else {
        return false;
    };
    first.len() > 3
        && !first.eq_ignore_ascii_case("image")
        && dom
            .attr_by_local_name(second, "alt")
            .is_some_and(|second| first.eq_ignore_ascii_case(second.trim()))
}

fn has_meaningful_image_description(dom: &Dom, image: NodeId) -> bool {
    let meaningful = |value: &str| {
        let value = value.trim().to_lowercase();
        !value.is_empty()
            && ![
                "image unavailable",
                "unavailable image",
                "placeholder",
                "loading",
                "blank image",
            ]
            .iter()
            .any(|label| value == *label)
    };
    if ["alt", "aria-label", "title"]
        .into_iter()
        .filter_map(|name| dom.attr_by_local_name(image, name))
        .any(meaningful)
    {
        return true;
    }
    dom.ancestors(image)
        .find(|&ancestor| dom.tag(ancestor) == Some(Tag::Figure))
        .is_some_and(|figure| {
            dom.descendants(figure)
                .filter(|&node| dom.tag(node) == Some(Tag::Img))
                .take(2)
                .count()
                == 1
                && dom.descendants(figure).any(|node| {
                    dom.tag(node) == Some(Tag::Figcaption) && dom.has_non_whitespace_text(node)
                })
        })
}

fn valid_image_attribute(value: Option<&str>) -> Option<&str> {
    value.filter(|value| !value.trim().is_empty() && !value.trim().eq_ignore_ascii_case("null"))
}

fn image_has_placeholder_source(dom: &Dom, image: NodeId) -> bool {
    dom.attr(image, AttrName::Src).is_none_or(|source| {
        let source = source.to_ascii_lowercase();
        source.contains("placeholder")
            || source.contains("blank.gif")
            || source.contains("spacer.gif")
            || source.contains("transparent.gif")
            || crate::constants::parse_b64_data_url(&source)
                .is_some_and(|(end, _)| source.len().saturating_sub(end) < 133)
    })
}

fn image_has_orphan_placeholder_source(dom: &Dom, image: NodeId) -> bool {
    dom.attr(image, AttrName::Src).is_some_and(|source| {
        let path = source
            .split(['?', '#'])
            .next()
            .unwrap_or(source)
            .trim_end_matches('/');
        let basename = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
        let stem = basename
            .rsplit_once('.')
            .map_or(basename.as_str(), |(stem, _)| stem);
        matches!(
            basename.as_str(),
            "blank.gif" | "spacer.gif" | "transparent.gif"
        ) || matches!(
            stem,
            "placeholder" | "grey-placeholder" | "image-placeholder" | "photo-placeholder"
        )
    })
}

fn previous_element_sibling(dom: &Dom, node: NodeId) -> Option<NodeId> {
    let mut previous = dom.prev_sibling(node);
    while let Some(candidate) = previous {
        if dom.is_element(candidate) {
            return Some(candidate);
        }
        if dom
            .text_node(candidate)
            .is_some_and(|text| !text.trim().is_empty())
        {
            return None;
        }
        previous = dom.prev_sibling(candidate);
    }
    None
}

fn single_image(dom: &Dom, node: NodeId) -> Option<NodeId> {
    if dom.tag(node) == Some(Tag::Img) {
        return Some(node);
    }
    if dom.has_non_whitespace_text(node) {
        return None;
    }
    let mut images = dom
        .descendants(node)
        .filter(|&descendant| dom.tag(descendant) == Some(Tag::Img));
    let image = images.next()?;
    images.next().is_none().then_some(image)
}

fn same_image_url(dom: &Dom, first: NodeId, second: NodeId) -> bool {
    [AttrName::Src, AttrName::DataSrc]
        .into_iter()
        .any(|attribute| {
            dom.attr(first, attribute)
                .filter(|value| !value.is_empty())
                .is_some_and(|value| dom.attr(second, attribute) == Some(value))
        })
}

fn is_hydration_placeholder(dom: &Dom, image: NodeId) -> bool {
    let named = [AttrName::Class, AttrName::Id]
        .into_iter()
        .filter_map(|attribute| dom.attr(image, attribute))
        .any(|value| {
            let value = value.to_ascii_lowercase();
            value.contains("placeholder") || value.contains("hydration")
        });
    named
        || image_has_placeholder_source(dom, image)
        || [AttrName::Width, AttrName::Height]
            .into_iter()
            .filter_map(|attribute| dom.attr(image, attribute)?.parse::<u32>().ok())
            .any(|size| size <= 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown::dom_to_markdown;

    fn normalized(html: &str) -> (Dom, NodeId) {
        let mut dom = Dom::parse_document(html).unwrap();
        let root = dom.body().unwrap();
        normalize(&mut dom, root, &mut Vec::new());
        (dom, root)
    }

    #[test]
    fn parses_and_selects_srcset_candidates() {
        assert_eq!(
            best_srcset_candidate("small.jpg 320w, medium.jpg 640w, large.jpg 1280w")
                .map(|candidate| candidate.url),
            Some("large.jpg")
        );
        assert_eq!(
            best_srcset_candidate("small.jpg 1x, large.jpg 2x").map(|candidate| candidate.url),
            Some("large.jpg")
        );
        assert_eq!(
            best_srcset_candidate("image.jpg?crop=1,2 900w, fallback.jpg 400w")
                .map(|candidate| candidate.url),
            Some("image.jpg?crop=1,2")
        );
        assert_eq!(
            best_srcset_candidate("broken.jpg nope, valid.jpg 2x").map(|candidate| candidate.url),
            Some("valid.jpg")
        );
        assert_eq!(
            best_srcset_candidate("first.jpg, second.jpg").map(|candidate| candidate.url),
            Some("first.jpg")
        );
    }

    #[test]
    fn selects_the_best_picture_and_lazy_sources() {
        let (dom, root) = normalized(
            r#"<picture><source data-srcset="small.webp 400w, large.webp 1200w"><img src="blank.gif" alt="View"></picture>"#,
        );
        assert_eq!(dom_to_markdown(&dom, root, 0), "![View](large.webp)\n");
        let source = dom.first_descendant_by_tag(root, Tag::Source).unwrap();
        assert_eq!(
            dom.attr(source, AttrName::Srcset),
            Some("small.webp 400w, large.webp 1200w")
        );
    }

    #[test]
    fn uses_one_picture_source_without_comparing_media_descriptors() {
        let (dom, root) = normalized(
            r#"<picture><source media="(max-width: 600px)" srcset="mobile.jpg 2000w"><source srcset="default.jpg 1200w"><img src="fallback.jpg" alt="View"></picture>"#,
        );
        assert_eq!(dom_to_markdown(&dom, root, 0), "![View](mobile.jpg)\n");
    }

    #[test]
    fn removes_local_duplicates_and_keeps_distant_repetitions() {
        let (dom, root) = normalized(
            r#"<div><img class="lazy placeholder" src="same.jpg" alt="Diagram"></div><div><img src="same.jpg" srcset="same.jpg 1x, large.jpg 2x" alt="Diagram"></div><p>Discussion between uses.</p><img src="same.jpg" alt="Diagram">"#,
        );
        assert_eq!(
            dom.descendants(root)
                .filter(|&node| dom.tag(node) == Some(Tag::Img))
                .count(),
            2
        );
        assert!(dom_to_markdown(&dom, root, 0).contains("large.jpg"));
    }

    #[test]
    fn prefers_a_real_image_over_a_named_placeholder() {
        let (dom, root) = normalized(
            r#"<img class="hydration-placeholder" src="same.jpg" alt="Diagram"><img src="same.jpg" alt="Diagram">"#,
        );
        let image = dom.first_descendant_by_tag(root, Tag::Img).unwrap();
        assert_eq!(
            dom.descendants(root)
                .filter(|&node| dom.tag(node) == Some(Tag::Img))
                .count(),
            1
        );
        assert!(dom.attr(image, AttrName::Class).is_none());
    }

    #[test]
    fn chooses_full_lightbox_image_and_ignores_generic_alt_text() {
        let (dom, root) = normalized(
            r#"<a href="full.jpg"><img src="thumb.jpg" alt="Scene"></a><img src="full.jpg" alt="Scene"><p>Break</p><img src="one.jpg" srcset="one.jpg 1x, one-large.jpg 2x" alt="image"><img src="two.jpg" alt="image">"#,
        );
        let markdown = dom_to_markdown(&dom, root, 0);
        assert!(!markdown.contains("thumb.jpg"));
        assert!(markdown.contains("full.jpg"));
        assert!(markdown.contains("one-large.jpg"));
        assert!(markdown.contains("two.jpg"));
    }

    #[test]
    fn recognizes_density_responsive_duplicates() {
        let (dom, root) = normalized(
            r#"<img src="small.jpg" alt="Detailed diagram"><img src="small.jpg" srcset="small.jpg 1x, large.jpg 2x" alt="Detailed diagram">"#,
        );
        assert_eq!(
            dom.descendants(root)
                .filter(|&node| dom.tag(node) == Some(Tag::Img))
                .count(),
            1
        );
        assert_eq!(
            dom_to_markdown(&dom, root, 0),
            "![Detailed diagram](large.jpg)\n"
        );
    }

    #[test]
    fn retains_the_higher_density_duplicate() {
        let (dom, root) = normalized(
            r#"<img src="small.jpg" srcset="small.jpg 1x" alt="Detailed diagram"><img src="small.jpg" srcset="small.jpg 1x, large.jpg 2x" alt="Detailed diagram">"#,
        );
        assert_eq!(
            dom.descendants(root)
                .filter(|&node| dom.tag(node) == Some(Tag::Img))
                .count(),
            1
        );
        assert_eq!(
            dom_to_markdown(&dom, root, 0),
            "![Detailed diagram](large.jpg)\n"
        );
    }

    #[test]
    fn a_loaded_lazy_class_is_not_placeholder_evidence() {
        let (dom, root) = normalized(
            r#"<img src="small.jpg" alt="Detailed diagram"><img class="lazyloaded" src="small.jpg" srcset="small.jpg 1x, large.jpg 2x" alt="Detailed diagram">"#,
        );
        assert_eq!(
            dom.descendants(root)
                .filter(|&node| dom.tag(node) == Some(Tag::Img))
                .count(),
            1
        );
        assert_eq!(
            dom_to_markdown(&dom, root, 0),
            "![Detailed diagram](large.jpg)\n"
        );
    }

    #[test]
    fn duplicate_cleanup_keeps_non_image_siblings() {
        let (dom, root) = normalized(
            r#"<div><img class="placeholder" src="same.jpg" alt="View"><video src="clip.mp4"></video></div><img src="same.jpg" alt="View">"#,
        );
        assert!(dom.first_descendant_by_tag(root, Tag::Video).is_some());
    }

    #[test]
    fn deduplicates_matching_images_inside_one_figure() {
        let (dom, root) = normalized(
            r#"<figure><img src="plot.jpg" alt="Plot"><span><img src="plot.jpg" alt="Plot"></span><figcaption>Results</figcaption></figure>"#,
        );
        assert_eq!(
            dom.descendants(root)
                .filter(|&node| dom.tag(node) == Some(Tag::Img))
                .count(),
            1
        );
    }
}
