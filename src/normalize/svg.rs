use crate::dom::{AttrName, Dom, NodeId, Tag};
use html5ever::ns;
use smallvec::SmallVec;
use std::collections::{HashMap, HashSet};

const MAX_ACCESSIBLE_REFERENCES_PER_NODE: usize = 64;
const MAX_ACCESSIBLE_REFERENCES: usize = 256;

/// Replaces SVG implementation markup with a compact accessible representation.
///
/// SVG elements share one internal tag. Use qualified local names in this pass
/// so that style, animation, title, text, and grouping elements stay distinct.
pub(super) fn normalize(dom: &mut Dom, root: NodeId) {
    if !remove_implementation_nodes(dom, root) {
        return;
    }
    let labelled_content = labelled_content(dom, root);
    let hidden = hidden_nodes(dom, root);

    let svg_roots = outermost_svg_roots(dom, root);
    for svg in svg_roots.into_iter().rev() {
        if dom.parent(svg).is_none() || is_math_rendering(dom, svg) {
            continue;
        }
        replace_svg(dom, svg, &labelled_content, &hidden);
    }
}

fn remove_implementation_nodes(dom: &mut Dom, root: NodeId) -> bool {
    let mut has_svg = false;
    let mut nodes = SmallVec::<[NodeId; 32]>::new();
    for (node, _) in dom.element_descendants_snapshot_with_depth(root) {
        let Some(local) = svg_local_name(dom, node) else {
            continue;
        };
        has_svg |= local == "svg";
        if matches!(
            local,
            "style"
                | "script"
                | "animate"
                | "animateColor"
                | "animateMotion"
                | "animateTransform"
                | "discard"
                | "mpath"
                | "set"
        ) {
            nodes.push(node);
        }
    }
    for node in nodes {
        dom.detach(node);
    }
    has_svg
}

fn replace_svg(
    dom: &mut Dom,
    svg: NodeId,
    labelled_content: &HashMap<String, (NodeId, String)>,
    hidden: &[bool],
) {
    if hidden[svg.index()] {
        dom.detach(svg);
        return;
    }
    let content = chart_content(dom, svg, labelled_content, hidden);
    if content.rows.len() < 2
        && content.accessible_labels.len() < 2
        && !has_chart_evidence(dom, svg, hidden)
    {
        return;
    }
    let descriptions =
        chart_descriptions(dom, svg, &content.rows, &content.label_source_texts, hidden);
    if descriptions.is_empty() && content.rows.is_empty() && content.accessible_labels.is_empty() {
        dom.detach(svg);
        return;
    }

    let Ok(container) = dom.create_html_element(Tag::Div) else {
        return;
    };
    for attribute in [AttrName::Id, AttrName::Class] {
        if let Some(value) = dom.attr(svg, attribute).map(str::to_owned) {
            dom.set_attr(container, attribute, &value);
        }
    }
    for description in &descriptions {
        let Ok(paragraph) = dom.create_html_element(Tag::P) else {
            continue;
        };
        let Ok(text) = dom.create_text(description) else {
            continue;
        };
        dom.append_child(paragraph, text);
        dom.append_child(container, paragraph);
    }
    for label in content
        .accessible_labels
        .iter()
        .filter(|label| !descriptions.contains(label))
    {
        append_paragraph(dom, container, label);
    }
    if content.rows.len() >= 2 {
        append_chart_table(dom, container, svg, &content.rows);
    } else {
        for row in content.rows {
            append_paragraph(dom, container, &format!("{}: {}", row.label, row.value));
        }
    }
    dom.replace_with(svg, container);
}

fn outermost_svg_roots(dom: &Dom, root: NodeId) -> SmallVec<[NodeId; 16]> {
    let mut roots = SmallVec::new();
    let mut inside_svg = vec![false; dom.len()];
    if is_svg_element(dom, root, "svg") {
        roots.push(root);
    }
    for (node, _) in dom.element_descendants_snapshot_with_depth(root) {
        let parent_is_svg = dom
            .parent(node)
            .is_some_and(|parent| inside_svg[parent.index()] || is_svg_element(dom, parent, "svg"));
        inside_svg[node.index()] = parent_is_svg;
        if is_svg_element(dom, node, "svg") && !parent_is_svg {
            roots.push(node);
        }
    }
    roots
}

