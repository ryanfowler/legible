use crate::dom::NodeId;

/// A sorted node-indexed collection for feature-local results.
///
/// Use this for rare semantic features. Shared facts and hot arbitrary-node
/// lookups should keep their dense representation.
pub(super) struct SparseNodeValues<T> {
    entries: Vec<(NodeId, T)>,
    dense_index: Option<Vec<u32>>,
}

impl<T> SparseNodeValues<T> {
    pub(super) fn new() -> Self {
        Self {
            entries: Vec::new(),
            dense_index: None,
        }
    }

    pub(super) fn with_capacity(capacity: usize) -> Self {
        Self {
            entries: Vec::with_capacity(capacity),
            dense_index: None,
        }
    }

    pub(super) fn push(&mut self, node: NodeId, value: T) {
        self.entries.push((node, value));
    }

    pub(super) fn sort(&mut self) {
        self.entries.sort_unstable_by_key(|(node, _)| *node);
        self.dense_index = None;
    }

    pub(super) fn build_dense_index_if_dense(&mut self, node_count: usize) {
        if self.entries.len().saturating_mul(2) <= node_count {
            return;
        }
        let mut index = vec![u32::MAX; node_count];
        for (slot, (node, _)) in self.entries.iter().enumerate() {
            index[node.index()] = slot as u32;
        }
        self.dense_index = Some(index);
    }

    pub(super) fn get(&self, node: NodeId) -> Option<&T> {
        if self.entries.is_empty() {
            return None;
        }
        if let Some(index) = &self.dense_index {
            let slot = *index.get(node.index())?;
            return (slot != u32::MAX).then(|| &self.entries[slot as usize].1);
        }
        self.entries
            .binary_search_by_key(&node, |(entry, _)| *entry)
            .ok()
            .map(|index| &self.entries[index].1)
    }

    pub(super) fn get_at(&self, index: usize) -> Option<&T> {
        self.entries.get(index).map(|(_, value)| value)
    }

    pub(super) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub(super) fn iter(&self) -> impl Iterator<Item = (NodeId, &T)> {
        self.entries.iter().map(|(node, value)| (*node, value))
    }

    pub(super) fn into_iter(self) -> impl Iterator<Item = (NodeId, T)> {
        self.entries.into_iter()
    }

    // Used by the benchmark-only complex storage report.
    #[allow(dead_code)]
    pub(super) fn allocated_bytes(&self) -> usize {
        self.entries
            .capacity()
            .saturating_mul(std::mem::size_of::<(NodeId, T)>())
            .saturating_add(
                self.dense_index
                    .as_ref()
                    .map_or(0, |index| index.capacity() * std::mem::size_of::<u32>()),
            )
    }
}

impl<T> Default for SparseNodeValues<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// A sorted set of source nodes for a rare feature.
pub(super) struct SparseNodeSet {
    nodes: Vec<NodeId>,
    dense_index: Option<Vec<u64>>,
}

impl SparseNodeSet {
    pub(super) fn new() -> Self {
        Self {
            nodes: Vec::new(),
            dense_index: None,
        }
    }

    pub(super) fn with_capacity(capacity: usize) -> Self {
        Self {
            nodes: Vec::with_capacity(capacity),
            dense_index: None,
        }
    }

    pub(super) fn push(&mut self, node: NodeId) {
        self.nodes.push(node);
    }

    pub(super) fn sort(&mut self) {
        self.nodes.sort_unstable();
        self.nodes.dedup();
        self.dense_index = None;
    }

    pub(super) fn build_dense_index(&mut self, node_count: usize) {
        if self.nodes.len().saturating_mul(64) <= node_count {
            return;
        }
        let mut index = vec![0; node_count.div_ceil(64)];
        for node in &self.nodes {
            let index = &mut index[node.index() / 64];
            *index |= 1_u64 << (node.index() % 64);
        }
        self.dense_index = Some(index);
        self.nodes = Vec::new();
    }

    pub(super) fn contains(&self, node: NodeId) -> bool {
        if let Some(index) = &self.dense_index {
            return index
                .get(node.index() / 64)
                .is_some_and(|value| value & (1 << (node.index() % 64)) != 0);
        }
        if self.nodes.is_empty() {
            return false;
        }
        self.nodes.binary_search(&node).is_ok()
    }

    #[cfg(test)]
    pub(super) fn iter(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.nodes.iter().copied()
    }

    // Used by the benchmark-only complex storage report.
    #[allow(dead_code)]
    pub(super) fn allocated_bytes(&self) -> usize {
        self.nodes
            .capacity()
            .saturating_mul(std::mem::size_of::<NodeId>())
            .saturating_add(
                self.dense_index
                    .as_ref()
                    .map_or(0, |index| index.capacity() * std::mem::size_of::<u64>()),
            )
    }
}

impl Default for SparseNodeSet {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sparse_values_are_node_indexed_after_sorting() {
        let mut values = SparseNodeValues::new();
        values.push(NodeId(7), "seven");
        values.push(NodeId(2), "two");
        values.sort();

        assert_eq!(values.get(NodeId(2)), Some(&"two"));
        assert_eq!(values.get(NodeId(7)), Some(&"seven"));
        assert_eq!(values.get(NodeId(3)), None);
    }

    #[test]
    fn sparse_sets_deduplicate_nodes() {
        let mut nodes = SparseNodeSet::new();
        nodes.push(NodeId(7));
        nodes.push(NodeId(2));
        nodes.push(NodeId(7));
        nodes.sort();

        assert!(nodes.contains(NodeId(2)));
        assert!(nodes.contains(NodeId(7)));
        assert!(!nodes.contains(NodeId(3)));
        assert_eq!(nodes.iter().count(), 2);
    }

    #[test]
    fn sparse_sets_can_use_a_compact_dense_lookup_index() {
        let mut nodes = SparseNodeSet::new();
        nodes.push(NodeId(65));
        nodes.sort();
        nodes.build_dense_index(66);

        assert!(nodes.contains(NodeId(65)));
        assert!(!nodes.contains(NodeId(64)));
    }
}
