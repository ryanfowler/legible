//! Shared canonical output for discussion and threaded pages.

use super::{append_text, create_element, import_children, new_output};
use crate::dom::{AttrName, Dom, NodeId, Tag};
use crate::page_kind::PageKind;

const MAX_REPLY_DEPTH: usize = 4_096;

struct ReplyFrame {
    item: NodeId,
    nested_list: Option<NodeId>,
}

/// Builds the stable semantic HTML shape used by specialized discussions.
pub(super) struct DiscussionBuilder {
    dom: Dom,
    root: NodeId,
    primary: Option<NodeId>,
    replies: Option<NodeId>,
    reply_list: Option<NodeId>,
    retained_at_depth: Vec<Option<ReplyFrame>>,
}

impl DiscussionBuilder {
    pub(super) fn new() -> Option<Self> {
        let (mut dom, root) = new_output()?;
        dom.set_attr(root, AttrName::DataLegibleKind, "discussion");
        Some(Self {
            dom,
            root,
            primary: None,
            replies: None,
            reply_list: None,
            retained_at_depth: Vec::new(),
        })
    }

    /// Adds the primary title as the first child of the primary article.
    pub(super) fn set_title(&mut self, source: &Dom, title: NodeId) -> bool {
        let Some(primary) = self.primary_article() else {
            return false;
        };
        let Some(heading) = create_element(&mut self.dom, primary, Tag::H1) else {
            return false;
        };
        if source.tag(title) == Some(Tag::H1) {
            import_children(source, title, &mut self.dom, heading)
        } else if source.tag(title).is_some_and(is_inline_title_tag) {
            let Ok(title) = self.dom.import_subtree(source, title) else {
                return false;
            };
            self.dom.append_child(heading, title);
            true
        } else {
            append_text(&mut self.dom, heading, source.text(title).trim())
        }
    }

    /// Adds a plain-text primary byline or metadata paragraph.
    pub(super) fn append_primary_text(&mut self, value: &str) -> bool {
        let Some(primary) = self.primary_article() else {
            return false;
        };
        let Some(byline) = create_element(&mut self.dom, primary, Tag::P) else {
            return false;
        };
        self.dom.set_attr(byline, AttrName::DataLegibleByline, "");
        append_text(&mut self.dom, byline, value)
    }

    /// Adds structured author and time metadata to the primary post.
    pub(super) fn append_primary_byline(
        &mut self,
        author: Option<&str>,
        time: Option<&str>,
    ) -> bool {
        let Some(primary) = self.primary_article() else {
            return false;
        };
        self.append_byline(primary, author, time, AttrName::DataLegibleByline)
            .is_some()
    }

    /// Imports rich primary-post content into its canonical body wrapper.
    pub(super) fn append_primary_body(&mut self, source: &Dom, body: NodeId) -> bool {
        self.append_primary_body_filtered(source, body, |_| true)
    }

    /// Imports primary content while omitting known peripheral top-level nodes.
    pub(super) fn append_primary_body_filtered(
        &mut self,
        source: &Dom,
        body: NodeId,
        mut retain: impl FnMut(NodeId) -> bool,
    ) -> bool {
        let Some(primary) = self.primary_article() else {
            return false;
        };
        let Some(content) = create_element(&mut self.dom, primary, Tag::Div) else {
            return false;
        };
        self.dom.set_attr(content, AttrName::DataLegibleBody, "");
        self.import_filtered_children(source, body, content, &mut retain)
    }

    /// Sets the heading used for the reply section.
    pub(super) fn set_reply_heading(&mut self, label: &str) -> bool {
        let Some(replies) = self.replies_section() else {
            return false;
        };
        let Some(heading) = create_element(&mut self.dom, replies, Tag::H2) else {
            return false;
        };
        append_text(&mut self.dom, heading, label)
    }

    /// Adds a reply at `depth`, preserving the nearest retained ancestor.
    ///
    /// A missing body represents a deleted or unavailable reply. It updates
    /// the depth state but emits no item, so surviving children attach to the
    /// nearest valid ancestor.
    pub(super) fn append_reply(
        &mut self,
        source: &Dom,
        depth: usize,
        author: Option<&str>,
        time: Option<&str>,
        body: Option<NodeId>,
    ) -> Option<()> {
        self.append_reply_filtered(source, depth, author, time, body, |_| true)
    }

