//! Canonical semantic HTML rendering.

use std::fmt::Write as _;

use crate::document::{Document, ListKind, MediaKind, NodeKindView as NodeKind, OperationKind};

pub(crate) fn render_html(document: &Document, capacity: usize) -> String {
    let mut output = String::with_capacity(capacity.max(512));
    let mut containers = Vec::with_capacity(32);
    for (index, operation) in document.operations().iter().copied().enumerate() {
        if operation.is_close() {
            let opening = document.operation_opening_index(operation);
            let parent_is_list = containers
                .len()
                .checked_sub(2)
                .and_then(|index| containers.get(index))
                .copied()
                == Some(OperationKind::List);
            if let Some(node) = document.operation_view(opening) {
                if let Some(tag) = html_close_tag(node, parent_is_list) {
                    output.push_str("</");
                    output.push_str(tag);
                    output.push('>');
                }
                containers.pop();
            }
            continue;
        }

        let parent_is_list = containers.last().copied() == Some(OperationKind::List);
        let Some(node) = document.operation_view(index) else {
            continue;
        };
        match node {
            NodeKind::Text(text) => {
                escape_text(&mut output, text);
            }
            NodeKind::Paragraph => {
                open(&mut output, "p");
            }
            NodeKind::Heading { level } => {
                let tag = match level {
                    1 => "h1",
                    2 => "h2",
                    3 => "h3",
                    4 => "h4",
                    5 => "h5",
                    _ => "h6",
                };
                open(&mut output, tag);
            }
            NodeKind::BlockGroup => {
                open(&mut output, "div");
            }
            NodeKind::BlockQuote | NodeKind::Callout(_) => {
                open(&mut output, "blockquote");
            }
            NodeKind::Strong => {
                open(&mut output, "strong");
            }
            NodeKind::Emphasis => {
                open(&mut output, "em");
            }
            NodeKind::Strikethrough => {
                open(&mut output, "del");
            }
            NodeKind::List(list) => match list.kind {
                ListKind::Unordered => {
                    open(&mut output, "ul");
                }
                ListKind::Ordered => {
                    output.push_str("<ol");
                    if let Some(start) = list.start.filter(|start| *start != 1) {
                        write!(output, " start=\"{start}\"").unwrap();
                    }
                    output.push('>');
                }
            },
            NodeKind::ListItem => {
                open(&mut output, "li");
            }
            NodeKind::Table(_) => {
                open(&mut output, "table");
            }
            NodeKind::TableCaption => {
                open(&mut output, "caption");
            }
            NodeKind::TableRow => {
                open(&mut output, "tr");
            }
            NodeKind::TableCell(cell) => {
                let tag = if cell.header { "th" } else { "td" };
                output.push('<');
                output.push_str(tag);
                if cell.colspan > 1 {
                    write!(output, " colspan=\"{}\"", cell.colspan).unwrap();
                }
                if cell.rowspan > 1 {
                    write!(output, " rowspan=\"{}\"", cell.rowspan).unwrap();
                }
                if let Some(alignment) = cell.alignment {
                    let value = match alignment {
                        crate::document::TableAlignment::Left => "left",
                        crate::document::TableAlignment::Center => "center",
                        crate::document::TableAlignment::Right => "right",
                    };
                    write!(output, " align=\"{value}\"").unwrap();
                }
                output.push('>');
            }
            NodeKind::Figure => {
                open(&mut output, "figure");
            }
            NodeKind::Figcaption => {
                open(&mut output, "figcaption");
            }
            NodeKind::Details => {
                open(&mut output, "details");
            }
            NodeKind::Summary => {
                open(&mut output, "summary");
            }
            NodeKind::DefinitionList => {
                open(&mut output, "dl");
            }
            NodeKind::DefinitionTerm => {
                open(&mut output, "dt");
            }
            NodeKind::DefinitionDescription => {
                open(&mut output, "dd");
            }
            NodeKind::FootnoteDefinition(footnote) => {
                let tag = if parent_is_list { "li" } else { "aside" };
                output.push('<');
                output.push_str(tag);
                output.push_str(" id=\"footnote-");
                if let Some(label) = document.footnote_label(footnote) {
                    escape_attribute(&mut output, label);
                }
                output.push('"');
                if !parent_is_list {
                    output.push_str(" role=\"doc-footnote\"");
                }
                output.push('>');
            }
            NodeKind::Link(link) => {
                output.push_str("<a href=\"");
                escape_attribute(&mut output, &link.destination);
                output.push('"');
                if let Some(title) = &link.title {
                    output.push_str(" title=\"");
                    escape_attribute(&mut output, title);
                    output.push('"');
                }
                output.push('>');
            }
            NodeKind::CodeBlock(code) => {
                output.push_str("<pre><code");
                if let Some(language) = &code.language {
                    output.push_str(" class=\"language-");
                    escape_attribute(&mut output, language);
                    output.push('"');
                }
                output.push('>');
                escape_text(&mut output, &code.text);
                output.push_str("</code></pre>");
            }
            NodeKind::InlineCode(code) => {
                output.push_str("<code>");
                escape_text(&mut output, code);
                output.push_str("</code>");
            }
            NodeKind::Image(image) => {
                output.push_str("<img src=\"");
                escape_attribute(&mut output, &image.source);
                output.push_str("\" alt=\"");
                escape_attribute(&mut output, &image.alt);
                output.push('"');
                if let Some(title) = &image.title {
                    output.push_str(" title=\"");
                    escape_attribute(&mut output, title);
                    output.push('"');
                }
                if let Some(width) = image.width {
                    write!(output, " width=\"{width}\"").unwrap();
                }
                if let Some(height) = image.height {
                    write!(output, " height=\"{height}\"").unwrap();
                }
                output.push('>');
            }
            NodeKind::HardBreak => {
                output.push_str("<br>");
            }
            NodeKind::ThematicBreak => {
                output.push_str("<hr>");
            }
            NodeKind::FootnoteReference(footnote) => {
                if let Some(label) = document.footnote_label(footnote) {
                    output.push_str("<sup><a href=\"#footnote-");
                    escape_attribute(&mut output, label);
                    output.push_str("\" role=\"doc-noteref\">");
                    escape_text(&mut output, label);
                    output.push_str("</a></sup>");
                }
            }
            NodeKind::TaskMarker(marker) => {
                output.push_str("<input type=\"checkbox\" disabled=\"\"");
                if marker.checked {
                    output.push_str(" checked=\"\"");
                }
                if let Some(label) = &marker.fallback_label {
                    output.push_str(" aria-label=\"");
                    escape_attribute(&mut output, label);
                    output.push('"');
                }
                output.push('>');
                if let Some(label) = &marker.fallback_label {
                    escape_text(&mut output, label);
                }
            }
            NodeKind::InlineMath(math) => {
                output.push_str("<span class=\"math\">");
                escape_text(&mut output, &math.source);
                output.push_str("</span>");
            }
            NodeKind::DisplayMath(math) => {
                output.push_str("<span class=\"math display-math\">");
                escape_text(&mut output, &math.source);
                output.push_str("</span>");
            }
            NodeKind::Media(media) => match media.kind {
                MediaKind::Audio => {
                    output.push_str("<audio controls src=\"");
                    escape_attribute(&mut output, &media.source);
                    output.push_str("\"></audio>");
                }
                MediaKind::Video => {
                    output.push_str("<video controls src=\"");
                    escape_attribute(&mut output, &media.source);
                    output.push_str("\"></video>");
                }
                MediaKind::Embedded => {
                    output.push_str("<a href=\"");
                    escape_attribute(&mut output, &media.source);
                    output.push_str("\">");
                    escape_text(&mut output, media.title.as_deref().unwrap_or(&media.source));
                    output.push_str("</a>");
                }
            },
            NodeKind::Invalid => {}
        }
        if document
            .operation_kind(index)
            .is_some_and(OperationKind::is_container)
        {
            containers.push(document.operation_kind(index).unwrap());
        }
    }
    output
}

