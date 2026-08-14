//! Semantic document intermediate representation.
//!
//! This module is crate-private while the representation is under development.
//! The arena keeps traversal and destruction stack-safe for deeply nested input.

#![allow(dead_code)]

mod builder;
mod compiler;
pub(crate) mod stats;
mod text;
mod uri;
mod validate;

pub(crate) use builder::{BuildError, DocumentBuilder};
pub(crate) use compiler::{CompileContext, compile_document};
pub(crate) use stats::DocumentStats;
pub(crate) use uri::{DestinationKind, safe_destination};

use std::fmt;

/// An index into a [`Document`] arena.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct DocumentNodeId(u32);

impl DocumentNodeId {
    fn index(self) -> usize {
        self.0 as usize
    }
}

/// A semantic document stored as a compact node arena.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Document {
    nodes: Vec<DocumentNode>,
    roots: Vec<DocumentNodeId>,
    footnotes: Vec<FootnoteDefinition>,
}

impl Document {
    pub(crate) fn roots(
        &self,
    ) -> impl ExactSizeIterator<Item = DocumentNodeId> + DoubleEndedIterator + '_ {
        self.roots.iter().copied()
    }

    pub(crate) fn node(&self, id: DocumentNodeId) -> Option<&DocumentNode> {
        self.nodes.get(id.index())
    }

    pub(crate) fn children(&self, parent: DocumentNodeId) -> Children<'_> {
        Children {
            document: self,
            next: self.first_child(parent),
        }
    }

    pub(crate) fn first_child(&self, parent: DocumentNodeId) -> Option<DocumentNodeId> {
        self.node(parent).and_then(|node| node.first_child)
    }

    pub(crate) fn next_sibling(&self, node: DocumentNodeId) -> Option<DocumentNodeId> {
        self.node(node).and_then(|node| node.next_sibling)
    }

    pub(crate) fn footnote(&self, id: FootnoteId) -> Option<&FootnoteDefinition> {
        self.footnotes
            .get(id.index())
            .filter(|definition| definition.id == id)
    }

    pub(crate) fn footnotes(&self) -> &[FootnoteDefinition] {
        &self.footnotes
    }

    pub(crate) fn len(&self) -> usize {
        self.nodes.len()
    }

    pub(crate) fn validate(&self) -> Result<(), ValidationError> {
        validate::validate(self)
    }

    #[cfg(test)]
    pub(crate) fn debug_tree(&self) -> String {
        debug_tree(self)
    }
}

pub(crate) struct Children<'a> {
    document: &'a Document,
    next: Option<DocumentNodeId>,
}

impl Iterator for Children<'_> {
    type Item = DocumentNodeId;

    fn next(&mut self) -> Option<Self::Item> {
        let id = self.next?;
        self.next = self.document.node(id).and_then(|node| node.next_sibling);
        Some(id)
    }
}

/// One semantic node and its arena links.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DocumentNode {
    kind: NodeKind,
    first_child: Option<DocumentNodeId>,
    next_sibling: Option<DocumentNodeId>,
}

