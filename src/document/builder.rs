use std::collections::HashMap;
use std::mem::size_of;
use std::sync::OnceLock;

use super::stats::{CompileStats, has_visible_inline_text};
use super::{
    Callout, CodeBlock, Document, DocumentNodeId, EventOp, FootnoteId, FootnoteRecord, Image, Link,
    List, MathValue, Media, OP_CLOSE, OperationKind, SemanticKind, Table, TableCell, TaskMarker,
    TextRef,
};
use thiserror::Error;

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub(crate) enum BuildError {
    #[error("document exceeds semantic node capacity")]
    CapacityExceeded,
    #[error("semantic parent does not exist")]
    InvalidParent,
    #[error("footnote has more than one definition")]
    DuplicateFootnoteDefinition,
}

/// Conservative capacity estimates for one semantic lowering pass.
///
/// The plan describes semantic shape, not source size. A source node can be
/// transparent, while a semantic container also emits a close operation.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct BuildCapacityPlan {
    pub(crate) semantic_nodes: usize,
    pub(crate) containers: usize,
    pub(crate) max_depth: usize,
    pub(crate) text_bytes: usize,
    pub(crate) text_refs: usize,
    pub(crate) code_blocks: usize,
    pub(crate) links: usize,
    pub(crate) images: usize,
    pub(crate) lists: usize,
    pub(crate) tables: usize,
    pub(crate) table_cells: usize,
    pub(crate) callouts: usize,
    pub(crate) task_markers: usize,
    pub(crate) math_values: usize,
    pub(crate) media: usize,
    pub(crate) footnotes: usize,
}

impl BuildCapacityPlan {
    #[cfg(test)]
    pub(crate) fn from_source_node_count(source_nodes: usize) -> Self {
        Self {
            semantic_nodes: source_nodes,
            containers: source_nodes,
            max_depth: source_nodes.min(64),
            ..Self::default()
        }
    }

    fn operation_capacity(self) -> usize {
        self.semantic_nodes.saturating_add(self.containers)
    }

    #[cfg(feature = "bench-instrumentation")]
    fn payload_capacity_bytes(self) -> usize {
        self.text_refs
            .saturating_mul(size_of::<TextRef>())
            .saturating_add(self.code_blocks.saturating_mul(size_of::<CodeBlock>()))
            .saturating_add(self.links.saturating_mul(size_of::<Link>()))
            .saturating_add(self.images.saturating_mul(size_of::<Image>()))
            .saturating_add(self.lists.saturating_mul(size_of::<List>()))
            .saturating_add(self.tables.saturating_mul(size_of::<Table>()))
            .saturating_add(self.table_cells.saturating_mul(size_of::<TableCell>()))
            .saturating_add(self.callouts.saturating_mul(size_of::<Callout>()))
            .saturating_add(self.task_markers.saturating_mul(size_of::<TaskMarker>()))
            .saturating_add(self.math_values.saturating_mul(size_of::<MathValue>()))
            .saturating_add(self.media.saturating_mul(size_of::<Media>()))
    }

    #[cfg(feature = "bench-instrumentation")]
    fn requested_bytes(self) -> usize {
        self.operation_capacity()
            .saturating_mul(size_of::<EventOp>() + size_of::<u32>())
            .saturating_add(self.max_depth.saturating_mul(size_of::<OpenFrame>()))
            .saturating_add(self.text_bytes)
            .saturating_add(self.payload_capacity_bytes())
            .saturating_add(self.footnotes.saturating_mul(size_of::<FootnoteRecord>()))
            .saturating_add(
                self.footnotes
                    .saturating_mul(size_of::<(FootnoteId, usize)>()),
            )
    }
}

pub(crate) struct SemanticTapeBuilder {
    ops: Vec<EventOp>,
    ends: Vec<u32>,
    open: Vec<OpenFrame>,
    last_root_child: Option<DocumentNodeId>,
    text: String,
    payloads: PayloadTables,
    pending_root_space: bool,
    footnotes: Vec<FootnoteRecord>,
    footnote_index: HashMap<FootnoteId, usize>,
    node_count: usize,
    output_capacity_hint: usize,
    compile_stats: CompileStats,
    #[cfg(feature = "bench-instrumentation")]
    plan: BuildCapacityPlan,
    #[cfg(feature = "bench-instrumentation")]
    metrics_recorded: bool,
    metrics: BuilderMetrics,
}

struct OpenFrame {
    node: DocumentNodeId,
    flags: u8,
    last_child: Option<DocumentNodeId>,
    pending_space: bool,
}

#[cfg(feature = "bench-instrumentation")]
#[derive(Default)]
struct BuilderMetrics {
    reallocations: usize,
    max_open_depth: usize,
    shrink_bytes: usize,
}

#[cfg(not(feature = "bench-instrumentation"))]
#[derive(Default)]
struct BuilderMetrics;

impl SemanticTapeBuilder {
    #[cfg(test)]
    pub(crate) fn with_capacity(capacity: usize) -> Self {
        Self::with_plan(BuildCapacityPlan::from_source_node_count(capacity))
    }

    pub(crate) fn with_plan(plan: BuildCapacityPlan) -> Self {
        Self {
            ops: Vec::with_capacity(plan.operation_capacity()),
            ends: Vec::with_capacity(plan.operation_capacity()),
            open: Vec::with_capacity(plan.max_depth),
            last_root_child: None,
            text: String::with_capacity(plan.text_bytes),
            payloads: PayloadTables::with_plan(plan),
            pending_root_space: false,
            footnotes: Vec::with_capacity(plan.footnotes),
            footnote_index: HashMap::with_capacity(plan.footnotes),
            node_count: 0,
            output_capacity_hint: 0,
            compile_stats: CompileStats::default(),
            #[cfg(feature = "bench-instrumentation")]
            plan,
            #[cfg(feature = "bench-instrumentation")]
            metrics_recorded: false,
            metrics: BuilderMetrics::default(),
        }
    }

