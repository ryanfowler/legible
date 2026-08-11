//! Semantic normalization for retained content.

mod callouts;
mod footnotes;
mod headings;
mod images;
mod lists;
mod math;
mod media;

use crate::cleaning::{repeated_listing_start, simplify_nested_elements};
use crate::dom::{AttrName, Dom, NodeId, Tag};
use crate::scoring::{has_single_tag_inside_element, is_element_without_content};
use smallvec::SmallVec;

/// Normalizes retained markup into a predictable tree for all serializers.
///
/// The order preserves information that later, more general passes can hide.
/// Images resolve lazy and responsive sources before figure processing. Heading
/// and list roles become native HTML before wrapper cleanup. Code and figure
/// detection run while source classes are still available. Table normalization
/// runs last because it can replace complete structural subtrees.
pub(crate) fn normalize_semantics(dom: &mut Dom, root: NodeId, nodes: &mut Vec<NodeId>) {
    math::normalize(dom, root);
    media::normalize(dom, root);
    callouts::normalize(dom, root);
    images::normalize(dom, root, nodes);
    headings::normalize(dom, root);
    lists::normalize(dom, root);
    normalize_code_blocks(dom, root);
    normalize_figures(dom, root);
    footnotes::normalize(dom, root);
    normalize_repeated_table_listings(dom, root);
    normalize_layout_tables(dom, root);
}

/// Captures semantic source data that hard cleanup would otherwise remove.
pub(crate) fn preserve_semantics_before_cleanup(dom: &mut Dom, root: NodeId) {
    math::normalize(dom, root);
    media::normalize(dom, root);
    callouts::normalize(dom, root);
    footnotes::normalize(dom, root);
}

/// Removes decorative media while source sizing and naming evidence is intact.
pub(crate) fn remove_decorative_media_before_cleanup(dom: &mut Dom, root: NodeId) {
    images::remove_decorative_media(dom, root);
}

pub(crate) fn adopt_external_footnotes(
    definitions: &footnotes::Definitions,
    fragment: &mut Dom,
    fragment_root: NodeId,
) {
    footnotes::adopt_external(definitions, fragment, fragment_root);
}

pub(crate) fn collect_external_footnotes(dom: &Dom) -> footnotes::Definitions {
    footnotes::collect_external(dom)
}

/// Preserves explicit ARIA document structure in the scoring-only DOM.
///
/// Readability preparation can turn leaf `div` elements into paragraphs. Run
/// the same heading and list passes first so that operation cannot erase
/// author-provided semantics. The retained fragment runs the full pipeline.
pub(crate) fn normalize_scoring_structure(dom: &mut Dom, root: NodeId) {
    headings::normalize_roles(dom, root);
    lists::normalize(dom, root);
}

pub(crate) fn has_primary_heading_semantics(dom: &Dom, node: NodeId) -> bool {
    matches!(dom.tag(node), Some(Tag::H1 | Tag::H2)) || headings::has_primary_role(dom, node)
}

pub(crate) use math::is_accessible_math;

