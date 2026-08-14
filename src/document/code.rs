//! Source recognition for semantic code compilation.

use crate::dom::{AttrName, Dom, NodeId, Tag};

/// A code block recovered from source HTML.
pub(crate) struct RecognizedCode {
    pub(crate) language: Option<String>,
    pub(crate) text: String,
}

/// Recognizes a preformatted block or a multiline orphan `code` element.
pub(crate) fn recognize_block(dom: &Dom, node: NodeId) -> Option<RecognizedCode> {
    let recognized_as_block = dom.tag(node) == Some(Tag::Pre) || is_multiline_orphan(dom, node);
    recognize_known_block(dom, node, recognized_as_block)
}

/// Compiles a block whose multiline status was computed by the caller.
pub(crate) fn recognize_known_block(
    dom: &Dom,
    node: NodeId,
    recognized_as_block: bool,
) -> Option<RecognizedCode> {
    let content = match dom.tag(node) {
        Some(Tag::Pre) => {
            if has_single_tag_inside_element(dom, node, Tag::Code) {
                dom.element_children(node).next().unwrap_or(node)
            } else {
                node
            }
        }
        Some(Tag::Code) if recognized_as_block => node,
        _ => return None,
    };
    let mut nested_language = None;
    let text = code_text(dom, content, &mut nested_language);
    let language = if dom.tag(node) == Some(Tag::Pre) {
        block_language_hint(dom, content, node)
    } else {
        language_hint(dom, node)
    }
    .or(nested_language);
    Some(RecognizedCode { language, text })
}

/// Recognizes a syntax-highlighter table with a parallel line-number gutter.
pub(crate) fn recognize_gutter_table(dom: &Dom, table: NodeId) -> Option<RecognizedCode> {
    let source_pres = gutter_table_sources(dom, table)?;
    let first = *source_pres.first()?;
    let mut language = None;
    let mut text = String::new();
    for pre in source_pres {
        let block = recognize_block(dom, pre)?;
        language = language.or(block.language);
        if !text.ends_with('\n') && !text.is_empty() && !block.text.trim().is_empty() {
            text.push('\n');
        }
        text.push_str(&block.text);
    }
    Some(RecognizedCode {
        language: language.or_else(|| language_from_ancestors(dom, first)),
        text,
    })
}

/// Returns true for a syntax-highlighter table with a presentation gutter.
pub(crate) fn is_gutter_table(dom: &Dom, table: NodeId) -> bool {
    gutter_table_sources(dom, table).is_some()
}

fn gutter_table_sources(dom: &Dom, table: NodeId) -> Option<Vec<NodeId>> {
    if dom.tag(table) != Some(Tag::Table) {
        return None;
    }
    let cells = dom
        .table_descendants(table)
        .into_iter()
        .filter(|&node| matches!(dom.tag(node), Some(Tag::Td | Tag::Th)))
        .collect::<Vec<_>>();
    let pre_blocks = cells
        .iter()
        .filter_map(|&cell| {
            dom.table_descendants(cell)
                .into_iter()
                .find(|&node| dom.tag(node) == Some(Tag::Pre))
        })
        .collect::<Vec<_>>();
    let has_gutter = pre_blocks
        .iter()
        .copied()
        .any(|pre| has_line_number_marker(dom, pre))
        || cells.iter().copied().any(|cell| is_gutter_cell(dom, cell))
        || has_line_number_table_class(dom, table);
    if !has_gutter {
        return None;
    }
    let source_pres = pre_blocks
        .into_iter()
        .filter(|&pre| !is_gutter_pre(dom, pre))
        .collect::<Vec<_>>();
    (!source_pres.is_empty()).then_some(source_pres)
}

/// Returns true when a class contains evidence needed by the compiler.
///
/// Final DOM cleanup removes unrelated classes. It retains these source markers
/// until semantic compilation consumes them.
pub(crate) fn class_is_semantic_evidence(dom: &Dom, node: NodeId) -> bool {
    language_hint(dom, node).is_some()
        || has_code_wrapper_name(dom, node)
        || is_language_label(dom, node)
        || is_line_wrapper(dom, node)
        || is_line_number_element(dom, node)
        || is_gutter_cell(dom, node)
        || has_line_number_table_class(dom, node)
}

/// Returns true when `code` has block semantics without a `pre` ancestor.
pub(crate) fn is_multiline_orphan(dom: &Dom, code: NodeId) -> bool {
    let multiline = std::iter::once(code)
        .chain(dom.descendants(code))
        .any(|node| {
            dom.tag(node) == Some(Tag::Br)
                || dom.text_node(node).is_some_and(|text| text.contains('\n'))
        });
    is_multiline_orphan_with_evidence(dom, code, multiline)
}

