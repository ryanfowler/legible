//! Structural recognition of discussion replies.
//!
//! A reply entry is a list item whose first element is an `<article>`. The tag
//! of the first element in the article is the shape of the entry, such as
//! `<strong>` for an author label. Replies in one list share a shape. A teaser
//! card often starts with a heading, so callers that must separate replies
//! from teasers also reject heading-led entries.

use crate::dom::{Dom, NodeId, Tag};

/// The number of sibling nodes on each side of an item that a peer search
/// inspects. A peer reply is almost always near the item, and the limit keeps
/// item checks linear in large lists.
const PEER_SEARCH_LIMIT: usize = 32;

/// A reply entry in a list item.
#[derive(Clone, Copy)]
pub(crate) struct ReplyEntry {
    /// The `<article>` that leads the list item.
    pub(crate) article: NodeId,
    shape: Tag,
}

impl ReplyEntry {
    /// Returns the reply entry of `item` when `item` is a list item that
    /// starts with an article.
    pub(crate) fn of(dom: &Dom, item: NodeId) -> Option<Self> {
        if dom.tag(item) != Some(Tag::Li) {
            return None;
        }
        let article = dom
            .element_children(item)
            .next()
            .filter(|&entry| dom.tag(entry) == Some(Tag::Article))?;
        let shape = dom.tag(dom.element_children(article).next()?)?;
        Some(Self { article, shape })
    }

    /// True when the article starts with a heading, as a teaser card does.
    pub(crate) fn is_heading_led(self) -> bool {
        matches!(
            self.shape,
            Tag::H1 | Tag::H2 | Tag::H3 | Tag::H4 | Tag::H5 | Tag::H6
        )
    }

    /// True when `item` holds a reply entry with the same shape as `self`.
    fn matches(self, dom: &Dom, item: NodeId) -> bool {
        Self::of(dom, item).is_some_and(|entry| entry.shape == self.shape)
    }

    /// True when the list that holds `item` also holds another reply entry
    /// with the same shape near `item`. `self` must be the entry of `item`.
    pub(crate) fn has_peer(self, dom: &Dom, item: NodeId) -> bool {
        let search = |step: fn(&Dom, NodeId) -> Option<NodeId>| {
            std::iter::successors(step(dom, item), |&node| step(dom, node))
                .take(PEER_SEARCH_LIMIT)
                .any(|other| self.matches(dom, other))
        };
        dom.parent(item).is_some_and(|list| is_list(dom, list))
            && (search(Dom::prev_sibling) || search(Dom::next_sibling))
    }
}

/// Returns true when `list` is a collapsed branch of a discussion: the only
/// nested list inside a reply item, where each item is a reply with the same
/// shape. The shape can be a heading, such as an `<h3>` author name. The server sends the replies, and the client hides them until a
/// reader expands the branch. A hidden list beside a visible list in the same
/// item is a responsive duplicate, so it is not a collapsed branch.
pub(crate) fn is_collapsed_branch(dom: &Dom, list: NodeId) -> bool {
    if !is_list(dom, list) {
        return false;
    }
    let Some(parent) = dom.parent(list) else {
        return false;
    };
    let Some(entry) = ReplyEntry::of(dom, parent) else {
        return false;
    };
    if dom
        .element_children(parent)
        .any(|child| child != list && is_list(dom, child))
    {
        return false;
    }
    let mut items = dom.element_children(list).peekable();
    items.peek().is_some() && items.all(|item| entry.matches(dom, item))
}

fn is_list(dom: &Dom, node: NodeId) -> bool {
    matches!(dom.tag(node), Some(Tag::Ol | Tag::Ul))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn first(dom: &Dom, tag: Tag) -> NodeId {
        dom.descendants(dom.root())
            .find(|&node| dom.tag(node) == Some(tag))
            .unwrap()
    }

    #[test]
    fn recognizes_reply_entries_and_peers() {
        let dom = Dom::parse_document(
            "<ol><li><article><strong>Ari</strong><p>One.</p></article></li>\
             <li><article><strong>Bo</strong><p>Two.</p></article></li>\
             <li><article><h3>Teaser</h3><p>Three.</p></article></li></ol>",
        )
        .unwrap();
        let items: Vec<_> = dom
            .descendants(dom.root())
            .filter(|&node| dom.tag(node) == Some(Tag::Li))
            .collect();
        let entry = ReplyEntry::of(&dom, items[0]).unwrap();
        assert_eq!(entry.article, first(&dom, Tag::Article));
        assert!(!entry.is_heading_led());
        assert!(entry.has_peer(&dom, items[0]));
        // A heading-led article can be a teaser card.
        let teaser = ReplyEntry::of(&dom, items[2]).unwrap();
        assert!(teaser.is_heading_led());
        assert!(!teaser.has_peer(&dom, items[2]));
    }

    #[test]
    fn a_reply_without_a_peer_of_the_same_shape_is_alone() {
        let dom = Dom::parse_document(
            "<ol><li><article><strong>Ari</strong><p>One.</p></article></li>\
             <li><article><p>Two.</p></article></li></ol>",
        )
        .unwrap();
        let item = first(&dom, Tag::Li);
        let entry = ReplyEntry::of(&dom, item).unwrap();
        assert!(!entry.has_peer(&dom, item));
    }

    #[test]
    fn collapsed_branches_repeat_the_reply_shape() {
        let branch = |nested: &str| {
            let dom = Dom::parse_document(&format!(
                "<ol><li><article><strong>Ari</strong><p>One.</p></article>{nested}</li></ol>"
            ))
            .unwrap();
            let list = dom
                .descendants(dom.root())
                .filter(|&node| is_list(&dom, node))
                .nth(1)
                .unwrap();
            is_collapsed_branch(&dom, list)
        };
        let reply = "<li><article><strong>Bo</strong><p>Two.</p></article></li>";
        assert!(branch(&format!("<ol>{reply}</ol>")));
        assert!(!branch("<ol></ol>"));
        assert!(!branch(&format!(
            "<ol>{reply}<li><a href=\"/x\">Menu entry</a></li></ol>"
        )));
        assert!(!branch(
            "<ol><li><article><h3>Teaser</h3><p>Two.</p></article></li></ol>"
        ));
        // Heading-led replies under a heading-led reply form a branch.
        let dom = Dom::parse_document(
            "<ol><li><article><h3>Ari</h3><p>One.</p></article>\
             <ol><li><article><h3>Bo</h3><p>Two.</p></article></li></ol></li></ol>",
        )
        .unwrap();
        let list = dom
            .descendants(dom.root())
            .filter(|&node| is_list(&dom, node))
            .nth(1)
            .unwrap();
        assert!(is_collapsed_branch(&dom, list));
        // A second list beside the branch makes the branch a duplicate.
        assert!(!branch(&format!("<ol>{reply}</ol><ol>{reply}</ol>")));
    }
}
