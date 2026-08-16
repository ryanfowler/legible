//! Benchmark-only prototypes for the private semantic representation.
//!
//! This module deliberately does not participate in production extraction. It
//! adapts the production semantic tape through its compatibility view and
//! compares that view with benchmark-only sequential layouts.

use criterion::{BenchmarkId, Criterion, Throughput};
use std::fmt::Write as _;
use std::hint::black_box;
use std::mem::size_of;

use crate::document::{self, Document, NodeKindView as NodeKind};
use crate::dom::{self, Tag};

const NO_PAYLOAD: u32 = u32::MAX;
const OPEN_BIT: u8 = 0x80;
const CLOSE_BIT: u8 = 0x40;
const KIND_MASK: u8 = 0x3f;

#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Kind {
    Paragraph,
    BlockGroup,
    Heading,
    BlockQuote,
    CodeBlock,
    List,
    ListItem,
    Table,
    TableCaption,
    TableRow,
    TableCell,
    Figure,
    Figcaption,
    Details,
    Summary,
    ThematicBreak,
    DefinitionList,
    DefinitionTerm,
    DefinitionDescription,
    Callout,
    FootnoteDefinition,
    Text,
    Emphasis,
    Strong,
    Strikethrough,
    InlineCode,
    Link,
    Image,
    HardBreak,
    FootnoteReference,
    TaskMarker,
    InlineMath,
    DisplayMath,
    Media,
}

impl Kind {
    fn from_u8(value: u8) -> Self {
        // The prototypes are built internally. Keep invalid operation streams
        // impossible to construct rather than adding an Option to every view.
        debug_assert!(value <= Kind::Media as u8);
        // SAFETY: The builder only writes discriminants from this enum. This
        // match keeps the conversion safe and makes malformed benchmark data
        // fail closed in non-debug builds.
        match value {
            0 => Self::Paragraph,
            1 => Self::BlockGroup,
            2 => Self::Heading,
            3 => Self::BlockQuote,
            4 => Self::CodeBlock,
            5 => Self::List,
            6 => Self::ListItem,
            7 => Self::Table,
            8 => Self::TableCaption,
            9 => Self::TableRow,
            10 => Self::TableCell,
            11 => Self::Figure,
            12 => Self::Figcaption,
            13 => Self::Details,
            14 => Self::Summary,
            15 => Self::ThematicBreak,
            16 => Self::DefinitionList,
            17 => Self::DefinitionTerm,
            18 => Self::DefinitionDescription,
            19 => Self::Callout,
            20 => Self::FootnoteDefinition,
            21 => Self::Text,
            22 => Self::Emphasis,
            23 => Self::Strong,
            24 => Self::Strikethrough,
            25 => Self::InlineCode,
            26 => Self::Link,
            27 => Self::Image,
            28 => Self::HardBreak,
            29 => Self::FootnoteReference,
            30 => Self::TaskMarker,
            31 => Self::InlineMath,
            32 => Self::DisplayMath,
            33 => Self::Media,
            _ => Self::BlockGroup,
        }
    }

    fn is_container(self) -> bool {
        !matches!(
            self,
            Self::CodeBlock
                | Self::Text
                | Self::InlineCode
                | Self::Image
                | Self::HardBreak
                | Self::ThematicBreak
                | Self::FootnoteReference
                | Self::TaskMarker
                | Self::InlineMath
                | Self::DisplayMath
                | Self::Media
        )
    }
}

#[derive(Clone)]
enum Payload {
    Text(Box<str>),
    CodeBlock {
        language: Option<Box<str>>,
        text: Box<str>,
    },
    Link {
        destination: Box<str>,
        title: Option<Box<str>>,
        fragment_only: bool,
    },
    Image {
        source: Box<str>,
        alt: Box<str>,
        title: Option<Box<str>>,
        width: Option<u32>,
        height: Option<u32>,
    },
    List {
        ordered: bool,
        start: Option<i64>,
    },
    Table {
        column_count: Option<u32>,
    },
    TableCell {
        header: bool,
        colspan: u32,
        rowspan: u32,
        alignment: u8,
    },
    Callout {
        kind: u8,
        title: Option<Box<str>>,
    },
    TaskMarker {
        checked: bool,
        fallback_label: Option<Box<str>>,
    },
    Math {
        source: Box<str>,
        format: u8,
        fallback_text: Option<Box<str>>,
    },
    Media {
        kind: u8,
        source: Box<str>,
        title: Option<Box<str>>,
    },
    Footnote {
        id: u32,
    },
}

impl Payload {
    fn string_bytes(&self) -> usize {
        fn optional_len(value: &Option<Box<str>>) -> usize {
            value.as_deref().map_or(0, str::len)
        }

        match self {
            Self::Text(value) => value.len(),
            Self::CodeBlock { language, text } => optional_len(language) + text.len(),
            Self::Link {
                destination, title, ..
            } => destination.len() + optional_len(title),
            Self::Image {
                source, alt, title, ..
            } => source.len() + alt.len() + optional_len(title),
            Self::List { .. } | Self::Table { .. } | Self::TableCell { .. } => 0,
            Self::Callout { title, .. } => optional_len(title),
            Self::TaskMarker { fallback_label, .. } => optional_len(fallback_label),
            Self::Math {
                source,
                fallback_text,
                ..
            } => source.len() + optional_len(fallback_text),
            Self::Media { source, title, .. } => source.len() + optional_len(title),
            Self::Footnote { .. } => 0,
        }
    }

