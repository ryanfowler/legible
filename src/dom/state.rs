#[cfg(test)]
use std::mem::size_of;

const HAS_SENTENCE_END: u16 = 1 << 0;
const HAS_TEXT: u16 = 1 << 1;
const HAS_NON_WHITESPACE: u16 = 1 << 2;
const HAS_ALPHANUMERIC: u16 = 1 << 3;
const STARTS_WITH_WHITESPACE: u16 = 1 << 4;
const ENDS_WITH_WHITESPACE: u16 = 1 << 5;
const ENDS_WITH_DOT: u16 = 1 << 6;
const HAS_SENTENCE_BREAK: u16 = 1 << 7;

#[derive(Debug, Clone, Copy, Default)]
struct NodeStatsFlags(u16);

impl NodeStatsFlags {
    #[inline]
    const fn get(self, flag: u16) -> bool {
        self.0 & flag != 0
    }

    #[inline]
    fn set(&mut self, flag: u16, value: bool) {
        if value {
            self.0 |= flag;
        } else {
            self.0 &= !flag;
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct NodeStats {
    // Readability compares these values with small thresholds. Saturating
    // 32-bit counters keep the dense per-node cache compact on 64-bit targets.
    pub(crate) text_length: u32,
    pub(crate) word_count: u32,
    pub(crate) comma_count: u32,
    pub(crate) sentence_end_count: u32,
    pub(crate) alphabetic_chars: u32,
    pub(crate) digit_chars: u32,
    flags: NodeStatsFlags,
}

impl NodeStats {
    #[inline]
    pub(crate) const fn has_sentence_end(self) -> bool {
        self.flags.get(HAS_SENTENCE_END)
    }

    #[inline]
    pub(crate) const fn has_text(self) -> bool {
        self.flags.get(HAS_TEXT)
    }

    #[inline]
    pub(crate) const fn has_non_whitespace(self) -> bool {
        self.flags.get(HAS_NON_WHITESPACE)
    }

    #[inline]
    pub(crate) const fn has_alphanumeric(self) -> bool {
        self.flags.get(HAS_ALPHANUMERIC)
    }

    #[inline]
    pub(crate) const fn starts_with_whitespace(self) -> bool {
        self.flags.get(STARTS_WITH_WHITESPACE)
    }

    #[inline]
    pub(crate) const fn ends_with_whitespace(self) -> bool {
        self.flags.get(ENDS_WITH_WHITESPACE)
    }

    #[inline]
    pub(crate) const fn ends_with_dot(self) -> bool {
        self.flags.get(ENDS_WITH_DOT)
    }

    #[inline]
    pub(crate) const fn has_sentence_break(self) -> bool {
        self.flags.get(HAS_SENTENCE_BREAK)
    }

    #[inline]
    pub(crate) fn set_has_sentence_end(&mut self, value: bool) {
        self.flags.set(HAS_SENTENCE_END, value);
    }

    #[inline]
    pub(crate) fn set_has_text(&mut self, value: bool) {
        self.flags.set(HAS_TEXT, value);
    }

    #[inline]
    pub(crate) fn set_has_non_whitespace(&mut self, value: bool) {
        self.flags.set(HAS_NON_WHITESPACE, value);
    }

    #[inline]
    pub(crate) fn set_has_alphanumeric(&mut self, value: bool) {
        self.flags.set(HAS_ALPHANUMERIC, value);
    }

    #[inline]
    pub(crate) fn set_starts_with_whitespace(&mut self, value: bool) {
        self.flags.set(STARTS_WITH_WHITESPACE, value);
    }

    #[inline]
    pub(crate) fn set_ends_with_whitespace(&mut self, value: bool) {
        self.flags.set(ENDS_WITH_WHITESPACE, value);
    }

    #[inline]
    pub(crate) fn set_ends_with_dot(&mut self, value: bool) {
        self.flags.set(ENDS_WITH_DOT, value);
    }

    #[inline]
    pub(crate) fn set_has_sentence_break(&mut self, value: bool) {
        self.flags.set(HAS_SENTENCE_BREAK, value);
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) enum DataTableState {
    #[default]
    Unknown,
    Layout,
    Listing,
    Data,
}

const SCORE_INITIALIZED: u8 = 1 << 0;
const SCORE_SEEN: u8 = 1 << 1;

#[derive(Debug, Clone, Copy, Default)]
struct ScoreFlags(u8);

impl ScoreFlags {
    #[inline]
    const fn get(self, flag: u8) -> bool {
        self.0 & flag != 0
    }

    #[inline]
    fn set(&mut self, flag: u8, value: bool) {
        if value {
            self.0 |= flag;
        } else {
            self.0 &= !flag;
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct ScoreEntry {
    epoch: u32,
    content_score: f64,
    flags: ScoreFlags,
}

#[derive(Debug, Clone, Copy, Default)]
// Six u32 counters plus a u16 flag word, followed by the epoch, keep this
// dense text-stat entry at 32 bytes. Score entries are 16 bytes and table
// classifications are 12 bytes, so cold state no longer widens every score
// entry.
struct TextStatsEntry {
    epoch: u32,
    stats: NodeStats,
}

#[derive(Debug, Clone, Copy)]
struct TableStateEntry {
    node: super::NodeId,
    epoch: u32,
    state: DataTableState,
}

#[derive(Debug, Clone)]
struct LinkLengthCache {
    values: Vec<f64>,
    epochs: Vec<u32>,
    epoch: u32,
}

impl Default for LinkLengthCache {
    fn default() -> Self {
        Self {
            values: Vec::new(),
            epochs: Vec::new(),
            epoch: 1,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct NodeStateStore {
    // Score state is grown only for nodes that participate in scoring. Text
    // statistics use their own dense cache because cleanup and ranking both
    // query them for many non-candidate nodes.
    scores: Vec<ScoreEntry>,
    stats: Vec<TextStatsEntry>,
    source_stats: Vec<TextStatsEntry>,
    source_stats_enabled: bool,
    table_states: Vec<TableStateEntry>,
    table_states_sorted: bool,
    state_epoch: u32,
    stats_epoch: u32,
    source_stats_epoch: u32,
    link_lengths: Option<LinkLengthCache>,
}

/// Score state that is specific to one scoring policy.
///
/// Text statistics, link lengths, and table classifications are source facts
/// and belong to the shared scoring cache. Keep only readability scores in a
/// variant overlay so weighted and unweighted retries do not carry copies of
/// the source-sized fact cache.
#[derive(Debug, Clone)]
pub(crate) struct ScoreStore {
    scores: Vec<ScoreEntry>,
    state_epoch: u32,
}

impl Default for ScoreStore {
    fn default() -> Self {
        Self::new()
    }
}

impl ScoreStore {
    pub(crate) fn new() -> Self {
        Self {
            scores: Vec::new(),
            state_epoch: 1,
        }
    }

    fn entry(&self, id: super::NodeId) -> Option<&ScoreEntry> {
        self.scores
            .get(id.index())
            .filter(|entry| entry.epoch == self.state_epoch)
    }

    fn entry_mut(&mut self, id: super::NodeId) -> &mut ScoreEntry {
        if id.index() >= self.scores.len() {
            self.scores.resize(id.index() + 1, ScoreEntry::default());
        }
        let entry = &mut self.scores[id.index()];
        if entry.epoch != self.state_epoch {
            *entry = ScoreEntry {
                epoch: self.state_epoch,
                ..ScoreEntry::default()
            };
        }
        entry
    }

    #[inline]
    pub(crate) fn mark_seen(&mut self, id: super::NodeId) -> bool {
        let entry = self.entry_mut(id);
        if entry.flags.get(SCORE_SEEN) {
            false
        } else {
            entry.flags.set(SCORE_SEEN, true);
            true
        }
    }

    #[inline]
    pub(crate) fn has(&self, id: super::NodeId) -> bool {
        self.entry(id)
            .is_some_and(|entry| entry.flags.get(SCORE_INITIALIZED))
    }

    #[inline]
    pub(crate) fn get(&self, id: super::NodeId) -> f64 {
        self.entry(id)
            .filter(|entry| entry.flags.get(SCORE_INITIALIZED))
            .map_or(0.0, |entry| entry.content_score)
    }

    #[inline]
    pub(crate) fn get_if_initialized(&self, id: super::NodeId) -> Option<f64> {
        self.entry(id)
            .filter(|entry| entry.flags.get(SCORE_INITIALIZED))
            .map(|entry| entry.content_score)
    }

    #[inline]
    pub(crate) fn add(&mut self, id: super::NodeId, value: f64) {
        let entry = self.entry_mut(id);
        entry.content_score += value;
        entry.flags.set(SCORE_INITIALIZED, true);
    }

    #[inline]
    pub(crate) fn initialize_if_absent(&mut self, id: super::NodeId, score: f64) -> bool {
        let entry = self.entry_mut(id);
        if entry.flags.get(SCORE_INITIALIZED) {
            false
        } else {
            entry.content_score = score;
            entry.flags.set(SCORE_INITIALIZED, true);
            true
        }
    }

    #[inline]
    pub(crate) fn set(&mut self, id: super::NodeId, score: f64) {
        let entry = self.entry_mut(id);
        entry.content_score = score;
        entry.flags.set(SCORE_INITIALIZED, true);
    }
}

impl NodeStateStore {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    fn advance_epoch(epoch: &mut u32, reset: impl FnOnce()) {
        *epoch = epoch.wrapping_add(1);
        if *epoch == 0 {
            reset();
            *epoch = 1;
        }
    }

    fn sync_scores(&mut self, len: usize) {
        if len > self.scores.len() {
            self.scores.resize(len, ScoreEntry::default());
        }
    }

    fn sync_stats(&mut self, len: usize) {
        if len > self.stats.len() {
            self.stats.resize(len, TextStatsEntry::default());
        }
    }

    fn score_entry(&self, id: super::NodeId) -> Option<&ScoreEntry> {
        self.scores
            .get(id.index())
            .filter(|entry| entry.epoch == self.state_epoch)
    }

    fn score_entry_mut(&mut self, id: super::NodeId) -> &mut ScoreEntry {
        self.sync_scores(id.index() + 1);
        let entry = &mut self.scores[id.index()];
        if entry.epoch != self.state_epoch {
            *entry = ScoreEntry {
                epoch: self.state_epoch,
                ..ScoreEntry::default()
            };
        }
        entry
    }

    fn table_state(&self, id: super::NodeId) -> Option<DataTableState> {
        if self.table_states_sorted {
            self.table_states
                .binary_search_by_key(&id, |entry| entry.node)
                .ok()
                .and_then(|index| {
                    let entry = &self.table_states[index];
                    (entry.epoch == self.state_epoch).then_some(entry.state)
                })
        } else {
            self.table_states
                .iter()
                .rev()
                .find(|entry| entry.node == id && entry.epoch == self.state_epoch)
                .map(|entry| entry.state)
        }
    }

    pub(crate) fn clear(&mut self) {
        Self::advance_epoch(&mut self.state_epoch, || {
            for entry in &mut self.scores {
                *entry = ScoreEntry::default();
            }
            for entry in &mut self.table_states {
                entry.epoch = 0;
            }
        });
        self.clear_stats();
        Self::advance_epoch(&mut self.source_stats_epoch, || {
            for entry in &mut self.source_stats {
                entry.epoch = 0;
            }
        });
        self.link_lengths = None;
        self.source_stats_enabled = false;
    }

    pub(crate) fn clear_stats(&mut self) {
        Self::advance_epoch(&mut self.stats_epoch, || {
            for entry in &mut self.stats {
                entry.epoch = 0;
            }
        });
        if let Some(cache) = &mut self.link_lengths {
            Self::advance_epoch(&mut cache.epoch, || {
                cache.epochs.fill(0);
            });
        }
    }

    #[allow(dead_code)]
    pub(crate) fn mark_score_seen(&mut self, id: super::NodeId) -> bool {
        let entry = self.score_entry_mut(id);
        if entry.flags.get(SCORE_SEEN) {
            false
        } else {
            entry.flags.set(SCORE_SEEN, true);
            true
        }
    }

    pub(crate) fn get_stats(&self, id: super::NodeId) -> Option<&NodeStats> {
        self.stats
            .get(id.index())
            .filter(|entry| entry.epoch == self.stats_epoch)
            .map(|entry| &entry.stats)
    }

    pub(crate) fn get_source_stats(&self, id: super::NodeId) -> Option<&NodeStats> {
        self.source_stats
            .get(id.index())
            .filter(|entry| entry.epoch == self.source_stats_epoch)
            .map(|entry| &entry.stats)
    }

    pub(crate) fn get_stats_or_source(&self, id: super::NodeId) -> Option<&NodeStats> {
        self.get_stats(id).or_else(|| self.get_source_stats(id))
    }

    pub(crate) fn enable_source_stats(&mut self) {
        Self::advance_epoch(&mut self.source_stats_epoch, || {
            for entry in &mut self.source_stats {
                entry.epoch = 0;
            }
        });
        self.source_stats_enabled = true;
    }

    pub(crate) fn source_stats_enabled(&self) -> bool {
        self.source_stats_enabled
    }

    pub(crate) fn disable_source_stats(&mut self) {
        self.source_stats_enabled = false;
    }

    pub(crate) fn set_stats(&mut self, id: super::NodeId, stats: NodeStats) {
        self.sync_stats(id.index() + 1);
        self.stats[id.index()] = TextStatsEntry {
            epoch: self.stats_epoch,
            stats,
        };
    }

    pub(crate) fn set_source_stats(&mut self, id: super::NodeId, stats: NodeStats) {
        if id.index() >= self.source_stats.len() {
            self.source_stats
                .resize(id.index() + 1, TextStatsEntry::default());
        }
        self.source_stats[id.index()] = TextStatsEntry {
            epoch: self.source_stats_epoch,
            stats,
        };
    }

    /// Forces the next `get_stats` call to recompute stats for this node.
    /// Does not reset link lengths; callers must handle those separately
    /// if the node's link contribution has changed.
    pub(crate) fn invalidate_stats(&mut self, id: super::NodeId) {
        self.sync_stats(id.index() + 1);
        self.stats[id.index()].epoch = 0;
    }

    pub(crate) fn enable_link_lengths(&mut self) {
        self.link_lengths
            .get_or_insert_with(LinkLengthCache::default);
    }

    pub(crate) fn link_lengths_enabled(&self) -> bool {
        self.link_lengths.is_some()
    }

    pub(crate) fn link_length(&self, id: super::NodeId) -> f64 {
        let Some(cache) = &self.link_lengths else {
            return 0.0;
        };
        if cache.epochs.get(id.index()) == Some(&cache.epoch) {
            cache.values.get(id.index()).copied().unwrap_or(0.0)
        } else {
            0.0
        }
    }

    pub(crate) fn set_link_length(&mut self, id: super::NodeId, length: f64) {
        let Some(cache) = &mut self.link_lengths else {
            return;
        };
        if id.index() >= cache.values.len() {
            let new_len = id.index() + 1;
            cache.values.resize(new_len, 0.0);
            cache.epochs.resize(new_len, 0);
        }
        cache.values[id.index()] = length;
        cache.epochs[id.index()] = cache.epoch;
    }

    #[cfg(test)]
    pub(crate) fn has(&self, id: super::NodeId) -> bool {
        self.score_entry(id)
            .is_some_and(|entry| entry.flags.get(SCORE_INITIALIZED))
    }

    #[allow(dead_code)]
    pub(crate) fn get_content_score(&self, id: super::NodeId) -> f64 {
        self.score_entry(id)
            .filter(|entry| entry.flags.get(SCORE_INITIALIZED))
            .map_or(0.0, |entry| entry.content_score)
    }

    #[allow(dead_code)]
    pub(crate) fn get_content_score_if_initialized(&self, id: super::NodeId) -> Option<f64> {
        self.score_entry(id)
            .filter(|entry| entry.flags.get(SCORE_INITIALIZED))
            .map(|entry| entry.content_score)
    }

    #[allow(dead_code)]
    pub(crate) fn add_content_score(&mut self, id: super::NodeId, value: f64) {
        let entry = self.score_entry_mut(id);
        entry.content_score += value;
        entry.flags.set(SCORE_INITIALIZED, true);
    }

    #[cfg(test)]
    pub(crate) fn initialize_if_absent(&mut self, id: super::NodeId, score: f64) -> bool {
        let entry = self.score_entry_mut(id);
        if entry.flags.get(SCORE_INITIALIZED) {
            false
        } else {
            entry.content_score = score;
            entry.flags.set(SCORE_INITIALIZED, true);
            true
        }
    }

    #[allow(dead_code)]
    pub(crate) fn set_score(&mut self, id: super::NodeId, score: f64) {
        let entry = self.score_entry_mut(id);
        entry.content_score = score;
        entry.flags.set(SCORE_INITIALIZED, true);
    }

    pub(crate) fn set_data_table(&mut self, id: super::NodeId, state: DataTableState) {
        self.table_states.push(TableStateEntry {
            node: id,
            epoch: self.state_epoch,
            state,
        });
        self.table_states_sorted = false;
    }

    pub(crate) fn finish_data_tables(&mut self) {
        if !self.table_states_sorted {
            self.table_states.sort_by_key(|entry| entry.node);
            let mut unique = 0;
            for index in 0..self.table_states.len() {
                if unique > 0 && self.table_states[unique - 1].node == self.table_states[index].node
                {
                    // Stable sorting keeps the latest write last for duplicate
                    // table classifications.
                    self.table_states[unique - 1] = self.table_states[index];
                } else {
                    self.table_states[unique] = self.table_states[index];
                    unique += 1;
                }
            }
            self.table_states.truncate(unique);
            self.table_states_sorted = true;
        }
    }

    pub(crate) fn is_data_table(&self, id: super::NodeId) -> Option<bool> {
        match self.table_state(id) {
            Some(DataTableState::Data) => Some(true),
            Some(DataTableState::Layout | DataTableState::Listing) => Some(false),
            Some(DataTableState::Unknown) | None => None,
        }
    }
}

impl Default for NodeStateStore {
    fn default() -> Self {
        Self {
            scores: Vec::new(),
            stats: Vec::new(),
            source_stats: Vec::new(),
            source_stats_enabled: false,
            table_states: Vec::new(),
            table_states_sorted: true,
            state_epoch: 1,
            stats_epoch: 1,
            source_stats_epoch: 1,
            link_lengths: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_layout_is_split_and_compact() {
        assert_eq!(size_of::<NodeStats>(), 28);
        assert_eq!(size_of::<TextStatsEntry>(), 32);
        assert_eq!(size_of::<ScoreEntry>(), 16);
        assert_eq!(size_of::<TableStateEntry>(), 12);
    }

    #[test]
    fn clear_invalidates_each_state_without_filling_storage() {
        let id = super::super::NodeId(3);
        let mut store = NodeStateStore::new();
        store.enable_link_lengths();
        store.set_stats(id, NodeStats::default());
        store.set_link_length(id, 12.0);
        store.set_score(id, 7.0);
        store.set_data_table(id, DataTableState::Data);

        for _ in 0..4 {
            store.clear();

            assert!(store.get_stats(id).is_none());
            assert_eq!(store.link_length(id), 0.0);
            assert!(!store.has(id));
            assert_eq!(store.is_data_table(id), None);

            store.set_stats(id, NodeStats::default());
            store.set_link_length(id, 12.0);
            store.set_score(id, 7.0);
            store.set_data_table(id, DataTableState::Data);
        }
    }

    #[test]
    fn clear_stats_preserves_scores_and_table_state() {
        let id = super::super::NodeId(1);
        let mut store = NodeStateStore::new();
        store.enable_link_lengths();
        store.set_stats(id, NodeStats::default());
        store.set_link_length(id, 4.0);
        store.set_score(id, 9.0);
        store.set_data_table(id, DataTableState::Data);

        store.clear_stats();

        assert!(store.get_stats(id).is_none());
        assert_eq!(store.link_length(id), 0.0);
        assert_eq!(store.get_content_score(id), 9.0);
        assert_eq!(store.is_data_table(id), Some(true));
    }

    #[test]
    fn unordered_table_updates_finalize_to_binary_search_storage() {
        let mut store = NodeStateStore::new();
        let first = super::super::NodeId(8);
        let second = super::super::NodeId(2);
        let third = super::super::NodeId(5);
        store.set_data_table(first, DataTableState::Data);
        store.set_data_table(second, DataTableState::Data);
        store.set_data_table(second, DataTableState::Layout);
        store.set_data_table(third, DataTableState::Listing);

        assert_eq!(store.is_data_table(second), Some(false));
        store.finish_data_tables();

        assert!(store.table_states_sorted);
        assert_eq!(store.is_data_table(first), Some(true));
        assert_eq!(store.is_data_table(second), Some(false));
        assert_eq!(store.is_data_table(third), Some(false));
    }

    #[test]
    fn epoch_wrap_resets_state() {
        let id = super::super::NodeId(0);
        let mut store = NodeStateStore::new();
        store.set_score(id, 3.0);
        store.state_epoch = u32::MAX;

        store.clear();

        assert!(!store.has(id));
        assert_eq!(store.state_epoch, 1);
    }

    #[test]
    fn score_seen_and_link_epochs_reset_without_losing_current_scores() {
        let id = super::super::NodeId(2);
        let mut store = NodeStateStore::new();
        store.enable_link_lengths();
        assert!(store.mark_score_seen(id));
        assert!(!store.mark_score_seen(id));
        store.set_score(id, 5.0);
        store.set_stats(id, NodeStats::default());
        store.set_link_length(id, 8.0);
        store.set_data_table(id, DataTableState::Data);

        store.stats_epoch = u32::MAX;
        store.link_lengths.as_mut().unwrap().epoch = u32::MAX;
        store.clear_stats();

        assert!(!store.mark_score_seen(id));
        assert!(store.has(id));
        assert_eq!(store.get_content_score(id), 5.0);
        assert_eq!(store.is_data_table(id), Some(true));
        assert!(store.get_stats(id).is_none());
        assert_eq!(store.link_length(id), 0.0);
    }
}
