//! Semantic normalization for retained content.

use crate::cleaning::{fix_lazy_images, repeated_listing_start, simplify_nested_elements};
use crate::dom::{AttrName, Dom, NodeId, Tag};
use crate::scoring::{has_single_tag_inside_element, is_element_without_content};
use smallvec::SmallVec;

/// Normalizes retained markup into a predictable tree for all serializers.
pub(crate) fn normalize_semantics(dom: &mut Dom, root: NodeId, nodes: &mut Vec<NodeId>) {
    normalize_images(dom, root, nodes);
    normalize_code_blocks(dom, root);
    normalize_figures(dom, root);
    normalize_footnotes(dom, root);
    normalize_repeated_table_listings(dom, root);
    normalize_layout_tables(dom, root);
}

/// Finishes normalization after URL and attribute cleanup.
pub(crate) fn finish_normalization(dom: &mut Dom, root: NodeId, nodes: &mut Vec<NodeId>) {
    simplify_nested_elements(dom, root, nodes);
    remove_empty_nodes(dom, root, nodes);
}

fn normalize_images(dom: &mut Dom, root: NodeId, nodes: &mut Vec<NodeId>) {
    fix_lazy_images(dom, root, nodes);

    // Markdown needs one concrete image URL. Use the first srcset candidate
    // when an image has no src. Keep the complete srcset for HTML output.
    nodes.clear();
    nodes.extend(
        dom.descendants(root)
            .filter(|&node| dom.tag(node) == Some(Tag::Img)),
    );
    for &image in nodes.iter() {
        let replace_placeholder =
            dom.attr(image, AttrName::Src).is_none() || image_has_placeholder_source(dom, image);
        if replace_placeholder
            && let Some((value, is_srcset)) =
                picture_source(dom, image).map(|(value, is_srcset)| (value.to_owned(), is_srcset))
        {
            if is_srcset {
                dom.set_attr(image, AttrName::Srcset, &value);
                if let Some(src) = first_srcset_candidate(&value) {
                    dom.set_attr(image, AttrName::Src, src);
                }
            } else {
                dom.set_attr(image, AttrName::Src, &value);
            }
        }
        if dom.attr(image, AttrName::Src).is_none()
            && let Some(src) = dom
                .attr(image, AttrName::Srcset)
                .and_then(first_srcset_candidate)
                .map(str::to_owned)
        {
            dom.set_attr(image, AttrName::Src, &src);
        }
    }

    // Remove only adjacent hydration variants. Repeated images separated by
    // text or another element remain distinct content.
    let images: SmallVec<[NodeId; 32]> = nodes.iter().copied().collect();
    for image in images {
        if dom.parent(image).is_none() {
            continue;
        }
        let Some(previous) = previous_element_sibling(dom, image) else {
            continue;
        };
        let Some(previous_image) = single_image(dom, previous) else {
            continue;
        };
        if same_image_url(dom, previous_image, image)
            && (is_hydration_placeholder(dom, previous_image)
                || is_hydration_placeholder(dom, image))
        {
            let remove = if is_hydration_placeholder(dom, previous_image) {
                previous
            } else {
                image
            };
            dom.detach(remove);
        }
    }
}

fn picture_source(dom: &Dom, image: NodeId) -> Option<(&str, bool)> {
    let picture = dom
        .ancestors(image)
        .find(|&ancestor| dom.tag(ancestor) == Some(Tag::Picture))?;
    if let Some(value) = valid_image_attribute(dom.attr(picture, AttrName::DataSrcset)) {
        return Some((value, true));
    }
    if let Some(value) = valid_image_attribute(dom.attr(picture, AttrName::DataSrc)) {
        return Some((value, false));
    }
    for source in dom
        .descendants(picture)
        .filter(|&node| dom.tag(node) == Some(Tag::Source))
    {
        if let Some(value) = valid_image_attribute(
            dom.attr(source, AttrName::DataSrcset)
                .or_else(|| dom.attr(source, AttrName::Srcset)),
        ) {
            return Some((value, true));
        }
        if let Some(value) = valid_image_attribute(
            dom.attr(source, AttrName::DataSrc)
                .or_else(|| dom.attr(source, AttrName::Src)),
        ) {
            return Some((value, false));
        }
    }
    None
}