    fn string_values(&self) -> usize {
        fn optional_count(value: &Option<Box<str>>) -> usize {
            usize::from(value.is_some())
        }

        match self {
            Self::Text(_) => 1,
            Self::CodeBlock { language, .. } => 1 + optional_count(language),
            Self::Link { title, .. } => 1 + optional_count(title),
            Self::Image { title, .. } => 2 + optional_count(title),
            Self::List { .. } | Self::Table { .. } | Self::TableCell { .. } => 0,
            Self::Callout { title, .. } => optional_count(title),
            Self::TaskMarker { fallback_label, .. } => optional_count(fallback_label),
            Self::Math { fallback_text, .. } => 1 + optional_count(fallback_text),
            Self::Media { title, .. } => 1 + optional_count(title),
            Self::Footnote { .. } => 0,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Value<'a> {
    None,
    Text(&'a str),
    CodeBlock {
        language: Option<&'a str>,
        text: &'a str,
    },
    Link {
        destination: &'a str,
        title: Option<&'a str>,
        fragment_only: bool,
    },
    Image {
        source: &'a str,
        alt: &'a str,
        title: Option<&'a str>,
        width: Option<u32>,
        height: Option<u32>,
    },
    List {
        ordered: bool,
        start: Option<i64>,
    },
    Table {
        column_count: Option<u32>,
    },
    TableCell {
        header: bool,
        colspan: u32,
        rowspan: u32,
        alignment: u8,
    },
    Callout {
        kind: u8,
        title: Option<&'a str>,
    },
    TaskMarker {
        checked: bool,
        fallback_label: Option<&'a str>,
    },
    Math {
        source: &'a str,
        format: u8,
        fallback_text: Option<&'a str>,
    },
    Media {
        kind: u8,
        source: &'a str,
        title: Option<&'a str>,
    },
    Footnote {
        label: &'a str,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct View<'a> {
    kind: Kind,
    aux: u16,
    value: Value<'a>,
}

enum ArenaTask {
    Enter(document::DocumentNodeId),
    Siblings(document::DocumentNodeId),
    Exit(document::DocumentNodeId),
}

type PreorderClose = (u32, Kind, u32, u16);

#[repr(C)]
#[derive(Clone, Copy)]
struct PreorderNode {
    subtree_end: u32,
    payload: u32,
    aux: u16,
    kind: u8,
    flags: u8,
}

struct PreorderDocument {
    nodes: Vec<PreorderNode>,
    payloads: Vec<Payload>,
    footnotes: Vec<Option<Box<str>>>,
    output_capacity_hint: usize,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Op {
    payload: u32,
    aux: u16,
    opcode: u8,
    flags: u8,
}

struct EventDocument {
    ops: Vec<Op>,
    payloads: Vec<Payload>,
    footnotes: Vec<Option<Box<str>>>,
    output_capacity_hint: usize,
}

struct CapturedNode {
    source_id: document::DocumentNodeId,
    kind: Kind,
    aux: u16,
    payload: Option<Payload>,
    has_payload: bool,
    subtree_end: usize,
}

impl PreorderDocument {
    fn from_document(document: &Document) -> Self {
        let captured = capture_document(document);
        let payload_count = captured
            .iter()
            .filter(|node| node.payload.is_some())
            .count();
        let footnotes = collect_footnotes(document, &captured);
        let mut nodes = Vec::with_capacity(captured.len());
        let mut payloads = Vec::with_capacity(payload_count);

        for node in captured {
            let payload = node.payload.map_or(NO_PAYLOAD, |value| {
                let index = payloads.len();
                payloads.push(value);
                index as u32
            });
            nodes.push(PreorderNode {
                subtree_end: node.subtree_end as u32,
                payload,
                aux: node.aux,
                kind: node.kind as u8,
                flags: u8::from(node.kind.is_container()),
            });
        }

        Self {
            nodes,
            payloads,
            footnotes,
            output_capacity_hint: document.output_capacity_hint(),
        }
    }

    fn retained_bytes(&self) -> usize {
        size_of::<Self>()
            + self.nodes.capacity() * size_of::<PreorderNode>()
            + self.payloads.capacity() * size_of::<Payload>()
            + self.footnotes.capacity() * size_of::<Option<Box<str>>>()
            + self
                .payloads
                .iter()
                .map(Payload::string_bytes)
                .sum::<usize>()
            + self
                .footnotes
                .iter()
                .flatten()
                .map(|label| label.len())
                .sum::<usize>()
    }

    fn string_bytes(&self) -> usize {
        self.payloads
            .iter()
            .map(Payload::string_bytes)
            .sum::<usize>()
            + self
                .footnotes
                .iter()
                .flatten()
                .map(|label| label.len())
                .sum::<usize>()
    }

    fn string_values(&self) -> usize {
        self.payloads
            .iter()
            .map(Payload::string_values)
            .sum::<usize>()
            + self.footnotes.iter().flatten().count()
    }
}

impl EventDocument {
    fn from_document(document: &Document) -> Self {
        let mut captured = capture_document(document);
        let footnotes = collect_footnotes(document, &captured);
        let payload_count = captured.iter().filter(|node| node.has_payload).count();
        let container_count = captured
            .iter()
            .filter(|node| node.kind.is_container())
            .count();
        let mut payloads = Vec::with_capacity(payload_count);
        for node in &mut captured {
            if let Some(value) = node.payload.take() {
                payloads.push(value);
            }
        }

        let mut ops = Vec::with_capacity(captured.len() + container_count);
        append_event_ops(&captured, &mut ops);

        Self {
            ops,
            payloads,
            footnotes,
            output_capacity_hint: document.output_capacity_hint(),
        }
    }

    fn retained_bytes(&self) -> usize {
        size_of::<Self>()
            + self.ops.capacity() * size_of::<Op>()
            + self.payloads.capacity() * size_of::<Payload>()
            + self.footnotes.capacity() * size_of::<Option<Box<str>>>()
            + self
                .payloads
                .iter()
                .map(Payload::string_bytes)
                .sum::<usize>()
            + self
                .footnotes
                .iter()
                .flatten()
                .map(|label| label.len())
                .sum::<usize>()
    }

    fn string_bytes(&self) -> usize {
        self.payloads
            .iter()
            .map(Payload::string_bytes)
            .sum::<usize>()
            + self
                .footnotes
                .iter()
                .flatten()
                .map(|label| label.len())
                .sum::<usize>()
    }

    fn string_values(&self) -> usize {
        self.payloads
            .iter()
            .map(Payload::string_values)
            .sum::<usize>()
            + self.footnotes.iter().flatten().count()
    }
}

fn collect_footnotes(document: &Document, nodes: &[CapturedNode]) -> Vec<Option<Box<str>>> {
    let highest_id = nodes
        .iter()
        .filter_map(|node| match node.payload.as_ref() {
            Some(Payload::Footnote { id }) => Some(*id as usize),
            _ => None,
        })
        .max();
    let Some(highest_id) = highest_id else {
        return Vec::new();
    };

    let mut labels = vec![None; highest_id + 1];
    for node in nodes {
        let Some(Payload::Footnote { id }) = node.payload.as_ref() else {
            continue;
        };
        let Ok(footnote_id) = document::FootnoteId::from_index(*id as usize) else {
            continue;
        };
        if labels[*id as usize].is_none() {
            labels[*id as usize] = document.footnote_label(footnote_id).map(Into::into);
        }
    }
    labels
}

fn append_event_ops(nodes: &[CapturedNode], output: &mut Vec<Op>) {
    let mut open: Vec<(usize, u32, u16)> = Vec::with_capacity(32);
    let mut payload_index = 0u32;
    for (index, node) in nodes.iter().enumerate() {
        while open
            .last()
            .is_some_and(|parent| nodes[parent.0].subtree_end == index)
        {
            let (parent, payload, aux) = open.pop().unwrap();
            output.push(Op {
                payload,
                aux,
                opcode: nodes[parent].kind as u8 | CLOSE_BIT,
                flags: 0,
            });
        }

        let payload = if node.has_payload {
            let index = payload_index;
            payload_index += 1;
            index
        } else {
            NO_PAYLOAD
        };
        let kind = node.kind as u8;
        if node.kind.is_container() {
            output.push(Op {
                payload,
                aux: node.aux,
                opcode: kind | OPEN_BIT,
                flags: 0,
            });
            open.push((index, payload, node.aux));
        } else {
            output.push(Op {
                payload,
                aux: node.aux,
                opcode: kind,
                flags: 0,
            });
        }
    }
    while let Some((parent, payload, aux)) = open.pop() {
        output.push(Op {
            payload,
            aux,
            opcode: nodes[parent].kind as u8 | CLOSE_BIT,
            flags: 0,
        });
    }
}

fn capture_document(document: &Document) -> Vec<CapturedNode> {
    enum Task {
        Enter(document::DocumentNodeId),
        Siblings(document::DocumentNodeId),
        Exit(usize),
    }

    let mut captured = Vec::with_capacity(document.len());
    let mut tasks = Vec::with_capacity(32);
    tasks.extend(document.root_ids().rev().map(Task::Enter));

    while let Some(task) = tasks.pop() {
        match task {
            Task::Enter(id) => {
                let Some(node) = document.node(id) else {
                    continue;
                };
                let (kind, aux, payload) = capture_node(&node.kind());
                let index = captured.len();
                captured.push(CapturedNode {
                    source_id: id,
                    kind,
                    aux,
                    has_payload: payload.is_some(),
                    payload,
                    subtree_end: 0,
                });
                tasks.push(Task::Exit(index));
                if let Some(child) = document.first_child(id) {
                    tasks.push(Task::Siblings(child));
                }
            }
            Task::Siblings(id) => {
                if let Some(sibling) = document.next_sibling(id) {
                    tasks.push(Task::Siblings(sibling));
                }
                tasks.push(Task::Enter(id));
            }
            Task::Exit(index) => captured[index].subtree_end = captured.len(),
        }
    }
    captured
}

fn assert_captured_fields(document: &Document) {
    let captured = capture_document(document);
    let footnotes = collect_footnotes(document, &captured);
    for node in captured {
        let expected = source_view(document, node.source_id).unwrap();
        let actual = View {
            kind: node.kind,
            aux: node.aux,
            value: node.payload.as_ref().map_or(Value::None, |payload| {
                value_from_payload(payload, &footnotes)
            }),
        };
        assert_eq!(
            expected, actual,
            "semantic fields differ for a captured node"
        );
    }
}

fn capture_node(kind: &NodeKind) -> (Kind, u16, Option<Payload>) {
    let no_payload = || (Kind::BlockGroup, 0, None);
    match kind {
        NodeKind::Paragraph => (Kind::Paragraph, 0, None),
        NodeKind::BlockGroup => no_payload(),
        NodeKind::Heading { .. } => (
            Kind::Heading,
            kind.heading_level().unwrap_or(1) as u16,
            None,
        ),
        NodeKind::BlockQuote => (Kind::BlockQuote, 0, None),
        NodeKind::CodeBlock(code) => (
            Kind::CodeBlock,
            0,
            Some(Payload::CodeBlock {
                language: code.language.clone(),
                text: code.text.clone(),
            }),
        ),
        NodeKind::List(list) => (
            Kind::List,
            0,
            Some(Payload::List {
                ordered: matches!(list.kind, document::ListKind::Ordered),
                start: list.start,
            }),
        ),
        NodeKind::ListItem => (Kind::ListItem, 0, None),
        NodeKind::Table(table) => (
            Kind::Table,
            0,
            Some(Payload::Table {
                column_count: table.column_count,
            }),
        ),
        NodeKind::TableCaption => (Kind::TableCaption, 0, None),
        NodeKind::TableRow => (Kind::TableRow, 0, None),
        NodeKind::TableCell(cell) => (
            Kind::TableCell,
            0,
            Some(Payload::TableCell {
                header: cell.header,
                colspan: cell.colspan,
                rowspan: cell.rowspan,
                alignment: alignment_code(cell.alignment),
            }),
        ),
        NodeKind::Figure => (Kind::Figure, 0, None),
        NodeKind::Figcaption => (Kind::Figcaption, 0, None),
        NodeKind::Details => (Kind::Details, 0, None),
        NodeKind::Summary => (Kind::Summary, 0, None),
        NodeKind::ThematicBreak => (Kind::ThematicBreak, 0, None),
        NodeKind::DefinitionList => (Kind::DefinitionList, 0, None),
        NodeKind::DefinitionTerm => (Kind::DefinitionTerm, 0, None),
        NodeKind::DefinitionDescription => (Kind::DefinitionDescription, 0, None),
        NodeKind::Callout(callout) => (
            Kind::Callout,
            0,
            Some(Payload::Callout {
                kind: callout_kind_code(callout.kind),
                title: callout.title.clone(),
            }),
        ),
        NodeKind::FootnoteDefinition(id) => (
            Kind::FootnoteDefinition,
            0,
            Some(Payload::Footnote {
                id: id.index() as u32,
            }),
        ),
        NodeKind::Text(text) => (Kind::Text, 0, Some(Payload::Text(text.as_str().into()))),
        NodeKind::Emphasis => (Kind::Emphasis, 0, None),
        NodeKind::Strong => (Kind::Strong, 0, None),
        NodeKind::Strikethrough => (Kind::Strikethrough, 0, None),
        NodeKind::InlineCode(text) => (
            Kind::InlineCode,
            0,
            Some(Payload::Text(text.as_str().into())),
        ),
        NodeKind::Link(link) => (
            Kind::Link,
            0,
            Some(Payload::Link {
                destination: link.destination.clone(),
                title: link.title.clone(),
                fragment_only: link.fragment_only,
            }),
        ),
        NodeKind::Image(image) => (
            Kind::Image,
            0,
            Some(Payload::Image {
                source: image.source.clone(),
                alt: image.alt.clone(),
                title: image.title.clone(),
                width: image.width,
                height: image.height,
            }),
        ),
        NodeKind::HardBreak => (Kind::HardBreak, 0, None),
        NodeKind::FootnoteReference(id) => (
            Kind::FootnoteReference,
            0,
            Some(Payload::Footnote {
                id: id.index() as u32,
            }),
        ),
        NodeKind::TaskMarker(marker) => (
            Kind::TaskMarker,
            0,
            Some(Payload::TaskMarker {
                checked: marker.checked,
                fallback_label: marker.fallback_label.clone(),
            }),
        ),
        NodeKind::InlineMath(math) => (
            Kind::InlineMath,
            0,
            Some(Payload::Math {
                source: math.source.clone(),
                format: math_format_code(math.format),
                fallback_text: math.fallback_text.clone(),
            }),
        ),
        NodeKind::DisplayMath(math) => (
            Kind::DisplayMath,
            0,
            Some(Payload::Math {
                source: math.source.clone(),
                format: math_format_code(math.format),
                fallback_text: math.fallback_text.clone(),
            }),
        ),
        NodeKind::Media(media) => (
            Kind::Media,
            0,
            Some(Payload::Media {
                kind: media_kind_code(media.kind),
                source: media.source.clone(),
                title: media.title.clone(),
            }),
        ),
        NodeKind::Invalid => unreachable!("benchmark captured an invalid semantic node"),
    }
}

fn alignment_code(alignment: Option<document::TableAlignment>) -> u8 {
    match alignment {
        None => 0,
        Some(document::TableAlignment::Left) => 1,
        Some(document::TableAlignment::Center) => 2,
        Some(document::TableAlignment::Right) => 3,
    }
}

fn callout_kind_code(kind: document::CalloutKind) -> u8 {
    match kind {
        document::CalloutKind::Note => 0,
        document::CalloutKind::Warning => 1,
        document::CalloutKind::Tip => 2,
        document::CalloutKind::Important => 3,
        document::CalloutKind::Caution => 4,
        document::CalloutKind::Info => 5,
    }
}

fn math_format_code(format: document::MathFormat) -> u8 {
    match format {
        document::MathFormat::Tex => 0,
        document::MathFormat::Text => 1,
    }
}

fn media_kind_code(kind: document::MediaKind) -> u8 {
    match kind {
        document::MediaKind::Audio => 0,
        document::MediaKind::Video => 1,
        document::MediaKind::Embedded => 2,
    }
}

fn source_view<'a>(document: &'a Document, id: document::DocumentNodeId) -> Option<View<'a>> {
    let node = document.node(id)?;
    let view = match node.kind() {
        NodeKind::Paragraph => View {
            kind: Kind::Paragraph,
            aux: 0,
            value: Value::None,
        },
        NodeKind::BlockGroup => View {
            kind: Kind::BlockGroup,
            aux: 0,
            value: Value::None,
        },
        NodeKind::Heading { .. } => View {
            kind: Kind::Heading,
            aux: node.kind().heading_level().unwrap_or(1) as u16,
            value: Value::None,
        },
        NodeKind::BlockQuote => View {
            kind: Kind::BlockQuote,
            aux: 0,
            value: Value::None,
        },
        NodeKind::CodeBlock(code) => View {
            kind: Kind::CodeBlock,
            aux: 0,
            value: Value::CodeBlock {
                language: code.language.as_deref(),
                text: &code.text,
            },
        },
        NodeKind::List(list) => View {
            kind: Kind::List,
            aux: 0,
            value: Value::List {
                ordered: matches!(list.kind, document::ListKind::Ordered),
                start: list.start,
            },
        },
        NodeKind::ListItem => View {
            kind: Kind::ListItem,
            aux: 0,
            value: Value::None,
        },
        NodeKind::Table(table) => View {
            kind: Kind::Table,
            aux: 0,
            value: Value::Table {
                column_count: table.column_count,
            },
        },
        NodeKind::TableCaption => View {
            kind: Kind::TableCaption,
            aux: 0,
            value: Value::None,
        },
        NodeKind::TableRow => View {
            kind: Kind::TableRow,
            aux: 0,
            value: Value::None,
        },
        NodeKind::TableCell(cell) => View {
            kind: Kind::TableCell,
            aux: 0,
            value: Value::TableCell {
                header: cell.header,
                colspan: cell.colspan,
                rowspan: cell.rowspan,
                alignment: alignment_code(cell.alignment),
            },
        },
        NodeKind::Figure => View {
            kind: Kind::Figure,
            aux: 0,
            value: Value::None,
        },
        NodeKind::Figcaption => View {
            kind: Kind::Figcaption,
            aux: 0,
            value: Value::None,
        },
        NodeKind::Details => View {
            kind: Kind::Details,
            aux: 0,
            value: Value::None,
        },
        NodeKind::Summary => View {
            kind: Kind::Summary,
            aux: 0,
            value: Value::None,
        },
        NodeKind::ThematicBreak => View {
            kind: Kind::ThematicBreak,
            aux: 0,
            value: Value::None,
        },
        NodeKind::DefinitionList => View {
            kind: Kind::DefinitionList,
            aux: 0,
            value: Value::None,
        },
        NodeKind::DefinitionTerm => View {
            kind: Kind::DefinitionTerm,
            aux: 0,
            value: Value::None,
        },
        NodeKind::DefinitionDescription => View {
            kind: Kind::DefinitionDescription,
            aux: 0,
            value: Value::None,
        },
        NodeKind::Callout(callout) => View {
            kind: Kind::Callout,
            aux: 0,
            value: Value::Callout {
                kind: callout_kind_code(callout.kind),
                title: callout.title.as_deref(),
            },
        },
        NodeKind::FootnoteDefinition(id) => View {
            kind: Kind::FootnoteDefinition,
            aux: 0,
            value: Value::Footnote {
                label: document.footnote_label(id).unwrap_or_default(),
            },
        },
        NodeKind::Text(text) => View {
            kind: Kind::Text,
            aux: 0,
            value: Value::Text(text.as_str()),
        },
        NodeKind::Emphasis => View {
            kind: Kind::Emphasis,
            aux: 0,
            value: Value::None,
        },
        NodeKind::Strong => View {
            kind: Kind::Strong,
            aux: 0,
            value: Value::None,
        },
        NodeKind::Strikethrough => View {
            kind: Kind::Strikethrough,
            aux: 0,
            value: Value::None,
        },
        NodeKind::InlineCode(text) => View {
            kind: Kind::InlineCode,
            aux: 0,
            value: Value::Text(text.as_str()),
        },
        NodeKind::Link(link) => View {
            kind: Kind::Link,
            aux: 0,
            value: Value::Link {
                destination: &link.destination,
                title: link.title.as_deref(),
                fragment_only: link.fragment_only,
            },
        },
        NodeKind::Image(image) => View {
            kind: Kind::Image,
            aux: 0,
            value: Value::Image {
                source: &image.source,
                alt: &image.alt,
                title: image.title.as_deref(),
                width: image.width,
                height: image.height,
            },
        },
        NodeKind::HardBreak => View {
            kind: Kind::HardBreak,
            aux: 0,
            value: Value::None,
        },
        NodeKind::FootnoteReference(id) => View {
            kind: Kind::FootnoteReference,
            aux: 0,
            value: Value::Footnote {
                label: document.footnote_label(id).unwrap_or_default(),
            },
        },
        NodeKind::TaskMarker(marker) => View {
            kind: Kind::TaskMarker,
            aux: 0,
            value: Value::TaskMarker {
                checked: marker.checked,
                fallback_label: marker.fallback_label.as_deref(),
            },
        },
        NodeKind::InlineMath(math) => View {
            kind: Kind::InlineMath,
            aux: 0,
            value: Value::Math {
                source: &math.source,
                format: math_format_code(math.format),
                fallback_text: math.fallback_text.as_deref(),
            },
        },
        NodeKind::DisplayMath(math) => View {
            kind: Kind::DisplayMath,
            aux: 0,
            value: Value::Math {
                source: &math.source,
                format: math_format_code(math.format),
                fallback_text: math.fallback_text.as_deref(),
            },
        },
        NodeKind::Media(media) => View {
            kind: Kind::Media,
            aux: 0,
            value: Value::Media {
                kind: media_kind_code(media.kind),
                source: &media.source,
                title: media.title.as_deref(),
            },
        },
        NodeKind::Invalid => return None,
    };
    Some(view)
}

fn preorder_view<'a>(
    node: &'a PreorderNode,
    payloads: &'a [Payload],
    footnotes: &'a [Option<Box<str>>],
) -> View<'a> {
    View {
        kind: Kind::from_u8(node.kind),
        aux: node.aux,
        value: payload_value(node.payload, payloads, footnotes),
    }
}

fn event_view<'a>(
    op: &'a Op,
    payloads: &'a [Payload],
    footnotes: &'a [Option<Box<str>>],
) -> View<'a> {
    View {
        kind: Kind::from_u8(op.opcode & KIND_MASK),
        aux: op.aux,
        value: payload_value(op.payload, payloads, footnotes),
    }
}

