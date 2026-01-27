//! DOM cleaning functions for Readability.

use crate::constants::{
    DEPRECATED_SIZE_ATTRIBUTE_ELEMS, PRESENTATIONAL_ATTRIBUTES, flags::*, regexps,
};
use crate::dom::{NodeDataStore, get_tag_name, node_select};
use crate::scoring::{
    get_class_weight, get_inner_text, get_link_density, get_text_density,
    has_single_tag_inside_element, is_element_without_content, is_phrasing_content,
};
use dom_query::{Document, Node};
use regex::Regex;

/// Prepare the document for parsing by cleaning up styles, etc.
pub fn prep_document(doc: &Document) {
    // Remove all style tags in head
    let styles: Vec<_> = doc.select("style").nodes().to_vec();
    for style in styles {
        style.remove_from_parent();
    }

    // Replace double br's with p tags in body
    if let Some(body) = doc.select("body").nodes().first() {
        replace_brs(body);
    }

    // Replace font tags with span
    let fonts: Vec<_> = doc.select("font").nodes().to_vec();
    for font in fonts {
        font.rename("span");
    }
}

/// Get the next element node, skipping whitespace text nodes.
fn next_element<'a>(node: &Node<'a>) -> Option<Node<'a>> {
    let mut next = node.next_sibling();
    while let Some(ref n) = next {
        if n.is_element() {
            return next;
        }
        if n.is_text() && !regexps::WHITESPACE.is_match(n.text().as_ref()) {
            return None;
        }
        next = n.next_sibling();
    }
    None
}

