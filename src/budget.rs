//! Parser and structured-data resource budgets.

/// Limits resource use while parsing a document and its JSON-LD scripts.
///
/// A value of `0` means no caller-configured limit, except for JSON-LD depth,
/// which uses a conservative internal safety cap. The limits apply to the
/// caller-provided document. Legible's internal fragment parsing keeps its
/// existing behavior.
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
    /// Maximum number of text bytes in the DOM.
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