fn payload_value<'a>(
    index: u32,
    payloads: &'a [Payload],
    footnotes: &'a [Option<Box<str>>],
) -> Value<'a> {
    let Some(payload) = (index != NO_PAYLOAD)
        .then(|| payloads.get(index as usize))
        .flatten()
    else {
        return Value::None;
    };
    value_from_payload(payload, footnotes)
}

fn value_from_payload<'a>(payload: &'a Payload, footnotes: &'a [Option<Box<str>>]) -> Value<'a> {
    match payload {
        Payload::Text(value) => Value::Text(value.as_ref()),
        Payload::CodeBlock { language, text } => Value::CodeBlock {
            language: language.as_deref(),
            text: text.as_ref(),
        },
        Payload::Link {
            destination,
            title,
            fragment_only,
        } => Value::Link {
            destination: destination.as_ref(),
            title: title.as_deref(),
            fragment_only: *fragment_only,
        },
        Payload::Image {
            source,
            alt,
            title,
            width,
            height,
        } => Value::Image {
            source: source.as_ref(),
            alt: alt.as_ref(),
            title: title.as_deref(),
            width: *width,
            height: *height,
        },
        Payload::List { ordered, start } => Value::List {
            ordered: *ordered,
            start: *start,
        },
        Payload::Table { column_count } => Value::Table {
            column_count: *column_count,
        },
        Payload::TableCell {
            header,
            colspan,
            rowspan,
            alignment,
        } => Value::TableCell {
            header: *header,
            colspan: *colspan,
            rowspan: *rowspan,
            alignment: *alignment,
        },
        Payload::Callout { kind, title } => Value::Callout {
            kind: *kind,
            title: title.as_deref(),
        },
        Payload::TaskMarker {
            checked,
            fallback_label,
        } => Value::TaskMarker {
            checked: *checked,
            fallback_label: fallback_label.as_deref(),
        },
        Payload::Math {
            source,
            format,
            fallback_text,
        } => Value::Math {
            source: source.as_ref(),
            format: *format,
            fallback_text: fallback_text.as_deref(),
        },
        Payload::Media {
            kind,
            source,
            title,
        } => Value::Media {
            kind: *kind,
            source: source.as_ref(),
            title: title.as_deref(),
        },
        Payload::Footnote { id } => Value::Footnote {
            label: footnotes
                .get(*id as usize)
                .and_then(Option::as_deref)
                .unwrap_or_default(),
        },
    }
}

