use super::{Document, DocumentNodeId, NodeKindView as NodeKind};
use smallvec::SmallVec;

/// Measurements derived from the semantic document.
///
/// These values describe the retained semantic result. Source-only HTML
/// structure and removed page chrome are not included.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct DocumentStats {
    /// Number of characters in normalized document text.
    pub text_length: usize,
    /// Number of whitespace-separated words in normalized document text.
    pub word_count: usize,
    /// Number of characters contributed by link content.
    pub link_text_length: usize,
    /// Weighted link text length divided by normalized document text length.
    /// Fragment-only links with a non-empty target contribute 0.3 weight.
    pub link_density: f64,
    /// Number of semantic paragraphs.
    pub paragraph_count: usize,
    /// Number of semantic headings.
    pub heading_count: usize,
    /// Number of semantic list items.
    pub list_item_count: usize,
    /// Number of semantic code blocks.
    pub code_block_count: usize,
    /// Number of semantic data tables.
    pub table_count: usize,
    /// Number of semantic figures.
    pub figure_count: usize,
    /// Number of semantic images.
    pub image_count: usize,
    /// Number of footnote references.
    pub footnote_reference_count: usize,
    /// Number of footnote definitions.
    pub footnote_definition_count: usize,
    /// Number of inline and display math expressions.
    pub math_count: usize,
    /// Number of blocks that provide useful structural evidence.
    pub structured_block_count: usize,
    /// Whether normalized text contains an alphanumeric character.
    pub has_alphanumeric_text: bool,
    /// Number of alphabetic characters in normalized text.
    pub alphabetic_chars: usize,
    /// Number of numeric characters in normalized text.
    pub digit_chars: usize,
    /// Whether the result contains table-header, code, or math context.
    pub has_contextual_structure: bool,
}

#[derive(Clone, Copy, Default, Eq, PartialEq)]
pub(crate) enum Separator {
    #[default]
    None,
    Space,
    Newline,
}

pub(crate) fn walk_text(
    document: &Document,
    block_newlines: bool,
    preserve_line_breaks: bool,
    capacity: Option<usize>,
    collect_stats: bool,
) -> (Option<String>, DocumentStats) {
    let roots: SmallVec<[_; 16]> = document.root_ids().collect();
    walk_text_from_roots(
        document,
        &roots,
        block_newlines,
        preserve_line_breaks,
        capacity,
        collect_stats,
    )
}

pub(crate) fn render_document_text(document: &Document, capacity: usize) -> String {
    walk_text(document, false, false, Some(capacity), false)
        .0
        .unwrap_or_default()
}

pub(crate) fn compute_document_stats(document: &Document) -> DocumentStats {
    walk_text(document, false, false, None, true).1
}

pub(crate) fn render_node_text(document: &Document, root: DocumentNodeId) -> String {
    walk_text_from_roots(document, &[root], false, false, Some(0), false)
        .0
        .unwrap_or_default()
}