/// Replace multiple <br> elements with <p> tags.
/// Consecutive BRs (with optional whitespace between) are converted to paragraph breaks.
/// Following phrasing content is moved into the new P until we hit a block element
/// or another BR pair.
fn replace_brs(elem: &Node<'_>) {
    let brs: Vec<_> = node_select(elem, "br").nodes().to_vec();

    for br in brs {
        // Check if this BR has been removed (as part of a previous BR chain)
        if br.parent().is_none() {
            continue;
        }

        let mut next = next_element(&br);
        let mut replaced = false;

        // Remove consecutive BR elements after this one
        while let Some(ref n) = next {
            if let Some(tag) = get_tag_name(n) {
                if tag == "BR" {
                    replaced = true;
                    let next_sibling = next_element(n);
                    n.remove_from_parent();
                    next = next_sibling;
                    continue;
                }
            }
            break;
        }

        // If we found consecutive BRs, replace this BR with a P and move following content into it
        if replaced {
            // Rename the BR to P
            br.rename("p");
            let p = br;

            // Move following phrasing content into the P
            let mut next = p.next_sibling();
            while let Some(n) = next {
                // If we hit another BR followed by BR, stop
                if n.is_element() {
                    if let Some(tag) = get_tag_name(&n) {
                        if tag == "BR" {
                            if let Some(next_elem) = next_element(&n) {
                                if let Some(next_tag) = get_tag_name(&next_elem) {
                                    if next_tag == "BR" {
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }

                // If this is not phrasing content, stop
                if !is_phrasing_content(&n) {
                    break;
                }

                // Move this node into the P
                let sibling = n.next_sibling();
                p.append_child(&n);
                next = sibling;
            }

            // Trim trailing whitespace text nodes from the P
            loop {
                if let Some(last) = p.children().last() {
                    if last.is_text() && regexps::WHITESPACE.is_match(last.text().as_ref()) {
                        last.remove_from_parent();
                        continue;
                    }
                }
                break;
            }

            // If the P is inside another P, convert the parent to DIV
            if let Some(parent) = p.parent() {
                if let Some(parent_tag) = get_tag_name(&parent) {
                    if parent_tag == "P" {
                        parent.rename("div");
                    }
                }
            }
        }
    }
}

/// Remove script and noscript tags from the document.
pub fn remove_scripts(doc: &Document) {
    let scripts: Vec<_> = doc.select("script, noscript").nodes().to_vec();
    for script in scripts {
        script.remove_from_parent();
    }
}

/// Clean an element of all tags of the given type.
pub fn clean(node: &Node<'_>, tag: &str, allowed_video_regex: &Regex) {
    let is_embed = tag == "object" || tag == "embed" || tag == "iframe";
    let elements: Vec<_> = node_select(node, tag).nodes().to_vec();

    for elem in elements {
        if is_embed {
            // Check if any attribute contains allowed video URL
            let attrs = elem.attrs();
            let mut keep = false;
            for attr in attrs.iter() {
                if allowed_video_regex.is_match(attr.value.as_ref()) {
                    keep = true;
                    break;
                }
            }
            if keep {
                continue;
            }

            // For object tags, also check innerHTML
            if tag == "object" {
                let inner = elem.inner_html();
                if allowed_video_regex.is_match(inner.as_ref()) {
                    continue;
                }
            }
        }

        elem.remove_from_parent();
    }
}

/// Remove presentational styles from an element and its children.
pub fn clean_styles(node: &Node<'_>) {
    if let Some(tag) = get_tag_name(node)
        && tag.to_lowercase() == "svg"
    {
        return;
    }

    // Remove presentational attributes
    for attr in PRESENTATIONAL_ATTRIBUTES.iter() {
        node.remove_attr(attr);
    }

    // Remove deprecated size attributes on certain elements
    if let Some(tag) = get_tag_name(node)
        && DEPRECATED_SIZE_ATTRIBUTE_ELEMS.contains(tag.as_str())
    {
        node.remove_attr("width");
        node.remove_attr("height");
    }

    // Clean children
    for child in node.element_children() {
        clean_styles(&child);
    }
}

/// Clean classes from an element, keeping only preserved classes.
pub fn clean_classes(node: &Node<'_>, classes_to_preserve: &[String]) {
    if let Some(class_attr) = node.attr("class") {
        let preserved: Vec<&str> = class_attr
            .split_whitespace()
            .filter(|c| classes_to_preserve.iter().any(|p| p == *c))
            .collect();

        if preserved.is_empty() {
            node.remove_attr("class");
        } else {
            node.set_attr("class", &preserved.join(" "));
        }
    }

    for child in node.element_children() {
        clean_classes(&child, classes_to_preserve);
    }
}

/// Clean spurious headers from an element.
pub fn clean_headers(node: &Node<'_>, flags: u32) {
    let headings: Vec<_> = node_select(node, "h1, h2").nodes().to_vec();
    let to_remove: Vec<_> = headings
        .iter()
        .filter(|h| get_class_weight(h, flags) < 0)
        .map(|h| h.id)
        .collect();

    for heading in headings {
        if to_remove.contains(&heading.id) {
            heading.remove_from_parent();
        }
    }
}

/// Clean elements conditionally based on content analysis.
pub fn clean_conditionally(
    node: &Node<'_>,
    tag: &str,
    flags: u32,
    allowed_video_regex: &Regex,
    store: &NodeDataStore,
    link_density_modifier: f64,
) {
    if (flags & FLAG_CLEAN_CONDITIONALLY) == 0 {
        return;
    }

    let elements: Vec<_> = node_select(node, tag).nodes().to_vec();
    let to_remove: Vec<_> = elements
        .iter()
        .filter(|elem| {
            should_remove_conditionally(
                elem,
                tag,
                flags,
                allowed_video_regex,
                store,
                link_density_modifier,
            )
        })
        .map(|e| e.id)
        .collect();

    for elem in elements {
        if to_remove.contains(&elem.id) {
            elem.remove_from_parent();
        }
    }
}

fn should_remove_conditionally(
    node: &Node<'_>,
    tag: &str,
    flags: u32,
    allowed_video_regex: &Regex,
    store: &NodeDataStore,
    link_density_modifier: f64,
) -> bool {
    // Check if this is a data table
    if tag == "table"
        && let Some(is_data_table) = store.is_data_table(&node.id)
        && is_data_table
    {
        return false;
    }

    // Check if inside a data table
    let mut parent = node.parent();
    while let Some(p) = parent {
        if let Some(ptag) = get_tag_name(&p)
            && ptag == "TABLE"
            && let Some(is_data_table) = store.is_data_table(&p.id)
            && is_data_table
        {
            return false;
        }
        parent = p.parent();
    }

    // Check if inside code element
    let mut parent = node.parent();
    let mut depth = 0;
    while let Some(p) = parent {
        if depth > 3 {
            break;
        }
        if let Some(ptag) = get_tag_name(&p)
            && ptag == "CODE"
        {
            return false;
        }
        parent = p.parent();
        depth += 1;
    }

    // Check if this element contains data tables
    for table in node_select(node, "table").nodes().iter() {
        if let Some(is_data_table) = store.is_data_table(&table.id)
            && is_data_table
        {
            return false;
        }
    }

    let is_list = tag == "ul" || tag == "ol";
    let is_list = if !is_list {
        // Check if this element is mostly a list
        let list_nodes = node_select(node, "ul, ol");
        let mut list_length = 0;
        for list in list_nodes.nodes().iter() {
            list_length += get_inner_text(list, true).len();
        }
        let inner_text = get_inner_text(node, true);
        if !inner_text.is_empty() {
            list_length as f64 / inner_text.len() as f64 > 0.9
        } else {
            false
        }
    } else {
        is_list
    };

    let weight = get_class_weight(node, flags);

    if weight < 0 {
        return true;
    }

    // Check comma count
    let comma_count = get_inner_text(node, true).matches(',').count();
    if comma_count >= 10 {
        return false;
    }

    // Various content-based checks
    let p_count = node_select(node, "p").length();
    let img_count = node_select(node, "img").length();
    let li_count = node_select(node, "li").length().saturating_sub(100);
    let input_count = node_select(node, "input").length();
    let heading_density = get_text_density(node, &["h1", "h2", "h3", "h4", "h5", "h6"]);

    let mut embed_count = 0;
    let embeds = node_select(node, "object, embed, iframe");
    for embed in embeds.nodes().iter() {
        // Check if embed has allowed video URL
        let attrs = embed.attrs();
        let mut has_video = false;
        for attr in attrs.iter() {
            if allowed_video_regex.is_match(attr.value.as_ref()) {
                has_video = true;
                break;
            }
        }
        if has_video {
            return false;
        }

        // Check object innerHTML
        if let Some(etag) = get_tag_name(embed)
            && etag == "OBJECT"
            && allowed_video_regex.is_match(embed.inner_html().as_ref())
        {
            return false;
        }

        embed_count += 1;
    }

    let inner_text = get_inner_text(node, true);

    // Check for ad/loading words
    if regexps::AD_WORDS.is_match(&inner_text) || regexps::LOADING_WORDS.is_match(&inner_text) {
        return true;
    }

    let content_length = inner_text.len();
    let link_density = get_link_density(node);

    let textish_tags = [
        "SPAN",
        "LI",
        "TD",
        "BLOCKQUOTE",
        "DL",
        "DIV",
        "IMG",
        "OL",
        "P",
        "PRE",
        "TABLE",
        "UL",
    ];
    let text_density = get_text_density(node, &textish_tags);

    // Check if this is a child of figure
    let is_figure_child = {
        let mut parent = node.parent();
        let mut depth = 0;
        let mut result = false;
        while let Some(p) = parent {
            if depth > 3 {
                break;
            }
            if let Some(ptag) = get_tag_name(&p)
                && ptag == "FIGURE"
            {
                result = true;
                break;
            }
            parent = p.parent();
            depth += 1;
        }
        result
    };

    // Apply removal checks - combine conditions since they all result in removal
    let have_to_remove =
        // Bad p to img ratio
        (!is_figure_child && img_count > 1 && (p_count as f64 / img_count as f64) < 0.5)
        // Too many li's outside a list
        || (!is_list && li_count > p_count)
        // Too many inputs
        || (input_count > p_count / 3)
        // Suspiciously short content
        || (!is_list
            && !is_figure_child
            && heading_density < 0.9
            && content_length < 25
            && (img_count == 0 || img_count > 2)
            && link_density > 0.0)
        // Low weight and linky
        || (!is_list && weight < 25 && link_density > 0.2 + link_density_modifier)
        // High weight but mostly links
        || (weight >= 25 && link_density > 0.5 + link_density_modifier)
        // Suspicious embed
        || ((embed_count == 1 && content_length < 75) || embed_count > 1)
        // No useful content
        || (img_count == 0 && text_density == 0.0);

    // Allow simple lists of images to remain
    if is_list && have_to_remove {
        let children = node.element_children();
        for child in &children {
            if child.element_children().len() > 1 {
                return have_to_remove;
            }
        }
        let li_count = node_select(node, "li").length();
        if img_count == li_count {
            return false;
        }
    }

    have_to_remove
}

/// Mark tables as data tables or layout tables.
pub fn mark_data_tables(root: &Node<'_>, store: &mut NodeDataStore) {
    let tables: Vec<_> = node_select(root, "table").nodes().to_vec();

    for table in tables {
        // Check role="presentation"
        if let Some(role) = table.attr("role")
            && role.as_ref() == "presentation"
        {
            store.set_data_table(table.id, false);
            continue;
        }

        // Check datatable="0"
        if let Some(datatable) = table.attr("datatable")
            && datatable.as_ref() == "0"
        {
            store.set_data_table(table.id, false);
            continue;
        }

        // Has summary attribute
        if table.has_attr("summary") {
            store.set_data_table(table.id, true);
            continue;
        }

        // Has caption with content
        let captions = node_select(&table, "caption");
        if captions.length() > 0
            && let Some(caption) = captions.nodes().first()
            && !caption.children().is_empty()
        {
            store.set_data_table(table.id, true);
            continue;
        }

        // Has data-related descendants
        let data_descendants = ["col", "colgroup", "tfoot", "thead", "th"];
        let has_data_descendant = data_descendants
            .iter()
            .any(|tag| node_select(&table, tag).length() > 0);

        if has_data_descendant {
            store.set_data_table(table.id, true);
            continue;
        }

        // Has nested table - it's a layout table
        if node_select(&table, "table").length() > 0 {
            store.set_data_table(table.id, false);
            continue;
        }

        // Count rows and columns
        let (rows, columns) = get_row_and_column_count(&table);

        if columns == 1 || rows == 1 {
            store.set_data_table(table.id, false);
            continue;
        }

        if rows >= 10 || columns > 4 {
            store.set_data_table(table.id, true);
            continue;
        }

        store.set_data_table(table.id, rows * columns > 10);
    }
}

fn get_row_and_column_count(table: &Node<'_>) -> (usize, usize) {
    let mut rows = 0;
    let mut columns = 0;

    for tr in node_select(table, "tr").nodes().iter() {
        let rowspan = tr
            .attr("rowspan")
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(1);
        rows += rowspan;

        let mut cols_in_row = 0;
        for td in node_select(tr, "td").nodes().iter() {
            let colspan = td
                .attr("colspan")
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(1);
            cols_in_row += colspan;
        }
        columns = columns.max(cols_in_row);
    }

    (rows, columns)
}

/// Fix lazy-loaded images.
pub fn fix_lazy_images(root: &Node<'_>) {
    let elems: Vec<_> = node_select(root, "img, picture, figure").nodes().to_vec();

    for elem in elems {
        // Check for base64 placeholder images
        if let Some(src) = elem.attr("src")
            && let Some(caps) = regexps::B64_DATA_URL.captures(src.as_ref())
        {
            let mime = caps.get(1).map(|m| m.as_str()).unwrap_or("");

            // Skip SVG as they can be meaningful at small sizes
            if mime == "image/svg+xml" {
                continue;
            }

            // Check if there are other image attributes
            let mut src_could_be_removed = false;
            let attrs = elem.attrs();
            for attr in attrs.iter() {
                if attr.name.local.as_ref() == "src" {
                    continue;
                }
                if regexps::IMAGE_EXTENSION.is_match(attr.value.as_ref()) {
                    src_could_be_removed = true;
                    break;
                }
            }

            // Remove small placeholder images
            if src_could_be_removed {
                let b64_start = caps.get(0).map(|m| m.end()).unwrap_or(0);
                let b64_length = src.len() - b64_start;
                if b64_length < 133 {
                    elem.remove_attr("src");
                }
            }
        }

        // Check if already has src/srcset and not lazy
        let has_src = elem.attr("src").is_some();
        let has_srcset = elem
            .attr("srcset")
            .map(|s| !s.is_empty() && s.as_ref() != "null")
            .unwrap_or(false);
        let has_lazy_class = elem
            .attr("class")
            .map(|c| c.to_lowercase().contains("lazy"))
            .unwrap_or(false);

        if (has_src || has_srcset) && !has_lazy_class {
            continue;
        }

        // Look for image URLs in other attributes
        let attrs = elem.attrs();
        for attr in attrs.iter() {
            let name = attr.name.local.as_ref();
            if name == "src" || name == "srcset" || name == "alt" {
                continue;
            }

            let value = attr.value.as_ref();
            let copy_to = if regexps::IMAGE_SRCSET.is_match(value) {
                Some("srcset")
            } else if regexps::IMAGE_SRC.is_match(value) {
                Some("src")
            } else {
                None
            };

            if let Some(target) = copy_to
                && let Some(tag) = get_tag_name(&elem)
            {
                if tag == "IMG" || tag == "PICTURE" {
                    elem.set_attr(target, value);
                } else if tag == "FIGURE" {
                    // Create img if figure doesn't have one
                    if node_select(&elem, "img, picture").length() == 0 {
                        let html = format!("<img {}=\"{}\">", target, value);
                        elem.set_html(html.as_str());
                    }
                }
            }
        }
    }
}

/// Unwrap images from noscript tags.
pub fn unwrap_noscript_images(doc: &Document) {
    // First, remove images without useful sources
    let imgs: Vec<_> = doc.select("img").nodes().to_vec();
    for img in imgs {
        let mut has_useful_attr = false;
        let attrs = img.attrs();
        for attr in attrs.iter() {
            let name = attr.name.local.as_ref();
            match name {
                "src" | "srcset" | "data-src" | "data-srcset" => {
                    has_useful_attr = true;
                    break;
                }
                _ => {
                    if regexps::IMAGE_EXTENSION.is_match(attr.value.as_ref()) {
                        has_useful_attr = true;
                        break;
                    }
                }
            }
        }
        if !has_useful_attr {
            img.remove_from_parent();
        }
    }

    // Process noscript tags - simplified version
    let noscripts: Vec<_> = doc.select("noscript").nodes().to_vec();
    for noscript in noscripts {
        use crate::scoring::is_single_image;

        if !is_single_image(&noscript) {
            continue;
        }

        // Get the previous element sibling
        let prev_element = {
            let mut prev = noscript.prev_sibling();
            while let Some(ref p) = prev {
                if p.is_element() {
                    break;
                }
                prev = p.prev_sibling();
            }
            prev
        };

        if let Some(prev) = prev_element
            && is_single_image(&prev)
        {
            // Replace previous element with noscript content
            let noscript_html = noscript.inner_html();
            prev.set_html(noscript_html.as_ref());
            noscript.remove_from_parent();
        }
    }
}

/// Simplify nested elements by removing empty ones and unwrapping single-child containers.
pub fn simplify_nested_elements(article_content: &Node<'_>) {
    let to_process: Vec<_> = article_content.descendants_it().collect();

    for node in to_process {
        if !node.is_element() {
            continue;
        }

        if let Some(tag) = get_tag_name(&node)
            && (tag == "DIV" || tag == "SECTION")
        {
            // Don't touch readability-generated IDs
            if let Some(id) = node.attr("id")
                && id.starts_with("readability")
            {
                continue;
            }

            if is_element_without_content(&node) {
                node.remove_from_parent();
                continue;
            }

            if has_single_tag_inside_element(&node, "DIV")
                || has_single_tag_inside_element(&node, "SECTION")
            {
                // Unwrap the single child
                let children = node.element_children();
                if let Some(child) = children.first() {
                    let child_html = child.inner_html();
                    node.set_html(child_html.as_ref());
                }
            }
        }
    }
}

/// Clean matched nodes based on a filter function.
pub fn clean_matched_nodes<F>(node: &Node<'_>, filter: F)
where
    F: Fn(&Node<'_>, &str) -> bool,
{
    let descendants: Vec<_> = node.descendants_it().collect();

    for n in descendants {
        if !n.is_element() {
            continue;
        }

        let class = n.attr("class").map(|s| s.to_string()).unwrap_or_default();
        let id = n.attr("id").map(|s| s.to_string()).unwrap_or_default();
        let match_string = format!("{} {}", class, id);

        if filter(&n, &match_string) {
            n.remove_from_parent();
        }
    }
}