#[derive(Clone, Copy)]
enum Projection {
    Markdown,
    Html,
    Text,
}

fn render_arena(document: &Document, projection: Projection) -> String {
    let mut output = String::with_capacity(document.output_capacity_hint());
    let mut tasks = Vec::with_capacity(32);
    tasks.extend(document.root_ids().rev().map(ArenaTask::Enter));
    while let Some(task) = tasks.pop() {
        match task {
            ArenaTask::Enter(id) => {
                let Some(view) = source_view(document, id) else {
                    continue;
                };
                emit_enter(&mut output, projection, view);
                if view.kind.is_container() {
                    tasks.push(ArenaTask::Exit(id));
                    if let Some(child) = document.first_child(id) {
                        tasks.push(ArenaTask::Siblings(child));
                    }
                }
            }
            ArenaTask::Siblings(id) => {
                if let Some(sibling) = document.next_sibling(id) {
                    tasks.push(ArenaTask::Siblings(sibling));
                }
                tasks.push(ArenaTask::Enter(id));
            }
            ArenaTask::Exit(id) => {
                if let Some(view) = source_view(document, id) {
                    emit_exit(&mut output, projection, view);
                }
            }
        }
    }
    output
}

fn render_preorder(document: &PreorderDocument, projection: Projection) -> String {
    let mut output = String::with_capacity(document.output_capacity_hint);
    let mut closes: Vec<PreorderClose> = Vec::with_capacity(32);
    let mut index = 0usize;
    while index < document.nodes.len() {
        while closes.last().is_some_and(|close| close.0 == index as u32) {
            let (_, kind, payload, aux) = closes.pop().unwrap();
            emit_exit(
                &mut output,
                projection,
                View {
                    kind,
                    aux,
                    value: payload_value(payload, &document.payloads, &document.footnotes),
                },
            );
        }
        let node = document.nodes[index];
        let view = preorder_view(&node, &document.payloads, &document.footnotes);
        emit_enter(&mut output, projection, view);
        if node.flags & 1 != 0 {
            closes.push((node.subtree_end, view.kind, node.payload, node.aux));
        }
        index += 1;
    }
    while let Some((_, kind, payload, aux)) = closes.pop() {
        emit_exit(
            &mut output,
            projection,
            View {
                kind,
                aux,
                value: payload_value(payload, &document.payloads, &document.footnotes),
            },
        );
    }
    output
}

