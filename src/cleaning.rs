//! DOM cleaning functions for Readability.

use crate::constants::{
    DEPRECATED_SIZE_ATTRIBUTE_ELEMS, PRESENTATIONAL_ATTRIBUTES, flags::*, regexps,
};
use crate::dom::{NodeDataStore, get_tag_name, node_select, node_select_matcher};
use crate::scoring::{
    get_class_weight, get_inner_text, get_link_density_cached, get_or_compute_stats,
    get_text_density_cached, has_single_tag_inside_element, is_element_without_content,
    is_phrasing_content,
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
    // Use eq_ignore_ascii_case to avoid allocation (Phase 4.1)
    if let Some(tag) = get_tag_name(node)
        && tag.eq_ignore_ascii_case("SVG")
    {
        return;
    }

    // Remove presentational attributes
    for attr in PRESENTATIONAL_ATTRIBUTES.iter() {
        node.remove_attr(attr);
    }

    // Remove deprecated size attributes on certain elements
    if let Some(tag) = get_tag_name(node)
        && DEPRECATED_SIZE_ATTRIBUTE_ELEMS.contains(&*tag)
    {
        node.remove_attr("width");
        node.remove_attr("height");
    }

    // Clean children
    for child in node.element_children() {
        clean_styles(&child);
    }
}

/// Clean spurious headers from an element.
pub fn clean_headers(node: &Node<'_>, flags: u32, selectors: &Selectors) {
    let headings: Vec<_> = node_select_matcher(node, &selectors.h1_h2).nodes().to_vec();
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

    // Get or compute cached stats for this node
    let stats = get_or_compute_stats(node, store);
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

    // Various content-based checks - single traversal instead of 4 separate selector calls
    let (p_count, img_count, li_count, input_count) = {
        let mut p: usize = 0;
        let mut img: usize = 0;
        let mut li: usize = 0;
        let mut input: usize = 0;
        for descendant in node.descendants_it() {
            if let Some(tag) = get_tag_name(&descendant) {
                match &*tag {
                    "P" => p += 1,
                    "IMG" => img += 1,
                    "LI" => li += 1,
                    "INPUT" => input += 1,
                    _ => {}
                }
            }
        }
        (p, img, li.saturating_sub(100), input)
    };
    let heading_density = get_text_density_cached(
        node,
        content_length,
        &["h1", "h2", "h3", "h4", "h5", "h6"],
        store,
    );

    let mut embed_count = 0;
    let embeds = node_select_matcher(node, &selectors.object_embed_iframe);
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

    // Check for ad/loading words - use RegexSet for single-pass matching
    let inner_text = get_inner_text(node, true);
    if regexps::AD_LOADING_SET.is_match(&inner_text) {
        return true;
    }

    let link_density = get_link_density_cached(node, content_length, store, selectors);

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
    let text_density = get_text_density_cached(node, content_length, &textish_tags, store);

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
                    if node_select_matcher(&elem, &selectors.img_picture).length() == 0 {
                        let html = format!("<img {}=\"{}\">", target, value);
                        elem.set_html(html.as_str());
                    }
                }
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
                        || regexps::IMAGE_EXTENSION.is_match(attr_value);

                    if is_image_attr {
                        // Skip if new img already has the same value for this attribute
                        if let Some(new_value) = new_img.attr(attr_name)
                            && new_value.as_ref() == attr_value
                        {
                            continue;
                        }

                        // If new img already has this attribute, prefix with data-old-
                        let target_name = if new_img.has_attr(attr_name) {
                            format!("data-old-{}", attr_name)
                        } else {
                            attr_name.to_string()
                        };

                        new_img.set_attr(&target_name, attr_value);
                    }
                }

                // Replace the entire previous element with the new img
                // Build the new img HTML and replace prev element
                let new_img_html = new_img.html().to_string();
                prev.after_html(new_img_html.as_str());
                prev.remove_from_parent();
                noscript.remove_from_parent();
            }
        }
    }
}

/// Simplify nested elements by removing empty ones and unwrapping single-child containers.
pub fn simplify_nested_elements(article_content: &Node<'_>, selectors: &Selectors) {
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

            if is_element_without_content(&node, selectors) {
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
                        let name = attr.name.local.to_string();
                        let value = attr.value.to_string();
                        // Only copy if child doesn't already have this attribute
                        if child.attr(&name).is_none() {
                            child.set_attr(&name, &value);
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
        match_string_buf.clear();
        if let Some(class) = n.attr("class") {
            match_string_buf.push_str(class.as_ref());
        }
        match_string_buf.push(' ');
        if let Some(id) = n.attr("id") {
            match_string_buf.push_str(id.as_ref());
        }

        if filter(&n, &match_string_buf) {
            n.remove_from_parent();
        }
    }
}
