//! Content scoring and DOM text helpers.
use crate::constants::{flags::*, has_byline, is_div_to_p_elem, is_phrasing_elem, regexps};
use crate::dom::{AttrName, Dom, NodeId, NodeStateStore, NodeStats, Tag};
use smallvec::SmallVec;
fn is_hash_url(s: &str) -> bool {
    s.starts_with('#') && s.len() > 1
}
fn stats_for_text(text: &str) -> NodeStats {
    let mut s = NodeStats {
        has_text: !text.is_empty(),
        starts_with_whitespace: text.starts_with(char::is_whitespace),
        ends_with_whitespace: text.ends_with(char::is_whitespace),
        ..Default::default()
    };
    let mut prev = true;
    let mut dot = false;
    let mut text_length = 0usize;
    let mut comma_count = 0usize;

    // Most article text is ASCII. Scan bytes to avoid UTF-8 decoding and
    // Unicode whitespace tables in the hot loop.
    if text.is_ascii() {
        for &byte in text.as_bytes() {
            if byte.is_ascii_whitespace() {
                s.has_sentence_break |= dot;
                dot = false;
                if !prev {
                    text_length += 1;
                    prev = true
                }
            } else {
                s.has_non_whitespace = true;
                dot = byte == b'.';
                comma_count += usize::from(byte == b',');
                text_length += 1;
                prev = false
            }
        }
    } else {
        for c in text.chars() {
            if c.is_whitespace() {
                s.has_sentence_break |= dot;
                dot = false;
                if !prev {
                    text_length += 1;
                    prev = true
                }
            } else {
                s.has_non_whitespace = true;
                dot = c == '.';
                comma_count += usize::from(
                    c == ','
                        || matches!(
                            c,
                            '\u{060C}'
                                | '\u{FE50}'
                                | '\u{FE10}'
                                | '\u{FE11}'
                                | '\u{2E41}'
                                | '\u{2E34}'
                                | '\u{2E32}'
                                | '\u{FF0C}'
                        ),
                );
                text_length += 1;
                prev = false
            }
        }
    }
    if prev && text_length > 0 {
        text_length -= 1
    }
    s.text_length = text_length.min(u32::MAX as usize) as u32;
    s.comma_count = comma_count.min(u32::MAX as usize) as u32;
    s.ends_with_dot = dot;
    s.has_sentence_end = s.has_sentence_break || dot;
    s
}
fn append_stats(a: &mut NodeStats, b: &NodeStats) {
    if !b.has_text {
        return;
    }
    if !a.has_text {
        *a = *b;
        return;
    }
    a.has_sentence_break |= b.has_sentence_break || (a.ends_with_dot && b.starts_with_whitespace);
    if a.has_non_whitespace
        && b.has_non_whitespace
        && (a.ends_with_whitespace || b.starts_with_whitespace)
    {
        a.text_length = a.text_length.saturating_add(1)
    }
    a.text_length = a.text_length.saturating_add(b.text_length);
    a.comma_count = a.comma_count.saturating_add(b.comma_count);
    a.has_non_whitespace |= b.has_non_whitespace;
    a.ends_with_whitespace = b.ends_with_whitespace;
    a.ends_with_dot = b.ends_with_dot;
    a.has_sentence_end = a.has_sentence_break || a.ends_with_dot
}
pub fn get_or_compute_stats(dom: &Dom, id: NodeId, store: &mut NodeStateStore) -> NodeStats {
    if let Some(s) = store.get_stats(id) {
        return *s;
    }

    let mut stack = SmallVec::<[(NodeId, bool); 16]>::new();
    stack.push((id, false));
    while let Some((n, expanded)) = stack.pop() {
        if store.get_stats(n).is_some() {
            continue;
        }
        if !expanded {
            stack.push((n, true));
            for c in dom.children_rev(n) {
                if store.get_stats(c).is_none() {
                    stack.push((c, false))
                }
            }
            continue;
        }
        let mut s = match dom.text_node(n) {
            Some(t) => stats_for_text(t),
            None => NodeStats::default(),
        };
        let cache_links = store.link_lengths_enabled();
        let mut link_length = 0.0;
        for c in dom.children(n) {
            if let Some(cs) = store.get_stats(c) {
                append_stats(&mut s, cs)
            }
            if cache_links {
                link_length += store.link_length(c);
            }
        }
        if cache_links {
            if dom.tag(n) == Some(Tag::A) {
                link_length = s.text_length as f64
                    * if dom.attr(n, AttrName::Href).is_some_and(is_hash_url) {
                        0.3
                    } else {
                        1.0
                    };
            }
            store.set_link_length(n, link_length);
        }
        s.has_sentence_end = s.has_sentence_break || s.ends_with_dot;
        store.set_stats(n, s)
    }
    store.get_stats(id).copied().unwrap_or_default()
}
pub fn compute_initial_readability_data(dom: &Dom, id: NodeId, flags: u32) -> f64 {
    let score = match dom.tag(id) {
        Some(Tag::Div) => 5.,
        Some(Tag::Pre | Tag::Td | Tag::Blockquote) => 3.,
        Some(
            Tag::Address | Tag::Ol | Tag::Ul | Tag::Dl | Tag::Dd | Tag::Dt | Tag::Li | Tag::Form,
        ) => -3.,
        Some(Tag::H1 | Tag::H2 | Tag::H3 | Tag::H4 | Tag::H5 | Tag::H6 | Tag::Th) => -5.,
        _ => 0.,
    };
    score + get_class_weight(dom, id, flags) as f64
}
pub fn initialize_node(dom: &Dom, id: NodeId, store: &mut NodeStateStore, flags: u32) {
    store.initialize_if_absent(id, compute_initial_readability_data(dom, id, flags));
}
pub fn get_class_weight(dom: &Dom, id: NodeId, flags: u32) -> i32 {
    if flags & FLAG_WEIGHT_CLASSES == 0 {
        return 0;
    }
    let mut w = 0;
    for a in dom.attrs(id) {
        if !matches!(a.name.local.as_ref(), "class" | "id") || a.value.is_empty() {
            continue;
        }
        let m = regexps::CLASS_WEIGHT_SET.matches(a.value.as_ref());
        if m.matched(0) {
            w -= 25
        }
        if m.matched(1) {
            w += 25
        }
    }
    w
}
pub fn has_non_empty_inner_text(dom: &Dom, id: NodeId) -> bool {
    dom.has_non_whitespace_text(id)
}
pub fn get_inner_text<'a>(dom: &Dom, id: NodeId, out: &'a mut String) -> &'a str {
    out.clear();
    dom.append_text(id, out);
    out.trim()
}
pub fn get_normalized_inner_text<'a>(dom: &Dom, id: NodeId, out: &'a mut String) -> &'a str {
    out.clear();
    dom.append_normalized_text(id, out);
    out
}
pub fn get_inner_text_owned(dom: &Dom, id: NodeId) -> String {
    let mut out = String::new();
    dom.append_text(id, &mut out);
    let start = out.len() - out.trim_start().len();
    let end = out.trim_end().len();
    if end == 0 {
        out.clear();
    } else {
        out.truncate(end);
        if start != 0 {
            out.drain(..start);
        }
    }
    out
}
pub fn get_link_density_with_text(
    dom: &Dom,
    id: NodeId,
    text: Option<&str>,
    mut store: Option<&mut NodeStateStore>,
) -> f64 {
    let total = text.map_or_else(|| dom.normalized_char_count(id), |x| x.chars().count());
    if total == 0 {
        return 0.;
    }
    let mut links = 0.;
    for x in dom.descendants(id) {
        if dom.tag(x) != Some(Tag::A) {
            continue;
        }
        let len = if let Some(st) = store.as_deref_mut() {
            get_or_compute_stats(dom, x, st).text_length as usize
        } else {
            dom.normalized_char_count(x)
        };
        links += len as f64
            * if dom.attr(x, AttrName::Href).is_some_and(is_hash_url) {
                0.3
            } else {
                1.
            }
    }
    links / total as f64
}
pub fn get_link_density(dom: &Dom, id: NodeId) -> f64 {
    get_link_density_with_text(dom, id, None, None)
}
pub fn get_link_density_cached(dom: &Dom, id: NodeId, len: u32, store: &mut NodeStateStore) -> f64 {
    if len == 0 {
        return 0.;
    }
    get_or_compute_stats(dom, id, store);
    if dom.tag(id) == Some(Tag::A) {
        // Link density excludes the root itself. This case is not part of the
        // normal candidate path, but preserve the helper's original behavior.
        let links = dom
            .children(id)
            .map(|child| store.link_length(child))
            .sum::<f64>();
        links / len as f64
    } else {
        store.link_length(id) / len as f64
    }
}
pub fn is_whitespace(dom: &Dom, id: NodeId) -> bool {
    dom.text_node(id).is_some_and(|t| t.trim().is_empty()) || dom.tag(id) == Some(Tag::Br)
}
pub fn is_phrasing_content(dom: &Dom, id: NodeId) -> bool {
    fn go(d: &Dom, n: NodeId, depth: u32) -> bool {
        if d.is_text(n) {
            return true;
        }
        let Some(t) = d.tag(n) else { return false };
        if is_phrasing_elem(t) {
            return true;
        }
        matches!(t, Tag::A | Tag::Del | Tag::Ins)
            && depth < 10
            && d.children(n).all(|c| go(d, c, depth + 1))
    }
    go(dom, id, 0)
}
pub fn wrap_phrasing_content_in_p(dom: &mut Dom, div: NodeId) {
    let children: SmallVec<[NodeId; 8]> = dom.children(div).collect();
    let mut i = 0;
    while i < children.len() {
        if !is_phrasing_content(dom, children[i]) {
            i += 1;
            continue;
        }
        let mut j = i;
        let mut content = false;
        while j < children.len() && is_phrasing_content(dom, children[j]) {
            content |= !dom.is_text(children[j])
                || dom
                    .text_node(children[j])
                    .is_some_and(|t| !t.trim().is_empty());
            j += 1
        }
        if content {
            let mut a = i;
            let mut b = j;
            while a < b && is_whitespace(dom, children[a]) {
                a += 1
            }
            while a < b && is_whitespace(dom, children[b - 1]) {
                b -= 1
            }
            if a < b {
                let p = dom.create_html_element(Tag::P).expect("DOM node limit");
                dom.insert_before(children[a], p);
                for &x in &children[a..b] {
                    dom.append_child(p, x)
                }
                for &x in children[i..a].iter().chain(children[b..j].iter()) {
                    dom.detach(x)
                }
            }
        }
        i = j
    }
}
pub fn is_element_without_content(dom: &Dom, id: NodeId) -> bool {
    dom.is_element(id)
        && dom
            .element_children(id)
            .all(|c| matches!(dom.tag(c), Some(Tag::Br | Tag::Hr)))
        && !dom.has_non_whitespace_text(id)
}
pub fn has_single_tag_inside_element(dom: &Dom, id: NodeId, tag: Tag) -> bool {
    let mut found = false;
    for c in dom.children(id) {
        if dom.is_element(c) {
            if found || dom.tag(c) != Some(tag) {
                return false;
            }
            found = true
        } else if dom.is_text(c)
            && dom
                .text_node(c)
                .is_some_and(|t| t.ends_with(|x: char| !x.is_whitespace()))
        {
            return false;
        }
    }
    found
}
pub fn has_child_block_element(dom: &Dom, id: NodeId) -> bool {
    dom.descendants(id)
        .any(|x| dom.is_element(x) && dom.tag(x).is_some_and(is_div_to_p_elem))
}
pub fn is_probably_visible(dom: &Dom, id: NodeId) -> bool {
    if dom.attr(id, AttrName::Style).is_some_and(has_hidden_style)
        || dom.has_attr(id, AttrName::Hidden)
    {
        return false;
    }
    if dom.attr(id, AttrName::AriaHidden) == Some("true")
        && !dom
            .attr(id, AttrName::Class)
            .is_some_and(|x| x.contains("fallback-image"))
    {
        return false;
    }
    true
}
pub fn is_valid_byline(dom: &Dom, id: NodeId, ms: &str, text_buffer: &mut String) -> bool {
    let ok = dom.attr(id, AttrName::Rel) == Some("author")
        || dom
            .attr(id, AttrName::ItemProp)
            .is_some_and(|x| x.contains("author"))
        || has_byline(ms);
    if !ok {
        return false;
    }
    let t = get_inner_text(dom, id, text_buffer);
    !t.is_empty() && t.len() < 400 && t.chars().count() < 100
}
fn has_hidden_style(style: &str) -> bool {
    let style = style.as_bytes();
    (0..style.len()).any(|start| {
        matches_style_declaration(style, start, b"display", b"none")
            || matches_style_declaration(style, start, b"visibility", b"hidden")
    })
}