fn render_events(document: &EventDocument, projection: Projection) -> String {
    let mut output = String::with_capacity(document.output_capacity_hint);
    for op in &document.ops {
        let view = event_view(op, &document.payloads, &document.footnotes);
        if op.opcode & CLOSE_BIT != 0 {
            emit_exit(&mut output, projection, view);
        } else {
            emit_enter(&mut output, projection, view);
        }
    }
    output
}

fn arena_task_peak(document: &Document) -> usize {
    let mut tasks = Vec::with_capacity(32);
    tasks.extend(document.root_ids().rev().map(ArenaTask::Enter));
    let mut peak = tasks.len();
    while let Some(task) = tasks.pop() {
        match task {
            ArenaTask::Enter(id) => {
                if source_view(document, id).is_some_and(|view| view.kind.is_container()) {
                    tasks.push(ArenaTask::Exit(id));
                    if let Some(child) = document.first_child(id) {
                        tasks.push(ArenaTask::Siblings(child));
                    }
                }
            }
            ArenaTask::Siblings(id) => {
                if let Some(sibling) = document.next_sibling(id) {
                    tasks.push(ArenaTask::Siblings(sibling));
                }
                tasks.push(ArenaTask::Enter(id));
            }
            ArenaTask::Exit(_) => {}
        }
        peak = peak.max(tasks.len());
    }
    peak
}

fn preorder_close_peak(document: &PreorderDocument) -> usize {
    let mut closes: Vec<PreorderClose> = Vec::with_capacity(32);
    let mut peak = 0;
    for (index, node) in document.nodes.iter().enumerate() {
        while closes.last().is_some_and(|close| close.0 == index as u32) {
            closes.pop();
        }
        if node.flags & 1 != 0 {
            closes.push((
                node.subtree_end,
                Kind::from_u8(node.kind),
                node.payload,
                node.aux,
            ));
            peak = peak.max(closes.len());
        }
    }
    peak
}

fn emit_enter(output: &mut String, projection: Projection, view: View<'_>) {
    if let Value::Link { fragment_only, .. } = view.value {
        let _ = fragment_only;
    }
    if let Value::Callout { kind, .. } = view.value {
        let _ = kind;
    }
    if let Value::Math { format, .. } = view.value {
        let _ = format;
    }
    match projection {
        Projection::Markdown => emit_markdown_enter(output, view),
        Projection::Html => emit_html_enter(output, view),
        Projection::Text => emit_text_enter(output, view),
    }
}

fn emit_exit(output: &mut String, projection: Projection, view: View<'_>) {
    match projection {
        Projection::Markdown => emit_markdown_exit(output, view),
        Projection::Html => emit_html_exit(output, view),
        Projection::Text => {}
    }
}

