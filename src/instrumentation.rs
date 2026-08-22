//! Optional extraction measurements used by the benchmark reporting tools.
//!
//! The default build keeps the state thread-local and all measurement methods
//! empty. The `bench-instrumentation` feature enables the counters and phase
//! timers. The reporting binary owns the global allocator wrapper.

#[cfg(feature = "bench-instrumentation")]
use std::time::Instant;

#[cfg(feature = "bench-instrumentation")]
const PHASE_COUNT: usize = 10;

/// A measured extraction phase.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(usize)]
pub enum Phase {
    /// HTML parsing, including fragment parsing during extraction.
    Parse = 0,
    /// Metadata and structured-data discovery.
    Metadata = 1,
    /// Source preparation and specialized extraction.
    Preparation = 2,
    /// Candidate discovery and source-only indexing.
    CandidateDiscovery = 3,
    /// Readability preparation, score propagation, and ranking.
    Scoring = 4,
    /// Root selection and boundary construction.
    RootSelection = 5,
    /// Copying the selected source subtree into a fragment.
    FragmentCopy = 6,
    /// Cleanup and retained-source preparation.
    Cleanup = 7,
    /// Semantic compilation into the private event tape.
    SemanticCompilation = 8,
    /// Lazy output rendering.
    Rendering = 9,
}

/// Identifies the cleanup phase that requested a fragment snapshot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(usize)]
pub enum SnapshotKind {
    /// Initial selected-fragment snapshot.
    CleanupSelectedInitial = 0,
    /// Snapshot after heading selection.
    CleanupSelectedHeading = 1,
    /// Snapshot during media preparation.
    MediaPreparation = 2,
    /// Snapshot during decorative-media cleanup.
    DecorativeMedia = 3,
    /// Snapshot during style cleanup.
    StyleCleanup = 4,
    /// Snapshot during table classification.
    TableClassification = 5,
    /// Snapshot during hard cleanup.
    HardCleanup = 6,
    /// Initial heuristic-cleanup snapshot.
    HeuristicInitial = 7,
    /// Snapshot after explicit heuristic cleanup.
    HeuristicAfterExplicit = 8,
    /// Snapshot before taxonomy cleanup.
    HeuristicBeforeTaxonomy = 9,
    /// Snapshot before job-profile cleanup.
    HeuristicBeforeJob = 10,
    /// Snapshot after job-profile cleanup.
    HeuristicAfterJob = 11,
    /// Snapshot before direct-content cleanup.
    HeuristicBeforeDirect = 12,
    /// Final heuristic-cleanup snapshot.
    HeuristicFinal = 13,
    /// Snapshot during global document-chrome cleanup.
    GlobalChrome = 14,
    /// Snapshot during inline document-chrome cleanup.
    InlineChrome = 15,
    /// Initial repeated-content snapshot.
    RepeatedInitial = 16,
    /// Snapshot after comment cleanup.
    RepeatedAfterComments = 17,
    /// Snapshot after document-chrome cleanup.
    RepeatedAfterChrome = 18,
    /// Snapshot after link cleanup.
    RepeatedAfterLinks = 19,
    /// Snapshot during final normalization.
    FinalNormalization = 20,
}

#[cfg(feature = "bench-instrumentation")]
const SNAPSHOT_KIND_COUNT: usize = SnapshotKind::FinalNormalization as usize + 1;

