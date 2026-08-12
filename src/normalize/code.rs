//! Canonical code block normalization.

use crate::dom::{AttrName, Dom, NodeId, Tag};
use crate::scoring::has_single_tag_inside_element;

/// Converts source code markup into `pre > code` blocks before cleanup can
/// change line breaks or discard syntax-highlighter token spans.
pub(super) fn normalize(dom: &mut Dom, root: NodeId) {
    let nodes = dom.element_descendants_snapshot_with_depth(root);
    for (node, _) in nodes {
        if dom.parent(node).is_none() {
            continue;
        }
        match dom.tag(node) {
            Some(Tag::Pre) => ensure_code_child(dom, node),
            Some(Tag::Code) if dom.tag(dom.parent(node).unwrap_or(root)) != Some(Tag::Pre) => {
                promote_multiline_code(dom, root, node);
            }
            _ => {}
        }
    }

    let pre_blocks: Vec<_> = dom
        .element_descendants_snapshot_with_depth(root)
        .into_iter()
        .map(|(node, _)| node)
        .filter(|&node| dom.tag(node) == Some(Tag::Pre))
        .collect();
    for pre in pre_blocks {
        if dom.parent(pre).is_some() {
            normalize_line_breaks(dom, pre);
            reconstruct_line_wrappers(dom, pre);
        }
    }

    // Common syntax highlighters put one pre block inside a decorative div.
    let wrappers = dom.element_descendants_snapshot_with_depth(root);
    for (wrapper, _) in wrappers.into_iter().rev() {
        if dom.parent(wrapper).is_none() || dom.tag(wrapper) != Some(Tag::Div) {
            continue;
        }
        let named_as_code = [AttrName::Class, AttrName::Id]
            .into_iter()
            .filter_map(|attribute| dom.attr(wrapper, attribute))
            .any(|value| {
                value.split_whitespace().any(|token| {
                    let token = token.to_ascii_lowercase();
                    token == "highlight"
                        || token == "codehilite"
                        || token == "sourcecode"
                        || token.starts_with("highlight-")
                })
            });
        if named_as_code
            && has_single_tag_inside_element(dom, wrapper, Tag::Pre)
            && let Some(pre) = dom.element_children(wrapper).next()
        {
            dom.replace_with(wrapper, pre);
        }
    }
}

fn ensure_code_child(dom: &mut Dom, pre: NodeId) {
    let code = if has_single_tag_inside_element(dom, pre, Tag::Code) {
        dom.element_children(pre).next()
    } else {
        let Ok(code) = dom.create_html_element(Tag::Code) else {
            return;
        };
        dom.move_children(pre, code);
        dom.append_child(pre, code);
        Some(code)
    };
    if let Some(code) = code
        && let Some(language) = block_language_hint(dom, code, pre)
    {
        dom.set_attr(code, AttrName::DataLanguage, &language);
    }
}

fn promote_multiline_code(dom: &mut Dom, root: NodeId, code: NodeId) {
    let parent = dom.parent(code).unwrap_or(root);
    let phrasing_parent = matches!(
        dom.tag(parent),
        Some(
            Tag::P
                | Tag::H1
                | Tag::H2
                | Tag::H3
                | Tag::H4
                | Tag::H5
                | Tag::H6
                | Tag::A
                | Tag::Td
                | Tag::Th
        )
    );
    let sole_content = has_single_tag_inside_element(dom, parent, Tag::Code);
    let block = (!phrasing_parent || sole_content)
        && std::iter::once(code)
            .chain(dom.descendants(code))
            .any(|node| {
                dom.tag(node) == Some(Tag::Br)
                    || dom.text_node(node).is_some_and(|text| text.contains('\n'))
            });
    if !block {
        return;
    }
    if dom.tag(parent) == Some(Tag::P) && sole_content {
        dom.rename_html(parent, Tag::Div);
    }
    let language = language_hint(dom, code);
    let Ok(pre) = dom.create_html_element(Tag::Pre) else {
        return;
    };
    dom.insert_before(code, pre);
    dom.append_child(pre, code);
    if let Some(language) = language {
        dom.set_attr(code, AttrName::DataLanguage, &language);
    }
}

fn normalize_line_breaks(dom: &mut Dom, pre: NodeId) {
    let Some(code) = dom
        .element_children(pre)
        .find(|&node| dom.tag(node) == Some(Tag::Code))
    else {
        return;
    };
    if !dom
        .descendants(code)
        .filter_map(|node| dom.text_node(node))
        .any(|text| !text.trim().is_empty())
    {
        return;
    }
    let breaks: Vec<_> = dom
        .descendants(code)
        .filter(|&node| dom.tag(node) == Some(Tag::Br))
        .collect();
    for line_break in breaks {
        if dom.parent(line_break).is_some()
            && let Ok(newline) = dom.create_text("\n")
        {
            dom.replace_with(line_break, newline);
        }
    }
}

fn reconstruct_line_wrappers(dom: &mut Dom, pre: NodeId) {
    let Some(code) = dom
        .element_children(pre)
        .find(|&node| dom.tag(node) == Some(Tag::Code))
    else {
        return;
    };
    let lines: Vec<_> = dom
        .element_children(code)
        .filter(|&node| is_line_wrapper(dom, node))
        .collect();
    for pair in lines.windows(2) {
        let [line, next_line] = pair else {
            continue;
        };
        if line_ends_with_break(dom, *line) || has_line_break_between(dom, *line, *next_line) {
            continue;
        }
        if let Ok(newline) = dom.create_text("\n") {
            dom.insert_before(*next_line, newline);
        }
    }
}