fn valid_image_attribute(value: Option<&str>) -> Option<&str> {
    value.filter(|value| !value.trim().is_empty() && !value.trim().eq_ignore_ascii_case("null"))
}

fn image_has_placeholder_source(dom: &Dom, image: NodeId) -> bool {
    dom.attr(image, AttrName::Src).is_some_and(|source| {
        let source = source.to_ascii_lowercase();
        source.contains("placeholder")
            || source.contains("blank.gif")
            || source.contains("spacer.gif")
            || source.contains("transparent.gif")
            || crate::constants::parse_b64_data_url(&source)
                .is_some_and(|(end, _)| source.len().saturating_sub(end) < 133)
    })
}

fn first_srcset_candidate(srcset: &str) -> Option<&str> {
    srcset
        .split(',')
        .next()?
        .split_ascii_whitespace()
        .next()
        .filter(|value| !value.is_empty())
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
    [
        AttrName::Src,
        AttrName::Srcset,
        AttrName::DataSrc,
        AttrName::DataSrcset,
    ]
    .into_iter()
    .any(|attribute| {
        dom.attr(first, attribute)
            .filter(|value| !value.is_empty())
            .is_some_and(|value| dom.attr(second, attribute) == Some(value))
    })
}

fn is_hydration_placeholder(dom: &Dom, image: NodeId) -> bool {
    let name = [AttrName::Class, AttrName::Id]
        .into_iter()
        .filter_map(|attribute| dom.attr(image, attribute))
        .any(|value| {
            let value = value.to_ascii_lowercase();
            value.contains("placeholder") || value.contains("lazy") || value.contains("hydration")
        });
    name || dom.attr(image, AttrName::Src).is_none()
        || [AttrName::Width, AttrName::Height]
            .into_iter()
            .filter_map(|attribute| dom.attr(image, attribute)?.parse::<u32>().ok())
            .any(|size| size <= 1)
}

fn normalize_code_blocks(dom: &mut Dom, root: NodeId) {
    let nodes = dom.element_descendants_snapshot_with_depth(root);
    for (node, _) in nodes {
        if dom.parent(node).is_none() {
            continue;
        }
        match dom.tag(node) {
            Some(Tag::Pre) => {
                let code = if has_single_tag_inside_element(dom, node, Tag::Code) {
                    dom.element_children(node).next()
                } else {
                    let Ok(code) = dom.create_html_element(Tag::Code) else {
                        continue;
                    };
                    dom.move_children(node, code);
                    dom.append_child(node, code);
                    Some(code)
                };
                if let Some(code) = code
                    && let Some(language) =
                        language_hint(dom, code).or_else(|| language_hint(dom, node))
                {
                    dom.set_attr(code, AttrName::DataLanguage, &language);
                }
            }
            Some(Tag::Code) if dom.tag(dom.parent(node).unwrap_or(root)) != Some(Tag::Pre) => {
                let block = dom
                    .text_node(dom.first_child(node).unwrap_or(node))
                    .is_some_and(|text| text.contains('\n'))
                    || language_hint(dom, node).is_some();
                if block {
                    let language = language_hint(dom, node);
                    let Ok(pre) = dom.create_html_element(Tag::Pre) else {
                        continue;
                    };
                    dom.insert_before(node, pre);
                    dom.append_child(pre, node);
                    if let Some(language) = language {
                        dom.set_attr(node, AttrName::DataLanguage, &language);
                    }
                }
            }
            _ => {}
        }
    }

    // Common syntax highlighters put one pre block inside a decorative div.
    let wrappers = dom.element_descendants_snapshot_with_depth(root);
    for (wrapper, _) in wrappers.into_iter().rev() {
        if dom.parent(wrapper).is_none() || dom.tag(wrapper) != Some(Tag::Div) {
            continue;
        }
        let named_as_code = [AttrName::Class, AttrName::Id]
            .into_iter()
            .filter_map(|attribute| dom.attr(wrapper, attribute))
            .any(|value| {
                value.split_whitespace().any(|token| {
                    let token = token.to_ascii_lowercase();
                    token == "highlight"
                        || token == "codehilite"
                        || token == "sourcecode"
                        || token.starts_with("highlight-")
                })
            });
        if named_as_code
            && has_single_tag_inside_element(dom, wrapper, Tag::Pre)
            && let Some(pre) = dom.element_children(wrapper).next()
        {
            dom.replace_with(wrapper, pre);
        }
    }
}