fn emit_markdown_enter(output: &mut String, view: View<'_>) {
    match view.kind {
        Kind::Text => value_text(output, view.value),
        Kind::CodeBlock => {
            if let Value::CodeBlock { language, text } = view.value {
                output.push_str("\n```");
                if let Some(language) = language {
                    output.push_str(language);
                }
                output.push('\n');
                output.push_str(text);
                output.push_str("\n```\n");
            }
        }
        Kind::InlineCode => {
            output.push('`');
            value_text(output, view.value);
            output.push('`');
        }
        Kind::Link => output.push('['),
        Kind::Image => {
            if let Value::Image { alt, .. } = view.value {
                output.push_str("![");
                output.push_str(alt);
                output.push_str("](");
                if let Value::Image { source, title, .. } = view.value {
                    output.push_str(source);
                    if let Some(title) = title {
                        output.push_str(" \"");
                        output.push_str(title);
                        output.push('"');
                    }
                }
                output.push(')');
            }
        }
        Kind::FootnoteReference => {
            if let Value::Footnote { label } = view.value {
                output.push_str("[^");
                output.push_str(label);
                output.push(']');
            }
        }
        Kind::TaskMarker => {
            if let Value::TaskMarker { checked, .. } = view.value {
                output.push('[');
                output.push(if checked { 'x' } else { ' ' });
                output.push_str("] ");
            }
        }
        Kind::InlineMath => {
            if let Value::Math { source, .. } = view.value {
                output.push('$');
                output.push_str(source);
                output.push('$');
            }
        }
        Kind::DisplayMath => {
            if let Value::Math { source, .. } = view.value {
                output.push_str("\n$$\n");
                output.push_str(source);
                output.push_str("\n$$\n");
            }
        }
        Kind::Media => {
            if let Value::Media { source, title, .. } = view.value {
                output.push('[');
                output.push_str(title.unwrap_or(source));
                output.push_str("](");
                output.push_str(source);
                output.push(')');
            }
        }
        Kind::HardBreak => output.push_str("  \n"),
        Kind::ThematicBreak => output.push_str("\n---\n"),
        Kind::Heading => {
            output.push('\n');
            for _ in 0..view.aux.max(1) {
                output.push('#');
            }
            output.push(' ');
        }
        Kind::BlockQuote => output.push_str("\n> "),
        Kind::Strong => output.push_str("**"),
        Kind::Emphasis => output.push('*'),
        Kind::Strikethrough => output.push_str("~~"),
        Kind::List => output.push('\n'),
        Kind::ListItem => output.push_str("- "),
        Kind::Table => output.push('\n'),
        Kind::TableRow => output.push('|'),
        Kind::TableCell => output.push(' '),
        Kind::Paragraph
        | Kind::BlockGroup
        | Kind::TableCaption
        | Kind::Figure
        | Kind::Figcaption
        | Kind::Details
        | Kind::Summary
        | Kind::DefinitionList
        | Kind::DefinitionTerm
        | Kind::DefinitionDescription
        | Kind::Callout
        | Kind::FootnoteDefinition => output.push('\n'),
    }
}

fn emit_markdown_exit(output: &mut String, view: View<'_>) {
    match view.kind {
        Kind::Link => {
            output.push(']');
            if let Value::Link {
                destination, title, ..
            } = view.value
            {
                output.push('(');
                output.push_str(destination);
                if let Some(title) = title {
                    output.push_str(" \"");
                    output.push_str(title);
                    output.push('"');
                }
                output.push(')');
            }
        }
        Kind::Strong => output.push_str("**"),
        Kind::Emphasis => output.push('*'),
        Kind::Strikethrough => output.push_str("~~"),
        Kind::TableRow => output.push_str("|\n"),
        Kind::Paragraph
        | Kind::BlockGroup
        | Kind::Heading
        | Kind::BlockQuote
        | Kind::List
        | Kind::ListItem
        | Kind::Table
        | Kind::TableCaption
        | Kind::TableCell
        | Kind::Figure
        | Kind::Figcaption
        | Kind::Details
        | Kind::Summary
        | Kind::DefinitionList
        | Kind::DefinitionTerm
        | Kind::DefinitionDescription
        | Kind::Callout
        | Kind::FootnoteDefinition => output.push('\n'),
        _ => {}
    }
}

fn emit_html_enter(output: &mut String, view: View<'_>) {
    match view.kind {
        Kind::Text => {
            if let Value::Text(value) = view.value {
                escape_html(output, value);
            }
        }
        Kind::CodeBlock => {
            if let Value::CodeBlock { language, text } = view.value {
                output.push_str("<pre><code");
                if let Some(language) = language {
                    output.push_str(" class=\"language-");
                    escape_attribute(output, language);
                    output.push('"');
                }
                output.push('>');
                escape_html(output, text);
                output.push_str("</code></pre>");
            }
        }
        Kind::InlineCode => {
            output.push_str("<code>");
            if let Value::Text(value) = view.value {
                escape_html(output, value);
            }
            output.push_str("</code>");
        }
        Kind::Link => {
            output.push_str("<a href=\"");
            if let Value::Link { destination, .. } = view.value {
                escape_attribute(output, destination);
            }
            output.push_str("\">");
        }
        Kind::Image => {
            if let Value::Image {
                source,
                alt,
                title,
                width,
                height,
            } = view.value
            {
                output.push_str("<img src=\"");
                escape_attribute(output, source);
                output.push_str("\" alt=\"");
                escape_attribute(output, alt);
                output.push('"');
                if let Some(title) = title {
                    output.push_str(" title=\"");
                    escape_attribute(output, title);
                    output.push('"');
                }
                if let Some(width) = width {
                    write!(output, " width=\"{width}\"").unwrap();
                }
                if let Some(height) = height {
                    write!(output, " height=\"{height}\"").unwrap();
                }
                output.push('>');
            }
        }
        Kind::FootnoteReference => {
            if let Value::Footnote { label } = view.value {
                output.push_str("<sup><a href=\"#footnote-");
                escape_attribute(output, label);
                output.push_str("\" role=\"doc-noteref\">");
                escape_html(output, label);
                output.push_str("</a></sup>");
            }
        }
        Kind::TaskMarker => {
            if let Value::TaskMarker {
                checked,
                fallback_label,
            } = view.value
            {
                output.push_str("<input type=\"checkbox\" disabled=\"\"");
                if checked {
                    output.push_str(" checked=\"\"");
                }
                if let Some(label) = fallback_label {
                    output.push_str(" aria-label=\"");
                    escape_attribute(output, label);
                    output.push('"');
                }
                output.push('>');
                if let Some(label) = fallback_label {
                    escape_html(output, label);
                }
            }
        }
        Kind::InlineMath | Kind::DisplayMath => {
            let class = if matches!(view.kind, Kind::DisplayMath) {
                "math display-math"
            } else {
                "math"
            };
            if let Value::Math { source, .. } = view.value {
                write!(output, "<span class=\"{class}\">").unwrap();
                escape_html(output, source);
                output.push_str("</span>");
            }
        }
        Kind::Media => {
            if let Value::Media {
                kind,
                source,
                title,
            } = view.value
            {
                match kind {
                    0 => {
                        output.push_str("<audio controls src=\"");
                        escape_attribute(output, source);
                        output.push_str("\"></audio>");
                    }
                    1 => {
                        output.push_str("<video controls src=\"");
                        escape_attribute(output, source);
                        output.push_str("\"></video>");
                    }
                    _ => {
                        output.push_str("<a href=\"");
                        escape_attribute(output, source);
                        output.push_str("\">");
                        escape_html(output, title.unwrap_or(source));
                        output.push_str("</a>");
                    }
                }
            }
        }
        Kind::HardBreak => output.push_str("<br>"),
        Kind::ThematicBreak => output.push_str("<hr>"),
        Kind::Paragraph => output.push_str("<p>"),
        Kind::BlockGroup => output.push_str("<div>"),
        Kind::Heading => write!(output, "<h{}>", view.aux.clamp(1, 6)).unwrap(),
        Kind::BlockQuote | Kind::Callout => output.push_str("<blockquote>"),
        Kind::Strong => output.push_str("<strong>"),
        Kind::Emphasis => output.push_str("<em>"),
        Kind::Strikethrough => output.push_str("<del>"),
        Kind::List => {
            if let Value::List { ordered, start } = view.value {
                if ordered {
                    output.push_str("<ol");
                    if let Some(start) = start.filter(|start| *start != 1) {
                        write!(output, " start=\"{start}\"").unwrap();
                    }
                    output.push('>');
                } else {
                    output.push_str("<ul>");
                }
            }
        }
        Kind::ListItem => output.push_str("<li>"),
        Kind::Table => output.push_str("<table>"),
        Kind::TableCaption => output.push_str("<caption>"),
        Kind::TableRow => output.push_str("<tr>"),
        Kind::TableCell => {
            if let Value::TableCell {
                header,
                colspan,
                rowspan,
                alignment,
            } = view.value
            {
                output.push('<');
                output.push_str(if header { "th" } else { "td" });
                if colspan > 1 {
                    write!(output, " colspan=\"{colspan}\"").unwrap();
                }
                if rowspan > 1 {
                    write!(output, " rowspan=\"{rowspan}\"").unwrap();
                }
                if let Some(alignment) = alignment_name(alignment) {
                    write!(output, " align=\"{alignment}\"").unwrap();
                }
                output.push('>');
            }
        }
        Kind::Figure => output.push_str("<figure>"),
        Kind::Figcaption => output.push_str("<figcaption>"),
        Kind::Details => output.push_str("<details>"),
        Kind::Summary => output.push_str("<summary>"),
        Kind::DefinitionList => output.push_str("<dl>"),
        Kind::DefinitionTerm => output.push_str("<dt>"),
        Kind::DefinitionDescription => output.push_str("<dd>"),
        Kind::FootnoteDefinition => {
            output.push_str("<aside id=\"footnote-");
            if let Value::Footnote { label } = view.value {
                escape_attribute(output, label);
            }
            output.push_str("\" role=\"doc-footnote\">");
        }
    }
}

