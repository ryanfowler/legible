//! DOM preparation and article cleanup.
#![allow(clippy::collapsible_if)]
use crate::constants::{
    PRESENTATIONAL_ATTRIBUTES, flags::*, has_image_extension, has_image_src, has_image_srcset,
    is_deprecated_size_attribute_elem, regexps,
};
use crate::dom::{AttrName, Dom, NodeId, Tag};
use crate::scoring::{
    get_class_weight, get_inner_text, get_link_density_cached, get_or_compute_stats,
    has_single_tag_inside_element, is_element_without_content, is_phrasing_content,
};
use regex::Regex;
use std::borrow::Cow;

pub fn prep_document(dom: &mut Dom) {
    let mut ids = Vec::new();
    dom.collect_descendants_by_tag(dom.root(), Tag::Style, &mut ids);
    for &id in &ids {
        dom.detach(id);
    }
    if let Some(body) = dom.body() {
        replace_brs(dom, body);
    }
    ids.clear();
    dom.collect_descendants_by_tag(dom.root(), Tag::Font, &mut ids);
    for id in ids {
        if dom.parent(id).is_some() {
            dom.rename_html(id, Tag::Span);
        }
    }
}
fn next_element(dom: &Dom, id: NodeId) -> Option<NodeId> {
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
fn replace_brs(dom: &mut Dom, root: NodeId) {
    let ids: Vec<_> = dom
        .descendants(root)
        .filter(|&x| dom.tag(x) == Some(Tag::Br))
        .collect();
    for br in ids {
        if dom.parent(br).is_none() {
            continue;
        }
        let mut next = next_element(dom, br);
        let mut replaced = false;
        while let Some(x) = next {
            if dom.tag(x) == Some(Tag::Br) {
                replaced = true;
                next = next_element(dom, x);
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
                && next_element(dom, x).is_some_and(|y| dom.tag(y) == Some(Tag::Br))
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
            if dom.is_text(x) && dom.text_node(x).is_some_and(|t| t.trim().is_empty()) {
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
pub fn remove_scripts(dom: &mut Dom) {
    let ids: Vec<_> = dom
        .descendants(dom.root())
        .filter(|&x| matches!(dom.tag(x), Some(Tag::Script | Tag::Noscript)))
        .collect();
    for id in ids {
        if dom.parent(id).is_some() {
            dom.detach(id);
        }
    }
}
pub fn clean_tags(dom: &mut Dom, root: NodeId, tags: &[Tag], allowed: &Regex) {
    let ids: Vec<_> = dom
        .descendants(root)
        .filter(|&id| dom.tag(id).is_some_and(|t| tags.contains(&t)))
        .collect();
    for id in ids {
        if dom.parent(id).is_none() {
            continue;
        }
        let tag = dom.tag(id).unwrap();
        if matches!(tag, Tag::Object | Tag::Embed | Tag::Iframe) {
            let keep = dom
                .attrs(id)
                .iter()
                .any(|a| allowed.is_match(a.value.as_ref()))
                || (tag == Tag::Object
                    && allowed.is_match(&dom.inner_html(id).unwrap_or_default()));
            if keep {
                continue;
            }
        }
        dom.detach(id);
    }
}
pub fn clean_styles(dom: &mut Dom, root: NodeId) {
    let ids: Vec<_> = std::iter::once(root).chain(dom.descendants(root)).collect();
    for id in ids {
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
pub fn clean_headers(dom: &mut Dom, root: NodeId, flags: u32) {
    let ids: Vec<_> = dom
        .descendants(root)
        .filter(|&id| matches!(dom.tag(id), Some(Tag::H1 | Tag::H2)))
        .collect();
    for id in ids {
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
    modifier: f64,
) {
    if flags & FLAG_CLEAN_CONDITIONALLY == 0 {
        return;
    }
    let ids: Vec<_> = dom
        .descendants(root)
        .filter(|&id| dom.tag(id).is_some_and(|t| tags.contains(&t)))
        .collect();
    store.clear_stats();
    for id in ids.into_iter().rev() {
        if dom.parent(id).is_some()
            && should_remove(dom, id, semantic, flags, allowed, store, modifier)
        {
            dom.detach(id);
        }
    }
}
fn should_remove(
    dom: &Dom,
    id: NodeId,
    semantic: Tag,
    flags: u32,
    allowed: &Regex,
    store: &mut crate::dom::NodeStateStore,
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
    let mut heading = 0;
    let mut textish = 0;
    let mut list = 0;
    for x in dom.descendants(id) {
        match dom.tag(x) {
            Some(Tag::P) => {
                pc += 1;
                textish += get_or_compute_stats(dom, x, store).text_length;
            }
            Some(Tag::Img) => {
                imgs += 1;
                textish += get_or_compute_stats(dom, x, store).text_length;
            }
            Some(Tag::Li) => {
                lis += 1;
                textish += get_or_compute_stats(dom, x, store).text_length;
            }
            Some(Tag::Input) => inputs += 1,
            Some(Tag::Ul | Tag::Ol) => {
                let n = get_or_compute_stats(dom, x, store).text_length;
                list += n;
                textish += n;
            }
            Some(Tag::H1 | Tag::H2 | Tag::H3 | Tag::H4 | Tag::H5 | Tag::H6) => {
                heading += get_or_compute_stats(dom, x, store).text_length
            }
            Some(
                Tag::Span | Tag::Td | Tag::Blockquote | Tag::Dl | Tag::Div | Tag::Pre | Tag::Table,
            ) => textish += get_or_compute_stats(dom, x, store).text_length,
            Some(Tag::Object | Tag::Embed | Tag::Iframe) => {
                if dom
                    .attrs(x)
                    .iter()
                    .any(|a| allowed.is_match(a.value.as_ref()))
                    || dom.tag(x) == Some(Tag::Object)
                        && allowed.is_match(&dom.inner_html(x).unwrap_or_default())
                {
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
    if stats.text_length <= 32 && regexps::AD_LOADING_SET.is_match(&get_inner_text(dom, id, false))
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
pub fn mark_data_tables(dom: &Dom, root: NodeId, store: &mut crate::dom::NodeStateStore) {
    let ids: Vec<_> = dom
        .descendants(root)
        .filter(|&x| dom.tag(x) == Some(Tag::Table))
        .collect();
    for id in ids {
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
pub fn fix_lazy_images(dom: &mut Dom, root: NodeId) {
    let ids: Vec<_> = dom
        .descendants(root)
        .filter(|&x| matches!(dom.tag(x), Some(Tag::Img | Tag::Picture | Tag::Figure)))
        .collect();
    for id in ids {
        let mut src = false;
        let mut srcset = false;
        let mut lazy = false;
        let mut b64 = false;
        let mut other = false;
        let mut lazy_src = None;
        let mut lazy_srcset = None;
        for a in dom.attrs(id) {
            let v = a.value.as_ref();
            match a.known {
                AttrName::Src => {
                    src = !v.is_empty();
                    if let Some(c) = regexps::B64_DATA_URL.captures(v)
                        && c.get(1).map(|m| m.as_str()) != Some("image/svg+xml")
                    {
                        b64 = true;
                    }
                }
                AttrName::Srcset => srcset = !v.is_empty() && v != "null",
                AttrName::Class => {
                    lazy |= v.split_whitespace().any(|x| x.eq_ignore_ascii_case("lazy"))
                }
                _ => {
                    other |= has_image_extension(v);
                    if has_image_srcset(v) {
                        lazy_srcset = Some(v.to_string())
                    } else if has_image_src(v) {
                        lazy_src = Some(v.to_string())
                    }
                }
            }
        }
        if b64
            && other
            && let Some(v) = dom.attr(id, AttrName::Src)
            && let Some(c) = regexps::B64_DATA_URL.captures(v)
            && v.len()
                .saturating_sub(c.get(0).map(|m| m.end()).unwrap_or(0))
                < 133
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
                let _ = dom.set_inner_html(
                    id,
                    &format!("<img {}=\"{}\">", attr.as_str(), escape_attr(&value)),
                );
            }
            _ => {}
        }
    }
}
fn escape_attr(s: &str) -> Cow<'_, str> {
    if !s.contains(['&', '"', '<', '>']) {
        Cow::Borrowed(s)
    } else {
        let mut o = String::new();
        for c in s.chars() {
            match c {
                '&' => o.push_str("&amp;"),
                '"' => o.push_str("&quot;"),
                '<' => o.push_str("&lt;"),
                '>' => o.push_str("&gt;"),
                _ => o.push(c),
            }
        }
        Cow::Owned(o)
    }
}
fn is_single_image_markup(html: &str) -> bool {
    let trimmed = html.trim();
    let lower = trimmed.to_ascii_lowercase();
    lower.starts_with("<img")
        && lower.matches("<img").count() == 1
        && lower.matches('<').count() == 1
        && trimmed.ends_with('>')
}

fn useful_image(dom: &Dom, id: NodeId) -> bool {
    dom.attrs(id).iter().any(|a| {
        let name = a.name.local.as_ref();
        matches!(name, "src" | "srcset" | "data-src" | "data-srcset")
            || has_image_extension(a.value.as_ref())
    })
}

fn copy_image_attributes(dom: &mut Dom, from: NodeId, to: NodeId) {
    let attrs: Vec<_> = dom
        .attrs(from)
        .iter()
        .filter(|a| {
            matches!(a.known, AttrName::Src | AttrName::Srcset)
                || has_image_extension(a.value.as_ref())
        })
        .map(|a| (a.name.clone(), a.value.clone()))
        .collect();
    for (name, value) in attrs {
        if dom.attr_by_local_name(to, name.local.as_ref()).is_none() {
            dom.set_attr_qual(to, name, value);
        }
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
    let images: Vec<_> = dom
        .descendants(id)
        .filter(|&node| dom.tag(node) == Some(Tag::Img))
        .collect();
    if images.len() == 1 {
        Some(images[0])
    } else {
        None
    }
}

pub fn unwrap_noscript_images(dom: &mut Dom) {
    let images: Vec<_> = dom
        .descendants(dom.root())
        .filter(|&id| dom.tag(id) == Some(Tag::Img) && !useful_image(dom, id))
        .collect();
    for id in images {
        if dom.parent(id).is_some() {
            dom.detach(id);
        }
    }
    let ids: Vec<_> = dom
        .descendants(dom.root())
        .filter(|&x| dom.tag(x) == Some(Tag::Noscript))
        .collect();
    for id in ids {
        if dom.parent(id).is_none() {
            continue;
        }
        let image_ids: Vec<_> = dom
            .descendants(id)
            .filter(|&node| dom.tag(node) == Some(Tag::Img))
            .collect();
        if dom.has_non_whitespace_text(id) {
            continue;
        }
        if image_ids.len() == 1 {
            let image = image_ids[0];
            if let Some(previous) = previous_element(dom, id)
                && let Some(previous_image) = single_image_element(dom, previous)
            {
                copy_image_attributes(dom, previous_image, image);
                dom.insert_before(id, image);
                dom.detach(previous);
                dom.detach(id);
                continue;
            }
            continue;
        }
        let Some(html) = dom.inner_html(id).ok() else {
            continue;
        };
        if !is_single_image_markup(&html) {
            continue;
        }
        let Ok(inserted) = dom.insert_html_after(id, &html) else {
            continue;
        };
        let Some(new_image) = inserted.iter().copied().find_map(|node| {
            if dom.tag(node) == Some(Tag::Img) {
                Some(node)
            } else {
                dom.first_descendant_by_tag(node, Tag::Img)
            }
        }) else {
            continue;
        };
        if let Some(previous) = previous_element(dom, id)
            && let Some(previous_image) = single_image_element(dom, previous)
        {
            copy_image_attributes(dom, previous_image, new_image);
            dom.insert_before(id, new_image);
            dom.detach(previous);
            dom.detach(id);
        }
    }
}
pub fn simplify_nested_elements(dom: &mut Dom, root: NodeId) {
    let ids: Vec<_> = dom.descendants(root).collect();
    for id in ids.into_iter().rev() {
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
pub fn clean_matched_nodes<F>(dom: &mut Dom, root: NodeId, filter: F)
where
    F: Fn(&Dom, NodeId, &str) -> bool,
{
    let ids: Vec<_> = dom.descendants(root).collect();
    let mut match_string = String::new();
    for id in ids.into_iter().rev() {
        crate::dom::build_match_string(dom, id, &mut match_string);
        if dom.parent(id).is_some() && filter(dom, id, &match_string) {
            dom.detach(id)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dom::{AttrName, NodeStateStore};

    #[test]
    fn replaces_short_base64_image_placeholders() {
        let mut dom = Dom::parse_document(
            r#"<img src="data:image/png;base64,AAAA" data-src="https://example.com/image.jpg">"#,
        )
        .unwrap();
        let root = dom.root();
        let image = dom.first_descendant_by_tag(root, Tag::Img).unwrap();
        fix_lazy_images(&mut dom, root);
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
        clean_conditionally(
            &mut dom,
            root,
            &[Tag::Div],
            Tag::Div,
            FLAG_CLEAN_CONDITIONALLY,
            &Regex::new("$").unwrap(),
            &mut store,
            0.0,
        );
        assert!(dom.parent(div).is_none());
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
    fn preserves_svg_presentation_attributes() {
        let mut dom = Dom::parse_document(
            r#"<svg width="10" height="10"><path fill="red" stroke="blue"/></svg>"#,
        )
        .unwrap();
        let root = dom.root();
        let svg = dom.first_descendant_by_tag(root, Tag::Svg).unwrap();
        let path = dom.first_descendant_by_tag(svg, Tag::Svg).unwrap();
        clean_styles(&mut dom, root);
        assert_eq!(dom.attr_by_local_name(svg, "width"), Some("10"));
        assert_eq!(dom.attr_by_local_name(path, "fill"), Some("red"));
    }
}