impl SnapshotKind {
    /// Returns all snapshot kinds in stable report order.
    #[cfg(feature = "bench-instrumentation")]
    pub fn all() -> &'static [Self] {
        &[
            Self::CleanupSelectedInitial,
            Self::CleanupSelectedHeading,
            Self::MediaPreparation,
            Self::DecorativeMedia,
            Self::StyleCleanup,
            Self::TableClassification,
            Self::HardCleanup,
            Self::HeuristicInitial,
            Self::HeuristicAfterExplicit,
            Self::HeuristicBeforeTaxonomy,
            Self::HeuristicBeforeJob,
            Self::HeuristicAfterJob,
            Self::HeuristicBeforeDirect,
            Self::HeuristicFinal,
            Self::GlobalChrome,
            Self::InlineChrome,
            Self::RepeatedInitial,
            Self::RepeatedAfterComments,
            Self::RepeatedAfterChrome,
            Self::RepeatedAfterLinks,
            Self::FinalNormalization,
        ]
    }

    /// Returns the stable machine-readable snapshot kind name.
    #[cfg(feature = "bench-instrumentation")]
    pub fn name(self) -> &'static str {
        match self {
            Self::CleanupSelectedInitial => "cleanup_selected_initial",
            Self::CleanupSelectedHeading => "cleanup_selected_heading",
            Self::MediaPreparation => "media_preparation",
            Self::DecorativeMedia => "decorative_media",
            Self::StyleCleanup => "style_cleanup",
            Self::TableClassification => "table_classification",
            Self::HardCleanup => "hard_cleanup",
            Self::HeuristicInitial => "heuristic_initial",
            Self::HeuristicAfterExplicit => "heuristic_after_explicit",
            Self::HeuristicBeforeTaxonomy => "heuristic_before_taxonomy",
            Self::HeuristicBeforeJob => "heuristic_before_job",
            Self::HeuristicAfterJob => "heuristic_after_job",
            Self::HeuristicBeforeDirect => "heuristic_before_direct",
            Self::HeuristicFinal => "heuristic_final",
            Self::GlobalChrome => "global_chrome",
            Self::InlineChrome => "inline_chrome",
            Self::RepeatedInitial => "repeated_initial",
            Self::RepeatedAfterComments => "repeated_after_comments",
            Self::RepeatedAfterChrome => "repeated_after_chrome",
            Self::RepeatedAfterLinks => "repeated_after_links",
            Self::FinalNormalization => "final_normalization",
        }
    }
}

impl Phase {
    #[cfg(feature = "bench-instrumentation")]
    const ALL: [Self; PHASE_COUNT] = [
        Self::Parse,
        Self::Metadata,
        Self::Preparation,
        Self::CandidateDiscovery,
        Self::Scoring,
        Self::RootSelection,
        Self::FragmentCopy,
        Self::Cleanup,
        Self::SemanticCompilation,
        Self::Rendering,
    ];

    /// Returns all phases in stable report order.
    #[cfg(feature = "bench-instrumentation")]
    pub fn all() -> &'static [Self] {
        &Self::ALL
    }

    /// Returns the stable machine-readable phase name.
    #[cfg(feature = "bench-instrumentation")]
    pub fn name(self) -> &'static str {
        match self {
            Self::Parse => "parse",
            Self::Metadata => "metadata",
            Self::Preparation => "preparation",
            Self::CandidateDiscovery => "candidate_discovery",
            Self::Scoring => "scoring",
            Self::RootSelection => "root_selection",
            Self::FragmentCopy => "fragment_copy",
            Self::Cleanup => "cleanup",
            Self::SemanticCompilation => "semantic_compilation",
            Self::Rendering => "rendering",
        }
    }
}

