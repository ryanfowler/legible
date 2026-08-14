use crate::dom::{AttrName, Dom, NodeId, Tag};
use smallvec::SmallVec;
use std::borrow::Cow;
use std::collections::HashMap;
use url::form_urlencoded;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ImageVariant {
    Mobile,
    Desktop,
}

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

/// Returns one high-confidence lead-media sibling for a narrow prose root.
///
/// Structural root selection can correctly choose a focused prose wrapper
/// while leaving its lead image near the selected root. Keep the expansion
/// local so a remote header image cannot enter the article.
pub(super) fn adjacent_lead_media(dom: &Dom, content_root: NodeId) -> Option<NodeId> {
    std::iter::once(content_root)
        .chain(dom.ancestors(content_root).take(8))
        .flat_map(|node| {
            std::iter::successors(previous_element_sibling(dom, node), |&sibling| {
                previous_element_sibling(dom, sibling)
            })
            .take(8)
        })
        .find(|&candidate| {
            let image = match dom.tag(candidate) {
                Some(Tag::Img) => Some(candidate),
                Some(Tag::P) => single_image(dom, candidate),
                Some(Tag::Figure | Tag::Picture) => sole_descendant_image(dom, candidate),
                _ => None,
            };
            image.is_some_and(|image| {
                (matches!(dom.tag(candidate), Some(Tag::Figure | Tag::Picture))
                    || has_responsive_candidate(dom, image)
                    || static_dimensions(dom, image)
                        .into_iter()
                        .flatten()
                        .any(|dimension| dimension >= 200))
                    && has_meaningful_image_description(dom, image)
                    && !has_adjacent_lead_peripheral_role(dom, image)
                    && image_role_score(dom, image, true, 1).is_meaningful()
            })
        })
}