    /// Records an attempt that stops before a document can be finished.
    pub(crate) fn record_abandoned_attempt(&mut self) {
        #[cfg(feature = "bench-instrumentation")]
        {
            let peak_bytes = self.capacity_bytes();
            self.record_capacity_metrics(peak_bytes);
        }
    }

    /// Emits one semantic item to the private tape.
    ///
    /// `DocumentNodeId` is a temporary opener handle for source-order close
    /// state. The builder stores no semantic tree links.
    pub(crate) fn emit(
        &mut self,
        parent: Option<DocumentNodeId>,
        kind: SemanticKind,
    ) -> Result<DocumentNodeId, BuildError> {
        if !self.valid_parent(parent) {
            return Err(BuildError::InvalidParent);
        }
        if self.take_pending_space(parent) && is_inline_sibling(&kind) {
            self.append_pending_separator(parent)?;
        }
        self.compile_stats.record_kind(&kind);
        self.append_kind(parent, kind)
    }

    pub(crate) fn append_inline_code(
        &mut self,
        parent: Option<DocumentNodeId>,
        value: &str,
    ) -> Result<DocumentNodeId, BuildError> {
        if !self.valid_parent(parent) {
            return Err(BuildError::InvalidParent);
        }
        if self.take_pending_space(parent) {
            self.append_pending_separator(parent)?;
        }
        let value = self.append_text(value)?;
        let payload = push_payload(&mut self.payloads.text_refs, value, &mut self.metrics);
        self.append_operation(parent, OperationKind::InlineCode, payload, 0)
    }

    pub(crate) fn append_prose(
        &mut self,
        parent: Option<DocumentNodeId>,
        value: &str,
    ) -> Result<Option<DocumentNodeId>, BuildError> {
        if !self.valid_parent(parent) {
            return Err(BuildError::InvalidParent);
        }
        let normalized = super::text::normalize_prose_fragment(value);
        self.append_normalized_prose_fragment(parent, normalized)
    }

    pub(crate) fn append_normalized_prose(
        &mut self,
        parent: Option<DocumentNodeId>,
        value: &str,
    ) -> Result<Option<DocumentNodeId>, BuildError> {
        if !self.valid_parent(parent) {
            return Err(BuildError::InvalidParent);
        }
        let normalized = super::text::normalize_prose_fragment(value);
        self.append_normalized_prose_fragment(parent, normalized)
    }

    pub(super) fn append_normalized_prose_fragment(
        &mut self,
        parent: Option<DocumentNodeId>,
        value: super::text::NormalizedFragment<'_>,
    ) -> Result<Option<DocumentNodeId>, BuildError> {
        if !self.valid_parent(parent) {
            return Err(BuildError::InvalidParent);
        }
        self.append_normalized_prose_value(parent, value)
    }

    pub(crate) fn close(&mut self, node: DocumentNodeId) -> Result<(), BuildError> {
        let Some(operation) = self.ops.get(node.index()).copied() else {
            return Err(BuildError::InvalidParent);
        };
        if operation.is_close()
            || !operation.kind().is_container()
            || self.ends[node.index()] != 0
            || self.open.last().is_none_or(|frame| frame.node != node)
        {
            return Err(BuildError::InvalidParent);
        }
        let close = u32::try_from(self.ops.len()).map_err(|_| BuildError::CapacityExceeded)?;
        let frame = self.open.pop().ok_or(BuildError::InvalidParent)?;
        let flags = frame.flags;
        self.ops[node.index()].flags = flags;
        push_tracked(
            &mut self.ops,
            EventOp {
                payload: node.0,
                aux: 0,
                opcode: operation.opcode | OP_CLOSE,
                flags,
            },
            &mut self.metrics,
        );
        push_tracked(&mut self.ends, 0, &mut self.metrics);
        self.ends[node.index()] = close;
        if let Some(parent) = self.open.last_mut() {
            parent.flags |= flags;
        }
        Ok(())
    }

    pub(crate) fn define_footnote(
        &mut self,
        id: FootnoteId,
        label: &str,
        node: DocumentNodeId,
    ) -> Result<(), BuildError> {
        if self.footnote_index.contains_key(&id) {
            return Err(BuildError::DuplicateFootnoteDefinition);
        }
        if node.index() >= self.ops.len() || self.ops[node.index()].is_close() {
            return Err(BuildError::InvalidParent);
        }
        let index = self.footnotes.len();
        let previous_capacity = self.footnote_index.capacity();
        self.footnote_index.insert(id, index);
        note_reallocation(
            &mut self.metrics,
            previous_capacity,
            self.footnote_index.capacity(),
        );
        self.output_capacity_hint = self.output_capacity_hint.saturating_add(label.len());
        push_tracked(
            &mut self.footnotes,
            FootnoteRecord {
                id,
                label: label.into(),
                node,
            },
            &mut self.metrics,
        );
        Ok(())
    }

    pub(crate) fn is_block_group(&self, node: DocumentNodeId) -> bool {
        self.operation_kind(node) == Some(OperationKind::BlockGroup)
    }

    pub(crate) fn is_list(&self, node: DocumentNodeId) -> bool {
        self.operation_kind(node) == Some(OperationKind::List)
    }

    pub(crate) fn is_list_item(&self, node: DocumentNodeId) -> bool {
        self.operation_kind(node) == Some(OperationKind::ListItem)
    }

    pub(crate) fn is_container(&self, node: DocumentNodeId) -> bool {
        self.operation_kind(node)
            .is_some_and(OperationKind::is_container)
    }

    pub(crate) fn is_redundant_formatting(
        &self,
        parent: Option<DocumentNodeId>,
        kind: &SemanticKind,
    ) -> bool {
        let Some(parent) = parent else {
            return false;
        };
        let Some(parent) = self.operation_kind(parent) else {
            return false;
        };
        matches!(
            (kind, parent),
            (SemanticKind::Strong, OperationKind::Strong)
                | (SemanticKind::Emphasis, OperationKind::Emphasis)
                | (SemanticKind::Strikethrough, OperationKind::Strikethrough)
        )
    }

