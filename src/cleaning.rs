//! DOM cleaning functions for Readability.

use std::borrow::Cow;

use crate::constants::{
    DEPRECATED_SIZE_ATTRIBUTE_ELEMS, PRESENTATIONAL_ATTRIBUTES, flags::*, has_image_extension,
    has_image_src, has_image_srcset, regexps,
};
use crate::dom::{
    NodeDataStore, build_match_string, get_tag_name, node_select, node_select_matcher,
};
use crate::scoring::{
    get_class_weight, get_link_density_cached, get_or_compute_stats,
    get_or_compute_stats_with_text, get_text_density_cached, has_single_tag_inside_element,
    is_element_without_content, is_phrasing_content,
};
use crate::selectors::Selectors;
use dom_query::{Document, Node};
use regex::Regex;

/// Prepare the document for parsing by cleaning up styles, etc.
pub fn prep_document(doc: &Document, selectors: &Selectors) {
    // Remove all style tags in head
    let styles: Vec<_> = doc.select_matcher(&selectors.style).nodes().to_vec();
    for style in styles {
        style.remove_from_parent();
    }

    // Replace double br's with p tags in body
    if let Some(body) = doc.select_matcher(&selectors.body).nodes().first() {
        replace_brs(body, selectors);
    }

    // Replace font tags with span
    let fonts: Vec<_> = doc.select_matcher(&selectors.font).nodes().to_vec();
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
        if n.is_text() && !n.text().trim().is_empty() {
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
fn replace_brs(elem: &Node<'_>, selectors: &Selectors) {
    let brs: Vec<_> = node_select_matcher(elem, &selectors.br).nodes().to_vec();

    for br in brs {
        // Check if this BR has been removed (as part of a previous BR chain)
        if br.parent().is_none() {
            continue;
        }

        let mut next = next_element(&br);
        let mut replaced = false;

        // Remove consecutive BR elements after this one
        while let Some(ref n) = next {
            if let Some(tag) = get_tag_name(n)
                && tag == "BR"
            {
                replaced = true;
                let next_sibling = next_element(n);
                n.remove_from_parent();
                next = next_sibling;
                continue;
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
                if n.is_element()
                    && let Some(tag) = get_tag_name(&n)
                    && tag == "BR"
                    && let Some(next_elem) = next_element(&n)
                    && let Some(next_tag) = get_tag_name(&next_elem)
                    && next_tag == "BR"
                {
                    break;
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
                if let Some(last) = p.children().last()
                    && last.is_text()
                    && last.text().trim().is_empty()
                {
                    last.remove_from_parent();
                    continue;
                }
                break;
            }

            // If the P is inside another P, convert the parent to DIV
            if let Some(parent) = p.parent()
                && let Some(parent_tag) = get_tag_name(&parent)
                && parent_tag == "P"
            {
                parent.rename("div");
            }
        }
    }
}

/// Remove script and noscript tags from the document.
pub fn remove_scripts(doc: &Document, selectors: &Selectors) {
    let scripts: Vec<_> = doc
        .select_matcher(&selectors.script_noscript)
        .nodes()
        .to_vec();
    for script in scripts {
        script.remove_from_parent();
    }
}

/// Clean multiple tag types in a single descendant traversal.
/// Embed-like tags (object, embed, iframe) get special handling for allowed video URLs.
pub fn clean_tags(node: &Node<'_>, tags: &[&str], allowed_video_regex: &Regex) {
    let elements: Vec<_> = node.descendants_it().filter(|n| n.is_element()).collect();

    for elem in elements {
        let tag = match get_tag_name(&elem) {
            Some(t) => t,
            None => continue,
        };

        // Check if this tag is in our target list (case-insensitive without allocating)
        if !tags.iter().any(|&t| t.eq_ignore_ascii_case(&tag)) {
            continue;
        }

        let is_embed = tag == "OBJECT" || tag == "EMBED" || tag == "IFRAME";
        if is_embed {
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

            if tag == "OBJECT" {
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
    let nodes: Vec<_> = std::iter::once(*node)
        .chain(node.descendants_it())
        .collect();

    for current in nodes {
        if !current.is_element() {
            continue;
        }

        if let Some(tag) = get_tag_name(&current)
            && tag == "SVG"
        {
            continue;
        }

        // Remove presentational attributes
        for attr in PRESENTATIONAL_ATTRIBUTES.iter() {
            current.remove_attr(attr);
        }

        // Remove deprecated size attributes on certain elements
        if let Some(tag) = get_tag_name(&current)
            && DEPRECATED_SIZE_ATTRIBUTE_ELEMS.contains(&*tag)
        {
            current.remove_attr("width");
            current.remove_attr("height");
        }
    }
}

/// Clean spurious headers from an element.
pub fn clean_headers(node: &Node<'_>, flags: u32, selectors: &Selectors) {
    // Collect only the headings that need removal, avoiding intermediate Vec
    let to_remove: Vec<_> = node_select_matcher(node, &selectors.h1_h2)
        .nodes()
        .iter()
        .filter(|h| get_class_weight(h, flags) < 0)
        .cloned()
        .collect();

    for heading in to_remove {
        heading.remove_from_parent();
    }
}

/// Clean elements conditionally based on content analysis.
pub fn clean_conditionally(
    node: &Node<'_>,
    tag: &str,
    flags: u32,
    allowed_video_regex: &Regex,
    store: &mut NodeDataStore,
    link_density_modifier: f64,
    selectors: &Selectors,
) {
    if (flags & FLAG_CLEAN_CONDITIONALLY) == 0 {
        return;
    }

    // Collect elements first, then process in reverse order like JS does.
    // Important: We evaluate and remove one at a time so that removing a
    // nested element affects the counts for parent elements.
    let elements: Vec<_> = node_select(node, tag).nodes().to_vec();

    // Process in reverse order (back to front) like JavaScript
    for elem in elements.into_iter().rev() {
        // Skip if element was already removed (e.g., its parent was removed)
        if elem.parent().is_none() {
            continue;
        }

        if should_remove_conditionally(
            &elem,
            tag,
            flags,
            allowed_video_regex,
            store,
            link_density_modifier,
            selectors,
        ) {
            elem.remove_from_parent();
        }
    }
}

fn should_remove_conditionally(
    node: &Node<'_>,
    tag: &str,
    flags: u32,
    allowed_video_regex: &Regex,
    store: &mut NodeDataStore,
    link_density_modifier: f64,
    selectors: &Selectors,
) -> bool {
    // Check if this is a data table
    if tag == "table"
        && let Some(is_data_table) = store.is_data_table(&node.id)
        && is_data_table
    {
        return false;
    }

    // Combined parent chain walk for data table and code checks (Phase 4.2)
    // This single walk checks multiple conditions instead of walking ancestors twice
    let mut parent = node.parent();
    let mut depth = 0;
    while let Some(p) = parent {
        if let Some(ptag) = get_tag_name(&p) {
            // Check if inside a data table (no depth limit)
            if ptag == "TABLE"
                && let Some(is_data_table) = store.is_data_table(&p.id)
                && is_data_table
            {
                return false;
            }
            // Check if inside code element (depth limit of 3)
            if depth <= 3 && ptag == "CODE" {
                return false;
            }
        }
        parent = p.parent();
        depth += 1;
    }

    // Check if this element contains data tables
    for table in node_select_matcher(node, &selectors.table).nodes().iter() {
        if let Some(is_data_table) = store.is_data_table(&table.id)
            && is_data_table
        {
            return false;
        }
    }

    // Get or compute cached stats for this node, also returning the inner text
    // Get or compute cached stats for this node, also returning the inner text
    // to avoid a redundant get_inner_text call for the AD_LOADING_SET check below.
    let (stats, inner_text) = get_or_compute_stats_with_text(node, store);
    let content_length = stats.text_length;

    let is_list = tag == "ul" || tag == "ol";
    let is_list = if !is_list {
        // Check if this element is mostly a list
        let list_nodes = node_select_matcher(node, &selectors.ul_ol);
        let mut list_length = 0;
        for list in list_nodes.nodes().iter() {
            let list_stats = get_or_compute_stats(list, store);
            list_length += list_stats.text_length;
        }
        if content_length > 0 {
            list_length as f64 / content_length as f64 > 0.9
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

    // Check comma count using cached stats
    // Note: stats.comma_count is the raw count, check >= 10
    if stats.comma_count >= 10 {
        return false;
    }

    // Various content-based checks and embed video checks in a single traversal
    let mut p_count = 0usize;
    let mut img_count = 0usize;
    let mut li_count = 0usize;
    let mut input_count = 0usize;
    let mut embed_count = 0usize;

    for descendant in node.descendants_it() {
        if let Some(desc_tag) = get_tag_name(&descendant) {
            match &*desc_tag {
                "P" => p_count += 1,
                "IMG" => img_count += 1,
                "LI" => li_count += 1,
                "INPUT" => input_count += 1,
                "OBJECT" | "EMBED" | "IFRAME" => {
                    let attrs = descendant.attrs();
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
                    if desc_tag == "OBJECT"
                        && allowed_video_regex.is_match(descendant.inner_html().as_ref())
                    {
                        return false;
                    }
                    embed_count += 1;
                }
                _ => {}
            }
        }
    }
    let li_count = li_count.saturating_sub(100);

    let heading_density = get_text_density_cached(node, content_length, &selectors.headings, store);

    // Check for ad/loading words - use RegexSet for single-pass matching
    if regexps::AD_LOADING_SET.is_match(&inner_text) {
        return true;
    }

    let link_density = get_link_density_cached(node, content_length, store, selectors);

    let text_density =
        get_text_density_cached(node, content_length, &selectors.textish_tags, store);

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
        let li_count = node_select_matcher(node, &selectors.li).length();
        if img_count == li_count {
            return false;
        }
    }

    have_to_remove
}

/// Mark tables as data tables or layout tables.
pub fn mark_data_tables(root: &Node<'_>, store: &mut NodeDataStore, selectors: &Selectors) {
    let tables: Vec<_> = node_select_matcher(root, &selectors.table).nodes().to_vec();

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
        let captions = node_select_matcher(&table, &selectors.caption);
        if captions.length() > 0
            && let Some(caption) = captions.nodes().first()
            && !caption.children().is_empty()
        {
            store.set_data_table(table.id, true);
            continue;
        }

        // Has data-related descendants
        let has_data_descendant =
            node_select_matcher(&table, &selectors.table_data_elements).length() > 0;

        if has_data_descendant {
            store.set_data_table(table.id, true);
            continue;
        }

        // Has nested table - it's a layout table
        if node_select_matcher(&table, &selectors.table).length() > 0 {
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

    // Use element_children() instead of selector queries for better performance.
    // Tables can have TBODY, THEAD, etc. as direct children, so we need to handle that.
    for child in table.element_children() {
        if let Some(tag) = get_tag_name(&child) {
            match &*tag {
                "TR" => {
                    let (row_count, col_count) = count_row(&child);
                    rows += row_count;
                    columns = columns.max(col_count);
                }
                "TBODY" | "THEAD" | "TFOOT" => {
                    // Process TRs inside these wrapper elements
                    for tr_child in child.element_children() {
                        if let Some(tr_tag) = get_tag_name(&tr_child)
                            && &*tr_tag == "TR"
                        {
                            let (row_count, col_count) = count_row(&tr_child);
                            rows += row_count;
                            columns = columns.max(col_count);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    (rows, columns)
}

/// Count rows (from rowspan) and columns (from cells) in a single TR element.
fn count_row(tr: &Node<'_>) -> (usize, usize) {
    let rowspan = tr
        .attr("rowspan")
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(1);

    let mut cols_in_row = 0;
    for cell in tr.element_children() {
        if let Some(cell_tag) = get_tag_name(&cell)
            && (cell_tag == "TD" || cell_tag == "TH")
        {
            let colspan = cell
                .attr("colspan")
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(1);
            cols_in_row += colspan;
        }
    }

    (rowspan, cols_in_row)
}

/// Fix lazy-loaded images.
pub fn fix_lazy_images(root: &Node<'_>, selectors: &Selectors) {
    let elems: Vec<_> = node_select_matcher(root, &selectors.img_picture_figure)
        .nodes()
        .to_vec();

    for elem in elems {
        let tag = get_tag_name(&elem);

        // Check for base64 placeholder images (uses the `src` attribute)
        let mut has_b64_placeholder = false;
        let mut has_other_img_attr = false;

        // Single pass: gather all relevant information from attributes
        let mut has_src = false;
        let mut has_srcset = false;
        let mut has_lazy_class = false;
        let mut lazy_src: Option<&str> = None;
        let mut lazy_srcset: Option<&str> = None;

        let attrs = elem.attrs();
        for attr in attrs.iter() {
            let name = attr.name.local.as_ref();
            let value = attr.value.as_ref();

            match name {
                "src" => {
                    has_src = !value.is_empty();
                    if let Some(caps) = regexps::B64_DATA_URL.captures(value) {
                        let mime = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                        if mime != "image/svg+xml" {
                            has_b64_placeholder = true;
                        }
                    }
                }
                "srcset" => {
                    has_srcset = !value.is_empty() && value != "null";
                }
                "class" => {
                    has_lazy_class = value
                        .split_whitespace()
                        .any(|cls| cls.eq_ignore_ascii_case("lazy"));
                }
                _ => {
                    if has_image_extension(value) {
                        has_other_img_attr = true;
                    }
                    // Check for lazy-load image URLs in other attributes
                    // Last matching value wins (mimics JS, where later iterations overwrite)
                    if has_image_srcset(value) {
                        lazy_srcset = Some(value);
                    } else if has_image_src(value) {
                        lazy_src = Some(value);
                    }
                }
            }
        }

        // Handle base64 placeholder
        if has_b64_placeholder && has_other_img_attr {
            // Re-check b64 length (need the src value again)
            if let Some(src) = elem.attr("src")
                && let Some(caps) = regexps::B64_DATA_URL.captures(src.as_ref())
            {
                let b64_start = caps.get(0).map(|m| m.end()).unwrap_or(0);
                let b64_length = src.len() - b64_start;
                if b64_length < 133 {
                    elem.remove_attr("src");
                    has_src = false;
                }
            }
        }

        if (has_src || has_srcset) && !has_lazy_class {
            continue;
        }

        // Apply the lazy image URL if found
        if let Some(value) = lazy_srcset
            && let Some(ref tag_str) = tag
        {
            let target = "srcset";
            if tag_str == "IMG" || tag_str == "PICTURE" {
                elem.set_attr(target, value);
            } else if tag_str == "FIGURE"
                && node_select_matcher(&elem, &selectors.img_picture).length() == 0
            {
                let escaped = escape_html_attr(value);
                let html = format!("<img {}=\"{}\">", target, escaped);
                elem.set_html(html.as_str());
            }
        } else if let Some(value) = lazy_src
            && let Some(ref tag_str) = tag
        {
            let target = "src";
            if tag_str == "IMG" || tag_str == "PICTURE" {
                elem.set_attr(target, value);
            } else if tag_str == "FIGURE"
                && node_select_matcher(&elem, &selectors.img_picture).length() == 0
            {
                let escaped = escape_html_attr(value);
                let html = format!("<img {}=\"{}\">", target, escaped);
                elem.set_html(html.as_str());
            }
        }
    }
}

/// Unwrap images from noscript tags.
pub fn unwrap_noscript_images(doc: &Document, selectors: &Selectors) {
    // First, remove images without useful sources
    let imgs: Vec<_> = doc.select_matcher(&selectors.img).nodes().to_vec();
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
                    if has_image_extension(attr.value.as_ref()) {
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

    // Process noscript tags
    let noscripts: Vec<_> = doc.select_matcher(&selectors.noscript).nodes().to_vec();
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
            // Parse noscript content into a temporary container
            let noscript_html = noscript.inner_html();
            let tmp_doc = Document::from(format!("<div>{}</div>", noscript_html).as_str());
            let new_img = tmp_doc.select("img").nodes().first().cloned();

            // Get the actual img element from the previous element
            let prev_img = if get_tag_name(&prev).as_deref() == Some("IMG") {
                Some(prev)
            } else {
                node_select_matcher(&prev, &selectors.img)
                    .nodes()
                    .first()
                    .cloned()
            };

            if let (Some(new_img), Some(prev_img)) = (new_img, prev_img) {
                // Copy image-related attributes from old img to new img
                // This preserves lazy-loading attributes like data-src
                for attr in prev_img.attrs().iter() {
                    let attr_name = attr.name.local.as_ref();
                    let attr_value = attr.value.as_ref();

                    // Skip empty attributes
                    if attr_value.is_empty() {
                        continue;
                    }

                    // Only copy src, srcset, or attributes containing image extensions
                    let is_image_attr = attr_name == "src"
                        || attr_name == "srcset"
                        || has_image_extension(attr_value);

                    if is_image_attr {
                        // Skip if new img already has the same value for this attribute
                        if let Some(new_value) = new_img.attr(attr_name)
                            && new_value.as_ref() == attr_value
                        {
                            continue;
                        }

                        // If new img already has this attribute, prefix with data-old-
                        let target_name: Cow<str> = if new_img.has_attr(attr_name) {
                            format!("data-old-{}", attr_name).into()
                        } else {
                            Cow::Borrowed(attr_name)
                        };

                        new_img.set_attr(&target_name, attr_value);
                    }
                }

                // Replace the entire previous element with the new img
                // Build the new img HTML and replace prev element
                prev.after_html(new_img.html());
                prev.remove_from_parent();
                noscript.remove_from_parent();
            }
        }
    }
}

/// Simplify nested elements by removing empty ones and unwrapping single-child containers.
pub fn simplify_nested_elements(article_content: &Node<'_>, _selectors: &Selectors) {
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
                // Replace node with its single child, copying attributes from node to child
                let children = node.element_children();
                if let Some(child) = children.first() {
                    // Copy all attributes from node to child (like JS does)
                    for attr in node.attrs() {
                        let name: &str = &attr.name.local;
                        let value: &str = &attr.value;
                        // Only copy if child doesn't already have this attribute
                        if child.attr(name).is_none() {
                            child.set_attr(name, value);
                        }
                    }

                    // Replace node with child in the DOM tree
                    node.insert_after(child);
                    node.remove_from_parent();
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

    // Reusable buffer for match_string to avoid allocations per node
    let mut match_string_buf = String::with_capacity(128);

    for n in descendants {
        if !n.is_element() {
            continue;
        }

        // Build match_string for filter - reuse buffer to avoid allocations
        build_match_string(&n, &mut match_string_buf);

        if filter(&n, &match_string_buf) {
            n.remove_from_parent();
        }
    }
}

/// Escape special characters for use in an HTML attribute value.
fn escape_html_attr<'a>(s: &'a str) -> Cow<'a, str> {
    if !s.contains(['&', '"', '<', '>']) {
        return Cow::Borrowed(s);
    }
    let mut result = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '&' => result.push_str("&amp;"),
            '"' => result.push_str("&quot;"),
            '<' => result.push_str("&lt;"),
            '>' => result.push_str("&gt;"),
            _ => result.push(c),
        }
    }
    Cow::Owned(result)
}
