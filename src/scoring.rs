//! Content scoring logic for Readability.

use crate::constants::{flags::*, is_div_to_p_elem, is_phrasing_elem, regexps};
use crate::dom::{NodeDataStore, NodeStats, get_tag_name, has_tag_name};
use crate::selectors::Selectors;
use dom_query::{Node, NodeData};

/// Check if a URL is a hash URL (starts with '#' and has content after it).
/// Equivalent to the regex `^#.+` but avoids regex overhead.
#[inline]
fn is_hash_url(s: &str) -> bool {
    s.starts_with('#') && s.len() > 1
}

/// Get or compute stats for a node, caching the result.
pub fn get_or_compute_stats(node: &Node<'_>, store: &mut NodeDataStore) -> NodeStats {
    if let Some(stats) = store.get_stats(&node.id) {
        return *stats;
    }

    // Avoid populating the cache for every inline descendant of small subtrees.
    // Those nodes are unlikely to be queried independently, so a single direct
    // scan is cheaper than many hash-table insertions.
    if let Some(stats) = stats_for_small_subtree(node, 64) {
        store.set_stats(node.id, stats);
        return stats;
    }

    // Compute the subtree bottom-up so each text node is scanned once even when
    // stats are later requested for several nested ancestors.
    let mut stack = vec![(*node, false)];
    while let Some((current, expanded)) = stack.pop() {
        if store.get_stats(&current.id).is_some() {
            continue;
        }

        if !expanded {
            stack.push((current, true));
            let children: Vec<_> = current
                .children_it(true)
                .filter(|child| store.get_stats(&child.id).is_none())
                .collect();
            stack.extend(children.into_iter().map(|child| (child, false)));
            continue;
        }

        let mut stats = current
            .query(|tree_node| match &tree_node.data {
                NodeData::Text { contents } => stats_for_text(contents),
                _ => NodeStats::default(),
            })
            .unwrap_or_default();

        for child in current.children_it(false) {
            if let Some(child_stats) = store.get_stats(&child.id) {
                append_stats(&mut stats, child_stats);
            }
        }
        stats.has_sentence_end = stats.has_sentence_break || stats.ends_with_dot;
        store.set_stats(current.id, stats);
    }

    store.get_stats(&node.id).copied().unwrap_or_default()
}

fn stats_for_small_subtree(node: &Node<'_>, max_nodes: usize) -> Option<NodeStats> {
    let mut stats = NodeStats::default();

    for (index, descendant) in std::iter::once(*node)
        .chain(node.descendants_it())
        .enumerate()
    {
        if index == max_nodes {
            return None;
        }
        descendant.query(|tree_node| {
            if let NodeData::Text { contents } = &tree_node.data {
                append_stats(&mut stats, &stats_for_text(contents));
            }
        });
    }

    Some(stats)
}

fn stats_for_text(text: &str) -> NodeStats {
    let mut stats = NodeStats {
        has_text: !text.is_empty(),
        starts_with_whitespace: text.starts_with(char::is_whitespace),
        ends_with_whitespace: text.ends_with(char::is_whitespace),
        ..NodeStats::default()
    };
    let mut previous_was_whitespace = true;
    let mut last_was_dot = false;

    for c in text.chars() {
        if c.is_whitespace() {
            stats.has_sentence_break |= last_was_dot;
            last_was_dot = false;
            if !previous_was_whitespace {
                stats.text_length += 1;
                previous_was_whitespace = true;
            }
        } else {
            stats.has_non_whitespace = true;
            last_was_dot = c == '.';
            stats.comma_count += usize::from(
                c == ','
                    || (c as u32 >= 0x0600
                        && matches!(
                            c,
                            '\u{060C}'
                                | '\u{FE50}'
                                | '\u{FE10}'
                                | '\u{FE11}'
                                | '\u{2E41}'
                                | '\u{2E34}'
                                | '\u{2E32}'
                                | '\u{FF0C}'
                        )),
            );
            stats.text_length += 1;
            previous_was_whitespace = false;
        }
    }

    if previous_was_whitespace && stats.text_length > 0 {
        stats.text_length -= 1;
    }
    stats.ends_with_dot = last_was_dot;
    stats.has_sentence_end = stats.has_sentence_break || stats.ends_with_dot;
    stats
}

