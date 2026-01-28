//! DOM abstraction layer for working with HTML documents.

mod node;
mod traversal;

pub use node::*;
pub use traversal::{get_tag_name, has_ancestor_tag};

use dom_query::{Matcher, Node, Selection};

/// Create a Selection from a Node for querying.
/// This is needed because dom_query's NodeRef doesn't have a select() method directly.
pub fn node_select<'a>(node: &Node<'a>, selector: &str) -> Selection<'a> {
    Selection::from(*node).select(selector)
}

/// Create a Selection from a Node using a pre-compiled Matcher.
/// This avoids repeated parsing of selector strings in hot paths.
pub fn node_select_matcher<'a>(node: &Node<'a>, matcher: &Matcher) -> Selection<'a> {
    Selection::from(*node).select_matcher(matcher)
}
