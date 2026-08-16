use std::borrow::Cow;
use std::collections::HashMap;
use std::mem::size_of;
use std::sync::OnceLock;

use super::{
    Callout, CodeBlock, Document, DocumentNodeId, EventOp, FootnoteId, FootnoteRecord, Image, Link,
    List, MathValue, Media, NodeKind, OP_CLOSE, OperationKind, Table, TableCell, TaskMarker,
    TextValue,
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
    pending_spaces: Vec<bool>,
    pending_root_space: bool,
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
            pending_spaces: Vec::with_capacity(capacity),
            pending_root_space: false,
            footnotes: Vec::new(),
            footnote_index: HashMap::new(),
            output_capacity_hint: 0,
        }
    }

    pub(crate) fn append(
        &mut self,
        parent: Option<DocumentNodeId>,
        kind: NodeKind,
    ) -> Result<DocumentNodeId, BuildError> {
        if parent.is_some_and(|id| {
            id.index() >= self.nodes.len() || !source_is_container(&self.nodes[id.index()].kind)
        }) {
            return Err(BuildError::InvalidParent);
        }
        let pending_space = match parent {
            Some(id) => std::mem::take(&mut self.pending_spaces[id.index()]),
            None => std::mem::take(&mut self.pending_root_space),
        };
        if pending_space && is_inline_sibling(&kind) {
            let previous = match parent {
                Some(id) => self.last_children[id.index()],
                None => self.roots.last().copied(),
            };
            if let Some(previous) = previous
                && let NodeKind::Text(text) = &mut self.nodes[previous.index()].kind
            {
                if !text.ends_with(' ') {
                    text.as_mut_string().push(' ');
                    self.output_capacity_hint = self.output_capacity_hint.saturating_add(1);
                }
            } else {
                self.append_raw(parent, NodeKind::Text(TextValue::new(" ")))?;
            }
        }
        self.append_raw(parent, kind)
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
        self.pending_spaces.push(false);

        if let Some(parent) = parent {
            if let Some(previous) = self.last_children[parent.index()] {
                self.nodes[previous.index()].next_sibling = Some(id);
            } else {
                self.nodes[parent.index()].first_child = Some(id);
            }
            self.last_children[parent.index()] = Some(id);
        } else {
            self.roots.push(id);
        }
        Ok(id)
    }

    pub(crate) fn append_prose(
        &mut self,
        parent: Option<DocumentNodeId>,
        value: &str,
    ) -> Result<Option<DocumentNodeId>, BuildError> {
        if parent.is_some_and(|id| id.index() >= self.nodes.len()) {
            return Err(BuildError::InvalidParent);
        }
        let normalized = super::text::normalize_prose_fragment(value);
        self.append_normalized_prose_value(parent, normalized)
    }

    /// Appends a fragment that is already canonical prose.
    pub(crate) fn append_normalized_prose(
        &mut self,
        parent: Option<DocumentNodeId>,
        value: &str,
    ) -> Result<Option<DocumentNodeId>, BuildError> {
        if parent.is_some_and(|id| id.index() >= self.nodes.len()) {
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
        let previous = match parent {
            Some(id) => self.last_children[id.index()],
            None => self.roots.last().copied(),
        };
        if value_ref == " " {
            if previous.is_some() {
                match parent {
                    Some(id) => self.pending_spaces[id.index()] = true,
                    None => self.pending_root_space = true,
                }
            }
            return Ok(None);
        }
        let pending_space = match parent {
            Some(id) => std::mem::take(&mut self.pending_spaces[id.index()]),
            None => std::mem::take(&mut self.pending_root_space),
        };
        let needs_leading_space = pending_space && !value_ref.starts_with(' ');
        if let Some(previous) = previous
            && let NodeKind::Text(existing) = &mut self.nodes[previous.index()].kind
        {
            let leading_space = needs_leading_space && !existing.ends_with(' ');
            self.output_capacity_hint = self
                .output_capacity_hint
                .saturating_add(value_ref.len().saturating_add(usize::from(leading_space)));
            if leading_space {
                existing.as_mut_string().push(' ');
            }
            existing.append_normalized_prose(value_ref);
            return Ok(Some(previous));
        }

        let mut owned = match value {
            Cow::Borrowed(value) => {
                let mut owned = String::with_capacity(
                    value.len().saturating_add(usize::from(needs_leading_space)),
                );
                owned.push_str(value);
                owned
            }
            Cow::Owned(value) => value,
        };
        if needs_leading_space {
            owned.insert(0, ' ');
        }
        if owned.is_empty() {
            return Ok(None);
        }
        self.append(parent, NodeKind::Text(TextValue::new(owned)))
            .map(Some)
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
        let node_count = self.nodes.len();
        let mut ops = Vec::with_capacity(node_count.saturating_mul(2));
        let mut ends = Vec::with_capacity(node_count.saturating_mul(2));
        let mut roots = Vec::with_capacity(self.roots.len());
        let mut remap = vec![DocumentNodeId(0); node_count];
        let mut payloads = PayloadTables::default();
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
                    let new_id = emit_open(&mut ops, &mut ends, &mut payloads, kind);
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

        compact_excess_capacity(&mut ops);
        compact_excess_capacity(&mut ends);
        compact_excess_capacity(&mut roots);
        compact_excess_capacity(&mut payloads.text_values);
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
            text_values: payloads.text_values,
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

#[derive(Default)]
struct PayloadTables {
    text_values: Vec<TextValue>,
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
    kind: NodeKind,
) -> DocumentNodeId {
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
        NodeKind::Text(value) => (
            OperationKind::Text,
            push_payload(&mut payloads.text_values, value),
            0,
        ),
        NodeKind::Emphasis => (OperationKind::Emphasis, 0, 0),
        NodeKind::Strong => (OperationKind::Strong, 0, 0),
        NodeKind::Strikethrough => (OperationKind::Strikethrough, 0, 0),
        NodeKind::InlineCode(value) => (
            OperationKind::InlineCode,
            push_payload(&mut payloads.text_values, value),
            0,
        ),
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
            builder.append(Some(code), NodeKind::Text(TextValue::new("lost"))),
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
}