fn language_hint(dom: &Dom, node: NodeId) -> Option<String> {
    if let Some(value) = dom
        .attr(node, AttrName::DataLanguage)
        .or_else(|| dom.attr(node, AttrName::Lang))
        .or_else(|| dom.attr_by_local_name(node, "data-lang"))
        && let Some(language) = valid_language(value)
    {
        return Some(language.to_owned());
    }
    for token in dom.attr(node, AttrName::Class)?.split_whitespace() {
        let Some(value) = token
            .strip_prefix("language-")
            .or_else(|| token.strip_prefix("lang-"))
        else {
            continue;
        };
        if let Some(language) = valid_language(value) {
            return Some(language.to_owned());
        }
    }
    None
}

fn valid_language(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()
        && value.len() <= 32
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'-' | b'_' | b'.')))
    .then_some(value)
}

fn normalize_figures(dom: &mut Dom, root: NodeId) {
    let nodes = dom.element_descendants_snapshot_with_depth(root);
    for (wrapper, _) in nodes.into_iter().rev() {
        if dom.parent(wrapper).is_none()
            || !matches!(dom.tag(wrapper), Some(Tag::Div | Tag::P | Tag::Section))
        {
            continue;
        }
        let named_as_figure = [AttrName::Class, AttrName::Id]
            .into_iter()
            .filter_map(|attribute| dom.attr(wrapper, attribute))
            .any(|value| {
                value.split_whitespace().any(|token| {
                    matches!(
                        token.to_ascii_lowercase().as_str(),
                        "figure" | "image-with-caption" | "media-with-caption"
                    )
                })
            });
        if !named_as_figure
            || dom
                .descendants(wrapper)
                .any(|node| dom.tag(node) == Some(Tag::Figure))
        {
            continue;
        }
        let images: SmallVec<[NodeId; 2]> = dom
            .descendants(wrapper)
            .filter(|&node| dom.tag(node) == Some(Tag::Img))
            .collect();
        if images.len() != 1 {
            continue;
        }
        let caption = dom.descendants(wrapper).find(|&node| {
            dom.tag(node) == Some(Tag::Figcaption)
                || [AttrName::Class, AttrName::Id]
                    .into_iter()
                    .filter_map(|attribute| dom.attr(node, attribute))
                    .any(|value| {
                        value.split_whitespace().any(|token| {
                            matches!(
                                token.to_ascii_lowercase().as_str(),
                                "caption" | "figcaption" | "image-caption"
                            )
                        })
                    })
        });
        if let Some(caption) = caption {
            dom.rename_html(wrapper, Tag::Figure);
            dom.rename_html(caption, Tag::Figcaption);
        }
    }
}

fn normalize_footnotes(dom: &mut Dom, root: NodeId) {
    let nodes = dom.element_descendants_snapshot_with_depth(root);
    for (node, _) in nodes {
        if dom.parent(node).is_some()
            && dom.attr(node, AttrName::Role).is_some_and(|role| {
                role.split_whitespace()
                    .any(|value| value.eq_ignore_ascii_case("doc-footnote"))
            })
            && matches!(dom.tag(node), Some(Tag::Div | Tag::Aside))
        {
            dom.rename_html(node, Tag::Section);
        }
    }
}