/// Classifies orphan code from caller-cached subtree evidence.
pub(crate) fn is_multiline_orphan_with_evidence(dom: &Dom, code: NodeId, multiline: bool) -> bool {
    if !multiline || dom.tag(code) != Some(Tag::Code) {
        return false;
    }
    let Some(parent) = dom.parent(code) else {
        return false;
    };
    if dom.tag(parent) == Some(Tag::Pre) {
        return false;
    }
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
    !phrasing_parent || sole_content
}

/// Counts source structures that compile to semantic code-block leaves.
pub(crate) fn count_blocks(dom: &Dom, root: NodeId) -> usize {
    let nodes: Vec<_> = std::iter::once(root).chain(dom.descendants(root)).collect();
    let multiline = multiline_content(dom, &nodes);
    let mut count = 0;
    let mut tasks: Vec<_> = dom.children_rev(root).collect();
    while let Some(node) = tasks.pop() {
        let block = dom.tag(node) == Some(Tag::Pre)
            || is_multiline_orphan_with_evidence(dom, node, multiline[node.index()]);
        if block || is_gutter_table(dom, node) {
            count += 1;
        } else {
            tasks.extend(dom.children_rev(node));
        }
    }
    count
}

pub(crate) fn multiline_content(dom: &Dom, nodes: &[NodeId]) -> Vec<bool> {
    let mut multiline = vec![false; dom.len()];
    for &node in nodes.iter().rev() {
        multiline[node.index()] = dom.tag(node) == Some(Tag::Br)
            || dom.text_node(node).is_some_and(|text| text.contains('\n'))
            || dom.children(node).any(|child| multiline[child.index()]);
    }
    multiline
}

fn code_text(dom: &Dom, root: NodeId, nested_language: &mut Option<String>) -> String {
    let mut output = String::new();
    let mut authored_text = false;
    let mut break_position = None;
    let mut break_count = 0;
    let mut seen_line_wrapper = false;
    let mut boundary_since_line_wrapper = false;
    let mut tasks: Vec<_> = dom.children_rev(root).collect();
    while let Some(node) = tasks.pop() {
        if is_line_number_element(dom, node) {
            continue;
        }
        if dom.tag(node) == Some(Tag::Table)
            && let Some(block) = recognize_gutter_table(dom, node)
        {
            authored_text |= !block.text.is_empty();
            if nested_language.is_none() {
                *nested_language = block.language;
            }
            output.push_str(&block.text);
            continue;
        }
        if dom.parent(node) == Some(root) && is_line_wrapper(dom, node) {
            if seen_line_wrapper && !boundary_since_line_wrapper {
                output.push('\n');
            }
            seen_line_wrapper = true;
            boundary_since_line_wrapper = false;
        }
        if let Some(text) = dom.text_node(node) {
            authored_text |= !text.is_empty();
            boundary_since_line_wrapper |= seen_line_wrapper && text.contains('\n');
            output.push_str(text);
            continue;
        }
        if dom.tag(node) == Some(Tag::Br) {
            break_count += 1;
            break_position = Some(output.len());
            boundary_since_line_wrapper |= seen_line_wrapper;
            output.push('\n');
            continue;
        }
        tasks.extend(dom.children_rev(node));
    }
    // A lone break in an otherwise empty pre is commonly a textarea mirror or
    // placeholder. Two breaks encode a blank line even without other text.
    if break_count == 1
        && !authored_text
        && let Some(position) = break_position
    {
        output.remove(position);
    }
    output
}

fn language_from_ancestors(dom: &Dom, node: NodeId) -> Option<String> {
    std::iter::once(node)
        .chain(dom.ancestors(node))
        .find_map(|ancestor| language_hint(dom, ancestor))
}

fn has_line_number_table_class(dom: &Dom, table: NodeId) -> bool {
    [AttrName::Class, AttrName::Id]
        .into_iter()
        .filter_map(|attribute| dom.attr(table, attribute))
        .flat_map(str::split_whitespace)
        .any(|token| {
            token_is_one_of(
                token,
                &[
                    "highlighttable",
                    "lntable",
                    "rouge-table",
                    "rouge-line-table",
                ],
            )
        })
}