/// Counts collected for one thread's instrumented extraction run.
#[cfg(feature = "bench-instrumentation")]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ExtractionCounters {
    /// Number of source and fragment parse calls.
    pub parse_calls: u64,
    /// Number of full source scans.
    pub source_full_scans: u64,
    /// Number of source element snapshots.
    pub source_element_snapshots: u64,
    /// Number of fragment snapshot rebuilds.
    pub fragment_snapshot_rebuilds: u64,
    /// Number of fragment snapshot requests.
    pub fragment_snapshot_requests: u64,
    /// Number of fragment snapshot requests served from a cache.
    pub fragment_snapshot_reuses: u64,
    /// Number of nodes in rebuilt fragment snapshots.
    pub fragment_snapshot_nodes: u64,
    /// Number of nodes covered by reused fragment snapshots.
    pub fragment_snapshot_reuse_nodes: u64,
    /// Rebuilt snapshot counts by snapshot kind.
    pub fragment_snapshot_by_kind: [u64; SNAPSHOT_KIND_COUNT],
    /// Reused snapshot counts by snapshot kind.
    pub fragment_snapshot_reuses_by_kind: [u64; SNAPSHOT_KIND_COUNT],
    /// Rebuilt snapshot node counts by snapshot kind.
    pub fragment_snapshot_nodes_by_kind: [u64; SNAPSHOT_KIND_COUNT],
    /// Reused snapshot node counts by snapshot kind.
    pub fragment_snapshot_reuse_nodes_by_kind: [u64; SNAPSHOT_KIND_COUNT],
    /// Number of prepared source analyses.
    pub prepared_source_builds: u64,
    /// Number of entries in prepared source analyses.
    pub prepared_source_entries: u64,
    /// Number of DOM clones.
    pub dom_clones: u64,
    /// Number of selected-fragment copies.
    pub fragment_copies: u64,
    /// Number of extraction strategies started.
    pub strategies_started: u64,
    /// Number of logical attempt plans.
    pub logical_attempt_plans: u64,
    /// Number of physical attempt executions.
    pub physical_attempt_executions: u64,
    /// Number of duplicate attempts removed.
    pub deduplicated_attempts: u64,
    /// Number of unique attempt plans.
    pub unique_attempt_plans: u64,
    /// Number of nodes scored.
    pub scoring_nodes: u64,
    /// Number of cleaned nodes.
    pub cleaned_nodes: u64,
    /// Number of semantic source nodes.
    pub semantic_source_nodes: u64,
    /// Number of emitted semantic operations.
    pub semantic_operations: u64,
    /// Number of allocations.
    pub allocations: u64,
    /// Total allocated bytes.
    pub allocated_bytes: u64,
    /// Number of deallocations.
    pub deallocations: u64,
    /// Total deallocated bytes.
    pub deallocated_bytes: u64,
    /// Peak live bytes for the run.
    pub peak_live_bytes: u64,
    /// Live bytes at the end of the run.
    pub final_live_bytes: u64,
    /// Estimated bytes retained by the final semantic document.
    pub final_retained_bytes: u64,
    /// Bytes copied while cloning DOM data.
    pub dom_clone_bytes: u64,
    /// Final operation-vector capacity.
    pub builder_ops_capacity: u64,
    /// Final close-operation capacity.
    pub builder_ends_capacity: u64,
    /// Final open-stack capacity.
    pub builder_open_capacity: u64,
    /// Final semantic-text capacity.
    pub builder_text_capacity: u64,
    /// Final payload capacity.
    pub builder_payload_capacity: u64,
    /// Final footnote capacity.
    pub builder_footnotes_capacity: u64,
    /// Final footnote-index capacity.
    pub builder_footnote_index_capacity: u64,
    /// Capacity requested by the semantic builder in bytes.
    pub builder_requested_capacity_bytes: u64,
    /// Final capacity of the semantic builder in bytes.
    pub builder_final_capacity_bytes: u64,
    /// Peak capacity of the semantic builder in bytes.
    pub builder_peak_capacity_bytes: u64,
    /// Number of semantic-builder reallocations.
    pub builder_reallocations: u64,
    /// Maximum open-container depth in the semantic builder.
    pub builder_max_open_depth: u64,
    /// Bytes released by semantic-builder shrinking.
    pub builder_shrink_bytes: u64,
    /// JSON-LD source bytes scanned.
    pub json_ld_bytes: u64,
    /// Estimated parsed JSON-LD bytes.
    pub json_ld_parsed_bytes: u64,
    /// Estimated retained JSON-LD bytes.
    pub json_ld_retained_bytes: u64,
}

/// Counts for source-pipeline work that is deferred or skipped.
#[cfg(feature = "bench-instrumentation")]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DeferredWorkCounters {
    /// Number of content-hint scans.
    pub content_hint_scans: u64,
    /// Number of content-excerpt scans.
    pub content_excerpt_scans: u64,
    /// Number of final-DOM node scans.
    pub final_dom_node_scans: u64,
    /// Number of external-footnote scans.
    pub external_footnote_scans: u64,
}

/// Accumulated phase time in nanoseconds.
#[cfg(feature = "bench-instrumentation")]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PhaseDurations {
    /// Time spent parsing, in nanoseconds.
    pub parse: u64,
    /// Time spent discovering metadata, in nanoseconds.
    pub metadata: u64,
    /// Time spent preparing the source, in nanoseconds.
    pub preparation: u64,
    /// Time spent discovering candidates, in nanoseconds.
    pub candidate_discovery: u64,
    /// Time spent scoring candidates, in nanoseconds.
    pub scoring: u64,
    /// Time spent selecting the root, in nanoseconds.
    pub root_selection: u64,
    /// Time spent copying the selected fragment, in nanoseconds.
    pub fragment_copy: u64,
    /// Time spent cleaning the fragment, in nanoseconds.
    pub cleanup: u64,
    /// Time spent compiling semantic content, in nanoseconds.
    pub semantic_compilation: u64,
    /// Time spent rendering output, in nanoseconds.
    pub rendering: u64,
}

#[cfg(feature = "bench-instrumentation")]
impl PhaseDurations {
    /// Returns the duration for a phase in stable report order.
    #[cfg(feature = "bench-instrumentation")]
    pub fn get(self, phase: Phase) -> u64 {
        match phase {
            Phase::Parse => self.parse,
            Phase::Metadata => self.metadata,
            Phase::Preparation => self.preparation,
            Phase::CandidateDiscovery => self.candidate_discovery,
            Phase::Scoring => self.scoring,
            Phase::RootSelection => self.root_selection,
            Phase::FragmentCopy => self.fragment_copy,
            Phase::Cleanup => self.cleanup,
            Phase::SemanticCompilation => self.semantic_compilation,
            Phase::Rendering => self.rendering,
        }
    }
}

