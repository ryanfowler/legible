//! DOM preparation and content cleanup.
#![allow(clippy::collapsible_if)]
use crate::constants::{
    PRESENTATIONAL_ATTRIBUTES, has_image_extension, has_image_src, has_image_srcset,
    is_deprecated_size_attribute_elem, parse_b64_data_url,
};
use crate::dom::{AttrName, Dom, NodeId, Tag};
use crate::scoring::{
    get_inner_text, get_link_density_cached, get_or_compute_stats, has_single_tag_inside_element,
    is_element_without_content, is_phrasing_content,
};
use html5ever::{LocalName, QualName, ns};
use regex::Regex;
use smallvec::SmallVec;

pub fn prep_document(dom: &mut Dom) {
    // Preserve the required preparation order. Remove inactive subtrees,
    // normalize BR runs, and only then rename deprecated font elements.
    let mut ids: Vec<_> = dom
        .descendants(dom.root())
        .filter(|&id| matches!(dom.tag(id), Some(Tag::Script | Tag::Noscript | Tag::Style)))
        .collect();
    for &id in &ids {
        dom.detach(id);
    }

    ids.clear();
    if let Some(body) = dom.body() {
        ids.extend(
            dom.descendants(body)
                .filter(|&id| dom.tag(id) == Some(Tag::Br)),
        );
    }
    replace_brs(dom, &ids);

    ids.clear();
    ids.extend(
        dom.descendants(dom.root())
            .filter(|&id| dom.tag(id) == Some(Tag::Font)),
    );
    for id in ids {
        dom.rename_html(id, Tag::Span);
    }
}

pub(crate) fn next_non_whitespace_sibling(dom: &Dom, id: NodeId) -> Option<NodeId> {
    let mut n = dom.next_sibling(id);
    while let Some(x) = n {
        if dom.is_element(x) {
            return Some(x);
        }
        if dom.is_text(x) && dom.text_node(x).is_some_and(|t| !t.trim().is_empty()) {
            return None;
        }
        n = dom.next_sibling(x);
    }
    None
}
fn replace_brs(dom: &mut Dom, ids: &[NodeId]) {
    for &br in ids {
        if dom.tag(br) != Some(Tag::Br) {
            continue;
        }
        if dom.parent(br).is_none() {
            continue;
        }
        let mut next = next_non_whitespace_sibling(dom, br);
        let mut replaced = false;
        while let Some(x) = next {
            if dom.tag(x) == Some(Tag::Br) {
                replaced = true;
                next = next_non_whitespace_sibling(dom, x);
                dom.detach(x);
            } else {
                break;
            }
        }
        if !replaced {
            continue;
        }
        dom.rename_html(br, Tag::P);
        let mut n = dom.next_sibling(br);
        while let Some(x) = n {
            if dom.tag(x) == Some(Tag::Br)
                && next_non_whitespace_sibling(dom, x).is_some_and(|y| dom.tag(y) == Some(Tag::Br))
            {
                break;
            }
            if !is_phrasing_content(dom, x) {
                break;
            }
            n = dom.next_sibling(x);
            dom.append_child(br, x);
        }
        while let Some(x) = dom.last_child(br) {
            if crate::scoring::is_whitespace(dom, x) {
                dom.detach(x);
            } else {
                break;
            }
        }
        if let Some(p) = dom.parent(br)
            && dom.tag(p) == Some(Tag::P)
        {
            dom.rename_html(p, Tag::Div);
        }
    }
}
fn has_allowed_media(dom: &Dom, id: NodeId, allowed: &Regex) -> bool {
    if dom
        .attrs(id)
        .iter()
        .any(|attr| allowed.is_match(attr.value.as_ref()))
    {
        return true;
    }
    dom.tag(id) == Some(Tag::Object)
        && dom.descendants(id).any(|node| {
            dom.attrs(node)
                .iter()
                .any(|attr| allowed.is_match(attr.value.as_ref()))
                || dom
                    .text_node(node)
                    .is_some_and(|text| allowed.is_match(text))
        })
}

pub fn clean_styles(dom: &mut Dom, root: NodeId, nodes: &mut Vec<NodeId>) {
    nodes.clear();
    nodes.extend(std::iter::once(root).chain(dom.descendants(root)));
    for &id in nodes.iter() {
        if !dom.is_element(id) || dom.tag(id) == Some(Tag::Svg) {
            continue;
        }
        let has_size = dom.has_attr(id, AttrName::Width) || dom.has_attr(id, AttrName::Height);
        dom.remove_attrs(id, PRESENTATIONAL_ATTRIBUTES);
        if has_size && dom.tag(id).is_some_and(is_deprecated_size_attribute_elem) {
            dom.remove_attrs(id, &[AttrName::Width, AttrName::Height]);
        }
    }
}
fn is_protected_content(dom: &Dom, id: NodeId, store: &crate::dom::NodeStateStore) -> bool {
    matches!(
        dom.tag(id),
        Some(
            Tag::Pre
                | Tag::Code
                | Tag::Figure
                | Tag::Picture
                | Tag::Blockquote
                | Tag::Details
                | Tag::Math
                | Tag::Dl
        )
    ) || dom.tag(id) == Some(Tag::Table) && store.is_data_table(id) == Some(true)
}