fn is_gutter_pre(dom: &Dom, pre: NodeId) -> bool {
    has_line_number_marker(dom, pre)
        || dom.ancestors(pre).any(|ancestor| {
            [AttrName::Class, AttrName::Id]
                .into_iter()
                .filter_map(|attribute| dom.attr(ancestor, attribute))
                .flat_map(str::split_whitespace)
                .any(|token| token.eq_ignore_ascii_case("linenodiv"))
        })
        || dom
            .ancestors(pre)
            .find(|&ancestor| matches!(dom.tag(ancestor), Some(Tag::Td | Tag::Th)))
            .is_some_and(|cell| is_gutter_cell(dom, cell))
}

fn is_gutter_cell(dom: &Dom, cell: NodeId) -> bool {
    [AttrName::Class, AttrName::Id]
        .into_iter()
        .filter_map(|attribute| dom.attr(cell, attribute))
        .flat_map(str::split_whitespace)
        .any(|token| token_is_one_of(token, &["linenos", "rouge-gutter", "gutter"]))
}

fn has_line_number_marker(dom: &Dom, pre: NodeId) -> bool {
    std::iter::once(pre)
        .chain(dom.descendants(pre))
        .any(|node| is_line_number_element(dom, node))
}

fn is_line_number_element(dom: &Dom, node: NodeId) -> bool {
    [AttrName::Class, AttrName::Id]
        .into_iter()
        .filter_map(|attribute| dom.attr(node, attribute))
        .flat_map(str::split_whitespace)
        .any(|token| {
            token_is_one_of(
                token,
                &[
                    "lnt",
                    "lineno",
                    "line-number",
                    "line-numbers",
                    "line-numbers-rows",
                    "line-number-gutter",
                    "rouge-gutter",
                    "gutter",
                ],
            ) || strip_prefix_ascii_case(token, "line-number-").is_some()
        })
}

fn is_line_wrapper(dom: &Dom, node: NodeId) -> bool {
    dom.tag(node) == Some(Tag::Span)
        && (dom.attr_by_local_name(node, "data-line").is_some()
            || dom.attr_by_local_name(node, "line").is_some()
            || dom
                .attr(node, AttrName::Class)
                .is_some_and(|class| class.split_whitespace().any(|token| token == "line")))
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
        .or_else(|| language_from_header(dom, pre))
}

fn language_from_header(dom: &Dom, pre: NodeId) -> Option<String> {
    let mut branch = pre;
    for depth in 0..2 {
        let wrapper = dom.parent(branch)?;
        if depth > 0 && !is_code_wrapper(dom, wrapper) {
            return None;
        }
        let mut sibling = dom.prev_sibling(branch);
        let mut buffer = String::new();
        while let Some(node) = sibling {
            if dom.tag(node).is_none() {
                if dom
                    .text_node(node)
                    .is_some_and(|text| !text.trim().is_empty())
                {
                    return None;
                }
                sibling = dom.prev_sibling(node);
                continue;
            }
            if dom.tag(node) == Some(Tag::Pre)
                || dom
                    .descendants(node)
                    .any(|child| dom.tag(child) == Some(Tag::Pre))
            {
                return None;
            }
            let named_label = is_language_label(dom, node);
            if named_label
                && let Some(language) = dom
                    .attr_by_local_name(node, "data-language-label")
                    .and_then(normalize_explicit_language)
            {
                return Some(language);
            }
            let label = normalized_inner_text(dom, node, &mut buffer);
            return if named_label {
                language_from_label(label)
            } else {
                explicit_language_from_label(label)
            };
        }
        branch = wrapper;
    }
    None
}

fn normalized_inner_text<'a>(dom: &Dom, node: NodeId, buffer: &'a mut String) -> &'a str {
    buffer.clear();
    let text = dom.text(node);
    for word in text.split_whitespace() {
        if !buffer.is_empty() {
            buffer.push(' ');
        }
        buffer.push_str(word);
    }
    buffer
}

fn has_single_tag_inside_element(dom: &Dom, node: NodeId, tag: Tag) -> bool {
    let mut found = false;
    for child in dom.children(node) {
        if dom.is_element(child) {
            if found || dom.tag(child) != Some(tag) {
                return false;
            }
            found = true;
        } else if dom
            .text_node(child)
            .is_some_and(|text| !text.trim().is_empty())
        {
            return false;
        }
    }
    found
}