fn append_paragraph(dom: &mut Dom, container: NodeId, value: &str) {
    let Ok(paragraph) = dom.create_html_element(Tag::P) else {
        return;
    };
    let Ok(text) = dom.create_text(value) else {
        return;
    };
    dom.append_child(paragraph, text);
    dom.append_child(container, paragraph);
}

fn has_chart_evidence(dom: &Dom, svg: NodeId, hidden: &[bool]) -> bool {
    let structural_name = [AttrName::Class, AttrName::Id]
        .into_iter()
        .filter_map(|attribute| dom.attr(svg, attribute))
        .flat_map(|value| {
            value
                .split(|character: char| !character.is_ascii_alphanumeric())
                .filter(|token| !token.is_empty())
        })
        .any(|token| {
            matches!(
                token.to_ascii_lowercase().as_str(),
                "benchmark" | "benchmarks" | "chart" | "graph" | "plot"
            )
        });
    if structural_name {
        return true;
    }

    let mut text_count = 0;
    let mut has_number = false;
    let mut has_label = false;
    for value in dom
        .descendants(svg)
        .filter(|&node| is_svg_element(dom, node, "text"))
        .filter(|&node| !hidden[node.index()])
        .filter_map(|node| normalized_text(dom, node))
    {
        text_count += 1;
        has_number |= is_chart_number(&value);
        has_label |= value.chars().any(char::is_alphabetic);
        if text_count >= 2 && has_number && has_label {
            return true;
        }
    }
    let accessible_name = dom.attr(svg, AttrName::AriaLabel).is_some_and(|value| {
        value
            .split(|character: char| !character.is_ascii_alphanumeric())
            .any(|token| {
                matches!(
                    token.to_ascii_lowercase().as_str(),
                    "benchmark" | "benchmarks" | "chart" | "graph" | "plot"
                )
            })
    });
    accessible_name
        && dom.descendants(svg).any(|node| {
            !hidden[node.index()]
                && (is_svg_element(dom, node, "title") || is_svg_element(dom, node, "desc"))
        })
}

#[derive(Clone)]
struct ChartRow {
    label: String,
    value: String,
    source_texts: SmallVec<[NodeId; 2]>,
}

struct ChartContent {
    rows: Vec<ChartRow>,
    accessible_labels: Vec<String>,
    label_source_texts: Vec<NodeId>,
}

