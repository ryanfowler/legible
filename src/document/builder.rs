use std::borrow::Cow;
use std::collections::HashMap;
use std::mem::size_of;
use std::sync::OnceLock;

use super::{
    Callout, CodeBlock, Document, DocumentNodeId, EventOp, FootnoteId, FootnoteRecord, Image, Link,
    List, MathValue, Media, NodeKind, OP_CLOSE, OperationKind, Table, TableCell, TaskMarker,
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

/// Temporary source-order builder for the compact tape.
///
/// The builder accepts parent handles because complex recognition can defer
/// children. It keeps links only until `finish`; the retained document contains
/// operations and side tables, not this temporary tree.
struct BuildNode {
    kind: NodeKind,
    first_child: Option<DocumentNodeId>,
    next_sibling: Option<DocumentNodeId>,
}

pub(crate) struct DocumentBuilder {
    nodes: Vec<BuildNode>,
    roots: Vec<DocumentNodeId>,
    last_children: Vec<Option<DocumentNodeId>>,
    /// Enabled only by ordinary lowering, which may insert a deferred text
    /// boundary before an already opened semantic child.
    previous_siblings: Option<Vec<Option<DocumentNodeId>>>,
    pending_spaces: Vec<bool>,
    pending_root_space: bool,
    /// Prose and inline-code payloads share the retained text arena.
    text: String,
    /// Set when deferred builder work creates adjacent ranges that need one
    /// final source-order materialization.
    requires_text_materialization: bool,
    footnotes: Vec<FootnoteRecord>,
    footnote_index: HashMap<FootnoteId, usize>,
    output_capacity_hint: usize,
}

impl DocumentBuilder {
    pub(crate) fn with_capacity(capacity: usize) -> Self {
        Self {
            nodes: Vec::with_capacity(capacity),
            roots: Vec::new(),
            last_children: Vec::with_capacity(capacity),
            previous_siblings: None,
            pending_spaces: Vec::with_capacity(capacity),
            pending_root_space: false,
            text: String::new(),
            requires_text_materialization: false,
            footnotes: Vec::new(),
            footnote_index: HashMap::new(),
            output_capacity_hint: 0,
        }
    }

    pub(crate) fn enable_preorder_insertions(&mut self) {
        self.previous_siblings = Some(Vec::with_capacity(self.nodes.capacity()));
    }

    pub(crate) fn append(
        &mut self,
        parent: Option<DocumentNodeId>,
        kind: NodeKind,
    ) -> Result<DocumentNodeId, BuildError> {
        if !self.valid_parent(parent) {
            return Err(BuildError::InvalidParent);
        }
        if self.take_pending_space(parent) && is_inline_sibling(&kind) {
            self.append_pending_separator(parent)?;
        }
        self.append_raw(parent, kind)
    }

    /// Appends raw inline code to the same arena as canonical prose.
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
        self.append_raw(parent, NodeKind::InlineCode(value))
    }

    fn append_raw(
        &mut self,
        parent: Option<DocumentNodeId>,
        kind: NodeKind,
    ) -> Result<DocumentNodeId, BuildError> {
        let raw = u32::try_from(self.nodes.len()).map_err(|_| BuildError::CapacityExceeded)?;
        let id = DocumentNodeId(raw);
        self.output_capacity_hint = self
            .output_capacity_hint
            .saturating_add(kind.output_capacity_hint());
        self.nodes.push(BuildNode {
            kind,
            first_child: None,
            next_sibling: None,
        });
        self.last_children.push(None);
        if let Some(previous_siblings) = &mut self.previous_siblings {
            previous_siblings.push(None);
        }
        self.pending_spaces.push(false);

        if let Some(parent) = parent {
            if let Some(previous) = self.last_children[parent.index()] {
                self.nodes[previous.index()].next_sibling = Some(id);
                if let Some(previous_siblings) = &mut self.previous_siblings {
                    previous_siblings[id.index()] = Some(previous);
                }
            } else {
                self.nodes[parent.index()].first_child = Some(id);
            }
            self.last_children[parent.index()] = Some(id);
        } else {
            if let Some(&previous) = self.roots.last() {
                self.nodes[previous.index()].next_sibling = Some(id);
                if let Some(previous_siblings) = &mut self.previous_siblings {
                    previous_siblings[id.index()] = Some(previous);
                }
            }
            self.roots.push(id);
        }
        Ok(id)
    }

    /// Inserts a canonical prose text node immediately before an existing child.
    ///
    /// Ordinary lowering discovers some word boundaries only after it has
    /// opened a semantic child. The temporary builder keeps enough sibling
    /// links to insert that boundary without a second source traversal.
    pub(crate) fn insert_normalized_prose_before(
        &mut self,
        parent: Option<DocumentNodeId>,
        before: DocumentNodeId,
        value: &str,
    ) -> Result<(), BuildError> {
        if value.is_empty() || !self.valid_parent(parent) || before.index() >= self.nodes.len() {
            return Err(BuildError::InvalidParent);
        }
        let value = super::text::normalize_prose_fragment(value);
        if value.is_empty() {
            return Ok(());
        }
        let previous = self
            .previous_siblings
            .as_ref()
            .and_then(|siblings| siblings.get(before.index()).copied())
            .ok_or(BuildError::InvalidParent)?;
        let valid_before = match parent {
            Some(parent) => previous.map_or(
                self.nodes[parent.index()].first_child == Some(before),
                |previous| self.nodes[previous.index()].next_sibling == Some(before),
            ),
            None => previous.map_or(self.roots.first() == Some(&before), |previous| {
                self.nodes[previous.index()].next_sibling == Some(before)
            }),
        };
        if !valid_before {
            return Err(BuildError::InvalidParent);
        }
        let text = self.append_text(value.as_ref())?;
        let id = DocumentNodeId(
            u32::try_from(self.nodes.len()).map_err(|_| BuildError::CapacityExceeded)?,
        );
        self.nodes.push(BuildNode {
            kind: NodeKind::Text(text),
            first_child: None,
            next_sibling: Some(before),
        });
        self.last_children.push(None);
        if let Some(previous_siblings) = &mut self.previous_siblings {
            previous_siblings.push(previous);
        }
        self.pending_spaces.push(false);
        self.requires_text_materialization = true;

        if let Some(previous) = previous {
            self.nodes[previous.index()].next_sibling = Some(id);
        } else if let Some(parent) = parent {
            self.nodes[parent.index()].first_child = Some(id);
        } else if self.roots.first() == Some(&before) {
            self.roots[0] = id;
        } else {
            return Err(BuildError::InvalidParent);
        }
        if let Some(previous_siblings) = &mut self.previous_siblings {
            previous_siblings[before.index()] = Some(id);
        }
        Ok(())
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

    /// Appends prose as a distinct text node.
    ///
    /// Most prose should use the coalescing path. A source fallback that may
    /// receive a deferred boundary needs a stable insertion anchor instead.
    pub(crate) fn append_prose_unmerged(
        &mut self,
        parent: Option<DocumentNodeId>,
        value: &str,
    ) -> Result<Option<DocumentNodeId>, BuildError> {
        if !self.valid_parent(parent) {
            return Err(BuildError::InvalidParent);
        }
        let value = super::text::normalize_prose_fragment(value);
        if value.is_empty() {
            return Ok(None);
        }
        let needs_leading_space = self.take_pending_space(parent) && !value.starts_with(' ');
        let text = self.append_text_with_prefix(value.as_ref(), needs_leading_space)?;
        self.requires_text_materialization = true;
        self.append_raw(parent, NodeKind::Text(text)).map(Some)
    }

    /// Appends a fragment that is already canonical prose.
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
            && let NodeKind::Text(existing) = self.nodes[previous.index()].kind
        {
            let existing_value = self.text_slice(existing);
            let leading_space = needs_leading_space && !existing_value.ends_with(' ');
            let value = if existing_value.ends_with(' ') && value_ref.starts_with(' ') {
                &value_ref[1..]
            } else {
                value_ref
            };
            if existing.range().end == self.text.len() {
                let updated = self.extend_text(existing, leading_space, value)?;
                if let NodeKind::Text(text) = &mut self.nodes[previous.index()].kind {
                    *text = updated;
                }
                return Ok(Some(previous));
            }
            if !leading_space && value.is_empty() {
                return Ok(Some(previous));
            }
            let value = self.append_text_with_prefix(value, leading_space)?;
            self.requires_text_materialization = true;
            return self.append_raw(parent, NodeKind::Text(value)).map(Some);
        }

        let value = self.append_text_with_prefix(value_ref, needs_leading_space)?;
        self.append_raw(parent, NodeKind::Text(value)).map(Some)
    }

    fn valid_parent(&self, parent: Option<DocumentNodeId>) -> bool {
        parent.is_none_or(|id| {
            id.index() < self.nodes.len() && source_is_container(&self.nodes[id.index()].kind)
        })
    }

    fn previous_child(&self, parent: Option<DocumentNodeId>) -> Option<DocumentNodeId> {
        match parent {
            Some(id) => self.last_children[id.index()],
            None => self.roots.last().copied(),
        }
    }

    fn take_pending_space(&mut self, parent: Option<DocumentNodeId>) -> bool {
        match parent {
            Some(id) => std::mem::take(&mut self.pending_spaces[id.index()]),
            None => std::mem::take(&mut self.pending_root_space),
        }
    }

    fn set_pending_space(&mut self, parent: Option<DocumentNodeId>, value: bool) {
        match parent {
            Some(id) => self.pending_spaces[id.index()] = value,
            None => self.pending_root_space = value,
        }
    }

    fn append_pending_separator(
        &mut self,
        parent: Option<DocumentNodeId>,
    ) -> Result<(), BuildError> {
        let previous = self.previous_child(parent);
        if let Some(previous) = previous
            && let NodeKind::Text(existing) = self.nodes[previous.index()].kind
        {
            if !self.text_slice(existing).ends_with(' ') {
                if existing.range().end == self.text.len() {
                    let updated = self.extend_text(existing, false, " ")?;
                    if let NodeKind::Text(text) = &mut self.nodes[previous.index()].kind {
                        *text = updated;
                    }
                } else {
                    let separator = self.append_text(" ")?;
                    self.requires_text_materialization = true;
                    self.append_raw(parent, NodeKind::Text(separator))?;
                }
            }
        } else {
            let separator = self.append_text(" ")?;
            self.append_raw(parent, NodeKind::Text(separator))?;
        }
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
        Ok(reference)
    }

    fn text_slice(&self, value: TextRef) -> &str {
        self.text
            .get(value.range())
            .expect("text reference must point into the builder arena")
    }

    pub(crate) fn kind(&self, id: DocumentNodeId) -> Option<&NodeKind> {
        self.nodes.get(id.index()).map(|node| &node.kind)
    }

    pub(crate) fn kind_mut(&mut self, id: DocumentNodeId) -> Option<&mut NodeKind> {
        self.nodes.get_mut(id.index()).map(|node| &mut node.kind)
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
        if node.index() >= self.nodes.len() {
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

    pub(crate) fn finish(mut self) -> Document {
        let source_text = std::mem::take(&mut self.text);
        let source_node_count = self.nodes.len();
        let mut node_count = 0usize;
        let mut text = self
            .requires_text_materialization
            .then(|| String::with_capacity(source_text.len()));
        let mut ops = Vec::with_capacity(source_node_count.saturating_mul(2));
        let mut ends = Vec::with_capacity(source_node_count.saturating_mul(2));
        let mut roots = Vec::with_capacity(self.roots.len());
        let mut remap = vec![DocumentNodeId(0); source_node_count];
        let mut payloads = PayloadTables::default();
        let mut last_text = None;
        let mut tasks = Vec::with_capacity(32);
        tasks.extend(self.roots.iter().rev().map(|&root| BuildTask::Enter(root)));

        while let Some(task) = tasks.pop() {
            match task {
                BuildTask::Enter(old_id) => {
                    let kind = std::mem::replace(
                        &mut self.nodes[old_id.index()].kind,
                        NodeKind::BlockGroup,
                    );
                    let is_container = source_is_container(&kind)
                        || self.nodes[old_id.index()].first_child.is_some();
                    let previous_operations = ops.len();
                    let new_id = emit_open(
                        &mut ops,
                        &mut ends,
                        &mut payloads,
                        &source_text,
                        text.as_mut(),
                        &mut last_text,
                        kind,
                    );
                    if ops.len() > previous_operations {
                        node_count += 1;
                    }
                    remap[old_id.index()] = new_id;
                    if is_container {
                        tasks.push(BuildTask::Exit(new_id));
                        if let Some(child) = self.nodes[old_id.index()].first_child {
                            tasks.push(BuildTask::Siblings(child));
                        }
                    }
                }
                BuildTask::Siblings(old_id) => {
                    if let Some(sibling) = self.nodes[old_id.index()].next_sibling {
                        tasks.push(BuildTask::Siblings(sibling));
                    }
                    tasks.push(BuildTask::Enter(old_id));
                }
                BuildTask::Exit(open_id) => {
                    let kind = ops[open_id.index()].opcode;
                    let close = u32::try_from(ops.len()).unwrap_or(u32::MAX);
                    ops.push(EventOp {
                        payload: open_id.0,
                        aux: 0,
                        opcode: kind | OP_CLOSE,
                        flags: 0,
                    });
                    ends.push(0);
                    ends[open_id.index()] = close;
                    last_text = None;
                }
            }
        }

        roots.extend(self.roots.iter().map(|id| remap[id.index()]));

        // Reorder footnotes into ID order, then translate temporary builder IDs
        // to opening-operation IDs in the retained tape.
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
        for definition in &mut footnotes {
            definition.node = remap
                .get(definition.node.index())
                .copied()
                .unwrap_or(DocumentNodeId(u32::MAX));
        }

        if let Some(text) = text.as_mut() {
            compact_excess_string(text);
        }
        let text = text.unwrap_or(source_text);
        compact_excess_capacity(&mut ops);
        compact_excess_capacity(&mut ends);
        compact_excess_capacity(&mut roots);
        compact_excess_capacity(&mut payloads.text_refs);
        compact_excess_capacity(&mut payloads.code_blocks);
        compact_excess_capacity(&mut payloads.links);
        compact_excess_capacity(&mut payloads.images);
        compact_excess_capacity(&mut payloads.lists);
        compact_excess_capacity(&mut payloads.tables);
        compact_excess_capacity(&mut payloads.table_cells);
        compact_excess_capacity(&mut payloads.callouts);
        compact_excess_capacity(&mut payloads.task_markers);
        compact_excess_capacity(&mut payloads.math_values);
        compact_excess_capacity(&mut payloads.media);
        compact_excess_capacity(&mut footnotes);

        Document {
            ops,
            ends,
            roots,
            text,
            text_refs: payloads.text_refs,
            code_blocks: payloads.code_blocks,
            links: payloads.links,
            images: payloads.images,
            lists: payloads.lists,
            tables: payloads.tables,
            table_cells: payloads.table_cells,
            callouts: payloads.callouts,
            task_markers: payloads.task_markers,
            math_values: payloads.math_values,
            media: payloads.media,
            footnotes,
            node_count,
            output_capacity_hint: self.output_capacity_hint,
            stats: OnceLock::new(),
        }
    }
}

/// Direct builder for complex semantic lowering.
///
/// Unlike [`DocumentBuilder`], this builder never creates a temporary semantic
/// tree. It appends opening and leaf operations in source order and records
/// closing operations when the lowering task stack leaves a container. The
/// retained document is therefore built in one pass over the semantic source.
pub(crate) struct SemanticTapeBuilder {
    ops: Vec<EventOp>,
    ends: Vec<u32>,
    roots: Vec<DocumentNodeId>,
    text: String,
    payloads: PayloadTables,
    last_children: Vec<Option<DocumentNodeId>>,
    pending_spaces: Vec<bool>,
    pending_root_space: bool,
    footnotes: Vec<FootnoteRecord>,
    footnote_index: HashMap<FootnoteId, usize>,
    node_count: usize,
    output_capacity_hint: usize,
}

impl SemanticTapeBuilder {
    pub(crate) fn with_capacity(capacity: usize) -> Self {
        Self {
            ops: Vec::with_capacity(capacity.saturating_mul(2)),
            ends: Vec::with_capacity(capacity.saturating_mul(2)),
            roots: Vec::new(),
            text: String::new(),
            payloads: PayloadTables::default(),
            last_children: Vec::with_capacity(capacity.saturating_mul(2)),
            pending_spaces: Vec::with_capacity(capacity.saturating_mul(2)),
            pending_root_space: false,
            footnotes: Vec::new(),
            footnote_index: HashMap::new(),
            node_count: 0,
            output_capacity_hint: 0,
        }
    }

    pub(crate) fn append(
        &mut self,
        parent: Option<DocumentNodeId>,
        kind: NodeKind,
    ) -> Result<DocumentNodeId, BuildError> {
        if !self.valid_parent(parent) {
            return Err(BuildError::InvalidParent);
        }
        if self.take_pending_space(parent) && is_inline_sibling(&kind) {
            self.append_pending_separator(parent)?;
        }
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

    pub(crate) fn append_prose_unmerged(
        &mut self,
        parent: Option<DocumentNodeId>,
        value: &str,
    ) -> Result<Option<DocumentNodeId>, BuildError> {
        if !self.valid_parent(parent) {
            return Err(BuildError::InvalidParent);
        }
        let value = super::text::normalize_prose_fragment(value);
        if value.is_empty() {
            return Ok(None);
        }
        let needs_leading_space = self.take_pending_space(parent) && !value.starts_with(' ');
        let text = self.append_text_with_prefix(value.as_ref(), needs_leading_space)?;
        let payload = push_payload(&mut self.payloads.text_refs, text);
        self.append_operation(parent, OperationKind::Text, payload, 0)
            .map(Some)
    }

    pub(crate) fn close(&mut self, node: DocumentNodeId) -> Result<(), BuildError> {
        let Some(operation) = self.ops.get(node.index()).copied() else {
            return Err(BuildError::InvalidParent);
        };
        if operation.is_close() || !operation.kind().is_container() || self.ends[node.index()] != 0
        {
            return Err(BuildError::InvalidParent);
        }
        let close = u32::try_from(self.ops.len()).map_err(|_| BuildError::CapacityExceeded)?;
        self.ops.push(EventOp {
            payload: node.0,
            aux: 0,
            opcode: operation.opcode | OP_CLOSE,
            flags: 0,
        });
        self.ends.push(0);
        self.last_children.push(None);
        self.pending_spaces.push(false);
        self.ends[node.index()] = close;
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

    pub(crate) fn is_redundant_formatting(
        &self,
        parent: Option<DocumentNodeId>,
        kind: &NodeKind,
    ) -> bool {
        let Some(parent) = parent else {
            return false;
        };
        let Some(parent) = self.operation_kind(parent) else {
            return false;
        };
        matches!(
            (kind, parent),
            (NodeKind::Strong, OperationKind::Strong)
                | (NodeKind::Emphasis, OperationKind::Emphasis)
                | (NodeKind::Strikethrough, OperationKind::Strikethrough)
        )
    }

    pub(crate) fn table_mut(&mut self, node: DocumentNodeId) -> Option<&mut Table> {
        let operation = self.ops.get(node.index()).copied()?;
        (operation.kind() == OperationKind::Table)
            .then(|| self.payloads.tables.get_mut(operation.payload as usize))?
    }

    pub(crate) fn finish(mut self) -> Document {
        compact_excess_capacity(&mut self.ops);
        compact_excess_capacity(&mut self.ends);
        compact_excess_capacity(&mut self.roots);
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

        Document {
            ops: self.ops,
            ends: self.ends,
            roots: self.roots,
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
            stats: OnceLock::new(),
        }
    }

    fn append_kind(
        &mut self,
        parent: Option<DocumentNodeId>,
        kind: NodeKind,
    ) -> Result<DocumentNodeId, BuildError> {
        self.output_capacity_hint = self
            .output_capacity_hint
            .saturating_add(kind.output_capacity_hint());
        let (operation, payload, aux) = match kind {
            NodeKind::Paragraph => (OperationKind::Paragraph, 0, 0),
            NodeKind::BlockGroup => (OperationKind::BlockGroup, 0, 0),
            NodeKind::Heading { level } => (OperationKind::Heading, 0, u16::from(level)),
            NodeKind::BlockQuote => (OperationKind::BlockQuote, 0, 0),
            NodeKind::CodeBlock(value) => (
                OperationKind::CodeBlock,
                push_payload(&mut self.payloads.code_blocks, value),
                0,
            ),
            NodeKind::List(value) => (
                OperationKind::List,
                push_payload(&mut self.payloads.lists, value),
                0,
            ),
            NodeKind::ListItem => (OperationKind::ListItem, 0, 0),
            NodeKind::Table(value) => (
                OperationKind::Table,
                push_payload(&mut self.payloads.tables, value),
                0,
            ),
            NodeKind::TableCaption => (OperationKind::TableCaption, 0, 0),
            NodeKind::TableRow => (OperationKind::TableRow, 0, 0),
            NodeKind::TableCell(value) => (
                OperationKind::TableCell,
                push_payload(&mut self.payloads.table_cells, value),
                0,
            ),
            NodeKind::Figure => (OperationKind::Figure, 0, 0),
            NodeKind::Figcaption => (OperationKind::Figcaption, 0, 0),
            NodeKind::Details => (OperationKind::Details, 0, 0),
            NodeKind::Summary => (OperationKind::Summary, 0, 0),
            NodeKind::ThematicBreak => (OperationKind::ThematicBreak, 0, 0),
            NodeKind::DefinitionList => (OperationKind::DefinitionList, 0, 0),
            NodeKind::DefinitionTerm => (OperationKind::DefinitionTerm, 0, 0),
            NodeKind::DefinitionDescription => (OperationKind::DefinitionDescription, 0, 0),
            NodeKind::Callout(value) => (
                OperationKind::Callout,
                push_payload(&mut self.payloads.callouts, value),
                0,
            ),
            NodeKind::FootnoteDefinition(id) => (OperationKind::FootnoteDefinition, id.0, 0),
            NodeKind::Text(value) => (
                OperationKind::Text,
                push_payload(&mut self.payloads.text_refs, value),
                0,
            ),
            NodeKind::Emphasis => (OperationKind::Emphasis, 0, 0),
            NodeKind::Strong => (OperationKind::Strong, 0, 0),
            NodeKind::Strikethrough => (OperationKind::Strikethrough, 0, 0),
            NodeKind::InlineCode(value) => (
                OperationKind::InlineCode,
                push_payload(&mut self.payloads.text_refs, value),
                0,
            ),
            NodeKind::Link(value) => (
                OperationKind::Link,
                push_payload(&mut self.payloads.links, value),
                0,
            ),
            NodeKind::Image(value) => (
                OperationKind::Image,
                push_payload(&mut self.payloads.images, value),
                0,
            ),
            NodeKind::HardBreak => (OperationKind::HardBreak, 0, 0),
            NodeKind::FootnoteReference(id) => (OperationKind::FootnoteReference, id.0, 0),
            NodeKind::TaskMarker(value) => (
                OperationKind::TaskMarker,
                push_payload(&mut self.payloads.task_markers, value),
                0,
            ),
            NodeKind::InlineMath(value) => (
                OperationKind::InlineMath,
                push_payload(&mut self.payloads.math_values, value),
                0,
            ),
            NodeKind::DisplayMath(value) => (
                OperationKind::DisplayMath,
                push_payload(&mut self.payloads.math_values, value),
                0,
            ),
            NodeKind::Media(value) => (
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
            flags: 0,
        });
        self.ends
            .push(if operation.is_container() { 0 } else { index });
        self.last_children.push(None);
        self.pending_spaces.push(false);
        self.node_count += 1;
        if let Some(parent) = parent {
            self.last_children[parent.index()] = Some(id);
        } else {
            self.roots.push(id);
        }
        Ok(id)
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
        parent.is_none_or(|id| {
            self.ops.get(id.index()).is_some_and(|operation| {
                !operation.is_close()
                    && operation.kind().is_container()
                    && self.ends[id.index()] == 0
            })
        })
    }

    fn operation_kind(&self, node: DocumentNodeId) -> Option<OperationKind> {
        self.ops
            .get(node.index())
            .filter(|operation| !operation.is_close())
            .map(|operation| operation.kind())
    }

    fn previous_child(&self, parent: Option<DocumentNodeId>) -> Option<DocumentNodeId> {
        parent
            .map(|id| self.last_children[id.index()])
            .unwrap_or_else(|| self.roots.last().copied())
    }

    fn take_pending_space(&mut self, parent: Option<DocumentNodeId>) -> bool {
        match parent {
            Some(id) => std::mem::take(&mut self.pending_spaces[id.index()]),
            None => std::mem::take(&mut self.pending_root_space),
        }
    }

    fn set_pending_space(&mut self, parent: Option<DocumentNodeId>, value: bool) {
        match parent {
            Some(id) => self.pending_spaces[id.index()] = value,
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

enum BuildTask {
    Enter(DocumentNodeId),
    Siblings(DocumentNodeId),
    Exit(DocumentNodeId),
}

fn emit_open(
    ops: &mut Vec<EventOp>,
    ends: &mut Vec<u32>,
    payloads: &mut PayloadTables,
    source_text: &str,
    text: Option<&mut String>,
    last_text: &mut Option<DocumentNodeId>,
    kind: NodeKind,
) -> DocumentNodeId {
    if !matches!(&kind, NodeKind::Text(_)) {
        *last_text = None;
    }
    let (operation, payload, aux) = match kind {
        NodeKind::Paragraph => (OperationKind::Paragraph, 0, 0),
        NodeKind::BlockGroup => (OperationKind::BlockGroup, 0, 0),
        NodeKind::Heading { level } => (OperationKind::Heading, 0, u16::from(level)),
        NodeKind::BlockQuote => (OperationKind::BlockQuote, 0, 0),
        NodeKind::CodeBlock(value) => (
            OperationKind::CodeBlock,
            push_payload(&mut payloads.code_blocks, value),
            0,
        ),
        NodeKind::List(value) => (
            OperationKind::List,
            push_payload(&mut payloads.lists, value),
            0,
        ),
        NodeKind::ListItem => (OperationKind::ListItem, 0, 0),
        NodeKind::Table(value) => (
            OperationKind::Table,
            push_payload(&mut payloads.tables, value),
            0,
        ),
        NodeKind::TableCaption => (OperationKind::TableCaption, 0, 0),
        NodeKind::TableRow => (OperationKind::TableRow, 0, 0),
        NodeKind::TableCell(value) => (
            OperationKind::TableCell,
            push_payload(&mut payloads.table_cells, value),
            0,
        ),
        NodeKind::Figure => (OperationKind::Figure, 0, 0),
        NodeKind::Figcaption => (OperationKind::Figcaption, 0, 0),
        NodeKind::Details => (OperationKind::Details, 0, 0),
        NodeKind::Summary => (OperationKind::Summary, 0, 0),
        NodeKind::ThematicBreak => (OperationKind::ThematicBreak, 0, 0),
        NodeKind::DefinitionList => (OperationKind::DefinitionList, 0, 0),
        NodeKind::DefinitionTerm => (OperationKind::DefinitionTerm, 0, 0),
        NodeKind::DefinitionDescription => (OperationKind::DefinitionDescription, 0, 0),
        NodeKind::Callout(value) => (
            OperationKind::Callout,
            push_payload(&mut payloads.callouts, value),
            0,
        ),
        NodeKind::FootnoteDefinition(id) => (OperationKind::FootnoteDefinition, id.0, 0),
        NodeKind::Text(value) => {
            let source = source_text
                .get(value.range())
                .expect("builder text reference must be valid");
            if let Some(text) = text {
                if let Some(previous) = *last_text {
                    let payload = ops[previous.index()].payload as usize;
                    let existing = payloads.text_refs[payload];
                    payloads.text_refs[payload] = merge_document_text(text, existing, source);
                    return previous;
                }
                let reference = append_document_text(text, source);
                let index = DocumentNodeId(u32::try_from(ops.len()).unwrap_or(u32::MAX));
                *last_text = Some(index);
                (
                    OperationKind::Text,
                    push_payload(&mut payloads.text_refs, reference),
                    0,
                )
            } else {
                (
                    OperationKind::Text,
                    push_payload(&mut payloads.text_refs, value),
                    0,
                )
            }
        }
        NodeKind::Emphasis => (OperationKind::Emphasis, 0, 0),
        NodeKind::Strong => (OperationKind::Strong, 0, 0),
        NodeKind::Strikethrough => (OperationKind::Strikethrough, 0, 0),
        NodeKind::InlineCode(value) => {
            let source = source_text
                .get(value.range())
                .expect("builder text reference must be valid");
            let reference = text.map_or(value, |text| append_document_text(text, source));
            (
                OperationKind::InlineCode,
                push_payload(&mut payloads.text_refs, reference),
                0,
            )
        }
        NodeKind::Link(value) => (
            OperationKind::Link,
            push_payload(&mut payloads.links, value),
            0,
        ),
        NodeKind::Image(value) => (
            OperationKind::Image,
            push_payload(&mut payloads.images, value),
            0,
        ),
        NodeKind::HardBreak => (OperationKind::HardBreak, 0, 0),
        NodeKind::FootnoteReference(id) => (OperationKind::FootnoteReference, id.0, 0),
        NodeKind::TaskMarker(value) => (
            OperationKind::TaskMarker,
            push_payload(&mut payloads.task_markers, value),
            0,
        ),
        NodeKind::InlineMath(value) => (
            OperationKind::InlineMath,
            push_payload(&mut payloads.math_values, value),
            0,
        ),
        NodeKind::DisplayMath(value) => (
            OperationKind::DisplayMath,
            push_payload(&mut payloads.math_values, value),
            0,
        ),
        NodeKind::Media(value) => (
            OperationKind::Media,
            push_payload(&mut payloads.media, value),
            0,
        ),
    };
    let index = DocumentNodeId(u32::try_from(ops.len()).unwrap_or(u32::MAX));
    ops.push(EventOp {
        payload,
        aux,
        opcode: operation as u8,
        flags: 0,
    });
    ends.push(0);
    if !operation.is_container() {
        ends[index.index()] = index.0;
    }
    index
}

fn append_document_text(text: &mut String, value: &str) -> TextRef {
    let start = text.len();
    let reference =
        TextRef::new(start, value.len()).expect("semantic text arena exceeds u32 capacity");
    text.push_str(value);
    reference
}

fn merge_document_text(text: &mut String, existing: TextRef, value: &str) -> TextRef {
    let existing_value = text
        .get(existing.range())
        .expect("document text reference must be valid");
    let value = if existing_value.ends_with(' ') && value.starts_with(' ') {
        &value[1..]
    } else {
        value
    };
    let reference = TextRef::new(existing.start as usize, existing.len as usize + value.len())
        .expect("semantic text arena exceeds u32 capacity");
    text.push_str(value);
    reference
}

fn push_payload<T>(values: &mut Vec<T>, value: T) -> u32 {
    let index = u32::try_from(values.len()).unwrap_or(u32::MAX);
    values.push(value);
    index
}

fn source_is_container(kind: &NodeKind) -> bool {
    !matches!(
        kind,
        NodeKind::CodeBlock(_)
            | NodeKind::Text(_)
            | NodeKind::InlineCode(_)
            | NodeKind::Image(_)
            | NodeKind::HardBreak
            | NodeKind::ThematicBreak
            | NodeKind::FootnoteReference(_)
            | NodeKind::TaskMarker(_)
            | NodeKind::InlineMath(_)
            | NodeKind::DisplayMath(_)
            | NodeKind::Media(_)
    )
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

fn is_inline_sibling(kind: &NodeKind) -> bool {
    matches!(
        kind,
        NodeKind::Text(_)
            | NodeKind::Emphasis
            | NodeKind::Strong
            | NodeKind::Strikethrough
            | NodeKind::InlineCode(_)
            | NodeKind::Link(_)
            | NodeKind::Image(_)
            | NodeKind::HardBreak
            | NodeKind::FootnoteReference(_)
            | NodeKind::InlineMath(_)
            | NodeKind::Media(_)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_children_under_leaf_nodes() {
        let mut builder = DocumentBuilder::with_capacity(2);
        let code = builder
            .append(
                None,
                NodeKind::CodeBlock(CodeBlock {
                    language: None,
                    text: "code".into(),
                }),
            )
            .unwrap();
        assert_eq!(
            builder.append_prose(Some(code), "lost"),
            Err(BuildError::InvalidParent)
        );
    }

    #[test]
    fn thematic_break_is_one_tape_operation() {
        let mut builder = DocumentBuilder::with_capacity(1);
        builder.append(None, NodeKind::ThematicBreak).unwrap();
        let document = builder.finish();
        assert_eq!(document.ops.len(), 1);
        assert_eq!(document.ends[0], 0);
    }

    #[test]
    fn complex_tape_builder_emits_without_a_temporary_tree() {
        let mut builder = SemanticTapeBuilder::with_capacity(3);
        let paragraph = builder.append(None, NodeKind::Paragraph).unwrap();
        builder.append_prose(Some(paragraph), "direct").unwrap();
        let strong = builder.append(Some(paragraph), NodeKind::Strong).unwrap();
        builder.append_prose(Some(strong), "tape").unwrap();
        builder.close(strong).unwrap();
        builder.close(paragraph).unwrap();

        let document = builder.finish();
        document.validate().unwrap();
        assert_eq!(
            document.debug_tree(),
            "Paragraph\n  Text(\"direct\")\n  Strong\n    Text(\"tape\")\n"
        );
    }

    #[test]
    fn normalized_prose_merges_adjacent_fragments() {
        let mut builder = DocumentBuilder::with_capacity(2);
        let paragraph = builder.append(None, NodeKind::Paragraph).unwrap();
        let first = builder
            .append_normalized_prose(Some(paragraph), "first")
            .unwrap();
        let second = builder
            .append_normalized_prose(Some(paragraph), " second")
            .unwrap();

        assert_eq!(first, second);
        assert_eq!(
            builder.finish().debug_tree(),
            "Paragraph\n  Text(\"first second\")\n"
        );
    }

    #[test]
    fn pending_prose_boundaries_do_not_duplicate_existing_spaces() {
        let mut builder = DocumentBuilder::with_capacity(2);
        let paragraph = builder.append(None, NodeKind::Paragraph).unwrap();
        builder.append_prose(Some(paragraph), "a ").unwrap();
        builder
            .append_normalized_prose(Some(paragraph), " ")
            .unwrap();
        builder.append_prose(Some(paragraph), "b").unwrap();

        assert_eq!(
            builder.finish().debug_tree(),
            "Paragraph\n  Text(\"a b\")\n"
        );
    }

    #[test]
    fn prose_and_inline_code_share_one_text_arena() {
        let mut builder = DocumentBuilder::with_capacity(4);
        let paragraph = builder.append(None, NodeKind::Paragraph).unwrap();
        builder.append_prose(Some(paragraph), "first").unwrap();
        builder.append_prose(Some(paragraph), " ").unwrap();
        builder.append_inline_code(Some(paragraph), "code").unwrap();
        builder.append_prose(Some(paragraph), " tail").unwrap();

        let document = builder.finish();
        assert_eq!(document.text, "first code tail");
        assert_eq!(document.text_refs.len(), 3);
        assert_eq!(
            document.debug_tree(),
            "Paragraph\n  Text(\"first \")\n  InlineCode(\"code\")\n  Text(\" tail\")\n"
        );
    }

    #[test]
    fn interleaved_builder_appends_merge_at_finish_without_repacking() {
        let mut builder = DocumentBuilder::with_capacity(4);
        let first = builder.append(None, NodeKind::Paragraph).unwrap();
        builder.append_prose(Some(first), "first").unwrap();
        let second = builder.append(None, NodeKind::Paragraph).unwrap();
        builder.append_prose(Some(second), "second").unwrap();
        builder.append_prose(Some(first), " tail").unwrap();

        let document = builder.finish();
        assert_eq!(document.text, "first tailsecond");
        assert_eq!(document.len(), 4);
        document.validate().unwrap();
        assert_eq!(
            document.debug_tree(),
            "Paragraph\n  Text(\"first tail\")\nParagraph\n  Text(\"second\")\n"
        );
    }
}
