//! Optional structured extraction diagnostics.

/// The extraction strategy used for one attempt.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExtractionStrategyInfo {
    /// The default focused extraction.
    Normal,
    /// Extraction without conditional relevance cleanup.
    RelaxedCleanup,
    /// Extraction from a broader semantic boundary.
    BroadContent,
    /// Extraction guided by structured page data.
    StructuredDataHint,
    /// Extraction that can recover statically hidden content.
    RelaxedVisibility,
    /// Extraction from the document body.
    BodyFallback,
}

/// The reason for the selected content boundary.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RootSelectionReasonInfo {
    /// The root had the highest candidate rank.
    Ranked,
    /// A more specific child provided the best boundary.
    SpecificChild,
    /// Related candidate branches shared this parent.
    SharedParent,
    /// An ancestor contained the complete selected content.
    CompleteAncestor,
    /// Structured page data identified the root.
    StructuredData,
    /// The strategy selected the document body.
    BodyFallback,
}

/// One source of evidence for a selected root.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CandidateSourceInfo {
    /// A semantic element, role, ID, or class identified the root.
    Semantic,
    /// Readability-style prose scoring identified the root.
    Readability,
    /// Structured page data identified the root.
    StructuredData,
    /// Generic structural content identified the root.
    Generic,
}

/// A stable description of a selected root.
#[non_exhaustive]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RootInfo {
    /// The lowercase HTML tag name, when available.
    pub tag: Option<String>,
    /// The source element ID, when available.
    pub id: Option<String>,
    /// The source element classes.
    pub classes: Vec<String>,
    /// The reason for selecting this boundary.
    pub selection_reason: RootSelectionReasonInfo,
    /// Candidate evidence attached to this root.
    pub candidate_sources: Vec<CandidateSourceInfo>,
}

/// Content measurements for a source or result region.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ContentMetricsInfo {
    pub word_count: usize,
    pub text_chars: usize,
    pub paragraph_count: usize,
    pub heading_count: usize,
    pub structured_block_count: usize,
    pub link_density: f64,
}

/// Source-relative quality measurements for one result.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct QualityInfo {
    pub coverage: f64,
    pub best_attempt_score: f64,
    pub good: bool,
    pub suspiciously_small: bool,
}

/// The reason that an extraction attempt did not win.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AttemptRejectionReason {
    /// The selected root was document chrome.
    DocumentChrome,
    /// The result appeared to be an access barrier.
    AccessBarrier,
    /// The source itself was a short access barrier.
    SourceAccessBarrier,
    /// The result was an interactive application shell.
    InteractiveShell,
    /// The result was too short and lacked coherent context.
    IncoherentShortResult,
    /// The quality did not meet the acceptance threshold.
    LowQuality,
    /// Static hidden-content evidence requested a recovery attempt.
    PotentialHiddenContent,
    /// A visibility-relaxed result did not improve enough.
    InsufficientImprovement,
    /// A later attempt had better quality.
    Superseded,
}

/// Diagnostic data for one extraction attempt.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq)]
pub struct ExtractionAttempt {
    pub strategy: ExtractionStrategyInfo,
    pub selected_root: RootInfo,
    pub source: ContentMetricsInfo,
    pub result: ContentMetricsInfo,
    pub quality: QualityInfo,
    pub accepted: bool,
    pub rejection_reason: Option<AttemptRejectionReason>,
}

/// Structured information about the extraction decision.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq)]
pub struct ExtractionDiagnostics {
    pub selected_strategy: ExtractionStrategyInfo,
    pub attempts: Vec<ExtractionAttempt>,
}