    pub(crate) fn table_mut(&mut self, node: DocumentNodeId) -> Option<&mut Table> {
        let operation = self.ops.get(node.index()).copied()?;
        (operation.kind() == OperationKind::Table)
            .then(|| self.payloads.tables.get_mut(operation.payload as usize))?
    }

    pub(crate) fn finish(mut self) -> Result<Document, BuildError> {
        if !self.open.is_empty() {
            return Err(BuildError::InvalidParent);
        }
        #[cfg(feature = "bench-instrumentation")]
        let peak_capacity_bytes = self.capacity_bytes();
        self.compile_stats.semantic_text_bytes = self.text.len();
        compact_excess_capacity(&mut self.ops, &mut self.metrics);
        compact_excess_capacity(&mut self.ends, &mut self.metrics);
        compact_excess_capacity(&mut self.payloads.text_refs, &mut self.metrics);
        compact_excess_capacity(&mut self.payloads.code_blocks, &mut self.metrics);
        compact_excess_capacity(&mut self.payloads.links, &mut self.metrics);
        compact_excess_capacity(&mut self.payloads.images, &mut self.metrics);
        compact_excess_capacity(&mut self.payloads.lists, &mut self.metrics);
        compact_excess_capacity(&mut self.payloads.tables, &mut self.metrics);
        compact_excess_capacity(&mut self.payloads.table_cells, &mut self.metrics);
        compact_excess_capacity(&mut self.payloads.callouts, &mut self.metrics);
        compact_excess_capacity(&mut self.payloads.task_markers, &mut self.metrics);
        compact_excess_capacity(&mut self.payloads.math_values, &mut self.metrics);
        compact_excess_capacity(&mut self.payloads.media, &mut self.metrics);
        compact_excess_capacity(&mut self.footnotes, &mut self.metrics);
        compact_excess_string(&mut self.text, &mut self.metrics);

        #[cfg(feature = "bench-instrumentation")]
        self.record_capacity_metrics(peak_capacity_bytes);

        let mut footnotes = self.footnotes;
        if footnotes
            .iter()
            .all(|definition| definition.id.index() < footnotes.len())
        {
            let mut indexed: Vec<Option<FootnoteRecord>> = std::iter::repeat_with(|| None)
                .take(footnotes.len())
                .collect();
            for definition in footnotes.drain(..) {
                let index = definition.id.index();
                indexed[index] = Some(definition);
            }
            footnotes = indexed.into_iter().flatten().collect();
        }

        Ok(Document {
            ops: self.ops,
            ends: self.ends,
            text: self.text,
            text_refs: self.payloads.text_refs,
            code_blocks: self.payloads.code_blocks,
            links: self.payloads.links,
            images: self.payloads.images,
            lists: self.payloads.lists,
            tables: self.payloads.tables,
            table_cells: self.payloads.table_cells,
            callouts: self.payloads.callouts,
            task_markers: self.payloads.task_markers,
            math_values: self.payloads.math_values,
            media: self.payloads.media,
            footnotes,
            node_count: self.node_count,
            output_capacity_hint: self.output_capacity_hint,
            compile_stats: self.compile_stats,
            text_stats: OnceLock::new(),
        })
    }

    fn append_kind(
        &mut self,
        parent: Option<DocumentNodeId>,
        kind: SemanticKind,
    ) -> Result<DocumentNodeId, BuildError> {
        self.output_capacity_hint = self
            .output_capacity_hint
            .saturating_add(kind.output_capacity_hint());
        let (operation, payload, aux) = match kind {
            SemanticKind::Paragraph => (OperationKind::Paragraph, 0, 0),
            SemanticKind::BlockGroup => (OperationKind::BlockGroup, 0, 0),
            SemanticKind::Heading { level } => (OperationKind::Heading, 0, u16::from(level)),
            SemanticKind::BlockQuote => (OperationKind::BlockQuote, 0, 0),
            SemanticKind::CodeBlock(value) => (
                OperationKind::CodeBlock,
                push_payload(&mut self.payloads.code_blocks, value, &mut self.metrics),
                0,
            ),
            SemanticKind::List(value) => (
                OperationKind::List,
                push_payload(&mut self.payloads.lists, value, &mut self.metrics),
                0,
            ),
            SemanticKind::ListItem => (OperationKind::ListItem, 0, 0),
            SemanticKind::Table(value) => (
                OperationKind::Table,
                push_payload(&mut self.payloads.tables, value, &mut self.metrics),
                0,
            ),
            SemanticKind::TableCaption => (OperationKind::TableCaption, 0, 0),
            SemanticKind::TableRow => (OperationKind::TableRow, 0, 0),
            SemanticKind::TableCell(value) => (
                OperationKind::TableCell,
                push_payload(&mut self.payloads.table_cells, value, &mut self.metrics),
                0,
            ),
            SemanticKind::Figure => (OperationKind::Figure, 0, 0),
            SemanticKind::Figcaption => (OperationKind::Figcaption, 0, 0),
            SemanticKind::Details => (OperationKind::Details, 0, 0),
            SemanticKind::Summary => (OperationKind::Summary, 0, 0),
            SemanticKind::ThematicBreak => (OperationKind::ThematicBreak, 0, 0),
            SemanticKind::DefinitionList => (OperationKind::DefinitionList, 0, 0),
            SemanticKind::DefinitionTerm => (OperationKind::DefinitionTerm, 0, 0),
            SemanticKind::DefinitionDescription => (OperationKind::DefinitionDescription, 0, 0),
            SemanticKind::Callout(value) => (
                OperationKind::Callout,
                push_payload(&mut self.payloads.callouts, value, &mut self.metrics),
                0,
            ),
            SemanticKind::FootnoteDefinition(id) => (OperationKind::FootnoteDefinition, id.0, 0),
            SemanticKind::Emphasis => (OperationKind::Emphasis, 0, 0),
            SemanticKind::Strong => (OperationKind::Strong, 0, 0),
            SemanticKind::Strikethrough => (OperationKind::Strikethrough, 0, 0),
            SemanticKind::Link(value) => (
                OperationKind::Link,
                push_payload(&mut self.payloads.links, value, &mut self.metrics),
                0,
            ),
            SemanticKind::Image(value) => (
                OperationKind::Image,
                push_payload(&mut self.payloads.images, value, &mut self.metrics),
                0,
            ),
            SemanticKind::HardBreak => (OperationKind::HardBreak, 0, 0),
            SemanticKind::FootnoteReference(id) => (OperationKind::FootnoteReference, id.0, 0),
            SemanticKind::TaskMarker(value) => (
                OperationKind::TaskMarker,
                push_payload(&mut self.payloads.task_markers, value, &mut self.metrics),
                0,
            ),
            SemanticKind::InlineMath(value) => (
                OperationKind::InlineMath,
                push_payload(&mut self.payloads.math_values, value, &mut self.metrics),
                0,
            ),
            SemanticKind::DisplayMath(value) => (
                OperationKind::DisplayMath,
                push_payload(&mut self.payloads.math_values, value, &mut self.metrics),
                0,
            ),
            SemanticKind::Media(value) => (
                OperationKind::Media,
                push_payload(&mut self.payloads.media, value, &mut self.metrics),
                0,
            ),
        };
        self.append_operation(parent, operation, payload, aux)
    }

