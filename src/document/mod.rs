//! Private semantic representation and lowering support.
//!
//! This module is an implementation detail. Keep its storage and traversal APIs
//! private so the representation can change without changing Legible's public
//! output contract.

#![allow(dead_code)]

mod builder;
mod callouts;
mod code;
mod compiler;
mod facts;
mod figures;
mod footnotes;
mod headings;
mod images;
mod lists;
mod math;
mod media;
mod ordinary;
pub(crate) mod stats;
mod tables;
mod text;
mod uri;
mod validate;

pub(crate) use builder::{BuildError, DocumentBuilder, SemanticTapeBuilder};
pub(crate) use code::{
    count_blocks as source_code_block_count,
    is_multiline_orphan_with_evidence as is_multiline_code_with_evidence,
    multiline_content as code_multiline_content,
};
#[allow(unused_imports)]
pub(crate) use compiler::{
    CompileContext, compile_document, compile_document_owned_with_optional_source_facts,
    compile_document_owned_with_optional_source_facts_and_evidence,
    compile_document_with_optional_source_facts,
    compile_document_with_optional_source_facts_and_evidence,
};
pub(crate) use facts::{SemanticGate, SemanticSourceFacts, SourceEvidence};
pub(crate) use footnotes::{
    Definitions as ExternalFootnoteDefinitions, adopt_external as adopt_external_footnotes,
    collect_external as collect_external_footnotes,
};
pub(crate) use headings::permalink_nodes as heading_permalink_nodes;
pub(crate) use math::accessible_math_nodes;
pub(crate) fn media_cleanup_evidence(
    dom: &crate::dom::Dom,
    nodes: &[crate::dom::NodeId],
) -> (Vec<bool>, Vec<Option<crate::dom::NodeId>>) {
    media::cleanup_evidence(dom, nodes)
}

pub(crate) fn selected_image_sources_for_cleanup(
    dom: &crate::dom::Dom,
    nodes: &[crate::dom::NodeId],
) -> Vec<Option<Box<str>>> {
    images::analyze(dom, nodes, None).sources
}
pub(crate) use stats::DocumentStats;
pub(crate) fn semantic_source_is_protected(
    dom: &crate::dom::Dom,
    node: crate::dom::NodeId,
) -> bool {
    if !has_source_recognizer_gate(dom, node) {
        return false;
    }
    callouts::is_source_evidence(dom, node)
        || footnotes::is_source_evidence(dom, node)
        || math::is_source_evidence(dom, node)
}

/// Returns true when a source node may carry meaning that the plain-prose
/// compiler must preserve for semantic analysis.
pub(crate) fn semantic_source_evidence(
    dom: &crate::dom::Dom,
    node: crate::dom::NodeId,
    source_evidence: Option<&SourceEvidence>,
) -> bool {
    // Most source nodes cannot carry any of the semantic annotations below.
    // Avoid running the recognizers for ordinary prose wrappers. This is a hot
    // path for the plain-prose compiler and keeps the recognizer itself cheap
    // for callers that scan a complete fragment.
    if !has_source_recognizer_gate(dom, node) {
        return false;
    }

    if let Some(source_evidence) = source_evidence {
        return source_evidence.is_semantic_source(node)
            || source_evidence.callout_candidate(node)
            || source_evidence.footnote_candidate(node)
            || [
                crate::dom::AttrName::DataCallout,
                crate::dom::AttrName::DataFootnote,
                crate::dom::AttrName::DataFootnoteRef,
                crate::dom::AttrName::DataFootnotes,
                crate::dom::AttrName::DataMath,
            ]
            .into_iter()
            .any(|attribute| dom.attr(node, attribute).is_some());
    }

    semantic_source_is_protected(dom, node)
        || callouts::class_is_semantic_evidence(dom, node)
        || footnotes::class_is_semantic_evidence(dom, node)
        || footnotes::has_possible_footnote_evidence(dom, node)
        || [
            crate::dom::AttrName::DataCallout,
            crate::dom::AttrName::DataFootnote,
            crate::dom::AttrName::DataFootnoteRef,
            crate::dom::AttrName::DataFootnotes,
            crate::dom::AttrName::DataMath,
        ]
        .into_iter()
        .any(|attribute| dom.attr(node, attribute).is_some())
        || dom
            .attr_by_local_name(node, "data-type")
            .is_some_and(|value| {
                matches!(
                    value.to_ascii_lowercase().as_str(),
                    "footnote" | "noteref" | "footnotes"
                )
            })
}