fn append_stats(stats: &mut NodeStats, child: &NodeStats) {
    if !child.has_text {
        return;
    }

    if !stats.has_text {
        *stats = *child;
        return;
    }

    stats.has_sentence_break |=
        child.has_sentence_break || (stats.ends_with_dot && child.starts_with_whitespace);
    if stats.has_non_whitespace
        && child.has_non_whitespace
        && (stats.ends_with_whitespace || child.starts_with_whitespace)
    {
        stats.text_length += 1;
    }
    stats.text_length += child.text_length;
    stats.comma_count += child.comma_count;
    stats.has_non_whitespace |= child.has_non_whitespace;
    stats.ends_with_whitespace = child.ends_with_whitespace;
    stats.ends_with_dot = child.ends_with_dot;
    stats.has_sentence_end = stats.has_sentence_break || stats.ends_with_dot;
}

/// Compute the initial readability data for a node without storing it.
/// Used with NodeDataStore::initialize_if_absent for single-lookup initialization.
pub fn compute_initial_readability_data(
    node: &Node<'_>,
    flags: u32,
) -> crate::dom::ReadabilityData {
    let initial_score = match get_tag_name(node).as_deref() {
        Some("DIV") => 5.0,
        Some("PRE") | Some("TD") | Some("BLOCKQUOTE") => 3.0,
        Some("ADDRESS") | Some("OL") | Some("UL") | Some("DL") | Some("DD") | Some("DT")
        | Some("LI") | Some("FORM") => -3.0,
        Some("H1") | Some("H2") | Some("H3") | Some("H4") | Some("H5") | Some("H6")
        | Some("TH") => -5.0,
        _ => 0.0,
    };

    let class_weight = get_class_weight(node, flags);
    crate::dom::ReadabilityData::with_score(initial_score + class_weight as f64)
}

/// Initialize a node with readability data and initial score based on tag.
pub fn initialize_node(node: &Node<'_>, store: &mut NodeDataStore, flags: u32) {
    store.set(node.id, compute_initial_readability_data(node, flags));
}

/// Get the class/id weight of an element.
/// Positive weight for content-like classes, negative for non-content.
/// Uses RegexSet for efficient single-pass matching.
pub fn get_class_weight(node: &Node<'_>, flags: u32) -> i32 {
    if (flags & FLAG_WEIGHT_CLASSES) == 0 {
        return 0;
    }

    let mut weight: i32 = 0;

    // Check class name using RegexSet for 2 matches in single pass
    if let Some(class_name) = node.attr("class") {
        let class_str = class_name.as_ref();
        if !class_str.is_empty() {
            let matches = regexps::CLASS_WEIGHT_SET.matches(class_str);
            if matches.matched(0) {
                weight -= 25; // NEGATIVE matched
            }
            if matches.matched(1) {
                weight += 25; // POSITIVE matched
            }
        }
    }

    // Check ID using RegexSet for 2 matches in single pass
    if let Some(id) = node.attr("id") {
        let id_str = id.as_ref();
        if !id_str.is_empty() {
            let matches = regexps::CLASS_WEIGHT_SET.matches(id_str);
            if matches.matched(0) {
                weight -= 25; // NEGATIVE matched
            }
            if matches.matched(1) {
                weight += 25; // POSITIVE matched
            }
        }
    }

    weight
}

/// Check if a node has non-whitespace inner text, without allocating a String.
/// This is an optimized alternative to `!get_inner_text(n, false).is_empty()`.
pub fn has_non_empty_inner_text(node: &Node<'_>) -> bool {
    has_non_whitespace_text(node)
}

/// Get the inner text of a node, optionally normalizing whitespace.
pub fn get_inner_text(node: &Node<'_>, normalize_spaces: bool) -> String {
    let text = node.text();
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    if normalize_spaces {
        normalize_whitespace(trimmed)
    } else if trimmed.as_ptr() == text.as_ptr() && trimmed.len() == text.len() {
        // Text is already trimmed — avoid allocation when callers need ownership
        text.to_string()
    } else {
        trimmed.to_string()
    }
}

/// Collapse runs of 2+ whitespace characters into a single space.
/// Returns the original string (as a new allocation) if no collapsing is needed.
fn normalize_whitespace(s: &str) -> String {
    // Quick pre-check: only allocate a new string if there are consecutive whitespace chars
    let needs_normalize = s
        .as_bytes()
        .windows(2)
        .any(|w| w[0].is_ascii_whitespace() && w[1].is_ascii_whitespace())
        || s.bytes().any(|b| b == b'\t' || b == b'\n' || b == b'\r');
    if !needs_normalize {
        return s.to_string();
    }
    let mut result = String::with_capacity(s.len());
    let mut prev_ws = false;
    for c in s.chars() {
        if c.is_whitespace() {
            if !prev_ws {
                result.push(' ');
            }
            prev_ws = true;
        } else {
            result.push(c);
            prev_ws = false;
        }
    }
    result
}