    fn append_operation(
        &mut self,
        parent: Option<DocumentNodeId>,
        operation: OperationKind,
        payload: u32,
        aux: u16,
    ) -> Result<DocumentNodeId, BuildError> {
        self.append_operation_with_flags(parent, operation, payload, aux, None)
    }

    fn append_operation_with_flags(
        &mut self,
        parent: Option<DocumentNodeId>,
        operation: OperationKind,
        payload: u32,
        aux: u16,
        known_flags: Option<u8>,
    ) -> Result<DocumentNodeId, BuildError> {
        let index = u32::try_from(self.ops.len()).map_err(|_| BuildError::CapacityExceeded)?;
        let id = DocumentNodeId(index);
        let flags = known_flags.unwrap_or_else(|| self.operation_visibility(operation, payload));
        push_tracked(
            &mut self.ops,
            EventOp {
                payload,
                aux,
                opcode: operation as u8,
                flags,
            },
            &mut self.metrics,
        );
        push_tracked(
            &mut self.ends,
            if operation.is_container() { 0 } else { index },
            &mut self.metrics,
        );
        self.node_count += 1;
        if let Some(parent) = parent {
            let current = self
                .open
                .last_mut()
                .filter(|frame| frame.node == parent)
                .ok_or(BuildError::InvalidParent)?;
            current.last_child = Some(id);
            current.flags |= flags;
        } else {
            self.last_root_child = Some(id);
        }
        if operation.is_container() {
            push_tracked(
                &mut self.open,
                OpenFrame {
                    node: id,
                    flags,
                    last_child: None,
                    pending_space: false,
                },
                &mut self.metrics,
            );
            note_open_depth(&mut self.metrics, self.open.len());
        }
        Ok(id)
    }

    fn operation_visibility(&self, operation: OperationKind, payload: u32) -> u8 {
        match operation {
            OperationKind::Text | OperationKind::InlineCode => self
                .payloads
                .text_refs
                .get(payload as usize)
                .map(|value| self.text_slice(*value))
                .filter(|value| has_visible_inline_text(value))
                .map(|_| super::HAS_VISIBLE_TEXT)
                .unwrap_or_default(),
            OperationKind::CodeBlock => self
                .payloads
                .code_blocks
                .get(payload as usize)
                .filter(|value| has_visible_inline_text(value.text()))
                .map(|_| super::HAS_VISIBLE_TEXT)
                .unwrap_or_default(),
            OperationKind::Image => self
                .payloads
                .images
                .get(payload as usize)
                .filter(|value| has_visible_inline_text(&value.alt))
                .map(|_| super::HAS_VISIBLE_IMAGE)
                .unwrap_or_default(),
            OperationKind::TaskMarker => self
                .payloads
                .task_markers
                .get(payload as usize)
                .and_then(|value| value.fallback_label.as_deref())
                .filter(|value| has_visible_inline_text(value))
                .map(|_| super::HAS_VISIBLE_TEXT)
                .unwrap_or_default(),
            OperationKind::InlineMath | OperationKind::DisplayMath | OperationKind::Media => {
                super::HAS_VISIBLE_TEXT
            }
            _ => 0,
        }
    }