fn chart_content(
    dom: &Dom,
    svg: NodeId,
    labelled_content: &HashMap<String, (NodeId, String)>,
    hidden: &[bool],
) -> ChartContent {
    let mut content = ChartContent {
        rows: Vec::new(),
        accessible_labels: Vec::new(),
        label_source_texts: Vec::new(),
    };
    let mut seen_rows = HashSet::new();
    let mut seen_labels = HashSet::new();
    for node in std::iter::once(svg)
        .chain(dom.descendants(svg))
        .filter(|&node| dom.is_element(node) && !hidden[node.index()])
    {
        let text_nodes: SmallVec<[NodeId; 4]> = dom
            .element_children(node)
            .filter(|&node| is_svg_element(dom, node, "text"))
            .filter(|&node| !hidden[node.index()])
            .collect();
        if text_nodes.len() == 2 {
            let values: SmallVec<[String; 2]> = text_nodes
                .iter()
                .filter_map(|&text| normalized_text(dom, text))
                .collect();
            if values.len() == 2
                && let Some((label, value)) = label_and_value(&values[0], &values[1])
            {
                push_chart_row(
                    &mut content.rows,
                    &mut seen_rows,
                    label,
                    value,
                    text_nodes.iter().copied().collect(),
                );
            } else if values.len() == 2 {
                preserve_accessible_label(
                    &mut content,
                    &mut seen_rows,
                    &mut seen_labels,
                    &format!("{}: {}", values[0], values[1]),
                    text_nodes.iter().copied().collect(),
                );
            }
        }

        if let Some(label) = dom.attr(node, AttrName::AriaLabel) {
            preserve_accessible_label(
                &mut content,
                &mut seen_rows,
                &mut seen_labels,
                label,
                SmallVec::new(),
            );
        }
        if let Some(references) = dom.attr_by_local_name(node, "aria-labelledby") {
            let resolved: SmallVec<[(NodeId, &str); 4]> = references
                .split_ascii_whitespace()
                .take(MAX_ACCESSIBLE_REFERENCES_PER_NODE)
                .filter_map(|reference| {
                    labelled_content
                        .get(reference)
                        .map(|(node, value)| (*node, value.as_str()))
                })
                .filter(|(source, _)| !reference_text_is_retained_in_chart(dom, svg, *source))
                .collect();
            if !resolved.is_empty() {
                if resolved.len() == 2
                    && let Some((label, value)) = label_and_value(resolved[0].1, resolved[1].1)
                {
                    push_chart_row(
                        &mut content.rows,
                        &mut seen_rows,
                        label,
                        value,
                        resolved.iter().map(|(node, _)| *node).collect(),
                    );
                    continue;
                }
                let joined = resolved
                    .iter()
                    .map(|(_, value)| *value)
                    .collect::<SmallVec<[&str; 4]>>()
                    .join(" ");
                preserve_accessible_label(
                    &mut content,
                    &mut seen_rows,
                    &mut seen_labels,
                    &joined,
                    resolved.iter().map(|(node, _)| *node).collect(),
                );
            }
        }
        if node != svg {
            for label in dom
                .element_children(node)
                .filter(|&child| {
                    !hidden[child.index()]
                        && (is_svg_element(dom, child, "title")
                            || is_svg_element(dom, child, "desc"))
                })
                .filter_map(|child| normalized_text(dom, child))
            {
                preserve_accessible_label(
                    &mut content,
                    &mut seen_rows,
                    &mut seen_labels,
                    &label,
                    SmallVec::new(),
                );
            }
        }
    }
    content
}

fn labelled_content(dom: &Dom, root: NodeId) -> HashMap<String, (NodeId, String)> {
    let references: HashSet<&str> = std::iter::once(root)
        .chain(dom.descendants(root))
        .filter(|&node| svg_local_name(dom, node).is_some())
        .filter_map(|node| dom.attr_by_local_name(node, "aria-labelledby"))
        .flat_map(str::split_ascii_whitespace)
        .take(MAX_ACCESSIBLE_REFERENCES)
        .collect();
    if references.is_empty() {
        return HashMap::new();
    }
    std::iter::once(root)
        .chain(dom.descendants(root))
        .filter(|&node| dom.is_element(node))
        .filter_map(|node| {
            let id = dom.attr(node, AttrName::Id)?;
            if !references.contains(id) {
                return None;
            }
            Some((id.to_owned(), (node, normalized_text(dom, node)?)))
        })
        .collect()
}

fn reference_text_is_retained_in_chart(dom: &Dom, svg: NodeId, source: NodeId) -> bool {
    dom.parent(source) == Some(svg)
        && (is_svg_element(dom, source, "title") || is_svg_element(dom, source, "desc"))
}

fn hidden_nodes(dom: &Dom, root: NodeId) -> Vec<bool> {
    let mut hidden = vec![false; dom.len()];
    hidden[root.index()] = dom.attr(root, AttrName::AriaHidden) == Some("true");
    for (node, _) in dom.element_descendants_snapshot_with_depth(root) {
        hidden[node.index()] = dom.attr(node, AttrName::AriaHidden) == Some("true")
            || dom
                .parent(node)
                .is_some_and(|parent| hidden[parent.index()]);
    }
    hidden
}

fn label_and_value<'a>(first: &'a str, second: &'a str) -> Option<(&'a str, &'a str)> {
    match (is_chart_number(first), is_chart_number(second)) {
        (true, false) if second.chars().any(char::is_alphabetic) => Some((second, first)),
        (false, true) if first.chars().any(char::is_alphabetic) => Some((first, second)),
        _ => None,
    }
}