fn emit_html_exit(output: &mut String, view: View<'_>) {
    let tag = match view.kind {
        Kind::Paragraph => Some("p"),
        Kind::BlockGroup => Some("div"),
        Kind::Heading => Some(match view.aux.clamp(1, 6) {
            1 => "h1",
            2 => "h2",
            3 => "h3",
            4 => "h4",
            5 => "h5",
            _ => "h6",
        }),
        Kind::BlockQuote | Kind::Callout => Some("blockquote"),
        Kind::Strong => Some("strong"),
        Kind::Emphasis => Some("em"),
        Kind::Strikethrough => Some("del"),
        Kind::List => match view.value {
            Value::List { ordered: true, .. } => Some("ol"),
            Value::List { ordered: false, .. } => Some("ul"),
            _ => None,
        },
        Kind::ListItem => Some("li"),
        Kind::Table => Some("table"),
        Kind::TableCaption => Some("caption"),
        Kind::TableRow => Some("tr"),
        Kind::TableCell => match view.value {
            Value::TableCell { header: true, .. } => Some("th"),
            Value::TableCell { header: false, .. } => Some("td"),
            _ => None,
        },
        Kind::Figure => Some("figure"),
        Kind::Figcaption => Some("figcaption"),
        Kind::Details => Some("details"),
        Kind::Summary => Some("summary"),
        Kind::DefinitionList => Some("dl"),
        Kind::DefinitionTerm => Some("dt"),
        Kind::DefinitionDescription => Some("dd"),
        Kind::FootnoteDefinition => Some("aside"),
        _ => None,
    };
    if let Some(tag) = tag {
        write!(output, "</{tag}>").unwrap();
    }
}

fn emit_text_enter(output: &mut String, view: View<'_>) {
    match view.value {
        Value::Text(value) => append_text(output, value),
        Value::CodeBlock { text, .. } => append_text(output, text),
        Value::TaskMarker {
            fallback_label: Some(label),
            ..
        } => append_text(output, label),
        Value::Math {
            source,
            fallback_text,
            ..
        } => append_text(output, fallback_text.unwrap_or(source)),
        Value::Media { source, title, .. } => append_text(output, title.unwrap_or(source)),
        Value::Table { column_count } => {
            let _ = column_count;
        }
        Value::TableCell {
            header,
            colspan,
            rowspan,
            alignment,
        } => {
            let _ = (header, colspan, rowspan, alignment);
        }
        Value::Callout { title, .. } => {
            if let Some(title) = title {
                append_text(output, title);
            }
        }
        Value::TaskMarker { .. }
        | Value::Footnote { .. }
        | Value::Link { .. }
        | Value::Image { .. }
        | Value::List { .. }
        | Value::None => {
            if view.kind == Kind::HardBreak
                || view.kind == Kind::ThematicBreak
                || (view.kind.is_container()
                    && matches!(
                        view.kind,
                        Kind::Paragraph
                            | Kind::BlockGroup
                            | Kind::Heading
                            | Kind::BlockQuote
                            | Kind::ListItem
                            | Kind::TableRow
                            | Kind::TableCell
                            | Kind::Figure
                            | Kind::Figcaption
                            | Kind::Details
                            | Kind::Summary
                            | Kind::DefinitionTerm
                            | Kind::DefinitionDescription
                            | Kind::Callout
                            | Kind::FootnoteDefinition
                    ))
            {
                append_text(output, " ");
            }
        }
    }
}

fn value_text(output: &mut String, value: Value<'_>) {
    match value {
        Value::Text(value) => output.push_str(value),
        Value::CodeBlock { text, .. } => output.push_str(text),
        _ => {}
    }
}

fn append_text(output: &mut String, value: &str) {
    if !output.is_empty() && !output.ends_with(char::is_whitespace) {
        output.push(' ');
    }
    output.push_str(value);
}

fn alignment_name(value: u8) -> Option<&'static str> {
    match value {
        1 => Some("left"),
        2 => Some("center"),
        3 => Some("right"),
        _ => None,
    }
}