    fn append_normalized_prose_value(
        &mut self,
        parent: Option<DocumentNodeId>,
        value: super::text::NormalizedFragment<'_>,
    ) -> Result<Option<DocumentNodeId>, BuildError> {
        if value.is_empty() {
            return Ok(None);
        }
        let value_ref = value.as_ref();
        let append = value.append;
        let previous = self.previous_child(parent);
        if append.first_non_space.is_none() {
            if previous.is_some() {
                self.set_pending_space(parent, true);
            }
            return Ok(None);
        }
        let needs_leading_space = self.take_pending_space(parent) && !append.starts_with_space;
        if let Some(previous) = previous
            && self.ops[previous.index()].kind() == OperationKind::Text
        {
            let payload = self.ops[previous.index()].payload as usize;
            let existing = self.payloads.text_refs[payload];
            let existing_value = self.text_slice(existing);
            let leading_space = needs_leading_space && !existing_value.ends_with(' ');
            let value = if existing_value.ends_with(' ') && append.starts_with_space {
                &value_ref[1..]
            } else {
                value_ref
            };
            if existing.range().end == self.text.len() {
                let updated = self.extend_text(existing, leading_space, value)?;
                self.payloads.text_refs[payload] = updated;
                let flags = if append.has_visible_text {
                    super::HAS_VISIBLE_TEXT
                } else {
                    0
                };
                self.ops[previous.index()].flags |= flags;
                if let Some(parent) = self.open.last_mut() {
                    parent.flags |= flags;
                }
                return Ok(Some(previous));
            }
            if !leading_space && value.is_empty() {
                return Ok(Some(previous));
            }
            let value = self.append_text_with_prefix(value, leading_space)?;
            let payload = push_payload(&mut self.payloads.text_refs, value, &mut self.metrics);
            return self
                .append_operation_with_flags(
                    parent,
                    OperationKind::Text,
                    payload,
                    0,
                    Some(if append.has_visible_text {
                        super::HAS_VISIBLE_TEXT
                    } else {
                        0
                    }),
                )
                .map(Some);
        }
        let value = self.append_text_with_prefix(value_ref, needs_leading_space)?;
        let payload = push_payload(&mut self.payloads.text_refs, value, &mut self.metrics);
        self.append_operation_with_flags(
            parent,
            OperationKind::Text,
            payload,
            0,
            Some(if append.has_visible_text {
                super::HAS_VISIBLE_TEXT
            } else {
                0
            }),
        )
        .map(Some)
    }

    fn valid_parent(&self, parent: Option<DocumentNodeId>) -> bool {
        match parent {
            Some(id) => self.open.last().is_some_and(|frame| frame.node == id),
            None => self.open.is_empty(),
        }
    }

    fn operation_kind(&self, node: DocumentNodeId) -> Option<OperationKind> {
        self.ops
            .get(node.index())
            .filter(|operation| !operation.is_close())
            .map(|operation| operation.kind())
    }

    fn previous_child(&self, parent: Option<DocumentNodeId>) -> Option<DocumentNodeId> {
        match parent {
            Some(id) => self
                .open
                .last()
                .filter(|frame| frame.node == id)
                .and_then(|frame| frame.last_child),
            None => self.last_root_child,
        }
    }

    fn take_pending_space(&mut self, parent: Option<DocumentNodeId>) -> bool {
        match parent {
            Some(id) => self
                .open
                .last_mut()
                .filter(|frame| frame.node == id)
                .is_some_and(|frame| std::mem::take(&mut frame.pending_space)),
            None => std::mem::take(&mut self.pending_root_space),
        }
    }

    fn set_pending_space(&mut self, parent: Option<DocumentNodeId>, value: bool) {
        match parent {
            Some(id) => {
                if let Some(frame) = self.open.last_mut().filter(|frame| frame.node == id) {
                    frame.pending_space = value;
                }
            }
            None => self.pending_root_space = value,
        }
    }

    fn append_pending_separator(
        &mut self,
        parent: Option<DocumentNodeId>,
    ) -> Result<(), BuildError> {
        let previous = self.previous_child(parent);
        if let Some(previous) = previous
            && self.ops[previous.index()].kind() == OperationKind::Text
        {
            let payload = self.ops[previous.index()].payload as usize;
            let existing = self.payloads.text_refs[payload];
            if self.text_slice(existing).ends_with(' ') {
                return Ok(());
            }
            if existing.range().end == self.text.len() {
                let updated = self.extend_text(existing, false, " ")?;
                self.payloads.text_refs[payload] = updated;
                return Ok(());
            }
        }
        let separator = self.append_text(" ")?;
        let payload = push_payload(&mut self.payloads.text_refs, separator, &mut self.metrics);
        self.append_operation(parent, OperationKind::Text, payload, 0)?;
        Ok(())
    }

    fn append_text(&mut self, value: &str) -> Result<TextRef, BuildError> {
        self.append_text_with_prefix(value, false)
    }

    fn append_text_with_prefix(
        &mut self,
        value: &str,
        leading_space: bool,
    ) -> Result<TextRef, BuildError> {
        let start = self.text.len();
        let length = value
            .len()
            .checked_add(usize::from(leading_space))
            .ok_or(BuildError::CapacityExceeded)?;
        let reference = TextRef::new(start, length)?;
        if leading_space {
            push_str_tracked(&mut self.text, " ", &mut self.metrics);
        }
        push_str_tracked(&mut self.text, value, &mut self.metrics);
        self.output_capacity_hint = self.output_capacity_hint.saturating_add(length);
        self.compile_stats.add_semantic_text_bytes(length);
        Ok(reference)
    }

    fn extend_text(
        &mut self,
        existing: TextRef,
        leading_space: bool,
        value: &str,
    ) -> Result<TextRef, BuildError> {
        let existing_value = self.text_slice(existing);
        let leading_space = leading_space && !existing_value.ends_with(' ');
        let value = if existing_value.ends_with(' ') && value.starts_with(' ') {
            &value[1..]
        } else {
            value
        };
        if !leading_space && value.is_empty() {
            return Ok(existing);
        }
        debug_assert_eq!(existing.range().end, self.text.len());
        let added = value.len() + usize::from(leading_space);
        let reference = TextRef::new(existing.start as usize, existing.len as usize + added)?;
        if leading_space {
            push_str_tracked(&mut self.text, " ", &mut self.metrics);
        }
        push_str_tracked(&mut self.text, value, &mut self.metrics);
        self.output_capacity_hint = self.output_capacity_hint.saturating_add(added);
        self.compile_stats.add_semantic_text_bytes(added);
        Ok(reference)
    }

    fn text_slice(&self, value: TextRef) -> &str {
        self.text
            .get(value.range())
            .expect("text reference must point into the tape text arena")
    }