fn parse_accessible_pair(value: &str) -> Option<(&str, &str)> {
    [':', '='].into_iter().find_map(|separator| {
        let (first, second) = value.rsplit_once(separator)?;
        label_and_value(first.trim(), second.trim())
    })
}

fn push_chart_row(
    rows: &mut Vec<ChartRow>,
    seen: &mut HashSet<(String, String)>,
    label: &str,
    value: &str,
    source_texts: SmallVec<[NodeId; 2]>,
) {
    let key = (label.to_owned(), value.to_owned());
    if seen.insert(key.clone()) {
        rows.push(ChartRow {
            label: key.0,
            value: key.1,
            source_texts,
        });
    }
}

fn preserve_accessible_label(
    content: &mut ChartContent,
    seen_rows: &mut HashSet<(String, String)>,
    seen_labels: &mut HashSet<String>,
    label: &str,
    source_texts: SmallVec<[NodeId; 2]>,
) {
    let label = label.trim();
    if label.is_empty() {
        return;
    }
    if let Some((name, value)) = parse_accessible_pair(label) {
        push_chart_row(&mut content.rows, seen_rows, name, value, source_texts);
    } else if seen_labels.insert(label.to_owned()) {
        content.accessible_labels.push(label.to_owned());
        content.label_source_texts.extend(source_texts);
    }
}

fn chart_descriptions(
    dom: &Dom,
    svg: NodeId,
    rows: &[ChartRow],
    label_source_texts: &[NodeId],
    hidden: &[bool],
) -> Vec<String> {
    let mut descriptions = Vec::new();
    for local in ["title", "desc"] {
        for value in dom
            .element_children(svg)
            .filter(|&node| !hidden[node.index()] && is_svg_element(dom, node, local))
            .filter_map(|node| normalized_text(dom, node))
        {
            push_unique(&mut descriptions, value);
        }
    }

    if descriptions.is_empty()
        && let Some(value) = standalone_chart_title(dom, svg, rows, label_source_texts, hidden)
    {
        push_unique(&mut descriptions, value);
    }
    if let Some(value) = dom
        .attr(svg, AttrName::AriaLabel)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        push_unique(&mut descriptions, value.to_owned());
    }
    descriptions
}

fn standalone_chart_title(
    dom: &Dom,
    svg: NodeId,
    rows: &[ChartRow],
    label_source_texts: &[NodeId],
    hidden: &[bool],
) -> Option<String> {
    let mut row_text = vec![false; dom.len()];
    for text in rows.iter().flat_map(|row| &row.source_texts) {
        row_text[text.index()] = true;
    }
    for &text in label_source_texts {
        row_text[text.index()] = true;
    }
    dom.descendants(svg)
        .filter(|&node| is_svg_element(dom, node, "text"))
        .filter(|&node| !hidden[node.index()] && !row_text[node.index()])
        .filter_map(|node| normalized_text(dom, node))
        .find(|value| value.len() <= 160 && value.chars().any(char::is_alphabetic))
}

fn append_chart_table(dom: &mut Dom, container: NodeId, svg: NodeId, rows: &[ChartRow]) {
    let (label_header, value_header) = table_headers(dom, svg);
    let Ok(table) = dom.create_html_element(Tag::Table) else {
        return;
    };
    let Ok(head) = dom.create_html_element(Tag::Thead) else {
        return;
    };
    let Ok(body) = dom.create_html_element(Tag::Tbody) else {
        return;
    };
    dom.append_child(table, head);
    dom.append_child(table, body);
    append_table_row(dom, head, Tag::Th, &[label_header, value_header]);
    for row in rows {
        append_table_row(dom, body, Tag::Td, &[&row.label, &row.value]);
    }
    dom.append_child(container, table);
}

fn table_headers(dom: &Dom, svg: NodeId) -> (&'static str, &'static str) {
    let label = dom.attr(svg, AttrName::AriaLabel).unwrap_or_default();
    let lowercase = label.to_ascii_lowercase();
    if lowercase.contains("model") && lowercase.contains("score") {
        ("Model", "Score")
    } else {
        ("Label", "Value")
    }
}