fn html_close_tag(node: NodeKind, parent_is_list: bool) -> Option<&'static str> {
    Some(match node {
        NodeKind::Paragraph => "p",
        NodeKind::BlockGroup => "div",
        NodeKind::Heading { level } => match level {
            1 => "h1",
            2 => "h2",
            3 => "h3",
            4 => "h4",
            5 => "h5",
            _ => "h6",
        },
        NodeKind::BlockQuote | NodeKind::Callout(_) => "blockquote",
        NodeKind::List(list) => match list.kind {
            ListKind::Unordered => "ul",
            ListKind::Ordered => "ol",
        },
        NodeKind::ListItem => "li",
        NodeKind::Table(_) => "table",
        NodeKind::TableCaption => "caption",
        NodeKind::TableRow => "tr",
        NodeKind::TableCell(cell) => {
            if cell.header {
                "th"
            } else {
                "td"
            }
        }
        NodeKind::Figure => "figure",
        NodeKind::Figcaption => "figcaption",
        NodeKind::Details => "details",
        NodeKind::Summary => "summary",
        NodeKind::DefinitionList => "dl",
        NodeKind::DefinitionTerm => "dt",
        NodeKind::DefinitionDescription => "dd",
        NodeKind::FootnoteDefinition(_) => {
            if parent_is_list {
                "li"
            } else {
                "aside"
            }
        }
        NodeKind::Emphasis => "em",
        NodeKind::Strong => "strong",
        NodeKind::Strikethrough => "del",
        NodeKind::Link(_) => "a",
        _ => return None,
    })
}