/// A complete snapshot for one instrumented run.
#[cfg(feature = "bench-instrumentation")]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct InstrumentationSnapshot {
    /// Counters collected for the run.
    pub counters: ExtractionCounters,
    /// Phase durations collected for the run.
    pub phases: PhaseDurations,
}

#[cfg(feature = "bench-instrumentation")]
#[derive(Clone, Copy, Default)]
struct State {
    counters: ExtractionCounters,
    deferred_work: DeferredWorkCounters,
    phases: [u64; PHASE_COUNT],
    live_bytes: u64,
    allocation_baseline: u64,
    attempt_plan_mask: u8,
}

#[cfg(feature = "bench-instrumentation")]
std::thread_local! {
    static STATE: std::cell::Cell<State> = const { std::cell::Cell::new(State {
        counters: ExtractionCounters {
            parse_calls: 0,
            source_full_scans: 0,
            source_element_snapshots: 0,
            fragment_snapshot_rebuilds: 0,
            fragment_snapshot_requests: 0,
            fragment_snapshot_reuses: 0,
            fragment_snapshot_nodes: 0,
            fragment_snapshot_reuse_nodes: 0,
            fragment_snapshot_by_kind: [0; SNAPSHOT_KIND_COUNT],
            fragment_snapshot_reuses_by_kind: [0; SNAPSHOT_KIND_COUNT],
            fragment_snapshot_nodes_by_kind: [0; SNAPSHOT_KIND_COUNT],
            fragment_snapshot_reuse_nodes_by_kind: [0; SNAPSHOT_KIND_COUNT],
            prepared_source_builds: 0,
            prepared_source_entries: 0,
            dom_clones: 0,
            fragment_copies: 0,
            strategies_started: 0,
            logical_attempt_plans: 0,
            physical_attempt_executions: 0,
            deduplicated_attempts: 0,
            unique_attempt_plans: 0,
            scoring_nodes: 0,
            cleaned_nodes: 0,
            semantic_source_nodes: 0,
            semantic_operations: 0,
            allocations: 0,
            allocated_bytes: 0,
            deallocations: 0,
            deallocated_bytes: 0,
            peak_live_bytes: 0,
            final_live_bytes: 0,
            final_retained_bytes: 0,
            dom_clone_bytes: 0,
            builder_ops_capacity: 0,
            builder_ends_capacity: 0,
            builder_open_capacity: 0,
            builder_text_capacity: 0,
            builder_payload_capacity: 0,
            builder_footnotes_capacity: 0,
            builder_footnote_index_capacity: 0,
            builder_requested_capacity_bytes: 0,
            builder_final_capacity_bytes: 0,
            builder_peak_capacity_bytes: 0,
            builder_reallocations: 0,
            builder_max_open_depth: 0,
            builder_shrink_bytes: 0,
            json_ld_bytes: 0,
            json_ld_parsed_bytes: 0,
            json_ld_retained_bytes: 0,
        },
        deferred_work: DeferredWorkCounters {
            content_hint_scans: 0,
            content_excerpt_scans: 0,
            final_dom_node_scans: 0,
            external_footnote_scans: 0,
        },
        phases: [0; PHASE_COUNT],
        live_bytes: 0,
        allocation_baseline: 0,
        attempt_plan_mask: 0,
    }) }
}

#[cfg(not(feature = "bench-instrumentation"))]
#[derive(Clone, Default)]
pub(crate) struct PhaseGuard;

#[cfg(feature = "bench-instrumentation")]
pub(crate) struct PhaseGuard {
    phase: Phase,
    started: Instant,
}

impl PhaseGuard {
    #[inline(always)]
    pub(crate) fn new(phase: Phase) -> Self {
        #[cfg(feature = "bench-instrumentation")]
        {
            Self {
                phase,
                started: Instant::now(),
            }
        }

        #[cfg(not(feature = "bench-instrumentation"))]
        {
            let _ = phase;
            Self
        }
    }
}

#[cfg(feature = "bench-instrumentation")]
impl Drop for PhaseGuard {
    fn drop(&mut self) {
        let nanos = self.started.elapsed().as_nanos().min(u64::MAX as u128) as u64;
        STATE.with(|state| {
            let mut value = state.get();
            value.phases[self.phase as usize] =
                value.phases[self.phase as usize].saturating_add(nanos);
            state.set(value);
        });
    }
}