    pub(super) fn append_reply_filtered(
        &mut self,
        source: &Dom,
        depth: usize,
        author: Option<&str>,
        time: Option<&str>,
        body: Option<NodeId>,
        mut retain: impl FnMut(NodeId) -> bool,
    ) -> Option<()> {
        let depth = depth.min(MAX_REPLY_DEPTH);
        if self.retained_at_depth.len() > depth {
            self.retained_at_depth.truncate(depth);
        } else {
            self.retained_at_depth.resize_with(depth, || None);
        }

        let Some(body) = body else {
            return Some(());
        };

        let list =
            if let Some(parent_index) = self.retained_at_depth.iter().rposition(Option::is_some) {
                let parent_frame = self.retained_at_depth[parent_index].as_ref()?;
                if let Some(list) = parent_frame.nested_list {
                    list
                } else {
                    let parent = parent_frame.item;
                    let list = create_element(&mut self.dom, parent, Tag::Ul)?;
                    let parent_frame = self.retained_at_depth[parent_index].as_mut()?;
                    parent_frame.nested_list = Some(list);
                    list
                }
            } else {
                self.reply_list_node()?
            };

        let item = create_element(&mut self.dom, list, Tag::Li)?;
        self.dom.set_attr(item, AttrName::DataLegibleReply, "");
        self.append_byline(item, author, time, AttrName::DataLegibleReplyMeta)?;
        let content = create_element(&mut self.dom, item, Tag::Div)?;
        self.dom
            .set_attr(content, AttrName::DataLegibleReplyBody, "");
        if !self.import_filtered_children(source, body, content, &mut retain) {
            return None;
        }
        self.retained_at_depth.push(Some(ReplyFrame {
            item,
            nested_list: None,
        }));
        Some(())
    }

    pub(super) fn finish(self, identity: &'static str) -> super::SpecializedResult {
        super::SpecializedResult {
            dom: self.dom,
            root: self.root,
            kind: PageKind::Discussion,
            identity,
        }
    }

    fn primary_article(&mut self) -> Option<NodeId> {
        if let Some(primary) = self.primary {
            return Some(primary);
        }
        let primary = create_element(&mut self.dom, self.root, Tag::Article)?;
        self.dom.set_attr(primary, AttrName::DataLegiblePrimary, "");
        self.primary = Some(primary);
        Some(primary)
    }

    fn replies_section(&mut self) -> Option<NodeId> {
        if let Some(replies) = self.replies {
            return Some(replies);
        }
        let replies = create_element(&mut self.dom, self.root, Tag::Section)?;
        self.dom.set_attr(replies, AttrName::DataLegibleReplies, "");
        self.replies = Some(replies);
        Some(replies)
    }

    fn reply_list_node(&mut self) -> Option<NodeId> {
        if let Some(list) = self.reply_list {
            return Some(list);
        }
        let replies = self.replies_section()?;
        let list = create_element(&mut self.dom, replies, Tag::Ul)?;
        self.reply_list = Some(list);
        Some(list)
    }

    fn append_byline(
        &mut self,
        parent: NodeId,
        author: Option<&str>,
        time: Option<&str>,
        attribute: AttrName,
    ) -> Option<()> {
        let author = author.map(str::trim).filter(|value| !value.is_empty());
        let time = time.map(str::trim).filter(|value| !value.is_empty());
        if author.is_none() && time.is_none() {
            return Some(());
        }
        let byline = create_element(&mut self.dom, parent, Tag::P)?;
        self.dom.set_attr(byline, attribute, "");
        if let Some(author) = author {
            let strong = create_element(&mut self.dom, byline, Tag::Strong)?;
            self.dom.set_attr(strong, AttrName::DataLegibleAuthor, "");
            if !append_text(&mut self.dom, strong, author) {
                return None;
            }
        }
        if let Some(time) = time {
            if author.is_some() && !append_text(&mut self.dom, byline, " · ") {
                return None;
            }
            if !append_text(&mut self.dom, byline, time) {
                return None;
            }
        }
        Some(())
    }

    fn import_filtered_children(
        &mut self,
        source: &Dom,
        source_root: NodeId,
        destination: NodeId,
        retain: &mut impl FnMut(NodeId) -> bool,
    ) -> bool {
        for child in source.children(source_root) {
            if !retain(child) {
                continue;
            }
            let Ok(child) = self.dom.import_subtree(source, child) else {
                return false;
            };
            self.dom.append_child(destination, child);
        }
        true
    }
}