fn has_source_recognizer_gate(dom: &crate::dom::Dom, node: crate::dom::NodeId) -> bool {
    let Some(tag) = dom.tag(node) else {
        return false;
    };
    let has_class_or_id = dom.attr(node, crate::dom::AttrName::Class).is_some()
        || dom.attr(node, crate::dom::AttrName::Id).is_some();
    let has_role = dom.attr(node, crate::dom::AttrName::Role).is_some();
    let has_semantic_data = [
        "data-latex",
        "data-tex",
        "data-math",
        "data-formula",
        "data-type",
        "data-callout",
        "data-footnote",
        "data-footnote-ref",
        "data-footnotes",
    ]
    .into_iter()
    .any(|name| dom.attr_by_local_name(node, name).is_some())
        || dom.attr(node, crate::dom::AttrName::DataCallout).is_some()
        || dom.attr(node, crate::dom::AttrName::DataFootnote).is_some()
        || dom
            .attr(node, crate::dom::AttrName::DataFootnoteRef)
            .is_some()
        || dom
            .attr(node, crate::dom::AttrName::DataFootnotes)
            .is_some()
        || dom.attr(node, crate::dom::AttrName::DataMath).is_some();
    let has_semantic_tag = matches!(
        tag,
        crate::dom::Tag::A
            | crate::dom::Tag::Img
            | crate::dom::Tag::Label
            | crate::dom::Tag::Math
            | crate::dom::Tag::Script
    ) || dom
        .qual_name(node)
        .is_some_and(|name| name.local.as_ref().eq_ignore_ascii_case("mjx-container"));
    has_class_or_id || has_role || has_semantic_data || has_semantic_tag
}

pub(crate) use tables::repeated_listing_start;
pub(crate) use uri::{DestinationKind, safe_destination};

pub(crate) fn semantic_normalization_counts(
    dom: &crate::dom::Dom,
    root: crate::dom::NodeId,
) -> (usize, usize, usize) {
    let nodes: Vec<_> = std::iter::once(root).chain(dom.descendants(root)).collect();
    semantic_normalization_counts_for_nodes(dom, root, &nodes)
}

pub(crate) fn semantic_normalization_counts_for_nodes(
    dom: &crate::dom::Dom,
    root: crate::dom::NodeId,
    nodes: &[crate::dom::NodeId],
) -> (usize, usize, usize) {
    let footnotes = footnotes::FootnoteAnalysis::analyze(dom, root);
    let math = math::MathAnalysis::analyze(dom, nodes);
    let mut references = 0;
    let mut definitions = 0;
    let mut expressions = 0;
    for &node in nodes {
        references += usize::from(footnotes.reference(node).is_some());
        definitions += usize::from(footnotes.definition(node).is_some());
        expressions += usize::from(math.value(node).is_some());
    }
    (references, definitions, expressions)
}

pub(crate) fn table_normalization_counts(
    dom: &crate::dom::Dom,
    root: crate::dom::NodeId,
) -> (usize, usize) {
    let nodes: Vec<_> = std::iter::once(root).chain(dom.descendants(root)).collect();
    let analysis = tables::TableAnalysis::analyze(dom, &nodes);
    (analysis.flattened_count(), analysis.semantic_table_count())
}

use std::fmt;
use std::sync::OnceLock;

/// An index into a [`Document`] semantic tape.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct DocumentNodeId(u32);

impl DocumentNodeId {
    pub(crate) fn index(self) -> usize {
        self.0 as usize
    }
}

/// The structured semantic content extracted from one page.
pub(crate) struct Document {
    /// Sequential semantic operations. Container operations are paired with a
    /// close operation; leaf operations stand alone.
    ops: Vec<EventOp>,
    /// The operation index of each node's closing operation, or the node's own
    /// index for a leaf. This compatibility index keeps legacy internal
    /// consumers constant-time while renderers migrate to direct tape scans.
    ends: Vec<u32>,
    roots: Vec<DocumentNodeId>,
    /// Canonical semantic prose and inline-code storage.
    text: String,
    text_refs: Vec<TextRef>,
    code_blocks: Vec<CodeBlock>,
    links: Vec<Link>,
    images: Vec<Image>,
    lists: Vec<List>,
    tables: Vec<Table>,
    table_cells: Vec<TableCell>,
    callouts: Vec<Callout>,
    task_markers: Vec<TaskMarker>,
    math_values: Vec<MathValue>,
    media: Vec<Media>,
    footnotes: Vec<FootnoteRecord>,
    node_count: usize,
    output_capacity_hint: usize,
    stats: OnceLock<DocumentStats>,
}

