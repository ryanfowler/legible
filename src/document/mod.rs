//! Private semantic representation and lowering support.
//!
//! This module is an implementation detail. Keep its storage and traversal APIs
//! private so the representation can change without changing Legible's public
//! output contract.

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
mod sparse;
pub(crate) mod stats;
mod tables;
mod text;
mod uri;
#[cfg(any(test, feature = "fuzzing", debug_assertions))]
mod validate;

/// Semantic content flags stored on opening and closing tape operations.
pub(crate) const HAS_VISIBLE_TEXT: u8 = 1 << 0;
pub(crate) const HAS_VISIBLE_IMAGE: u8 = 1 << 1;

pub(crate) use builder::{BuildCapacityPlan, BuildError, SemanticTapeBuilder};
pub(crate) use code::{
    count_blocks as source_code_block_count,
    is_multiline_orphan_with_evidence as is_multiline_code_with_evidence,
    multiline_content as code_multiline_content,
};
#[allow(unused_imports)]
pub(crate) use compiler::{
    CompileContext, compile_document, compile_document_owned_with_optional_source_facts,
    compile_document_owned_with_optional_source_facts_and_evidence,
    compile_document_owned_with_optional_source_facts_and_evidence_and_retained_nodes,
    compile_document_with_optional_source_facts,
    compile_document_with_optional_source_facts_and_evidence,
    compile_document_with_optional_source_facts_and_evidence_and_retained_nodes,
    complex_storage_metrics_for_benchmark,
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
    let analysis = images::analyze(dom, nodes, None);
    analysis.into_sources(dom.len())
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
use tendril::StrTendril;

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
    /// The operation index of each opening operation's close operation, or the
    /// opening operation's own index for a leaf.
    ends: Vec<u32>,
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
    compile_stats: stats::CompileStats,
    text_stats: OnceLock<stats::TextStats>,
}

impl Document {
    /// Returns measurements derived from the semantic document.
    pub fn stats(&self) -> DocumentStats {
        let text_stats = *self
            .text_stats
            .get_or_init(|| stats::walk_text(self, false, false, None, true).1);
        stats::combine(self.compile_stats, text_stats)
    }

    /// Returns the normalized text for the complete document.
    pub fn text(&self) -> String {
        if let Some(stats) = self.text_stats.get() {
            return stats::render_document_text(self, stats.text_length);
        }

        let (text, text_stats) =
            stats::walk_text(self, false, false, Some(self.output_capacity_hint), true);
        let _ = self.text_stats.set(text_stats);
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
        self.compile_stats.paragraph_count
    }

    /// Returns the number of semantic headings.
    pub fn heading_count(&self) -> usize {
        self.compile_stats.heading_count
    }

    /// Returns the number of semantic list items.
    pub fn list_item_count(&self) -> usize {
        self.compile_stats.list_item_count
    }

    /// Returns the number of semantic code blocks.
    pub fn code_block_count(&self) -> usize {
        self.compile_stats.code_block_count
    }

    /// Returns the number of semantic data tables.
    pub fn table_count(&self) -> usize {
        self.compile_stats.table_count
    }

    /// Returns the number of semantic figures.
    pub fn figure_count(&self) -> usize {
        self.compile_stats.figure_count
    }

    /// Returns the number of semantic images.
    pub fn image_count(&self) -> usize {
        self.compile_stats.image_count
    }

    /// Returns the number of footnote references.
    pub fn footnote_reference_count(&self) -> usize {
        self.compile_stats.footnote_reference_count
    }

    /// Returns the number of footnote definitions.
    pub fn footnote_definition_count(&self) -> usize {
        self.compile_stats.footnote_definition_count
    }

    /// Returns the number of math expressions.
    pub fn math_count(&self) -> usize {
        self.compile_stats.math_count
    }

    pub(crate) fn structured_block_count(&self) -> usize {
        self.compile_stats.structured_block_count
    }

    pub(crate) fn has_contextual_structure(&self) -> bool {
        self.compile_stats.has_contextual_structure
    }

    pub(crate) fn footnote_label(&self, id: FootnoteId) -> Option<&str> {
        self.footnote_record(id)
            .map(|definition| definition.label.as_ref())
    }

