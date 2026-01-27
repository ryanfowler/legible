//! Configuration options for Readability parsing.

use regex::Regex;

/// Configuration options for the Readability parser.
#[derive(Clone)]
pub struct Options {
    /// Maximum number of elements to parse. 0 means no limit.
    pub max_elems_to_parse: usize,

    /// The number of top candidates to consider when analyzing competition.
    pub nb_top_candidates: usize,

    /// The minimum number of characters an article must have to return a result.
    pub char_threshold: usize,

    /// Classes to preserve on elements in the output.
    pub classes_to_preserve: Vec<String>,

    /// Whether to keep all classes on elements (if false, only preserved classes are kept).
    pub keep_classes: bool,

    /// Whether to disable JSON-LD metadata extraction.
    pub disable_json_ld: bool,

    /// Custom regex for allowed video URLs (youtube, vimeo, etc.).
    pub allowed_video_regex: Option<Regex>,

    /// Modifier for link density threshold (added to the base threshold).
    pub link_density_modifier: f64,

    /// Enable debug logging.
    pub debug: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            max_elems_to_parse: 0,
            nb_top_candidates: 5,
            char_threshold: 500,
            classes_to_preserve: vec!["page".to_string()],
            keep_classes: false,
            disable_json_ld: false,
            allowed_video_regex: None,
            link_density_modifier: 0.0,
            debug: false,
        }
    }
}

impl Options {
    /// Create a new Options with default values.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the maximum number of elements to parse.
    pub fn max_elems_to_parse(mut self, max: usize) -> Self {
        self.max_elems_to_parse = max;
        self
    }

    /// Set the number of top candidates to consider.
    pub fn nb_top_candidates(mut self, n: usize) -> Self {
        self.nb_top_candidates = n;
        self
    }

    /// Set the character threshold for article content.
    pub fn char_threshold(mut self, threshold: usize) -> Self {
        self.char_threshold = threshold;
        self
    }

    /// Add classes to preserve in the output.
    pub fn classes_to_preserve(mut self, classes: Vec<String>) -> Self {
        self.classes_to_preserve.extend(classes);
        self
    }

    /// Set whether to keep all classes.
    pub fn keep_classes(mut self, keep: bool) -> Self {
        self.keep_classes = keep;
        self
    }

    /// Set whether to disable JSON-LD metadata extraction.
    pub fn disable_json_ld(mut self, disable: bool) -> Self {
        self.disable_json_ld = disable;
        self
    }

    /// Set a custom regex for allowed video URLs.
    pub fn allowed_video_regex(mut self, regex: Regex) -> Self {
        self.allowed_video_regex = Some(regex);
        self
    }

    /// Set the link density modifier.
    pub fn link_density_modifier(mut self, modifier: f64) -> Self {
        self.link_density_modifier = modifier;
        self
    }

    /// Enable or disable debug mode.
    pub fn debug(mut self, debug: bool) -> Self {
        self.debug = debug;
        self
    }
}

/// Options for the `is_probably_readerable` function.
#[derive(Clone)]
pub struct ReaderableOptions {
    /// Minimum cumulated score to consider the document readerable.
    pub min_score: f64,

    /// Minimum node content length to consider for scoring.
    pub min_content_length: usize,
}

impl Default for ReaderableOptions {
    fn default() -> Self {
        Self {
            min_score: 20.0,
            min_content_length: 140,
        }
    }
}

impl ReaderableOptions {
    /// Create new ReaderableOptions with default values.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the minimum score.
    pub fn min_score(mut self, score: f64) -> Self {
        self.min_score = score;
        self
    }

    /// Set the minimum content length.
    pub fn min_content_length(mut self, length: usize) -> Self {
        self.min_content_length = length;
        self
    }
}