pub fn mark_data_tables(
    dom: &Dom,
    root: NodeId,
    store: &mut crate::dom::NodeStateStore,
    nodes: &mut Vec<NodeId>,
) {
    nodes.clear();
    nodes.extend(
        dom.descendants(root)
            .filter(|&x| dom.tag(x) == Some(Tag::Table)),
    );
    for &id in nodes.iter() {
        if dom.attr(id, AttrName::Role) == Some("presentation")
            || dom.attr(id, AttrName::DataTable) == Some("0")
        {
            store.set_data_table(id, crate::dom::DataTableState::Layout);
            continue;
        }
        if dom.has_attr(id, AttrName::Summary)
            || dom
                .descendants(id)
                .any(|x| dom.tag(x) == Some(Tag::Caption) && dom.children(x).next().is_some())
            || dom.descendants(id).any(|x| {
                matches!(
                    dom.tag(x),
                    Some(Tag::Col | Tag::Colgroup | Tag::Tfoot | Tag::Thead | Tag::Th)
                )
            })
        {
            store.set_data_table(id, crate::dom::DataTableState::Data);
            continue;
        }
        if dom.descendants(id).any(|x| dom.tag(x) == Some(Tag::Table)) {
            store.set_data_table(id, crate::dom::DataTableState::Layout);
            continue;
        }
        if is_repeated_listing_table(dom, id) {
            store.set_data_table(id, crate::dom::DataTableState::Listing);
            continue;
        }
        let mut rows = 0;
        let mut cols = 0;
        for tr in dom.descendants(id).filter(|&x| dom.tag(x) == Some(Tag::Tr)) {
            rows += dom
                .attr(tr, AttrName::RowSpan)
                .and_then(|v| v.parse().ok())
                .unwrap_or(1);
            let c = dom
                .element_children(tr)
                .filter(|&x| matches!(dom.tag(x), Some(Tag::Td | Tag::Th)))
                .map(|x| {
                    dom.attr(x, AttrName::ColSpan)
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(1)
                })
                .sum();
            cols = cols.max(c);
        }
        store.set_data_table(
            id,
            if cols == 1 || rows == 1 || rows < 10 && cols <= 4 && rows * cols <= 10 {
                crate::dom::DataTableState::Layout
            } else {
                crate::dom::DataTableState::Data
            },
        );
    }
}

/// Returns true for a conservative, rank-based repeated-content table.
///
/// A rank alone is not sufficient. The table must have several similarly
/// shaped linked rows and must not have explicit data-table semantics.
pub(crate) fn is_repeated_listing_table(dom: &Dom, table: NodeId) -> bool {
    repeated_listing_start(dom, table).is_some()
}