fn is_language_label(dom: &Dom, node: NodeId) -> bool {
    [AttrName::Class, AttrName::Id]
        .into_iter()
        .filter_map(|attribute| dom.attr(node, attribute))
        .flat_map(str::split_whitespace)
        .any(|token| {
            token_is_one_of(
                token,
                &[
                    "code-header",
                    "code-language",
                    "code-lang",
                    "language-label",
                    "highlight-header",
                    "codeblock-title",
                ],
            )
        })
        || dom
            .attr_by_local_name(node, "data-language-label")
            .is_some()
}

fn language_from_label(value: &str) -> Option<String> {
    explicit_language_from_label(value).or_else(|| {
        let value = value.trim().to_ascii_lowercase();
        known_language(&value).then_some(value)
    })
}

fn explicit_language_from_label(value: &str) -> Option<String> {
    let value = value.trim().to_ascii_lowercase();
    if let Some(explicit) = value
        .strip_prefix("language:")
        .or_else(|| value.strip_prefix("language "))
        .map(str::trim)
    {
        return normalize_explicit_language(explicit);
    }
    let explicit = value.strip_prefix("code:").map(str::trim).or_else(|| {
        value
            .strip_suffix(" code block")
            .or_else(|| value.strip_suffix(" code"))
            .map(str::trim)
    })?;
    known_language(explicit).then(|| explicit.to_owned())
}

fn is_code_wrapper(dom: &Dom, node: NodeId) -> bool {
    dom.tag(node) == Some(Tag::Div)
        && (language_hint(dom, node).is_some() || has_code_wrapper_name(dom, node))
}

fn has_code_wrapper_name(dom: &Dom, node: NodeId) -> bool {
    dom.tag(node) == Some(Tag::Div)
        && [AttrName::Class, AttrName::Id]
            .into_iter()
            .filter_map(|attribute| dom.attr(node, attribute))
            .flat_map(str::split_whitespace)
            .any(|token| {
                token_is_one_of(
                    token,
                    &[
                        "highlight",
                        "codehilite",
                        "sourcecode",
                        "code-block",
                        "codeblock",
                        "syntax-highlight",
                    ],
                )
            })
}

fn language_hint(dom: &Dom, node: NodeId) -> Option<String> {
    for name in [
        AttrName::DataLanguage,
        AttrName::DataLang,
        AttrName::DataCodeLanguage,
        AttrName::Language,
    ] {
        if let Some(language) = dom.attr(node, name).and_then(normalize_explicit_language) {
            return Some(language);
        }
    }

    if let Some(label) = dom
        .attr(node, AttrName::AriaLabel)
        .and_then(language_from_label)
        .or_else(|| {
            dom.attr(node, AttrName::Title)
                .and_then(explicit_language_from_label)
        })
    {
        return Some(label);
    }

    let classes = dom.attr(node, AttrName::Class).unwrap_or_default();
    let mut tokens = classes.split_whitespace();
    while let Some(token) = tokens.next() {
        let prefixed = strip_prefix_ascii_case(token, "language-")
            .or_else(|| strip_prefix_ascii_case(token, "lang-"))
            .or_else(|| strip_prefix_ascii_case(token, "highlight-source-"));
        if let Some(language) = prefixed.and_then(normalize_explicit_language) {
            return Some(language);
        }
        if token.eq_ignore_ascii_case("brush:")
            && let Some(language) = tokens
                .next()
                .map(|value| value.trim_end_matches(';').to_ascii_lowercase())
                .filter(|value| known_language(value))
        {
            return Some(language);
        }
        if let Some(language) = strip_prefix_ascii_case(token, "brush:")
            .map(|value| value.trim_end_matches(';').to_ascii_lowercase())
            .filter(|value| known_language(value))
        {
            return Some(language);
        }
    }
    None
}

fn token_is_one_of(token: &str, values: &[&str]) -> bool {
    values.iter().any(|value| token.eq_ignore_ascii_case(value))
}

fn strip_prefix_ascii_case<'a>(value: &'a str, prefix: &str) -> Option<&'a str> {
    let candidate = value.get(..prefix.len())?;
    candidate
        .eq_ignore_ascii_case(prefix)
        .then(|| &value[prefix.len()..])
}

fn normalize_explicit_language(value: &str) -> Option<String> {
    let value = value.trim().to_ascii_lowercase();
    (!value.is_empty()
        && value.len() <= 32
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'#' | b'-' | b'_' | b'.')
        }))
    .then_some(value)
}