impl Document {
    /// Returns measurements derived from the semantic document.
    pub fn stats(&self) -> DocumentStats {
        *self
            .stats
            .get_or_init(|| stats::compute_document_stats(self))
    }

    /// Returns the normalized text for the complete document.
    pub fn text(&self) -> String {
        if let Some(stats) = self.stats.get() {
            return stats::render_document_text(self, stats.text_length);
        }

        let (text, stats) =
            stats::walk_text(self, false, false, Some(self.output_capacity_hint), true);
        let _ = self.stats.set(stats);
        text.unwrap_or_default()
    }

    /// Returns the number of characters in [`Self::text`].
    pub fn text_length(&self) -> usize {
        self.stats().text_length
    }

    /// Returns the number of words in [`Self::text`].
    pub fn word_count(&self) -> usize {
        self.stats().word_count
    }

    /// Returns the number of characters contributed by link content.
    pub fn link_text_length(&self) -> usize {
        self.stats().link_text_length
    }

    /// Returns the fraction of normalized text contributed by links.
    pub fn link_density(&self) -> f64 {
        self.stats().link_density
    }

    /// Returns the number of semantic paragraphs.
    pub fn paragraph_count(&self) -> usize {
        self.stats().paragraph_count
    }

    /// Returns the number of semantic headings.
    pub fn heading_count(&self) -> usize {
        self.stats().heading_count
    }

    /// Returns the number of semantic list items.
    pub fn list_item_count(&self) -> usize {
        self.stats().list_item_count
    }

    /// Returns the number of semantic code blocks.
    pub fn code_block_count(&self) -> usize {
        self.stats().code_block_count
    }

    /// Returns the number of semantic data tables.
    pub fn table_count(&self) -> usize {
        self.stats().table_count
    }

    /// Returns the number of semantic figures.
    pub fn figure_count(&self) -> usize {
        self.stats().figure_count
    }

    /// Returns the number of semantic images.
    pub fn image_count(&self) -> usize {
        self.stats().image_count
    }

    /// Returns the number of footnote references.
    pub fn footnote_reference_count(&self) -> usize {
        self.stats().footnote_reference_count
    }

    /// Returns the number of footnote definitions.
    pub fn footnote_definition_count(&self) -> usize {
        self.stats().footnote_definition_count
    }

    /// Returns the number of math expressions.
    pub fn math_count(&self) -> usize {
        self.stats().math_count
    }

    pub(crate) fn footnote_label(&self, id: FootnoteId) -> Option<&str> {
        self.footnote_record(id)
            .map(|definition| definition.label.as_ref())
    }

