//! Normalized text rendering from the cleaned extraction DOM.

use crate::dom::{Dom, NodeId, Tag};
use smallvec::SmallVec;

#[derive(Clone, Debug, Default)]
pub(crate) struct TextOptions {
    block_newlines: bool,
    preserve_line_breaks: bool,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug)]
enum TextSeparator {
    Newline,
}

impl TextOptions {
    #[cfg(test)]
    fn block_separator(mut self, value: TextSeparator) -> Self {
        self.block_newlines = matches!(value, TextSeparator::Newline);
        self
    }

    #[cfg(test)]
    fn preserve_line_breaks(mut self, value: bool) -> Self {
        self.preserve_line_breaks = value;
        self
    }

    fn block_newlines(&self) -> bool {
        self.block_newlines
    }

    fn preserves_line_breaks(&self) -> bool {
        self.preserve_line_breaks
    }
}

pub(crate) fn render_text(
    dom: &Dom,
    root: NodeId,
    capacity: usize,
    options: &TextOptions,
) -> String {
    render_block_text(dom, root, capacity, options)
}

pub(crate) fn measure_text(dom: &Dom, root: NodeId) -> (usize, usize) {
    let mut output = NormalizedOutput::metrics();
    walk_block_text(dom, root, &TextOptions::default(), &mut output);
    output.measurements()
}

fn render_block_text(dom: &Dom, root: NodeId, capacity: usize, options: &TextOptions) -> String {
    let mut output = NormalizedOutput::with_capacity(capacity);
    walk_block_text(dom, root, options, &mut output);
    output.finish()
}

fn walk_block_text(dom: &Dom, root: NodeId, options: &TextOptions, output: &mut NormalizedOutput) {
    enum Task {
        Node(NodeId),
        BlockEnd,
    }

    let separator = if options.block_newlines() {
        Separator::Newline
    } else {
        Separator::Space
    };
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
                if tag == Some(Tag::Br) {
                    output.separator(if options.preserves_line_breaks() {
                        Separator::Newline
                    } else {
                        Separator::Space
                    });
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
}

fn is_text_block(tag: Tag) -> bool {
    matches!(
        tag,
        Tag::P
            | Tag::Div
            | Tag::Article
            | Tag::Address
            | Tag::Aside
            | Tag::Body
            | Tag::Caption
            | Tag::Dd
            | Tag::Details
            | Tag::Dl
            | Tag::Dt
            | Tag::Fieldset
            | Tag::Footer
            | Tag::Form
            | Tag::Header
            | Tag::Main
            | Tag::Nav
            | Tag::Pre
            | Tag::Section
            | Tag::Summary
            | Tag::Figure
            | Tag::Figcaption
            | Tag::Li
            | Tag::Tr
            | Tag::Td
            | Tag::Th
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
    output: Option<String>,
    pending: Separator,
    character_count: usize,
    word_count: usize,
    in_word: bool,
}

impl NormalizedOutput {
    fn with_capacity(capacity: usize) -> Self {
        Self {
            output: Some(String::with_capacity(capacity)),
            ..Self::default()
        }
    }

    fn metrics() -> Self {
        Self::default()
    }

    fn text(&mut self, text: &str) {
        if text.is_ascii() {
            let bytes = text.as_bytes();
            let mut index = 0;
            while index < bytes.len() {
                if bytes[index].is_ascii_whitespace() {
                    if self.character_count > 0 && self.pending == Separator::None {
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
                if !self.in_word {
                    self.word_count += 1;
                }
                self.in_word = true;
                self.character_count += index - start;
                if let Some(output) = &mut self.output {
                    output.push_str(&text[start..index]);
                }
            }
            return;
        }

        for character in text.chars() {
            if character.is_whitespace() {
                if self.character_count > 0 && self.pending == Separator::None {
                    self.pending = Separator::Space;
                }
            } else {
                self.flush();
                if !self.in_word {
                    self.word_count += 1;
                }
                self.in_word = true;
                self.character_count += 1;
                if let Some(output) = &mut self.output {
                    output.push(character);
                }
            }
        }
    }

    fn separator(&mut self, separator: Separator) {
        if self.character_count > 0
            && (separator == Separator::Newline || self.pending == Separator::None)
        {
            self.pending = separator;
        }
    }

    fn flush(&mut self) {
        let separator = match self.pending {
            Separator::None => None,
            Separator::Space => Some(' '),
            Separator::Newline => Some('\n'),
        };
        if let Some(separator) = separator {
            self.character_count += 1;
            self.in_word = false;
            if let Some(output) = &mut self.output {
                output.push(separator);
            }
        }
        self.pending = Separator::None;
    }

    fn finish(self) -> String {
        self.output.unwrap_or_default()
    }

    fn measurements(self) -> (usize, usize) {
        (self.character_count, self.word_count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_boundaries_breaks_blocks_and_templates() {
        let dom = Dom::parse_document(
            "<body><div>A<p>Hello <em>world</em>!</p>C<br>D<template>hidden</template></div><dl><dt>Term</dt><dd>Definition</dd></dl><details><summary>More</summary><pre>fixed text</pre></details></body>",
        )
        .unwrap();
        let body = dom.body().unwrap();
        assert_eq!(
            render_text(&dom, body, 0, &TextOptions::default()),
            "A Hello world! C D Term Definition More fixed text"
        );
        assert_eq!(
            render_text(
                &dom,
                body,
                0,
                &TextOptions::default().preserve_line_breaks(true),
            ),
            "A Hello world! C\nD Term Definition More fixed text"
        );
        assert_eq!(
            render_text(
                &dom,
                body,
                0,
                &TextOptions::default().block_separator(TextSeparator::Newline),
            ),
            "A\nHello world!\nC D\nTerm\nDefinition\nMore\nfixed text"
        );
        assert_eq!(
            render_text(
                &dom,
                body,
                0,
                &TextOptions::default()
                    .block_separator(TextSeparator::Newline)
                    .preserve_line_breaks(true),
            ),
            "A\nHello world!\nC\nD\nTerm\nDefinition\nMore\nfixed text"
        );
    }
}
