//! DOM preparation and content cleanup.
#![allow(clippy::collapsible_if)]
use crate::constants::{
    PRESENTATIONAL_ATTRIBUTES, has_image_extension, has_image_src, has_image_srcset,
    is_deprecated_size_attribute_elem, parse_b64_data_url,
};
use crate::dom::{AttrName, Dom, NodeId, NodeStats, Tag};
use crate::scoring::{
    get_inner_text, get_link_density_cached, get_or_compute_stats, has_hidden_utility_class,
    has_single_tag_inside_element, has_static_hidden_marker, is_element_without_content,
    is_hidden_utility_class, is_phrasing_content,
};
use html5ever::{LocalName, QualName, ns};
use regex::Regex;
use smallvec::SmallVec;

pub fn prep_document(dom: &mut Dom) {
    // Preserve the required preparation order. Remove inactive subtrees,
    // normalize BR runs, and only then rename deprecated font elements.
    let mut ids: Vec<_> = dom
        .descendants(dom.root())
        .filter(|&id| {
            matches!(dom.tag(id), Some(Tag::Noscript | Tag::Style))
                || dom.tag(id) == Some(Tag::Script)
                    && !dom.attr(id, AttrName::Type).is_some_and(|value| {
                        let value = value.trim().to_ascii_lowercase();
                        value == "math/tex" || value.starts_with("math/tex;") || value == "text/tex"
                    })
        })
        .collect();
    for &id in &ids {
        dom.detach(id);
    }

    ids.clear();
    if let Some(body) = dom.body() {
        ids.extend(dom.descendants(body).filter(|&id| {
            dom.tag(id) == Some(Tag::Br)
                && !dom
                    .ancestors(id)
                    .any(|ancestor| matches!(dom.tag(ancestor), Some(Tag::Pre | Tag::Code)))
        }));
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
    std::iter::once(id).chain(dom.ancestors(id)).any(|node| {
        dom.attr(node, AttrName::DataFootnote).is_some()
            || dom.attr(node, AttrName::DataFootnotes).is_some()
    }) || dom.attr(id, AttrName::DataMath).is_some()
        || matches!(
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
        )
        || dom.tag(id) == Some(Tag::Table) && store.is_data_table(id) == Some(true)
}

fn has_protected_ancestor(
    dom: &Dom,
    id: NodeId,
    root: NodeId,
    store: &crate::dom::NodeStateStore,
) -> bool {
    dom.ancestors(id)
        .take_while(|&ancestor| ancestor != root)
        .any(|ancestor| is_protected_content(dom, ancestor, store))
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

        // Collect all table evidence in one subtree walk. The previous version
        // scanned each table up to four times, which made malformed nested
        // tables disproportionately expensive.
        let mut has_data_structure = dom.has_attr(id, AttrName::Summary);
        let mut has_nested_table = false;
        let mut rows = 0_u32;
        let mut cols = 0_u32;
        for descendant in dom.descendants(id) {
            match dom.tag(descendant) {
                Some(Tag::Table) => has_nested_table = true,
                Some(Tag::Caption) if dom.children(descendant).next().is_some() => {
                    has_data_structure = true
                }
                Some(Tag::Col | Tag::Colgroup | Tag::Tfoot | Tag::Thead | Tag::Th) => {
                    has_data_structure = true
                }
                Some(Tag::Tr) => {
                    rows = rows.saturating_add(
                        dom.attr(descendant, AttrName::RowSpan)
                            .and_then(|value| value.parse().ok())
                            .unwrap_or(1),
                    );
                    let column_count = dom
                        .element_children(descendant)
                        .filter(|&cell| matches!(dom.tag(cell), Some(Tag::Td | Tag::Th)))
                        .map(|cell| {
                            dom.attr(cell, AttrName::ColSpan)
                                .and_then(|value| value.parse().ok())
                                .unwrap_or(1)
                        })
                        .fold(0_u32, u32::saturating_add);
                    cols = cols.max(column_count);
                }
                _ => {}
            }
        }
        if has_data_structure {
            store.set_data_table(id, crate::dom::DataTableState::Data);
        } else if has_nested_table {
            store.set_data_table(id, crate::dom::DataTableState::Layout);
        } else if is_repeated_listing_table(dom, id) {
            store.set_data_table(id, crate::dom::DataTableState::Listing);
        } else {
            store.set_data_table(
                id,
                if cols == 1
                    || rows == 1
                    || rows < 10 && cols <= 4 && rows.saturating_mul(cols) <= 10
                {
                    crate::dom::DataTableState::Layout
                } else {
                    crate::dom::DataTableState::Data
                },
            );
        }
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
        if is_element_without_content(dom, id) && dom.attr(id, AttrName::DataMath).is_none() {
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
fn preserve_media_from_hidden_variant(dom: &mut Dom, hidden: NodeId) {
    let adjacent_element = |forward: bool| {
        let mut sibling = if forward {
            dom.next_sibling(hidden)
        } else {
            dom.prev_sibling(hidden)
        };
        while sibling.is_some_and(|node| {
            dom.text_node(node)
                .is_some_and(|text| text.trim().is_empty())
        }) {
            sibling = sibling.and_then(|node| {
                if forward {
                    dom.next_sibling(node)
                } else {
                    dom.prev_sibling(node)
                }
            });
        }
        sibling.filter(|&node| dom.is_element(node))
    };
    let sibling = [adjacent_element(false), adjacent_element(true)]
        .into_iter()
        .flatten()
        .find(|&sibling| {
            !has_hidden_utility_class(dom, sibling)
                && dom.any_descendant_by_tags(sibling, &[Tag::Img])
        });
    let hidden_images: SmallVec<[NodeId; 4]> = dom
        .descendants(hidden)
        .filter(|&node| dom.tag(node) == Some(Tag::Img))
        .collect();
    let Some(sibling) = sibling else {
        for image in hidden_images {
            dom.insert_before(hidden, image);
        }
        return;
    };
    let visible_images: SmallVec<[NodeId; 4]> = dom
        .descendants(sibling)
        .filter(|&node| dom.tag(node) == Some(Tag::Img))
        .collect();
    let single_pair = hidden_images.len() == 1 && visible_images.len() == 1;
    for hidden_image in hidden_images {
        let target = visible_images.iter().copied().find(|&visible_image| {
            let hidden_alt = dom.attr_by_local_name(hidden_image, "alt");
            hidden_alt.is_some() && hidden_alt == dom.attr_by_local_name(visible_image, "alt")
        });
        let target = target.or_else(|| single_pair.then_some(visible_images[0]));
        if let Some(target) = target {
            copy_image_attributes(dom, hidden_image, target);
        } else if useful_image(dom, hidden_image) {
            dom.append_child(sibling, hidden_image);
        }
    }
}

pub(crate) fn hard_cleanup(
    dom: &mut Dom,
    root: NodeId,
    allowed_media: &Regex,
    relax_static_visibility: bool,
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
        let accessible_skip_link = tag == Tag::A
            && dom
                .attr(node, AttrName::Href)
                .is_some_and(|href| href.starts_with('#'))
            && dom.attr(node, AttrName::Class).is_some_and(|classes| {
                classes.split_ascii_whitespace().any(|class| {
                    class.eq_ignore_ascii_case("skip-link")
                        || class.to_ascii_lowercase().starts_with("skip-to-")
                })
            });
        let utility_visibility = has_hidden_utility_class(dom, node) && !accessible_skip_link;
        let static_visibility = has_static_hidden_marker(dom, node) || utility_visibility;
        let modal = dom.attr(node, AttrName::AriaModal) == Some("true")
            || dom.attr(node, AttrName::Role).is_some_and(|roles| {
                roles.split_whitespace().any(|role| {
                    role.eq_ignore_ascii_case("dialog") || role.eq_ignore_ascii_case("alertdialog")
                })
            })
            || static_visibility
                && dom.attr(node, AttrName::Class).is_some_and(|classes| {
                    classes.split_whitespace().any(|class| {
                        class.eq_ignore_ascii_case("modal") || class.eq_ignore_ascii_case("dialog")
                    })
                });
        let hidden = dom.attr(node, AttrName::AriaHidden) == Some("true") && !fallback_image
            || !relax_static_visibility && static_visibility
            || modal;
        if relax_static_visibility {
            dom.remove_attr(node, AttrName::Hidden);
            if let Some(classes) = dom.attr(node, AttrName::Class) {
                let retained = classes
                    .split_whitespace()
                    .filter(|class| !is_hidden_utility_class(class))
                    .collect::<SmallVec<[&str; 8]>>()
                    .join(" ");
                if retained.is_empty() {
                    dom.remove_attr(node, AttrName::Class);
                } else if retained != classes {
                    dom.set_attr(node, AttrName::Class, &retained);
                }
            }
        }
        let tracking_image = tag == Tag::Img
            && is_tracking_image(dom, node)
            && !has_lazy_image_candidate(dom, node)
            && !picture_has_lazy_source(dom, node);
        let executable = matches!(
            tag,
            Tag::Script | Tag::Style | Tag::Template | Tag::Link | Tag::Meta
        );
        let content_checkbox = tag == Tag::Input
            && dom
                .attr(node, AttrName::Type)
                .is_some_and(|value| value.eq_ignore_ascii_case("checkbox"))
            && dom
                .ancestors(node)
                .find(|&ancestor| {
                    matches!(dom.tag(ancestor), Some(Tag::Form | Tag::Li))
                        || dom.attr(ancestor, AttrName::Role).is_some_and(|roles| {
                            roles
                                .split_ascii_whitespace()
                                .any(|role| role.eq_ignore_ascii_case("listitem"))
                        })
                })
                .is_some_and(|ancestor| {
                    dom.tag(ancestor) == Some(Tag::Li)
                        || dom.attr(ancestor, AttrName::Role).is_some_and(|roles| {
                            roles
                                .split_ascii_whitespace()
                                .any(|role| role.eq_ignore_ascii_case("listitem"))
                        })
                });
        if content_checkbox {
            // Keep only the semantic state. The retained control is disabled,
            // so extracted HTML cannot change the source checklist.
            dom.remove_attr(node, AttrName::Other);
            dom.remove_attrs(
                node,
                &[
                    AttrName::Class,
                    AttrName::Id,
                    AttrName::Name,
                    AttrName::Style,
                    AttrName::AriaHidden,
                ],
            );
            dom.set_attr(node, AttrName::Disabled, "");
        }
        let control = matches!(
            tag,
            Tag::Input | Tag::Textarea | Tag::Select | Tag::Button | Tag::Datalist | Tag::Option
        ) && !content_checkbox;
        let disallowed_embed = matches!(tag, Tag::Object | Tag::Embed | Tag::Iframe)
            && !has_allowed_media(dom, node, allowed_media);
        if hidden || tracking_image || executable || control || disallowed_embed {
            if hidden && utility_visibility {
                preserve_media_from_hidden_variant(dom, node);
            }
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
    let snapshot = dom.element_descendants_snapshot_with_depth(root);
    store.clear_stats();
    store.enable_link_lengths();
    get_or_compute_stats(dom, root, store);

    // Count links once. Discovery uses the capped index instead of rescanning
    // each related-content subtree for links.
    let mut link_counts = vec![0_u8; dom.len()];
    for &(node, _) in &snapshot {
        link_counts[node.index()] = u8::from(dom.tag(node) == Some(Tag::A));
    }
    for &(node, _) in snapshot.iter().rev() {
        if let Some(parent) = dom.parent(node) {
            link_counts[parent.index()] = link_counts[parent.index()]
                .saturating_add(link_counts[node.index()])
                .min(3);
        }
    }

    let mut discovered_boundaries = vec![false; dom.len()];
    let mut inspected_subscription = vec![false; dom.len()];
    let mut table_depths = SmallVec::<[u32; 8]>::new();
    for &(node, depth) in &snapshot {
        while table_depths
            .last()
            .is_some_and(|&table_depth| table_depth >= depth)
        {
            table_depths.pop();
        }
        let inside_table = !table_depths.is_empty();
        if dom.tag(node) == Some(Tag::Table) {
            table_depths.push(depth);
        }
        if related_heading_signal(dom, node) != RelatedHeadingSignal::None {
            mark_related_heading_boundary(
                dom,
                node,
                root,
                &link_counts,
                store,
                &mut discovered_boundaries,
            );
        }
        if dom.tag(node) == Some(Tag::Form) {
            mark_subscription_boundary(
                dom,
                node,
                root,
                store,
                &mut inspected_subscription,
                &mut discovered_boundaries,
            );
        }
        if is_structural_breadcrumb_candidate(dom, node, inside_table) {
            discovered_boundaries[node.index()] = true;
        }
    }

    mark_data_tables(dom, root, store, &mut Vec::new());
    if remove_direct_peripheral_siblings(dom, root, &snapshot, &link_counts, store) {
        store.clear_stats();
    }
    let root_length = get_or_compute_stats(dom, root, store).text_length.max(1);

    // Keep only outermost candidates. A classifier can inspect the complete
    // subtree once instead of rescanning every nested wrapper.
    let mut boundary_depth = None;
    for (node, depth) in snapshot {
        if let Some(outer_depth) = boundary_depth {
            if depth > outer_depth && !discovered_boundaries[node.index()] {
                continue;
            }
            if depth <= outer_depth {
                boundary_depth = None;
            }
        }
        if dom.parent(node).is_some() {
            if discovered_boundaries[node.index()] {
                nodes.push(node);
                continue;
            }
            if is_heuristic_boundary(dom, node) {
                nodes.push(node);
                boundary_depth = Some(depth);
            }
        }
    }
    for &node in nodes.iter().rev() {
        if dom.parent(node).is_none()
            || is_protected_content(dom, node, store)
            || has_protected_ancestor(dom, node, root, store)
        {
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

        // Empty boundaries cannot contain useful positional clutter evidence.
        // Skipping them also avoids long sibling scans on empty form shells.
        let (at_start, at_end) = if stats.text_length == 0 {
            (false, false)
        } else {
            (
                near_content_start(dom, node, root, store),
                near_content_end(dom, node, root, store),
            )
        };
        let has_form = dom.tag(node) == Some(Tag::Form)
            || dom
                .descendants(node)
                .any(|descendant| dom.tag(descendant) == Some(Tag::Form));
        let metrics = PeripheralMetrics {
            name: &name,
            text: &text,
            stats,
            links,
            controls,
            has_form,
            link_density,
            at_start,
            at_end,
            short,
        };
        let related = is_related_content(dom, node, &metrics);

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

        let signup = is_newsletter_cta(&metrics);

        let breadcrumb = is_breadcrumb(dom, node, &metrics);
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
            && !breadcrumb
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
        let comment_ui = name.contains("comment")
            && stats.text_length < 180
            && (text.starts_with("comments")
                && text.contains("login")
                && text.contains("0 comments")
                || text.starts_with("login") && text.contains("0 comments")
                || text.starts_with("share: 0 comments")
                || text == "0 comments subscribe rss");

        let action_label = [
            "leave a comment",
            "share",
            "reply",
            "rate this",
            "answer this",
        ]
        .iter()
        .any(|label| text.trim() == *label || text.contains(&format!("{label} ")));
        let action_url = dom.descendants(node).any(|descendant| {
            dom.tag(descendant) == Some(Tag::A)
                && dom.attr(descendant, AttrName::Href).is_some_and(|href| {
                    let href = href.to_ascii_lowercase();
                    href.contains("/comments")
                        || href.contains("action=share")
                        || href.contains("/reply")
                        || href.contains("dialog=")
                })
        });
        let interaction_name = contains_any(
            &name,
            &[
                "toolbar",
                "article-actions",
                "post-actions",
                "feedback",
                "share",
            ],
        );
        let interaction_signals =
            usize::from(action_label) + usize::from(action_url) + usize::from(interaction_name);
        let terminal_action = links > 0
            && stats.text_length < 160
            && link_density >= 0.55
            && interaction_signals >= 2
            && near_content_end(dom, node, root, store);

        let taxonomy_name = name
            .split(|character: char| !character.is_ascii_alphanumeric())
            .any(|token| {
                matches!(
                    token,
                    "taxonomy" | "tags" | "entities" | "entitylist" | "taglist"
                )
            })
            || contains_any(
                &name,
                &[
                    "company-portals",
                    "company_portals",
                    "entity-list",
                    "entity_list",
                    "tag-list",
                    "tag_list",
                ],
            );
        let terminal_taxonomy = taxonomy_name
            && links >= 2
            && stats.text_length < 300
            && link_density >= 0.45
            && near_content_end(dom, node, root, store);
        let peripheral_panel_name = name
            .split(|character: char| !character.is_ascii_alphanumeric())
            .any(|token| matches!(token, "sidebar" | "comments" | "commentlist"));
        let terminal_peripheral_panel = peripheral_panel_name
            && links >= 3
            && short
            && link_density >= 0.2
            && (at_end || text.starts_with("comments") && text.contains("subscribe"));
        let print_citation = links >= 2
            && short
            && contains_any(&name, &["print-citation", "story-footer"])
            && text.contains("appears in print");

        if related
            || social
            || signup
            || breadcrumb
            || navigation
            || author
            || advertisement
            || consent
            || account
            || comment_ui
            || terminal_action
            || terminal_taxonomy
            || terminal_peripheral_panel
            || print_citation
        {
            if protected {
                hoist_protected_children(dom, node, store);
            }
            detach_and_invalidate_stats(dom, node, store);
        }
    }

    remove_contextual_boilerplate(dom, root, store, text_buffer, nodes);
}

fn remove_direct_peripheral_siblings(
    dom: &mut Dom,
    root: NodeId,
    snapshot: &[(NodeId, u32)],
    link_counts: &[u8],
    store: &mut crate::dom::NodeStateStore,
) -> bool {
    let mut seen = vec![false; dom.len()];
    seen[root.index()] = true;
    let mut parents = vec![root];
    for &(node, _) in snapshot {
        if (related_heading_signal(dom, node) == RelatedHeadingSignal::Strong
            || dom.tag(node) == Some(Tag::Form))
            && let Some(parent) = dom.parent(node)
            && !std::mem::replace(&mut seen[parent.index()], true)
        {
            parents.push(parent);
        }
    }

    let mut changed = false;
    for parent in parents {
        if dom.parent(parent).is_none() && parent != root {
            continue;
        }
        if is_protected_content(dom, parent, store)
            || has_protected_ancestor(dom, parent, root, store)
        {
            continue;
        }
        changed |= remove_direct_peripheral_children(dom, parent, link_counts, store);
    }
    changed
}

fn remove_direct_peripheral_children(
    dom: &mut Dom,
    parent: NodeId,
    link_counts: &[u8],
    store: &mut crate::dom::NodeStateStore,
) -> bool {
    let parent_name = node_name(dom, parent);
    let children: Vec<_> = dom.element_children(parent).collect();
    let mut remove = vec![false; children.len()];

    for (index, &child) in children.iter().enumerate() {
        if related_heading_signal(dom, child) != RelatedHeadingSignal::Strong
            || related_name_signal(&parent_name)
        {
            continue;
        }
        let mut links = 0_u8;
        let mut end = index;
        for (offset, &sibling) in children[index + 1..].iter().take(12).enumerate() {
            if matches!(
                dom.tag(sibling),
                Some(Tag::H1 | Tag::H2 | Tag::H3 | Tag::H4 | Tag::H5 | Tag::H6)
            ) {
                break;
            }
            let stats = get_or_compute_stats(dom, sibling, store);
            let sibling_links = link_counts[sibling.index()];
            if is_protected_content(dom, sibling, store)
                || sibling_links == 0 && stats.has_non_whitespace
            {
                break;
            }
            links = links.saturating_add(sibling_links).min(3);
            end = index + offset + 1;
        }
        if links >= 2 {
            remove[index..=end].fill(true);
        }
    }

    for (index, &child) in children.iter().enumerate() {
        if dom.tag(child) != Some(Tag::Form) || has_explicit_newsletter_name(&parent_name) {
            continue;
        }
        let mut start = index;
        let mut text = String::new();
        for &previous in children[..index].iter().rev().take(3) {
            let tag = dom.tag(previous);
            if !matches!(
                tag,
                Some(Tag::H2 | Tag::H3 | Tag::H4 | Tag::H5 | Tag::H6 | Tag::P)
            ) {
                break;
            }
            start -= 1;
            if matches!(tag, Some(Tag::H2 | Tag::H3 | Tag::H4 | Tag::H5 | Tag::H6)) {
                break;
            }
        }
        if start == index {
            continue;
        }
        for &sibling in &children[start..=index] {
            dom.append_normalized_text(sibling, &mut text);
            text.push(' ');
        }
        let name = node_name(dom, child);
        if has_newsletter_evidence(&name, &text.to_ascii_lowercase()) {
            remove[start..=index].fill(true);
        }
    }

    let mut changed = false;
    for (&node, remove) in children.iter().zip(remove) {
        if remove && dom.parent(node).is_some() {
            if is_protected_content(dom, node, store) {
                continue;
            }
            let protected = dom
                .descendants(node)
                .any(|descendant| is_protected_content(dom, descendant, store));
            if protected {
                hoist_protected_children(dom, node, store);
            }
            detach_and_invalidate_stats(dom, node, store);
            changed = true;
        }
    }
    changed
}

#[derive(Clone, Copy, Eq, PartialEq, Ord, PartialOrd)]
enum RelatedHeadingSignal {
    None,
    Ambiguous,
    Strong,
}

struct PeripheralMetrics<'a> {
    name: &'a str,
    text: &'a str,
    stats: NodeStats,
    links: usize,
    controls: usize,
    has_form: bool,
    link_density: f64,
    at_start: bool,
    at_end: bool,
    short: bool,
}

fn related_heading_signal(dom: &Dom, node: NodeId) -> RelatedHeadingSignal {
    if !matches!(
        dom.tag(node),
        Some(Tag::H2 | Tag::H3 | Tag::H4 | Tag::H5 | Tag::H6)
    ) {
        return RelatedHeadingSignal::None;
    }
    let mut text = String::new();
    dom.append_normalized_text(node, &mut text);
    let text = text.trim().to_ascii_lowercase();
    if matches!(
        text.as_str(),
        "related articles"
            | "related content"
            | "related posts"
            | "related stories"
            | "recommended"
            | "recommended reading"
            | "read next"
            | "more stories"
            | "more articles"
            | "more posts"
            | "you may also like"
            | "you might also like"
    ) || text
        .strip_prefix("more from ")
        .is_some_and(|suffix| !suffix.is_empty() && suffix.split_whitespace().count() <= 6)
    {
        RelatedHeadingSignal::Strong
    } else if matches!(
        text.as_str(),
        "related" | "further reading" | "see also" | "read more"
    ) {
        RelatedHeadingSignal::Ambiguous
    } else {
        // In particular, keep academic sections such as "Related Work".
        RelatedHeadingSignal::None
    }
}

fn mark_related_heading_boundary(
    dom: &Dom,
    heading: NodeId,
    root: NodeId,
    link_counts: &[u8],
    store: &crate::dom::NodeStateStore,
    boundaries: &mut [bool],
) {
    for candidate in dom
        .ancestors(heading)
        .take_while(|&node| node != root)
        .take(8)
    {
        if !matches!(
            dom.tag(candidate),
            Some(Tag::Aside | Tag::Div | Tag::Footer | Tag::Section)
        ) {
            continue;
        }
        // A heading is discovery evidence only. Keep long reference sections
        // out of the candidate set before the detailed classifier runs.
        if link_counts[candidate.index()] >= 2
            && store
                .get_stats(candidate)
                .is_some_and(|stats| stats.text_length < 1_200)
        {
            boundaries[candidate.index()] = true;
            break;
        }
    }
}

fn append_bounded_text(dom: &Dom, root: NodeId, node_limit: usize, output: &mut String) {
    for node in std::iter::once(root)
        .chain(dom.descendants(root))
        .take(node_limit)
    {
        let Some(text) = dom
            .text_node(node)
            .map(str::trim)
            .filter(|text| !text.is_empty())
        else {
            continue;
        };
        if !output.is_empty() {
            output.push(' ');
        }
        output.push_str(text);
    }
}

fn mark_subscription_boundary(
    dom: &Dom,
    form: NodeId,
    root: NodeId,
    store: &crate::dom::NodeStateStore,
    inspected: &mut [bool],
    boundaries: &mut [bool],
) {
    for candidate in dom.ancestors(form).take_while(|&node| node != root).take(4) {
        if !matches!(
            dom.tag(candidate),
            Some(Tag::Aside | Tag::Div | Tag::Footer | Tag::Section)
        ) {
            continue;
        }
        if std::mem::replace(&mut inspected[candidate.index()], true) {
            if boundaries[candidate.index()] {
                return;
            }
            continue;
        }
        if store
            .get_stats(candidate)
            .is_none_or(|stats| stats.text_length >= 800)
        {
            continue;
        }
        let name = node_name(dom, candidate);
        let mut text = String::new();
        append_bounded_text(dom, candidate, 128, &mut text);
        let text = text.to_ascii_lowercase();
        if !has_newsletter_evidence(&name, &text) {
            continue;
        }
        let has_direct_copy = dom.element_children(candidate).take(12).any(|child| {
            matches!(
                dom.tag(child),
                Some(Tag::H2 | Tag::H3 | Tag::H4 | Tag::H5 | Tag::H6 | Tag::P)
            ) && {
                let mut child_text = String::new();
                dom.append_normalized_text(child, &mut child_text);
                has_newsletter_cta_text(&child_text.to_ascii_lowercase())
            }
        });
        if has_explicit_newsletter_name(&name) || has_direct_copy {
            boundaries[candidate.index()] = true;
            return;
        }
    }
}

fn is_structural_breadcrumb_candidate(dom: &Dom, node: NodeId, inside_table: bool) -> bool {
    let semantic_navigation = dom.tag(node) == Some(Tag::Nav)
        || dom.attr(node, AttrName::Role).is_some_and(|roles| {
            roles
                .split_whitespace()
                .any(|role| role.eq_ignore_ascii_case("navigation"))
        });
    if dom.attr(node, AttrName::AriaLabel).is_some_and(|label| {
        matches!(
            label.trim().to_ascii_lowercase().as_str(),
            "breadcrumb" | "breadcrumbs"
        )
    }) {
        return true;
    }
    if !matches!(dom.tag(node), Some(Tag::Div | Tag::Nav | Tag::P)) {
        return false;
    }
    if inside_table {
        return false;
    }
    if dom.element_children(node).any(|child| {
        matches!(
            dom.tag(child),
            Some(Tag::Article | Tag::Div | Tag::P | Tag::Section)
        )
    }) {
        return false;
    }

    // Inspect only a small, shallow prefix. This finds ordinary unlabelled
    // trails without turning candidate discovery into nested subtree scans.
    let mut stack = SmallVec::<[(NodeId, u8); 24]>::new();
    stack.extend(dom.children(node).map(|child| (child, 0)));
    let mut visited = 0usize;
    let mut links = 0usize;
    let mut separator = 0usize;
    while let Some((current, depth)) = stack.pop() {
        visited += 1;
        if visited > 32 {
            return false;
        }
        links += usize::from(dom.tag(current) == Some(Tag::A));
        separator =
            separator.saturating_add(dom.text_node(current).map_or(0, breadcrumb_separator_count));
        if depth < 2 {
            stack.extend(dom.children(current).map(|child| (child, depth + 1)));
        }
    }
    links >= 2 && separator >= 2 && semantic_navigation
}

fn has_breadcrumb_name(dom: &Dom, node: NodeId, name: &str) -> bool {
    name.split(|character: char| !character.is_ascii_alphanumeric())
        .any(|token| matches!(token, "breadcrumb" | "breadcrumbs"))
        || dom.attr(node, AttrName::AriaLabel).is_some_and(|label| {
            matches!(
                label.trim().to_ascii_lowercase().as_str(),
                "breadcrumb" | "breadcrumbs"
            )
        })
}

fn breadcrumb_separator_count(text: &str) -> usize {
    text.chars()
        .filter(|&character| matches!(character, '>' | '/' | '›' | '»'))
        .take(3)
        .count()
}

fn is_breadcrumb(dom: &Dom, node: NodeId, metrics: &PeripheralMetrics<'_>) -> bool {
    if !metrics.at_start
        || metrics.links < 2
        || metrics.stats.text_length > 280
        || metrics.stats.sentence_end_count > 1
    {
        return false;
    }
    let all_fragment_links = dom
        .descendants(node)
        .filter(|&descendant| dom.tag(descendant) == Some(Tag::A))
        .all(|link| {
            dom.attr(link, AttrName::Href)
                .is_some_and(|href| href.starts_with('#'))
        });
    if all_fragment_links {
        return false;
    }

    let explicit = has_breadcrumb_name(dom, node, metrics.name);
    let navigation = dom.tag(node) == Some(Tag::Nav)
        || dom.attr(node, AttrName::Role).is_some_and(|roles| {
            roles
                .split_whitespace()
                .any(|role| role.eq_ignore_ascii_case("navigation"))
        });
    let separator = breadcrumb_separator_count(metrics.text) >= 2;
    let linked_list_items = dom
        .descendants(node)
        .filter(|&descendant| dom.tag(descendant) == Some(Tag::Li))
        .filter(|&item| {
            dom.descendants(item)
                .any(|descendant| dom.tag(descendant) == Some(Tag::A))
        })
        .take(2)
        .count();
    let list_shape = linked_list_items >= 2;
    let compact_links = metrics.stats.text_length <= (metrics.links as u32).saturating_mul(70);

    compact_links
        && metrics.link_density >= if explicit { 0.25 } else { 0.4 }
        && (explicit || separator || navigation && list_shape)
}

fn has_explicit_newsletter_name(name: &str) -> bool {
    name.split(|character: char| !character.is_ascii_alphanumeric())
        .any(|token| token == "newsletter")
}

fn has_subscription_name(name: &str) -> bool {
    name.contains("sign-up")
        || name
            .split(|character: char| !character.is_ascii_alphanumeric())
            .any(|token| matches!(token, "subscribe" | "subscription" | "signup"))
}

fn has_newsletter_evidence(name: &str, text: &str) -> bool {
    has_explicit_newsletter_name(name)
        || text.contains("newsletter")
        || has_subscription_name(name) && text.contains("email")
        || has_newsletter_cta_text(text)
            && (text.contains("inbox")
                || text.trim_start().starts_with("subscribe")
                || text.trim_start().starts_with("join"))
}

fn has_newsletter_cta_text(text: &str) -> bool {
    let text = text.trim();
    starts_with_any(
        text,
        &[
            "subscribe",
            "sign up",
            "join our newsletter",
            "join the newsletter",
            "get updates",
            "stay informed",
            "enter your email",
        ],
    ) || contains_any(
        text,
        &[
            "subscribe to our newsletter",
            "sign up for our newsletter",
            "get updates in your inbox",
            "enter your email",
        ],
    )
}

fn is_newsletter_cta(metrics: &PeripheralMetrics<'_>) -> bool {
    let action = metrics.has_form || metrics.controls > 0 || metrics.links > 0;
    let explicit_newsletter =
        has_explicit_newsletter_name(metrics.name) || metrics.text.contains("newsletter");
    let boundary = metrics.at_start || metrics.at_end || explicit_newsletter;
    let short_copy = metrics.stats.text_length < 800
        && metrics.stats.word_count <= 120
        && metrics.stats.sentence_end_count <= 8
        && metrics.short;
    let explicit_subscription_cta = has_subscription_name(metrics.name)
        && metrics.links > 0
        && metrics.text.trim_start().starts_with("subscribe");
    boundary
        && action
        && short_copy
        && (has_newsletter_evidence(metrics.name, metrics.text) || explicit_subscription_cta)
}

fn related_name_signal(name: &str) -> bool {
    contains_any(
        name,
        &[
            "related",
            "recommend",
            "more-stories",
            "more_stories",
            "read-next",
        ],
    )
}

fn related_text_signal(text: &str) -> bool {
    starts_with_any(
        text,
        &[
            "related",
            "recommended",
            "more stories",
            "you may also like",
        ],
    )
}

fn related_heading_signal_in(dom: &Dom, node: NodeId) -> RelatedHeadingSignal {
    dom.descendants(node)
        .map(|descendant| related_heading_signal(dom, descendant))
        .max()
        .unwrap_or(RelatedHeadingSignal::None)
}

fn has_academic_related_heading(dom: &Dom, node: NodeId) -> bool {
    dom.descendants(node).any(|descendant| {
        matches!(
            dom.tag(descendant),
            Some(Tag::H2 | Tag::H3 | Tag::H4 | Tag::H5 | Tag::H6)
        ) && {
            let mut text = String::new();
            dom.append_normalized_text(descendant, &mut text);
            text.trim().eq_ignore_ascii_case("related work")
        }
    })
}

fn linked_short_child_count(dom: &Dom, parent: NodeId) -> usize {
    dom.element_children(parent)
        .filter(|&child| {
            !matches!(
                dom.tag(child),
                Some(Tag::H1 | Tag::H2 | Tag::H3 | Tag::H4 | Tag::H5 | Tag::H6)
            ) && dom.normalized_char_count(child) <= 240
                && (dom.tag(child) == Some(Tag::A)
                    || dom
                        .descendants(child)
                        .any(|descendant| dom.tag(descendant) == Some(Tag::A)))
        })
        .take(3)
        .count()
}

fn has_repeated_link_cards(dom: &Dom, node: NodeId) -> bool {
    if linked_short_child_count(dom, node) >= 2 {
        return true;
    }
    dom.element_children(node).any(|container| {
        matches!(
            dom.tag(container),
            Some(Tag::Div | Tag::Ol | Tag::Section | Tag::Ul)
        ) && linked_short_child_count(dom, container) >= 2
    })
}

fn is_related_content(dom: &Dom, node: NodeId, metrics: &PeripheralMetrics<'_>) -> bool {
    if metrics.links < 2 || has_academic_related_heading(dom, node) {
        return false;
    }
    let heading = related_heading_signal_in(dom, node);
    let named = related_name_signal(metrics.name)
        || dom
            .descendants(node)
            .any(|descendant| related_name_signal(&node_name(dom, descendant)));
    if heading == RelatedHeadingSignal::None && !named && !related_text_signal(metrics.text) {
        return false;
    }
    // Preserve the established cleanup behavior for short, explicitly named
    // related blocks. Academic "Related Work" sections remain excluded.
    if metrics.short && metrics.link_density >= 0.2 {
        return true;
    }
    if metrics.stats.text_length >= 1_200 {
        return false;
    }
    let repeated_cards = has_repeated_link_cards(dom, node);
    let non_link_chars = f64::from(metrics.stats.text_length) * (1.0 - metrics.link_density);
    let sparse_text = non_link_chars <= 320.0 && metrics.stats.sentence_end_count <= 7;

    if !sparse_text {
        return false;
    }

    if metrics.at_end {
        return match heading {
            RelatedHeadingSignal::Strong => repeated_cards || metrics.link_density >= 0.45,
            RelatedHeadingSignal::Ambiguous => repeated_cards && metrics.link_density >= 0.45,
            RelatedHeadingSignal::None => named && repeated_cards && metrics.link_density >= 0.3,
        };
    }
    if metrics.at_start {
        return named && repeated_cards && metrics.link_density >= 0.2;
    }

    // Mid-article removal needs every strong signal. This keeps ordinary link
    // sections while removing a clear card interruption between prose blocks.
    heading == RelatedHeadingSignal::Strong
        && repeated_cards
        && metrics.link_density >= 0.35
        && metrics.stats.text_length < 700
        && non_link_chars <= 240.0
}

/// Removes short textual controls that do not always have useful class names.
///
/// Phrase matches are deliberately narrow. A match also needs document-boundary
/// or control evidence, except for labels that are complete conventional UI
/// phrases. This keeps the same words when they occur in normal prose.
fn remove_contextual_boilerplate(
    dom: &mut Dom,
    root: NodeId,
    store: &mut crate::dom::NodeStateStore,
    text_buffer: &mut String,
    nodes: &mut Vec<NodeId>,
) {
    let snapshot = dom.element_descendants_snapshot_with_depth(root);
    let mut has_nested_boundary = vec![false; dom.len()];
    for &(node, _) in snapshot.iter().rev() {
        if (is_contextual_text_boundary(dom, node) || has_nested_boundary[node.index()])
            && let Some(parent) = dom.parent(node)
        {
            has_nested_boundary[parent.index()] = true;
        }
    }
    nodes.clear();
    nodes.extend(snapshot.into_iter().map(|(node, _)| node).filter(|&node| {
        is_contextual_text_boundary(dom, node) && !has_nested_boundary[node.index()]
    }));

    // Populate every current subtree statistic in one bottom-up traversal.
    // The earlier structural pass can detach nodes, so do not reuse its cache.
    store.clear_stats();
    get_or_compute_stats(dom, root, store);

    for &node in nodes.iter().rev() {
        if dom.parent(node).is_none() || is_protected_content(dom, node, store) {
            continue;
        }
        if store
            .get_stats(node)
            .is_none_or(|stats| stats.text_length > 140)
        {
            continue;
        }
        let text = get_inner_text(dom, node, text_buffer);
        let text = text.trim().to_ascii_lowercase();
        if text.is_empty() {
            continue;
        }
        let name = node_name(dom, node);
        let link_or_control = dom.tag(node) == Some(Tag::Form)
            || dom.descendants(node).any(|descendant| {
                matches!(
                    dom.tag(descendant),
                    Some(Tag::A | Tag::Button | Tag::Input | Tag::Select | Tag::Textarea)
                )
            });
        let at_start = near_content_start(dom, node, root, store);
        let at_end = near_content_end(dom, node, root, store);

        let reading_time = is_reading_time_label(&text)
            && (at_start
                || contains_any(
                    &name,
                    &[
                        "read-time",
                        "read_time",
                        "reading-time",
                        "metadata",
                        "byline",
                    ],
                ));
        let advertisement = matches!(
            text.as_str(),
            "advertisement" | "advertisement continues below" | "sponsored" | "sponsored content"
        ) && (at_start || at_end || strong_ad_name(&name));
        let action = matches!(
            text.as_str(),
            "share"
                | "share this"
                | "share this article"
                | "share this story"
                | "read more"
                | "leave a comment"
        ) && link_or_control
            && (at_end || contains_any(&name, &["share", "action", "button", "toolbar"]));
        let subscription = (text.starts_with("sign up for our newsletter")
            || text.starts_with("subscribe to our newsletter")
            || text.starts_with("subscribe for updates"))
            && link_or_control
            && (at_start
                || at_end
                || contains_any(&name, &["newsletter", "subscribe", "signup", "sign-up"]));

        if reading_time || advertisement || action || subscription {
            detach_and_invalidate_stats(dom, node, store);
        }
    }
}

fn is_contextual_text_boundary(dom: &Dom, node: NodeId) -> bool {
    matches!(
        dom.tag(node),
        Some(Tag::Aside | Tag::Div | Tag::Footer | Tag::P | Tag::Section | Tag::Small)
    )
}

fn is_reading_time_label(text: &str) -> bool {
    let text = text
        .strip_prefix("reading time:")
        .map(str::trim)
        .unwrap_or(text);
    let mut words = text.split_ascii_whitespace();
    let Some(amount) = words.next() else {
        return false;
    };
    let Some(unit) = words.next() else {
        return false;
    };
    let Some(read) = words.next() else {
        return false;
    };
    amount.bytes().all(|byte| byte.is_ascii_digit())
        && matches!(unit, "min" | "mins" | "minute" | "minutes")
        && read == "read"
        && words.next().is_none()
}

fn hoist_protected_children(
    dom: &mut Dom,
    wrapper: NodeId,
    store: &mut crate::dom::NodeStateStore,
) {
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
        invalidate_stats_for_ancestors(dom, node, store);
        dom.insert_before(wrapper, node);
    }
}

fn detach_and_invalidate_stats(
    dom: &mut Dom,
    node: NodeId,
    store: &mut crate::dom::NodeStateStore,
) {
    invalidate_stats_for_ancestors(dom, node, store);
    dom.detach(node);
}

fn invalidate_stats_for_ancestors(dom: &Dom, node: NodeId, store: &mut crate::dom::NodeStateStore) {
    for ancestor in dom.ancestors(node) {
        store.invalidate_stats(ancestor);
        if store.link_lengths_enabled() {
            store.set_link_length(ancestor, 0.0);
        }
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
        Some(Tag::Div | Tag::Ol | Tag::P | Tag::Section | Tag::Ul)
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
            "toolbar",
            "actions",
            "feedback",
            "comment",
            "button-wrapper",
            "taxonomy",
            "company-portals",
            "entity-list",
            "entity_list",
            "tag-list",
            "tag_list",
        ],
    )
}

fn near_content_end(
    dom: &Dom,
    node: NodeId,
    root: NodeId,
    store: &mut crate::dom::NodeStateStore,
) -> bool {
    let mut current = node;
    let mut trailing_chars = 0_usize;
    loop {
        let mut sibling = dom.next_sibling(current);
        while let Some(next) = sibling {
            // Reuse the cached normalized character count. The previous code
            // rebuilt every following subtree for each heuristic boundary.
            trailing_chars = trailing_chars.saturating_add(
                crate::scoring::get_or_compute_stats(dom, next, store).text_length as usize,
            );
            if trailing_chars > 100 {
                return false;
            }
            sibling = dom.next_sibling(next);
        }
        if current == root {
            return true;
        }
        let Some(parent) = dom.parent(current) else {
            return true;
        };
        current = parent;
    }
}

fn near_content_start(
    dom: &Dom,
    node: NodeId,
    root: NodeId,
    store: &mut crate::dom::NodeStateStore,
) -> bool {
    let mut current = node;
    let mut leading_chars = 0_usize;
    loop {
        let mut sibling = dom.prev_sibling(current);
        while let Some(previous) = sibling {
            leading_chars = leading_chars.saturating_add(
                crate::scoring::get_or_compute_stats(dom, previous, store).text_length as usize,
            );
            if leading_chars > 100 {
                return false;
            }
            sibling = dom.prev_sibling(previous);
        }
        if current == root {
            return true;
        }
        let Some(parent) = dom.parent(current) else {
            return true;
        };
        current = parent;
    }
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
    fn preserves_code_line_breaks_during_document_preparation() {
        let mut dom = Dom::parse_document(
            "<body><pre><code>one<br><br>two</code></pre><code>three<br><br>four</code></body>",
        )
        .unwrap();

        prep_document(&mut dom);

        let body = dom.body().unwrap();
        assert_eq!(
            dom.descendants(body)
                .filter(|&node| dom.tag(node) == Some(Tag::Br))
                .count(),
            4
        );
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
        hard_cleanup(&mut dom, root, &allowed, false, &mut nodes);
        heuristic_cleanup(&mut dom, root, &mut store, &mut text, &mut nodes);
        dom.text(root)
    }

    #[test]
    fn extreme_table_spans_do_not_overflow() {
        let dom = Dom::parse_fragment(
            r#"<table><tr><td colspan="4294967295">A</td><td colspan="4294967295">B</td></tr><tr><td colspan="4294967295">C</td><td colspan="4294967295">D</td></tr></table>"#,
            Tag::Div,
        )
        .unwrap();
        let mut store = NodeStateStore::new();
        let mut tables = Vec::new();
        mark_data_tables(&dom, dom.root(), &mut store, &mut tables);
        let table = dom.first_descendant_by_tag(dom.root(), Tag::Table).unwrap();
        assert_eq!(store.is_data_table(table), Some(true));
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
    fn hard_cleanup_preserves_only_content_checkboxes() {
        let mut dom = Dom::parse_fragment(
            r#"<ul><li><label><input class="control" onclick="bad()" type="checkbox" checked> Done</label></li><li><form><input type="checkbox"> Option</form></li></ul><form><input type="checkbox"><button>Search</button></form>"#,
            Tag::Div,
        )
        .unwrap();
        let root = dom.root();
        hard_cleanup(
            &mut dom,
            root,
            &Regex::new("$").unwrap(),
            false,
            &mut Vec::new(),
        );
        let inputs: Vec<_> = dom
            .descendants(root)
            .filter(|&node| dom.tag(node) == Some(Tag::Input))
            .collect();
        assert_eq!(inputs.len(), 1);
        assert!(dom.has_attr(inputs[0], AttrName::Checked));
        assert!(dom.has_attr(inputs[0], AttrName::Disabled));
        assert_eq!(dom.attr(inputs[0], AttrName::Class), None);
        assert_eq!(dom.attr_by_local_name(inputs[0], "onclick"), None);
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
    fn heuristic_cleanup_removes_terminal_action_paragraphs() {
        let text = clean_fragment(
            r#"<article><p>This substantive article paragraph explains the complete result and gives useful context to readers.</p><p class="button-wrapper"><a href="/story/comments">Leave a comment</a></p><p class="button-wrapper"><a href="/story?action=share">Share</a></p></article>"#,
        );
        assert!(text.contains("substantive article"), "{text}");
        assert!(!text.contains("Leave a comment"), "{text}");
        assert!(!text.contains("Share"), "{text}");
    }

    #[test]
    fn heuristic_cleanup_removes_terminal_taxonomy_name_variants() {
        for class in ["entity-list", "entity_list", "tag-list", "tag_list"] {
            let html = format!(
                r#"<article><p>This substantive article paragraph explains the complete result and gives useful context to readers.</p><div class="{class}"><a href="/a">Alpha</a><a href="/b">Beta</a></div></article>"#
            );
            let text = clean_fragment(&html);
            assert!(text.contains("substantive article"), "{class}: {text}");
            assert!(!text.contains("Alpha"), "{class}: {text}");
        }
    }

    #[test]
    fn heuristic_cleanup_removes_contextual_text_boilerplate() {
        let text = clean_fragment(
            r#"<article><p class="reading-time">5 min read</p><p>This substantive article paragraph explains the complete result and gives useful context to readers.</p><p><a href="/more">Read more</a></p><p>Advertisement</p></article>"#,
        );
        assert!(text.contains("substantive article"), "{text}");
        for clutter in ["5 min read", "Read more", "Advertisement"] {
            assert!(!text.contains(clutter), "retained {clutter}: {text}");
        }
    }

    #[test]
    fn heuristic_cleanup_keeps_boilerplate_words_in_prose() {
        let text = clean_fragment(
            r#"<article><p>The advertisement changed television forever.</p><p>This guide takes five minutes to read more carefully, and it explains why people share this article in class.</p></article>"#,
        );
        assert!(text.contains("advertisement changed television"), "{text}");
        assert!(text.contains("read more carefully"), "{text}");
        assert!(text.contains("share this article in class"), "{text}");
    }

    #[test]
    fn heuristic_cleanup_invalidates_stats_after_nested_removal() {
        let advertisements = "<p>Advertisement</p>".repeat(10);
        let html = format!(
            "<article><p>Useful article content.</p><p><a href=\"/more\">Read more</a></p><div>{advertisements}</div></article>"
        );
        let text = clean_fragment(&html);
        assert!(text.contains("Useful article content"), "{text}");
        assert!(!text.contains("Read more"), "{text}");
        assert!(!text.contains("Advertisement"), "{text}");
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
        hard_cleanup(
            &mut dom,
            root,
            &Regex::new("$").unwrap(),
            false,
            &mut Vec::new(),
        );
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
        hard_cleanup(
            &mut dom,
            root,
            &Regex::new("$").unwrap(),
            false,
            &mut Vec::new(),
        );
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
    fn heuristic_cleanup_indexes_many_forms_and_related_sections_once() {
        let mut html = String::from("<main><p>Retained documentation.</p>");
        for index in 0..2_000 {
            html.push_str(&format!(
                "<form></form><section><p>Get updates in your inbox.</p><form><label>Email<input></label></form></section><aside class=\"related-links\"><h2>Related</h2><a href=\"/{index}/a\">A</a><a href=\"/{index}/b\">B</a></aside>"
            ));
        }
        html.push_str("</main>");

        let text = clean_fragment(&html);

        assert_eq!(text, "Retained documentation.");
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
