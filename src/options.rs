//! Options for article extraction and the quick readability check.

use regex::Regex;

/// Options for [`parse()`](crate::parse) and [`Document::parse`](crate::Document::parse).
///
/// Use the builder methods, or change the public fields directly.
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
/// `Options::new()` and `Options::default()` return the same values.
#[derive(Clone, Debug)]
pub struct Options {
    /// Maximum number of HTML elements that the document can contain.
    ///
    /// The default is `0`, which sets no limit. Legible checks the limit after HTML
    /// parsing and before extraction. It returns
    /// [`Error::TooManyElements`](crate::Error::TooManyElements) if the document
    /// exceeds the limit.
    pub max_elems_to_parse: usize,

    /// Number of high-score content candidates to compare.
    ///
    /// The default is `5`. A larger value can improve selection on a complex page, but
    /// it can increase processing time.
    pub nb_top_candidates: usize,

    /// Target minimum number of characters in the extracted article.
    ///
    /// The default is `500`. Legible retries with less filtering if the content is
    /// shorter. This value is not a strict minimum. After all retries, Legible can
    /// return shorter nonempty content.
    pub char_threshold: usize,

    /// CSS classes to keep in the output HTML.
    ///
    /// The default list contains `"page"`. This list applies only when
    /// [`keep_classes`](Options::keep_classes) is `false`.
    pub classes_to_preserve: Vec<String>,

    /// Controls whether Legible keeps all CSS classes in the output HTML.
    ///
    /// The default is `false`. If this value is `false`, Legible keeps only the classes
    /// in [`classes_to_preserve`](Options::classes_to_preserve).
    pub keep_classes: bool,

    /// Controls JSON-LD metadata extraction.
    ///
    /// The default is `false`. Set this value to `true` to ignore JSON-LD metadata.
    pub disable_json_ld: bool,

    /// Regular expression for permitted video embed URLs.
    ///
    /// `None` uses the built-in list of common video services. `Some(regex)` replaces
    /// the built-in list. Legible removes video embeds that do not match the regular
    /// expression.
    pub allowed_video_regex: Option<Regex>,

    /// Value that Legible adds to its link-density limits.
    ///
    /// The default is `0.0`. A positive value keeps more link-heavy content. A negative
    /// value removes more link-heavy content.
    pub link_density_modifier: f64,

    /// Controls extraction debug events.
    ///
    /// The default is `false`. With the `tracing` feature, `true` emits events to the
    /// configured tracing subscriber. Without that feature, this value has no effect.
    pub debug: bool,

    /// Enabled metadata source groups. Internal 0.5 extractor configuration.
    pub(crate) metadata_sources: u8,
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
            metadata_sources: 0b1111,
        }
    }
}

impl Options {
    /// Creates extraction options with default values.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the maximum number of HTML elements. Use `0` for no limit.
    pub fn max_elems_to_parse(mut self, max: usize) -> Self {
        self.max_elems_to_parse = max;
        self
    }

    /// Sets the number of high-score content candidates to compare.
    pub fn nb_top_candidates(mut self, n: usize) -> Self {
        self.nb_top_candidates = n;
        self
    }

    /// Sets the target minimum article length.
    pub fn char_threshold(mut self, threshold: usize) -> Self {
        self.char_threshold = threshold;
        self
    }

    /// Adds CSS classes to the list of classes to keep.
    ///
    /// This method extends the current list. The default list contains `"page"`.
    pub fn classes_to_preserve(mut self, classes: Vec<String>) -> Self {
        self.classes_to_preserve.extend(classes);
        self
    }

    /// Sets whether Legible keeps all CSS classes.
    pub fn keep_classes(mut self, keep: bool) -> Self {
        self.keep_classes = keep;
        self
    }

    /// Sets whether Legible disables JSON-LD metadata extraction.
    pub fn disable_json_ld(mut self, disable: bool) -> Self {
        self.disable_json_ld = disable;
        self
    }

    /// Replaces the built-in regular expression for permitted video URLs.
    pub fn allowed_video_regex(mut self, regex: Regex) -> Self {
        self.allowed_video_regex = Some(regex);
        self
    }

    /// Sets the value that Legible adds to its link-density limits.
    pub fn link_density_modifier(mut self, modifier: f64) -> Self {
        self.link_density_modifier = modifier;
        self
    }

    /// Sets whether Legible emits extraction debug events.
    pub fn debug(mut self, debug: bool) -> Self {
        self.debug = debug;
        self
    }
}

/// Options for [`is_probably_readerable`](crate::is_probably_readerable) and
/// [`Document::is_probably_readerable`](crate::Document::is_probably_readerable).
///
/// ```rust
/// use legible::{ReaderableOptions, is_probably_readerable};
///
/// let options = ReaderableOptions::new()
///     .min_score(30.0)
///     .min_content_length(100);
///
/// let text = "Article text. ".repeat(30);
/// let html = format!("<article><p>{text}</p></article>");
/// let likely_article = is_probably_readerable(&html, Some(options));
/// ```
#[derive(Clone)]
pub struct ReaderableOptions {
    /// Minimum total score for a positive result.
    ///
    /// The default is `20.0`. The check scores text in paragraph-like elements. A
    /// larger value requires more content.
    pub min_score: f64,

    /// Minimum text length of an element that can contribute to the score.
    ///
    /// The default is `140` characters. The check ignores shorter elements.
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
    /// Creates readability-check options with default values.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the minimum total score for a positive result.
    pub fn min_score(mut self, score: f64) -> Self {
        self.min_score = score;
        self
    }

    /// Sets the minimum text length of a scored element.
    pub fn min_content_length(mut self, length: usize) -> Self {
        self.min_content_length = length;
        self
    }
}