fn open(output: &mut String, tag: &'static str) {
    output.push('<');
    output.push_str(tag);
    output.push('>');
}

fn escape_text(output: &mut String, value: &str) {
    for character in value.chars() {
        match character {
            '&' => output.push_str("&amp;"),
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            _ => output.push(character),
        }
    }
}

fn escape_attribute(output: &mut String, value: &str) {
    for character in value.chars() {
        match character {
            '&' => output.push_str("&amp;"),
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            '"' => output.push_str("&quot;"),
            '\'' => output.push_str("&#39;"),
            _ => output.push(character),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{DocumentBuilder, Link, NodeKind};

    #[test]
    fn deeply_nested_html_rendering_is_stack_safe() {
        const DEPTH: usize = 10_000;
        let mut builder = DocumentBuilder::with_capacity(DEPTH + 1);
        let mut parent = None;
        for _ in 0..DEPTH {
            parent = Some(builder.append(parent, NodeKind::BlockGroup).unwrap());
        }
        builder.append_prose(parent, "deep").unwrap();
        let html = render_html(&builder.finish(), 0);
        assert!(html.contains("deep"));
    }

    #[test]
    fn semantic_values_are_escaped_in_canonical_html() {
        let mut builder = DocumentBuilder::with_capacity(3);
        let paragraph = builder.append(None, NodeKind::Paragraph).unwrap();
        builder.append_prose(Some(paragraph), "a < b & c").unwrap();
        let link = builder
            .append(
                Some(paragraph),
                NodeKind::Link(Link {
                    destination: "https://example.test/?a=1&b=2".into(),
                    title: Some("a \"title\"".into()),
                    fragment_only: false,
                }),
            )
            .unwrap();
        builder.append_prose(Some(link), "link").unwrap();
        let document = builder.finish();
        document.validate().unwrap();
        assert_eq!(
            render_html(&document, 0),
            "<p>a &lt; b &amp; c<a href=\"https://example.test/?a=1&amp;b=2\" title=\"a &quot;title&quot;\">link</a></p>"
        );
    }
}
