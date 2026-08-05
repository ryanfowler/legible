//! Normalized text rendering from the cleaned extraction DOM.

use crate::article::TextOptions;
use crate::dom::{Dom, NodeId, Tag};
use smallvec::SmallVec;

pub(crate) fn render_text(
    dom: &Dom,
    root: NodeId,
    capacity: usize,
    options: &TextOptions,
) -> String {
    if options.preserves_line_breaks() || options.block_newlines() {
        render_block_text(dom, root, capacity, options)
    } else {
        render_normalized_text(dom, root, capacity)
    }
}

fn render_normalized_text(dom: &Dom, root: NodeId, capacity: usize) -> String {
    let mut output = NormalizedOutput::with_capacity(capacity);
    let mut nodes = SmallVec::<[NodeId; 32]>::new();
    nodes.extend(dom.children_rev(root));
    while let Some(node) = nodes.pop() {
        if let Some(text) = dom.text_node(node) {
            output.text(text);
        } else if dom.tag(node) != Some(Tag::Template) {
            nodes.extend(dom.children_rev(node));
        }
    }
    output.finish()
}

fn render_block_text(dom: &Dom, root: NodeId, capacity: usize, options: &TextOptions) -> String {
    enum Task {
        Node(NodeId),
        BlockEnd,
    }

    let separator = if options.block_newlines() {
        Separator::Newline
    } else {
        Separator::Space
    };
    let mut output = NormalizedOutput::with_capacity(capacity);
    let mut tasks = SmallVec::<[Task; 32]>::new();
    tasks.extend(dom.children_rev(root).map(Task::Node));
    while let Some(task) = tasks.pop() {
        match task {
            Task::BlockEnd => output.separator(separator),
            Task::Node(node) => {
                if let Some(text) = dom.text_node(node) {
                    output.text(text);
                    continue;
                }
                let tag = dom.tag(node);
                if tag == Some(Tag::Template) {
                    continue;
                }
                if tag == Some(Tag::Br) && options.preserves_line_breaks() {
                    output.separator(Separator::Newline);
                    continue;
                }
                if tag.is_some_and(is_text_block) {
                    output.separator(separator);
                    tasks.push(Task::BlockEnd);
                }
                tasks.extend(dom.children_rev(node).map(Task::Node));
            }
        }
    }
    output.finish()
}

fn is_text_block(tag: Tag) -> bool {
    matches!(
        tag,
        Tag::P
            | Tag::Div
            | Tag::Article
            | Tag::Section
            | Tag::Li
            | Tag::Blockquote
            | Tag::H1
            | Tag::H2
            | Tag::H3
            | Tag::H4
            | Tag::H5
            | Tag::H6
    )
}

#[derive(Clone, Copy, Default, Eq, PartialEq)]
enum Separator {
    #[default]
    None,
    Space,
    Newline,
}

#[derive(Default)]
struct NormalizedOutput {
    output: String,
    pending: Separator,
}

impl NormalizedOutput {
    fn with_capacity(capacity: usize) -> Self {
        Self {
            output: String::with_capacity(capacity),
            pending: Separator::None,
        }
    }

    fn text(&mut self, text: &str) {
        if text.is_ascii() {
            let bytes = text.as_bytes();
            let mut index = 0;
            while index < bytes.len() {
                if bytes[index].is_ascii_whitespace() {
                    if !self.output.is_empty() && self.pending == Separator::None {
                        self.pending = Separator::Space;
                    }
                    index += 1;
                    continue;
                }
                self.flush();
                let start = index;
                while index < bytes.len() && !bytes[index].is_ascii_whitespace() {
                    index += 1;
                }
                self.output.push_str(&text[start..index]);
            }
            return;
        }

        for character in text.chars() {
            if character.is_whitespace() {
                if !self.output.is_empty() && self.pending == Separator::None {
                    self.pending = Separator::Space;
                }
            } else {
                self.flush();
                self.output.push(character);
            }
        }
    }

    fn separator(&mut self, separator: Separator) {
        if !self.output.is_empty()
            && (separator == Separator::Newline || self.pending == Separator::None)
        {
            self.pending = separator;
        }
    }

    fn flush(&mut self) {
        match self.pending {
            Separator::None => {}
            Separator::Space => self.output.push(' '),
            Separator::Newline => self.output.push('\n'),
        }
        self.pending = Separator::None;
    }

    fn finish(self) -> String {
        self.output
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_boundaries_breaks_blocks_and_templates() {
        let dom = Dom::parse_document(
            "<body><div>A<p>Hello <em>world</em>!</p>C<br>D<template>hidden</template></div></body>",
        )
        .unwrap();
        let body = dom.body().unwrap();
        assert_eq!(
            render_text(&dom, body, 0, &TextOptions::default()),
            "AHello world!CD"
        );
        assert_eq!(
            render_text(
                &dom,
                body,
                0,
                &TextOptions::default().preserve_line_breaks(true),
            ),
            "A Hello world! C\nD"
        );
        assert_eq!(
            render_text(
                &dom,
                body,
                0,
                &TextOptions::default().block_separator(crate::article::TextSeparator::Newline),
            ),
            "A\nHello world!\nCD"
        );
        assert_eq!(
            render_text(
                &dom,
                body,
                0,
                &TextOptions::default()
                    .block_separator(crate::article::TextSeparator::Newline)
                    .preserve_line_breaks(true),
            ),
            "A\nHello world!\nC\nD"
        );
    }
}
