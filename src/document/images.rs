//! Source image recognition for semantic document compilation.

use url::Url;

use crate::dom::{AttrName, Dom, NodeId, Tag};

use super::{DestinationKind, safe_destination};

#[derive(Clone, Copy, Debug, PartialEq)]
enum Descriptor {
    Width(u32),
    Density(f32),
    None,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Candidate<'a> {
    url: &'a str,
    descriptor: Descriptor,
}

pub(crate) struct ImageAnalysis {
    pub(super) sources: Vec<Option<Box<str>>>,
    synthetic: Vec<bool>,
}

impl ImageAnalysis {
    pub(crate) fn source(&self, node: NodeId) -> Option<&str> {
        self.sources.get(node.index()).and_then(Option::as_deref)
    }

    pub(crate) fn is_synthetic(&self, node: NodeId) -> bool {
        self.synthetic.get(node.index()).copied().unwrap_or(false)
    }
}

/// Selects image sources and synthetic image containers in linear passes.
pub(crate) fn analyze(dom: &Dom, nodes: &[NodeId], base_url: Option<&Url>) -> ImageAnalysis {
    if !nodes
        .iter()
        .any(|&node| matches!(dom.tag(node), Some(Tag::Img | Tag::Picture | Tag::Figure)))
    {
        return ImageAnalysis {
            sources: Vec::new(),
            synthetic: Vec::new(),
        };
    }
    let mut nearest_picture = vec![None; dom.len()];
    for &node in nodes {
        nearest_picture[node.index()] = if dom.tag(node) == Some(Tag::Picture) {
            Some(node)
        } else {
            dom.parent(node)
                .and_then(|parent| nearest_picture[parent.index()])
        };
    }

    let mut picture_sources = vec![None; dom.len()];
    for &node in nodes {
        if dom.tag(node) == Some(Tag::Picture) {
            picture_sources[node.index()] = select_node_source(dom, node, base_url);
        } else if dom.tag(node) == Some(Tag::Source)
            && let Some(picture) = nearest_picture[node.index()]
            && picture_sources[picture.index()].is_none()
        {
            picture_sources[picture.index()] = select_node_source(dom, node, base_url);
        }
    }

    let mut descendant_images = vec![false; dom.len()];
    for &node in nodes.iter().rev() {
        descendant_images[node.index()] = dom.tag(node) == Some(Tag::Img)
            || dom
                .children(node)
                .any(|child| descendant_images[child.index()]);
    }

    let mut sources = (0..dom.len()).map(|_| None).collect::<Vec<_>>();
    let mut synthetic = vec![false; dom.len()];
    for &node in nodes {
        match dom.tag(node) {
            Some(Tag::Img) => {
                sources[node.index()] = nearest_picture[node.index()]
                    .and_then(|picture| picture_sources[picture.index()].clone())
                    .or_else(|| select_node_source(dom, node, base_url));
            }
            Some(Tag::Picture | Tag::Figure) if !descendant_images[node.index()] => {
                sources[node.index()] = if dom.tag(node) == Some(Tag::Picture) {
                    picture_sources[node.index()].clone()
                } else {
                    select_node_source(dom, node, base_url)
                };
                synthetic[node.index()] = sources[node.index()].is_some();
            }
            _ => {}
        }
    }
    ImageAnalysis { sources, synthetic }
}

pub(crate) fn canonical_label(value: Option<&str>) -> Box<str> {
    value
        .unwrap_or_default()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .into_boxed_str()
}

fn select_node_source(dom: &Dom, node: NodeId, base_url: Option<&Url>) -> Option<Box<str>> {
    let srcsets = if dom.tag(node) == Some(Tag::Img) {
        [AttrName::Srcset, AttrName::DataSrcset]
    } else {
        [AttrName::DataSrcset, AttrName::Srcset]
    };
    for attribute in srcsets {
        if let Some(srcset) = usable_attribute(dom.attr(node, attribute))
            && let Some(source) = best_safe_candidate(srcset, base_url)
        {
            return Some(source);
        }
    }
    let direct_sources = if dom.tag(node) == Some(Tag::Img) {
        [AttrName::Src, AttrName::DataSrc]
    } else {
        [AttrName::DataSrc, AttrName::Src]
    };
    for attribute in direct_sources {
        if let Some(source) = usable_attribute(dom.attr(node, attribute))
            .filter(|source| !is_placeholder(source))
            .and_then(|source| safe_destination(source, base_url, DestinationKind::Resource))
        {
            return Some(source);
        }
    }

    // Some lazy loaders use custom data attributes. Keep this fallback local
    // to compilation so implementation attributes do not enter the IR.
    for attribute in dom.attrs(node) {
        let value = attribute.value.as_ref();
        if looks_like_srcset(value)
            && let Some(source) = best_safe_candidate(value, base_url)
        {
            return Some(source);
        }
        if looks_like_image_url(value)
            && !is_placeholder(value)
            && let Some(source) = safe_destination(value, base_url, DestinationKind::Resource)
        {
            return Some(source);
        }
    }
    None
}

fn best_safe_candidate(srcset: &str, base_url: Option<&Url>) -> Option<Box<str>> {
    parse_srcset(srcset)
        .into_iter()
        .filter(|candidate| !is_placeholder(candidate.url))
        .filter_map(|candidate| {
            safe_destination(candidate.url, base_url, DestinationKind::Resource)
                .map(|source| (candidate.descriptor, source))
        })
        .reduce(|best, candidate| {
            if better(candidate.0, best.0) {
                candidate
            } else {
                best
            }
        })
        .map(|(_, source)| source)
}