    pub(crate) fn root_ids(
        &self,
    ) -> impl ExactSizeIterator<Item = DocumentNodeId> + DoubleEndedIterator + '_ {
        self.roots.iter().copied()
    }

    pub(crate) fn node(&self, id: DocumentNodeId) -> Option<DocumentNode<'_>> {
        let operation = self.ops.get(id.index())?;
        (!operation.is_close()).then_some(DocumentNode { document: self, id })
    }

    pub(crate) fn child_ids(&self, parent: DocumentNodeId) -> Children<'_> {
        Children {
            document: self,
            next: self.first_child(parent),
        }
    }

    pub(crate) fn first_child(&self, parent: DocumentNodeId) -> Option<DocumentNodeId> {
        let child = DocumentNodeId(parent.0.checked_add(1)?);
        self.node(child)
            .filter(|_| child.0 < self.node_end(parent))
            .map(|_| child)
    }

    pub(crate) fn next_sibling(&self, node: DocumentNodeId) -> Option<DocumentNodeId> {
        let next = DocumentNodeId(self.node_end(node).checked_add(1)?);
        self.node(next).map(|_| next)
    }

    fn node_end(&self, id: DocumentNodeId) -> u32 {
        self.ends.get(id.index()).copied().unwrap_or(id.0)
    }

    fn kind_ref(&self, id: DocumentNodeId) -> NodeKindView<'_> {
        let operation = &self.ops[id.index()];
        let payload = operation.payload as usize;
        let text = self
            .text_refs
            .get(payload)
            .and_then(|reference| self.text.get(reference.range()));
        match operation.kind() {
            OperationKind::Paragraph => NodeKindView::Paragraph,
            OperationKind::BlockGroup => NodeKindView::BlockGroup,
            OperationKind::Heading => NodeKindView::Heading {
                level: operation.aux as u8,
            },
            OperationKind::BlockQuote => NodeKindView::BlockQuote,
            OperationKind::CodeBlock => self
                .code_blocks
                .get(payload)
                .map_or(NodeKindView::Invalid, NodeKindView::CodeBlock),
            OperationKind::List => self
                .lists
                .get(payload)
                .map_or(NodeKindView::Invalid, NodeKindView::List),
            OperationKind::ListItem => NodeKindView::ListItem,
            OperationKind::Table => self
                .tables
                .get(payload)
                .map_or(NodeKindView::Invalid, NodeKindView::Table),
            OperationKind::TableCaption => NodeKindView::TableCaption,
            OperationKind::TableRow => NodeKindView::TableRow,
            OperationKind::TableCell => self
                .table_cells
                .get(payload)
                .map_or(NodeKindView::Invalid, NodeKindView::TableCell),
            OperationKind::Figure => NodeKindView::Figure,
            OperationKind::Figcaption => NodeKindView::Figcaption,
            OperationKind::Details => NodeKindView::Details,
            OperationKind::Summary => NodeKindView::Summary,
            OperationKind::ThematicBreak => NodeKindView::ThematicBreak,
            OperationKind::DefinitionList => NodeKindView::DefinitionList,
            OperationKind::DefinitionTerm => NodeKindView::DefinitionTerm,
            OperationKind::DefinitionDescription => NodeKindView::DefinitionDescription,
            OperationKind::Callout => self
                .callouts
                .get(payload)
                .map_or(NodeKindView::Invalid, NodeKindView::Callout),
            OperationKind::FootnoteDefinition => {
                NodeKindView::FootnoteDefinition(FootnoteId(operation.payload))
            }
            OperationKind::Text => text.map_or(NodeKindView::Invalid, NodeKindView::Text),
            OperationKind::Emphasis => NodeKindView::Emphasis,
            OperationKind::Strong => NodeKindView::Strong,
            OperationKind::Strikethrough => NodeKindView::Strikethrough,
            OperationKind::InlineCode => {
                text.map_or(NodeKindView::Invalid, NodeKindView::InlineCode)
            }
            OperationKind::Link => self
                .links
                .get(payload)
                .map_or(NodeKindView::Invalid, NodeKindView::Link),
            OperationKind::Image => self
                .images
                .get(payload)
                .map_or(NodeKindView::Invalid, NodeKindView::Image),
            OperationKind::HardBreak => NodeKindView::HardBreak,
            OperationKind::FootnoteReference => {
                NodeKindView::FootnoteReference(FootnoteId(operation.payload))
            }
            OperationKind::TaskMarker => self
                .task_markers
                .get(payload)
                .map_or(NodeKindView::Invalid, NodeKindView::TaskMarker),
            OperationKind::InlineMath => self
                .math_values
                .get(payload)
                .map_or(NodeKindView::Invalid, NodeKindView::InlineMath),
            OperationKind::DisplayMath => self
                .math_values
                .get(payload)
                .map_or(NodeKindView::Invalid, NodeKindView::DisplayMath),
            OperationKind::Media => self
                .media
                .get(payload)
                .map_or(NodeKindView::Invalid, NodeKindView::Media),
        }
    }

    pub(crate) fn footnote_record(&self, id: FootnoteId) -> Option<&FootnoteRecord> {
        self.footnotes
            .get(id.index())
            .filter(|definition| definition.id == id)
    }

    pub(crate) fn len(&self) -> usize {
        self.node_count
    }

    /// Returns the number of top-level semantic items for benchmark reporting.
    pub(crate) fn root_count(&self) -> usize {
        self.roots.len()
    }

    /// Returns bytes owned by semantic string payloads for benchmark reporting.
    pub(crate) fn semantic_string_bytes(&self) -> usize {
        std::iter::once(self.text.capacity())
            .chain(self.code_blocks.iter().flat_map(|value| {
                std::iter::once(value.text.len()).chain(value.language.iter().map(|s| s.len()))
            }))
            .chain(self.links.iter().flat_map(|value| {
                std::iter::once(value.destination.len()).chain(value.title.iter().map(|s| s.len()))
            }))
            .chain(self.images.iter().flat_map(|value| {
                [value.source.len(), value.alt.len()]
                    .into_iter()
                    .chain(value.title.iter().map(|s| s.len()))
            }))
            .chain(
                self.callouts
                    .iter()
                    .filter_map(|value| value.title.as_ref().map(|s| s.len())),
            )
            .chain(
                self.task_markers
                    .iter()
                    .filter_map(|value| value.fallback_label.as_ref().map(|s| s.len())),
            )
            .chain(self.math_values.iter().flat_map(|value| {
                std::iter::once(value.source.len())
                    .chain(value.fallback_text.iter().map(|s| s.len()))
            }))
            .chain(self.media.iter().flat_map(|value| {
                std::iter::once(value.source.len()).chain(value.title.iter().map(|s| s.len()))
            }))
            .chain(self.footnotes.iter().map(|value| value.label.len()))
            .sum()
    }

    /// Returns owned semantic string values for benchmark reporting.
    pub(crate) fn semantic_string_value_count(&self) -> usize {
        usize::from(!self.text.is_empty())
            + self
                .code_blocks
                .iter()
                .map(|v| 1 + usize::from(v.language.is_some()))
                .sum::<usize>()
            + self
                .links
                .iter()
                .map(|v| 1 + usize::from(v.title.is_some()))
                .sum::<usize>()
            + self
                .images
                .iter()
                .map(|v| 2 + usize::from(v.title.is_some()))
                .sum::<usize>()
            + self.callouts.iter().filter(|v| v.title.is_some()).count()
            + self
                .task_markers
                .iter()
                .filter(|v| v.fallback_label.is_some())
                .count()
            + self
                .math_values
                .iter()
                .map(|v| 1 + usize::from(v.fallback_text.is_some()))
                .sum::<usize>()
            + self
                .media
                .iter()
                .map(|v| 1 + usize::from(v.title.is_some()))
                .sum::<usize>()
            + self.footnotes.len()
    }

    pub(crate) fn retained_bytes_estimate(&self) -> usize {
        let vector_bytes = self.ops.capacity() * std::mem::size_of::<EventOp>()
            + self.ends.capacity() * std::mem::size_of::<u32>()
            + self.roots.capacity() * std::mem::size_of::<DocumentNodeId>()
            + self.text_refs.capacity() * std::mem::size_of::<TextRef>()
            + self.code_blocks.capacity() * std::mem::size_of::<CodeBlock>()
            + self.links.capacity() * std::mem::size_of::<Link>()
            + self.images.capacity() * std::mem::size_of::<Image>()
            + self.lists.capacity() * std::mem::size_of::<List>()
            + self.tables.capacity() * std::mem::size_of::<Table>()
            + self.table_cells.capacity() * std::mem::size_of::<TableCell>()
            + self.callouts.capacity() * std::mem::size_of::<Callout>()
            + self.task_markers.capacity() * std::mem::size_of::<TaskMarker>()
            + self.math_values.capacity() * std::mem::size_of::<MathValue>()
            + self.media.capacity() * std::mem::size_of::<Media>()
            + self.footnotes.capacity() * std::mem::size_of::<FootnoteRecord>();
        std::mem::size_of::<Self>()
            .saturating_add(vector_bytes)
            .saturating_add(self.semantic_string_bytes())
    }

    pub(crate) fn operation_capacity(&self) -> usize {
        self.ops.capacity()
    }

    pub(crate) fn end_capacity(&self) -> usize {
        self.ends.capacity()
    }

    pub(crate) fn output_capacity_hint(&self) -> usize {
        self.output_capacity_hint
    }

    #[cfg(test)]
    pub(crate) fn stats_initialized(&self) -> bool {
        self.stats.get().is_some()
    }

    pub(crate) fn node_slot_size() -> usize {
        std::mem::size_of::<EventOp>()
    }

    pub(crate) fn validate(&self) -> Result<(), ValidationError> {
        validate::validate(self)
    }

    #[cfg(test)]
    pub(crate) fn debug_tree(&self) -> String {
        debug_tree(self)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
pub(crate) struct EventOp {
    payload: u32,
    aux: u16,
    opcode: u8,
    flags: u8,
}

const OP_CLOSE: u8 = 0x80;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
enum OperationKind {
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

impl OperationKind {
    fn from_opcode(opcode: u8) -> Option<Self> {
        Some(match opcode & !OP_CLOSE {
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
            _ => return None,
        })
    }

    fn is_container(self) -> bool {
        !matches!(
            self,
            Self::CodeBlock
                | Self::Text
                | Self::InlineCode
                | Self::ThematicBreak
                | Self::Image
                | Self::HardBreak
                | Self::FootnoteReference
                | Self::TaskMarker
                | Self::InlineMath
                | Self::DisplayMath
                | Self::Media
        )
    }
}

impl EventOp {
    fn kind(self) -> OperationKind {
        OperationKind::from_opcode(self.opcode & !OP_CLOSE)
            .expect("semantic tape contains an unknown operation")
    }

    fn is_close(self) -> bool {
        self.opcode & OP_CLOSE != 0
    }
}

pub(crate) struct DocumentNode<'a> {
    document: &'a Document,
    id: DocumentNodeId,
}

impl<'a> DocumentNode<'a> {
    pub(crate) fn kind(&self) -> NodeKindView<'a> {
        self.document.kind_ref(self.id)
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
        self.next = self.document.next_sibling(id);
        Some(id)
    }
}

