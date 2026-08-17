use std::borrow::Cow;
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
}

struct OpenFrame {
    node: DocumentNodeId,
    flags: u8,
    last_child: Option<DocumentNodeId>,
    pending_space: bool,
}

impl SemanticTapeBuilder {
    pub(crate) fn with_capacity(capacity: usize) -> Self {
        Self {
            ops: Vec::with_capacity(capacity.saturating_mul(2)),
            ends: Vec::with_capacity(capacity.saturating_mul(2)),
            open: Vec::with_capacity(capacity),
            last_root_child: None,
            text: String::new(),
            payloads: PayloadTables::default(),
            pending_root_space: false,
            footnotes: Vec::new(),
            footnote_index: HashMap::new(),
            node_count: 0,
            output_capacity_hint: 0,
            compile_stats: CompileStats::default(),
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
        let payload = push_payload(&mut self.payloads.text_refs, value);
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
        self.append_normalized_prose_value(parent, normalized)
    }

    pub(crate) fn append_normalized_prose(
        &mut self,
        parent: Option<DocumentNodeId>,
        value: &str,
    ) -> Result<Option<DocumentNodeId>, BuildError> {
        if !self.valid_parent(parent) {
            return Err(BuildError::InvalidParent);
        }
        self.append_normalized_prose_value(parent, Cow::Borrowed(value))
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
        self.ops.push(EventOp {
            payload: node.0,
            aux: 0,
            opcode: operation.opcode | OP_CLOSE,
            flags,
        });
        self.ends.push(0);
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
        self.footnote_index.insert(id, self.footnotes.len());
        self.output_capacity_hint = self.output_capacity_hint.saturating_add(label.len());
        self.footnotes.push(FootnoteRecord {
            id,
            label: label.into(),
            node,
        });
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
        self.compile_stats.semantic_text_bytes = self.text.len();
        compact_excess_capacity(&mut self.ops);
        compact_excess_capacity(&mut self.ends);
        compact_excess_capacity(&mut self.payloads.text_refs);
        compact_excess_capacity(&mut self.payloads.code_blocks);
        compact_excess_capacity(&mut self.payloads.links);
        compact_excess_capacity(&mut self.payloads.images);
        compact_excess_capacity(&mut self.payloads.lists);
        compact_excess_capacity(&mut self.payloads.tables);
        compact_excess_capacity(&mut self.payloads.table_cells);
        compact_excess_capacity(&mut self.payloads.callouts);
        compact_excess_capacity(&mut self.payloads.task_markers);
        compact_excess_capacity(&mut self.payloads.math_values);
        compact_excess_capacity(&mut self.payloads.media);
        compact_excess_capacity(&mut self.footnotes);
        compact_excess_string(&mut self.text);

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
                push_payload(&mut self.payloads.code_blocks, value),
                0,
            ),
            SemanticKind::List(value) => (
                OperationKind::List,
                push_payload(&mut self.payloads.lists, value),
                0,
            ),
            SemanticKind::ListItem => (OperationKind::ListItem, 0, 0),
            SemanticKind::Table(value) => (
                OperationKind::Table,
                push_payload(&mut self.payloads.tables, value),
                0,
            ),
            SemanticKind::TableCaption => (OperationKind::TableCaption, 0, 0),
            SemanticKind::TableRow => (OperationKind::TableRow, 0, 0),
            SemanticKind::TableCell(value) => (
                OperationKind::TableCell,
                push_payload(&mut self.payloads.table_cells, value),
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
                push_payload(&mut self.payloads.callouts, value),
                0,
            ),
            SemanticKind::FootnoteDefinition(id) => (OperationKind::FootnoteDefinition, id.0, 0),
            SemanticKind::Emphasis => (OperationKind::Emphasis, 0, 0),
            SemanticKind::Strong => (OperationKind::Strong, 0, 0),
            SemanticKind::Strikethrough => (OperationKind::Strikethrough, 0, 0),
            SemanticKind::Link(value) => (
                OperationKind::Link,
                push_payload(&mut self.payloads.links, value),
                0,
            ),
            SemanticKind::Image(value) => (
                OperationKind::Image,
                push_payload(&mut self.payloads.images, value),
                0,
            ),
            SemanticKind::HardBreak => (OperationKind::HardBreak, 0, 0),
            SemanticKind::FootnoteReference(id) => (OperationKind::FootnoteReference, id.0, 0),
            SemanticKind::TaskMarker(value) => (
                OperationKind::TaskMarker,
                push_payload(&mut self.payloads.task_markers, value),
                0,
            ),
            SemanticKind::InlineMath(value) => (
                OperationKind::InlineMath,
                push_payload(&mut self.payloads.math_values, value),
                0,
            ),
            SemanticKind::DisplayMath(value) => (
                OperationKind::DisplayMath,
                push_payload(&mut self.payloads.math_values, value),
                0,
            ),
            SemanticKind::Media(value) => (
                OperationKind::Media,
                push_payload(&mut self.payloads.media, value),
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
        let index = u32::try_from(self.ops.len()).map_err(|_| BuildError::CapacityExceeded)?;
        let id = DocumentNodeId(index);
        self.ops.push(EventOp {
            payload,
            aux,
            opcode: operation as u8,
            flags: self.operation_visibility(operation, payload),
        });
        self.ends
            .push(if operation.is_container() { 0 } else { index });
        let flags = self.ops[id.index()].flags;
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
            self.open.push(OpenFrame {
                node: id,
                flags,
                last_child: None,
                pending_space: false,
            });
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
        value: Cow<'_, str>,
    ) -> Result<Option<DocumentNodeId>, BuildError> {
        if value.is_empty() {
            return Ok(None);
        }
        let value_ref = value.as_ref();
        let previous = self.previous_child(parent);
        if value_ref == " " {
            if previous.is_some() {
                self.set_pending_space(parent, true);
            }
            return Ok(None);
        }
        let needs_leading_space = self.take_pending_space(parent) && !value_ref.starts_with(' ');
        if let Some(previous) = previous
            && self.ops[previous.index()].kind() == OperationKind::Text
        {
            let payload = self.ops[previous.index()].payload as usize;
            let existing = self.payloads.text_refs[payload];
            let existing_value = self.text_slice(existing);
            let leading_space = needs_leading_space && !existing_value.ends_with(' ');
            let value = if existing_value.ends_with(' ') && value_ref.starts_with(' ') {
                &value_ref[1..]
            } else {
                value_ref
            };
            if existing.range().end == self.text.len() {
                let updated = self.extend_text(existing, leading_space, value)?;
                self.payloads.text_refs[payload] = updated;
                let flags = self.operation_visibility(OperationKind::Text, payload as u32);
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
            let payload = push_payload(&mut self.payloads.text_refs, value);
            return self
                .append_operation(parent, OperationKind::Text, payload, 0)
                .map(Some);
        }
        let value = self.append_text_with_prefix(value_ref, needs_leading_space)?;
        let payload = push_payload(&mut self.payloads.text_refs, value);
        self.append_operation(parent, OperationKind::Text, payload, 0)
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
        let payload = push_payload(&mut self.payloads.text_refs, separator);
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
            self.text.push(' ');
        }
        self.text.push_str(value);
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
            self.text.push(' ');
        }
        self.text.push_str(value);
        self.output_capacity_hint = self.output_capacity_hint.saturating_add(added);
        self.compile_stats.add_semantic_text_bytes(added);
        Ok(reference)
    }

    fn text_slice(&self, value: TextRef) -> &str {
        self.text
            .get(value.range())
            .expect("text reference must point into the tape text arena")
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

fn push_payload<T>(values: &mut Vec<T>, value: T) -> u32 {
    let index = u32::try_from(values.len()).unwrap_or(u32::MAX);
    values.push(value);
    index
}

/// Releases capacity only when the retained saving is material.
fn compact_excess_capacity<T>(values: &mut Vec<T>) {
    const MINIMUM_SAVING_BYTES: usize = 4 * 1024;

    let unused = values.capacity().saturating_sub(values.len());
    let unused_bytes = unused.saturating_mul(size_of::<T>());
    if values.capacity() > values.len().saturating_mul(2) && unused_bytes >= MINIMUM_SAVING_BYTES {
        values.shrink_to_fit();
    }
}

fn compact_excess_string(value: &mut String) {
    const MINIMUM_SAVING_BYTES: usize = 4 * 1024;

    let unused = value.capacity().saturating_sub(value.len());
    if unused >= MINIMUM_SAVING_BYTES && unused.saturating_mul(4) >= value.capacity() {
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