impl DocumentNode {
    pub(crate) fn kind(&self) -> &NodeKind {
        &self.kind
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum NodeKind {
    Paragraph,
    /// A semantic block boundary with no more specific meaning.
    BlockGroup,
    Heading {
        level: u8,
    },
    BlockQuote,
    CodeBlock(CodeBlock),
    List(List),
    ListItem,
    Table(Table),
    TableCaption,
    TableRow,
    TableCell(TableCell),
    Figure,
    Figcaption,
    Details,
    Summary,
    ThematicBreak,
    DefinitionList,
    DefinitionTerm,
    DefinitionDescription,
    Callout(Callout),
    FootnoteDefinition(FootnoteId),
    Text(String),
    Emphasis,
    Strong,
    Strikethrough,
    InlineCode(Box<str>),
    Link(Link),
    Image(Image),
    HardBreak,
    FootnoteReference(FootnoteId),
    TaskMarker(TaskMarker),
    InlineMath(MathValue),
    DisplayMath(MathValue),
    Media(Media),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CodeBlock {
    pub(crate) language: Option<Box<str>>,
    pub(crate) text: Box<str>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Link {
    pub(crate) destination: Box<str>,
    pub(crate) title: Option<Box<str>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Image {
    pub(crate) source: Box<str>,
    pub(crate) alt: Box<str>,
    pub(crate) title: Option<Box<str>>,
    pub(crate) width: Option<u32>,
    pub(crate) height: Option<u32>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct List {
    pub(crate) kind: ListKind,
    pub(crate) start: Option<i64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ListKind {
    Ordered,
    Unordered,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Table {
    /// Exact width when no row spans make grid placement output-specific.
    pub(crate) column_count: Option<u32>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TableCell {
    pub(crate) header: bool,
    pub(crate) colspan: u32,
    pub(crate) rowspan: u32,
    pub(crate) alignment: Option<TableAlignment>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TableAlignment {
    Left,
    Center,
    Right,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Callout {
    pub(crate) kind: CalloutKind,
    pub(crate) title: Option<Box<str>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CalloutKind {
    Note,
    Warning,
    Tip,
    Important,
    Caution,
    Info,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TaskMarker {
    pub(crate) checked: bool,
    pub(crate) fallback_label: Option<Box<str>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct MathValue {
    pub(crate) source: Box<str>,
    pub(crate) format: MathFormat,
    pub(crate) fallback_text: Option<Box<str>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MathFormat {
    Tex,
    Text,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Media {
    pub(crate) kind: MediaKind,
    pub(crate) source: Box<str>,
    pub(crate) title: Option<Box<str>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MediaKind {
    Audio,
    Video,
    Embedded,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct FootnoteId(u32);

impl FootnoteId {
    fn index(self) -> usize {
        self.0 as usize
    }

    pub(crate) fn from_index(index: usize) -> Result<Self, BuildError> {
        u32::try_from(index)
            .map(Self)
            .map_err(|_| BuildError::CapacityExceeded)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct FootnoteDefinition {
    pub(crate) id: FootnoteId,
    pub(crate) label: Box<str>,
    pub(crate) node: DocumentNodeId,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ValidationError(Box<str>);

impl ValidationError {
    fn new(message: impl Into<Box<str>>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for ValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for ValidationError {}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::*;

    #[test]
    fn manual_documents_cover_initial_semantic_vocabulary() {
        let mut builder = DocumentBuilder::with_capacity(20);
        let paragraph = builder.append(None, NodeKind::Paragraph).unwrap();
        builder.append_prose(Some(paragraph), "nested ").unwrap();
        let emphasis = builder.append(Some(paragraph), NodeKind::Emphasis).unwrap();
        builder.append_prose(Some(emphasis), "formatting").unwrap();

        let list = builder
            .append(
                None,
                NodeKind::List(List {
                    kind: ListKind::Unordered,
                    start: None,
                }),
            )
            .unwrap();
        builder.append(Some(list), NodeKind::ListItem).unwrap();
        builder
            .append(
                None,
                NodeKind::CodeBlock(CodeBlock {
                    language: Some("rust".into()),
                    text: "fn main() {}\n".into(),
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
        builder.append(Some(figure), NodeKind::Figcaption).unwrap();
        let footnote = FootnoteId::from_index(0).unwrap();
        builder
            .append(None, NodeKind::FootnoteReference(footnote))
            .unwrap();
        let definition = builder
            .append(None, NodeKind::FootnoteDefinition(footnote))
            .unwrap();
        builder
            .define_footnote(footnote, "note", definition)
            .unwrap();

        let document = builder.finish();
        document.validate().unwrap();
        assert_eq!(document.roots().count(), 7);
    }

    #[test]
    fn deeply_nested_semantic_documents_are_stack_safe() {
        const DEPTH: usize = 10_000;
        let mut builder = DocumentBuilder::with_capacity(DEPTH + 1);
        let mut parent = None;
        for _ in 0..DEPTH {
            parent = Some(builder.append(parent, NodeKind::BlockQuote).unwrap());
        }
        builder.append_prose(parent, "deep").unwrap();
        let document = builder.finish();
        document.validate().unwrap();

        let mut count = 0;
        let mut stack: Vec<_> = document.roots().collect();
        while let Some(node) = stack.pop() {
            count += 1;
            stack.extend(document.children(node));
        }
        assert_eq!(count, DEPTH + 1);
    }

    #[test]
    fn prose_stays_under_its_requested_parent() {
        let mut builder = DocumentBuilder::with_capacity(3);
        builder.append_prose(None, "root").unwrap();
        let paragraph = builder.append(None, NodeKind::Paragraph).unwrap();
        builder.append_prose(Some(paragraph), "child").unwrap();
        assert_eq!(
            builder.finish().debug_tree(),
            "Text(\"root\")\nParagraph\n  Text(\"child\")\n"
        );

        let mut builder = DocumentBuilder::with_capacity(1);
        assert_eq!(
            builder.append_prose(Some(DocumentNodeId(u32::MAX)), "bad"),
            Err(BuildError::InvalidParent)
        );
    }

    #[test]
    fn validation_rejects_cycles_and_invalid_cells() {
        let mut builder = DocumentBuilder::with_capacity(2);
        let first = builder.append(None, NodeKind::Paragraph).unwrap();
        let second = builder.append(Some(first), NodeKind::Strong).unwrap();
        let mut cycle = builder.finish();
        cycle.nodes[second.index()].next_sibling = Some(second);
        assert!(cycle.validate().is_err());

        let mut builder = DocumentBuilder::with_capacity(1);
        builder
            .append(
                None,
                NodeKind::TableCell(TableCell {
                    header: false,
                    colspan: 0,
                    rowspan: 1,
                    alignment: None,
                }),
            )
            .unwrap();
        assert!(builder.finish().validate().is_err());
    }
}

#[cfg(test)]
fn debug_tree(document: &Document) -> String {
    enum Task {
        Node(DocumentNodeId, usize),
    }

    let mut output = String::new();
    let mut tasks = Vec::new();
    tasks.extend(document.roots.iter().rev().map(|&id| Task::Node(id, 0)));
    while let Some(Task::Node(id, depth)) = tasks.pop() {
        let Some(node) = document.node(id) else {
            continue;
        };
        output.push_str(&"  ".repeat(depth));
        write_kind(&mut output, &node.kind);
        output.push('\n');
        let children: Vec<_> = document.children(id).collect();
        tasks.extend(
            children
                .into_iter()
                .rev()
                .map(|child| Task::Node(child, depth + 1)),
        );
    }
    output
}

#[cfg(test)]
fn write_kind(output: &mut String, kind: &NodeKind) {
    use std::fmt::Write as _;
    match kind {
        NodeKind::Text(value) => write!(output, "Text({value:?})").unwrap(),
        NodeKind::Heading { level } => write!(output, "Heading(level={level})").unwrap(),
        NodeKind::CodeBlock(code) => write!(
            output,
            "CodeBlock(language={:?}, text={:?})",
            code.language, code.text
        )
        .unwrap(),
        NodeKind::List(list) => {
            write!(output, "List(kind={:?}, start={:?})", list.kind, list.start).unwrap()
        }
        NodeKind::Table(table) => {
            write!(output, "Table(columns={:?})", table.column_count).unwrap()
        }
        NodeKind::TableCell(cell) => write!(
            output,
            "TableCell(header={}, colspan={}, rowspan={}, alignment={:?})",
            cell.header, cell.colspan, cell.rowspan, cell.alignment
        )
        .unwrap(),
        NodeKind::Link(link) => write!(
            output,
            "Link(destination={:?}, title={:?})",
            link.destination, link.title
        )
        .unwrap(),
        NodeKind::Image(image) => write!(
            output,
            "Image(source={:?}, alt={:?}, title={:?}, width={:?}, height={:?})",
            image.source, image.alt, image.title, image.width, image.height
        )
        .unwrap(),
        NodeKind::InlineCode(value) => write!(output, "InlineCode({value:?})").unwrap(),
        NodeKind::FootnoteReference(id) => write!(output, "FootnoteReference({})", id.0).unwrap(),
        NodeKind::FootnoteDefinition(id) => write!(output, "FootnoteDefinition({})", id.0).unwrap(),
        NodeKind::TaskMarker(marker) => write!(
            output,
            "TaskMarker(checked={}, fallback={:?})",
            marker.checked, marker.fallback_label
        )
        .unwrap(),
        NodeKind::InlineMath(value) | NodeKind::DisplayMath(value) => write!(
            output,
            "{}(source={:?}, format={:?}, fallback={:?})",
            if matches!(kind, NodeKind::InlineMath(_)) {
                "InlineMath"
            } else {
                "DisplayMath"
            },
            value.source,
            value.format,
            value.fallback_text
        )
        .unwrap(),
        NodeKind::Callout(callout) => write!(
            output,
            "Callout(kind={:?}, title={:?})",
            callout.kind, callout.title
        )
        .unwrap(),
        NodeKind::Media(media) => write!(
            output,
            "Media(kind={:?}, source={:?}, title={:?})",
            media.kind, media.source, media.title
        )
        .unwrap(),
        other => write!(output, "{other:?}").unwrap(),
    }
}