fn escape_html(output: &mut String, value: &str) {
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

fn semantic_payload_fragment(target_bytes: usize) -> String {
    let mut html = String::with_capacity(target_bytes + 256);
    let mut index = 0;
    while html.len() < target_bytes {
        html.push_str(&format!(
            "<section><h2>Payload {index}</h2><aside class='admonition warning'><p>Warning {index}</p></aside><ol start='3'><li><input type='checkbox' checked>Task {index}</li></ol><figure><img src='/image-{index}.jpg' alt='Sized image' width='640' height='480'><figcaption>Caption {index}</figcaption></figure><table><tr><th colspan='2' rowspan='2' align='center'>Header</th></tr><tr><td>Cell {index}</td></tr></table><p data-latex='x^2'>Equation {index}</p><audio src='/audio-{index}.mp3'>Audio</audio></section>"
        ));
        index += 1;
    }
    html
}

fn build_document(html: &str) -> Document {
    let dom = dom::Dom::parse_fragment(html, Tag::Div).unwrap();
    let root = dom.root();
    let base = url::Url::parse("https://example.com/docs/page").unwrap();
    let context = document::CompileContext::new(Some(base.clone()), Some(&base));
    let source_evidence =
        document::SourceEvidence::analyze(&dom, root, &dom::NodeStateStore::new());
    let source_facts = document::SemanticSourceFacts::analyze(&dom, root);
    document::compile_document_with_optional_source_facts_and_evidence(
        &dom,
        root,
        &context,
        Some(&source_facts),
        Some(&source_evidence),
    )
    .unwrap()
}

fn format_name(projection: Projection) -> &'static str {
    match projection {
        Projection::Markdown => "markdown",
        Projection::Html => "html",
        Projection::Text => "text",
    }
}

fn assert_deep_layouts_are_stack_safe() {
    const DEPTH: usize = 10_000;
    let mut builder = document::DocumentBuilder::with_capacity(DEPTH + 1);
    let mut parent = None;
    for _ in 0..DEPTH {
        parent = Some(
            builder
                .append(parent, document::NodeKind::BlockQuote)
                .unwrap(),
        );
    }
    builder.append_prose(parent, "deep").unwrap();
    let document = builder.finish();
    assert_captured_fields(&document);
    let preorder = PreorderDocument::from_document(&document);
    let events = EventDocument::from_document(&document);

    for projection in [Projection::Markdown, Projection::Html, Projection::Text] {
        let expected = render_arena(&document, projection);
        assert_eq!(expected, render_preorder(&preorder, projection));
        assert_eq!(expected, render_events(&events, projection));
    }
}

pub(crate) fn benchmark(c: &mut Criterion) {
    assert_eq!(size_of::<PreorderNode>(), 12);
    assert_eq!(size_of::<Op>(), 8);
    assert_deep_layouts_are_stack_safe();

    let mut group = c.benchmark_group("compact_ir_prototype");
    group.sample_size(20);

    eprintln!(
        "compact-ir/layout: event_op_bytes={}, node_kind_bytes={}, preorder_header_bytes={}, event_header_bytes={}, payload_slot_bytes={}",
        size_of::<document::EventOp>(),
        size_of::<document::NodeKind>(),
        size_of::<PreorderNode>(),
        size_of::<Op>(),
        size_of::<Payload>(),
    );

    for (name, kind, bytes) in [
        ("simple-prose", "prose", 4_000),
        ("ordinary-inline", "ordinary-inline", 50_000),
        ("highlighted-code", "code", 100_000),
        ("table-heavy", "tables", 100_000),
        ("math", "math", 100_000),
        ("semantic-payloads", "semantic-payloads", 100_000),
        ("footnotes", "footnotes", 100_000),
        ("documentation", "reference", 100_000),
    ] {
        let html = if kind == "semantic-payloads" {
            semantic_payload_fragment(bytes)
        } else {
            super::retained_fragment(kind, bytes)
        };
        let document = build_document(&html);
        assert_captured_fields(&document);
        let preorder = PreorderDocument::from_document(&document);
        let events = EventDocument::from_document(&document);
        let semantic_nodes = document.len();
        let arena_stack_peak = arena_task_peak(&document);
        let preorder_stack_peak = preorder_close_peak(&preorder);

        let arena_bytes = document.retained_bytes_estimate();
        let preorder_bytes = preorder.retained_bytes();
        let event_bytes = events.retained_bytes();
        let arena_stack_capacity = arena_stack_peak.max(32).next_power_of_two();
        let preorder_stack_capacity = preorder_stack_peak.max(32).next_power_of_two();
        eprintln!(
            "compact-ir/{name}: production_compat_bytes={arena_bytes}, preorder_bytes={preorder_bytes}, event_bytes={event_bytes}, production_compat_non_string_bytes={}, preorder_non_string_bytes={}, event_non_string_bytes={}, production_compat_nodes={semantic_nodes}, preorder_nodes={}, event_ops={}, preorder_payloads={}, event_payloads={}, preorder_payload_slot_bytes={}, event_payload_slot_bytes={}, preorder_strings={}, event_strings={}, preorder_string_values={}, event_string_values={}, footnote_slots={}, production_compat_task_peak={}, preorder_close_peak={}, event_stack_peak=0, event_builder_stack_peak={}, production_compat_task_capacity_bytes={}, preorder_stack_capacity_bytes={}, event_builder_stack_capacity_bytes={}",
            arena_bytes.saturating_sub(document.semantic_string_bytes()),
            preorder_bytes.saturating_sub(preorder.string_bytes()),
            event_bytes.saturating_sub(events.string_bytes()),
            preorder.nodes.len(),
            events.ops.len(),
            preorder.payloads.len(),
            events.payloads.len(),
            preorder.payloads.capacity() * size_of::<Payload>(),
            events.payloads.capacity() * size_of::<Payload>(),
            preorder.string_bytes(),
            events.string_bytes(),
            preorder.string_values(),
            events.string_values(),
            preorder.footnotes.len().max(events.footnotes.len()),
            arena_stack_peak,
            preorder_stack_peak,
            preorder_stack_peak,
            arena_stack_capacity * size_of::<ArenaTask>(),
            preorder_stack_capacity * size_of::<PreorderClose>(),
            preorder_stack_capacity * size_of::<(usize, u32, u16)>(),
        );

        group.throughput(Throughput::Elements(semantic_nodes as u64));
        group.bench_function(BenchmarkId::new("build-preorder", name), |b| {
            b.iter(|| black_box(PreorderDocument::from_document(black_box(&document))))
        });
        group.bench_function(BenchmarkId::new("build-events", name), |b| {
            b.iter(|| black_box(EventDocument::from_document(black_box(&document))))
        });

        for projection in [Projection::Markdown, Projection::Html, Projection::Text] {
            let arena = render_arena(&document, projection);
            let preorder_output = render_preorder(&preorder, projection);
            let event_output = render_events(&events, projection);
            assert_eq!(
                arena,
                preorder_output,
                "preorder projection differs for {name}/{}",
                format_name(projection)
            );
            assert_eq!(
                arena,
                event_output,
                "event projection differs for {name}/{}",
                format_name(projection)
            );

            group.throughput(Throughput::Elements(semantic_nodes as u64));
            group.bench_function(
                BenchmarkId::new(
                    "production-compat",
                    format!("{name}/{}", format_name(projection)),
                ),
                |b| b.iter(|| black_box(render_arena(black_box(&document), projection))),
            );
            group.bench_function(
                BenchmarkId::new("preorder", format!("{name}/{}", format_name(projection))),
                |b| b.iter(|| black_box(render_preorder(black_box(&preorder), projection))),
            );
            group.bench_function(
                BenchmarkId::new("events", format!("{name}/{}", format_name(projection))),
                |b| b.iter(|| black_box(render_events(black_box(&events), projection))),
            );
        }
    }
    group.finish();
}