fn walk_text_from_roots(
    document: &Document,
    roots: &[DocumentNodeId],
    block_newlines: bool,
    preserve_line_breaks: bool,
    capacity: Option<usize>,
    collect_stats: bool,
) -> (Option<String>, DocumentStats) {
    enum Task {
        Node(DocumentNodeId),
        Siblings(DocumentNodeId),
        Boundary(Separator),
        EndLink { fragment: LinkFragment },
    }

    let block = if block_newlines {
        Separator::Newline
    } else {
        Separator::Space
    };
    let mut output = NormalizedOutput::new(capacity, collect_stats);
    let mut tasks = SmallVec::<[Task; 32]>::new();
    tasks.extend(roots.iter().rev().copied().map(Task::Node));
    while let Some(task) = tasks.pop() {
        let id = match task {
            Task::Boundary(separator) => {
                output.separator(separator);
                continue;
            }
            Task::EndLink { fragment } => {
                output.end_link(fragment);
                continue;
            }
            Task::Node(id) => id,
            Task::Siblings(id) => {
                if let Some(sibling) = document.next_sibling(id) {
                    tasks.push(Task::Siblings(sibling));
                }
                id
            }
        };
        let Some(node) = document.node(id) else {
            continue;
        };
        output.count(&node.kind());
        match node.kind() {
            NodeKind::Text(text) => output.text(text),
            NodeKind::CodeBlock(code) => {
                output.separator(block);
                output.text(&code.text);
                output.separator(block);
            }
            NodeKind::InlineCode(code) => output.text(code),
            NodeKind::Image(_) | NodeKind::FootnoteReference(_) | NodeKind::ThematicBreak => {}
            NodeKind::TaskMarker(marker) => {
                if let Some(label) = &marker.fallback_label {
                    output.text(label);
                }
            }
            NodeKind::InlineMath(math) => {
                output.text(math.fallback_text.as_deref().unwrap_or(&math.source));
            }
            NodeKind::DisplayMath(math) => {
                output.separator(block);
                output.text(math.fallback_text.as_deref().unwrap_or(&math.source));
                output.separator(block);
            }
            NodeKind::Media(media) => {
                if let Some(title) = &media.title {
                    output.text(title);
                }
            }
            NodeKind::HardBreak => output.separator(if preserve_line_breaks {
                Separator::Newline
            } else {
                Separator::Space
            }),
            NodeKind::Link(link) => {
                output.begin_link();
                tasks.push(Task::EndLink {
                    fragment: LinkFragment {
                        hash: link.fragment_only,
                    },
                });
                if let Some(child) = document.first_child(id) {
                    tasks.push(Task::Siblings(child));
                }
            }
            _ => {
                if is_block(&node.kind()) {
                    output.separator(block);
                    tasks.push(Task::Boundary(block));
                }
                if let Some(child) = document.first_child(id) {
                    tasks.push(Task::Siblings(child));
                }
            }
        }
    }
    output.finish()
}

fn is_block(kind: &NodeKind) -> bool {
    matches!(
        kind,
        NodeKind::Paragraph
            | NodeKind::BlockGroup
            | NodeKind::Heading { .. }
            | NodeKind::BlockQuote
            | NodeKind::ListItem
            | NodeKind::TableCaption
            | NodeKind::TableRow
            | NodeKind::TableCell(_)
            | NodeKind::Figure
            | NodeKind::Figcaption
            | NodeKind::Details
            | NodeKind::Summary
            | NodeKind::DefinitionTerm
            | NodeKind::DefinitionDescription
            | NodeKind::Callout(_)
            | NodeKind::FootnoteDefinition(_)
    )
}

#[derive(Clone, Copy)]
struct LinkFragment {
    hash: bool,
}

#[derive(Default)]
struct LinkOutput {
    pending: bool,
    character_count: usize,
}

impl LinkOutput {
    fn text(&mut self, text: &str, is_ascii: bool) {
        if is_ascii {
            self.text_ascii(text);
        } else {
            self.text_unicode(text);
        }
    }

    fn text_ascii(&mut self, text: &str) {
        let bytes = text.as_bytes();
        let mut index = 0;
        while index < bytes.len() {
            let run_start = index;
            while index < bytes.len() && !bytes[index].is_ascii_whitespace() {
                index += 1;
            }
            if run_start != index {
                if self.pending {
                    self.character_count += 1;
                    self.pending = false;
                }
                self.character_count += index - run_start;
            }

            let whitespace_start = index;
            while index < bytes.len() && bytes[index].is_ascii_whitespace() {
                index += 1;
            }
            if whitespace_start != index && self.character_count > 0 {
                self.pending = true;
            }
        }
    }

    fn text_unicode(&mut self, text: &str) {
        for character in text.chars() {
            if character.is_whitespace() {
                if self.character_count > 0 {
                    self.pending = true;
                }
            } else {
                if self.pending {
                    self.character_count += 1;
                    self.pending = false;
                }
                self.character_count += 1;
            }
        }
    }

    fn separator(&mut self) {
        if self.character_count > 0 {
            self.pending = true;
        }
    }
}

#[derive(Default)]
struct NormalizedOutput {
    output: Option<String>,
    collect_stats: bool,
    pending: Separator,
    character_count: usize,
    word_count: usize,
    in_word: bool,
    link_output: Option<LinkOutput>,
    link_text_length: usize,
    weighted_link_text_length: f64,
    paragraph_count: usize,
    heading_count: usize,
    list_item_count: usize,
    code_block_count: usize,
    table_count: usize,
    figure_count: usize,
    image_count: usize,
    footnote_reference_count: usize,
    footnote_definition_count: usize,
    math_count: usize,
    structured_block_count: usize,
    has_alphanumeric_text: bool,
    alphabetic_chars: usize,
    digit_chars: usize,
    has_contextual_structure: bool,
}