#[inline(always)]
pub(crate) fn record_parse_call() {
    #[cfg(feature = "bench-instrumentation")]
    add_counter(|counters| counters.parse_calls = counters.parse_calls.saturating_add(1));
}

#[inline(always)]
pub(crate) fn record_source_full_scan() {
    #[cfg(feature = "bench-instrumentation")]
    add_counter(|counters| {
        counters.source_full_scans = counters.source_full_scans.saturating_add(1)
    });
}

#[inline(always)]
pub(crate) fn record_source_element_snapshot() {
    #[cfg(feature = "bench-instrumentation")]
    add_counter(|counters| {
        counters.source_element_snapshots = counters.source_element_snapshots.saturating_add(1)
    });
}

#[inline(always)]
pub(crate) fn record_fragment_snapshot(kind: SnapshotKind, nodes: usize, reused: bool) {
    #[cfg(feature = "bench-instrumentation")]
    add_counter(|counters| {
        counters.fragment_snapshot_requests = counters.fragment_snapshot_requests.saturating_add(1);
        if reused {
            counters.fragment_snapshot_reuses = counters.fragment_snapshot_reuses.saturating_add(1);
            counters.fragment_snapshot_reuse_nodes = counters
                .fragment_snapshot_reuse_nodes
                .saturating_add(nodes as u64);
            counters.fragment_snapshot_reuses_by_kind[kind as usize] =
                counters.fragment_snapshot_reuses_by_kind[kind as usize].saturating_add(1);
            counters.fragment_snapshot_reuse_nodes_by_kind[kind as usize] = counters
                .fragment_snapshot_reuse_nodes_by_kind[kind as usize]
                .saturating_add(nodes as u64);
        } else {
            counters.fragment_snapshot_rebuilds =
                counters.fragment_snapshot_rebuilds.saturating_add(1);
            counters.fragment_snapshot_nodes = counters
                .fragment_snapshot_nodes
                .saturating_add(nodes as u64);
            counters.fragment_snapshot_by_kind[kind as usize] =
                counters.fragment_snapshot_by_kind[kind as usize].saturating_add(1);
            counters.fragment_snapshot_nodes_by_kind[kind as usize] = counters
                .fragment_snapshot_nodes_by_kind[kind as usize]
                .saturating_add(nodes as u64);
        }
    });
    #[cfg(not(feature = "bench-instrumentation"))]
    let _ = (kind, nodes, reused);
}

#[inline(always)]
pub(crate) fn record_prepared_source_build() {
    #[cfg(feature = "bench-instrumentation")]
    add_counter(|counters| {
        counters.prepared_source_builds = counters.prepared_source_builds.saturating_add(1)
    });
}

#[inline(always)]
pub(crate) fn record_prepared_source_entries(entries: usize) {
    #[cfg(not(feature = "bench-instrumentation"))]
    let _ = entries;
    #[cfg(feature = "bench-instrumentation")]
    add_counter(|counters| {
        counters.prepared_source_entries = counters
            .prepared_source_entries
            .saturating_add(entries as u64)
    });
}

#[inline(always)]
pub(crate) fn record_content_hint_scan() {
    #[cfg(feature = "bench-instrumentation")]
    add_deferred_work(|counters| {
        counters.content_hint_scans = counters.content_hint_scans.saturating_add(1)
    });
}

#[inline(always)]
pub(crate) fn record_content_excerpt_scan() {
    #[cfg(feature = "bench-instrumentation")]
    add_deferred_work(|counters| {
        counters.content_excerpt_scans = counters.content_excerpt_scans.saturating_add(1)
    });
}

#[inline(always)]
pub(crate) fn record_final_dom_node_scan() {
    #[cfg(feature = "bench-instrumentation")]
    add_deferred_work(|counters| {
        counters.final_dom_node_scans = counters.final_dom_node_scans.saturating_add(1)
    });
}

#[inline(always)]
pub(crate) fn record_external_footnote_scan() {
    #[cfg(feature = "bench-instrumentation")]
    add_deferred_work(|counters| {
        counters.external_footnote_scans = counters.external_footnote_scans.saturating_add(1)
    });
}

#[inline(always)]
#[cfg(feature = "bench-instrumentation")]
pub(crate) fn record_dom_clone(_bytes: usize) {
    #[cfg(feature = "bench-instrumentation")]
    add_counter(|counters| {
        counters.dom_clones = counters.dom_clones.saturating_add(1);
        counters.dom_clone_bytes = counters.dom_clone_bytes.saturating_add(_bytes as u64);
    });
}

