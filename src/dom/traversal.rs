//! DOM tree traversal utilities.

use dom_query::Node;

/// Get the next node in a depth-first traversal.
///
/// # Arguments
/// * `node` - The current node
/// * `ignore_self_and_kids` - If true, skip this node's children and find the next sibling/uncle
///
/// This matches the JavaScript `_getNextNode` function.
pub fn get_next_node<'a>(node: &Node<'a>, ignore_self_and_kids: bool) -> Option<Node<'a>> {
    // First check for kids if those aren't being ignored
    if !ignore_self_and_kids
        && let Some(first_child) = node.first_element_child()
    {
        return Some(first_child);
    }

    // Then for siblings...
    if let Some(next_sibling) = node.next_element_sibling() {
        return Some(next_sibling);
    }

    // And finally, move up the parent chain *and* find a sibling
    let mut current = node.parent();
    while let Some(parent) = current {
        if let Some(next_sibling) = parent.next_element_sibling() {
            return Some(next_sibling);
        }
        current = parent.parent();
    }

    None
}

/// Get all ancestors of a node up to a maximum depth.
///
/// # Arguments
/// * `node` - The starting node
/// * `max_depth` - Maximum number of ancestors to return (0 means unlimited)
pub fn get_node_ancestors<'a>(node: &Node<'a>, max_depth: usize) -> Vec<Node<'a>> {
    let mut ancestors = Vec::new();
    let mut current = node.parent();
    let mut depth = 0;

    while let Some(parent) = current {
        ancestors.push(parent);
        depth += 1;
        if max_depth > 0 && depth >= max_depth {
            break;
        }
        current = parent.parent();
    }

    ancestors
}

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