fn known_language(value: &str) -> bool {
    matches!(
        value,
        "ada"
            | "assembly"
            | "astro"
            | "bash"
            | "c"
            | "c#"
            | "c++"
            | "clojure"
            | "coffee"
            | "coffeescript"
            | "cpp"
            | "csharp"
            | "cmake"
            | "css"
            | "dart"
            | "diff"
            | "docker"
            | "dockerfile"
            | "elixir"
            | "elm"
            | "erb"
            | "erl"
            | "erlang"
            | "f#"
            | "fish"
            | "go"
            | "graphql"
            | "groovy"
            | "haskell"
            | "html"
            | "ini"
            | "java"
            | "javascript"
            | "js"
            | "json"
            | "jsonc"
            | "jsx"
            | "kotlin"
            | "latex"
            | "less"
            | "lisp"
            | "lua"
            | "make"
            | "makefile"
            | "markdown"
            | "md"
            | "mdx"
            | "nginx"
            | "objective-c"
            | "ocaml"
            | "perl"
            | "php"
            | "plaintext"
            | "powershell"
            | "proto"
            | "protobuf"
            | "python"
            | "r"
            | "rb"
            | "ruby"
            | "rust"
            | "sass"
            | "scala"
            | "scss"
            | "sh"
            | "shell"
            | "solidity"
            | "sql"
            | "svelte"
            | "swift"
            | "toml"
            | "tsx"
            | "typescript"
            | "vim"
            | "vue"
            | "wasm"
            | "xml"
            | "yaml"
            | "yml"
            | "zig"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(html: &str) -> RecognizedCode {
        let dom = Dom::parse_fragment(html, Tag::Div).unwrap();
        let node = dom
            .descendants(dom.root())
            .find(|&node| matches!(dom.tag(node), Some(Tag::Pre | Tag::Code)))
            .unwrap();
        recognize_block(&dom, node).unwrap()
    }

    #[test]
    fn reconstructs_highlighter_lines_and_removes_gutters() {
        let code = block(
            r#"<pre><code><span data-line><span class="lineno">1</span><span>let value = 1;</span></span><span data-line><span class="lineno">2</span><span>value</span><br></span></code></pre>"#,
        );
        assert_eq!(code.text, "let value = 1;\nvalue\n");
    }

    #[test]
    fn orphan_code_keeps_content_around_a_gutter_table() {
        let code = block(
            r#"<div><code>before
<table class="highlighttable"><tr><td class="linenos"><pre>1</pre></td><td><pre>source</pre></td></tr></table>
after</code></div>"#,
        );
        assert_eq!(code.text, "before\nsource\nafter");
    }

    #[test]
    fn indented_line_wrapper_markup_does_not_add_a_blank_line() {
        let code = block(
            "<pre><code><span data-line>first</span>\n  <span data-line>second</span></code></pre>",
        );
        assert_eq!(code.text, "first\n  second");
    }

    #[test]
    fn pre_with_mixed_content_does_not_drop_code_siblings() {
        let code = block("<pre>prompt <code>command</code> output <code>status</code></pre>");
        assert_eq!(code.text, "prompt command output status");
    }

    #[test]
    fn preserves_code_whitespace_and_blank_break_lines() {
        assert_eq!(
            block("<pre><code><span>    first  \n</span><span>\tsecond\n\n</span></code></pre>")
                .text,
            "    first  \n\tsecond\n\n"
        );
        assert_eq!(block("<pre><code><br><br></code></pre>").text, "\n\n");
        assert_eq!(block("<pre><code><br></code></pre>").text, "");
    }

    #[test]
    fn detects_languages_from_source_evidence() {
        for (html, expected) in [
            (r#"<pre data-lang="Rust"><code>x</code></pre>"#, "rust"),
            (r#"<pre><code language="TSX">x</code></pre>"#, "tsx"),
            (
                r#"<div class="language-python"><pre>x</pre></div>"#,
                "python",
            ),
            (r#"<pre class="brush: ruby;">x</pre>"#, "ruby"),
            (r#"<pre aria-label="F#">x</pre>"#, "f#"),
        ] {
            assert_eq!(block(html).language.as_deref(), Some(expected));
        }
    }

    #[test]
    fn combines_highlighter_table_source_rows() {
        let dom = Dom::parse_fragment(
            r#"<table class="highlighttable"><tr><td class="linenos"><pre>1</pre></td><td><pre>first
</pre></td></tr><tr><td class="linenos"><pre>2</pre></td><td><pre>second</pre></td></tr></table>"#,
            Tag::Div,
        )
        .unwrap();
        let table = dom.first_descendant_by_tag(dom.root(), Tag::Table).unwrap();
        assert_eq!(
            recognize_gutter_table(&dom, table).unwrap().text,
            "first\nsecond"
        );
    }
}