    #[cfg(feature = "bench-instrumentation")]
    fn capacity_bytes(&self) -> usize {
        self.ops.capacity() * size_of::<EventOp>()
            + self.ends.capacity() * size_of::<u32>()
            + self.open.capacity() * size_of::<OpenFrame>()
            + self.text.capacity()
            + self.payloads.capacity_bytes()
            + self.footnotes.capacity() * size_of::<FootnoteRecord>()
            + self.footnote_index.capacity() * size_of::<(FootnoteId, usize)>()
    }

    #[cfg(feature = "bench-instrumentation")]
    fn record_capacity_metrics(&mut self, peak_bytes: usize) {
        if self.metrics_recorded {
            return;
        }
        crate::instrumentation::record_builder_capacities(
            crate::instrumentation::BuilderCapacityReport {
                requested_bytes: self.plan.requested_bytes(),
                final_bytes: self.capacity_bytes(),
                peak_bytes,
                reallocations: self.metrics.reallocations,
                max_open_depth: self.metrics.max_open_depth,
                shrink_bytes: self.metrics.shrink_bytes,
                ops: self.ops.capacity() * size_of::<EventOp>(),
                ends: self.ends.capacity() * size_of::<u32>(),
                open: self.open.capacity() * size_of::<OpenFrame>(),
                text: self.text.capacity(),
                payload: self.payloads.capacity_bytes(),
                footnotes: self.footnotes.capacity() * size_of::<FootnoteRecord>(),
                footnote_index: self.footnote_index.capacity() * size_of::<(FootnoteId, usize)>(),
            },
        );
        self.metrics_recorded = true;
    }
}

#[derive(Default)]
struct PayloadTables {
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
}

impl PayloadTables {
    fn with_plan(plan: BuildCapacityPlan) -> Self {
        Self {
            text_refs: Vec::with_capacity(plan.text_refs),
            code_blocks: Vec::with_capacity(plan.code_blocks),
            links: Vec::with_capacity(plan.links),
            images: Vec::with_capacity(plan.images),
            lists: Vec::with_capacity(plan.lists),
            tables: Vec::with_capacity(plan.tables),
            table_cells: Vec::with_capacity(plan.table_cells),
            callouts: Vec::with_capacity(plan.callouts),
            task_markers: Vec::with_capacity(plan.task_markers),
            math_values: Vec::with_capacity(plan.math_values),
            media: Vec::with_capacity(plan.media),
        }
    }

    #[cfg(feature = "bench-instrumentation")]
    fn capacity_bytes(&self) -> usize {
        self.text_refs.capacity() * size_of::<TextRef>()
            + self.code_blocks.capacity() * size_of::<CodeBlock>()
            + self.links.capacity() * size_of::<Link>()
            + self.images.capacity() * size_of::<Image>()
            + self.lists.capacity() * size_of::<List>()
            + self.tables.capacity() * size_of::<Table>()
            + self.table_cells.capacity() * size_of::<TableCell>()
            + self.callouts.capacity() * size_of::<Callout>()
            + self.task_markers.capacity() * size_of::<TaskMarker>()
            + self.math_values.capacity() * size_of::<MathValue>()
            + self.media.capacity() * size_of::<Media>()
    }
}

fn push_payload<T>(values: &mut Vec<T>, value: T, metrics: &mut BuilderMetrics) -> u32 {
    let index = u32::try_from(values.len()).unwrap_or(u32::MAX);
    push_tracked(values, value, metrics);
    index
}

#[inline(always)]
fn push_tracked<T>(values: &mut Vec<T>, value: T, metrics: &mut BuilderMetrics) {
    #[cfg(feature = "bench-instrumentation")]
    let previous_capacity = values.capacity();
    values.push(value);
    #[cfg(feature = "bench-instrumentation")]
    note_reallocation(metrics, previous_capacity, values.capacity());
    #[cfg(not(feature = "bench-instrumentation"))]
    let _ = metrics;
}

#[inline(always)]
fn push_str_tracked(value: &mut String, text: &str, metrics: &mut BuilderMetrics) {
    #[cfg(feature = "bench-instrumentation")]
    let previous_capacity = value.capacity();
    value.push_str(text);
    #[cfg(feature = "bench-instrumentation")]
    note_reallocation(metrics, previous_capacity, value.capacity());
    #[cfg(not(feature = "bench-instrumentation"))]
    let _ = metrics;
}

#[inline(always)]
fn note_reallocation(metrics: &mut BuilderMetrics, previous: usize, current: usize) {
    #[cfg(feature = "bench-instrumentation")]
    if current != previous {
        metrics.reallocations = metrics.reallocations.saturating_add(1);
    }
    #[cfg(not(feature = "bench-instrumentation"))]
    let _ = (metrics, previous, current);
}

#[inline(always)]
fn note_open_depth(metrics: &mut BuilderMetrics, depth: usize) {
    #[cfg(feature = "bench-instrumentation")]
    {
        metrics.max_open_depth = metrics.max_open_depth.max(depth);
    }
    #[cfg(not(feature = "bench-instrumentation"))]
    let _ = (metrics, depth);
}

#[inline(always)]
fn note_shrink(metrics: &mut BuilderMetrics, bytes: usize) {
    #[cfg(feature = "bench-instrumentation")]
    {
        metrics.shrink_bytes = metrics.shrink_bytes.saturating_add(bytes);
    }
    #[cfg(not(feature = "bench-instrumentation"))]
    let _ = (metrics, bytes);
}

/// Releases capacity only for severe, measured overestimates.
fn compact_excess_capacity<T>(values: &mut Vec<T>, metrics: &mut BuilderMetrics) {
    const MINIMUM_SAVING_BYTES: usize = 64 * 1024;

    let unused = values.capacity().saturating_sub(values.len());
    let unused_bytes = unused.saturating_mul(size_of::<T>());
    if values.capacity() > values.len().saturating_mul(4) && unused_bytes >= MINIMUM_SAVING_BYTES {
        note_shrink(metrics, values.len().saturating_mul(size_of::<T>()));
        values.shrink_to_fit();
    }
}