#[inline(always)]
pub(crate) fn record_fragment_copy() {
    #[cfg(feature = "bench-instrumentation")]
    add_counter(|counters| counters.fragment_copies = counters.fragment_copies.saturating_add(1));
}

#[inline(always)]
pub(crate) fn record_strategy(plan_id: u8) {
    #[cfg(feature = "bench-instrumentation")]
    STATE.with(|state| {
        let mut value = state.get();
        value.counters.strategies_started = value.counters.strategies_started.saturating_add(1);
        let _ = plan_id;
        state.set(value);
    });
    #[cfg(not(feature = "bench-instrumentation"))]
    let _ = plan_id;
}

#[inline(always)]
pub(crate) fn record_unique_attempt_plan(plan_id: u8) {
    #[cfg(feature = "bench-instrumentation")]
    STATE.with(|state| {
        let mut value = state.get();
        if plan_id < u8::BITS as u8 {
            let bit = 1_u8 << plan_id;
            if value.attempt_plan_mask & bit == 0 {
                value.attempt_plan_mask |= bit;
                value.counters.unique_attempt_plans =
                    value.counters.unique_attempt_plans.saturating_add(1);
            }
        } else {
            value.counters.unique_attempt_plans =
                value.counters.unique_attempt_plans.saturating_add(1);
        }
        state.set(value);
    });
    #[cfg(not(feature = "bench-instrumentation"))]
    let _ = plan_id;
}

#[inline(always)]
pub(crate) fn record_logical_attempt_plan() {
    #[cfg(feature = "bench-instrumentation")]
    add_counter(|counters| {
        counters.logical_attempt_plans = counters.logical_attempt_plans.saturating_add(1)
    });
}

#[inline(always)]
pub(crate) fn record_physical_attempt_execution() {
    #[cfg(feature = "bench-instrumentation")]
    add_counter(|counters| {
        counters.physical_attempt_executions =
            counters.physical_attempt_executions.saturating_add(1)
    });
}

#[inline(always)]
pub(crate) fn record_deduplicated_attempt() {
    #[cfg(feature = "bench-instrumentation")]
    add_counter(|counters| {
        counters.deduplicated_attempts = counters.deduplicated_attempts.saturating_add(1)
    });
}

#[inline(always)]
pub(crate) fn record_scoring_nodes(_nodes: usize) {
    #[cfg(feature = "bench-instrumentation")]
    add_counter(|counters| {
        counters.scoring_nodes = counters.scoring_nodes.saturating_add(_nodes as u64)
    });
}

#[inline(always)]
pub(crate) fn record_cleaned_nodes(_nodes: usize) {
    #[cfg(feature = "bench-instrumentation")]
    add_counter(|counters| {
        counters.cleaned_nodes = counters.cleaned_nodes.saturating_add(_nodes as u64)
    });
}

#[inline(always)]
pub(crate) fn record_semantic_source_nodes(_nodes: usize) {
    #[cfg(feature = "bench-instrumentation")]
    add_counter(|counters| {
        counters.semantic_source_nodes =
            counters.semantic_source_nodes.saturating_add(_nodes as u64)
    });
}

#[inline(always)]
pub(crate) fn record_semantic_operations(_operations: usize) {
    #[cfg(feature = "bench-instrumentation")]
    add_counter(|counters| {
        counters.semantic_operations = counters
            .semantic_operations
            .saturating_add(_operations as u64)
    });
}

#[cfg(feature = "bench-instrumentation")]
pub(crate) struct BuilderCapacityReport {
    pub(crate) requested_bytes: usize,
    pub(crate) final_bytes: usize,
    pub(crate) peak_bytes: usize,
    pub(crate) reallocations: usize,
    pub(crate) max_open_depth: usize,
    pub(crate) shrink_bytes: usize,
    pub(crate) ops: usize,
    pub(crate) ends: usize,
    pub(crate) open: usize,
    pub(crate) text: usize,
    pub(crate) payload: usize,
    pub(crate) footnotes: usize,
    pub(crate) footnote_index: usize,
}

