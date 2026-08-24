//! Canonical semantic HTML rendering.

use std::fmt::{self, Write as _};

use crate::document::{Document, ListKind, MediaKind, OperationKind, SemanticItemView as Item};

pub(crate) fn render_html(document: &Document, capacity: usize) -> String {
    let mut output = String::with_capacity(capacity.max(512));
    write_html(document, &mut output).expect("writing to a String cannot fail");
    output
}

pub(crate) fn write_html<W: fmt::Write>(document: &Document, writer: &mut W) -> fmt::Result {
    let mut output = HtmlOutput::new(writer);
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
            Item::Text(text) => {
                escape_text(&mut output, text);
            }
            Item::Paragraph => {
                open(&mut output, "p");
            }
            Item::Heading { level } => {
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
            Item::BlockGroup => {
                open(&mut output, "div");
            }
            Item::BlockQuote | Item::Callout(_) => {
                open(&mut output, "blockquote");
            }
            Item::Strong => {
                open(&mut output, "strong");
            }
            Item::Emphasis => {
                open(&mut output, "em");
            }
            Item::Strikethrough => {
                open(&mut output, "del");
            }
            Item::List(list) => match list.kind {
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
            Item::ListItem => {
                open(&mut output, "li");
            }
            Item::Table(_) => {
                open(&mut output, "table");
            }
            Item::TableCaption => {
                open(&mut output, "caption");
            }
            Item::TableRow => {
                open(&mut output, "tr");
            }
            Item::TableCell(cell) => {
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
            Item::Figure => {
                open(&mut output, "figure");
            }
            Item::Figcaption => {
                open(&mut output, "figcaption");
            }
            Item::Details => {
                open(&mut output, "details");
            }
            Item::Summary => {
                open(&mut output, "summary");
            }
            Item::DefinitionList => {
                open(&mut output, "dl");
            }
            Item::DefinitionTerm => {
                open(&mut output, "dt");
            }
            Item::DefinitionDescription => {
                open(&mut output, "dd");
            }
            Item::FootnoteDefinition(footnote) => {
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
            Item::Link(link) => {
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
            Item::CodeBlock(code) => {
                output.push_str("<pre><code");
                if let Some(language) = &code.language {
                    output.push_str(" class=\"language-");
                    escape_attribute(&mut output, language);
                    output.push('"');
                }
                output.push('>');
                escape_text(&mut output, code.text());
                output.push_str("</code></pre>");
            }
            Item::InlineCode(code) => {
                output.push_str("<code>");
                escape_text(&mut output, code);
                output.push_str("</code>");
            }
            Item::Image(image) => {
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
            Item::HardBreak => {
                output.push_str("<br>");
            }
            Item::ThematicBreak => {
                output.push_str("<hr>");
            }
            Item::FootnoteReference(footnote) => {
                if let Some(label) = document.footnote_label(footnote) {
                    output.push_str("<sup><a href=\"#footnote-");
                    escape_attribute(&mut output, label);
                    output.push_str("\" role=\"doc-noteref\">");
                    escape_text(&mut output, label);
                    output.push_str("</a></sup>");
                }
            }
            Item::TaskMarker(marker) => {
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
            Item::InlineMath(math) => {
                output.push_str("<span class=\"math\">");
                escape_text(&mut output, &math.source);
                output.push_str("</span>");
            }
            Item::DisplayMath(math) => {
                output.push_str("<span class=\"math display-math\">");
                escape_text(&mut output, &math.source);
                output.push_str("</span>");
            }
            Item::Media(media) => match media.kind {
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
            Item::Invalid => {}
        }
        if document
            .operation_kind(index)
            .is_some_and(OperationKind::is_container)
        {
            containers.push(document.operation_kind(index).unwrap());
        }
    }
    output.finish()
}

struct HtmlOutput<'writer> {
    writer: &'writer mut dyn fmt::Write,
    error: Option<fmt::Error>,
}

impl<'writer> HtmlOutput<'writer> {
    fn new<W: fmt::Write>(writer: &'writer mut W) -> Self {
        Self {
            writer,
            error: None,
        }
    }

    fn push(&mut self, value: char) {
        let _ = self.write_char(value);
    }

    fn push_str(&mut self, value: &str) {
        let _ = self.write_str(value);
    }

    fn finish(self) -> fmt::Result {
        self.error.map_or(Ok(()), Err)
    }
}

impl fmt::Write for HtmlOutput<'_> {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        if self.error.is_none()
            && let Err(error) = self.writer.write_str(value)
        {
            self.error = Some(error);
        }
        Ok(())
    }

    fn write_char(&mut self, value: char) -> fmt::Result {
        if self.error.is_none()
            && let Err(error) = self.writer.write_char(value)
        {
            self.error = Some(error);
        }
        Ok(())
    }
}

fn html_close_tag(node: Item, parent_is_list: bool) -> Option<&'static str> {
    Some(match node {
        Item::Paragraph => "p",
        Item::BlockGroup => "div",
        Item::Heading { level } => match level {
            1 => "h1",
            2 => "h2",
            3 => "h3",
            4 => "h4",
            5 => "h5",
            _ => "h6",
        },
        Item::BlockQuote | Item::Callout(_) => "blockquote",
        Item::List(list) => match list.kind {
            ListKind::Unordered => "ul",
            ListKind::Ordered => "ol",
        },
        Item::ListItem => "li",
        Item::Table(_) => "table",
        Item::TableCaption => "caption",
        Item::TableRow => "tr",
        Item::TableCell(cell) => {
            if cell.header {
                "th"
            } else {
                "td"
            }
        }
        Item::Figure => "figure",
        Item::Figcaption => "figcaption",
        Item::Details => "details",
        Item::Summary => "summary",
        Item::DefinitionList => "dl",
        Item::DefinitionTerm => "dt",
        Item::DefinitionDescription => "dd",
        Item::FootnoteDefinition(_) => {
            if parent_is_list {
                "li"
            } else {
                "aside"
            }
        }
        Item::Emphasis => "em",
        Item::Strong => "strong",
        Item::Strikethrough => "del",
        Item::Link(_) => "a",
        _ => return None,
    })
}

fn open(output: &mut HtmlOutput<'_>, tag: &'static str) {
    output.push('<');
    output.push_str(tag);
    output.push('>');
}

fn escape_text(output: &mut HtmlOutput<'_>, value: &str) {
    if !value
        .as_bytes()
        .iter()
        .any(|&byte| matches!(byte, b'&' | b'<' | b'>'))
    {
        output.push_str(value);
        return;
    }
    let mut start = 0;
    for (index, character) in value.char_indices() {
        let replacement = match character {
            '&' => "&amp;",
            '<' => "&lt;",
            '>' => "&gt;",
            _ => continue,
        };
        output.push_str(&value[start..index]);
        output.push_str(replacement);
        start = index + character.len_utf8();
    }
    output.push_str(&value[start..]);
}

fn escape_attribute(output: &mut HtmlOutput<'_>, value: &str) {
    if !value
        .as_bytes()
        .iter()
        .any(|&byte| matches!(byte, b'&' | b'<' | b'>' | b'"' | b'\''))
    {
        output.push_str(value);
        return;
    }
    let mut start = 0;
    for (index, character) in value.char_indices() {
        let replacement = match character {
            '&' => "&amp;",
            '<' => "&lt;",
            '>' => "&gt;",
            '"' => "&quot;",
            '\'' => "&#39;",
            _ => continue,
        };
        output.push_str(&value[start..index]);
        output.push_str(replacement);
        start = index + character.len_utf8();
    }
    output.push_str(&value[start..]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{Link, SemanticKind as Item, SemanticTapeBuilder};

    #[test]
    fn deeply_nested_html_rendering_is_stack_safe() {
        const DEPTH: usize = 10_000;
        let mut builder = SemanticTapeBuilder::with_capacity(DEPTH + 1);
        let mut parent = None;
        let mut containers = Vec::with_capacity(DEPTH);
        for _ in 0..DEPTH {
            let container = builder.emit(parent, Item::BlockGroup).unwrap();
            containers.push(container);
            parent = Some(container);
        }
        builder.append_prose(parent, "deep").unwrap();
        for container in containers.into_iter().rev() {
            builder.close(container).unwrap();
        }
        let html = render_html(&builder.finish().unwrap(), 0);
        assert!(html.contains("deep"));
    }

    #[test]
    fn semantic_values_are_escaped_in_canonical_html() {
        let mut builder = SemanticTapeBuilder::with_capacity(3);
        let paragraph = builder.emit(None, Item::Paragraph).unwrap();
        builder
            .append_prose(Some(paragraph), "a < b & c > 世界")
            .unwrap();
        let link = builder
            .emit(
                Some(paragraph),
                Item::Link(Link {
                    destination: "https://example.test/?a=1&b=2".into(),
                    title: Some("a \"title\" 'quote'".into()),
                    fragment_only: false,
                }),
            )
            .unwrap();
        builder.append_prose(Some(link), "link 世界").unwrap();
        builder.close(link).unwrap();
        builder.close(paragraph).unwrap();
        let document = builder.finish().unwrap();
        document.validate().unwrap();
        assert_eq!(
            render_html(&document, 0),
            "<p>a &lt; b &amp; c &gt; 世界<a href=\"https://example.test/?a=1&amp;b=2\" title=\"a &quot;title&quot; &#39;quote&#39;\">link 世界</a></p>"
        );
    }
}
