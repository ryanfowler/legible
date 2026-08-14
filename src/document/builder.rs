use std::collections::HashMap;

use super::{Document, DocumentNode, DocumentNodeId, FootnoteDefinition, FootnoteId, NodeKind};
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
    nodes: Vec<DocumentNode>,
    roots: Vec<DocumentNodeId>,
    last_children: Vec<Option<DocumentNodeId>>,
    pending_spaces: Vec<bool>,
    pending_root_space: bool,
    footnotes: Vec<FootnoteDefinition>,
    footnote_index: HashMap<FootnoteId, usize>,
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
                    text.push(' ');
                }
            } else {
                self.append_raw(parent, NodeKind::Text(" ".into()))?;
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
        self.nodes.push(DocumentNode {
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
            super::text::merge_prose(existing, &normalized);
            return Ok(Some(previous));
        }
        self.append(parent, NodeKind::Text(normalized)).map(Some)
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
        self.footnotes.push(FootnoteDefinition {
            id,
            label: label.into(),
            node,
        });
        Ok(())
    }

    pub(crate) fn finish(self) -> Document {
        Document {
            nodes: self.nodes,
            roots: self.roots,
            footnotes: self.footnotes,
        }
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
