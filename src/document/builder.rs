use std::collections::HashMap;
use std::mem::size_of;
use std::sync::OnceLock;

use super::{ArenaNode, Document, DocumentNodeId, FootnoteId, FootnoteRecord, NodeKind, TextValue};
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

/// Builds sibling links while keeping per-node child storage out of the document.
pub(crate) struct DocumentBuilder {
    nodes: Vec<ArenaNode>,
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
        if parent.is_some_and(|id| id.index() >= self.nodes.len()) {
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
        let output_capacity_hint = kind.output_capacity_hint();
        self.nodes.push(ArenaNode {
            kind,
            first_child: None,
            next_sibling: None,
        });
        self.output_capacity_hint = self
            .output_capacity_hint
            .saturating_add(output_capacity_hint);
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
        let mut normalized = super::text::normalize_prose_fragment(value);
        if normalized.is_empty() {
            return Ok(None);
        }

        let previous = match parent {
            Some(id) => self.last_children[id.index()],
            None => self.roots.last().copied(),
        };
        if normalized == " " {
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
        if pending_space && !normalized.starts_with(' ') {
            normalized.insert(0, ' ');
        }
        if let Some(previous) = previous
            && let NodeKind::Text(existing) = &mut self.nodes[previous.index()].kind
        {
            self.output_capacity_hint = self.output_capacity_hint.saturating_add(normalized.len());
            super::text::merge_prose(existing.as_mut_string(), &normalized);
            return Ok(Some(previous));
        }
        self.append(parent, NodeKind::Text(TextValue::new(normalized)))
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
        self.footnote_index.insert(id, self.footnotes.len());
        self.output_capacity_hint = self.output_capacity_hint.saturating_add(label.len());
        self.footnotes.push(FootnoteRecord {
            id,
            label: label.into(),
            node,
        });
        Ok(())
    }

    pub(crate) fn finish(self) -> Document {
        let mut nodes = self.nodes;
        compact_excess_capacity(&mut nodes);
        let mut roots = self.roots;
        compact_excess_capacity(&mut roots);
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
        compact_excess_capacity(&mut footnotes);
        Document {
            nodes,
            roots,
            footnotes,
            output_capacity_hint: self.output_capacity_hint,
            stats: OnceLock::new(),
        }
    }
}

/// Releases capacity only when the retained saving is material.
///
/// Semantic compilation often preserves most DOM nodes. A reallocation does not
/// help those documents. Component-heavy code can collapse several DOM nodes into
/// one semantic leaf, so keeping the source-sized reservation would retain much
/// more memory than the document needs.
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
    fn finish_compacts_material_excess_node_capacity() {
        let mut builder = DocumentBuilder::with_capacity(1_000);
        builder.append(None, NodeKind::Paragraph).unwrap();

        let document = builder.finish();

        assert!(document.nodes.capacity() < 1_000);
    }

    #[test]
    fn finish_keeps_small_excess_capacity() {
        let mut builder = DocumentBuilder::with_capacity(20);
        builder.append(None, NodeKind::Paragraph).unwrap();

        let document = builder.finish();

        assert_eq!(document.nodes.capacity(), 20);
    }
}