fn is_line_wrapper(dom: &Dom, node: NodeId) -> bool {
    dom.tag(node) == Some(Tag::Span)
        && (dom.attr_by_local_name(node, "data-line").is_some()
            || dom.attr_by_local_name(node, "line").is_some()
            || dom
                .attr(node, AttrName::Class)
                .is_some_and(|class| class.split_whitespace().any(|token| token == "line")))
}

fn has_line_break_between(dom: &Dom, line: NodeId, next_line: NodeId) -> bool {
    let mut node = dom.next_sibling(line);
    while let Some(current) = node {
        if current == next_line {
            return false;
        }
        if dom.tag(current) == Some(Tag::Br)
            || dom
                .text_node(current)
                .is_some_and(|text| text.contains('\n'))
        {
            return true;
        }
        node = dom.next_sibling(current);
    }
    false
}

fn line_ends_with_break(dom: &Dom, line: NodeId) -> bool {
    let mut ends_with_break = false;
    for node in std::iter::once(line).chain(dom.descendants(line)) {
        if dom.tag(node) == Some(Tag::Br) {
            ends_with_break = true;
        } else if let Some(text) = dom.text_node(node).filter(|text| !text.is_empty()) {
            ends_with_break = text.ends_with('\n');
        }
    }
    ends_with_break
}

fn block_language_hint(dom: &Dom, code: NodeId, pre: NodeId) -> Option<String> {
    language_hint(dom, code)
        .or_else(|| language_hint(dom, pre))
        .or_else(|| {
            let parent = dom.parent(pre)?;
            is_code_wrapper(dom, parent).then(|| language_hint(dom, parent))?
        })
        .or_else(|| {
            let parent = dom.parent(pre)?;
            let grandparent = dom.parent(parent)?;
            (is_code_wrapper(dom, parent) && is_code_wrapper(dom, grandparent))
                .then(|| language_hint(dom, grandparent))?
        })
}

fn is_code_wrapper(dom: &Dom, node: NodeId) -> bool {
    dom.tag(node) == Some(Tag::Div)
        && (language_hint(dom, node).is_some()
            || [AttrName::Class, AttrName::Id]
                .into_iter()
                .filter_map(|attribute| dom.attr(node, attribute))
                .flat_map(str::split_whitespace)
                .any(|token| {
                    matches!(
                        token.to_ascii_lowercase().as_str(),
                        "highlight" | "codehilite" | "sourcecode"
                    )
                }))
}

fn language_hint(dom: &Dom, node: NodeId) -> Option<String> {
    if let Some(value) = dom
        .attr(node, AttrName::DataLanguage)
        .or_else(|| dom.attr(node, AttrName::Lang))
        .or_else(|| dom.attr_by_local_name(node, "data-lang"))
        && let Some(language) = valid_language(value)
    {
        return Some(language.to_owned());
    }
    for token in dom.attr(node, AttrName::Class)?.split_whitespace() {
        let Some(value) = token
            .strip_prefix("language-")
            .or_else(|| token.strip_prefix("lang-"))
        else {
            continue;
        };
        if let Some(language) = valid_language(value) {
            return Some(language.to_owned());
        }
    }
    None
}

fn valid_language(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()
        && value.len() <= 32
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'-' | b'_' | b'.')))
    .then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown::dom_to_markdown;

    #[test]
    fn reconstructs_token_spans_and_br_separated_lines() {
        let mut dom = Dom::parse_fragment(
            r#"<pre class="language-rust"><code><span>let</span><span> value</span><span> = 1;</span><br><span>println!("{value}");</span><br></code></pre>"#,
            Tag::Div,
        )
        .unwrap();
        let root = dom.root();

        normalize(&mut dom, root);

        assert_eq!(
            dom_to_markdown(&dom, root, 0),
            "```rust\nlet value = 1;\nprintln!(\"{value}\");\n```\n"
        );
        let code = dom.first_descendant_by_tag(root, Tag::Code).unwrap();
        assert!(
            !dom.descendants(code)
                .any(|node| dom.tag(node) == Some(Tag::Br))
        );
    }

    #[test]
    fn reconstructs_minified_line_wrapper_boundaries() {
        let mut dom = Dom::parse_fragment(
            r#"<pre><code><span data-line><span>first</span></span><span data-line>second</span></code></pre>"#,
            Tag::Div,
        )
        .unwrap();
        let root = dom.root();

        normalize(&mut dom, root);

        assert_eq!(dom_to_markdown(&dom, root, 0), "```\nfirst\nsecond\n```\n");
    }

    #[test]
    fn does_not_duplicate_line_wrapper_breaks() {
        let mut dom = Dom::parse_fragment(
            r#"<pre><code><span data-line>first<br></span><span data-line>second<br></span></code></pre>"#,
            Tag::Div,
        )
        .unwrap();
        let root = dom.root();

        normalize(&mut dom, root);

        assert_eq!(dom_to_markdown(&dom, root, 0), "```\nfirst\nsecond\n```\n");
    }
}
