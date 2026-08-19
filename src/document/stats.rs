#[cfg(test)]
use super::DocumentNodeId;
use super::{Document, SemanticItemView as Item, SemanticKind as OwnedSemanticKind};

/// Structural measurements collected while semantic operations are emitted.
///
/// These values do not depend on output rendering. Keeping them beside the
/// tape avoids a second semantic traversal when callers request a count.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct CompileStats {
    pub paragraph_count: usize,
    pub heading_count: usize,
    pub list_item_count: usize,
    pub code_block_count: usize,
    pub table_count: usize,
    pub non_empty_table_cell_count: usize,
    pub figure_count: usize,
    pub image_count: usize,
    pub footnote_reference_count: usize,
    pub footnote_definition_count: usize,
    pub math_count: usize,
    pub structured_block_count: usize,
    pub has_contextual_structure: bool,
    pub semantic_text_bytes: usize,
    pub raw_code_bytes: usize,
}

impl CompileStats {
    pub(crate) fn record_kind(&mut self, kind: &OwnedSemanticKind) {
        match kind {
            OwnedSemanticKind::Paragraph => self.paragraph_count += 1,
            OwnedSemanticKind::Heading { .. } => self.heading_count += 1,
            OwnedSemanticKind::ListItem => self.list_item_count += 1,
            OwnedSemanticKind::CodeBlock(code) => {
                self.code_block_count += 1;
                self.structured_block_count += 1;
                self.has_contextual_structure = true;
                self.raw_code_bytes = self.raw_code_bytes.saturating_add(code.text_len());
            }
            OwnedSemanticKind::Table(_) => {
                self.table_count += 1;
                self.structured_block_count += 1;
            }
            OwnedSemanticKind::TableCell(cell) => {
                self.has_contextual_structure |= cell.header;
            }
            OwnedSemanticKind::Figure => {
                self.figure_count += 1;
                self.structured_block_count += 1;
            }
            OwnedSemanticKind::Image(_) => self.image_count += 1,
            OwnedSemanticKind::FootnoteReference(_) => self.footnote_reference_count += 1,
            OwnedSemanticKind::FootnoteDefinition(_) => self.footnote_definition_count += 1,
            OwnedSemanticKind::InlineMath(_) | OwnedSemanticKind::DisplayMath(_) => {
                self.math_count += 1;
                self.structured_block_count += 1;
                self.has_contextual_structure = true;
            }
            OwnedSemanticKind::BlockQuote
            | OwnedSemanticKind::Details
            | OwnedSemanticKind::DefinitionList
            | OwnedSemanticKind::List(_)
            | OwnedSemanticKind::Callout(_) => self.structured_block_count += 1,
            _ => {}
        }
    }

    pub(crate) fn add_semantic_text_bytes(&mut self, bytes: usize) {
        self.semantic_text_bytes = self.semantic_text_bytes.saturating_add(bytes);
    }
}

/// Text measurements that are expensive enough to compute lazily.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct TextStats {
    pub text_length: usize,
    pub word_count: usize,
    pub link_text_length: usize,
    pub link_density: f64,
    pub has_alphanumeric_text: bool,
    pub alphabetic_chars: usize,
    pub digit_chars: usize,
}

pub(super) fn is_visible_inline_character(character: char) -> bool {
    !character.is_whitespace()
        && !matches!(character, '\u{00a0}' | '\u{200b}' | '\u{2060}' | '\u{feff}')
}

