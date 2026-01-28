//! Configuration options for Readability parsing.

use regex::Regex;

/// Configuration options for the [`Readability`](crate::Readability) parser.
///
/// Use the builder methods to customize parsing behavior:
///
/// ```rust
/// use legible::Options;
///
/// let options = Options::new()
///     .char_threshold(250)
///     .keep_classes(true)
///     .disable_json_ld(true);
/// ```
///
/// # Available Options
///
/// | Option | Default | Description |
/// |--------|---------|-------------|
/// | `max_elems_to_parse` | `0` | Maximum elements to parse (0 = unlimited) |
/// | `nb_top_candidates` | `5` | Number of top candidates to consider |
/// | `char_threshold` | `500` | Minimum article character length |
/// | `keep_classes` | `false` | Preserve CSS classes in output |
/// | `classes_to_preserve` | `["page"]` | Specific classes to keep |
/// | `disable_json_ld` | `false` | Skip JSON-LD metadata extraction |
/// | `allowed_video_regex` | - | Custom regex for allowed video embeds |
/// | `link_density_modifier` | `0.0` | Adjust link density threshold |
/// | `debug` | `false` | Enable debug logging |
#[derive(Clone)]
pub struct Options {
    /// Maximum number of elements to parse. Set to `0` for no limit.
    ///
    /// Use this to prevent excessive processing time on very large documents.
    /// Returns [`ReadabilityError::TooManyElements`](crate::ReadabilityError::TooManyElements)
    /// if the limit is exceeded.
    pub max_elems_to_parse: usize,

    /// The number of top candidates to consider when analyzing competition.
    ///
    /// Higher values may improve accuracy on complex pages but increase processing time.
    pub nb_top_candidates: usize,

    /// The minimum number of characters an article must have to return a result.
    ///
    /// If the extracted content is shorter than this threshold, the algorithm
    /// will retry with less aggressive filtering.
    pub char_threshold: usize,

    /// CSS classes to preserve on elements in the output.
    ///
    /// By default, most classes are stripped from the output HTML. Add class names
    /// here to preserve them (e.g., for styling purposes).
    pub classes_to_preserve: Vec<String>,

    /// Whether to keep all CSS classes on elements.
    ///
    /// If `true`, all classes are preserved. If `false`, only classes in
    /// `classes_to_preserve` are kept.
    pub keep_classes: bool,

    /// Whether to disable JSON-LD metadata extraction.
    ///
    /// JSON-LD is commonly used for structured article metadata. Disable this
    /// if you're experiencing issues with JSON-LD parsing.
    pub disable_json_ld: bool,

    /// Custom regex for allowed video embed URLs.
    ///
    /// By default, common video platforms (YouTube, Vimeo, etc.) are allowed.
    /// Set this to customize which video embeds are preserved.
    pub allowed_video_regex: Option<Regex>,

    /// Modifier for link density threshold.
    ///
    /// Added to the base threshold when determining if an element has too many links.
    /// Positive values make the algorithm more permissive of link-heavy content.
    pub link_density_modifier: f64,

    /// Enable debug logging to stderr.
    ///
    /// When enabled, the algorithm logs its decision-making process.
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

/// Options for the [`is_probably_readerable`](crate::is_probably_readerable) function.
///
/// Use these options to tune the quick readability check:
///
/// ```rust
/// use legible::{is_probably_readerable, ReaderableOptions};
///
/// let options = ReaderableOptions::new()
///     .min_score(30.0)
///     .min_content_length(100);
///
/// let html = "<html><body><article>Content...</article></body></html>";
/// if is_probably_readerable(html, Some(options)) {
///     println!("Document appears to be readerable");
/// }
/// ```
#[derive(Clone)]
pub struct ReaderableOptions {
    /// Minimum cumulated score to consider the document readerable.
    ///
    /// The score is calculated based on the length of text content in paragraph-like elements.
    /// Higher values require more substantial content. Default is `20.0`.
    pub min_score: f64,

    /// Minimum node content length to consider for scoring.
    ///
    /// Nodes with fewer characters than this threshold are ignored. Default is `140`.
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