fn normalize_repeated_table_listings(dom: &mut Dom, root: NodeId) {
    let tables: SmallVec<[(NodeId, u32); 8]> = dom
        .descendants(root)
        .filter_map(|node| repeated_listing_start(dom, node).map(|start| (node, start)))
        .collect();
    for (table, start) in tables.into_iter().rev() {
        if dom.parent(table).is_none() {
            continue;
        }
        let rows: SmallVec<[NodeId; 32]> = dom
            .descendants(table)
            .filter(|&node| {
                dom.tag(node) == Some(Tag::Tr)
                    && dom
                        .ancestors(node)
                        .find(|&ancestor| dom.tag(ancestor) == Some(Tag::Table))
                        == Some(table)
            })
            .collect();
        let Ok(container) = dom.create_html_element(Tag::Div) else {
            continue;
        };
        let Ok(list) = dom.create_html_element(Tag::Ol) else {
            continue;
        };
        if start != 1 {
            dom.set_attr(list, AttrName::Start, &start.to_string());
        }
        dom.append_child(container, list);
        let mut current_item = None;
        let mut expects_metadata = false;
        let mut buffer = String::new();

        for row in rows {
            remove_listing_controls(dom, row, &mut buffer);
            let cells: SmallVec<[NodeId; 8]> = dom
                .element_children(row)
                .filter(|&cell| matches!(dom.tag(cell), Some(Tag::Td | Tag::Th)))
                .collect();
            let rank = cells.first().is_some_and(|&cell| {
                let text = crate::scoring::get_normalized_inner_text(dom, cell, &mut buffer);
                let digits = text.trim().strip_suffix('.').unwrap_or(text.trim());
                !digits.is_empty()
                    && digits.len() <= 6
                    && digits.bytes().all(|byte| byte.is_ascii_digit())
            });
            if rank {
                let Ok(item) = dom.create_html_element(Tag::Li) else {
                    continue;
                };
                dom.append_child(list, item);
                move_meaningful_cells(dom, &cells[1..], item);
                current_item = Some(item);
                expects_metadata = true;
                continue;
            }
            if !dom.has_non_whitespace_text(row) {
                continue;
            }
            if expects_metadata && let Some(item) = current_item {
                if let Ok(line_break) = dom.create_html_element(Tag::Br) {
                    dom.append_child(item, line_break);
                }
                if let Ok(metadata) = dom.create_html_element(Tag::Small) {
                    dom.append_child(item, metadata);
                    move_meaningful_cells(dom, &cells, metadata);
                }
                expects_metadata = false;
            } else if let Ok(paragraph) = dom.create_html_element(Tag::P) {
                dom.append_child(container, paragraph);
                move_meaningful_cells(dom, &cells, paragraph);
            }
        }
        dom.replace_with(table, container);
    }
}

fn move_meaningful_cells(dom: &mut Dom, cells: &[NodeId], destination: NodeId) {
    let mut inserted = false;
    for &cell in cells {
        let meaningful = dom.has_non_whitespace_text(cell)
            || dom
                .descendants(cell)
                .any(|node| matches!(dom.tag(node), Some(Tag::Img | Tag::Picture)));
        if !meaningful {
            continue;
        }
        if inserted && let Ok(space) = dom.create_text(" ") {
            dom.append_child(destination, space);
        }
        dom.move_children(cell, destination);
        inserted = true;
    }
}

fn remove_listing_controls(dom: &mut Dom, row: NodeId, buffer: &mut String) {
    let anchors: SmallVec<[NodeId; 8]> = dom
        .descendants(row)
        .filter(|&node| dom.tag(node) == Some(Tag::A))
        .collect();
    for anchor in anchors {
        let text = crate::scoring::get_normalized_inner_text(dom, anchor, buffer)
            .trim()
            .to_ascii_lowercase();
        let empty = text.is_empty();
        let action_label = matches!(
            text.as_str(),
            "hide" | "vote" | "delete" | "share" | "login" | "sign in" | "subscribe"
        );
        let action_url = dom.attr(anchor, AttrName::Href).is_some_and(|href| {
            let href = href.to_ascii_lowercase();
            href.contains("action=")
                || href.contains("how=")
                || href.starts_with("vote?")
                || href.starts_with("hide?")
                || href.starts_with("delete?")
                || href.contains("/vote?")
                || href.contains("/hide?")
                || href.contains("/delete?")
        });
        let has_media = dom.descendants(anchor).any(|node| {
            matches!(
                dom.tag(node),
                Some(Tag::Img | Tag::Picture | Tag::Audio | Tag::Video)
            )
        });
        if empty && action_url && !has_media || action_label && action_url {
            if action_label && action_url {
                let previous = dom.prev_sibling(anchor).and_then(|node| {
                    let text = dom.text_node(node)?;
                    let trimmed = text.trim_end();
                    let retained = trimmed.trim_end_matches(is_control_separator_character);
                    (retained.len() != trimmed.len())
                        .then(|| (node, retained.trim_end().to_owned()))
                });
                let next = dom.next_sibling(anchor).and_then(|node| {
                    let text = dom.text_node(node)?;
                    let trimmed = text.trim_start();
                    let retained = trimmed.trim_start_matches(is_control_separator_character);
                    (retained.len() != trimmed.len())
                        .then(|| (node, retained.trim_start().to_owned()))
                });
                if previous.is_some() || next.is_some() {
                    if let Some((node, retained)) = previous {
                        if retained.is_empty() {
                            dom.detach(node);
                        } else {
                            dom.set_text(node, &retained);
                        }
                    }
                    if let Some((node, retained)) = next {
                        if retained.is_empty() {
                            dom.detach(node);
                        } else {
                            dom.set_text(node, &retained);
                        }
                    }
                    if let Ok(space) = dom.create_text(" ") {
                        dom.insert_before(anchor, space);
                    }
                }
            }
            dom.detach(anchor);
        }
    }
}

