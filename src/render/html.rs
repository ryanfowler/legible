//! Canonical semantic HTML rendering.

use smallvec::SmallVec;
use std::fmt::Write as _;

use crate::document::{Document, DocumentNodeId, ListKind, MediaKind, NodeKind};

pub(crate) fn render_html(document: &Document, capacity: usize) -> String {
    enum Task {
        Node(DocumentNodeId, bool),
        Close(&'static str),
    }

    let mut output = String::with_capacity(capacity.max(512));
    let mut tasks = Vec::with_capacity(32);
    tasks.extend(
        document
            .root_ids()
            .rev()
            .map(|root| Task::Node(root, false)),
    );
    while let Some(task) = tasks.pop() {
        match task {
            Task::Close(tag) => {
                output.push_str("</");
                output.push_str(tag);
                output.push('>');
            }
            Task::Node(id, parent_is_list) => {
                let Some(node) = document.node(id) else {
                    continue;
                };
                let close = match node.kind() {
                    NodeKind::Text(text) => {
                        escape_text(&mut output, text);
                        None
                    }
                    NodeKind::Paragraph => open(&mut output, "p"),
                    NodeKind::BlockGroup => open(&mut output, "div"),
                    NodeKind::Heading { level } => {
                        let tag = match level {
                            1 => "h1",
                            2 => "h2",
                            3 => "h3",
                            4 => "h4",
                            5 => "h5",
                            _ => "h6",
                        };
                        open(&mut output, tag)
                    }
                    NodeKind::BlockQuote | NodeKind::Callout(_) => open(&mut output, "blockquote"),
                    NodeKind::Strong => open(&mut output, "strong"),
                    NodeKind::Emphasis => open(&mut output, "em"),
                    NodeKind::Strikethrough => open(&mut output, "del"),
                    NodeKind::List(list) => match list.kind {
                        ListKind::Unordered => open(&mut output, "ul"),
                        ListKind::Ordered => {
                            output.push_str("<ol");
                            if let Some(start) = list.start.filter(|start| *start != 1) {
                                write!(output, " start=\"{start}\"").unwrap();
                            }
                            output.push('>');
                            Some("ol")
                        }
                    },
                    NodeKind::ListItem => open(&mut output, "li"),
                    NodeKind::Table(_) => open(&mut output, "table"),
                    NodeKind::TableCaption => open(&mut output, "caption"),
                    NodeKind::TableRow => open(&mut output, "tr"),
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
                        Some(tag)
                    }
                    NodeKind::Figure => open(&mut output, "figure"),
                    NodeKind::Figcaption => open(&mut output, "figcaption"),
                    NodeKind::Details => open(&mut output, "details"),
                    NodeKind::Summary => open(&mut output, "summary"),
                    NodeKind::DefinitionList => open(&mut output, "dl"),
                    NodeKind::DefinitionTerm => open(&mut output, "dt"),
                    NodeKind::DefinitionDescription => open(&mut output, "dd"),
                    NodeKind::FootnoteDefinition(footnote) => {
                        let tag = if parent_is_list { "li" } else { "aside" };
                        output.push('<');
                        output.push_str(tag);
                        output.push_str(" id=\"footnote-");
                        if let Some(definition) = document.footnote(*footnote) {
                            escape_attribute(&mut output, definition.label());
                        }
                        output.push('"');
                        if !parent_is_list {
                            output.push_str(" role=\"doc-footnote\"");
                        }
                        output.push('>');
                        Some(tag)
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
                        Some("a")
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
                        None
                    }
                    NodeKind::InlineCode(code) => {
                        output.push_str("<code>");
                        escape_text(&mut output, code);
                        output.push_str("</code>");
                        None
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
                        None
                    }
                    NodeKind::HardBreak => {
                        output.push_str("<br>");
                        None
                    }
                    NodeKind::ThematicBreak => {
                        output.push_str("<hr>");
                        None
                    }
                    NodeKind::FootnoteReference(footnote) => {
                        if let Some(definition) = document.footnote(*footnote) {
                            output.push_str("<sup><a href=\"#footnote-");
                            escape_attribute(&mut output, definition.label());
                            output.push_str("\" role=\"doc-noteref\">");
                            escape_text(&mut output, definition.label());
                            output.push_str("</a></sup>");
                        }
                        None
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
                        None
                    }
                    NodeKind::InlineMath(math) => {
                        output.push_str("<span class=\"math\">");
                        escape_text(&mut output, &math.source);
                        output.push_str("</span>");
                        None
                    }
                    NodeKind::DisplayMath(math) => {
                        output.push_str("<span class=\"math display-math\">");
                        escape_text(&mut output, &math.source);
                        output.push_str("</span>");
                        None
                    }
                    NodeKind::Media(media) => {
                        match media.kind {
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
                                escape_text(
                                    &mut output,
                                    media.title.as_deref().unwrap_or(&media.source),
                                );
                                output.push_str("</a>");
                            }
                        }
                        None
                    }
                };
                if let Some(tag) = close {
                    tasks.push(Task::Close(tag));
                    let children: SmallVec<[_; 16]> = document.child_ids(id).collect();
                    let parent_is_list = matches!(node.kind(), NodeKind::List(_));
                    tasks.extend(
                        children
                            .into_iter()
                            .rev()
                            .map(|child| Task::Node(child, parent_is_list)),
                    );
                }
            }
        }
    }
    output
}

fn open(output: &mut String, tag: &'static str) -> Option<&'static str> {
    output.push('<');
    output.push_str(tag);
    output.push('>');
    Some(tag)
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
