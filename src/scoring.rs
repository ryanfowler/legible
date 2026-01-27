//! Content scoring logic for Readability.

use crate::constants::{PHRASING_ELEMS, flags::*, regexps};
use crate::dom::{NodeDataStore, get_tag_name};
use dom_query::Node;

/// Initialize a node with readability data and initial score based on tag.
pub fn initialize_node(node: &Node<'_>, store: &mut NodeDataStore, flags: u32) {
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
    store.set(
        node.id,
        crate::dom::ReadabilityData::with_score(initial_score + class_weight as f64),
    );
}

/// Get the class/id weight of an element.
/// Positive weight for content-like classes, negative for non-content.
pub fn get_class_weight(node: &Node<'_>, flags: u32) -> i32 {
    if (flags & FLAG_WEIGHT_CLASSES) == 0 {
        return 0;
    }

    let mut weight: i32 = 0;

    // Check class name
    if let Some(class_name) = node.attr("class") {
        let class_str = class_name.as_ref();
        if !class_str.is_empty() {
            if regexps::NEGATIVE.is_match(class_str) {
                weight -= 25;
            }
            if regexps::POSITIVE.is_match(class_str) {
                weight += 25;
            }
        }
    }

    // Check ID
    if let Some(id) = node.attr("id") {
        let id_str = id.as_ref();
        if !id_str.is_empty() {
            if regexps::NEGATIVE.is_match(id_str) {
                weight -= 25;
            }
            if regexps::POSITIVE.is_match(id_str) {
                weight += 25;
            }
        }
    }

    weight
}

/// Get the inner text of a node, optionally normalizing whitespace.
pub fn get_inner_text(node: &Node<'_>, normalize_spaces: bool) -> String {
    let text = node.text().trim().to_string();
    if normalize_spaces {
        regexps::NORMALIZE.replace_all(&text, " ").to_string()
    } else {
        text
    }
}

/// Get the link density of an element (ratio of link text to total text).
pub fn get_link_density(node: &Node<'_>) -> f64 {
    use crate::dom::node_select;

    let text_length = get_inner_text(node, true).len();
    if text_length == 0 {
        return 0.0;
    }

    let mut link_length = 0.0;

    for link in node_select(node, "a").nodes().iter() {
        let href = link.attr("href").map(|s| s.to_string()).unwrap_or_default();
        // Hash URLs count less
        let coefficient = if regexps::HASH_URL.is_match(&href) {
            0.3
        } else {
            1.0
        };
        link_length += get_inner_text(link, true).len() as f64 * coefficient;
    }

    link_length / text_length as f64
}

/// Get the text density for specific tags within an element.
pub fn get_text_density(node: &Node<'_>, tags: &[&str]) -> f64 {
    use crate::dom::node_select;

    let text_length = get_inner_text(node, true).len();
    if text_length == 0 {
        return 0.0;
    }

    let mut children_length = 0;
    let selector = tags.join(",");

    for child in node_select(node, &selector).nodes().iter() {
        children_length += get_inner_text(child, true).len();
    }

    children_length as f64 / text_length as f64
}

/// Get the comma count in an element's text.
/// Returns comma count + 1 to match JS split().length behavior.
pub fn get_comma_count(node: &Node<'_>) -> usize {
    let text = get_inner_text(node, true);
    // JS uses split(commas).length which returns segments (commas + 1)
    regexps::COMMAS.find_iter(&text).count() + 1
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
    if node.is_text() {
        return true;
    }

    if let Some(tag) = get_tag_name(node) {
        if PHRASING_ELEMS.contains(tag.as_str()) {
            return true;
        }

        // A, DEL, INS are phrasing content if all their children are
        if tag == "A" || tag == "DEL" || tag == "INS" {
            return node
                .children()
                .iter()
                .all(|child| is_phrasing_content(child));
        }
    }

    false
}

/// Check if an element has no content.
pub fn is_element_without_content(node: &Node<'_>) -> bool {
    use crate::dom::node_select;

    if !node.is_element() {
        return false;
    }

    let text = node.text().trim().to_string();
    if !text.is_empty() {
        return false;
    }

    let children = node.element_children();
    if children.is_empty() {
        return true;
    }

    // Check if all children are just BR or HR
    let br_count = node_select(node, "br").length();
    let hr_count = node_select(node, "hr").length();

    children.len() == br_count + hr_count
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
    !node
        .children()
        .iter()
        .any(|child| child.is_text() && regexps::HAS_CONTENT.is_match(child.text().as_ref()))
}

/// Check if an element has any children that are block-level elements.
pub fn has_child_block_element(node: &Node<'_>) -> bool {
    use crate::constants::DIV_TO_P_ELEMS;

    for child in node.children() {
        if let Some(tag) = get_tag_name(&child)
            && DIV_TO_P_ELEMS.contains(tag.as_str())
        {
            return true;
        }
        if child.is_element() && has_child_block_element(&child) {
            return true;
        }
    }

    false
}

/// Check if a node is probably visible (not hidden).
pub fn is_probably_visible(node: &Node<'_>) -> bool {
    // Check style attribute for display: none or visibility: hidden
    if let Some(style) = node.attr("style") {
        let style_str = style.as_ref();
        if style_str.contains("display:none") || style_str.contains("display: none") {
            return false;
        }
        if style_str.contains("visibility:hidden") || style_str.contains("visibility: hidden") {
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
    // Check rel="author"
    if let Some(rel) = node.attr("rel")
        && rel.as_ref() == "author"
    {
        let text = node.text().trim().to_string();
        return !text.is_empty() && text.len() < 100;
    }

    // Check itemprop containing "author"
    if let Some(itemprop) = node.attr("itemprop")
        && itemprop.as_ref().contains("author")
    {
        let text = node.text().trim().to_string();
        return !text.is_empty() && text.len() < 100;
    }

    // Check byline pattern in class/id
    if regexps::BYLINE.is_match(match_string) {
        let text = node.text().trim().to_string();
        return !text.is_empty() && text.len() < 100;
    }

    false
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
        if children.len() != 1 || !n.text().trim().is_empty() {
            return false;
        }

        current = children.into_iter().next();
    }

    false
}