fn is_control_separator_character(character: char) -> bool {
    matches!(character, '|' | '·' | '-' | '–' | '—' | '•')
}

fn normalize_layout_tables(dom: &mut Dom, root: NodeId) {
    let tables: SmallVec<[NodeId; 16]> = dom
        .descendants(root)
        .filter(|&node| dom.tag(node) == Some(Tag::Table))
        .collect();
    for table in tables {
        if dom.parent(table).is_none() {
            continue;
        }
        let body = if has_single_tag_inside_element(dom, table, Tag::Tbody) {
            dom.element_children(table).next()
        } else {
            Some(table)
        };
        let Some(body) = body else { continue };
        if !has_single_tag_inside_element(dom, body, Tag::Tr) {
            continue;
        }
        let Some(row) = dom.element_children(body).next() else {
            continue;
        };
        if !has_single_tag_inside_element(dom, row, Tag::Td) {
            continue;
        }
        let Some(cell) = dom.element_children(row).next() else {
            continue;
        };
        let phrasing = dom
            .children(cell)
            .all(|node| crate::scoring::is_phrasing_content(dom, node));
        dom.rename_html(cell, if phrasing { Tag::P } else { Tag::Div });
        dom.replace_with(table, cell);
    }
}

fn remove_empty_nodes(dom: &mut Dom, root: NodeId, nodes: &mut Vec<NodeId>) {
    nodes.clear();
    nodes.extend(dom.descendants(root));
    for &node in nodes.iter().rev() {
        if dom.parent(node).is_some()
            && matches!(
                dom.tag(node),
                Some(
                    Tag::Div
                        | Tag::Section
                        | Tag::P
                        | Tag::Span
                        | Tag::Aside
                        | Tag::Footer
                        | Tag::Header
                )
            )
            && is_element_without_content(dom, node)
        {
            dom.detach(node);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown::dom_to_markdown;

    fn normalized(html: &str) -> (Dom, NodeId) {
        let mut dom = Dom::parse_fragment(html, Tag::Div).unwrap();
        let root = dom.root();
        let mut nodes = Vec::new();
        normalize_semantics(&mut dom, root, &mut nodes);
        finish_normalization(&mut dom, root, &mut nodes);
        (dom, root)
    }

    #[test]
    fn normalizes_highlighted_code_and_language() {
        let (dom, root) = normalized(
            r#"<div class="highlight"><pre class="highlight language-rust"><span>fn main() {</span>
<span>}</span></pre></div>"#,
        );
        assert_eq!(
            dom_to_markdown(&dom, root, 0),
            "```rust\nfn main() {\n}\n```\n"
        );
    }

    #[test]
    fn does_not_create_nested_figures() {
        let (dom, root) = normalized(
            r#"<div class="image-with-caption"><figure><img src="plot.png"><figcaption>Plot</figcaption></figure></div>"#,
        );
        assert_eq!(
            dom.descendants(root)
                .filter(|&node| dom.tag(node) == Some(Tag::Figure))
                .count(),
            1
        );
    }

    #[test]
    fn normalizes_captioned_image_wrapper() {
        let (dom, root) = normalized(
            r#"<div class="image-with-caption"><img src="plot.png" alt="Plot"><p class="caption">Result plot</p></div>"#,
        );
        assert!(
            dom.descendants(root)
                .any(|node| dom.tag(node) == Some(Tag::Figure))
        );
        assert!(
            dom.descendants(root)
                .any(|node| dom.tag(node) == Some(Tag::Figcaption))
        );
        assert_eq!(
            dom_to_markdown(&dom, root, 0),
            "![Plot](plot.png)\n\nResult plot\n"
        );
    }

    #[test]
    fn uses_srcset_when_src_is_missing() {
        let (dom, root) = normalized(r#"<img srcset="small.jpg 1x, large.jpg 2x" alt="Photo">"#);
        assert_eq!(dom_to_markdown(&dom, root, 0), "![Photo](small.jpg)\n");
    }

    #[test]
    fn uses_a_lazy_picture_source_for_markdown() {
        let (dom, root) = normalized(
            r#"<picture><source><source data-srcset="hero.webp 1x, hero-large.webp 2x"><img src="blank.gif" alt="Hero"></picture>"#,
        );
        assert_eq!(dom_to_markdown(&dom, root, 0), "![Hero](hero.webp)\n");

        let (dom, root) = normalized(
            r#"<picture><source data-src="null"><source data-src="hero.jpg"><img src="placeholder.gif" alt="Hero"></picture>"#,
        );
        assert_eq!(dom_to_markdown(&dom, root, 0), "![Hero](hero.jpg)\n");

        let (dom, root) = normalized(
            r#"<picture data-src="parent.jpg"><img width="1" src="blank.gif" alt="Parent"></picture>"#,
        );
        assert_eq!(dom_to_markdown(&dom, root, 0), "![Parent](parent.jpg)\n");
    }

    #[test]
    fn converts_ranked_listing_tables_but_keeps_data_tables() {
        let (dom, root) = normalized(
            r#"<table><tr><td>31.</td><td><a href='vote?how=up'></a></td><td><a href='/one'>First result</a> <a href='/one'><img src='one.jpg' alt='Preview'></a></td></tr><tr><td></td><td></td><td>10 points | <a href='hide?id=1'>hide</a> | <a href='/one/comments'>2 comments</a></td></tr><tr><td colspan='3'></td></tr><tr><td>32.</td><td><a href='vote?how=up'></a></td><td><a href='/two'>Second result</a></td></tr><tr><td></td><td></td><td>20 points | <a href='hide?id=2'>hide</a></td></tr><tr><td colspan='3'></td></tr><tr><td>33.</td><td></td><td><a href='/three'>Third result</a></td></tr><tr><td></td><td></td><td>30 points</td></tr><tr><td colspan='3'></td></tr></table><table><thead><tr><th>Name</th><th>Value</th></tr></thead><tbody><tr><td>A</td><td>1</td></tr></tbody></table><table><tr><td>1.</td><td><a href='/team/a'>Team A</a></td><td>30</td></tr><tr><td>2.</td><td><a href='/team/b'>Team B</a></td><td>28</td></tr><tr><td>3.</td><td><a href='/team/c'>Team C</a></td><td>25</td></tr><tr><td>4.</td><td><a href='/team/d'>Team D</a></td><td>22</td></tr><tr><td>5.</td><td><a href='/team/e'>Team E</a></td><td>20</td></tr><tr><td>6.</td><td><a href='/team/f'>Team F</a></td><td>18</td></tr></table>"#,
        );
        assert_eq!(
            dom.descendants(root)
                .filter(|&node| dom.tag(node) == Some(Tag::Ol))
                .count(),
            1
        );
        assert_eq!(
            dom.descendants(root)
                .filter(|&node| dom.tag(node) == Some(Tag::Li))
                .count(),
            3
        );
        assert_eq!(
            dom.descendants(root)
                .filter(|&node| dom.tag(node) == Some(Tag::Table))
                .count(),
            2
        );
        assert_eq!(
            dom.descendants(root)
                .filter(|&node| dom.tag(node) == Some(Tag::Img))
                .count(),
            1
        );
        let list = dom
            .descendants(root)
            .find(|&node| dom.tag(node) == Some(Tag::Ol))
            .unwrap();
        assert_eq!(dom.attr(list, AttrName::Start), Some("31"));
        let markdown = dom_to_markdown(&dom, root, 0);
        assert!(markdown.starts_with("31. "));
        assert!(!markdown.contains("hide"));
        assert!(!markdown.contains(" |  | "));
        assert!(markdown.find("First result").unwrap() < markdown.find("Third result").unwrap());
    }

    #[test]
    fn preserves_heading_levels_and_footnotes() {
        let (dom, root) = normalized(
            r##"<h1>Guide</h1><p>Text<a href="#note">[1]</a></p><aside id="note" role="doc-footnote">A reference.</aside>"##,
        );
        assert_eq!(
            dom_to_markdown(&dom, root, 0),
            "# Guide\n\nText[\\[1\\]](#note)\n\nA reference.\n"
        );
    }
}