fn append_table_row(dom: &mut Dom, section: NodeId, cell_tag: Tag, values: &[&str]) {
    let Ok(row) = dom.create_html_element(Tag::Tr) else {
        return;
    };
    for value in values {
        let Ok(cell) = dom.create_html_element(cell_tag) else {
            return;
        };
        let Ok(text) = dom.create_text(value) else {
            return;
        };
        dom.append_child(cell, text);
        dom.append_child(row, cell);
    }
    dom.append_child(section, row);
}

fn normalized_text(dom: &Dom, node: NodeId) -> Option<String> {
    let mut value = String::new();
    dom.append_normalized_text(node, &mut value);
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

fn is_chart_number(value: &str) -> bool {
    let value = value.trim().trim_end_matches('%').replace(',', "");
    !value.is_empty() && value.parse::<f64>().is_ok()
}

fn push_unique(values: &mut Vec<String>, value: String) {
    if !values.iter().any(|existing| existing == &value) {
        values.push(value);
    }
}

fn is_math_rendering(dom: &Dom, svg: NodeId) -> bool {
    dom.ancestors(svg).any(|node| {
        dom.tag(node) == Some(Tag::Math)
            || dom.attr(node, AttrName::DataMath).is_some()
            || dom
                .qual_name(node)
                .is_some_and(|name| name.local.as_ref().to_ascii_lowercase().starts_with("mjx"))
            || dom.attr(node, AttrName::Class).is_some_and(|classes| {
                classes.split_ascii_whitespace().any(|class| {
                    class.eq_ignore_ascii_case("mathjax")
                        || class.to_ascii_lowercase().starts_with("mjx")
                })
            })
    })
}

fn is_svg_element(dom: &Dom, node: NodeId, local: &str) -> bool {
    svg_local_name(dom, node).is_some_and(|name| name.eq_ignore_ascii_case(local))
}

fn svg_local_name(dom: &Dom, node: NodeId) -> Option<&str> {
    dom.qual_name(node)
        .filter(|name| name.ns == ns!(svg))
        .map(|name| name.local.as_ref())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn semantic_markdown(dom: &Dom, root: NodeId) -> String {
        let document = crate::document::compile_document(
            dom,
            root,
            &crate::document::CompileContext::default(),
        )
        .unwrap();
        crate::render::markdown::render_markdown(
            &document,
            0,
            crate::render::markdown::MarkdownConfig::default(),
        )
    }

    #[test]
    fn removes_svg_css_and_converts_grouped_chart_data() {
        let mut dom = Dom::parse_fragment(
            r#"<svg role="img" aria-label="Scores by model">
                <style>.chart { --ink: black } @media (dark) {} @keyframes fade {}</style>
                <text>AA Intelligence Index</text>
                <g><rect/><text>62</text><text>Fable 5 Max</text></g>
                <g><rect/><text>61</text><text>Grok 4.6</text><animate attributeName="opacity"/><animateColor attributeName="fill"/></g>
            </svg>"#,
            Tag::Div,
        )
        .unwrap();
        let root = dom.root();

        normalize(&mut dom, root);

        assert_eq!(
            semantic_markdown(&dom, root),
            "AA Intelligence Index\n\nScores by model\n\n| Model | Score |\n| --- | --- |\n| Fable 5 Max | 62 |\n| Grok 4.6 | 61 |\n"
        );
        assert!(!dom.text(root).contains("--ink"));
        assert!(!dom.descendants(root).any(|node| {
            matches!(
                svg_local_name(&dom, node),
                Some("style" | "animate" | "animateColor")
            )
        }));
    }

    #[test]
    fn keeps_descriptions_and_removes_unlabeled_svg_implementation() {
        let mut dom = Dom::parse_fragment(
            r#"<svg aria-label="Deployment chart"><title>Build status</title><desc>Three successful builds</desc><path/></svg><svg class="chart"><path/></svg>"#,
            Tag::Div,
        )
        .unwrap();
        let root = dom.root();

        normalize(&mut dom, root);

        assert_eq!(
            semantic_markdown(&dom, root),
            "Build status\n\nThree successful builds\n\nDeployment chart\n"
        );
    }

    #[test]
    fn converts_aria_labelled_chart_data() {
        let mut dom = Dom::parse_fragment(
            r#"<svg class="chart" aria-label="Release scores">
                <title>Release comparison</title>
                <g aria-label="Grok 4.6: 61"><rect/></g>
                <g aria-label="Fable 5 Max: 62"><rect/></g>
            </svg>
            <svg class="chart" aria-label="Referenced scores">
                <text id="model-a">Alpha</text><text id="score-a">10</text>
                <text id="model-b">Beta</text><text id="score-b">20</text>
                <rect aria-labelledby="model-a score-a"/>
                <rect aria-labelledby="model-b score-b"/>
            </svg>"#,
            Tag::Div,
        )
        .unwrap();
        let root = dom.root();

        normalize(&mut dom, root);

        assert_eq!(
            semantic_markdown(&dom, root),
            "Release comparison\n\nRelease scores\n\n| Label | Value |\n| --- | --- |\n| Grok 4.6 | 61 |\n| Fable 5 Max | 62 |\n\nReferenced scores\n\n| Label | Value |\n| --- | --- |\n| Alpha | 10 |\n| Beta | 20 |\n"
        );
    }

    #[test]
    fn preserves_descriptive_and_nested_accessible_mark_labels() {
        let mut dom = Dom::parse_fragment(
            r#"<svg class="chart" aria-label="Model comparison">
                <g><title>Grok 4.6: 61</title><rect/></g>
                <g><title>Fable 5 Max: 62</title><rect/></g>
                <g aria-label="Grok 4.6, score 61"><rect/></g>
                <text id="note">Higher is better</text>
                <path aria-labelledby="note"/>
            </svg>"#,
            Tag::Div,
        )
        .unwrap();
        let root = dom.root();

        normalize(&mut dom, root);

        assert_eq!(
            semantic_markdown(&dom, root),
            "Model comparison\n\nGrok 4.6, score 61\n\nHigher is better\n\n| Label | Value |\n| --- | --- |\n| Grok 4.6 | 61 |\n| Fable 5 Max | 62 |\n"
        );
    }

    #[test]
    fn resolves_external_and_non_text_accessible_references() {
        let mut dom = Dom::parse_fragment(
            r#"<p id="external-heading">External comparison</p>
            <svg class="chart" aria-labelledby="external-heading">
                <g><text>Alpha</text><text>10</text></g>
                <g><text>Beta</text><text>20</text></g>
            </svg>
            <svg class="chart" aria-labelledby="chart-title chart-desc">
                <title id="chart-title">Internal comparison</title>
                <desc id="chart-desc">Scores for two releases</desc>
                <g><text>Stable</text><text>30</text></g>
                <g><text>Preview</text><text>40</text></g>
            </svg>"#,
            Tag::Div,
        )
        .unwrap();
        let root = dom.root();

        normalize(&mut dom, root);

        assert_eq!(
            semantic_markdown(&dom, root),
            "External comparison\n\nExternal comparison\n\n| Label | Value |\n| --- | --- |\n| Alpha | 10 |\n| Beta | 20 |\n\nInternal comparison\n\nScores for two releases\n\n| Label | Value |\n| --- | --- |\n| Stable | 30 |\n| Preview | 40 |\n"
        );
    }

    #[test]
    fn handles_nested_chart_svgs_without_stale_state() {
        let mut dom = Dom::parse_fragment(
            r#"<svg class="chart" aria-label="Outer chart">
                <title>Nested comparison</title>
                <svg class="chart">
                    <g><text>Alpha</text><text>10</text></g>
                    <g><text>Beta</text><text>20</text></g>
                </svg>
            </svg>"#,
            Tag::Div,
        )
        .unwrap();
        let root = dom.root();

        normalize(&mut dom, root);

        assert_eq!(
            semantic_markdown(&dom, root),
            "Nested comparison\n\nOuter chart\n\n| Label | Value |\n| --- | --- |\n| Alpha | 10 |\n| Beta | 20 |\n"
        );

        let mut source = String::from(
            r#"<svg class="chart" aria-label="Deep chart"><title>Deep nesting</title>"#,
        );
        for _ in 0..2_000 {
            source.push_str("<svg>");
        }
        for _ in 0..2_000 {
            source.push_str("</svg>");
        }
        source.push_str("</svg>");
        let mut deep = Dom::parse_fragment(&source, Tag::Div).unwrap();
        let deep_root = deep.root();
        normalize(&mut deep, deep_root);
        assert!(semantic_markdown(&deep, deep_root).contains("Deep nesting"));
    }

    #[test]
    fn bounds_deep_accessible_reference_resolution() {
        use std::fmt::Write;

        let mut source = String::new();
        for index in 0..1_000 {
            write!(source, "<div id=label-{index}>").unwrap();
        }
        source.push_str("Referenced description");
        for _ in 0..1_000 {
            source.push_str("</div>");
        }
        source.push_str("<svg class=chart aria-labelledby=\"");
        for index in 0..1_000 {
            write!(source, "label-{index} ").unwrap();
        }
        source.push_str("\"><title>Bounded chart</title></svg>");
        let mut dom = Dom::parse_fragment(&source, Tag::Div).unwrap();
        let root = dom.root();

        normalize(&mut dom, root);

        assert!(semantic_markdown(&dom, root).contains("Bounded chart"));
    }

    #[test]
    fn preserves_formatted_values_and_excludes_hidden_layers() {
        let mut dom = Dom::parse_fragment(
            r#"<svg class="chart" aria-label="Formatted results">
                <g><text>Price</text><text>$10–$20</text></g>
                <g><text>Latency</text><text>61 ± 2 ms</text></g>
                <g aria-hidden="true"><text>Hidden duplicate</text><text>999</text><title>Secret layer</title></g>
            </svg>
            <svg class="chart" aria-hidden="true"><title>Hidden chart</title><text>Secret</text><text>100</text></svg>"#,
            Tag::Div,
        )
        .unwrap();
        let root = dom.root();

        normalize(&mut dom, root);

        let markdown = semantic_markdown(&dom, root);
        assert_eq!(
            markdown,
            "Formatted results\n\nPrice: $10–$20\n\nLatency: 61 ± 2 ms\n"
        );
        assert!(!markdown.contains("Hidden"));
        assert!(!markdown.contains("Secret"));
        assert!(!markdown.contains("999"));
    }

    #[test]
    fn preserves_non_chart_svg_structure() {
        let mut dom = Dom::parse_fragment(
            r#"<svg aria-label="Open menu"><title>Menu</title><path d="M0 0"/></svg><svg role="img" aria-label="Index"><title>Index</title><path/></svg><svg class="score-icon" aria-label="Score"><title>Score</title><circle/></svg>"#,
            Tag::Div,
        )
        .unwrap();
        let root = dom.root();

        normalize(&mut dom, root);

        assert!(
            dom.descendants(root)
                .filter(|&node| is_svg_element(&dom, node, "svg"))
                .count()
                == 3
        );
        assert_eq!(dom.text(root).trim(), "MenuIndexScore");
    }

    #[test]
    fn handles_large_charts_in_linear_passes() {
        use std::fmt::Write;

        let mut source = String::from(r#"<svg class="chart"><title>Large comparison</title>"#);
        for index in 0..2_000 {
            write!(
                source,
                "<g><text>{index}</text><text>Model {index}</text></g>"
            )
            .unwrap();
        }
        source.push_str("</svg>");
        let mut dom = Dom::parse_fragment(&source, Tag::Div).unwrap();
        let root = dom.root();

        normalize(&mut dom, root);

        let markdown = semantic_markdown(&dom, root);
        assert!(markdown.contains("| Model 0 | 0 |"));
        assert!(markdown.contains("| Model 1999 | 1999 |"));
    }
}