#[inline(always)]
#[cfg(feature = "bench-instrumentation")]
pub(crate) fn record_builder_capacities(report: BuilderCapacityReport) {
    #[cfg(feature = "bench-instrumentation")]
    add_counter(|counters| {
        counters.builder_requested_capacity_bytes = counters
            .builder_requested_capacity_bytes
            .saturating_add(report.requested_bytes as u64);
        counters.builder_final_capacity_bytes = counters
            .builder_final_capacity_bytes
            .saturating_add(report.final_bytes as u64);
        counters.builder_peak_capacity_bytes = counters
            .builder_peak_capacity_bytes
            .max(report.peak_bytes as u64);
        counters.builder_reallocations = counters
            .builder_reallocations
            .saturating_add(report.reallocations as u64);
        counters.builder_max_open_depth = counters
            .builder_max_open_depth
            .max(report.max_open_depth as u64);
        counters.builder_shrink_bytes = counters
            .builder_shrink_bytes
            .saturating_add(report.shrink_bytes as u64);
        counters.builder_ops_capacity = counters
            .builder_ops_capacity
            .saturating_add(report.ops as u64);
        counters.builder_ends_capacity = counters
            .builder_ends_capacity
            .saturating_add(report.ends as u64);
        counters.builder_open_capacity = counters
            .builder_open_capacity
            .saturating_add(report.open as u64);
        counters.builder_text_capacity = counters
            .builder_text_capacity
            .saturating_add(report.text as u64);
        counters.builder_payload_capacity = counters
            .builder_payload_capacity
            .saturating_add(report.payload as u64);
        counters.builder_footnotes_capacity = counters
            .builder_footnotes_capacity
            .saturating_add(report.footnotes as u64);
        counters.builder_footnote_index_capacity = counters
            .builder_footnote_index_capacity
            .saturating_add(report.footnote_index as u64);
    });
}

#[inline(always)]
pub(crate) fn record_json_ld_bytes(_bytes: usize) {
    #[cfg(feature = "bench-instrumentation")]
    add_counter(|counters| {
        counters.json_ld_bytes = counters.json_ld_bytes.saturating_add(_bytes as u64)
    });
}

#[inline(always)]
#[cfg(feature = "bench-instrumentation")]
pub(crate) fn record_json_ld_parsed_bytes(_bytes: usize) {
    add_counter(|counters| {
        counters.json_ld_parsed_bytes = counters.json_ld_parsed_bytes.saturating_add(_bytes as u64)
    });
}

#[inline(always)]
#[cfg(feature = "bench-instrumentation")]
pub(crate) fn record_json_ld_retained_bytes(_bytes: usize) {
    #[cfg(feature = "bench-instrumentation")]
    add_counter(|counters| {
        counters.json_ld_retained_bytes = counters
            .json_ld_retained_bytes
            .saturating_add(_bytes as u64)
    });
}

#[inline(always)]
pub(crate) fn record_retained_bytes(_bytes: usize) {
    #[cfg(feature = "bench-instrumentation")]
    add_counter(|counters| counters.final_retained_bytes = _bytes as u64);
}

/// Records one allocation from the reporting binary's allocator wrapper.
#[cfg(feature = "bench-instrumentation")]
pub fn record_allocation(bytes: usize) {
    STATE.with(|state| {
        let mut value = state.get();
        let bytes = bytes as u64;
        value.live_bytes = value.live_bytes.saturating_add(bytes);
        value.counters.allocations = value.counters.allocations.saturating_add(1);
        value.counters.allocated_bytes = value.counters.allocated_bytes.saturating_add(bytes);
        let run_live_bytes = value.live_bytes.saturating_sub(value.allocation_baseline);
        value.counters.peak_live_bytes = value.counters.peak_live_bytes.max(run_live_bytes);
        value.counters.final_live_bytes = run_live_bytes;
        state.set(value);
    });
}

/// Records one deallocation from the reporting binary's allocator wrapper.
#[cfg(feature = "bench-instrumentation")]
pub fn record_deallocation(bytes: usize) {
    STATE.with(|state| {
        let mut value = state.get();
        let bytes = bytes as u64;
        value.live_bytes = value.live_bytes.saturating_sub(bytes);
        value.counters.deallocations = value.counters.deallocations.saturating_add(1);
        value.counters.deallocated_bytes = value.counters.deallocated_bytes.saturating_add(bytes);
        value.counters.final_live_bytes =
            value.live_bytes.saturating_sub(value.allocation_baseline);
        state.set(value);
    });
}

/// Records a reallocation as a deallocation followed by an allocation.
#[cfg(feature = "bench-instrumentation")]
pub fn record_reallocation(old_bytes: usize, new_bytes: usize) {
    record_deallocation(old_bytes);
    record_allocation(new_bytes);
}

#[cfg(feature = "bench-instrumentation")]
fn add_counter(update: impl FnOnce(&mut ExtractionCounters)) {
    STATE.with(|state| {
        let mut value = state.get();
        update(&mut value.counters);
        state.set(value);
    });
}