fn compact_excess_string(value: &mut String, metrics: &mut BuilderMetrics) {
    const MINIMUM_SAVING_BYTES: usize = 64 * 1024;

    let unused = value.capacity().saturating_sub(value.len());
    if unused >= MINIMUM_SAVING_BYTES && unused >= value.len() {
        note_shrink(metrics, value.len());
        value.shrink_to_fit();
    }
}

fn is_inline_sibling(kind: &SemanticKind) -> bool {
    matches!(
        kind,
        SemanticKind::Emphasis
            | SemanticKind::Strong
            | SemanticKind::Strikethrough
            | SemanticKind::Link(_)
            | SemanticKind::Image(_)
            | SemanticKind::HardBreak
            | SemanticKind::FootnoteReference(_)
            | SemanticKind::InlineMath(_)
            | SemanticKind::Media(_)
    )
}

#[cfg(test)]
mod tests {
    use super::super::ListKind;
    use super::*;

    #[test]
    fn capacity_plan_reserves_by_shape() {
        let plan = BuildCapacityPlan {
            semantic_nodes: 100,
            containers: 7,
            max_depth: 3,
            text_bytes: 128,
            text_refs: 5,
            code_blocks: 2,
            links: 4,
            images: 3,
            lists: 2,
            tables: 1,
            table_cells: 6,
            callouts: 1,
            task_markers: 2,
            math_values: 3,
            media: 1,
            footnotes: 2,
        };
        let builder = SemanticTapeBuilder::with_plan(plan);

        assert_eq!(builder.ops.capacity(), 107);
        assert_eq!(builder.ends.capacity(), 107);
        assert_eq!(builder.open.capacity(), 3);
        assert_eq!(builder.text.capacity(), 128);
        assert_eq!(builder.payloads.text_refs.capacity(), 5);
        assert_eq!(builder.payloads.code_blocks.capacity(), 2);
        assert_eq!(builder.payloads.links.capacity(), 4);
        assert_eq!(builder.payloads.images.capacity(), 3);
        assert_eq!(builder.payloads.lists.capacity(), 2);
        assert_eq!(builder.payloads.tables.capacity(), 1);
        assert_eq!(builder.payloads.table_cells.capacity(), 6);
        assert_eq!(builder.payloads.callouts.capacity(), 1);
        assert_eq!(builder.payloads.task_markers.capacity(), 2);
        assert_eq!(builder.payloads.math_values.capacity(), 3);
        assert_eq!(builder.payloads.media.capacity(), 1);
        assert_eq!(builder.footnotes.capacity(), 2);
        assert!(builder.footnote_index.capacity() >= 2);
    }

    #[test]
    fn underestimated_plan_grows_without_changing_output() {
        let mut builder = SemanticTapeBuilder::with_plan(BuildCapacityPlan {
            semantic_nodes: 1,
            max_depth: 0,
            ..BuildCapacityPlan::default()
        });
        let paragraph = builder
            .emit(None, SemanticKind::Paragraph)
            .expect("paragraph should fit after growth");
        builder
            .append_prose(Some(paragraph), "text")
            .expect("text should fit after growth");
        builder.close(paragraph).expect("paragraph should close");
        let document = builder.finish().expect("document should finish");

        assert_eq!(document.text(), "text");
        assert!(document.validate().is_ok());
    }

    #[test]
    fn deep_open_stack_grows_from_a_small_depth_hint() {
        const DEPTH: usize = 1_024;
        let mut builder = SemanticTapeBuilder::with_plan(BuildCapacityPlan {
            semantic_nodes: 1,
            containers: 1,
            max_depth: 1,
            ..BuildCapacityPlan::default()
        });
        let mut open = Vec::with_capacity(DEPTH);
        let mut parent = None;
        for _ in 0..DEPTH {
            let node = builder.emit(parent, SemanticKind::BlockGroup).unwrap();
            open.push(node);
            parent = Some(node);
        }
        builder.append_prose(parent, "deep").unwrap();
        for node in open.into_iter().rev() {
            builder.close(node).unwrap();
        }

        let document = builder.finish().unwrap();
        assert_eq!(document.text(), "deep");
        assert!(document.validate().is_ok());
    }

    #[test]
    fn flat_input_keeps_the_open_capacity_small() {
        const COUNT: usize = 1_024;
        let mut builder = SemanticTapeBuilder::with_plan(BuildCapacityPlan {
            semantic_nodes: COUNT,
            containers: COUNT,
            max_depth: 1,
            ..BuildCapacityPlan::default()
        });
        assert_eq!(builder.open.capacity(), 1);
        for _ in 0..COUNT {
            let paragraph = builder.emit(None, SemanticKind::Paragraph).unwrap();
            builder.append_prose(Some(paragraph), "line").unwrap();
            builder.close(paragraph).unwrap();
        }

        let document = builder.finish().unwrap();
        assert_eq!(
            document
                .debug_tape()
                .lines()
                .filter(|line| line.trim() == "Paragraph")
                .count(),
            COUNT
        );
        assert!(document.validate().is_ok());
    }

