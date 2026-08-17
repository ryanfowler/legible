use crate::dom::{Dom, NodeId};

/// One source node retained after final relevance cleanup.
///
/// The depth is relative to the retained fragment root. It lets the ordinary
/// compiler close source frames from the next entry without rebuilding
/// subtree ranges or an event list.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RetainedEntry {
    pub(crate) node: NodeId,
    pub(crate) depth: u32,
    removed: bool,
}

/// Source-order entries retained by final cleanup.
pub(crate) struct RetainedStream {
    entries: Vec<RetainedEntry>,
}

impl RetainedStream {
    pub(crate) fn from_preorder(dom: &Dom, root: NodeId, nodes: &[NodeId]) -> Self {
        let mut entries = Vec::with_capacity(nodes.len());
        let mut ancestors = Vec::<NodeId>::new();

        for &node in nodes {
            let parent = dom.parent(node);
            while ancestors
                .last()
                .is_some_and(|&ancestor| parent != Some(ancestor))
            {
                ancestors.pop();
            }
            let depth = u32::try_from(ancestors.len()).unwrap_or(u32::MAX);
            entries.push(RetainedEntry {
                node,
                depth,
                removed: false,
            });
            if dom.is_element(node) {
                ancestors.push(node);
            }
        }

        debug_assert!(
            nodes
                .iter()
                .all(|&node| dom.parent(node).is_some() || node == root),
            "retained stream contains a detached source node"
        );
        Self { entries }
    }

    pub(crate) fn entries(&self) -> &[RetainedEntry] {
        &self.entries
    }

    pub(crate) fn iter_nodes(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.entries.iter().map(|entry| entry.node)
    }

    #[cfg(test)]
    pub(crate) fn iter(&self) -> impl Iterator<Item = &NodeId> {
        self.entries.iter().map(|entry| &entry.node)
    }

    pub(crate) fn prepend_root(&mut self, root: NodeId) {
        for entry in &mut self.entries {
            entry.depth = entry.depth.saturating_add(1);
        }
        self.entries.insert(
            0,
            RetainedEntry {
                node: root,
                depth: 0,
                removed: false,
            },
        );
    }

    pub(crate) fn mark_removed(&mut self, position: usize) {
        if let Some(entry) = self.entries.get_mut(position) {
            entry.removed = true;
        }
    }

    pub(crate) fn compact_removed(&mut self) {
        let mut write = 0;
        let mut removed_depth = None;
        for read in 0..self.entries.len() {
            let entry = self.entries[read];
            if removed_depth.is_some_and(|depth| entry.depth > depth) {
                continue;
            }
            removed_depth = None;
            if entry.removed {
                removed_depth = Some(entry.depth);
                continue;
            }
            self.entries[write] = entry;
            write += 1;
        }
        self.entries.truncate(write);
    }

    #[cfg(test)]
    pub(crate) fn contains(&self, node: &NodeId) -> bool {
        self.entries.iter().any(|entry| &entry.node == node)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dom::Tag;

    #[test]
    fn records_relative_depth_without_arena_positions() {
        let dom =
            Dom::parse_fragment("<div><p>one</p><p><em>two</em></p></div>", Tag::Div).unwrap();
        let root = dom.root();
        let nodes: Vec<_> = dom.descendants(root).collect();
        let stream = RetainedStream::from_preorder(&dom, root, &nodes);
        let depths: Vec<_> = stream.entries.iter().map(|entry| entry.depth).collect();
        assert_eq!(depths, [0, 1, 2, 1, 2, 3]);
    }
}