fn looks_like_srcset(value: &str) -> bool {
    parse_srcset(value).into_iter().any(|candidate| {
        !matches!(candidate.descriptor, Descriptor::None) && looks_like_image_url(candidate.url)
    })
}

fn looks_like_image_url(value: &str) -> bool {
    let value = value.trim();
    if value.is_empty() || value.contains(char::is_whitespace) {
        return false;
    }
    let path = value.split(['?', '#']).next().unwrap_or(value);
    let extension = path.rsplit_once('.').map(|(_, extension)| extension);
    extension.is_some_and(|extension| {
        ["jpg", "jpeg", "png", "gif", "webp", "avif", "svg", "bmp"]
            .iter()
            .any(|candidate| extension.eq_ignore_ascii_case(candidate))
    })
}

fn usable_attribute(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| {
        !value.is_empty()
            && !value.eq_ignore_ascii_case("null")
            && !value.eq_ignore_ascii_case("undefined")
    })
}

fn is_placeholder(value: &str) -> bool {
    let source = value.trim().to_ascii_lowercase();
    if source.starts_with("data:") {
        return true;
    }
    let path = source
        .split(['?', '#'])
        .next()
        .unwrap_or(&source)
        .trim_end_matches('/');
    let basename = path.rsplit('/').next().unwrap_or(path);
    let stem = basename.rsplit_once('.').map_or(basename, |(stem, _)| stem);
    matches!(
        basename,
        "blank.gif" | "spacer.gif" | "transparent.gif" | "pixel.gif"
    ) || matches!(
        stem,
        "placeholder" | "grey-placeholder" | "image-placeholder" | "photo-placeholder"
    )
}

fn parse_srcset(srcset: &str) -> Vec<Candidate<'_>> {
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
        let start = position;
        while position < bytes.len() && !bytes[position].is_ascii_whitespace() {
            position += 1;
        }
        let mut end = position;
        while end > start && bytes[end - 1] == b',' {
            end -= 1;
        }
        let ended_with_comma = end != position;
        let url = &srcset[start..end];
        let descriptor = if ended_with_comma {
            Some(Descriptor::None)
        } else {
            let start = position;
            while position < bytes.len() && bytes[position] != b',' {
                position += 1;
            }
            parse_descriptor(srcset[start..position].trim())
        };
        if !url.is_empty()
            && let Some(descriptor) = descriptor
        {
            candidates.push(Candidate { url, descriptor });
        }
        if position < bytes.len() {
            position += 1;
        }
    }
    candidates
}

fn parse_descriptor(value: &str) -> Option<Descriptor> {
    if value.is_empty() {
        return Some(Descriptor::None);
    }
    if value.split_ascii_whitespace().count() != 1 {
        return None;
    }
    if let Some(width) = value.strip_suffix('w') {
        return width
            .parse::<u32>()
            .ok()
            .filter(|width| *width > 0)
            .map(Descriptor::Width);
    }
    if let Some(density) = value.strip_suffix('x') {
        return density
            .parse::<f32>()
            .ok()
            .filter(|density| density.is_finite() && *density > 0.0)
            .map(Descriptor::Density);
    }
    None
}

fn better(candidate: Descriptor, current: Descriptor) -> bool {
    match (candidate, current) {
        (Descriptor::Width(left), Descriptor::Width(right)) => left > right,
        (Descriptor::Width(_), _) => true,
        (_, Descriptor::Width(_)) => false,
        (Descriptor::Density(left), Descriptor::Density(right)) => left > right,
        (Descriptor::Density(_), Descriptor::None) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn selected(html: &str) -> Option<Box<str>> {
        let dom = Dom::parse_fragment(html, Tag::Div).unwrap();
        let nodes: Vec<_> = std::iter::once(dom.root())
            .chain(dom.descendants(dom.root()))
            .collect();
        let image = dom.first_descendant_by_tag(dom.root(), Tag::Img)?;
        analyze(&dom, &nodes, None).source(image).map(Into::into)
    }

    #[test]
    fn selects_picture_lazy_and_responsive_sources_without_mutation() {
        for (html, expected) in [
            (
                r#"<picture><source data-srcset="small.webp 400w, large.webp 1200w"><img src="blank.gif"></picture>"#,
                "large.webp",
            ),
            (r#"<img srcset="small.jpg 1x, large.jpg 2x">"#, "large.jpg"),
            (r#"<img src="pixel.gif" data-src="real.jpg">"#, "real.jpg"),
            (
                r#"<picture><source src="small.jpg" data-src="large.jpg"><img src="fallback.jpg"></picture>"#,
                "large.jpg",
            ),
        ] {
            assert_eq!(selected(html).as_deref(), Some(expected));
        }
    }

    #[test]
    fn ranks_only_policy_valid_srcset_candidates() {
        assert_eq!(
            selected(r#"<img srcset="javascript:bad 2000w, safe.jpg 1000w">"#).as_deref(),
            Some("safe.jpg")
        );
    }

    #[test]
    fn active_srcset_precedes_stale_lazy_data() {
        assert_eq!(
            selected(
                r#"<img srcset="current.jpg 2x" data-srcset="stale.jpg 3x" src="fallback.jpg">"#
            )
            .as_deref(),
            Some("current.jpg")
        );
    }
}