/// The semantic meaning of a document node.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
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
    Text(TextRef),
    Emphasis,
    Strong,
    Strikethrough,
    InlineCode(TextRef),
    Link(Link),
    Image(Image),
    HardBreak,
    FootnoteReference(FootnoteId),
    TaskMarker(TaskMarker),
    InlineMath(MathValue),
    DisplayMath(MathValue),
    Media(Media),
}

impl NodeKind {
    pub(crate) fn heading_level(&self) -> Option<u8> {
        match self {
            Self::Heading { level } => Some(*level),
            _ => None,
        }
    }

    fn output_capacity_hint(&self) -> usize {
        match self {
            Self::Text(_) | Self::InlineCode(_) => 0,
            Self::CodeBlock(code) => code
                .text
                .len()
                .saturating_add(optional_boxed_str_len(&code.language)),
            Self::Link(link) => link
                .destination
                .len()
                .saturating_add(optional_boxed_str_len(&link.title)),
            Self::Image(image) => image
                .source
                .len()
                .saturating_add(image.alt.len())
                .saturating_add(optional_boxed_str_len(&image.title)),
            Self::Callout(callout) => optional_boxed_str_len(&callout.title),
            Self::TaskMarker(marker) => optional_boxed_str_len(&marker.fallback_label),
            Self::InlineMath(math) | Self::DisplayMath(math) => math
                .source
                .len()
                .saturating_add(optional_boxed_str_len(&math.fallback_text)),
            Self::Media(media) => media
                .source
                .len()
                .saturating_add(optional_boxed_str_len(&media.title)),
            _ => 0,
        }
    }