/// Finishes normalization after URL and attribute cleanup.
///
/// These passes are intentionally last. They discard empty presentation
/// wrappers after all semantic passes have consumed their source evidence.
pub(crate) fn finish_normalization(dom: &mut Dom, root: NodeId, nodes: &mut Vec<NodeId>) {
    simplify_nested_elements(dom, root, nodes);
    remove_empty_nodes(dom, root, nodes);
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
                    && let Some(language) = block_language_hint(dom, code, node)
                {
                    dom.set_attr(code, AttrName::DataLanguage, &language);
                }
            }
            Some(Tag::Code) if dom.tag(dom.parent(node).unwrap_or(root)) != Some(Tag::Pre) => {
                let parent = dom.parent(node).unwrap_or(root);
                let phrasing_parent = matches!(
                    dom.tag(parent),
                    Some(
                        Tag::P
                            | Tag::H1
                            | Tag::H2
                            | Tag::H3
                            | Tag::H4
                            | Tag::H5
                            | Tag::H6
                            | Tag::A
                            | Tag::Td
                            | Tag::Th
                    )
                );
                let block = !phrasing_parent
                    && std::iter::once(node)
                        .chain(dom.descendants(node))
                        .any(|descendant| {
                            dom.text_node(descendant)
                                .is_some_and(|text| text.contains('\n'))
                        });
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

fn block_language_hint(dom: &Dom, code: NodeId, pre: NodeId) -> Option<String> {
    language_hint(dom, code)
        .or_else(|| language_hint(dom, pre))
        .or_else(|| {
            // Syntax highlighters commonly store the language on one of two
            // decorative wrappers around the pre element. Keep this lookup
            // bounded and require code-wrapper structure.
            let parent = dom.parent(pre)?;
            is_code_wrapper(dom, parent).then(|| language_hint(dom, parent))?
        })
        .or_else(|| {
            let parent = dom.parent(pre)?;
            let grandparent = dom.parent(parent)?;
            (is_code_wrapper(dom, parent) && is_code_wrapper(dom, grandparent))
                .then(|| language_hint(dom, grandparent))?
        })
}

fn is_code_wrapper(dom: &Dom, node: NodeId) -> bool {
    if dom.tag(node) != Some(Tag::Div) {
        return false;
    }
    language_hint(dom, node).is_some()
        || [AttrName::Class, AttrName::Id]
            .into_iter()
            .filter_map(|attribute| dom.attr(node, attribute))
            .flat_map(str::split_whitespace)
            .any(|token| {
                matches!(
                    token.to_ascii_lowercase().as_str(),
                    "highlight" | "codehilite" | "sourcecode"
                )
            })
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

fn has_visible_heading_content(dom: &Dom, heading: NodeId) -> bool {
    std::iter::once(heading)
        .chain(dom.descendants(heading))
        .any(|node| {
            dom.text_node(node)
                .is_some_and(crate::markdown::has_visible_inline_text)
                || dom.tag(node) == Some(Tag::Img)
                    && (dom
                        .attr_by_local_name(node, "alt")
                        .is_some_and(crate::markdown::has_visible_inline_text)
                        || dom
                            .attr(node, AttrName::Src)
                            .is_some_and(|source| !source.trim().is_empty()))
        })
}

fn remove_empty_nodes(dom: &mut Dom, root: NodeId, nodes: &mut Vec<NodeId>) {
    nodes.clear();
    nodes.extend(dom.descendants(root));
    for &node in nodes.iter().rev() {
        if dom.parent(node).is_some()
            && dom.attr(node, AttrName::DataMath).is_none()
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
                        | Tag::H1
                        | Tag::H2
                        | Tag::H3
                        | Tag::H4
                        | Tag::H5
                        | Tag::H6
                )
            )
            && (is_element_without_content(dom, node)
                || matches!(
                    dom.tag(node),
                    Some(Tag::H1 | Tag::H2 | Tag::H3 | Tag::H4 | Tag::H5 | Tag::H6)
                ) && !has_visible_heading_content(dom, node))
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
    fn flattens_nested_identical_formatting() {
        let (dom, root) = normalized(
            "<p><strong>Listen to this <strong>post</strong>:</strong> <em>one <i>two</i></em></p>",
        );
        assert_eq!(
            dom_to_markdown(&dom, root, 0),
            "**Listen to this post:** *one two*\n"
        );
    }

    #[test]
    fn removes_orphan_placeholders_and_keeps_described_images() {
        let (dom, root) = normalized(
            r#"<img src="grey-placeholder.png"><img src="blank.gif" aria-label="image unavailable"><img src="placeholder.png" alt="A meaningful diagram"><img src="real.jpg" alt="Photo">"#,
        );
        assert_eq!(
            dom_to_markdown(&dom, root, 0),
            "![A meaningful diagram](placeholder.png)![Photo](real.jpg)\n"
        );
    }

    #[test]
    fn figure_caption_does_not_preserve_an_extra_placeholder() {
        let (dom, root) = normalized(
            r#"<figure><img src="blank.gif"><img src="photo.jpg" alt="Photo"><figcaption>A useful caption</figcaption></figure>"#,
        );
        assert_eq!(
            dom.descendants(root)
                .filter(|&node| dom.tag(node) == Some(Tag::Img))
                .count(),
            1
        );
    }

    #[test]
    fn placeholder_tokens_outside_known_basenames_do_not_remove_images() {
        let (dom, root) = normalized(
            r#"<img src="/photos/placeholder-design.jpg"><img src="/image.jpg?placeholder=true"><img src="https://placeholder.example/image.jpg">"#,
        );
        assert_eq!(
            dom.descendants(root)
                .filter(|&node| dom.tag(node) == Some(Tag::Img))
                .count(),
            3
        );
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
    fn keeps_language_classes_on_inline_code_inline() {
        let (dom, root) = normalized(
            r#"<p>Use <code class="language-plaintext"><span>cargo test</span></code> now.</p><table><tr><th>Call</th></tr><tr><td><code class="language-rust">run()</code></td></tr></table>"#,
        );
        let markdown = dom_to_markdown(&dom, root, 0);
        assert!(markdown.contains("Use `cargo test` now."));
        assert!(markdown.contains("Call\n`run()`"), "{markdown}");
        assert!(!markdown.contains("```plaintext"));
    }

    #[test]
    fn promotes_multiline_orphan_code_and_finds_wrapper_language() {
        let (dom, root) = normalized(
            r#"<code><span>first
second</span></code><div class="language-rust"><div class="highlight"><pre><code><span>fn main() {}</span></code></pre></div></div>"#,
        );
        let markdown = dom_to_markdown(&dom, root, 0);
        assert!(markdown.contains("```\nfirst\nsecond\n```"));
        assert!(markdown.contains("```rust\nfn main() {}\n```"));
    }

    #[test]
    fn removes_headings_without_visible_content() {
        let (dom, root) = normalized(
            "<h2> </h2><h2>&nbsp;<br></h2><h2>\u{200b}\u{2060}\u{feff}</h2><h2>Visible\u{200b}</h2><h2><img src='x' alt='Diagram'></h2><p>Text.</p>",
        );
        let markdown = dom_to_markdown(&dom, root, 0);
        assert_eq!(
            markdown,
            "## Visible\u{200b}\n\n## ![Diagram](x)\n\nText.\n"
        );
    }

    #[test]
    fn keeps_an_image_only_heading_without_alt_text() {
        let (dom, root) = normalized(r#"<h2><img src="diagram.png"></h2><p>Text.</p>"#);
        assert!(
            dom.descendants(root)
                .any(|node| dom.tag(node) == Some(Tag::H2))
        );
        assert_eq!(
            dom_to_markdown(&dom, root, 0),
            "![](diagram.png)\n\nText.\n"
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
        assert_eq!(dom_to_markdown(&dom, root, 0), "![Photo](large.jpg)\n");
    }

    #[test]
    fn uses_a_lazy_picture_source_for_markdown() {
        let (dom, root) = normalized(
            r#"<picture><source><source data-srcset="hero.webp 1x, hero-large.webp 2x"><img src="blank.gif" alt="Hero"></picture>"#,
        );
        assert_eq!(dom_to_markdown(&dom, root, 0), "![Hero](hero-large.webp)\n");

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
            "# Guide\n\nText[^note]\n\n[^note]: A reference.\n"
        );
    }
}