pub(crate) fn repeated_listing_start(dom: &Dom, table: NodeId) -> Option<u32> {
    if dom.tag(table) != Some(Tag::Table)
        || dom.has_attr(table, AttrName::Summary)
        || dom
            .attr(table, AttrName::DataTable)
            .is_some_and(|value| value != "0")
        || dom.attr(table, AttrName::Role).is_some_and(|role| {
            role.split_whitespace().any(|value| {
                value.eq_ignore_ascii_case("table")
                    || value.eq_ignore_ascii_case("grid")
                    || value.eq_ignore_ascii_case("treegrid")
            })
        })
        || dom.descendants(table).any(|node| {
            matches!(
                dom.tag(node),
                Some(Tag::Caption | Tag::Th | Tag::Thead | Tag::Tfoot)
            )
        })
    {
        return None;
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
    if rows.len() < 6 {
        return None;
    }

    let mut ranked_rows = 0usize;
    let mut linked_ranked_rows = 0usize;
    let mut metadata_rows = 0usize;
    let mut expect_metadata = false;
    let mut outside_text_after_rank = None;
    let mut first_rank: Option<u32> = None;
    let mut previous_rank: Option<u32> = None;
    let mut common_columns = None;
    let mut common_shape = 0usize;
    let mut buffer = String::new();
    for row in rows.iter().copied() {
        let cells: SmallVec<[NodeId; 8]> = dom
            .element_children(row)
            .filter(|&cell| matches!(dom.tag(cell), Some(Tag::Td | Tag::Th)))
            .collect();
        let rank = cells.first().and_then(|&cell| {
            let text = crate::scoring::get_normalized_inner_text(dom, cell, &mut buffer);
            parse_rank_text(text)
        });
        let Some(rank) = rank else {
            if dom.has_non_whitespace_text(row) {
                if expect_metadata {
                    metadata_rows += 1;
                    expect_metadata = false;
                } else if outside_text_after_rank.replace(ranked_rows).is_some() {
                    return None;
                }
            }
            continue;
        };
        if cells.len() < 2
            || previous_rank.is_some_and(|previous| previous.checked_add(1) != Some(rank))
        {
            return None;
        }
        first_rank.get_or_insert(rank);
        previous_rank = Some(rank);
        ranked_rows += 1;
        expect_metadata = true;
        if dom
            .descendants(row)
            .any(|node| dom.tag(node) == Some(Tag::A) && dom.has_non_whitespace_text(node))
        {
            linked_ranked_rows += 1;
        }
        match common_columns {
            Some(columns) if columns == cells.len() => common_shape += 1,
            None => {
                common_columns = Some(cells.len());
                common_shape = 1;
            }
            _ => {}
        }
    }

    (ranked_rows >= 3
        && linked_ranked_rows == ranked_rows
        && metadata_rows + 1 >= ranked_rows
        && outside_text_after_rank.is_none_or(|position| position == ranked_rows)
        && common_shape * 4 >= ranked_rows * 3
        && ranked_rows * 4 >= rows.len())
    .then_some(first_rank?)
}

fn parse_rank_text(text: &str) -> Option<u32> {
    let text = text.trim();
    let digits = text.strip_suffix('.').unwrap_or(text);
    (!digits.is_empty() && digits.len() <= 6 && digits.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| digits.parse().ok())
        .flatten()
}

pub fn fix_lazy_images(dom: &mut Dom, root: NodeId, nodes: &mut Vec<NodeId>) {
    nodes.clear();
    nodes.extend(
        dom.descendants(root)
            .filter(|&x| matches!(dom.tag(x), Some(Tag::Img | Tag::Picture | Tag::Figure))),
    );
    for &id in nodes.iter() {
        let mut src = false;
        let mut srcset = false;
        let mut lazy = false;
        let mut b64 = false;
        let mut other = false;
        let mut lazy_src = None;
        let mut lazy_srcset = None;
        for a in dom.attrs(id) {
            let v = a.value.as_ref();
            match AttrName::from_local(a.name.local.as_ref()) {
                AttrName::Src => {
                    src = !v.is_empty();
                    if let Some((_, media_type)) = parse_b64_data_url(v)
                        && media_type != "image/svg+xml"
                    {
                        b64 = true;
                    }
                }
                AttrName::Srcset => srcset = !v.is_empty() && v != "null",
                AttrName::DataSrc => {
                    other |= has_image_extension(v);
                    if has_image_src(v) {
                        lazy_src = Some(v.to_string())
                    }
                }
                AttrName::DataSrcset => {
                    other |= has_image_extension(v);
                    if has_image_srcset(v) {
                        lazy_srcset = Some(v.to_string())
                    }
                }
                AttrName::Class => {
                    lazy |= v.split_whitespace().any(|x| x.eq_ignore_ascii_case("lazy"))
                }
                _ => {
                    other |= has_image_extension(v);
                    if has_image_srcset(v) && lazy_srcset.is_none() {
                        lazy_srcset = Some(v.to_string())
                    } else if has_image_src(v) && lazy_src.is_none() {
                        lazy_src = Some(v.to_string())
                    }
                }
            }
        }
        if b64
            && other
            && let Some(v) = dom.attr(id, AttrName::Src)
            && let Some((end, _)) = parse_b64_data_url(v)
            && v.len().saturating_sub(end) < 133
        {
            dom.remove_attr(id, AttrName::Src);
            src = false;
        }
        if (src || srcset) && !lazy {
            continue;
        }
        let (value, attr) = if let Some(v) = lazy_srcset {
            (v, AttrName::Srcset)
        } else if let Some(v) = lazy_src {
            (v, AttrName::Src)
        } else {
            continue;
        };
        match dom.tag(id) {
            Some(Tag::Img) => dom.set_attr(id, attr, &value),
            Some(Tag::Picture) => {
                let image = dom
                    .first_descendant_by_tag(id, Tag::Img)
                    .or_else(|| dom.create_html_element(Tag::Img).ok());
                if let Some(image) = image {
                    dom.set_attr(image, attr, &value);
                    if dom.parent(image).is_none() {
                        dom.append_child(id, image);
                    }
                }
            }
            Some(Tag::Figure) if !dom.any_descendant_by_tags(id, &[Tag::Img, Tag::Picture]) => {
                if let Ok(image) = dom.create_html_element(Tag::Img) {
                    dom.set_attr(image, attr, &value);
                    dom.append_child(id, image);
                }
            }
            _ => {}
        }
    }
}
fn single_image_fragment(dom: &Dom) -> Option<NodeId> {
    let mut image = None;
    for node in dom.children(dom.root()) {
        if dom.tag(node) == Some(Tag::Img) && image.is_none() {
            image = Some(node);
        } else if !dom
            .text_node(node)
            .is_some_and(|text| text.trim().is_empty())
        {
            return None;
        }
    }
    image
}

fn useful_image(dom: &Dom, id: NodeId) -> bool {
    dom.attrs(id).iter().any(|attribute| {
        let name = attribute.name.local.as_ref();
        matches!(name, "src" | "srcset" | "data-src" | "data-srcset")
            || has_image_extension(attribute.value.as_ref())
    })
}

fn copy_image_attributes(dom: &mut Dom, from: NodeId, to: NodeId) {
    let attrs: Vec<_> = dom
        .attrs(from)
        .iter()
        .filter(|a| {
            !a.value.is_empty()
                && (matches!(
                    AttrName::from_local(a.name.local.as_ref()),
                    AttrName::Src | AttrName::Srcset
                ) || has_image_extension(a.value.as_ref()))
        })
        .map(|a| (a.name.clone(), a.value.clone()))
        .collect();
    for (mut name, value) in attrs {
        if dom.attr_by_local_name(to, name.local.as_ref()) == Some(value.as_ref()) {
            continue;
        }
        if dom.attr_by_local_name(to, name.local.as_ref()).is_some() {
            name = QualName::new(
                None,
                ns!(),
                LocalName::from(format!("data-old-{}", name.local)),
            );
        }
        dom.set_attr_qual(to, name, value);
    }
}

fn previous_element(dom: &Dom, id: NodeId) -> Option<NodeId> {
    let mut previous = dom.prev_sibling(id);
    while let Some(node) = previous {
        if dom.is_element(node) {
            return Some(node);
        }
        previous = dom.prev_sibling(node);
    }
    None
}

fn single_image_element(dom: &Dom, id: NodeId) -> Option<NodeId> {
    if dom.tag(id) == Some(Tag::Img) {
        return Some(id);
    }
    if dom.has_non_whitespace_text(id) {
        return None;
    }
    let mut images = dom
        .descendants(id)
        .filter(|&node| dom.tag(node) == Some(Tag::Img));
    let image = images.next()?;
    images.next().is_none().then_some(image)
}

fn is_tracking_image(dom: &Dom, id: NodeId) -> bool {
    [AttrName::Width, AttrName::Height]
        .into_iter()
        .filter_map(|name| dom.attr(id, name)?.parse::<u32>().ok())
        .any(|size| size <= 1)
}

fn is_placeholder_image(dom: &Dom, id: NodeId) -> bool {
    dom.attr(id, AttrName::Src).is_some_and(|src| {
        src.to_ascii_lowercase().contains("placeholder")
            || parse_b64_data_url(src).is_some_and(|(end, _)| src.len().saturating_sub(end) < 133)
    })
}

fn images_are_variants(dom: &Dom, first: NodeId, second: NodeId) -> bool {
    let same_nonempty_attr = |name| {
        dom.attr(first, name)
            .filter(|value| !value.is_empty())
            .is_some_and(|value| dom.attr(second, name) == Some(value))
    };
    let url_attrs = [
        AttrName::Src,
        AttrName::Srcset,
        AttrName::DataSrc,
        AttrName::DataSrcset,
    ];
    let mut same_url = false;
    let mut same_basename = false;
    for first_url in url_attrs.iter().filter_map(|&name| dom.attr(first, name)) {
        for second_url in url_attrs.iter().filter_map(|&name| dom.attr(second, name)) {
            same_url |= !first_url.is_empty() && first_url == second_url;
            let first_name = first_url
                .split(['?', '#'])
                .next()
                .and_then(|value| value.rsplit('/').next())
                .filter(|value| !value.is_empty());
            let second_name = second_url
                .split(['?', '#'])
                .next()
                .and_then(|value| value.rsplit('/').next())
                .filter(|value| !value.is_empty());
            same_basename |= first_name.is_some() && first_name == second_name;
        }
    }
    let same_dimensions =
        same_nonempty_attr(AttrName::Width) && same_nonempty_attr(AttrName::Height);
    let same_alt = dom
        .attr_by_local_name(first, "alt")
        .filter(|value| !value.is_empty())
        .is_some_and(|value| dom.attr_by_local_name(second, "alt") == Some(value));
    same_url || same_dimensions && (same_basename || same_alt)
}

fn previous_useful_image(
    dom: &Dom,
    id: NodeId,
    fallback: NodeId,
) -> (Option<(NodeId, NodeId)>, SmallVec<[NodeId; 1]>) {
    let Some(immediate) = previous_element(dom, id) else {
        return (None, SmallVec::new());
    };
    let Some(immediate_image) = single_image_element(dom, immediate) else {
        return (None, SmallVec::new());
    };
    if useful_image(dom, immediate_image) {
        return if images_are_variants(dom, immediate_image, fallback)
            || is_placeholder_image(dom, immediate_image)
        {
            (Some((immediate, immediate_image)), SmallVec::new())
        } else {
            (None, SmallVec::new())
        };
    }

    // Some lazy-image implementations put an empty hydration image between a
    // low-resolution image and its noscript fallback. Scan past only one such
    // placeholder, and require matching image metadata before merging them.
    if let Some(previous) = previous_element(dom, immediate)
        && let Some(previous_image) = single_image_element(dom, previous)
        && useful_image(dom, previous_image)
        && images_are_variants(dom, previous_image, fallback)
    {
        let mut placeholders = SmallVec::new();
        placeholders.push(immediate);
        return (Some((previous, previous_image)), placeholders);
    }

    (Some((immediate, immediate_image)), SmallVec::new())
}

pub fn unwrap_noscript_images(dom: &mut Dom) {
    // Inspect noscript fallbacks without first deleting placeholder images.
    // Replace a placeholder only after a usable fallback image is available.
    let candidates: Vec<_> = dom
        .descendants(dom.root())
        .filter(|&id| dom.tag(id) == Some(Tag::Noscript))
        .collect();
    for id in candidates {
        if dom.parent(id).is_none() {
            continue;
        }
        let image_ids: SmallVec<[NodeId; 2]> = dom
            .descendants(id)
            .filter(|&node| dom.tag(node) == Some(Tag::Img))
            .collect();
        if image_ids.len() == 1 && !dom.has_non_whitespace_text(id) {
            let image = image_ids[0];
            if is_tracking_image(dom, image) {
                continue;
            }
            let (previous, placeholders) = previous_useful_image(dom, id, image);
            if let Some((previous, previous_image)) = previous {
                copy_image_attributes(dom, previous_image, image);
                dom.detach(previous);
                for placeholder in placeholders {
                    dom.detach(placeholder);
                }
            }
            dom.insert_before(id, image);
            dom.detach(id);
            continue;
        }
        if !image_ids.is_empty() {
            continue;
        }
        let mut text_nodes = dom.children(id).filter(|&node| {
            dom.is_text(node) && dom.text_node(node).is_some_and(|t| !t.trim().is_empty())
        });
        let Some(text_node) = text_nodes.next() else {
            continue;
        };
        if text_nodes.next().is_some() {
            continue;
        }
        let Some(markup) = dom.text_node(text_node) else {
            continue;
        };
        let Ok(fragment) = Dom::parse_fragment(markup, Tag::Div) else {
            continue;
        };
        let Some(source_image) = single_image_fragment(&fragment) else {
            continue;
        };
        if is_tracking_image(&fragment, source_image) {
            continue;
        }
        let Ok(new_image) = dom.import_subtree(&fragment, source_image) else {
            continue;
        };
        let (previous, placeholders) = previous_useful_image(dom, id, new_image);
        if let Some((previous, previous_image)) = previous {
            copy_image_attributes(dom, previous_image, new_image);
            dom.detach(previous);
            for placeholder in placeholders {
                dom.detach(placeholder);
            }
        }
        dom.insert_before(id, new_image);
        dom.detach(id);
    }
}
pub fn simplify_nested_elements(dom: &mut Dom, root: NodeId, nodes: &mut Vec<NodeId>) {
    nodes.clear();
    nodes.extend(dom.descendants(root));
    for &id in nodes.iter().rev() {
        if !matches!(dom.tag(id), Some(Tag::Div | Tag::Section)) {
            continue;
        }
        if dom
            .attr(id, AttrName::Id)
            .is_some_and(|value| value.starts_with("legible-content"))
        {
            continue;
        }
        if is_element_without_content(dom, id) {
            dom.detach(id);
            continue;
        }
        if has_single_tag_inside_element(dom, id, Tag::Div)
            || has_single_tag_inside_element(dom, id, Tag::Section)
        {
            if let Some(child) = dom.element_children(id).next() {
                let attrs: Vec<_> = dom
                    .attrs(id)
                    .iter()
                    .map(|a| (a.name.clone(), a.value.clone()))
                    .collect();
                for (a, v) in attrs {
                    if dom.attr_by_local_name(child, a.local.as_ref()).is_none() {
                        dom.set_attr_qual(child, a, v)
                    }
                }
                dom.replace_with(id, child);
            }
        }
    }
}
/// Removes content that is not useful in a retained semantic fragment.
///
/// This phase uses only high-confidence rules. It removes executable markup,
/// hidden scaffolding, tracking images, and interactive controls. It keeps the
/// text and structure around removed form controls.
pub(crate) fn hard_cleanup(
    dom: &mut Dom,
    root: NodeId,
    allowed_media: &Regex,
    nodes: &mut Vec<NodeId>,
) {
    nodes.clear();
    nodes.extend(
        dom.element_descendants_snapshot_with_depth(root)
            .into_iter()
            .map(|(node, _)| node),
    );
    for &node in nodes.iter().rev() {
        if dom.parent(node).is_none() {
            continue;
        }
        let Some(tag) = dom.tag(node) else { continue };
        let fallback_image = tag == Tag::Img
            && dom
                .attr(node, AttrName::Class)
                .is_some_and(|class| class.contains("fallback-image"));
        let hidden = dom.has_attr(node, AttrName::Hidden)
            || dom.attr(node, AttrName::AriaHidden) == Some("true") && !fallback_image
            || dom.attr(node, AttrName::Style).is_some_and(|style| {
                let compact: String = style
                    .bytes()
                    .filter(|byte| !byte.is_ascii_whitespace())
                    .map(char::from)
                    .collect();
                let compact = compact.to_ascii_lowercase();
                compact.contains("display:none") || compact.contains("visibility:hidden")
            });
        let tracking_image = tag == Tag::Img
            && is_tracking_image(dom, node)
            && !has_lazy_image_candidate(dom, node)
            && !picture_has_lazy_source(dom, node);
        let executable = matches!(
            tag,
            Tag::Script | Tag::Style | Tag::Template | Tag::Link | Tag::Meta
        );
        let control = matches!(
            tag,
            Tag::Input | Tag::Textarea | Tag::Select | Tag::Button | Tag::Datalist | Tag::Option
        );
        let disallowed_embed = matches!(tag, Tag::Object | Tag::Embed | Tag::Iframe)
            && !has_allowed_media(dom, node, allowed_media);
        if hidden || tracking_image || executable || control || disallowed_embed {
            dom.detach(node);
        }
    }
}

/// Removes clutter only when several independent signals agree.
pub(crate) fn heuristic_cleanup(
    dom: &mut Dom,
    root: NodeId,
    store: &mut crate::dom::NodeStateStore,
    text_buffer: &mut String,
    nodes: &mut Vec<NodeId>,
) {
    nodes.clear();
    let mut boundary_depth = None;
    for (node, depth) in dom.element_descendants_snapshot_with_depth(root) {
        if let Some(outer_depth) = boundary_depth {
            if depth > outer_depth {
                continue;
            }
            boundary_depth = None;
        }
        if is_heuristic_boundary(dom, node) {
            nodes.push(node);
            boundary_depth = Some(depth);
        }
    }
    store.clear_stats();
    store.enable_link_lengths();
    mark_data_tables(dom, root, store, &mut Vec::new());
    let root_length = get_or_compute_stats(dom, root, store).text_length.max(1);
    for &node in nodes.iter().rev() {
        if dom.parent(node).is_none() || is_protected_content(dom, node, store) {
            continue;
        }
        let stats = get_or_compute_stats(dom, node, store);
        let text = get_inner_text(dom, node, text_buffer).to_ascii_lowercase();
        let name = node_name(dom, node);
        let links = dom
            .descendants(node)
            .filter(|&descendant| dom.tag(descendant) == Some(Tag::A))
            .count();
        let controls = dom
            .descendants(node)
            .filter(|&descendant| {
                matches!(
                    dom.tag(descendant),
                    Some(Tag::Input | Tag::Textarea | Tag::Select | Tag::Button)
                )
            })
            .count();
        let protected = dom
            .descendants(node)
            .any(|descendant| is_protected_content(dom, descendant, store));
        let link_density = get_link_density_cached(dom, node, stats.text_length, store);
        let short = stats.text_length < 350 || stats.text_length * 5 < root_length;

        let related_name = contains_any(
            &name,
            &["related", "recommended", "more-stories", "more_stories"],
        ) || dom.descendants(node).any(|descendant| {
            contains_any(
                &node_name(dom, descendant),
                &["related", "recommended", "more-stories", "more_stories"],
            )
        });
        let related_text = starts_with_any(
            &text,
            &[
                "related",
                "recommended",
                "more stories",
                "you may also like",
            ],
        );
        let related = (related_name || related_text) && links >= 2 && link_density >= 0.2 && short;

        let social_name = contains_any(&name, &["share", "social", "sharedaddy"]);
        let social_links = dom
            .descendants(node)
            .filter(|&descendant| {
                dom.tag(descendant) == Some(Tag::A)
                    && dom.attr(descendant, AttrName::Href).is_some_and(|href| {
                        contains_any(
                            &href.to_ascii_lowercase(),
                            &["facebook.", "twitter.", "x.com/", "linkedin.", "reddit."],
                        )
                    })
            })
            .count();
        let social = social_name && (social_links > 0 || links >= 2) && short;

        let signup_terms = contains_any(
            &format!("{name} {text}"),
            &["newsletter", "subscribe", "sign-up", "signup", "sign up"],
        );
        let has_form = dom.tag(node) == Some(Tag::Form)
            || dom
                .descendants(node)
                .any(|descendant| dom.tag(descendant) == Some(Tag::Form));
        let signup = signup_terms && (controls > 0 || links > 0 || has_form) && short;

        let navigation_semantic = dom.tag(node) == Some(Tag::Nav)
            || dom.attr(node, AttrName::Role).is_some_and(|role| {
                role.split_whitespace()
                    .any(|value| value.eq_ignore_ascii_case("navigation"))
            });
        let menu_name = contains_any(&name, &["menu", "navigation", "breadcrumb"]);
        let documentation_toc = dom
            .attr_by_local_name(node, "aria-label")
            .is_some_and(|label| {
                let label = label.trim().to_ascii_lowercase();
                label == "on this page" || label == "table of contents" || label == "contents"
            })
            || contains_any(
                &name,
                &["table-of-contents", "table_of_contents", "docs-toc"],
            );
        let navigation = navigation_semantic
            && !documentation_toc
            && (menu_name || links >= 3)
            && link_density >= 0.6
            && stats.text_length < 500;

        let author_name = contains_any(&name, &["author-bio", "author_bio", "profile", "bio"]);
        let author = author_name && short && (social_links > 0 || links >= 2);

        let advertisement =
            strong_ad_name(&name) && short && (links > 0 || stats.text_length < 100);
        let consent = contains_any(
            &format!("{name} {text}"),
            &["cookie consent", "cookie-banner", "consent-banner"],
        ) && short;
        let account = contains_any(&name, &["login", "sign-in", "signin"])
            && (controls > 0 || links > 0)
            && short;

        if related
            || social
            || signup
            || navigation
            || author
            || advertisement
            || consent
            || account
        {
            if protected {
                hoist_protected_children(dom, node, store);
            }
            dom.detach(node);
        }
    }
}

fn hoist_protected_children(dom: &mut Dom, wrapper: NodeId, store: &crate::dom::NodeStateStore) {
    let protected: SmallVec<[NodeId; 4]> = dom
        .descendants(wrapper)
        .filter(|&node| {
            is_protected_content(dom, node, store)
                && !dom
                    .ancestors(node)
                    .take_while(|&ancestor| ancestor != wrapper)
                    .any(|ancestor| is_protected_content(dom, ancestor, store))
        })
        .collect();
    for node in protected {
        dom.insert_before(wrapper, node);
    }
}

fn has_lazy_image_candidate(dom: &Dom, image: NodeId) -> bool {
    dom.attrs(image).iter().any(|attribute| {
        let name = attribute.name.local.as_ref();
        name.starts_with("data-")
            && (has_image_src(attribute.value.as_ref())
                || has_image_srcset(attribute.value.as_ref()))
    })
}

fn picture_has_lazy_source(dom: &Dom, image: NodeId) -> bool {
    let Some(picture) = dom
        .ancestors(image)
        .find(|&ancestor| dom.tag(ancestor) == Some(Tag::Picture))
    else {
        return false;
    };
    if dom
        .attr(picture, AttrName::DataSrc)
        .is_some_and(has_image_src)
        || dom
            .attr(picture, AttrName::DataSrcset)
            .is_some_and(has_image_srcset)
    {
        return true;
    }
    dom.descendants(picture).any(|source| {
        dom.tag(source) == Some(Tag::Source)
            && (dom
                .attr(source, AttrName::DataSrc)
                .or_else(|| dom.attr(source, AttrName::Src))
                .is_some_and(has_image_src)
                || dom
                    .attr(source, AttrName::DataSrcset)
                    .or_else(|| dom.attr(source, AttrName::Srcset))
                    .is_some_and(has_image_srcset))
    })
}

fn is_heuristic_boundary(dom: &Dom, node: NodeId) -> bool {
    if matches!(
        dom.tag(node),
        Some(Tag::Aside | Tag::Footer | Tag::Form | Tag::Header | Tag::Nav)
    ) {
        return true;
    }
    matches!(
        dom.tag(node),
        Some(Tag::Div | Tag::Ol | Tag::Section | Tag::Ul)
    ) && contains_any(
        &node_name(dom, node),
        &[
            "related",
            "recommend",
            "share",
            "social",
            "newsletter",
            "subscribe",
            "signup",
            "menu",
            "navigation",
            "breadcrumb",
            "author",
            "profile",
            "bio",
            "advert",
            "sponsor",
            "cookie",
            "consent",
            "login",
            "signin",
            "sign-in",
            "sidebar",
        ],
    )
}

fn node_name(dom: &Dom, node: NodeId) -> String {
    let mut value = String::new();
    for name in [AttrName::Class, AttrName::Id] {
        if let Some(part) = dom.attr(node, name) {
            if !value.is_empty() {
                value.push(' ');
            }
            value.push_str(part);
        }
    }
    value.make_ascii_lowercase();
    value
}

fn contains_any(value: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| value.contains(needle))
}

