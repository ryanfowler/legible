//! DOM abstraction layer for working with HTML documents.

mod node;
mod traversal;

pub use node::*;
pub use traversal::get_tag_name;

use dom_query::{Matcher, Node, Selection};

/// Create a Selection from a Node using a pre-compiled Matcher.
/// This avoids repeated parsing of selector strings in hot paths.
pub fn node_select_matcher<'a>(node: &Node<'a>, matcher: &Matcher) -> Selection<'a> {
    Selection::from(*node).select_matcher(matcher)
}

/// Build a match string from a node's class and id attributes into a reusable buffer.
/// Format: "{class} {id}" — used for regex matching against node identity.
pub fn build_match_string(node: &Node<'_>, buf: &mut String) {
    buf.clear();
    if let Some(class) = node.attr("class") {
        buf.push_str(class.as_ref());
    }
    buf.push(' ');
    if let Some(id) = node.attr("id") {
        buf.push_str(id.as_ref());
    }
}
