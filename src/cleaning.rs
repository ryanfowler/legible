//! DOM preparation and article cleanup.
#![allow(clippy::collapsible_if)]
use crate::constants::{
    PRESENTATIONAL_ATTRIBUTES, flags::*, has_image_extension, has_image_src, has_image_srcset,
    is_deprecated_size_attribute_elem, parse_b64_data_url, regexps,
};
use crate::dom::{AttrName, Dom, NodeId, Tag};
use crate::scoring::{
    get_class_weight, get_inner_text, get_link_density_cached, get_or_compute_stats,
    has_single_tag_inside_element, is_element_without_content, is_phrasing_content,
};
use html5ever::{LocalName, QualName, ns};
use regex::Regex;
use smallvec::SmallVec;

pub fn prep_document(dom: &mut Dom) {
    // Preserve Readability's preparation order. Remove inactive subtrees,
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

pub fn clean_tags(
    dom: &mut Dom,
    root: NodeId,
    tags: &[Tag],
    allowed: &Regex,
    nodes: &mut Vec<NodeId>,
) {
    nodes.clear();
    nodes.extend(
        dom.descendants(root)
            .filter(|&id| dom.tag(id).is_some_and(|t| tags.contains(&t))),
    );
    for &id in nodes.iter() {
        if dom.parent(id).is_none() {
            continue;
        }
        let tag = dom.tag(id).unwrap();
        if matches!(tag, Tag::Object | Tag::Embed | Tag::Iframe) {
            if has_allowed_media(dom, id, allowed) {
                continue;
            }
        }
        dom.detach(id);
    }
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
pub fn clean_headers(dom: &mut Dom, root: NodeId, flags: u32, nodes: &mut Vec<NodeId>) {
    nodes.clear();
    nodes.extend(
        dom.descendants(root)
            .filter(|&id| matches!(dom.tag(id), Some(Tag::H1 | Tag::H2))),
    );
    for &id in nodes.iter() {
        if get_class_weight(dom, id, flags) < 0 {
            dom.detach(id);
        }
    }
}
#[allow(clippy::too_many_arguments)]
pub fn clean_conditionally(
    dom: &mut Dom,
    root: NodeId,
    tags: &[Tag],
    semantic: Tag,
    flags: u32,
    allowed: &Regex,
    store: &mut crate::dom::NodeStateStore,
    text_buffer: &mut String,
    nodes: &mut Vec<NodeId>,
    modifier: f64,
) {
    if flags & FLAG_CLEAN_CONDITIONALLY == 0 {
        return;
    }
    nodes.clear();
    nodes.extend(
        dom.descendants(root)
            .filter(|&id| dom.tag(id).is_some_and(|t| tags.contains(&t))),
    );
    store.clear_stats();
    for &id in nodes.iter().rev() {
        if dom.parent(id).is_some()
            && should_remove(
                dom,
                id,
                semantic,
                flags,
                allowed,
                store,
                text_buffer,
                modifier,
            )
        {
            dom.detach(id);
        }
    }
}
#[allow(clippy::too_many_arguments)]
fn should_remove(
    dom: &Dom,
    id: NodeId,
    semantic: Tag,
    flags: u32,
    allowed: &Regex,
    store: &mut crate::dom::NodeStateStore,
    text_buffer: &mut String,
    modifier: f64,
) -> bool {
    if semantic == Tag::Table && store.is_data_table(id) == Some(true) {
        return false;
    }
    let mut p = dom.parent(id);
    let mut depth = 0;
    let mut figure = false;
    while let Some(x) = p {
        if dom.tag(x) == Some(Tag::Table) && store.is_data_table(x) == Some(true) {
            return false;
        }
        if depth <= 3 && dom.tag(x) == Some(Tag::Code) {
            return false;
        }
        if depth <= 3 && dom.tag(x) == Some(Tag::Figure) {
            figure = true;
        }
        p = dom.parent(x);
        depth += 1;
    }
    if dom
        .descendants(id)
        .any(|x| dom.tag(x) == Some(Tag::Table) && store.is_data_table(x) == Some(true))
    {
        return false;
    }
    let stats = get_or_compute_stats(dom, id, store);
    let weight = get_class_weight(dom, id, flags);
    if weight < 0 {
        return true;
    }
    if stats.comma_count >= 10 {
        return false;
    }
    let mut pc = 0;
    let mut imgs = 0;
    let mut lis = 0usize;
    let mut inputs = 0;
    let mut embeds = 0;
    let mut heading = 0u64;
    let mut textish = 0u64;
    let mut list = 0u64;
    for x in dom.descendants(id) {
        match dom.tag(x) {
            Some(Tag::P) => {
                pc += 1;
                textish += u64::from(get_or_compute_stats(dom, x, store).text_length);
            }
            Some(Tag::Img) => {
                imgs += 1;
                textish += u64::from(get_or_compute_stats(dom, x, store).text_length);
            }
            Some(Tag::Li) => {
                lis += 1;
                textish += u64::from(get_or_compute_stats(dom, x, store).text_length);
            }
            Some(Tag::Input) => inputs += 1,
            Some(Tag::Ul | Tag::Ol) => {
                let length = u64::from(get_or_compute_stats(dom, x, store).text_length);
                list += length;
                textish += length;
            }
            Some(Tag::H1 | Tag::H2 | Tag::H3 | Tag::H4 | Tag::H5 | Tag::H6) => {
                heading += u64::from(get_or_compute_stats(dom, x, store).text_length)
            }
            Some(
                Tag::Span | Tag::Td | Tag::Blockquote | Tag::Dl | Tag::Div | Tag::Pre | Tag::Table,
            ) => textish += u64::from(get_or_compute_stats(dom, x, store).text_length),
            Some(Tag::Object | Tag::Embed | Tag::Iframe) => {
                if has_allowed_media(dom, x, allowed) {
                    return false;
                }
                embeds += 1;
            }
            _ => {}
        }
    }
    lis = lis.saturating_sub(100);
    let is_list = semantic == Tag::Ul
        || semantic == Tag::Ol
        || stats.text_length > 0 && list as f64 / stats.text_length as f64 > 0.9;
    if stats.text_length <= 32
        && regexps::AD_LOADING_SET.is_match(get_inner_text(dom, id, text_buffer))
    {
        return true;
    }
    let hd = if stats.text_length > 0 {
        heading as f64 / stats.text_length as f64
    } else {
        0.
    };
    let ld = get_link_density_cached(dom, id, stats.text_length, store);
    let td = if stats.text_length > 0 {
        textish as f64 / stats.text_length as f64
    } else {
        0.
    };
    let remove = (!figure && imgs > 1 && (pc as f64 / imgs as f64) < 0.5)
        || (!is_list && lis > pc)
        || inputs > pc / 3
        || (!is_list
            && !figure
            && hd < 0.9
            && stats.text_length < 25
            && (imgs == 0 || imgs > 2)
            && ld > 0.0)
        || (!is_list && weight < 25 && ld > 0.2 + modifier)
        || (weight >= 25 && ld > 0.5 + modifier)
        || (embeds == 1 && stats.text_length < 75)
        || embeds > 1
        || (imgs == 0 && td == 0.0);
    if is_list && remove {
        if dom
            .element_children(id)
            .any(|child| dom.element_children(child).nth(1).is_some())
        {
            return true;
        }
        let li = dom
            .descendants(id)
            .filter(|&x| dom.tag(x) == Some(Tag::Li))
            .count();
        if imgs == li {
            return false;
        }
    }
    remove
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
            Some(Tag::Img | Tag::Picture) => dom.set_attr(id, attr, &value),
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
            .is_some_and(|x| x.starts_with("readability"))
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
pub fn clean_matched_nodes<F>(
    dom: &mut Dom,
    root: NodeId,
    nodes: &mut Vec<NodeId>,
    match_buffer: &mut String,
    mut filter: F,
) where
    F: FnMut(&Dom, NodeId, &str) -> bool,
{
    nodes.clear();
    nodes.extend(dom.descendants(root));
    for &id in nodes.iter().rev() {
        crate::dom::build_match_string(dom, id, match_buffer);
        if dom.parent(id).is_some() && filter(dom, id, match_buffer) {
            dom.detach(id)
        }
    }
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
    fn removes_loading_placeholders_during_conditional_cleanup() {
        let mut dom = Dom::parse_document("<div>Loading...</div>").unwrap();
        let root = dom.root();
        let div = dom.first_descendant_by_tag(root, Tag::Div).unwrap();
        let mut store = NodeStateStore::new();
        let mut text_buffer = String::new();
        let mut nodes = Vec::new();
        clean_conditionally(
            &mut dom,
            root,
            &[Tag::Div],
            Tag::Div,
            FLAG_CLEAN_CONDITIONALLY,
            &Regex::new("$").unwrap(),
            &mut store,
            &mut text_buffer,
            &mut nodes,
            0.0,
        );
        assert!(dom.parent(div).is_none());
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

    #[test]
    fn detects_allowed_media_in_object_descendant_attributes_and_text() {
        let allowed = Regex::new("video\\.example").unwrap();
        let mut dom = Dom::parse_document(
            r#"<div><object id="attr"><param value="//video.example/id"></object><object id="text">//video.example/other</object><object id="remove"><param value="//ads.example/id"></object></div>"#,
        )
        .unwrap();
        let root = dom.first_descendant_by_tag(dom.root(), Tag::Div).unwrap();

        clean_tags(&mut dom, root, &[Tag::Object], &allowed, &mut Vec::new());

        assert!(
            dom.descendants(root)
                .any(|id| dom.attr(id, AttrName::Id) == Some("attr"))
        );
        assert!(
            dom.descendants(root)
                .any(|id| dom.attr(id, AttrName::Id) == Some("text"))
        );
        assert!(
            !dom.descendants(root)
                .any(|id| dom.attr(id, AttrName::Id) == Some("remove"))
        );
    }

    #[test]
    fn does_not_allow_iframe_from_matching_fallback_text() {
        let allowed = Regex::new("video\\.example").unwrap();
        let mut dom = Dom::parse_document(
            r#"<div><iframe src="//ads.example">//video.example/id</iframe></div>"#,
        )
        .unwrap();
        let root = dom.first_descendant_by_tag(dom.root(), Tag::Div).unwrap();

        clean_tags(&mut dom, root, &[Tag::Iframe], &allowed, &mut Vec::new());

        assert!(
            !dom.descendants(root)
                .any(|id| dom.tag(id) == Some(Tag::Iframe))
        );
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