    fn retained_value_bytes(&self) -> usize {
        match self {
            Self::Text(_) | Self::InlineCode(_) => 0,
            Self::CodeBlock(code) => {
                optional_boxed_str_len(&code.language).saturating_add(code.text.len())
            }
            Self::Link(link) => link
                .destination
                .len()
                .saturating_add(optional_boxed_str_len(&link.title)),
            Self::Image(image) => image
                .source
                .len()
                .saturating_add(image.alt.len())
                .saturating_add(optional_boxed_str_len(&image.title)),
            Self::Callout(callout) => optional_boxed_str_len(&callout.title),
            Self::TaskMarker(marker) => optional_boxed_str_len(&marker.fallback_label),
            Self::InlineMath(math) | Self::DisplayMath(math) => math
                .source
                .len()
                .saturating_add(optional_boxed_str_len(&math.fallback_text)),
            Self::Media(media) => media
                .source
                .len()
                .saturating_add(optional_boxed_str_len(&media.title)),
            _ => 0,
        }
    }

    fn semantic_string_value_count(&self) -> usize {
        match self {
            Self::Text(_) | Self::InlineCode(_) => 1,
            Self::CodeBlock(code) => 1 + usize::from(code.language.is_some()),
            Self::Link(link) => 1 + usize::from(link.title.is_some()),
            Self::Image(image) => 2 + usize::from(image.title.is_some()),
            Self::Callout(callout) => usize::from(callout.title.is_some()),
            Self::TaskMarker(marker) => usize::from(marker.fallback_label.is_some()),
            Self::InlineMath(math) | Self::DisplayMath(math) => {
                1 + usize::from(math.fallback_text.is_some())
            }
            Self::Media(media) => 1 + usize::from(media.title.is_some()),
            _ => 0,
        }
    }
}