#[cfg(feature = "bench-instrumentation")]
fn add_deferred_work(update: impl FnOnce(&mut DeferredWorkCounters)) {
    STATE.with(|state| {
        let mut value = state.get();
        update(&mut value.deferred_work);
        state.set(value);
    });
}

/// Clears the current thread's measurement state.
#[cfg(feature = "bench-instrumentation")]
pub fn reset() {
    STATE.with(|state| {
        let current = state.get();
        state.set(State {
            live_bytes: current.live_bytes,
            allocation_baseline: current.live_bytes,
            ..State::default()
        });
    });
}

/// Returns a copy of the current thread's measurement state.
#[cfg(feature = "bench-instrumentation")]
pub fn snapshot() -> InstrumentationSnapshot {
    STATE.with(|state| {
        let value = state.get();
        let phases = value.phases;
        InstrumentationSnapshot {
            counters: value.counters,
            phases: PhaseDurations {
                parse: phases[Phase::Parse as usize],
                metadata: phases[Phase::Metadata as usize],
                preparation: phases[Phase::Preparation as usize],
                candidate_discovery: phases[Phase::CandidateDiscovery as usize],
                scoring: phases[Phase::Scoring as usize],
                root_selection: phases[Phase::RootSelection as usize],
                fragment_copy: phases[Phase::FragmentCopy as usize],
                cleanup: phases[Phase::Cleanup as usize],
                semantic_compilation: phases[Phase::SemanticCompilation as usize],
                rendering: phases[Phase::Rendering as usize],
            },
        }
    })
}

/// Returns deferred-work counters for the current thread.
#[cfg(feature = "bench-instrumentation")]
pub fn deferred_work_snapshot() -> DeferredWorkCounters {
    STATE.with(|state| state.get().deferred_work)
}

#[cfg(all(test, feature = "bench-instrumentation"))]
mod tests {
    use super::*;

    #[test]
    fn counters_and_phase_names_are_stable() {
        reset();
        record_parse_call();
        let guard = PhaseGuard::new(Phase::Parse);
        drop(guard);
        let snapshot = snapshot();
        assert_eq!(snapshot.counters.parse_calls, 1);
        assert!(snapshot.phases.parse > 0);
        assert_eq!(Phase::Parse.name(), "parse");
        assert_eq!(Phase::all().len(), PHASE_COUNT);
    }

    #[test]
    fn semantic_operation_count_includes_nested_container_operations() {
        reset();
        let page = crate::extract(
            "<main><div><p>This nested paragraph contains enough text for extraction.</p></div></main>",
            None,
        )
        .expect("nested content should extract");
        let counters = snapshot().counters;

        assert!(counters.semantic_operations > page.paragraph_count() as u64);
    }

    #[test]
    fn allocator_reset_uses_a_live_byte_baseline() {
        reset();
        record_allocation(100);
        reset();
        record_allocation(40);
        assert_eq!(snapshot().counters.peak_live_bytes, 40);
        record_deallocation(40);
        assert_eq!(snapshot().counters.final_live_bytes, 0);
        record_deallocation(100);
        reset();
    }

    #[test]
    fn builder_capacity_metrics_accumulate_except_for_peak() {
        reset();
        let report = |peak_bytes, reallocations, shrink_bytes| BuilderCapacityReport {
            requested_bytes: 10,
            final_bytes: 20,
            peak_bytes,
            reallocations,
            max_open_depth: 3,
            shrink_bytes,
            ops: 1,
            ends: 2,
            open: 3,
            text: 4,
            payload: 5,
            footnotes: 6,
            footnote_index: 7,
        };
        record_builder_capacities(report(100, 2, 8));
        record_builder_capacities(report(50, 3, 9));

        let counters = snapshot().counters;
        assert_eq!(counters.builder_requested_capacity_bytes, 20);
        assert_eq!(counters.builder_final_capacity_bytes, 40);
        assert_eq!(counters.builder_peak_capacity_bytes, 100);
        assert_eq!(counters.builder_reallocations, 5);
        assert_eq!(counters.builder_max_open_depth, 3);
        assert_eq!(counters.builder_shrink_bytes, 17);
        reset();
    }
}

#[cfg(all(test, not(feature = "bench-instrumentation")))]
mod disabled_tests {
    use super::PhaseGuard;

    #[test]
    fn disabled_measurement_guard_has_no_state() {
        assert_eq!(std::mem::size_of::<PhaseGuard>(), 0);
    }
}