    #[test]
    fn underestimated_capacity_preserves_all_rendered_outputs() {
        fn render(plan: BuildCapacityPlan) -> (String, String, String) {
            let mut builder = SemanticTapeBuilder::with_plan(plan);
            let heading = builder
                .emit(None, SemanticKind::Heading { level: 2 })
                .unwrap();
            builder.append_prose(Some(heading), "Heading").unwrap();
            builder.close(heading).unwrap();

            let paragraph = builder.emit(None, SemanticKind::Paragraph).unwrap();
            builder.append_prose(Some(paragraph), "A ").unwrap();
            let link = builder
                .emit(
                    Some(paragraph),
                    SemanticKind::Link(Link {
                        destination: "https://example.test".into(),
                        title: None,
                        fragment_only: false,
                    }),
                )
                .unwrap();
            builder.append_prose(Some(link), "link").unwrap();
            builder.close(link).unwrap();
            builder.close(paragraph).unwrap();

            let code = builder
                .emit(
                    None,
                    SemanticKind::CodeBlock(CodeBlock {
                        language: Some("rust".into()),
                        text: "let value = 1;".into(),
                    }),
                )
                .unwrap();
            let _ = code;
            let document = builder.finish().unwrap();
            let html = crate::render::html::render_html(&document, 0);
            let markdown = crate::render::markdown::render_markdown(
                &document,
                0,
                crate::render::markdown::MarkdownConfig::default(),
            );
            (html, markdown, document.text())
        }

        let expected = render(BuildCapacityPlan {
            semantic_nodes: 32,
            containers: 8,
            max_depth: 4,
            text_bytes: 32,
            text_refs: 8,
            code_blocks: 1,
            links: 1,
            ..BuildCapacityPlan::default()
        });
        let actual = render(BuildCapacityPlan::default());
        assert_eq!(actual, expected);
    }

    #[test]
    fn underestimated_plan_grows_every_payload_table() {
        let mut builder = SemanticTapeBuilder::with_plan(BuildCapacityPlan::default());
        let footnote_id = FootnoteId::from_index(0).unwrap();
        let footnote = builder
            .emit(None, SemanticKind::FootnoteDefinition(footnote_id))
            .unwrap();
        builder.define_footnote(footnote_id, "1", footnote).unwrap();
        builder.append_prose(Some(footnote), "Footnote").unwrap();
        builder.close(footnote).unwrap();

        let root = builder.emit(None, SemanticKind::BlockGroup).unwrap();
        let paragraph = builder.emit(Some(root), SemanticKind::Paragraph).unwrap();
        builder.append_prose(Some(paragraph), "Text").unwrap();
        builder.append_inline_code(Some(paragraph), "code").unwrap();
        let link = builder
            .emit(
                Some(paragraph),
                SemanticKind::Link(Link {
                    destination: "https://example.test".into(),
                    title: None,
                    fragment_only: false,
                }),
            )
            .unwrap();
        builder.close(link).unwrap();
        builder
            .emit(
                Some(paragraph),
                SemanticKind::Image(Image {
                    source: "https://example.test/image.png".into(),
                    alt: "Image".into(),
                    title: None,
                    width: None,
                    height: None,
                }),
            )
            .unwrap();
        builder
            .emit(
                Some(paragraph),
                SemanticKind::TaskMarker(TaskMarker {
                    checked: false,
                    fallback_label: Some("Task".into()),
                }),
            )
            .unwrap();
        builder
            .emit(
                Some(paragraph),
                SemanticKind::InlineMath(MathValue {
                    source: "x".into(),
                    format: super::super::MathFormat::Tex,
                    fallback_text: Some("x".into()),
                }),
            )
            .unwrap();
        builder
            .emit(
                Some(paragraph),
                SemanticKind::DisplayMath(MathValue {
                    source: "y".into(),
                    format: super::super::MathFormat::Tex,
                    fallback_text: Some("y".into()),
                }),
            )
            .unwrap();
        builder
            .emit(
                Some(paragraph),
                SemanticKind::Media(Media {
                    kind: super::super::MediaKind::Audio,
                    source: "https://example.test/audio.mp3".into(),
                    title: None,
                }),
            )
            .unwrap();
        builder
            .emit(
                Some(paragraph),
                SemanticKind::FootnoteReference(footnote_id),
            )
            .unwrap();
        builder.close(paragraph).unwrap();

        builder
            .emit(
                Some(root),
                SemanticKind::CodeBlock(CodeBlock {
                    language: None,
                    text: "code block".into(),
                }),
            )
            .unwrap();
        let list = builder
            .emit(
                Some(root),
                SemanticKind::List(List {
                    kind: ListKind::Unordered,
                    start: None,
                }),
            )
            .unwrap();
        let item = builder.emit(Some(list), SemanticKind::ListItem).unwrap();
        builder.append_prose(Some(item), "Item").unwrap();
        builder.close(item).unwrap();
        builder.close(list).unwrap();

        let table = builder
            .emit(
                Some(root),
                SemanticKind::Table(Table {
                    column_count: Some(1),
                }),
            )
            .unwrap();
        let row = builder.emit(Some(table), SemanticKind::TableRow).unwrap();
        let cell = builder
            .emit(
                Some(row),
                SemanticKind::TableCell(TableCell {
                    header: false,
                    colspan: 1,
                    rowspan: 1,
                    alignment: None,
                }),
            )
            .unwrap();
        builder.append_prose(Some(cell), "Cell").unwrap();
        builder.close(cell).unwrap();
        builder.close(row).unwrap();
        builder.close(table).unwrap();

        let callout = builder
            .emit(
                Some(root),
                SemanticKind::Callout(Callout {
                    kind: super::super::CalloutKind::Note,
                    title: None,
                }),
            )
            .unwrap();
        builder.append_prose(Some(callout), "Callout").unwrap();
        builder.close(callout).unwrap();
        builder.close(root).unwrap();

        assert_eq!(builder.payloads.code_blocks.len(), 1);
        assert_eq!(builder.payloads.links.len(), 1);
        assert_eq!(builder.payloads.images.len(), 1);
        assert_eq!(builder.payloads.lists.len(), 1);
        assert_eq!(builder.payloads.tables.len(), 1);
        assert_eq!(builder.payloads.table_cells.len(), 1);
        assert_eq!(builder.payloads.callouts.len(), 1);
        assert_eq!(builder.payloads.task_markers.len(), 1);
        assert_eq!(builder.payloads.math_values.len(), 2);
        assert_eq!(builder.payloads.media.len(), 1);
        assert_eq!(builder.footnotes.len(), 1);
        assert!(builder.payloads.text_refs.len() >= 3);

        let document = builder.finish().unwrap();
        assert!(document.validate().is_ok());
    }
}