    /// Returns the retained semantic operations in source order.
    ///
    /// Renderers use this view for their sequential tape interpreters. Keep
    /// the operation payload accessors small so the representation remains
    /// private to the crate.
    pub(crate) fn operations(&self) -> &[EventOp] {
        &self.ops
    }

    pub(crate) fn operation_kind(&self, index: usize) -> Option<OperationKind> {
        self.ops.get(index).map(|operation| operation.kind())
    }

    pub(crate) fn operation_view(&self, index: usize) -> Option<SemanticItemView<'_>> {
        let operation = self.ops.get(index)?;
        if operation.is_close() {
            return None;
        }
        Some(self.kind_ref(index))
    }

    pub(crate) fn operation_end(&self, index: usize) -> usize {
        self.ends.get(index).copied().unwrap_or(index as u32) as usize
    }

    pub(crate) fn operation_opening_index(&self, operation: EventOp) -> usize {
        operation.payload() as usize
    }

    fn kind_ref(&self, index: usize) -> SemanticItemView<'_> {
        let operation = &self.ops[index];
        let payload = operation.payload as usize;
        let text = self
            .text_refs
            .get(payload)
            .and_then(|reference| self.text.get(reference.range()));
        match operation.kind() {
            OperationKind::Paragraph => SemanticItemView::Paragraph,
            OperationKind::BlockGroup => SemanticItemView::BlockGroup,
            OperationKind::Heading => SemanticItemView::Heading {
                level: operation.aux as u8,
            },
            OperationKind::BlockQuote => SemanticItemView::BlockQuote,
            OperationKind::CodeBlock => self
                .code_blocks
                .get(payload)
                .map_or(SemanticItemView::Invalid, SemanticItemView::CodeBlock),
            OperationKind::List => self
                .lists
                .get(payload)
                .map_or(SemanticItemView::Invalid, SemanticItemView::List),
            OperationKind::ListItem => SemanticItemView::ListItem,
            OperationKind::Table => self
                .tables
                .get(payload)
                .map_or(SemanticItemView::Invalid, SemanticItemView::Table),
            OperationKind::TableCaption => SemanticItemView::TableCaption,
            OperationKind::TableRow => SemanticItemView::TableRow,
            OperationKind::TableCell => self
                .table_cells
                .get(payload)
                .map_or(SemanticItemView::Invalid, SemanticItemView::TableCell),
            OperationKind::Figure => SemanticItemView::Figure,
            OperationKind::Figcaption => SemanticItemView::Figcaption,
            OperationKind::Details => SemanticItemView::Details,
            OperationKind::Summary => SemanticItemView::Summary,
            OperationKind::ThematicBreak => SemanticItemView::ThematicBreak,
            OperationKind::DefinitionList => SemanticItemView::DefinitionList,
            OperationKind::DefinitionTerm => SemanticItemView::DefinitionTerm,
            OperationKind::DefinitionDescription => SemanticItemView::DefinitionDescription,
            OperationKind::Callout => self
                .callouts
                .get(payload)
                .map_or(SemanticItemView::Invalid, SemanticItemView::Callout),
            OperationKind::FootnoteDefinition => {
                SemanticItemView::FootnoteDefinition(FootnoteId(operation.payload))
            }
            OperationKind::Text => text.map_or(SemanticItemView::Invalid, SemanticItemView::Text),
            OperationKind::Emphasis => SemanticItemView::Emphasis,
            OperationKind::Strong => SemanticItemView::Strong,
            OperationKind::Strikethrough => SemanticItemView::Strikethrough,
            OperationKind::InlineCode => {
                text.map_or(SemanticItemView::Invalid, SemanticItemView::InlineCode)
            }
            OperationKind::Link => self
                .links
                .get(payload)
                .map_or(SemanticItemView::Invalid, SemanticItemView::Link),
            OperationKind::Image => self
                .images
                .get(payload)
                .map_or(SemanticItemView::Invalid, SemanticItemView::Image),
            OperationKind::HardBreak => SemanticItemView::HardBreak,
            OperationKind::FootnoteReference => {
                SemanticItemView::FootnoteReference(FootnoteId(operation.payload))
            }
            OperationKind::TaskMarker => self
                .task_markers
                .get(payload)
                .map_or(SemanticItemView::Invalid, SemanticItemView::TaskMarker),
            OperationKind::InlineMath => self
                .math_values
                .get(payload)
                .map_or(SemanticItemView::Invalid, SemanticItemView::InlineMath),
            OperationKind::DisplayMath => self
                .math_values
                .get(payload)
                .map_or(SemanticItemView::Invalid, SemanticItemView::DisplayMath),
            OperationKind::Media => self
                .media
                .get(payload)
                .map_or(SemanticItemView::Invalid, SemanticItemView::Media),
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
    #[allow(dead_code)]
    pub(crate) fn root_count(&self) -> usize {
        let mut depth = 0usize;
        let mut roots = 0usize;
        for operation in &self.ops {
            if operation.is_close() {
                depth = depth.saturating_sub(1);
            } else {
                if depth == 0 {
                    roots += 1;
                }
                if operation.kind().is_container() {
                    depth += 1;
                }
            }
        }
        roots
    }

    /// Returns bytes owned by semantic string payloads for benchmark reporting.
    pub(crate) fn semantic_string_bytes(&self) -> usize {
        std::iter::once(self.text.capacity())
            .chain(self.code_blocks.iter().flat_map(|value| {
                std::iter::once(value.text_len()).chain(value.language.iter().map(|s| s.len()))
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
    #[allow(dead_code)]
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

    #[allow(dead_code)]
    pub(crate) fn operation_capacity(&self) -> usize {
        self.ops.capacity()
    }

    #[allow(dead_code)]
    pub(crate) fn end_capacity(&self) -> usize {
        self.ends.capacity()
    }

    pub(crate) fn output_capacity_hint(&self) -> usize {
        self.output_capacity_hint
    }

    #[cfg(test)]
    pub(crate) fn stats_initialized(&self) -> bool {
        self.text_stats.get().is_some()
    }

    #[allow(dead_code)]
    pub(crate) fn node_slot_size() -> usize {
        std::mem::size_of::<EventOp>()
    }

    #[cfg(any(test, feature = "fuzzing", debug_assertions))]
    pub(crate) fn validate(&self) -> Result<(), ValidationError> {
        validate::validate(self)
    }

    #[cfg(test)]
    pub(crate) fn debug_tape(&self) -> String {
        debug_tape(self)
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
pub(crate) enum OperationKind {
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

    pub(crate) fn is_container(self) -> bool {
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
    pub(crate) fn kind(self) -> OperationKind {
        OperationKind::from_opcode(self.opcode & !OP_CLOSE)
            .expect("semantic tape contains an unknown operation")
    }

    pub(crate) fn is_close(self) -> bool {
        self.opcode & OP_CLOSE != 0
    }

    pub(crate) fn payload(self) -> u32 {
        self.payload
    }

    pub(crate) fn flags(self) -> u8 {
        self.flags
    }
}

/// The semantic meaning of a document node.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub(crate) enum SemanticKind {
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
    Emphasis,
    Strong,
    Strikethrough,
    Link(Link),
    Image(Image),
    HardBreak,
    FootnoteReference(FootnoteId),
    TaskMarker(TaskMarker),
    InlineMath(MathValue),
    DisplayMath(MathValue),
    Media(Media),
}

impl SemanticKind {
    fn output_capacity_hint(&self) -> usize {
        match self {
            Self::CodeBlock(code) => code
                .text_len()
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
}

/// A borrowed view of one compact tape operation. Payload values live in
/// type-specific side tables and are borrowed only for the duration of a
/// view.
#[derive(Clone, Copy, Debug)]
pub(crate) enum SemanticItemView<'a> {
    Paragraph,
    BlockGroup,
    Heading {
        level: u8,
    },
    BlockQuote,
    CodeBlock(&'a CodeBlock),
    List(&'a List),
    ListItem,
    #[allow(dead_code)]
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
    #[allow(dead_code)]
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
pub(crate) enum RawCodeText {
    Owned(Box<str>),
    Source(StrTendril),
}

impl From<&str> for RawCodeText {
    fn from(value: &str) -> Self {
        Self::Owned(value.into())
    }
}

impl From<String> for RawCodeText {
    fn from(value: String) -> Self {
        Self::Owned(value.into_boxed_str())
    }
}

impl From<StrTendril> for RawCodeText {
    fn from(value: StrTendril) -> Self {
        Self::Source(value)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CodeBlock {
    pub(crate) language: Option<Box<str>>,
    pub(crate) text: RawCodeText,
}

impl CodeBlock {
    /// Returns the detected language, when available.
    pub fn language(&self) -> Option<&str> {
        self.language.as_deref()
    }

    /// Returns the normalized preformatted code.
    pub fn text(&self) -> &str {
        match &self.text {
            RawCodeText::Owned(text) => text,
            RawCodeText::Source(text) => text.as_ref(),
        }
    }

    pub(crate) fn text_len(&self) -> usize {
        self.text().len()
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TableCell {
    pub(crate) header: bool,
    pub(crate) colspan: u32,
    pub(crate) rowspan: u32,
    pub(crate) alignment: Option<TableAlignment>,
}

impl TableCell {
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
    /// Returns an accessible text fallback, when available.
    pub fn fallback_text(&self) -> Option<&str> {
        self.fallback_text.as_deref()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MathFormat {
    Tex,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Media {
    pub(crate) kind: MediaKind,
    pub(crate) source: Box<str>,
    pub(crate) title: Option<Box<str>>,
}

impl Media {
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
    #[cfg(any(test, feature = "fuzzing", debug_assertions))]
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
fn debug_tape(document: &Document) -> String {
    let mut output = String::new();
    let mut depth = 0usize;
    for (index, operation) in document.operations().iter().copied().enumerate() {
        if operation.is_close() {
            depth = depth.saturating_sub(1);
            continue;
        }
        output.push_str(&"  ".repeat(depth));
        if let Some(kind) = document.operation_view(index) {
            write_item(&mut output, kind);
        }
        output.push('\n');
        if operation.kind().is_container() {
            depth += 1;
        }
    }
    output
}

#[cfg(test)]
fn write_item(output: &mut String, kind: SemanticItemView<'_>) {
    use SemanticItemView as Item;
    use std::fmt::Write as _;
    match kind {
        Item::Text(value) => write!(output, "Text({:?})", value).unwrap(),
        Item::Heading { level } => write!(output, "Heading(level={level})").unwrap(),
        Item::CodeBlock(code) => write!(
            output,
            "CodeBlock(language={:?}, text={:?})",
            code.language,
            code.text()
        )
        .unwrap(),
        Item::List(list) => {
            write!(output, "List(kind={:?}, start={:?})", list.kind, list.start).unwrap()
        }
        Item::Table(table) => write!(output, "Table(columns={:?})", table.column_count).unwrap(),
        Item::TableCell(cell) => write!(
            output,
            "TableCell(header={}, colspan={}, rowspan={}, alignment={:?})",
            cell.header, cell.colspan, cell.rowspan, cell.alignment
        )
        .unwrap(),
        Item::Link(link) => write!(
            output,
            "Link(destination={:?}, title={:?})",
            link.destination, link.title
        )
        .unwrap(),
        Item::Image(image) => write!(
            output,
            "Image(source={:?}, alt={:?}, title={:?}, width={:?}, height={:?})",
            image.source, image.alt, image.title, image.width, image.height
        )
        .unwrap(),
        Item::InlineCode(value) => write!(output, "InlineCode({:?})", value).unwrap(),
        Item::FootnoteReference(id) => write!(output, "FootnoteReference({})", id.0).unwrap(),
        Item::FootnoteDefinition(id) => write!(output, "FootnoteDefinition({})", id.0).unwrap(),
        Item::TaskMarker(marker) => write!(
            output,
            "TaskMarker(checked={}, fallback={:?})",
            marker.checked, marker.fallback_label
        )
        .unwrap(),
        Item::InlineMath(value) | Item::DisplayMath(value) => write!(
            output,
            "{}(source={:?}, format={:?}, fallback={:?})",
            if matches!(kind, Item::InlineMath(_)) {
                "InlineMath"
            } else {
                "DisplayMath"
            },
            value.source,
            value.format,
            value.fallback_text
        )
        .unwrap(),
        Item::Callout(callout) => write!(
            output,
            "Callout(kind={:?}, title={:?})",
            callout.kind, callout.title
        )
        .unwrap(),
        Item::Media(media) => write!(
            output,
            "Media(kind={:?}, source={:?}, title={:?})",
            media.kind, media.source, media.title
        )
        .unwrap(),
        other => write!(output, "{other:?}").unwrap(),
    }
}
