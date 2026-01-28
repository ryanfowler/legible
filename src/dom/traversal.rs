//! DOM tree traversal utilities.

use dom_query::Node;

/// Check if a node has an ancestor with the given tag name.
///
/// # Arguments
/// * `node` - The starting node
/// * `tag_name` - The tag name to look for (case-insensitive)
/// * `max_depth` - Maximum depth to search (negative means unlimited)
/// * `filter` - Optional filter function that must return true for the ancestor to match
pub fn has_ancestor_tag<'a, F>(
    node: &Node<'a>,
    tag_name: &str,
    max_depth: i32,
    filter: Option<F>,
) -> bool
where
    F: Fn(&Node<'a>) -> bool,
{
    let tag_upper = tag_name.to_uppercase();
    let mut depth = 0;
    let mut current = node.parent();

    while let Some(parent) = current {
        if max_depth > 0 && depth > max_depth {
            return false;
        }

        if let Some(parent_tag) = parent.node_name()
            && parent_tag.to_uppercase() == tag_upper
        {
            if let Some(ref f) = filter {
                if f(&parent) {
                    return true;
                }
            } else {
                return true;
            }
        }

        current = parent.parent();
        depth += 1;
    }

    false
}

/// Get the tag name of a node in uppercase.
pub fn get_tag_name(node: &Node<'_>) -> Option<String> {
    node.node_name().map(|n| n.to_uppercase())
}