fn matches_style_declaration(style: &[u8], start: usize, property: &[u8], value: &[u8]) -> bool {
    let property_end = start + property.len();
    if property_end > style.len() || !style[start..property_end].eq_ignore_ascii_case(property) {
        return false;
    }
    let mut cursor = property_end;
    while style.get(cursor).is_some_and(u8::is_ascii_whitespace) {
        cursor += 1;
    }
    if style.get(cursor) != Some(&b':') {
        return false;
    }
    cursor += 1;
    while style.get(cursor).is_some_and(u8::is_ascii_whitespace) {
        cursor += 1;
    }
    let value_end = cursor + value.len();
    value_end <= style.len() && style[cursor..value_end].eq_ignore_ascii_case(value)
}

#[cfg(test)]
mod tests {
    use super::{get_link_density, get_link_density_cached, stats_for_text};
    use crate::dom::{Dom, NodeStateStore, Tag};

    #[test]
    fn text_stats_match_for_ascii_and_unicode_paths() {
        let ascii = stats_for_text(" a,\t b. c ");
        assert_eq!(ascii.text_length, 7);
        assert_eq!(ascii.comma_count, 1);
        assert!(ascii.starts_with_whitespace);
        assert!(ascii.ends_with_whitespace);
        assert!(ascii.has_sentence_break);
        assert!(ascii.has_sentence_end);

        let unicode = stats_for_text("\u{3000}甲， 乙.\u{a0}");
        assert_eq!(unicode.text_length, 5);
        assert_eq!(unicode.comma_count, 1);
        assert!(unicode.starts_with_whitespace);
        assert!(unicode.ends_with_whitespace);
        assert!(unicode.has_sentence_end);
    }

    #[test]
    fn cached_link_density_matches_structural_scan() {
        let dom = Dom::parse_fragment(
            r##"plain <a href="/full">full</a> <a href="#hash">hash</a>"##,
            Tag::Div,
        )
        .unwrap();
        let root = dom.root();
        let mut store = NodeStateStore::new();
        store.enable_link_lengths();
        let len = dom.normalized_char_count(root);

        let expected = get_link_density(&dom, root);
        let actual = get_link_density_cached(&dom, root, len as u32, &mut store);
        assert!((actual - expected).abs() < f64::EPSILON);
    }
}