fn starts_with_any(value: &str, needles: &[&str]) -> bool {
    let value = value.trim_start();
    needles.iter().any(|needle| value.starts_with(needle))
}

fn strong_ad_name(value: &str) -> bool {
    value
        .split(|character: char| !character.is_ascii_alphanumeric())
        .any(|token| {
            matches!(
                token,
                "ad" | "ads" | "advert" | "advertisement" | "sponsor" | "sponsored"
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dom::{AttrName, NodeStateStore};

    #[test]
    fn normalizes_br_runs_before_renaming_fonts() {
        let mut dom = Dom::parse_document("<body><div><br><br><font>text</font></div>").unwrap();

        prep_document(&mut dom);

        let body = dom.body().unwrap();
        let paragraph = dom.first_descendant_by_tag(body, Tag::P).unwrap();
        let span = dom.first_descendant_by_tag(body, Tag::Span).unwrap();
        assert_eq!(dom.parent(paragraph), dom.parent(span));
        assert!(!dom.descendants(paragraph).any(|id| id == span));
    }

    #[test]
    fn replaces_short_base64_image_placeholders() {
        let mut dom = Dom::parse_document(
            r#"<img src="data:image/png;base64,AAAA" data-src="https://example.com/image.jpg">"#,
        )
        .unwrap();
        let root = dom.root();
        let image = dom.first_descendant_by_tag(root, Tag::Img).unwrap();
        fix_lazy_images(&mut dom, root, &mut Vec::new());
        assert_eq!(
            dom.attr(image, AttrName::Src),
            Some("https://example.com/image.jpg")
        );
    }

    #[test]
    fn preserves_unmatched_image_placeholders() {
        let mut dom = Dom::parse_document(
            r#"<main><img id="placeholder"><noscript>Image unavailable</noscript></main>"#,
        )
        .unwrap();
        let placeholder = dom.first_descendant_by_tag(dom.root(), Tag::Img).unwrap();

        unwrap_noscript_images(&mut dom);

        assert!(dom.parent(placeholder).is_some());
    }

    #[test]
    fn does_not_merge_a_fallback_with_an_unrelated_adjacent_image() {
        let mut dom = Dom::parse_document(
            r#"<main><img src="first.jpg"><noscript><img src="second.jpg"></noscript></main>"#,
        )
        .unwrap();

        unwrap_noscript_images(&mut dom);

        let sources: Vec<_> = dom
            .descendants(dom.root())
            .filter(|&id| dom.tag(id) == Some(Tag::Img))
            .filter_map(|id| dom.attr(id, AttrName::Src))
            .collect();
        assert_eq!(sources, ["first.jpg", "second.jpg"]);
    }

    #[test]
    fn does_not_merge_distinct_images_with_equal_dimensions() {
        let mut dom = Dom::parse_document(
            r#"<main><img src="first.jpg" width="300" height="200"><noscript><img src="second.jpg" width="300" height="200"></noscript></main>"#,
        )
        .unwrap();

        unwrap_noscript_images(&mut dom);

        let sources: Vec<_> = dom
            .descendants(dom.root())
            .filter(|&id| dom.tag(id) == Some(Tag::Img))
            .filter_map(|id| dom.attr(id, AttrName::Src))
            .collect();
        assert_eq!(sources, ["first.jpg", "second.jpg"]);
    }

    #[test]
    fn does_not_merge_distinct_images_with_the_same_basename() {
        let mut dom = Dom::parse_document(
            r#"<main><img src="/first/image.jpg"><noscript><img src="/second/image.jpg"></noscript></main>"#,
        )
        .unwrap();

        unwrap_noscript_images(&mut dom);

        let sources: Vec<_> = dom
            .descendants(dom.root())
            .filter(|&id| dom.tag(id) == Some(Tag::Img))
            .filter_map(|id| dom.attr(id, AttrName::Src))
            .collect();
        assert_eq!(sources, ["/first/image.jpg", "/second/image.jpg"]);
    }

    #[test]
    fn does_not_merge_a_fallback_with_an_unrelated_earlier_image() {
        let mut dom = Dom::parse_document(
            r#"<main><img src="first.jpg"><img><noscript><img src="second.jpg"></noscript></main>"#,
        )
        .unwrap();

        unwrap_noscript_images(&mut dom);

        let sources: Vec<_> = dom
            .descendants(dom.root())
            .filter(|&id| dom.tag(id) == Some(Tag::Img))
            .filter_map(|id| dom.attr(id, AttrName::Src))
            .collect();
        assert_eq!(sources, ["first.jpg", "second.jpg"]);
    }

    #[test]
    fn unwraps_only_single_image_noscripts_and_replaces_placeholder() {
        let mut dom = Dom::parse_document(
            r#"<img src="placeholder.jpg"><noscript><img src="real.jpg"></noscript>"#,
        )
        .unwrap();
        unwrap_noscript_images(&mut dom);
        let images: Vec<_> = dom
            .descendants(dom.root())
            .filter(|&id| dom.tag(id) == Some(Tag::Img))
            .collect();
        assert_eq!(images.len(), 1);
        assert_eq!(dom.attr(images[0], AttrName::Src), Some("real.jpg"));
    }

    #[test]
    fn promotes_a_standalone_direct_noscript_image() {
        let mut dom =
            Dom::parse_document(r#"<main><noscript><img src="standalone.jpg"></noscript></main>"#)
                .unwrap();

        unwrap_noscript_images(&mut dom);

        let image = dom.first_descendant_by_tag(dom.root(), Tag::Img).unwrap();
        assert_eq!(dom.attr(image, AttrName::Src), Some("standalone.jpg"));
        assert!(
            !dom.descendants(dom.root())
                .any(|id| dom.tag(id) == Some(Tag::Noscript))
        );
    }

    #[test]
    fn parses_escaped_noscript_image_text_without_serializing() {
        let mut dom = Dom::parse_document(
            r#"<img src="placeholder.jpg"><noscript>&lt;img src="real.jpg" data-id="1"&gt;</noscript>"#,
        )
        .unwrap();

        unwrap_noscript_images(&mut dom);

        let images: Vec<_> = dom
            .descendants(dom.root())
            .filter(|&id| dom.tag(id) == Some(Tag::Img))
            .collect();
        assert_eq!(images.len(), 1);
        assert_eq!(dom.attr(images[0], AttrName::Src), Some("real.jpg"));
        assert_eq!(dom.attr_by_local_name(images[0], "data-id"), Some("1"));
    }

    #[test]
    fn promotes_a_standalone_escaped_noscript_image() {
        let mut dom = Dom::parse_document(
            r#"<main><noscript>&lt;img src="standalone.jpg"&gt;</noscript></main>"#,
        )
        .unwrap();

        unwrap_noscript_images(&mut dom);

        let image = dom.first_descendant_by_tag(dom.root(), Tag::Img).unwrap();
        assert_eq!(dom.attr(image, AttrName::Src), Some("standalone.jpg"));
        assert!(
            !dom.descendants(dom.root())
                .any(|id| dom.tag(id) == Some(Tag::Noscript))
        );
    }

    #[test]
    fn prefers_data_src_over_image_like_alt_text() {
        let mut dom = Dom::parse_document(
            r#"<img class="lazy" alt="placeholder.jpg" data-src="https://example.com/article.jpg">"#,
        )
        .unwrap();
        let root = dom.root();
        let image = dom.first_descendant_by_tag(root, Tag::Img).unwrap();

        fix_lazy_images(&mut dom, root, &mut Vec::new());

        assert_eq!(
            dom.attr(image, AttrName::Src),
            Some("https://example.com/article.jpg")
        );
    }

    #[test]
    fn applies_lazy_picture_source_to_its_image() {
        let mut dom = Dom::parse_document(
            r#"<picture data-src="photo.jpg"><source srcset="photo.webp"><img alt="Photo"></picture>"#,
        )
        .unwrap();
        let root = dom.root();
        let image = dom.first_descendant_by_tag(root, Tag::Img).unwrap();

        fix_lazy_images(&mut dom, root, &mut Vec::new());

        assert_eq!(dom.attr(image, AttrName::Src), Some("photo.jpg"));
        let picture = dom.first_descendant_by_tag(root, Tag::Picture).unwrap();
        assert_eq!(dom.attr(picture, AttrName::Src), None);
    }

    #[test]
    fn adds_lazy_figure_image_without_removing_caption() {
        let mut dom = Dom::parse_document(
            r#"<figure data-src="image.jpg?x=1&amp;y=2"><figcaption>old</figcaption></figure>"#,
        )
        .unwrap();
        let root = dom.root();
        let figure = dom.first_descendant_by_tag(root, Tag::Figure).unwrap();

        fix_lazy_images(&mut dom, root, &mut Vec::new());

        let image = dom.first_descendant_by_tag(figure, Tag::Img).unwrap();
        assert_eq!(dom.attr(image, AttrName::Src), Some("image.jpg?x=1&y=2"));
        assert_eq!(dom.element_children(figure).count(), 2);
        assert!(
            dom.first_descendant_by_tag(figure, Tag::Figcaption)
                .is_some()
        );
    }

    fn clean_fragment(html: &str) -> String {
        let mut dom = Dom::parse_fragment(html, Tag::Div).unwrap();
        let root = dom.root();
        let mut nodes = Vec::new();
        let mut store = NodeStateStore::new();
        let mut text = String::new();
        let allowed = Regex::new("video\\.example").unwrap();
        clean_styles(&mut dom, root, &mut nodes);
        hard_cleanup(&mut dom, root, &allowed, &mut nodes);
        heuristic_cleanup(&mut dom, root, &mut store, &mut text, &mut nodes);
        dom.text(root)
    }

    #[test]
    fn explicit_data_table_semantics_prevent_listing_classification() {
        for attribute in [r#"role="table""#, r#"datatable="1""#] {
            let html = format!(
                r#"<table {attribute}><tr><td>1.</td><td><a href='/a'>A</a></td></tr><tr><td></td><td>A details</td></tr><tr><td>2.</td><td><a href='/b'>B</a></td></tr><tr><td></td><td>B details</td></tr><tr><td>3.</td><td><a href='/c'>C</a></td></tr><tr><td></td><td>C details</td></tr></table>"#
            );
            let dom = Dom::parse_fragment(&html, Tag::Div).unwrap();
            let table = dom.first_descendant_by_tag(dom.root(), Tag::Table).unwrap();
            assert!(repeated_listing_start(&dom, table).is_none());
        }
    }

    #[test]
    fn hard_cleanup_removes_controls_and_keeps_form_text() {
        let text = clean_fragment(
            r#"<form><p>Configuration details remain useful.</p><label>Name<input></label><button>Submit</button></form><script>bad()</script>"#,
        );
        assert!(text.contains("Configuration details"), "{text}");
        assert!(!text.contains("Submit"), "{text}");
        assert!(!text.contains("bad"), "{text}");
    }

    #[test]
    fn heuristic_cleanup_removes_strong_clutter() {
        let text = clean_fragment(
            r#"<main><p>Primary documentation remains.</p>
            <nav class="menu"><a href="/a">A</a><a href="/b">B</a><a href="/c">C</a></nav>
            <aside class="related"><h2>Related stories</h2><a href="/1">One story</a><a href="/2">Two story</a></aside>
            <div class="social-share"><a href="https://twitter.com/share">Twitter</a><a href="https://facebook.com/share">Facebook</a></div>
            <aside class="newsletter"><p>Subscribe to our newsletter</p><form><input><button>Join</button></form></aside>
            <aside class="author-bio"><p>About the author</p><a href="/author">Profile</a><a href="https://x.com/a">Social</a></aside>
            <div class="advertisement"><a href="/buy">Sponsored</a></div></main>"#,
        );
        assert!(text.contains("Primary documentation"), "{text}");
        for clutter in [
            "One story",
            "Twitter",
            "Subscribe",
            "About the author",
            "Sponsored",
        ] {
            assert!(!text.contains(clutter), "retained {clutter}: {text}");
        }
    }

    #[test]
    fn heuristic_cleanup_keeps_substantial_callouts_and_documentation_toc() {
        let text = clean_fragment(
            r##"<main>
            <aside class="sidebar callout"><h2>Compatibility note</h2><p>This callout contains substantial guidance. It explains supported systems, migration constraints, failure behavior, recovery steps, and validation requirements.</p><pre><code>cargo test</code></pre></aside>
            <nav aria-label="On this page"><h2>Contents</h2><a href="#one">Installation and configuration reference</a><a href="#two">Detailed API behavior and examples</a><a href="#three">Troubleshooting and recovery guidance</a></nav>
            <p>The primary guide provides complete instructions.</p></main>"##,
        );
        assert!(text.contains("Compatibility note"), "{text}");
        assert!(text.contains("cargo test"), "{text}");
        assert!(text.contains("Installation and configuration"), "{text}");
    }

    #[test]
    fn hard_cleanup_preserves_lazy_tracking_placeholders() {
        let mut dom = Dom::parse_fragment(
            r#"<img width="1" height="1" src="blank.gif" data-src="photo.jpg" alt="Photo"><picture data-src="other.jpg"><img width="1" alt="Other"></picture><img width="1" data-lazy-src="lazy.jpg"><img height="1" data-original="original.jpg">"#,
            Tag::Div,
        )
        .unwrap();
        let root = dom.root();
        let image = dom.first_descendant_by_tag(root, Tag::Img).unwrap();
        hard_cleanup(&mut dom, root, &Regex::new("$").unwrap(), &mut Vec::new());
        assert!(dom.parent(image).is_some());
        let picture_image = dom
            .descendants(root)
            .filter(|&node| dom.tag(node) == Some(Tag::Img))
            .nth(1)
            .unwrap();
        assert!(dom.parent(picture_image).is_some());
        let remaining_images = dom
            .descendants(root)
            .filter(|&node| dom.tag(node) == Some(Tag::Img))
            .count();
        assert_eq!(remaining_images, 4);
    }

    #[test]
    fn hard_cleanup_preserves_math_fallback_images() {
        let mut dom = Dom::parse_fragment(
            r#"<math><mi>x</mi></math><img class="mwe-math-fallback-image-inline" aria-hidden="true" src="equation.svg" alt="x">"#,
            Tag::Div,
        )
        .unwrap();
        let root = dom.root();
        hard_cleanup(&mut dom, root, &Regex::new("$").unwrap(), &mut Vec::new());
        assert!(dom.first_descendant_by_tag(root, Tag::Img).is_some());
    }

    #[test]
    fn heuristic_cleanup_scans_nested_boundaries_once() {
        let depth = 2_000;
        let mut html = "<div class=\"sidebar\">".repeat(depth);
        html.push_str("Retained documentation.");
        html.push_str(&"</div>".repeat(depth));
        let text = clean_fragment(&html);
        assert!(text.contains("Retained documentation"));
    }

    #[test]
    fn preserves_svg_presentation_attributes() {
        let mut dom = Dom::parse_document(
            r#"<svg width="10" height="10"><path fill="red" stroke="blue"/></svg>"#,
        )
        .unwrap();
        let root = dom.root();
        let svg = dom.first_descendant_by_tag(root, Tag::Svg).unwrap();
        let path = dom.first_descendant_by_tag(svg, Tag::Svg).unwrap();
        clean_styles(&mut dom, root, &mut Vec::new());
        assert_eq!(dom.attr_by_local_name(svg, "width"), Some("10"));
        assert_eq!(dom.attr_by_local_name(path, "fill"), Some("red"));
    }
}
