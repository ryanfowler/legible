//! Node wrapper and readability data attachment.

use dom_query::NodeId;
use hashbrown::HashMap;

/// Readability data attached to nodes during processing.
#[derive(Debug, Clone, Default)]
pub struct ReadabilityData {
    /// The content score for this node.
    pub content_score: f64,
}

/// Cached text statistics for a node to avoid repeated computation.
#[derive(Debug, Clone, Copy, Default)]
pub struct NodeStats {
    /// Character count of normalized inner text.
    pub text_length: usize,
    /// Number of commas in the text (raw count, not +1).
    pub comma_count: usize,
    /// Whether the text ends with sentence-ending punctuation.
    pub has_sentence_end: bool,
    pub(crate) has_text: bool,
    pub(crate) has_non_whitespace: bool,
    pub(crate) starts_with_whitespace: bool,
    pub(crate) ends_with_whitespace: bool,
    pub(crate) ends_with_dot: bool,
    pub(crate) has_sentence_break: bool,
}

impl ReadabilityData {
    /// Create a new ReadabilityData with an initial score.
    pub fn with_score(score: f64) -> Self {
        Self {
            content_score: score,
        }
    }
}

/// Storage for node-attached readability data.
/// Since we can't attach data directly to DOM nodes like JavaScript can,
/// we use a HashMap keyed by NodeId.
#[derive(Debug, Default)]
pub struct NodeDataStore {
    data: HashMap<NodeId, ReadabilityData>,
    /// Track which tables are data tables (vs layout tables).
    data_tables: HashMap<NodeId, bool>,
    /// Cached text statistics for nodes.
    stats: HashMap<NodeId, NodeStats>,
}

impl NodeDataStore {
    /// Create a new empty NodeDataStore.
    pub fn new() -> Self {
        Self {
            data: HashMap::new(),
            data_tables: HashMap::new(),
            stats: HashMap::new(),
        }
    }

    /// Get the readability data for a node, if it exists.
    pub fn get(&self, node_id: &NodeId) -> Option<&ReadabilityData> {
        self.data.get(node_id)
    }

    /// Get mutable readability data for a node, if it exists.
    pub fn get_mut(&mut self, node_id: &NodeId) -> Option<&mut ReadabilityData> {
        self.data.get_mut(node_id)
    }

    /// Set the readability data for a node.
    pub fn set(&mut self, node_id: NodeId, data: ReadabilityData) {
        self.data.insert(node_id, data);
    }

    /// Check if a node has readability data.
    pub fn has(&self, node_id: &NodeId) -> bool {
        self.data.contains_key(node_id)
    }

    /// Get the content score for a node.
    pub fn get_content_score(&self, node_id: &NodeId) -> f64 {
        self.data
            .get(node_id)
            .map(|d| d.content_score)
            .unwrap_or(0.0)
    }

    /// Add to the content score of a node.
    pub fn add_content_score(&mut self, node_id: NodeId, score: f64) {
        let data = self.data.entry(node_id).or_default();
        data.content_score += score;
    }

    /// Initialize a node's readability data if it doesn't already exist.
    /// Uses the entry API for a single HashMap lookup.
    /// Returns true if the node was newly initialized, false if it already existed.
    pub fn initialize_if_absent(&mut self, node_id: NodeId, data: ReadabilityData) -> bool {
        match self.data.entry(node_id) {
            hashbrown::hash_map::Entry::Occupied(_) => false,
            hashbrown::hash_map::Entry::Vacant(entry) => {
                entry.insert(data);
                true
            }
        }
    }

    /// Set whether a table is a data table.
    pub fn set_data_table(&mut self, node_id: NodeId, is_data_table: bool) {
        self.data_tables.insert(node_id, is_data_table);
    }

    /// Check if a table is marked as a data table.
    pub fn is_data_table(&self, node_id: &NodeId) -> Option<bool> {
        self.data_tables.get(node_id).copied()
    }

    /// Clear all stored data.
    pub fn clear(&mut self) {
        self.data.clear();
        self.data_tables.clear();
        self.stats.clear();
    }

    /// Clear cached text statistics after structural DOM mutations.
    ///
    /// Readability scores and data-table classifications remain valid and are
    /// intentionally preserved.
    pub fn clear_stats(&mut self) {
        self.stats.clear();
    }

    /// Get the cached stats for a node, if they exist.
    pub fn get_stats(&self, node_id: &NodeId) -> Option<&NodeStats> {
        self.stats.get(node_id)
    }

    /// Set the cached stats for a node.
    pub fn set_stats(&mut self, node_id: NodeId, stats: NodeStats) {
        self.stats.insert(node_id, stats);
    }
}