pub(super) fn remove_decorative_media(dom: &mut Dom, root: NodeId) {
    let nodes = dom.element_descendants_snapshot_with_depth(root);
    let mut responsive_content = vec![false; dom.len()];
    let mut svg_description_content = vec![false; dom.len()];
    for &(node, _) in nodes.iter().rev() {
        responsive_content[node.index()] |= source_has_responsive_or_lazy_image(dom, node);
        svg_description_content[node.index()] |=
            is_svg_description_element(dom, node) && dom.has_non_whitespace_text(node);
        if let Some(parent) = dom.parent(node) {
            responsive_content[parent.index()] |= responsive_content[node.index()];
            svg_description_content[parent.index()] |= svg_description_content[node.index()];
        }
    }

    let mut protected_context = vec![false; dom.len()];
    let mut responsive_picture_context = vec![false; dom.len()];
    let mut contexts: Vec<(bool, bool)> = Vec::new();
    let root_context = (
        dom.attr(root, AttrName::DataMath).is_some(),
        dom.tag(root) == Some(Tag::Picture) && responsive_content[root.index()],
    );
    for &(node, depth) in &nodes {
        while contexts.len() >= depth as usize {
            contexts.pop();
        }
        let (parent_protected, parent_responsive) =
            contexts.last().copied().unwrap_or(root_context);
        let protected = parent_protected || dom.attr(node, AttrName::DataMath).is_some();
        let responsive = parent_responsive
            || dom.tag(node) == Some(Tag::Picture) && responsive_content[node.index()];
        protected_context[node.index()] = protected;
        responsive_picture_context[node.index()] = responsive;
        contexts.push((protected, responsive));
    }

    let first_paragraph = nodes
        .iter()
        .position(|&(node, _)| dom.tag(node) == Some(Tag::P));
    let mut positions = vec![usize::MAX; dom.len()];
    for (position, &(node, _)) in nodes.iter().enumerate() {
        positions[node.index()] = position;
    }
    let mut repetitions: HashMap<String, u16> = HashMap::new();
    for &(node, _) in &nodes {
        if dom.tag(node) == Some(Tag::Img)
            && let Some(resource) = primary_image_resource(dom, node)
        {
            let count = repetitions.entry(resource).or_default();
            *count = count.saturating_add(1);
        }
    }

    // Controls often provide a lightbox around a real figure. Hard cleanup
    // removes controls, so move high-confidence media out of them first.
    let controls: SmallVec<[NodeId; 8]> = nodes
        .iter()
        .filter_map(|&(node, _)| (dom.tag(node) == Some(Tag::Button)).then_some(node))
        .collect();
    for control in controls {
        if dom.parent(control).is_none() {
            continue;
        }
        let Some(image) = dom
            .descendants(control)
            .find(|&node| dom.tag(node) == Some(Tag::Img))
        else {
            continue;
        };
        let media_ancestors: SmallVec<[NodeId; 4]> = dom
            .ancestors(image)
            .take_while(|&ancestor| ancestor != control)
            .collect();
        let media = media_ancestors
            .iter()
            .copied()
            .find(|&ancestor| dom.tag(ancestor) == Some(Tag::Figure))
            .or_else(|| {
                media_ancestors
                    .iter()
                    .copied()
                    .find(|&ancestor| dom.tag(ancestor) == Some(Tag::Picture))
            })
            .unwrap_or(image);
        let role = image_role_score(
            dom,
            image,
            first_paragraph.is_some_and(|first| positions[image.index()] < first),
            repetitions
                .get(&primary_image_resource(dom, image).unwrap_or_default())
                .copied()
                .unwrap_or(1),
        );
        if role.is_meaningful() {
            dom.insert_before(control, media);
        }
    }

    for &(node, _) in nodes.iter().rev() {
        if dom.parent(node).is_none() || !matches!(dom.tag(node), Some(Tag::Img | Tag::Svg)) {
            continue;
        }
        if dom.tag(node) == Some(Tag::Svg) && svg_description_content[node.index()]
            || protected_context[node.index()]
        {
            continue;
        }
        let repeated = primary_image_resource(dom, node)
            .and_then(|resource| repetitions.get(&resource).copied())
            .unwrap_or(1);
        let mut role = image_role_score(
            dom,
            node,
            first_paragraph.is_some_and(|first| positions[node.index()] < first),
            repeated,
        );
        let strong_peripheral = has_strong_peripheral_role(dom, node);
        if !strong_peripheral
            && (responsive_picture_context[node.index()]
                || has_responsive_candidate(dom, node)
                || has_lazy_candidate(dom, node))
        {
            role.positive += 9;
        }
        if role.should_remove() {
            dom.detach(node);
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct ImageRoleEvidence {
    descriptive: bool,
    captioned: bool,
    responsive: bool,
    meaningful_dimensions: bool,
    lead_position: bool,
    figure: bool,
    small_dimensions: bool,
    tracking_dimensions: bool,
    profile_context: bool,
    author_media_context: bool,
    avatar_context: bool,
    related_context: bool,
    promotional_context: bool,
    media_control_context: bool,
    logo_or_icon_context: bool,
    card_grid_context: bool,
    repetitions: u16,
}

impl ImageRoleEvidence {
    fn collect(dom: &Dom, image: NodeId, before_first_paragraph: bool, repetitions: u16) -> Self {
        let dimensions = static_dimensions(dom, image);
        let tracking_dimensions = dimensions
            .into_iter()
            .flatten()
            .any(|dimension| dimension <= 1);
        let small_dimensions = dimensions
            .into_iter()
            .flatten()
            .any(|dimension| dimension <= 32);
        let meaningful_dimensions = dimensions
            .into_iter()
            .flatten()
            .max()
            .is_some_and(|size| size >= 200);
        let names = image_context_name(dom, image);
        let structural_names = image_structural_context_name(dom, image);
        Self {
            descriptive: has_meaningful_image_description(dom, image),
            captioned: has_figure_caption(dom, image),
            responsive: has_responsive_candidate(dom, image) || has_lazy_candidate(dom, image),
            meaningful_dimensions,
            lead_position: is_lead_position(dom, image, before_first_paragraph),
            figure: dom
                .ancestors(image)
                .any(|ancestor| dom.tag(ancestor) == Some(Tag::Figure)),
            small_dimensions,
            tracking_dimensions,
            profile_context: contains_role_token(
                &names,
                &[
                    "avatar", "avatars", "author", "authors", "byline", "founder", "founders",
                    "profile", "bio",
                ],
            ),
            author_media_context: contains_role_token(
                &names,
                &["author", "authors", "byline", "profile", "bio"],
            ) && contains_role_token(
                &names,
                &["avatar", "headshot", "image", "photo", "portrait"],
            ),
            avatar_context: contains_role_token(&names, &["avatar", "avatars"]),
            related_context: contains_role_token(
                &names,
                &[
                    "related",
                    "recommend",
                    "recirculation",
                    "more-stories",
                    "more_articles",
                    "tout",
                ],
            ),
            promotional_context: contains_role_token(
                &structural_names,
                &[
                    "ad",
                    "ads",
                    "advert",
                    "advertisement",
                    "advertorial",
                    "marketing",
                    "promo",
                    "promotion",
                    "promotional",
                    "sponsor",
                    "sponsored",
                ],
            ),
            media_control_context: has_media_control_context(dom, image),
            logo_or_icon_context: contains_role_token(
                &names,
                &["favicon", "logo", "icon", "badge", "sprite", "integration"],
            ),
            card_grid_context: contains_role_token(&names, &["card", "cards", "grid", "tile"]),
            repetitions,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct ImageRoleScore {
    positive: i16,
    negative: i16,
}

impl ImageRoleScore {
    fn is_meaningful(self) -> bool {
        self.positive >= 4 && self.positive > self.negative
    }

    fn should_remove(self) -> bool {
        self.negative >= 5 && self.negative >= self.positive + 2
    }
}

fn image_role_score(
    dom: &Dom,
    image: NodeId,
    before_first_paragraph: bool,
    repetitions: u16,
) -> ImageRoleScore {
    let evidence = ImageRoleEvidence::collect(dom, image, before_first_paragraph, repetitions);
    let mut score = ImageRoleScore::default();

    score.positive += i16::from(evidence.descriptive) * 7;
    score.positive += i16::from(evidence.captioned) * 7;
    score.positive += i16::from(evidence.responsive) * 3;
    score.positive += i16::from(evidence.meaningful_dimensions) * 2;
    score.positive += i16::from(
        evidence.lead_position
            && (evidence.descriptive
                || evidence.captioned
                || evidence.responsive
                || evidence.meaningful_dimensions),
    ) * 2;
    score.positive += i16::from(evidence.figure) * 2;

    score.negative += i16::from(evidence.small_dimensions && decorative_image_name(dom, image)) * 3;
    score.negative += i16::from(
        evidence.tracking_dimensions
            && (!evidence.descriptive || decorative_image_name(dom, image)),
    ) * 10;
    score.negative += i16::from(evidence.profile_context) * 4;
    score.negative += i16::from(evidence.author_media_context) * 12;
    score.negative += i16::from(evidence.avatar_context) * 8;
    score.negative += i16::from(evidence.logo_or_icon_context) * 5;
    score.negative += i16::from(contains_role_token(
        &image_context_name(dom, image),
        &["favicon", "logo", "integration"],
    )) * 3;
    score.negative += i16::from(evidence.related_context) * 24;
    score.negative += i16::from(evidence.promotional_context) * 24;
    score.negative += i16::from(evidence.media_control_context) * 24;
    score.negative += i16::from(evidence.card_grid_context) * 2;
    score.negative += i16::from(evidence.profile_context && evidence.repetitions >= 2) * 8;
    score.negative += if evidence.repetitions >= 3 {
        4
    } else if evidence.repetitions == 2 {
        2
    } else {
        0
    };
    score
}

fn image_context_name(dom: &Dom, image: NodeId) -> String {
    let mut name = String::new();
    for node in std::iter::once(image).chain(dom.ancestors(image).take(6)) {
        if let Some(tag) = dom.qual_name(node) {
            name.push(' ');
            name.push_str(tag.local.as_ref());
        }
        for attribute in [AttrName::Class, AttrName::Id, AttrName::Role, AttrName::Src] {
            if let Some(value) = dom.attr(node, attribute) {
                name.push(' ');
                name.push_str(value);
            }
        }
    }
    name.to_ascii_lowercase()
}

fn image_structural_context_name(dom: &Dom, image: NodeId) -> String {
    let mut name = String::new();
    for node in std::iter::once(image).chain(dom.ancestors(image).take(6)) {
        if let Some(tag) = dom.qual_name(node) {
            name.push(' ');
            name.push_str(tag.local.as_ref());
        }
        for attribute in [AttrName::Class, AttrName::Id, AttrName::Role] {
            if let Some(value) = dom.attr(node, attribute) {
                name.push(' ');
                name.push_str(value);
            }
        }
    }
    name.to_ascii_lowercase()
}

fn has_media_control_context(dom: &Dom, image: NodeId) -> bool {
    let names = image_structural_context_name(dom, image);
    let list_context = dom
        .ancestors(image)
        .any(|ancestor| matches!(dom.tag(ancestor), Some(Tag::Li | Tag::Ol | Tag::Ul)));
    let list_control_marker = list_context
        && dom
            .ancestors(image)
            .chain(std::iter::once(image))
            .take(6)
            .any(|node| {
                dom.attrs(node).iter().any(|attribute| {
                    let name = attribute.name.local.as_ref();
                    name.starts_with("data-")
                        && (contains_role_token(
                            name,
                            &["carousel", "gallery", "lightbox", "slide", "thumbnail"],
                        ) || contains_role_token(
                            attribute.value.as_ref(),
                            &["carousel", "gallery", "lightbox", "slide", "thumbnail"],
                        ))
                })
            });
    list_control_marker
        || contains_role_token(
            &names,
            &[
                "carousel",
                "gallery",
                "lightbox",
                "slideshow",
                "slider",
                "swiper",
                "thumb",
                "thumbnail",
                "zoom",
            ],
        )
}

fn is_lead_position(dom: &Dom, image: NodeId, before_first_paragraph: bool) -> bool {
    if !before_first_paragraph {
        return false;
    }
    let container = dom
        .ancestors(image)
        .find(|&ancestor| matches!(dom.tag(ancestor), Some(Tag::Figure | Tag::Picture)))
        .unwrap_or(image);
    let mut previous = previous_element_sibling(dom, container);
    let mut preceding = 0;
    while let Some(sibling) = previous {
        preceding += 1;
        if preceding > 4 {
            return false;
        }
        previous = previous_element_sibling(dom, sibling);
    }
    true
}

fn has_strong_peripheral_role(dom: &Dom, image: NodeId) -> bool {
    let names = image_structural_context_name(dom, image);
    contains_role_token(
        &names,
        &[
            "avatar",
            "avatars",
            "author",
            "authors",
            "byline",
            "founder",
            "founders",
            "profile",
            "bio",
            "related",
            "recommend",
            "recirculation",
            "more-stories",
            "more_articles",
            "tout",
            "ad",
            "ads",
            "advert",
            "advertisement",
            "advertorial",
            "marketing",
            "promo",
            "promotion",
            "promotional",
            "sponsor",
            "sponsored",
            "favicon",
            "logo",
            "integration",
        ],
    )
}

fn has_adjacent_lead_peripheral_role(dom: &Dom, image: NodeId) -> bool {
    has_strong_peripheral_role(dom, image)
        || contains_role_token(
            &image_context_name(dom, image),
            &["icon", "badge", "sprite"],
        )
}

fn contains_role_token(value: &str, patterns: &[&str]) -> bool {
    let normalize = |text: &str| {
        text.split(|character: char| !character.is_ascii_alphanumeric())
            .filter(|token| !token.is_empty())
            .collect::<SmallVec<[&str; 8]>>()
            .join(" ")
    };
    patterns.iter().any(|pattern| {
        let pattern = normalize(pattern);
        if !pattern.contains(' ') {
            return value
                .split(|character: char| !character.is_ascii_alphanumeric())
                .any(|token| token == pattern);
        }
        value
            .split_ascii_whitespace()
            .any(|field| normalize(field) == pattern)
    })
}

fn sole_descendant_image(dom: &Dom, root: NodeId) -> Option<NodeId> {
    let mut images = dom
        .descendants(root)
        .filter(|&node| dom.tag(node) == Some(Tag::Img));
    let image = images.next()?;
    images.next().is_none().then_some(image)
}

fn primary_image_resource(dom: &Dom, image: NodeId) -> Option<String> {
    let source = [AttrName::Src, AttrName::DataSrc]
        .into_iter()
        .find_map(|attribute| non_placeholder_image_attribute(dom.attr(image, attribute)))
        .map(str::to_owned)
        .or_else(|| best_responsive_source(dom, image))
        .or_else(|| {
            [AttrName::Srcset, AttrName::DataSrcset]
                .into_iter()
                .find_map(|attribute| dom.attr(image, attribute).and_then(best_srcset_candidate))
                .map(|candidate| candidate.url.to_owned())
        })?;
    let resource = image_resource(&source);
    let resource = resource.split('#').next().unwrap_or(resource.as_ref());
    let identity = if resource.contains("X-Amz-Signature=")
        || resource.contains("X-Amz-Credential=")
        || resource.contains("x-amz-signature=")
        || resource.contains("x-amz-credential=")
    {
        resource.split('?').next().unwrap_or(resource)
    } else {
        resource
    };
    Some(identity.to_owned())
}

fn static_dimensions(dom: &Dom, node: NodeId) -> [Option<u32>; 2] {
    let mut dimensions = [
        dom.attr(node, AttrName::Width)
            .and_then(parse_css_dimension),
        dom.attr(node, AttrName::Height)
            .and_then(parse_css_dimension),
    ];
    if let Some(style) = dom.attr(node, AttrName::Style) {
        for declaration in style.split(';') {
            let Some((name, value)) = declaration.split_once(':') else {
                continue;
            };
            let target = if name.trim().eq_ignore_ascii_case("width") {
                Some(0)
            } else if name.trim().eq_ignore_ascii_case("height") {
                Some(1)
            } else {
                None
            };
            if let Some(target) = target
                && dimensions[target].is_none()
            {
                dimensions[target] = parse_css_dimension(value);
            }
        }
    }
    if dom.tag(node) == Some(Tag::Svg)
        && dimensions.iter().all(Option::is_none)
        && let Some(view_box) = dom
            .attrs(node)
            .iter()
            .find(|attribute| {
                attribute
                    .name
                    .local
                    .as_ref()
                    .eq_ignore_ascii_case("viewbox")
            })
            .map(|attribute| attribute.value.as_ref())
    {
        let values: Vec<_> = view_box
            .split(|character: char| character.is_ascii_whitespace() || character == ',')
            .filter(|value| !value.is_empty())
            .collect();
        if values.len() == 4 {
            dimensions = [
                parse_css_dimension(values[2]),
                parse_css_dimension(values[3]),
            ];
        }
    }
    if dimensions.iter().all(Option::is_none)
        && let Some(source) = dom.attr(node, AttrName::Src)
    {
        dimensions = dimensions_from_url(source);
    }
    dimensions
}

fn parse_css_dimension(value: &str) -> Option<u32> {
    let value = value.trim();
    let number = value.strip_suffix("px").map(str::trim).unwrap_or(value);
    number.parse::<f32>().ok().and_then(|number| {
        (number.is_finite() && number >= 0.0 && number <= u32::MAX as f32)
            .then_some(number.round() as u32)
    })
}

fn dimensions_from_url(source: &str) -> [Option<u32>; 2] {
    let mut dimensions = [None, None];
    let path = source.split(['?', '#']).next().unwrap_or(source);
    for segment in path.split('/') {
        if let Some((width, height)) = segment.split_once('x')
            && width.bytes().all(|byte| byte.is_ascii_digit())
            && height.bytes().all(|byte| byte.is_ascii_digit())
        {
            dimensions = [parse_css_dimension(width), parse_css_dimension(height)];
        }
        let mut tokens = segment.split(['-', '_']);
        while let Some(token) = tokens.next() {
            let target = match token.to_ascii_lowercase().as_str() {
                "w" | "width" => Some(0),
                "h" | "height" => Some(1),
                _ => None,
            };
            if let Some(target) = target
                && let Some(value) = tokens.next()
            {
                dimensions[target] = parse_css_dimension(value);
            }
        }
    }
    if let Some(query) = source.split_once('?').map(|(_, query)| query) {
        for pair in query.split('&') {
            let Some((name, value)) = pair.split_once('=') else {
                continue;
            };
            let target = match name.to_ascii_lowercase().as_str() {
                "w" | "width" => Some(0),
                "h" | "height" => Some(1),
                _ => None,
            };
            if let Some(target) = target {
                dimensions[target] = parse_css_dimension(value);
            }
        }
    }
    dimensions
}

fn has_responsive_candidate(dom: &Dom, node: NodeId) -> bool {
    [AttrName::Srcset, AttrName::DataSrcset]
        .into_iter()
        .filter_map(|attribute| dom.attr(node, attribute))
        .any(|srcset| {
            parse_srcset(srcset).into_iter().any(|candidate| {
                !is_placeholder_resource(candidate.url)
                    && (matches!(candidate.descriptor, SrcsetDescriptor::Width(width) if width > 32)
                        || matches!(candidate.descriptor, SrcsetDescriptor::Density(_)))
            })
        })
}

fn is_placeholder_resource(source: &str) -> bool {
    let source = source.trim().to_ascii_lowercase();
    if matches!(source.as_str(), "null" | "undefined") {
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

fn source_has_responsive_or_lazy_image(dom: &Dom, node: NodeId) -> bool {
    if !matches!(dom.tag(node), Some(Tag::Picture | Tag::Source)) {
        return false;
    }
    [AttrName::Srcset, AttrName::DataSrcset]
        .into_iter()
        .filter_map(|attribute| valid_image_attribute(dom.attr(node, attribute)))
        .any(|value| {
            parse_srcset(value)
                .into_iter()
                .any(|candidate| !is_placeholder_resource(candidate.url))
        })
        || [AttrName::Src, AttrName::DataSrc]
            .into_iter()
            .any(|attribute| non_placeholder_image_attribute(dom.attr(node, attribute)).is_some())
}

fn is_svg_description_element(dom: &Dom, node: NodeId) -> bool {
    dom.qual_name(node).is_some_and(|name| {
        matches!(
            name.local.as_ref().to_ascii_lowercase().as_str(),
            "title" | "desc"
        )
    })
}

fn has_lazy_candidate(dom: &Dom, node: NodeId) -> bool {
    dom.attrs(node).iter().any(|attribute| {
        attribute.name.local.as_ref().starts_with("data-")
            && ((crate::constants::has_image_src(attribute.value.as_ref())
                && non_placeholder_image_attribute(Some(attribute.value.as_ref())).is_some())
                || (crate::constants::has_image_srcset(attribute.value.as_ref())
                    && parse_srcset(attribute.value.as_ref())
                        .into_iter()
                        .any(|candidate| !is_placeholder_resource(candidate.url))))
    })
}

fn decorative_image_name(dom: &Dom, node: NodeId) -> bool {
    [AttrName::Class, AttrName::Id, AttrName::Src]
        .into_iter()
        .filter_map(|attribute| dom.attr(node, attribute))
        .flat_map(|value| {
            value
                .split(|character: char| !character.is_ascii_alphanumeric())
                .filter(|token| !token.is_empty())
        })
        .any(|token| {
            matches!(
                token.to_ascii_lowercase().as_str(),
                "avatar"
                    | "badge"
                    | "bullet"
                    | "icon"
                    | "logo"
                    | "pixel"
                    | "spacer"
                    | "sprite"
                    | "tracking"
            )
        })
}

/// Removes local hydration duplicates while source implementation evidence is intact.
///
/// Source selection is not performed here. The semantic compiler chooses one
/// responsive or lazy resource and discards the implementation wrappers.
pub(super) fn deduplicate_selected(dom: &mut Dom, root: NodeId, nodes: &mut Vec<NodeId>) {
    let source_nodes: Vec<_> = std::iter::once(root).chain(dom.descendants(root)).collect();
    let selected_sources = crate::document::selected_image_sources_for_cleanup(dom, &source_nodes);
    nodes.clear();
    nodes.extend(
        dom.descendants(root)
            .filter(|&node| dom.tag(node) == Some(Tag::Img)),
    );
    deduplicate(dom, nodes, &selected_sources);

    // An unresolved placeholder has no visual content. Accessible text or a
    // sole figure caption can still make the image meaningful.
    for &image in nodes.iter() {
        if dom.parent(image).is_some()
            && has_image_source_evidence(dom, image)
            && !has_usable_image_source(dom, image)
            && !has_meaningful_image_description(dom, image)
        {
            dom.detach(image);
        }
    }
}

fn has_image_source_evidence(dom: &Dom, image: NodeId) -> bool {
    [
        AttrName::Src,
        AttrName::DataSrc,
        AttrName::Srcset,
        AttrName::DataSrcset,
    ]
    .into_iter()
    .any(|attribute| dom.attr(image, attribute).is_some())
        || dom
            .ancestors(image)
            .find(|&ancestor| dom.tag(ancestor) == Some(Tag::Picture))
            .is_some_and(|picture| {
                dom.descendants(picture)
                    .any(|node| dom.tag(node) == Some(Tag::Source))
            })
}

fn has_usable_image_source(dom: &Dom, image: NodeId) -> bool {
    [AttrName::Src, AttrName::DataSrc]
        .into_iter()
        .any(|attribute| non_placeholder_image_attribute(dom.attr(image, attribute)).is_some())
        || [AttrName::Srcset, AttrName::DataSrcset]
            .into_iter()
            .filter_map(|attribute| dom.attr(image, attribute))
            .any(|value| {
                parse_srcset(value)
                    .into_iter()
                    .any(|candidate| !is_placeholder_resource(candidate.url))
            })
        || dom
            .ancestors(image)
            .find(|&ancestor| dom.tag(ancestor) == Some(Tag::Picture))
            .is_some_and(|picture| {
                dom.descendants(picture)
                    .filter(|&node| dom.tag(node) == Some(Tag::Source))
                    .any(|source| source_has_responsive_or_lazy_image(dom, source))
            })
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
            if let Some(candidate) = [AttrName::DataSrcset, AttrName::Srcset]
                .into_iter()
                .filter_map(|attribute| dom.attr(node, attribute))
                .filter_map(|value| valid_image_attribute(Some(value)))
                .find_map(best_non_placeholder_srcset_candidate)
            {
                return Some(candidate.url.to_owned());
            }
            if let Some(src) =
                first_usable_image_attribute(dom, node, [AttrName::DataSrc, AttrName::Src])
            {
                return Some(src.to_owned());
            }
        }
    }

    [AttrName::Srcset, AttrName::DataSrcset]
        .into_iter()
        .filter_map(|attribute| dom.attr(image, attribute))
        .filter_map(|value| valid_image_attribute(Some(value)))
        .find_map(best_non_placeholder_srcset_candidate)
        .map(|candidate| candidate.url.to_owned())
        .or_else(|| {
            let replace = dom.attr(image, AttrName::Src).is_none()
                || image_has_placeholder_source(dom, image);
            replace
                .then(|| first_usable_image_attribute(dom, image, [AttrName::DataSrc]))
                .flatten()
                .map(str::to_owned)
        })
}

fn best_non_placeholder_srcset_candidate(srcset: &str) -> Option<SrcsetCandidate<'_>> {
    parse_srcset(srcset)
        .into_iter()
        .filter(|candidate| !is_placeholder_resource(candidate.url))
        .reduce(|best, candidate| {
            if candidate_is_better(&candidate, &best) {
                candidate
            } else {
                best
            }
        })
}

fn non_placeholder_image_attribute(value: Option<&str>) -> Option<&str> {
    valid_image_attribute(value).filter(|source| !is_placeholder_resource(source))
}

fn first_usable_image_attribute(
    dom: &Dom,
    node: NodeId,
    attributes: impl IntoIterator<Item = AttrName>,
) -> Option<&str> {
    attributes
        .into_iter()
        .filter_map(|attribute| dom.attr(node, attribute))
        .find_map(|value| non_placeholder_image_attribute(Some(value)))
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

fn deduplicate(dom: &mut Dom, nodes: &[NodeId], selected_sources: &[Option<Box<str>>]) {
    // Adjacent wrappers are strong hydration evidence. The pair must also
    // share a source, a meaningful description plus quality evidence, or a
    // lightbox destination.
    for &image in nodes {
        if dom.parent(image).is_none() {
            continue;
        }
        let current_container = image_group_container(dom, image);
        let Some(previous_container) = previous_element_sibling(dom, current_container) else {
            continue;
        };
        let Some(previous_image) = single_media_image(dom, previous_container) else {
            continue;
        };
        let previous_removal = media_group_removal(dom, previous_container, previous_image);
        let current_removal = media_group_removal(dom, current_container, image);
        if !likely_duplicate(dom, previous_image, image, selected_sources) {
            continue;
        }
        let remove = if lightbox_links_to(dom, previous_image, image) {
            previous_removal
        } else if lightbox_links_to(dom, image, previous_image) {
            current_removal
        } else if better_image(dom, previous_image, image) {
            previous_removal
        } else {
            current_removal
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
        if images.len() != 2 || !likely_duplicate(dom, images[0], images[1], selected_sources) {
            continue;
        }
        let remove = if better_image(dom, images[0], images[1]) {
            image_container_within(dom, images[0], figure)
        } else {
            image_container_within(dom, images[1], figure)
        };
        dom.detach(remove);
    }
}

fn likely_duplicate(
    dom: &Dom,
    first: NodeId,
    second: NodeId,
    selected_sources: &[Option<Box<str>>],
) -> bool {
    if same_image_url(first, second, selected_sources) {
        return same_image_url_duplicate(dom, first, second);
    }
    if same_responsive_variant_group(dom, first, second) {
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

fn same_image_url_duplicate(dom: &Dom, first: NodeId, second: NodeId) -> bool {
    let first_figure = dom
        .ancestors(first)
        .find(|&ancestor| dom.tag(ancestor) == Some(Tag::Figure));
    let second_figure = dom
        .ancestors(second)
        .find(|&ancestor| dom.tag(ancestor) == Some(Tag::Figure));
    if first_figure.is_some() && second_figure.is_some() && first_figure != second_figure {
        return match (
            figure_caption_text(dom, first),
            figure_caption_text(dom, second),
        ) {
            (Some(first), Some(second)) => first == second,
            _ => true,
        };
    }
    true
}

fn image_quality(dom: &Dom, image: NodeId) -> (bool, bool, Option<(u8, u32)>, u64) {
    let real = !is_hydration_placeholder(dom, image);
    let descriptive = has_meaningful_image_description(dom, image);
    let responsive = responsive_quality(dom, image).max(next_image_width(dom, image));
    let declared_width = dom
        .attr(image, AttrName::Width)
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);
    let declared_height = dom
        .attr(image, AttrName::Height)
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);
    (
        real,
        descriptive,
        responsive,
        declared_width * declared_height,
    )
}

fn better_image(dom: &Dom, first: NodeId, second: NodeId) -> bool {
    let first_quality = image_quality(dom, first);
    let second_quality = image_quality(dom, second);
    if first_quality != second_quality {
        return second_quality > first_quality;
    }
    image_variant_preference(dom, second) > image_variant_preference(dom, first)
}

fn image_variant_preference(dom: &Dom, image: NodeId) -> u8 {
    match image_variant_identity(dom, image).map(|(_, variant)| variant) {
        Some(ImageVariant::Desktop) => 2,
        None => 1,
        Some(ImageVariant::Mobile) => 0,
    }
}

fn responsive_quality(dom: &Dom, image: NodeId) -> Option<(u8, u32)> {
    dom.attr(image, AttrName::Srcset)
        .or_else(|| dom.attr(image, AttrName::DataSrcset))
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

fn image_group_container(dom: &Dom, image: NodeId) -> NodeId {
    if let Some(figure) = dom.ancestors(image).find(|&ancestor| {
        dom.tag(ancestor) == Some(Tag::Figure)
            && dom
                .descendants(ancestor)
                .filter(|&node| dom.tag(node) == Some(Tag::Img))
                .take(2)
                .count()
                == 1
    }) {
        return figure;
    }
    image_container(dom, image)
}

fn is_media_group_container(dom: &Dom, node: NodeId) -> bool {
    if dom.tag(node) != Some(Tag::Figure)
        || dom
            .descendants(node)
            .filter(|&descendant| dom.tag(descendant) == Some(Tag::Img))
            .take(2)
            .count()
            != 1
    {
        return false;
    }
    dom.descendants(node).all(|descendant| {
        if dom.text_node(descendant).is_some() {
            return dom
                .ancestors(descendant)
                .any(|ancestor| dom.tag(ancestor) == Some(Tag::Figcaption));
        }
        let in_caption = dom
            .ancestors(descendant)
            .any(|ancestor| dom.tag(ancestor) == Some(Tag::Figcaption));
        match dom.tag(descendant) {
            Some(Tag::Img | Tag::Picture | Tag::Source | Tag::A | Tag::Div | Tag::Span) => true,
            Some(Tag::Figcaption) => true,
            Some(
                Tag::B
                | Tag::Br
                | Tag::Code
                | Tag::Em
                | Tag::I
                | Tag::Kbd
                | Tag::Mark
                | Tag::P
                | Tag::Q
                | Tag::Samp
                | Tag::Small
                | Tag::Strong
                | Tag::Sub
                | Tag::Sup
                | Tag::U,
            ) => in_caption,
            _ => false,
        }
    })
}

fn media_group_removal(dom: &Dom, container: NodeId, image: NodeId) -> NodeId {
    if is_media_group_container(dom, container) {
        container
    } else {
        image_container(dom, image)
    }
}

fn single_media_image(dom: &Dom, node: NodeId) -> Option<NodeId> {
    if dom.tag(node) == Some(Tag::Img) {
        return Some(node);
    }
    if matches!(dom.tag(node), Some(Tag::Figure | Tag::Picture)) {
        let mut images = dom
            .descendants(node)
            .filter(|&descendant| dom.tag(descendant) == Some(Tag::Img));
        let image = images.next()?;
        return images.next().is_none().then_some(image);
    }
    single_image(dom, node)
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
        Some(Tag::A | Tag::Picture | Tag::Span | Tag::Div | Tag::P)
    ) && single_image(dom, wrapper) == Some(image)
        && std::iter::once(wrapper)
            .chain(dom.descendants(wrapper))
            .all(|node| {
                dom.text_node(node)
                    .is_some_and(|text| text.trim().is_empty())
                    || dom.is_comment(node)
                    || matches!(
                        dom.tag(node),
                        Some(
                            Tag::A
                                | Tag::Picture
                                | Tag::Source
                                | Tag::Span
                                | Tag::Div
                                | Tag::P
                                | Tag::Img
                        )
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

fn same_responsive_variant_group(dom: &Dom, first: NodeId, second: NodeId) -> bool {
    let Some((first_base, first_variant)) = image_variant_identity(dom, first) else {
        return false;
    };
    let Some((second_base, second_variant)) = image_variant_identity(dom, second) else {
        return false;
    };
    first_variant != second_variant
        && first_base.eq_ignore_ascii_case(&second_base)
        && same_media_description(dom, first, second)
}

fn same_media_description(dom: &Dom, first: NodeId, second: NodeId) -> bool {
    same_meaningful_alt(dom, first, second) || same_figure_caption(dom, first, second)
}

fn same_figure_caption(dom: &Dom, first: NodeId, second: NodeId) -> bool {
    figure_caption_text(dom, first)
        .is_some_and(|first| figure_caption_text(dom, second).is_some_and(|second| first == second))
}

fn figure_caption_text(dom: &Dom, image: NodeId) -> Option<String> {
    let figure = dom
        .ancestors(image)
        .find(|&ancestor| dom.tag(ancestor) == Some(Tag::Figure))?;
    let caption = dom.descendants(figure).find(|&node| {
        dom.tag(node) == Some(Tag::Figcaption) && dom.has_non_whitespace_text(node)
    })?;
    let mut text = String::new();
    dom.append_normalized_text(caption, &mut text);
    let text = text.trim();
    (!text.is_empty()).then_some(text.to_owned())
}

fn image_variant_identity(dom: &Dom, image: NodeId) -> Option<(String, ImageVariant)> {
    image_urls(dom, image)
        .into_iter()
        .find_map(image_variant_resource)
}

fn image_variant_resource(url: &str) -> Option<(String, ImageVariant)> {
    let resource = image_resource(url);
    let path = resource
        .split(['?', '#'])
        .next()
        .unwrap_or(resource.as_ref());
    let (directory, file) = path.rsplit_once('/').unwrap_or(("", path));
    let stem = file.rsplit_once('.').map_or(file, |(stem, _)| stem);

    let mut variant = None;
    let mut base_tokens = Vec::new();
    for token in stem.split(|character: char| !character.is_ascii_alphanumeric()) {
        if token.is_empty() {
            continue;
        }
        if let Some(candidate) = image_variant_marker(token) {
            variant = Some(candidate);
        } else {
            base_tokens.push(token.to_ascii_lowercase());
        }
    }
    let variant = variant?;
    if base_tokens.is_empty() {
        return None;
    }
    let base = if directory.is_empty() {
        base_tokens.join("-")
    } else {
        format!(
            "{}/{}",
            directory.to_ascii_lowercase(),
            base_tokens.join("-")
        )
    };
    Some((base, variant))
}

fn image_variant_marker(token: &str) -> Option<ImageVariant> {
    let token = token.to_ascii_lowercase();
    for (name, variant) in [
        ("mobile", ImageVariant::Mobile),
        ("desktop", ImageVariant::Desktop),
    ] {
        let Some(suffix) = token.strip_prefix(name) else {
            continue;
        };
        let valid_suffix = suffix.is_empty()
            || suffix.bytes().all(|byte| byte.is_ascii_digit())
            || suffix.strip_prefix('v').is_some_and(|version| {
                !version.is_empty() && version.bytes().all(|byte| byte.is_ascii_digit())
            });
        if valid_suffix {
            return Some(variant);
        }
    }
    None
}

fn has_meaningful_image_description(dom: &Dom, image: NodeId) -> bool {
    if has_direct_meaningful_description(dom, image) || has_nearby_explanatory_text(dom, image) {
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

fn has_nearby_explanatory_text(dom: &Dom, image: NodeId) -> bool {
    let names = image_context_name(dom, image);
    if dom.tag(image) != Some(Tag::Img)
        || !has_usable_image_source(dom, image)
        || has_strong_peripheral_role(dom, image)
        || has_media_control_context(dom, image)
        || contains_role_token(&names, &["badge", "favicon", "logo", "sprite"])
        || contains_role_token(&names, &["icon"])
            && !contains_role_token(
                &names,
                &[
                    "chart",
                    "diagram",
                    "figure",
                    "hero",
                    "illustration",
                    "photo",
                    "screenshot",
                ],
            )
    {
        return false;
    }
    let mut media_child = image;
    for ancestor in dom.ancestors(image).take(4) {
        let children: SmallVec<[NodeId; 6]> = dom.element_children(ancestor).collect();
        if children.len() > 4 {
            media_child = ancestor;
            continue;
        }
        let Some(position) = children.iter().position(|&child| {
            child == media_child || dom.descendants(child).any(|descendant| descendant == image)
        }) else {
            media_child = ancestor;
            continue;
        };
        if position > 0 && is_explanatory_text(dom, children[position - 1])
            || position + 1 < children.len() && is_explanatory_text(dom, children[position + 1])
        {
            return true;
        }
        media_child = ancestor;
    }
    false
}

fn is_explanatory_text(dom: &Dom, node: NodeId) -> bool {
    let mut text = String::new();
    dom.append_normalized_text(node, &mut text);
    let text = text.trim();
    if text.len() < 12 || text.split_whitespace().count() < 3 {
        return false;
    }
    let semantic_tag = matches!(
        dom.tag(node),
        Some(
            Tag::Blockquote
                | Tag::Caption
                | Tag::Figcaption
                | Tag::H1
                | Tag::H2
                | Tag::H3
                | Tag::H4
                | Tag::H5
                | Tag::H6
                | Tag::P
        )
    );
    let named_context = [AttrName::Class, AttrName::Id]
        .into_iter()
        .filter_map(|attribute| dom.attr(node, attribute))
        .flat_map(|value| value.split(|character: char| !character.is_ascii_alphanumeric()))
        .any(|token| {
            matches!(
                token.to_ascii_lowercase().as_str(),
                "caption" | "description" | "feature" | "figure" | "summary" | "text"
            )
        });
    semantic_tag || named_context
}

fn has_figure_caption(dom: &Dom, image: NodeId) -> bool {
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

fn has_direct_meaningful_description(dom: &Dom, image: NodeId) -> bool {
    ["alt", "aria-label", "title"]
        .into_iter()
        .filter_map(|name| dom.attr_by_local_name(image, name))
        .any(|value| {
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
        })
}

fn valid_image_attribute(value: Option<&str>) -> Option<&str> {
    value.filter(|value| {
        let value = value.trim();
        !value.is_empty()
            && !value.eq_ignore_ascii_case("null")
            && !value.eq_ignore_ascii_case("undefined")
    })
}

fn image_has_placeholder_source(dom: &Dom, image: NodeId) -> bool {
    dom.attr(image, AttrName::Src).is_none_or(|source| {
        let source = source.to_ascii_lowercase();
        is_placeholder_resource(&source)
            || source.contains("placeholder")
            || source.contains("blank.gif")
            || source.contains("spacer.gif")
            || source.contains("transparent.gif")
            || crate::constants::parse_b64_data_url(&source)
                .is_some_and(|(end, _)| source.len().saturating_sub(end) < 133)
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

fn same_image_url(first: NodeId, second: NodeId, selected_sources: &[Option<Box<str>>]) -> bool {
    match (
        selected_sources[first.index()].as_deref(),
        selected_sources[second.index()].as_deref(),
    ) {
        (Some(first), Some(second)) => image_resource(first) == image_resource(second),
        _ => false,
    }
}

fn image_urls(dom: &Dom, image: NodeId) -> SmallVec<[&str; 8]> {
    let mut urls = SmallVec::new();
    for attribute in [
        AttrName::Src,
        AttrName::DataSrc,
        AttrName::Srcset,
        AttrName::DataSrcset,
    ] {
        let Some(value) = dom.attr(image, attribute).filter(|value| !value.is_empty()) else {
            continue;
        };
        if matches!(attribute, AttrName::Srcset | AttrName::DataSrcset) {
            urls.extend(
                parse_srcset(value)
                    .into_iter()
                    .map(|candidate| candidate.url),
            );
        } else {
            urls.push(value);
        }
    }
    urls
}

fn image_resource(url: &str) -> Cow<'_, str> {
    next_image_parameter(url, "url").map_or(Cow::Borrowed(url), Cow::Owned)
}

fn next_image_parameter(url: &str, parameter: &str) -> Option<String> {
    let (path, query) = url.split_once('?')?;
    if !path.trim_end_matches('/').ends_with("/_next/image") {
        return None;
    }
    form_urlencoded::parse(query.as_bytes())
        .find_map(|(name, value)| (name == parameter).then(|| value.into_owned()))
}

fn next_image_width(dom: &Dom, image: NodeId) -> Option<(u8, u32)> {
    image_urls(dom, image)
        .into_iter()
        .filter_map(|url| next_image_parameter(url, "w")?.parse::<u32>().ok())
        .max()
        .map(|width| (2, width))
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
    fn semantic_markdown(dom: &Dom, root: NodeId) -> String {
        let document = crate::document::compile_document(
            dom,
            root,
            &crate::document::CompileContext::default(),
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
        deduplicate_selected(&mut dom, root, &mut Vec::new());
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
        assert_eq!(semantic_markdown(&dom, root), "![View](large.webp)\n");
        let source = dom.first_descendant_by_tag(root, Tag::Source).unwrap();
        assert_eq!(dom.attr(source, AttrName::Srcset), None);
        assert_eq!(
            dom.attr(source, AttrName::DataSrcset),
            Some("small.webp 400w, large.webp 1200w")
        );
    }

    #[test]
    fn repetition_identity_prefers_a_real_responsive_source_over_a_placeholder() {
        let dom = Dom::parse_document(
            r#"<main><img src="pixel.gif" srcset="map-small.png 800w, map-large.png 1600w" alt="Map"></main>"#,
        )
        .unwrap();
        let root = dom.body().unwrap();
        let image = dom.first_descendant_by_tag(root, Tag::Img).unwrap();
        assert_eq!(
            primary_image_resource(&dom, image).as_deref(),
            Some("map-large.png")
        );
    }

    #[test]
    fn responsive_normalization_skips_placeholder_candidates() {
        let (dom, root) = normalized(
            r#"<picture><source srcset="pixel.gif 1600w, map.png 800w"><img src="fallback.jpg" alt="Map"></picture>"#,
        );
        assert_eq!(semantic_markdown(&dom, root), "![Map](map.png)\n");
    }

    #[test]
    fn lazy_normalization_replaces_a_pixel_source_with_a_real_data_source() {
        let (dom, root) =
            normalized(r#"<img src="pixel.gif" data-src="map.png" alt="Map of the survey area">"#);
        assert_eq!(
            semantic_markdown(&dom, root),
            "![Map of the survey area](map.png)\n"
        );
    }

    #[test]
    fn lazy_normalization_replaces_an_undefined_source_with_a_real_data_source() {
        let (dom, root) =
            normalized(r#"<img src="undefined" data-src="map.png" alt="Map of the survey area">"#);
        assert_eq!(
            semantic_markdown(&dom, root),
            "![Map of the survey area](map.png)\n"
        );
    }

    #[test]
    fn responsive_normalization_tries_the_next_srcset_attribute() {
        let (dom, root) = normalized(
            r#"<picture><source data-srcset="broken nope" srcset="map.png 800w"><img src="fallback.jpg" alt="Map"></picture>"#,
        );
        assert_eq!(semantic_markdown(&dom, root), "![Map](map.png)\n");
    }

    #[test]
    fn removes_placeholder_candidates_from_active_srcsets() {
        let (dom, root) = normalized(
            r#"<picture><source srcset="pixel.gif 1600w, map.webp 800w"><img src="pixel.gif" srcset="transparent.gif 2x, map.jpg 1x" alt="Survey map"></picture>"#,
        );
        assert_eq!(semantic_markdown(&dom, root), "![Survey map](map.webp)\n");
    }

    #[test]
    fn removes_an_undescribed_image_with_only_placeholder_sources() {
        let (dom, root) =
            normalized(r#"<main><img src="pixel.gif" srcset="transparent.gif 2x"></main>"#);
        assert!(dom.first_descendant_by_tag(root, Tag::Img).is_none());
    }

    #[test]
    fn uses_one_picture_source_without_comparing_media_descriptors() {
        let (dom, root) = normalized(
            r#"<picture><source media="(max-width: 600px)" srcset="mobile.jpg 2000w"><source srcset="default.jpg 1200w"><img src="fallback.jpg" alt="View"></picture>"#,
        );
        assert_eq!(semantic_markdown(&dom, root), "![View](mobile.jpg)\n");
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
        assert!(semantic_markdown(&dom, root).contains("large.jpg"));
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
        let markdown = semantic_markdown(&dom, root);
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
            semantic_markdown(&dom, root),
            "![Detailed diagram](large.jpg)\n"
        );
    }

    #[test]
    fn deduplicates_adjacent_mobile_and_desktop_figures() {
        let (dom, root) = normalized(
            r#"<main><figure><img src="charts/uncertainty-Mobilev1.svg" alt="Fractal uncertainty"></figure><figure><img src="charts/uncertainty-Desktopv1.svg" alt="Fractal uncertainty"></figure></main>"#,
        );
        let images: SmallVec<[NodeId; 2]> = dom
            .descendants(root)
            .filter(|&node| dom.tag(node) == Some(Tag::Img))
            .collect();
        assert_eq!(images.len(), 1);
        assert_eq!(
            dom.attr(images[0], AttrName::Src),
            Some("charts/uncertainty-Desktopv1.svg")
        );
    }

    #[test]
    fn duplicate_figure_media_does_not_remove_unique_siblings() {
        let (dom, root) = normalized(
            r#"<main><figure><img src="charts/uncertainty-Mobilev1.svg" alt="Fractal uncertainty"><video src="walkthrough.mp4"></video><p>The video explains the complete topology.</p></figure><figure><img src="charts/uncertainty-Desktopv1.svg" alt="Fractal uncertainty"></figure></main>"#,
        );
        let images: SmallVec<[NodeId; 2]> = dom
            .descendants(root)
            .filter(|&node| dom.tag(node) == Some(Tag::Img))
            .collect();
        assert_eq!(images.len(), 1);
        assert_eq!(
            dom.attr(images[0], AttrName::Src),
            Some("charts/uncertainty-Desktopv1.svg")
        );
        assert!(dom.first_descendant_by_tag(root, Tag::Video).is_some());
        assert!(semantic_markdown(&dom, root).contains("complete topology"));
    }

    #[test]
    fn keeps_distinct_mobile_and_desktop_figures_without_shared_description() {
        let (dom, root) = normalized(
            r#"<main><figure><img src="charts/latency-Mobile.svg" alt="Latency on mobile"></figure><figure><img src="charts/latency-Desktop.svg" alt="Latency on desktop"></figure></main>"#,
        );
        assert_eq!(
            dom.descendants(root)
                .filter(|&node| dom.tag(node) == Some(Tag::Img))
                .count(),
            2
        );
    }

    #[test]
    fn keeps_same_url_figures_with_distinct_captions() {
        let (dom, root) = normalized(
            r#"<main><figure><img src="shared.jpg" alt="Product view"><figcaption>Front view of the product.</figcaption></figure><figure><img src="shared.jpg" alt="Product view"><figcaption>Rear view of the product.</figcaption></figure></main>"#,
        );
        assert_eq!(
            dom.descendants(root)
                .filter(|&node| dom.tag(node) == Some(Tag::Img))
                .count(),
            2
        );
        assert_eq!(
            dom.descendants(root)
                .filter(|&node| dom.tag(node) == Some(Tag::Figcaption))
                .count(),
            2
        );
    }

    #[test]
    fn deduplicates_nextjs_optimizer_variants_and_keeps_the_largest() {
        let (dom, root) = normalized(
            r#"<p><img src="/_next/image?url=%2Fphoto.jpg&amp;w=32&amp;q=20" alt="Photo"></p><p><img src="/_next/image?url=%2Fphoto.jpg&amp;w=1600&amp;q=85" alt="Photo"></p>"#,
        );
        let images: SmallVec<[NodeId; 2]> = dom
            .descendants(root)
            .filter(|&node| dom.tag(node) == Some(Tag::Img))
            .collect();
        assert_eq!(images.len(), 1);
        assert_eq!(
            dom.attr(images[0], AttrName::Src),
            Some("/_next/image?url=%2Fphoto.jpg&w=1600&q=85")
        );
    }

    #[test]
    fn keeps_distinct_nextjs_resources_with_generic_alt_text() {
        let (dom, root) = normalized(
            r#"<img src="/_next/image?url=%2Fbefore.jpg&amp;w=1200" alt="image"><img src="/_next/image?url=%2Fafter.jpg&amp;w=1200" alt="image">"#,
        );
        assert_eq!(
            dom.descendants(root)
                .filter(|&node| dom.tag(node) == Some(Tag::Img))
                .count(),
            2
        );
    }

    #[test]
    fn nextjs_density_sets_keep_the_largest_optimizer_variant() {
        let (dom, root) = normalized(
            r#"<p><img srcset="/_next/image?url=%2Fphoto.jpg&amp;w=32 1x, /_next/image?url=%2Fphoto.jpg&amp;w=64 2x" alt="Photo"></p><p><img srcset="/_next/image?url=%2Fphoto.jpg&amp;w=800 1x, /_next/image?url=%2Fphoto.jpg&amp;w=1600 2x" alt="Photo"></p>"#,
        );
        let images: SmallVec<[NodeId; 2]> = dom
            .descendants(root)
            .filter(|&node| dom.tag(node) == Some(Tag::Img))
            .collect();
        assert_eq!(images.len(), 1);
        assert_eq!(
            semantic_markdown(&dom, root),
            "![Photo](/_next/image?url=%2Fphoto.jpg&w=1600)\n"
        );
    }

    #[test]
    fn shared_low_resolution_candidates_do_not_merge_distinct_images() {
        for html in [
            r#"<img srcset="shared.jpg 1x, before.jpg 2x" alt="Before"><img srcset="shared.jpg 1x, after.jpg 2x" alt="After">"#,
            r#"<img src="shared.jpg" srcset="before.jpg 2x" alt="Before"><img src="shared.jpg" srcset="after.jpg 2x" alt="After">"#,
        ] {
            let (dom, root) = normalized(html);
            assert_eq!(
                dom.descendants(root)
                    .filter(|&node| dom.tag(node) == Some(Tag::Img))
                    .count(),
                2
            );
        }
    }

    #[test]
    fn retains_the_higher_density_duplicate() {
        for html in [
            r#"<img src="small.jpg" srcset="small.jpg 1x" alt="Detailed diagram"><img src="small.jpg" srcset="small.jpg 1x, large.jpg 2x" alt="Detailed diagram">"#,
            r#"<img src="small.jpg" alt="Detailed diagram"><img src="small.jpg" data-srcset="small.jpg 1x, large.jpg 2x" alt="Detailed diagram">"#,
        ] {
            let (dom, root) = normalized(html);
            assert_eq!(
                dom.descendants(root)
                    .filter(|&node| dom.tag(node) == Some(Tag::Img))
                    .count(),
                1
            );
            assert_eq!(
                semantic_markdown(&dom, root),
                "![Detailed diagram](large.jpg)\n"
            );
        }
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
            semantic_markdown(&dom, root),
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

    #[test]
    fn removes_small_decorative_images_from_static_evidence() {
        let mut dom = Dom::parse_document(
            r#"<main><img src="pixel.gif" style="width: 1px; height: 1px"><img class="author-avatar" src="person.jpg" width="32" height="32"><img src="/cdn/w_24/action-icon.svg"><svg class="action-icon" viewBox="0 0 16 16"><path></path></svg></main>"#,
        )
        .unwrap();
        let root = dom.body().unwrap();
        remove_decorative_media(&mut dom, root);
        assert_eq!(
            dom.descendants(root)
                .filter(|&node| matches!(dom.tag(node), Some(Tag::Img | Tag::Svg)))
                .count(),
            0
        );
    }

    #[test]
    fn nearby_explanatory_text_protects_a_product_screenshot() {
        let mut dom = Dom::parse_document(
            r#"<main><section class="product-screenshot"><img class="icon" src="workspace.png"><p>The workspace screenshot shows the complete review flow.</p></section></main>"#,
        )
        .unwrap();
        let root = dom.body().unwrap();
        remove_decorative_media(&mut dom, root);
        assert!(dom.first_descendant_by_tag(root, Tag::Img).is_some());
    }

    #[test]
    fn nearby_prose_does_not_protect_an_action_icon() {
        let mut dom = Dom::parse_document(
            r#"<main><section><img class="action-icon" src="action.svg"><p>The next section explains the complete review flow.</p></section></main>"#,
        )
        .unwrap();
        let root = dom.body().unwrap();
        remove_decorative_media(&mut dom, root);
        assert!(dom.first_descendant_by_tag(root, Tag::Img).is_none());
    }

    #[test]
    fn nearby_prose_does_not_protect_peripheral_images() {
        let mut dom = Dom::parse_document(
            r#"<main><section class="author-photo"><img src="author.jpg" alt="Portrait of the author"><p>The author explains the complete review flow.</p></section><section class="promo"><img src="promo.jpg" alt="Try the complete review platform"><p>The platform explains the complete review flow.</p></section><section class="advert"><img src="advert.jpg" alt="A complete review platform"><p>The platform explains the complete review flow.</p></section><section class="sponsored"><img src="sponsored.jpg" alt="A complete review platform"><p>The platform explains the complete review flow.</p></section></main>"#,
        )
        .unwrap();
        let root = dom.body().unwrap();
        remove_decorative_media(&mut dom, root);
        assert!(dom.first_descendant_by_tag(root, Tag::Img).is_none());
    }

    #[test]
    fn keeps_small_meaningful_and_responsive_images() {
        let mut dom = Dom::parse_document(
            r#"<main><img src="status-dot.png" width="1" alt="Service is available"><img class="icon" src="status.png" width="16" alt="Service is available"><img class="icon" src="small.jpg" width="16" srcset="small.jpg 16w, diagram.jpg 640w"><picture><source srcset="large.jpg 2x"><img class="icon" src="small.jpg" width="16"></picture><figure><img class="icon" src="diagram.svg?w=24" width="24"><figcaption>Network topology</figcaption></figure><img class="equation" src="equation.png" width="16" alt="x = y"><svg class="status-icon" width="16" height="16"><title>Warning status</title><path></path></svg></main>"#,
        )
        .unwrap();
        let root = dom.body().unwrap();
        remove_decorative_media(&mut dom, root);
        assert_eq!(
            dom.descendants(root)
                .filter(|&node| dom.tag(node) == Some(Tag::Img))
                .count(),
            6
        );
        assert!(
            dom.descendants(root)
                .any(|node| dom.tag(node) == Some(Tag::Svg))
        );
    }

    #[test]
    fn root_semantics_protect_small_images() {
        for html in [
            r#"<figure><img class="icon" width="16" src="small"><figcaption>Result diagram</figcaption></figure>"#,
            r#"<picture><source srcset="/media/asset/123 640w"><img class="icon" width="16" src="small"></picture>"#,
            r#"<div data-legible-math="inline"><img class="icon" width="16" src="small"></div>"#,
        ] {
            let mut dom = Dom::parse_document(html).unwrap();
            let root = dom
                .descendants(dom.root())
                .find(|&node| matches!(dom.tag(node), Some(Tag::Figure | Tag::Picture | Tag::Div)))
                .unwrap();
            remove_decorative_media(&mut dom, root);
            assert!(
                dom.first_descendant_by_tag(root, Tag::Img).is_some(),
                "{html}"
            );
        }
    }

    #[test]
    fn classifies_related_cards_and_repeated_avatars_as_peripheral() {
        let mut dom = Dom::parse_document(
            r#"<main><p>The article explains the complete result.</p><section class="more-stories card-grid"><a href="/next"><figure><img src="next.jpg" width="800" alt="A related report" srcset="next.jpg 800w"><figcaption>A related report</figcaption></figure></a></section><section class="founders"><div class="card"><img src="https://s3.test/opaque/53b72097.jpg?X-Amz-Signature=one" srcset="https://s3.test/opaque/53b72097.jpg?X-Amz-Signature=one 2x" width="800" alt="Pat Example"></div><div class="card"><img src="https://s3.test/opaque/53b72097.jpg?X-Amz-Signature=two" srcset="https://s3.test/opaque/53b72097.jpg?X-Amz-Signature=two 2x" width="800" alt="Pat Example"></div></section></main>"#,
        )
        .unwrap();
        let root = dom.body().unwrap();
        remove_decorative_media(&mut dom, root);
        assert!(dom.first_descendant_by_tag(root, Tag::Img).is_none());
    }

    #[test]
    fn keeps_case_distinct_signed_profile_resources() {
        let mut dom = Dom::parse_document(
            r#"<main><section class="founders"><div class="card"><img src="https://s3.test/opaque/AbC.jpg?X-Amz-Signature=one" width="800" alt="Pat Example"></div><div class="card"><img src="https://s3.test/opaque/abc.jpg?X-Amz-Signature=two" width="800" alt="Sam Example"></div></section></main>"#,
        )
        .unwrap();
        let root = dom.body().unwrap();
        remove_decorative_media(&mut dom, root);
        assert_eq!(
            dom.descendants(root)
                .filter(|&node| dom.tag(node) == Some(Tag::Img))
                .count(),
            2
        );
    }

    #[test]
    fn preserves_meaningful_media_from_a_lightbox_control() {
        let mut dom = Dom::parse_document(
            r#"<main><p>Introduction.</p><button><figure><img src="lead.jpg" alt=""><figcaption>Detailed lead illustration</figcaption></figure></button><p>Article body.</p></main>"#,
        )
        .unwrap();
        let root = dom.body().unwrap();
        let button = dom.first_descendant_by_tag(root, Tag::Button).unwrap();
        remove_decorative_media(&mut dom, root);
        let figure = dom.first_descendant_by_tag(root, Tag::Figure).unwrap();
        assert_eq!(dom.parent(figure), dom.parent(button));
        assert_eq!(dom.next_sibling(figure), Some(button));
    }

    #[test]
    fn preserves_a_responsive_picture_from_a_lightbox_control() {
        let mut dom = Dom::parse_document(
            r#"<main><p>Introduction.</p><button><picture><source srcset="lead-large.webp 1200w"><img src="lead-small.jpg" alt="Detailed lead illustration"></picture></button><p>Article body.</p></main>"#,
        )
        .unwrap();
        let root = dom.body().unwrap();
        let button = dom.first_descendant_by_tag(root, Tag::Button).unwrap();
        remove_decorative_media(&mut dom, root);
        let picture = dom.first_descendant_by_tag(root, Tag::Picture).unwrap();
        assert_eq!(dom.parent(picture), dom.parent(button));
        assert!(dom.first_descendant_by_tag(picture, Tag::Source).is_some());
    }

    #[test]
    fn recognizes_adjacent_figure_and_picture_leads() {
        for html in [
            r#"<main><figure><img src="lead.jpg" srcset="lead.jpg 1200w" alt=""><figcaption>Detailed lead diagram</figcaption></figure><article><p>Article body.</p></article></main>"#,
            r#"<main><picture><source srcset="lead.webp 1200w"><img src="lead.jpg" alt="Detailed lead diagram"></picture><article><p>Article body.</p></article></main>"#,
            r#"<main><figure><img src="lead.jpg" width="1200" height="700" alt="Detailed lead diagram"><figcaption>Detailed lead diagram</figcaption></figure><article><p>Article body.</p></article></main>"#,
        ] {
            let dom = Dom::parse_document(html).unwrap();
            let article = dom
                .first_descendant_by_tag(dom.root(), Tag::Article)
                .unwrap();
            let lead = adjacent_lead_media(&dom, article).expect(html);
            assert!(matches!(dom.tag(lead), Some(Tag::Figure | Tag::Picture)));
        }
        for html in [
            r#"<main><figure class="author"><img src="author.jpg" width="800" alt="Portrait of the author"></figure><article><p>Article body.</p></article></main>"#,
            r#"<main><img class="site-logo" src="logo.svg" width="800" alt="Site logo"><article><p>Article body.</p></article></main>"#,
        ] {
            let dom = Dom::parse_document(html).unwrap();
            let article = dom
                .first_descendant_by_tag(dom.root(), Tag::Article)
                .unwrap();
            assert!(adjacent_lead_media(&dom, article).is_none(), "{html}");
        }
    }
}
