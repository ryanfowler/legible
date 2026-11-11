//! Parser and structured-data resource budgets.

/// Limits resource use while parsing a document and its JSON-LD scripts.
///
/// A value of `0` means no caller-configured limit, except for JSON-LD depth,
/// which uses a conservative internal safety cap. The limits apply to the
/// caller-provided document. Legible's internal fragment parsing keeps its
/// existing behavior.
///
/// [`ParseBudget::default`] has no limits. Use [`ParseBudget::untrusted`] for
/// HTML from the network or from other untrusted sources. Change single
/// limits with struct update syntax:
///
/// ```rust
/// use legible::{Extractor, ParseBudget};
///
/// let budget = ParseBudget {
///     max_input_bytes: 4 * 1024 * 1024,
///     ..ParseBudget::untrusted()
/// };
/// let extractor = Extractor::builder().parse_budget(budget).build();
/// # let _ = extractor;
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParseBudget {
    /// Maximum input size in bytes.
    pub max_input_bytes: usize,
    /// Maximum number of allocated DOM nodes, including the document root.
    pub max_nodes: usize,
    /// Maximum number of HTML elements.
    pub max_elements: usize,
    /// Maximum number of attributes across all elements.
    pub max_total_attributes: usize,
    /// Maximum number of attributes on one element.
    pub max_attributes_per_element: usize,
    /// Maximum number of parsed text bytes. This includes executable script
    /// text that the parser does not keep.
    pub max_text_bytes: usize,
    /// Maximum element nesting depth.
    pub max_depth: usize,
    /// Maximum total JSON-LD script bytes.
    pub max_json_ld_bytes: usize,
    /// Maximum number of typed JSON-LD items.
    pub max_json_ld_items: usize,
    /// Maximum JSON-LD nesting depth. Zero uses the internal safety cap.
    pub max_json_ld_depth: usize,
}

impl ParseBudget {
    /// Returns limits for HTML from untrusted sources.
    ///
    /// The limits accept large real pages and stop hostile input early:
    ///
    /// | Limit | Value |
    /// | --- | ---: |
    /// | Input bytes | 16 MiB |
    /// | DOM nodes | 2,000,000 |
    /// | Elements | 1,000,000 |
    /// | Total attributes | 4,000,000 |
    /// | Attributes per element | 512 |
    /// | Text bytes | 16 MiB |
    /// | Element depth | 512 |
    /// | JSON-LD bytes | 4 MiB |
    /// | JSON-LD items | 10,000 |
    /// | JSON-LD depth | 128 |
    ///
    /// The depth limit matters most. Some extraction checks walk ancestors
    /// or subtrees of each element, so their cost grows with the document
    /// depth. Chromium's HTML parser uses the same depth bound. The values
    /// can change in a minor release.
    pub const fn untrusted() -> Self {
        Self {
            max_input_bytes: 16 * 1024 * 1024,
            max_nodes: 2_000_000,
            max_elements: 1_000_000,
            max_total_attributes: 4_000_000,
            max_attributes_per_element: 512,
            max_text_bytes: 16 * 1024 * 1024,
            max_depth: 512,
            max_json_ld_bytes: 4 * 1024 * 1024,
            max_json_ld_items: 10_000,
            max_json_ld_depth: 128,
        }
    }
}
