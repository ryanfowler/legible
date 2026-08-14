use super::{Document, DocumentNodeId, NodeKind};
use smallvec::SmallVec;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct DocumentStats {
    pub(crate) text_length: usize,
    pub(crate) word_count: usize,
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
) -> (Option<String>, DocumentStats) {
    let roots: SmallVec<[_; 16]> = document.root_ids().collect();
    walk_text_from_roots(
        document,
        &roots,
        block_newlines,
        preserve_line_breaks,
        capacity,
    )
}

pub(crate) fn render_document_text(document: &Document) -> String {
    walk_text(document, false, false, Some(0))
        .0
        .unwrap_or_default()
}

pub(crate) fn measure_document(document: &Document) -> DocumentStats {
    walk_text(document, false, false, None).1
}

pub(crate) fn render_node_text(document: &Document, root: DocumentNodeId) -> String {
    walk_text_from_roots(document, &[root], false, false, Some(0))
        .0
        .unwrap_or_default()
}

fn walk_text_from_roots(
    document: &Document,
    roots: &[DocumentNodeId],
    block_newlines: bool,
    preserve_line_breaks: bool,
    capacity: Option<usize>,
) -> (Option<String>, DocumentStats) {
    enum Task {
        Node(DocumentNodeId),
        Boundary(Separator),
    }

    let block = if block_newlines {
        Separator::Newline
    } else {
        Separator::Space
    };
    let mut output = NormalizedOutput::new(capacity);
    let mut tasks = SmallVec::<[Task; 32]>::new();
    tasks.extend(roots.iter().rev().copied().map(Task::Node));
    while let Some(task) = tasks.pop() {
        match task {
            Task::Boundary(separator) => output.separator(separator),
            Task::Node(id) => {
                let Some(node) = document.node(id) else {
                    continue;
                };
                match node.kind() {
                    NodeKind::Text(text) => output.text(text),
                    NodeKind::CodeBlock(code) => {
                        output.separator(block);
                        output.text(&code.text);
                        output.separator(block);
                    }
                    NodeKind::InlineCode(code) => output.text(code),
                    NodeKind::Image(_)
                    | NodeKind::FootnoteReference(_)
                    | NodeKind::ThematicBreak => {}
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
                    kind => {
                        if is_block(kind) {
                            output.separator(block);
                            tasks.push(Task::Boundary(block));
                        }
                        let children: SmallVec<[_; 8]> = document.child_ids(id).collect();
                        tasks.extend(children.into_iter().rev().map(Task::Node));
                    }
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

#[derive(Default)]
struct NormalizedOutput {
    output: Option<String>,
    pending: Separator,
    character_count: usize,
    word_count: usize,
    in_word: bool,
}

impl NormalizedOutput {
    fn new(capacity: Option<usize>) -> Self {
        Self {
            output: capacity.map(String::with_capacity),
            ..Self::default()
        }
    }

    fn text(&mut self, text: &str) {
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

    fn finish(self) -> (Option<String>, DocumentStats) {
        (
            self.output,
            DocumentStats {
                text_length: self.character_count,
                word_count: self.word_count,
            },
        )
    }
}
