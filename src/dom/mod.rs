//! DOM abstraction layer for working with HTML documents.

mod node;
mod traversal;

pub use node::*;
pub use traversal::{get_tag_name, has_ancestor_tag};

use dom_query::{Node, Selection};

/// Create a Selection from a Node for querying.
/// This is needed because dom_query's NodeRef doesn't have a select() method directly.
pub fn node_select<'a>(node: &Node<'a>, selector: &str) -> Selection<'a> {
    Selection::from(*node).select(selector)
}