/// Get the link density of an element with optional pre-extracted text.
/// Use this when you already have the inner text to avoid redundant extraction.
pub fn get_link_density_with_text(
    node: &Node<'_>,
    node_text: Option<&str>,
    _selectors: &Selectors,
) -> f64 {
    let text_length = match node_text {
        Some(t) => t.chars().count(),
        None => node.normalized_char_count(),
    };
    if text_length == 0 {
        return 0.0;
    }

    let mut link_length = 0.0;

    for link in node
        .descendants_it()
        .filter(|descendant| has_tag_name(descendant, "a"))
    {
        // Check href directly without allocating a new String
        let coefficient = match link.attr("href") {
            Some(href) if is_hash_url(href.as_ref()) => 0.3,
            _ => 1.0,
        };
        link_length += link.normalized_char_count() as f64 * coefficient;
    }

    link_length / text_length as f64
}

/// Get the link density of an element (ratio of link text to total text).
pub fn get_link_density(node: &Node<'_>, selectors: &Selectors) -> f64 {
    get_link_density_with_text(node, None, selectors)
}

/// Get the link density using a pre-computed parent text length.
/// Caches link text stats for efficiency.
pub fn get_link_density_cached(
    node: &Node<'_>,
    parent_text_length: usize,
    store: &mut NodeDataStore,
    _selectors: &Selectors,
) -> f64 {
    if parent_text_length == 0 {
        return 0.0;
    }

    let mut link_length = 0.0;

    for link in node
        .descendants_it()
        .filter(|descendant| has_tag_name(descendant, "a"))
    {
        // Get or compute stats for the link
        let link_stats = get_or_compute_stats(&link, store);

        // Check href directly without allocating a new String
        let coefficient = match link.attr("href") {
            Some(href) if is_hash_url(href.as_ref()) => 0.3,
            _ => 1.0,
        };
        link_length += link_stats.text_length as f64 * coefficient;
    }

    link_length / parent_text_length as f64
}

/// Check if a node is whitespace.
pub fn is_whitespace(node: &Node<'_>) -> bool {
    if node.is_text() {
        let text = node.text();
        return text.trim().is_empty();
    }
    if node.is_element()
        && let Some(tag) = get_tag_name(node)
    {
        return tag == "BR";
    }
    false
}

/// Check if a node qualifies as phrasing content.
pub fn is_phrasing_content(node: &Node<'_>) -> bool {
    is_phrasing_content_depth(node, 0)
}

fn is_phrasing_content_depth(node: &Node<'_>, depth: u32) -> bool {
    if node.is_text() {
        return true;
    }

    if let Some(tag) = get_tag_name(node) {
        if is_phrasing_elem(&tag) {
            return true;
        }

        // A, DEL, INS are phrasing content if all their children are.
        // Depth-limited to prevent excessive recursion on pathological DOMs.
        if (tag == "A" || tag == "DEL" || tag == "INS") && depth < 10 {
            return node
                .children()
                .iter()
                .all(|child| is_phrasing_content_depth(child, depth + 1));
        }
    }

    false
}

/// Wrap consecutive phrasing content in P tags by moving existing nodes.
/// This handles cases where text is placed directly inside DIVs without P tags.
pub fn wrap_phrasing_content_in_p(div: &Node<'_>) {
    let children: Vec<_> = div.children();
    let mut i = 0;

    while i < children.len() {
        let child = &children[i];

        // If this is phrasing content, collect consecutive phrasing content nodes
        if is_phrasing_content(child) {
            let mut phrasing_nodes = Vec::new();
            let mut j = i;

            // Collect all consecutive phrasing content
            while j < children.len() && is_phrasing_content(&children[j]) {
                phrasing_nodes.push(j);
                j += 1;
            }

            // Only wrap if we collected content (not just whitespace)
            let has_content = phrasing_nodes.iter().any(|&idx| {
                let n = &children[idx];
                if n.is_text() {
                    !n.text().trim().is_empty()
                } else {
                    true
                }
            });

            if has_content && !phrasing_nodes.is_empty() {
                // Trim leading/trailing whitespace using index tracking - O(n) instead of O(n²)
                let mut start = 0;
                let mut end = phrasing_nodes.len();

                // Trim leading whitespace nodes
                while start < end && is_whitespace(&children[phrasing_nodes[start]]) {
                    start += 1;
                }

                // Trim trailing whitespace nodes
                while start < end && is_whitespace(&children[phrasing_nodes[end - 1]]) {
                    end -= 1;
                }

                // Only wrap if we still have content after trimming
                if start < end {
                    let trimmed_nodes = &phrasing_nodes[start..end];
                    if let Some(first_node) = children.get(trimmed_nodes[0]) {
                        let p = div.tree.new_element("p");
                        first_node.insert_before(&p);

                        for &idx in trimmed_nodes {
                            if let Some(n) = children.get(idx) {
                                p.append_child(n);
                            }
                        }

                        for &idx in phrasing_nodes[..start]
                            .iter()
                            .chain(phrasing_nodes[end..].iter())
                        {
                            if let Some(n) = children.get(idx) {
                                n.remove_from_parent();
                            }
                        }
                    }
                }
            }

            i = j;
        } else {
            i += 1;
        }
    }
}

