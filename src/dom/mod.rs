//! DOM abstraction layer for working with HTML documents.

pub mod manipulation;
pub mod node;
pub mod traversal;

pub use manipulation::*;
pub use node::*;
pub use traversal::*;

use dom_query::{Node, Selection};

/// Create a Selection from a Node for querying.
/// This is needed because dom_query's NodeRef doesn't have a select() method directly.
pub fn node_select<'a>(node: &Node<'a>, selector: &str) -> Selection<'a> {
    Selection::from(*node).select(selector)
}