impl NormalizedOutput {
    fn new(capacity: Option<usize>, collect_stats: bool) -> Self {
        Self {
            output: capacity.map(String::with_capacity),
            collect_stats,
            ..Self::default()
        }
    }

    fn count(&mut self, kind: &NodeKind) {
        if !self.collect_stats {
            return;
        }
        match kind {
            NodeKind::Paragraph => self.paragraph_count += 1,
            NodeKind::Heading { .. } => self.heading_count += 1,
            NodeKind::ListItem => self.list_item_count += 1,
            NodeKind::CodeBlock(_) => {
                self.code_block_count += 1;
                self.structured_block_count += 1;
                self.has_contextual_structure = true;
            }
            NodeKind::Table(_) => {
                self.table_count += 1;
                self.structured_block_count += 1;
            }
            NodeKind::TableCell(cell) => {
                self.has_contextual_structure |= cell.header;
            }
            NodeKind::Figure => {
                self.figure_count += 1;
                self.structured_block_count += 1;
            }
            NodeKind::Image(_) => self.image_count += 1,
            NodeKind::FootnoteReference(_) => self.footnote_reference_count += 1,
            NodeKind::FootnoteDefinition(_) => self.footnote_definition_count += 1,
            NodeKind::InlineMath(_) | NodeKind::DisplayMath(_) => {
                self.math_count += 1;
                self.structured_block_count += 1;
                self.has_contextual_structure = true;
            }
            NodeKind::BlockQuote
            | NodeKind::Details
            | NodeKind::DefinitionList
            | NodeKind::List(_)
            | NodeKind::Callout(_) => self.structured_block_count += 1,
            _ => {}
        }
    }

    fn text(&mut self, text: &str) {
        if self.collect_stats {
            self.text_with_stats(text);
        } else {
            self.text_without_stats(text);
        }
    }

    fn text_with_stats(&mut self, text: &str) {
        let is_ascii = text.is_ascii();
        if let Some(link_output) = &mut self.link_output {
            link_output.text(text, is_ascii);
        }
        if is_ascii {
            self.text_ascii_with_stats(text);
        } else {
            self.text_unicode_with_stats(text);
        }
    }

    fn text_without_stats(&mut self, text: &str) {
        if text.is_ascii() {
            self.text_ascii_without_stats(text);
        } else {
            self.text_unicode_without_stats(text);
        }
    }

