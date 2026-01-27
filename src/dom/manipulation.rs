//! DOM manipulation utilities.

use dom_query::Node;

/// Remove a node from the document and return the next node for traversal.
pub fn remove_and_get_next<'a>(node: &Node<'a>) -> Option<Node<'a>> {
    let next = super::get_next_node(node, true);
    node.remove_from_parent();
    next
}

/// Get the inner HTML of a node.
pub fn get_inner_html(node: &Node<'_>) -> String {
    node.inner_html().to_string()
}

/// Get the outer HTML of a node.
pub fn get_outer_html(node: &Node<'_>) -> String {
    node.html().to_string()
}

/// Get the text content of a node.
pub fn get_text_content(node: &Node<'_>) -> String {
    node.text().to_string()
}

/// Get an attribute value from a node.
pub fn get_attribute(node: &Node<'_>, name: &str) -> Option<String> {
    node.attr(name).map(|s| s.to_string())
}

/// Set an attribute on a node.
pub fn set_attribute(node: &Node<'_>, name: &str, value: &str) {
    node.set_attr(name, value);
}

/// Remove an attribute from a node.
pub fn remove_attribute(node: &Node<'_>, name: &str) {
    node.remove_attr(name);
}

/// Check if a node has an attribute.
pub fn has_attribute(node: &Node<'_>, name: &str) -> bool {
    node.has_attr(name)
}