pub(crate) fn has_visible_inline_text(text: &str) -> bool {
    text.chars().any(is_visible_inline_character)
}

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
    /// Number of semantic table cells with visible text.
    pub non_empty_table_cell_count: usize,
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
    /// Number of bytes in retained raw code blocks.
    pub raw_code_bytes: usize,
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
) -> (Option<String>, TextStats) {
    walk_text_range(
        document,
        0,
        document.operations().len(),
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

#[cfg(test)]
pub(crate) fn render_node_text(document: &Document, root: DocumentNodeId) -> String {
    walk_text_range(
        document,
        root.index(),
        document.operation_end(root.index()).saturating_add(1),
        false,
        false,
        Some(0),
        false,
    )
    .0
    .unwrap_or_default()
}

fn walk_text_range(
    document: &Document,
    start: usize,
    end: usize,
    block_newlines: bool,
    preserve_line_breaks: bool,
    capacity: Option<usize>,
    collect_stats: bool,
) -> (Option<String>, TextStats) {
    let block = if block_newlines {
        Separator::Newline
    } else {
        Separator::Space
    };
    let mut output = NormalizedOutput::new(capacity, collect_stats);
    let mut index = start;
    while index < end {
        let Some(operation) = document.operations().get(index).copied() else {
            break;
        };
        if operation.is_close() {
            let opening = document.operation_opening_index(operation);
            match document.operation_kind(opening) {
                Some(super::OperationKind::Link) => {
                    if let Some(Item::Link(link)) = document.operation_view(opening) {
                        output.end_link(LinkFragment {
                            hash: link.fragment_only,
                        });
                    }
                }
                Some(kind) if is_block_operation(kind) => output.separator(block),
                _ => {}
            }
            index += 1;
            continue;
        }

        let Some(node) = document.operation_view(index) else {
            index += 1;
            continue;
        };
        match node {
            Item::Text(text) => output.text(text),
            Item::CodeBlock(code) => {
                output.separator(block);
                output.text(code.text());
                output.separator(block);
            }
            Item::InlineCode(code) => output.text(code),
            Item::Image(_) | Item::FootnoteReference(_) | Item::ThematicBreak => {}
            Item::TaskMarker(marker) => {
                if let Some(label) = marker.fallback_label.as_deref() {
                    output.text(label);
                }
            }
            Item::InlineMath(math) => {
                output.text(math.fallback_text.as_deref().unwrap_or(&math.source));
            }
            Item::DisplayMath(math) => {
                output.separator(block);
                output.text(math.fallback_text.as_deref().unwrap_or(&math.source));
                output.separator(block);
            }
            Item::Media(media) => {
                if let Some(title) = media.title.as_deref() {
                    output.text(title);
                }
            }
            Item::HardBreak => output.separator(if preserve_line_breaks {
                Separator::Newline
            } else {
                Separator::Space
            }),
            Item::Link(_) => output.begin_link(),
            kind => {
                if is_block(&kind) {
                    output.separator(block);
                }
            }
        }
        index += 1;
    }
    output.finish()
}

fn is_block(kind: &Item) -> bool {
    matches!(
        kind,
        Item::Paragraph
            | Item::BlockGroup
            | Item::Heading { .. }
            | Item::BlockQuote
            | Item::ListItem
            | Item::TableCaption
            | Item::TableRow
            | Item::TableCell(_)
            | Item::Figure
            | Item::Figcaption
            | Item::Details
            | Item::Summary
            | Item::DefinitionTerm
            | Item::DefinitionDescription
            | Item::Callout(_)
            | Item::FootnoteDefinition(_)
    )
}

fn is_block_operation(kind: super::OperationKind) -> bool {
    matches!(
        kind,
        super::OperationKind::Paragraph
            | super::OperationKind::BlockGroup
            | super::OperationKind::Heading
            | super::OperationKind::BlockQuote
            | super::OperationKind::ListItem
            | super::OperationKind::TableCaption
            | super::OperationKind::TableRow
            | super::OperationKind::TableCell
            | super::OperationKind::Figure
            | super::OperationKind::Figcaption
            | super::OperationKind::Details
            | super::OperationKind::Summary
            | super::OperationKind::DefinitionTerm
            | super::OperationKind::DefinitionDescription
            | super::OperationKind::Callout
            | super::OperationKind::FootnoteDefinition
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
    has_alphanumeric_text: bool,
    alphabetic_chars: usize,
    digit_chars: usize,
}

impl NormalizedOutput {
    fn new(capacity: Option<usize>, collect_stats: bool) -> Self {
        Self {
            output: capacity.map(String::with_capacity),
            collect_stats,
            ..Self::default()
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

    fn finish(self) -> (Option<String>, TextStats) {
        (
            self.output,
            TextStats {
                text_length: self.character_count,
                word_count: self.word_count,
                link_text_length: self.link_text_length,
                link_density: if self.character_count == 0 {
                    0.0
                } else {
                    (self.weighted_link_text_length / self.character_count as f64).clamp(0.0, 1.0)
                },
                has_alphanumeric_text: self.has_alphanumeric_text,
                alphabetic_chars: self.alphabetic_chars,
                digit_chars: self.digit_chars,
            },
        )
    }
}

pub(crate) fn combine(compile: CompileStats, text: TextStats) -> DocumentStats {
    DocumentStats {
        text_length: text.text_length,
        word_count: text.word_count,
        link_text_length: text.link_text_length,
        link_density: text.link_density,
        paragraph_count: compile.paragraph_count,
        heading_count: compile.heading_count,
        list_item_count: compile.list_item_count,
        code_block_count: compile.code_block_count,
        table_count: compile.table_count,
        non_empty_table_cell_count: compile.non_empty_table_cell_count,
        figure_count: compile.figure_count,
        image_count: compile.image_count,
        footnote_reference_count: compile.footnote_reference_count,
        footnote_definition_count: compile.footnote_definition_count,
        math_count: compile.math_count,
        structured_block_count: compile.structured_block_count,
        has_alphanumeric_text: text.has_alphanumeric_text,
        alphabetic_chars: text.alphabetic_chars,
        digit_chars: text.digit_chars,
        has_contextual_structure: compile.has_contextual_structure,
        raw_code_bytes: compile.raw_code_bytes,
    }
}

#[cfg(test)]
mod tests {
    use crate::document::{
        CodeBlock, FootnoteId, Image, Link, List, ListKind, MathFormat, MathValue,
        SemanticKind as Item, SemanticTapeBuilder, Table, TableCell,
    };

    #[test]
    fn counts_semantic_result_metrics() {
        let mut builder = SemanticTapeBuilder::with_capacity(30);
        let heading = builder.emit(None, Item::Heading { level: 2 }).unwrap();
        builder.append_prose(Some(heading), "Heading").unwrap();
        builder.close(heading).unwrap();
        let paragraph = builder.emit(None, Item::Paragraph).unwrap();
        builder.append_prose(Some(paragraph), "Hello ").unwrap();
        let link = builder
            .emit(
                Some(paragraph),
                Item::Link(Link {
                    destination: "https://example.test/guide".into(),
                    title: None,
                    fragment_only: false,
                }),
            )
            .unwrap();
        builder.append_prose(Some(link), "world").unwrap();
        builder.close(link).unwrap();
        builder.close(paragraph).unwrap();
        let list = builder
            .emit(
                None,
                Item::List(List {
                    kind: ListKind::Unordered,
                    start: None,
                }),
            )
            .unwrap();
        let item = builder.emit(Some(list), Item::ListItem).unwrap();
        builder.append_prose(Some(item), "item").unwrap();
        builder.close(item).unwrap();
        let empty_item = builder.emit(Some(list), Item::ListItem).unwrap();
        builder.close(empty_item).unwrap();
        builder.close(list).unwrap();
        builder
            .emit(
                None,
                Item::CodeBlock(CodeBlock {
                    language: None,
                    text: "let x = 1;".into(),
                }),
            )
            .unwrap();
        let table = builder
            .emit(
                None,
                Item::Table(Table {
                    column_count: Some(1),
                }),
            )
            .unwrap();
        let row = builder.emit(Some(table), Item::TableRow).unwrap();
        let cell = builder
            .emit(
                Some(row),
                Item::TableCell(TableCell {
                    header: true,
                    colspan: 1,
                    rowspan: 1,
                    alignment: None,
                }),
            )
            .unwrap();
        builder.append_prose(Some(cell), "Name").unwrap();
        builder.close(cell).unwrap();
        builder.close(row).unwrap();
        builder.close(table).unwrap();
        let figure = builder.emit(None, Item::Figure).unwrap();
        builder
            .emit(
                Some(figure),
                Item::Image(Image {
                    source: "image.png".into(),
                    alt: "An image".into(),
                    title: None,
                    width: None,
                    height: None,
                }),
            )
            .unwrap();
        builder.close(figure).unwrap();
        let footnote = FootnoteId::from_index(0).unwrap();
        builder
            .emit(None, Item::FootnoteReference(footnote))
            .unwrap();
        let definition = builder
            .emit(None, Item::FootnoteDefinition(footnote))
            .unwrap();
        builder.append_prose(Some(definition), "Note").unwrap();
        builder.close(definition).unwrap();
        builder.define_footnote(footnote, "1", definition).unwrap();
        builder
            .emit(
                None,
                Item::InlineMath(MathValue {
                    source: "x".into(),
                    format: MathFormat::Tex,
                    fallback_text: Some("x".into()),
                }),
            )
            .unwrap();
        builder
            .emit(
                None,
                Item::DisplayMath(MathValue {
                    source: "y".into(),
                    format: MathFormat::Tex,
                    fallback_text: Some("y".into()),
                }),
            )
            .unwrap();

        let document = builder.finish().unwrap();
        let stats = document.stats();
        assert_eq!(stats.text_length, document.text().chars().count());
        assert_eq!(stats.word_count, 12);
        assert_eq!(stats.link_text_length, 5);
        assert_eq!(stats.paragraph_count, 1);
        assert_eq!(stats.heading_count, 1);
        assert_eq!(stats.list_item_count, 2);
        assert_eq!(stats.code_block_count, 1);
        assert_eq!(stats.table_count, 1);
        assert_eq!(stats.non_empty_table_cell_count, 1);
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
        let mut builder = SemanticTapeBuilder::with_capacity(2);
        let link = builder
            .emit(
                None,
                Item::Link(Link {
                    destination: "#section".into(),
                    title: None,
                    fragment_only: true,
                }),
            )
            .unwrap();
        builder.append_prose(Some(link), "  linked text  ").unwrap();
        builder.close(link).unwrap();
        let document = builder.finish().unwrap();
        let stats = document.stats();
        assert_eq!(stats.link_text_length, 11);
        assert_eq!(stats.link_density, 3.3 / 11.0);
    }

    #[test]
    fn ascii_and_unicode_text_paths_preserve_normalization_and_metrics() {
        let mut builder = SemanticTapeBuilder::with_capacity(4);
        let paragraph = builder.emit(None, Item::Paragraph).unwrap();
        builder
            .append_prose(Some(paragraph), "  ASCII\twords 42!  ")
            .unwrap();
        builder.close(paragraph).unwrap();
        let link = builder
            .emit(
                None,
                Item::Link(Link {
                    destination: "https://example.test".into(),
                    title: None,
                    fragment_only: false,
                }),
            )
            .unwrap();
        builder.append_prose(Some(link), "  世界  café  ").unwrap();
        builder.close(link).unwrap();
        let document = builder.finish().unwrap();

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
        let document = builder.finish().unwrap();
        assert_eq!(document.stats().text_length, 4);
    }
}