    fn text_ascii_with_stats(&mut self, text: &str) {
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
            if !self.in_word {
                self.word_count += 1;
            }
            self.in_word = true;
            let run_start = index;
            while index < bytes.len() && !bytes[index].is_ascii_whitespace() {
                let byte = bytes[index];
                self.has_alphanumeric_text |= byte.is_ascii_alphanumeric();
                self.alphabetic_chars += usize::from(byte.is_ascii_alphabetic());
                self.digit_chars += usize::from(byte.is_ascii_digit());
                index += 1;
            }
            self.character_count += index - run_start;
            if let Some(output) = &mut self.output {
                output.push_str(&text[run_start..index]);
            }
        }
    }

    fn text_ascii_without_stats(&mut self, text: &str) {
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

            self.flush_without_stats();
            let run_start = index;
            while index < bytes.len() && !bytes[index].is_ascii_whitespace() {
                index += 1;
            }
            self.character_count += index - run_start;
            if let Some(output) = &mut self.output {
                output.push_str(&text[run_start..index]);
            }
        }
    }

    fn text_unicode_with_stats(&mut self, text: &str) {
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
                self.has_alphanumeric_text |= character.is_alphanumeric();
                self.alphabetic_chars += usize::from(character.is_alphabetic());
                self.digit_chars += usize::from(character.is_numeric());
                self.character_count += 1;
                if let Some(output) = &mut self.output {
                    output.push(character);
                }
            }
        }
    }

    fn text_unicode_without_stats(&mut self, text: &str) {
        for character in text.chars() {
            if character.is_whitespace() {
                if self.character_count > 0 && self.pending == Separator::None {
                    self.pending = Separator::Space;
                }
            } else {
                self.flush_without_stats();
                self.character_count += 1;
                if let Some(output) = &mut self.output {
                    output.push(character);
                }
            }
        }
    }

    fn separator(&mut self, separator: Separator) {
        if self.collect_stats
            && let Some(link_output) = &mut self.link_output
        {
            link_output.separator();
        }
        if self.character_count > 0
            && (separator == Separator::Newline || self.pending == Separator::None)
        {
            self.pending = separator;
        }
    }

    fn begin_link(&mut self) {
        if !self.collect_stats {
            return;
        }
        debug_assert!(self.link_output.is_none());
        self.link_output = Some(LinkOutput::default());
    }

    fn end_link(&mut self, fragment: LinkFragment) {
        if !self.collect_stats {
            return;
        }
        let Some(link_output) = self.link_output.take() else {
            return;
        };
        self.link_text_length = self
            .link_text_length
            .saturating_add(link_output.character_count);
        let weight = if fragment.hash { 0.3 } else { 1.0 };
        self.weighted_link_text_length += link_output.character_count as f64 * weight;
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

    fn flush_without_stats(&mut self) {
        let separator = match self.pending {
            Separator::None => None,
            Separator::Space => Some(' '),
            Separator::Newline => Some('\n'),
        };
        if let Some(separator) = separator {
            self.character_count += 1;
            if let Some(output) = &mut self.output {
                output.push(separator);
            }
        }
        self.pending = Separator::None;
    }

    fn finish(self) -> (Option<String>, DocumentStats) {
        (
            self.output,
            DocumentStats {
                text_length: self.character_count,
                word_count: self.word_count,
                link_text_length: self.link_text_length,
                link_density: if self.character_count == 0 {
                    0.0
                } else {
                    (self.weighted_link_text_length / self.character_count as f64).clamp(0.0, 1.0)
                },
                paragraph_count: self.paragraph_count,
                heading_count: self.heading_count,
                list_item_count: self.list_item_count,
                code_block_count: self.code_block_count,
                table_count: self.table_count,
                figure_count: self.figure_count,
                image_count: self.image_count,
                footnote_reference_count: self.footnote_reference_count,
                footnote_definition_count: self.footnote_definition_count,
                math_count: self.math_count,
                structured_block_count: self.structured_block_count,
                has_alphanumeric_text: self.has_alphanumeric_text,
                alphabetic_chars: self.alphabetic_chars,
                digit_chars: self.digit_chars,
                has_contextual_structure: self.has_contextual_structure,
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use crate::document::{
        CodeBlock, DocumentBuilder, FootnoteId, Image, Link, List, ListKind, MathFormat, MathValue,
        NodeKind, Table, TableCell,
    };

    #[test]
    fn counts_semantic_result_metrics() {
        let mut builder = DocumentBuilder::with_capacity(30);
        let heading = builder
            .append(None, NodeKind::Heading { level: 2 })
            .unwrap();
        builder.append_prose(Some(heading), "Heading").unwrap();
        let paragraph = builder.append(None, NodeKind::Paragraph).unwrap();
        builder.append_prose(Some(paragraph), "Hello ").unwrap();
        let link = builder
            .append(
                Some(paragraph),
                NodeKind::Link(Link {
                    destination: "https://example.test/guide".into(),
                    title: None,
                    fragment_only: false,
                }),
            )
            .unwrap();
        builder.append_prose(Some(link), "world").unwrap();
        let list = builder
            .append(
                None,
                NodeKind::List(List {
                    kind: ListKind::Unordered,
                    start: None,
                }),
            )
            .unwrap();
        let item = builder.append(Some(list), NodeKind::ListItem).unwrap();
        builder.append_prose(Some(item), "item").unwrap();
        builder.append(Some(list), NodeKind::ListItem).unwrap();
        builder
            .append(
                None,
                NodeKind::CodeBlock(CodeBlock {
                    language: None,
                    text: "let x = 1;".into(),
                }),
            )
            .unwrap();
        let table = builder
            .append(
                None,
                NodeKind::Table(Table {
                    column_count: Some(1),
                }),
            )
            .unwrap();
        let row = builder.append(Some(table), NodeKind::TableRow).unwrap();
        builder
            .append(
                Some(row),
                NodeKind::TableCell(TableCell {
                    header: true,
                    colspan: 1,
                    rowspan: 1,
                    alignment: None,
                }),
            )
            .unwrap();
        let figure = builder.append(None, NodeKind::Figure).unwrap();
        builder
            .append(
                Some(figure),
                NodeKind::Image(Image {
                    source: "image.png".into(),
                    alt: "An image".into(),
                    title: None,
                    width: None,
                    height: None,
                }),
            )
            .unwrap();
        let footnote = FootnoteId::from_index(0).unwrap();
        builder
            .append(None, NodeKind::FootnoteReference(footnote))
            .unwrap();
        let definition = builder
            .append(None, NodeKind::FootnoteDefinition(footnote))
            .unwrap();
        builder.append_prose(Some(definition), "Note").unwrap();
        builder.define_footnote(footnote, "1", definition).unwrap();
        builder
            .append(
                None,
                NodeKind::InlineMath(MathValue {
                    source: "x".into(),
                    format: MathFormat::Tex,
                    fallback_text: Some("x".into()),
                }),
            )
            .unwrap();
        builder
            .append(
                None,
                NodeKind::DisplayMath(MathValue {
                    source: "y".into(),
                    format: MathFormat::Tex,
                    fallback_text: Some("y".into()),
                }),
            )
            .unwrap();

        let document = builder.finish();
        let stats = document.stats();
        assert_eq!(stats.text_length, document.text().chars().count());
        assert_eq!(stats.word_count, 11);
        assert_eq!(stats.link_text_length, 5);
        assert_eq!(stats.paragraph_count, 1);
        assert_eq!(stats.heading_count, 1);
        assert_eq!(stats.list_item_count, 2);
        assert_eq!(stats.code_block_count, 1);
        assert_eq!(stats.table_count, 1);
        assert_eq!(stats.figure_count, 1);
        assert_eq!(stats.image_count, 1);
        assert_eq!(stats.footnote_reference_count, 1);
        assert_eq!(stats.footnote_definition_count, 1);
        assert_eq!(stats.math_count, 2);
        assert_eq!(stats.structured_block_count, 6);
        assert_eq!(stats.link_density, 5.0 / stats.text_length as f64);
    }

    #[test]
    fn link_text_excludes_outer_whitespace() {
        let mut builder = DocumentBuilder::with_capacity(2);
        let link = builder
            .append(
                None,
                NodeKind::Link(Link {
                    destination: "#section".into(),
                    title: None,
                    fragment_only: true,
                }),
            )
            .unwrap();
        builder.append_prose(Some(link), "  linked text  ").unwrap();
        let document = builder.finish();
        let stats = document.stats();
        assert_eq!(stats.link_text_length, 11);
        assert_eq!(stats.link_density, 3.3 / 11.0);
    }

    #[test]
    fn ascii_and_unicode_text_paths_preserve_normalization_and_metrics() {
        let mut builder = DocumentBuilder::with_capacity(4);
        let paragraph = builder.append(None, NodeKind::Paragraph).unwrap();
        builder
            .append_prose(Some(paragraph), "  ASCII\twords 42!  ")
            .unwrap();
        let link = builder
            .append(
                None,
                NodeKind::Link(Link {
                    destination: "https://example.test".into(),
                    title: None,
                    fragment_only: false,
                }),
            )
            .unwrap();
        builder.append_prose(Some(link), "  世界  café  ").unwrap();
        let document = builder.finish();

        assert_eq!(
            super::render_node_text(&document, paragraph),
            "ASCII words 42!"
        );
        assert!(!document.stats_initialized());
        assert_eq!(document.text(), "ASCII words 42! 世界 café");

        let stats = document.stats();
        assert_eq!(stats.text_length, 23);
        assert_eq!(stats.word_count, 5);
        assert_eq!(stats.link_text_length, 7);
        assert_eq!(stats.alphabetic_chars, 16);
        assert_eq!(stats.digit_chars, 2);
        assert!(stats.has_alphanumeric_text);
        assert_eq!(
            super::render_document_text(&document, stats.text_length),
            document.text()
        );
    }

    #[test]
    fn deeply_nested_stats_are_stack_safe() {
        const DEPTH: usize = 10_000;
        let mut builder = DocumentBuilder::with_capacity(DEPTH + 1);
        let mut parent = None;
        for _ in 0..DEPTH {
            parent = Some(builder.append(parent, NodeKind::BlockGroup).unwrap());
        }
        builder.append_prose(parent, "deep").unwrap();
        let document = builder.finish();
        assert_eq!(document.stats().text_length, 4);
    }
}