fn is_inline_title_tag(tag: Tag) -> bool {
    matches!(
        tag,
        Tag::A
            | Tag::Abbr
            | Tag::B
            | Tag::Bdi
            | Tag::Bdo
            | Tag::Cite
            | Tag::Code
            | Tag::Data
            | Tag::Dfn
            | Tag::Em
            | Tag::I
            | Tag::Kbd
            | Tag::Mark
            | Tag::Q
            | Tag::Ruby
            | Tag::Samp
            | Tag::Small
            | Tag::Span
            | Tag::Strong
            | Tag::Sub
            | Tag::Sup
            | Tag::Time
            | Tag::U
            | Tag::Var
            | Tag::Wbr
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(source: &Dom, text: &str) -> NodeId {
        source
            .descendants(source.root())
            .find(|&node| source.tag(node) == Some(Tag::P) && source.text(node) == text)
            .unwrap()
    }

    #[test]
    fn preserves_rich_primary_and_reply_semantics() {
        let source = Dom::parse_document(
            "<h1>Topic</h1><div id=primary><p>Lead <em>text</em>.</p></div>\
             <div id=reply><p>Reply <code>code</code>.</p></div>",
        )
        .unwrap();
        let title = source
            .first_descendant_by_tag(source.root(), Tag::H1)
            .unwrap();
        let primary_body = source
            .first_descendant_by_tag(source.root(), Tag::Div)
            .unwrap();
        let reply_body = source
            .descendants(source.root())
            .find(|&node| source.attr(node, AttrName::Id) == Some("reply"))
            .unwrap();

        let mut builder = DiscussionBuilder::new().unwrap();
        assert!(builder.set_title(&source, title));
        assert!(builder.append_primary_body(&source, primary_body));
        builder.set_reply_heading("Replies");
        builder
            .append_reply(&source, 0, Some("Ada"), Some("now"), Some(reply_body))
            .unwrap();
        let result = builder.finish("test");
        let html = result.dom.html(result.root).unwrap();

        assert!(html.contains("data-legible-kind=\"discussion\""));
        assert!(html.contains("data-legible-primary"));
        assert!(html.contains("data-legible-replies"));
        assert!(html.contains("data-legible-reply-body"));
        assert!(html.contains("<em>text</em>"));
        assert!(html.contains("<code>code</code>"));
    }

    #[test]
    fn flattens_block_title_candidates_into_the_heading() {
        let source = Dom::parse_document("<div id=title>Block <span>title</span></div>").unwrap();
        let title = source
            .descendants(source.root())
            .find(|&node| source.attr(node, AttrName::Id) == Some("title"))
            .unwrap();
        let mut builder = DiscussionBuilder::new().unwrap();

        assert!(builder.set_title(&source, title));
        let result = builder.finish("test");
        let html = result.dom.html(result.root).unwrap();

        assert!(html.contains("<h1>Block title</h1>"));
        assert!(!html.contains("<h1><div"));
    }

    #[test]
    fn attaches_children_to_nearest_retained_ancestor() {
        let source = Dom::parse_document("<p>root</p><p>child</p><p>sibling</p>").unwrap();
        let mut builder = DiscussionBuilder::new().unwrap();
        builder.set_reply_heading("Replies");
        builder
            .append_reply(&source, 0, Some("root"), None, Some(body(&source, "root")))
            .unwrap();
        builder
            .append_reply(&source, 1, Some("deleted"), None, None)
            .unwrap();
        builder
            .append_reply(
                &source,
                3,
                Some("child"),
                None,
                Some(body(&source, "child")),
            )
            .unwrap();
        builder
            .append_reply(
                &source,
                0,
                Some("sibling"),
                None,
                Some(body(&source, "sibling")),
            )
            .unwrap();
        let result = builder.finish("test");
        let reply_items: Vec<_> = result
            .dom
            .descendants(result.root)
            .filter(|&node| result.dom.attr(node, AttrName::DataLegibleReply).is_some())
            .collect();

        assert_eq!(reply_items.len(), 3);
        let nested_list = result.dom.parent(reply_items[1]).unwrap();
        assert_eq!(result.dom.parent(nested_list), Some(reply_items[0]));
        assert_eq!(
            result.dom.parent(reply_items[2]),
            result.dom.parent(reply_items[0])
        );
    }

    #[test]
    fn handles_thousands_of_replies_without_recursive_state() {
        let source = Dom::parse_document("<p>reply</p>").unwrap();
        let reply = source
            .first_descendant_by_tag(source.root(), Tag::P)
            .unwrap();
        let mut builder = DiscussionBuilder::new().unwrap();
        builder.set_reply_heading("Replies");
        for _ in 0..2_000 {
            builder
                .append_reply(&source, 0, Some("reader"), None, Some(reply))
                .unwrap();
        }
        let result = builder.finish("test");
        assert_eq!(
            result
                .dom
                .descendants(result.root)
                .filter(|&node| result.dom.attr(node, AttrName::DataLegibleReply).is_some())
                .count(),
            2_000
        );
    }
}
