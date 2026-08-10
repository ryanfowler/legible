#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct NodeStats {
    // Readability compares these values with small thresholds. Saturating
    // 32-bit counters keep the dense per-node cache compact on 64-bit targets.
    pub(crate) text_length: u32,
    pub(crate) word_count: u32,
    pub(crate) comma_count: u32,
    pub(crate) sentence_end_count: u32,
    pub(crate) has_sentence_end: bool,
    pub(crate) has_text: bool,
    pub(crate) has_non_whitespace: bool,
    pub(crate) starts_with_whitespace: bool,
    pub(crate) ends_with_whitespace: bool,
    pub(crate) ends_with_dot: bool,
    pub(crate) has_sentence_break: bool,
}
#[derive(Debug, Clone, Copy, Default)]
pub(crate) enum DataTableState {
    #[default]
    Unknown,
    Layout,
    Data,
}
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct NodeState {
    pub(crate) content_score: f64,
    pub(crate) score_initialized: bool,
    score_seen: bool,
    pub(crate) data_table: DataTableState,
    pub(crate) stats: NodeStats,
    pub(crate) stats_epoch: u32,
}
#[derive(Debug, Default)]
pub(crate) struct NodeStateStore {
    entries: Vec<NodeState>,
    stats_epoch: u32,
    link_lengths: Option<Vec<f64>>,
}
impl NodeStateStore {
    pub(crate) fn new() -> Self {
        Self {
            stats_epoch: 1,
            ..Self::default()
        }
    }
    pub(crate) fn sync_len(&mut self, len: usize) {
        if len > self.entries.len() {
            self.entries.resize(len, NodeState::default())
        }
    }
    pub(crate) fn clear(&mut self) {
        self.entries
            .iter_mut()
            .for_each(|e| *e = NodeState::default());
        self.stats_epoch = 1;
        self.link_lengths = None;
    }
    pub(crate) fn clear_stats(&mut self) {
        self.stats_epoch = self.stats_epoch.wrapping_add(1);
        if let Some(lengths) = &mut self.link_lengths {
            lengths.fill(0.0);
        }
        if self.stats_epoch == 0 {
            self.stats_epoch = 1;
            for e in &mut self.entries {
                e.stats_epoch = 0
            }
        }
    }
    pub(crate) fn mark_score_seen(&mut self, id: super::NodeId) -> bool {
        self.sync_len(id.index() + 1);
        let entry = &mut self.entries[id.index()];
        if entry.score_seen {
            false
        } else {
            entry.score_seen = true;
            true
        }
    }
    pub(crate) fn get_stats(&self, id: super::NodeId) -> Option<&NodeStats> {
        self.entries
            .get(id.index())
            .filter(|e| e.stats_epoch == self.stats_epoch)
            .map(|e| &e.stats)
    }
    pub(crate) fn set_stats(&mut self, id: super::NodeId, s: NodeStats) {
        self.sync_len(id.index() + 1);
        self.entries[id.index()].stats = s;
        self.entries[id.index()].stats_epoch = self.stats_epoch
    }
    pub(crate) fn enable_link_lengths(&mut self) {
        self.link_lengths
            .get_or_insert_with(|| vec![0.0; self.entries.len()]);
    }
    pub(crate) fn link_lengths_enabled(&self) -> bool {
        self.link_lengths.is_some()
    }
    pub(crate) fn link_length(&self, id: super::NodeId) -> f64 {
        self.link_lengths
            .as_ref()
            .and_then(|lengths| lengths.get(id.index()))
            .copied()
            .unwrap_or(0.0)
    }
    pub(crate) fn set_link_length(&mut self, id: super::NodeId, length: f64) {
        let Some(lengths) = &mut self.link_lengths else {
            return;
        };
        if id.index() >= lengths.len() {
            lengths.resize(id.index() + 1, 0.0);
        }
        lengths[id.index()] = length;
    }
    pub(crate) fn get(&self, id: super::NodeId) -> Option<&NodeState> {
        self.entries.get(id.index()).filter(|e| e.score_initialized)
    }
    pub(crate) fn has(&self, id: super::NodeId) -> bool {
        self.get(id).is_some()
    }
    pub(crate) fn get_content_score(&self, id: super::NodeId) -> f64 {
        self.get(id).map_or(0.0, |e| e.content_score)
    }
    pub(crate) fn add_content_score(&mut self, id: super::NodeId, v: f64) {
        self.sync_len(id.index() + 1);
        let e = &mut self.entries[id.index()];
        e.content_score += v;
        e.score_initialized = true
    }
    pub(crate) fn initialize_if_absent(&mut self, id: super::NodeId, score: f64) -> bool {
        self.sync_len(id.index() + 1);
        let e = &mut self.entries[id.index()];
        if e.score_initialized {
            false
        } else {
            e.content_score = score;
            e.score_initialized = true;
            true
        }
    }
    pub(crate) fn set_score(&mut self, id: super::NodeId, score: f64) {
        self.sync_len(id.index() + 1);
        let e = &mut self.entries[id.index()];
        e.content_score = score;
        e.score_initialized = true
    }
    pub(crate) fn set_data_table(&mut self, id: super::NodeId, state: DataTableState) {
        self.sync_len(id.index() + 1);
        self.entries[id.index()].data_table = state
    }
    pub(crate) fn is_data_table(&self, id: super::NodeId) -> Option<bool> {
        let e = self.entries.get(id.index())?;
        match e.data_table {
            DataTableState::Data => Some(true),
            DataTableState::Layout => Some(false),
            DataTableState::Unknown => None,
        }
    }
}