/// Check if an element has no content.
pub fn is_element_without_content(node: &Node<'_>) -> bool {
    if !node.is_element() {
        return false;
    }

    if has_non_whitespace_text(node) {
        return false;
    }

    let children = node.element_children();
    if children.is_empty() {
        return true;
    }

    // Check if all children are just BR or HR (uses element_children directly
    // to avoid the overhead of selector engine queries)
    children.iter().all(|child| {
        get_tag_name(child)
            .as_deref()
            .is_some_and(|tag| tag == "BR" || tag == "HR")
    })
}

/// Check if this node has only whitespace and a single element with given tag.
pub fn has_single_tag_inside_element(node: &Node<'_>, tag: &str) -> bool {
    let children = node.element_children();

    // There should be exactly 1 element child with given tag
    if children.len() != 1 {
        return false;
    }

    if let Some(child_tag) = get_tag_name(&children[0]) {
        if child_tag != tag {
            return false;
        }
    } else {
        return false;
    }

    // And there should be no text nodes with real content
    !node.children().iter().any(|child| {
        child.is_text()
            && child
                .text()
                .as_ref()
                .ends_with(|c: char| !c.is_whitespace())
    })
}

/// Check if an element has any children that are block-level elements.
pub fn has_child_block_element(node: &Node<'_>) -> bool {
    node.descendants_it()
        .filter(|child| child.is_element())
        .any(|child| get_tag_name(&child).is_some_and(|tag| is_div_to_p_elem(&tag)))
}

/// Check if a node is probably visible (not hidden).
pub fn is_probably_visible(node: &Node<'_>) -> bool {
    // Check style attribute for display:none or visibility:hidden,
    // ignoring case and whitespace variations. Both patterns are checked
    // in a single pass through the style string.
    if let Some(style) = node.attr("style") {
        let style_str = style.as_ref();
        if has_hidden_style(style_str) {
            return false;
        }
    }

    // Check for hidden attribute
    if node.has_attr("hidden") {
        return false;
    }

    // Check aria-hidden, but allow fallback-image class
    if let Some(aria_hidden) = node.attr("aria-hidden")
        && aria_hidden.as_ref() == "true"
    {
        if let Some(class) = node.attr("class") {
            if !class.as_ref().contains("fallback-image") {
                return false;
            }
        } else {
            return false;
        }
    }

    true
}

/// Check if a node is a valid byline element.
pub fn is_valid_byline(node: &Node<'_>, match_string: &str) -> bool {
    let is_byline_attr = node.attr("rel").is_some_and(|rel| rel.as_ref() == "author")
        || node
            .attr("itemprop")
            .is_some_and(|ip| ip.as_ref().contains("author"))
        || regexps::BYLINE.is_match(match_string);

    if !is_byline_attr {
        return false;
    }

    let text = node.text();
    let trimmed = text.trim();
    // Short-circuit: a UTF-8 char is at most 4 bytes, so < 400 bytes means < 100 chars.
    !trimmed.is_empty() && trimmed.len() < 400 && trimmed.chars().count() < 100
}

/// Check if node is image or contains exactly one image.
pub fn is_single_image(node: &Node<'_>) -> bool {
    let mut current = Some(*node);

    while let Some(n) = current {
        if let Some(tag) = get_tag_name(&n)
            && tag == "IMG"
        {
            return true;
        }

        let children = n.element_children();
        if children.len() != 1 || has_non_whitespace_text(&n) {
            return false;
        }

        current = children.into_iter().next();
    }

    false
}

/// Check whether a node or any descendant text node contains non-whitespace text.
/// This avoids constructing the full concatenated descendant text when callers only
/// need an emptiness check.
fn has_non_whitespace_text(node: &Node<'_>) -> bool {
    if node.is_text() {
        return node.text().chars().any(|c| !c.is_whitespace());
    }

    node.descendants_it().any(|descendant| {
        descendant.is_text() && descendant.text().chars().any(|c| !c.is_whitespace())
    })
}