/// A borrowed view of one compact tape operation. Payload values live in
/// type-specific side tables and are borrowed only for the duration of a
/// view.
#[derive(Clone, Copy, Debug)]
pub(crate) enum NodeKindView<'a> {
    Paragraph,
    BlockGroup,
    Heading { level: u8 },
    BlockQuote,
    CodeBlock(&'a CodeBlock),
    List(&'a List),
    ListItem,
    Table(&'a Table),
    TableCaption,
    TableRow,
    TableCell(&'a TableCell),
    Figure,
    Figcaption,
    Details,
    Summary,
    ThematicBreak,
    DefinitionList,
    DefinitionTerm,
    DefinitionDescription,
    Callout(&'a Callout),
    FootnoteDefinition(FootnoteId),
    Text(&'a str),
    Emphasis,
    Strong,
    Strikethrough,
    InlineCode(&'a str),
    Link(&'a Link),
    Image(&'a Image),
    HardBreak,
    FootnoteReference(FootnoteId),
    TaskMarker(&'a TaskMarker),
    InlineMath(&'a MathValue),
    DisplayMath(&'a MathValue),
    Media(&'a Media),
    Invalid,
}

impl NodeKindView<'_> {
    pub(crate) fn heading_level(self) -> Option<u8> {
        match self {
            Self::Heading { level } => Some(level),
            _ => None,
        }
    }
}

fn optional_boxed_str_len(value: &Option<Box<str>>) -> usize {
    value.as_deref().map_or(0, str::len)
}

/// A range into the document-owned canonical text arena.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TextRef {
    start: u32,
    len: u32,
}

impl TextRef {
    fn new(start: usize, len: usize) -> Result<Self, BuildError> {
        let end = start.checked_add(len).ok_or(BuildError::CapacityExceeded)?;
        u32::try_from(end).map_err(|_| BuildError::CapacityExceeded)?;
        Ok(Self {
            start: u32::try_from(start).map_err(|_| BuildError::CapacityExceeded)?,
            len: u32::try_from(len).map_err(|_| BuildError::CapacityExceeded)?,
        })
    }

    fn range(self) -> std::ops::Range<usize> {
        let start = self.start as usize;
        start..start + self.len as usize
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CodeBlock {
    pub(crate) language: Option<Box<str>>,
    pub(crate) text: Box<str>,
}

impl CodeBlock {
    /// Returns the detected language, when available.
    pub fn language(&self) -> Option<&str> {
        self.language.as_deref()
    }

    /// Returns the normalized preformatted code.
    pub fn text(&self) -> &str {
        &self.text
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Link {
    pub(crate) destination: Box<str>,
    pub(crate) title: Option<Box<str>>,
    // Retain whether the source link was fragment-only for weighted link density.
    // The destination may be resolved to an absolute URL during compilation.
    pub(crate) fragment_only: bool,
}

impl Link {
    /// Returns the resolved, policy-validated destination.
    pub fn destination(&self) -> &str {
        &self.destination
    }

    /// Returns the optional link title.
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Image {
    pub(crate) source: Box<str>,
    pub(crate) alt: Box<str>,
    pub(crate) title: Option<Box<str>>,
    pub(crate) width: Option<u32>,
    pub(crate) height: Option<u32>,
}

impl Image {
    /// Returns the selected, policy-validated image source.
    pub fn source(&self) -> &str {
        &self.source
    }
    /// Returns the image alternative text.
    pub fn alt(&self) -> &str {
        &self.alt
    }
    /// Returns the optional image title.
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }
    /// Returns the declared width in pixels, when available.
    pub fn width(&self) -> Option<u32> {
        self.width
    }
    /// Returns the declared height in pixels, when available.
    pub fn height(&self) -> Option<u32> {
        self.height
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct List {
    pub(crate) kind: ListKind,
    pub(crate) start: Option<i64>,
}

impl List {
    /// Returns whether this list is ordered or unordered.
    pub fn kind(&self) -> ListKind {
        self.kind
    }
    /// Returns the first ordinal for an ordered list.
    pub fn start(&self) -> Option<i64> {
        self.start
    }
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

impl Table {
    /// Returns the exact column count when the semantic grid has one.
    pub fn column_count(&self) -> Option<u32> {
        self.column_count
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TableCell {
    pub(crate) header: bool,
    pub(crate) colspan: u32,
    pub(crate) rowspan: u32,
    pub(crate) alignment: Option<TableAlignment>,
}

impl TableCell {
    /// Returns true for a semantic header cell.
    pub fn is_header(&self) -> bool {
        self.header
    }
    /// Returns the column span. This value is at least one.
    pub fn colspan(&self) -> u32 {
        self.colspan
    }
    /// Returns the row span. This value is at least one.
    pub fn rowspan(&self) -> u32 {
        self.rowspan
    }
    /// Returns the normalized cell alignment.
    pub fn alignment(&self) -> Option<TableAlignment> {
        self.alignment
    }
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

impl Callout {
    /// Returns the normalized callout category.
    pub fn kind(&self) -> CalloutKind {
        self.kind
    }
    /// Returns the optional callout title.
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }
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

impl TaskMarker {
    /// Returns true when the task is checked.
    pub fn is_checked(&self) -> bool {
        self.checked
    }
    /// Returns text used by formats without task-list syntax.
    pub fn fallback_label(&self) -> Option<&str> {
        self.fallback_label.as_deref()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct MathValue {
    pub(crate) source: Box<str>,
    pub(crate) format: MathFormat,
    pub(crate) fallback_text: Option<Box<str>>,
}

impl MathValue {
    /// Returns the recovered math source.
    pub fn source(&self) -> &str {
        &self.source
    }
    /// Returns the source format.
    pub fn format(&self) -> MathFormat {
        self.format
    }
    /// Returns an accessible text fallback, when available.
    pub fn fallback_text(&self) -> Option<&str> {
        self.fallback_text.as_deref()
    }
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

impl Media {
    /// Returns the media category.
    pub fn kind(&self) -> MediaKind {
        self.kind
    }
    /// Returns the selected, policy-validated source.
    pub fn source(&self) -> &str {
        &self.source
    }
    /// Returns the optional media title.
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }
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
    pub(crate) fn index(self) -> usize {
        self.0 as usize
    }

    pub(crate) fn from_index(index: usize) -> Result<Self, BuildError> {
        u32::try_from(index)
            .map(Self)
            .map_err(|_| BuildError::CapacityExceeded)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct FootnoteRecord {
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
        assert_eq!(document.root_ids().count(), 7);
        let definition = document.footnote_record(footnote).unwrap();
        assert_eq!(definition.id, footnote);
        assert!(matches!(
            document.node(definition.node).map(|node| node.kind()),
            Some(NodeKindView::FootnoteDefinition(id)) if id == footnote
        ));
        assert_eq!(document.footnotes.len(), 1);
    }

    #[test]
    fn event_tape_uses_compact_headers_and_explicit_closes() {
        assert_eq!(std::mem::size_of::<EventOp>(), 8);

        let mut builder = DocumentBuilder::with_capacity(2);
        let paragraph = builder.append(None, NodeKind::Paragraph).unwrap();
        builder.append_prose(Some(paragraph), "tape").unwrap();
        let document = builder.finish();

        assert_eq!(document.len(), 2);
        assert_eq!(document.ops.len(), 3);
        assert_eq!(document.ops[0].opcode & OP_CLOSE, 0);
        assert_ne!(document.ops[2].opcode & OP_CLOSE, 0);
        assert_eq!(document.ends[0], 2);
        assert_eq!(
            document.first_child(DocumentNodeId(0)),
            Some(DocumentNodeId(1))
        );
        assert_eq!(document.next_sibling(DocumentNodeId(0)), None);
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
        let mut stack: Vec<_> = document.root_ids().collect();
        while let Some(node) = stack.pop() {
            count += 1;
            stack.extend(document.child_ids(node));
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
        builder.append(Some(first), NodeKind::Strong).unwrap();
        let mut cycle = builder.finish();
        cycle.ends[first.index()] = u32::MAX;
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
        write_kind(&mut output, node.kind());
        output.push('\n');
        let children: Vec<_> = document.child_ids(id).collect();
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
fn write_kind(output: &mut String, kind: NodeKindView<'_>) {
    use NodeKindView as NodeKind;
    use std::fmt::Write as _;
    match kind {
        NodeKind::Text(value) => write!(output, "Text({:?})", value).unwrap(),
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
        NodeKind::InlineCode(value) => write!(output, "InlineCode({:?})", value).unwrap(),
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