/// Check if a style string contains "display:none" or "visibility:hidden"
/// (case-insensitive, whitespace-tolerant). Scans the string once for both patterns.
fn has_hidden_style(haystack: &str) -> bool {
    let hbytes = haystack.as_bytes();
    let hlen = hbytes.len();
    if hlen == 0 {
        return false;
    }

    // Both patterns contain no whitespace and are lowercase. We scan for
    // 'd' (display) or 'v' (visibility) as starting anchors.
    let display_pat: &[u8] = b"display:none";
    let vis_pat: &[u8] = b"visibility:hidden";

    let mut i = 0;
    while i < hlen {
        let b = hbytes[i].to_ascii_lowercase();
        let needle = if b == b'd' {
            display_pat
        } else if b == b'v' {
            vis_pat
        } else {
            i += 1;
            continue;
        };

        let needle_len = needle.len();
        if i + needle_len > hlen {
            i += 1;
            continue;
        }

        let mut hi = i;
        let mut ni = 0;
        let mut matches = true;
        while ni < needle_len && hi < hlen {
            if hbytes[hi].is_ascii_whitespace() {
                hi += 1;
                continue;
            }
            if hbytes[hi].to_ascii_lowercase() != needle[ni] {
                matches = false;
                break;
            }
            hi += 1;
            ni += 1;
        }
        if matches && ni == needle_len {
            return true;
        }
        i += 1;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use dom_query::Document;

    fn concatenated_stats(text: &str) -> NodeStats {
        let mut expected = NodeStats::default();
        let mut previous_was_whitespace = true;
        let mut last_was_dot = false;
        for c in text.chars() {
            if c.is_whitespace() {
                expected.has_sentence_end |= last_was_dot;
                last_was_dot = false;
                if !previous_was_whitespace {
                    expected.text_length += 1;
                    previous_was_whitespace = true;
                }
            } else {
                last_was_dot = c == '.';
                expected.comma_count += usize::from(
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
                expected.text_length += 1;
                previous_was_whitespace = false;
            }
        }
        if previous_was_whitespace && expected.text_length > 0 {
            expected.text_length -= 1;
        }
        expected.has_sentence_end |= last_was_dot;
        expected
    }

    fn assert_stats_match(node: &Node<'_>, store: &mut NodeDataStore) {
        let expected = concatenated_stats(&node.text());
        let actual = get_or_compute_stats(node, store);
        assert_eq!(actual.text_length, expected.text_length);
        assert_eq!(actual.comma_count, expected.comma_count);
        assert_eq!(actual.has_sentence_end, expected.has_sentence_end);
    }

    #[test]
    fn cached_node_stats_match_concatenated_text_semantics() {
        let cases = [
            "<div>  alpha,<span> beta.</span>\n<strong>gamma\u{060c}</strong>  </div>",
            "<div><span>not.</span><span>ended</span></div>",
            "<div><span>end.</span><i> </i><span>next</span></div>",
            "<div> \n <span>one</span><i></i> <b>two</b> </div>",
        ];

        for html in cases {
            let doc = Document::from(html);
            let node = doc.select("div").nodes().first().copied().unwrap();
            assert_stats_match(&node, &mut NodeDataStore::new());
        }
    }

    #[test]
    fn bottom_up_stats_match_large_mixed_subtree() {
        let mut html = String::from("<div> leading.");
        for index in 0..70 {
            match index % 4 {
                0 => html.push_str("<span> word,</span>"),
                1 => html.push_str("<i> </i>"),
                2 => html.push_str("<b>sentence.</b>\n"),
                _ => html.push_str("<em>joined</em><strong>text</strong>"),
            }
        }
        html.push_str(" trailing\u{060c}</div>");

        let doc = Document::from(html);
        let node = doc.select("div").nodes().first().copied().unwrap();
        assert!(node.descendants_it().count() > 64);
        assert_stats_match(&node, &mut NodeDataStore::new());
    }

    #[test]
    fn clearing_stats_recomputes_a_mutated_large_subtree() {
        let mut html = String::from("<div>");
        for _ in 0..70 {
            html.push_str("<span>cached text, </span>");
        }
        html.push_str("</div>");

        let doc = Document::from(html);
        let node = doc.select("div").nodes().first().copied().unwrap();
        let removed = doc.select("span").nodes().last().copied().unwrap();
        let mut store = NodeDataStore::new();

        let before = get_or_compute_stats(&node, &mut store);
        removed.remove_from_parent();
        let expected = concatenated_stats(&node.text());
        assert_ne!(before.text_length, expected.text_length);
        assert_eq!(
            get_or_compute_stats(&node, &mut store).text_length,
            before.text_length
        );

        store.clear_stats();
        assert_stats_match(&node, &mut store);
    }
}
