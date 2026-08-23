//! Conservative extraction for static discussion markup.

use super::{
    DocumentContext, SpecializedExtractor, SpecializedResult, discussion::DiscussionBuilder,
    has_class,
};
use crate::dom::{AttrName, Dom, NodeId, Tag};
use std::collections::HashSet;

pub(super) struct GenericDiscussionExtractor;

struct DiscussionData {
    primary: NodeId,
    title: NodeId,
    body: NodeId,
    replies: Vec<ReplyRecord>,
}

struct ReplyRecord {
    short_id: String,
    body: NodeId,
    author: Option<String>,
    time: Option<String>,
    normalized_body: String,
    depth: usize,
}

impl SpecializedExtractor for GenericDiscussionExtractor {
    fn matches(&self, context: &DocumentContext<'_>) -> bool {
        has_discussion_markers(context.dom)
    }

    fn extract(&self, context: &DocumentContext<'_>) -> Option<SpecializedResult> {
        let source = context.dom;
        let discussion = analyze_discussion(source)?;

        let mut builder = DiscussionBuilder::new()?;
        if !builder.set_title(source, discussion.title) {
            return None;
        }
        let (author, time) = entry_metadata(source, discussion.primary, is_primary);
        if !builder.append_primary_byline(author.as_deref(), time.as_deref())
            || !builder.append_primary_body(source, discussion.body)
            || !builder.set_reply_heading("Replies")
        {
            return None;
        }
        let mut seen_replies = HashSet::new();
        for reply in &discussion.replies {
            if !seen_replies.insert(reply.short_id.as_str()) {
                continue;
            }
            builder.append_reply(
                source,
                reply.depth,
                reply.author.as_deref(),
                reply.time.as_deref(),
                Some(reply.body),
            )?;
        }
        Some(builder.finish("generic-discussion"))
    }
}

fn has_discussion_markers(dom: &Dom) -> bool {
    let mut primary = false;
    let mut body = false;
    let mut replies = 0_u8;
    for node in dom.descendants(dom.root()) {
        primary |= has_class(dom, node, "h-entry");
        body |= has_class(dom, node, "story_text") || has_class(dom, node, "e-content");
        if has_class(dom, node, "comment")
            && dom
                .attr_by_local_name(node, "data-shortid")
                .is_some_and(|value| !value.is_empty())
        {
            replies = replies.saturating_add(1);
        }
        if primary && body && replies >= 2 {
            return true;
        }
    }
    false
}

fn analyze_discussion(dom: &Dom) -> Option<DiscussionData> {
    for primary in dom
        .descendants(dom.root())
        .filter(|&node| is_primary(dom, node))
    {
        let Some(title) = find_title(dom, primary) else {
            continue;
        };
        for container in dom.ancestors(primary) {
            if matches!(dom.tag(container), Some(Tag::Body | Tag::Html)) || container == dom.root()
            {
                break;
            }
            let Some(body) = find_primary_body(dom, container, primary) else {
                continue;
            };
            let Some(replies) = dom
                .descendants(container)
                .filter(|&node| dom.tag(node) == Some(Tag::Ol) && has_class(dom, node, "comments"))
                .map(|comments| collect_replies(dom, comments))
                .find(|replies| {
                    replies
                        .iter()
                        .map(|reply| reply.normalized_body.as_str())
                        .collect::<HashSet<_>>()
                        .len()
                        >= 2
                })
            else {
                continue;
            };
            return Some(DiscussionData {
                primary,
                title,
                body,
                replies,
            });
        }
    }
    None
}

fn is_primary(dom: &Dom, node: NodeId) -> bool {
    has_class(dom, node, "h-entry")
        && dom
            .descendants(node)
            .any(|descendant| has_class(dom, descendant, "u-url"))
}

fn find_title(dom: &Dom, primary: NodeId) -> Option<NodeId> {
    dom.descendants(primary).find(|&node| {
        has_class(dom, node, "u-url")
            && dom
                .ancestors(node)
                .take_while(|&ancestor| ancestor != primary)
                .any(|ancestor| {
                    dom.attr(ancestor, AttrName::Role).is_some_and(|role| {
                        role.split_ascii_whitespace().any(|role| role == "heading")
                    }) && dom.attr_by_local_name(ancestor, "aria-level") == Some("1")
                })
    })
}

fn find_primary_body(dom: &Dom, container: NodeId, primary: NodeId) -> Option<NodeId> {
    dom.descendants(container).find(|&node| {
        (has_class(dom, node, "story_text") || has_class(dom, node, "e-content"))
            && has_content(dom, node)
            && !dom
                .ancestors(node)
                .any(|ancestor| is_reply_container(dom, ancestor))
            && dom
                .ancestors(node)
                .find(|&ancestor| is_primary(dom, ancestor))
                .is_none_or(|entry| entry == primary)
    })
}

fn collect_replies(dom: &Dom, comments: NodeId) -> Vec<ReplyRecord> {
    dom.descendants(comments)
        .filter(|&node| is_reply_container(dom, node))
        .filter_map(|node| {
            let body = find_reply_body(dom, node)?;
            let short_id = dom.attr_by_local_name(node, "data-shortid")?.to_owned();
            let (author, time) = entry_metadata(dom, node, is_reply_container);
            Some(ReplyRecord {
                short_id,
                body,
                author,
                time,
                normalized_body: normalized_text(dom, body),
                depth: reply_depth(dom, node),
            })
        })
        .collect()
}

fn find_reply_body(dom: &Dom, reply: NodeId) -> Option<NodeId> {
    dom.descendants(reply).find(|&node| {
        (has_class(dom, node, "comment_text")
            || has_class(dom, node, "comment-body")
            || has_class(dom, node, "e-content"))
            && dom
                .ancestors(node)
                .find(|&ancestor| is_reply_container(dom, ancestor))
                == Some(reply)
            && has_content(dom, node)
    })
}

fn is_reply_container(dom: &Dom, node: NodeId) -> bool {
    has_class(dom, node, "comment") && dom.attr_by_local_name(node, "data-shortid").is_some()
}

fn entry_metadata(
    dom: &Dom,
    entry: NodeId,
    is_container: fn(&Dom, NodeId) -> bool,
) -> (Option<String>, Option<String>) {
    let direct = |node| {
        dom.ancestors(node)
            .find(|&ancestor| is_container(dom, ancestor))
            == Some(entry)
    };
    let mut author = None;
    let mut time = None;
    for node in dom.descendants(entry).filter(|&node| direct(node)) {
        if author.is_none()
            && dom.tag(node) == Some(Tag::A)
            && (has_class(dom, node, "user_is_author")
                || has_class(dom, node, "p-author")
                || dom
                    .ancestors(node)
                    .take_while(|&ancestor| ancestor != entry)
                    .any(|ancestor| has_class(dom, ancestor, "byline")))
        {
            let value = normalized_text(dom, node);
            if !value.is_empty() {
                author = Some(value);
            }
        }
        if time.is_none() && dom.tag(node) == Some(Tag::Time) {
            let value = normalized_text(dom, node);
            if !value.is_empty() {
                time = Some(value);
            }
        }
        if author.is_some() && time.is_some() {
            break;
        }
    }
    (author, time)
}

fn reply_depth(dom: &Dom, reply: NodeId) -> usize {
    dom.ancestors(reply)
        .filter(|&ancestor| {
            dom.tag(ancestor) == Some(Tag::Ol) && has_class(dom, ancestor, "comments")
        })
        .count()
}

fn has_content(dom: &Dom, node: NodeId) -> bool {
    !normalized_text(dom, node).is_empty()
}

fn normalized_text(dom: &Dom, node: NodeId) -> String {
    dom.text(node)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_a_nested_static_discussion() {
        let dom = Dom::parse_document(
            r#"<main>
                <div class="h-entry"><span role="heading" aria-level="1"><a class="u-url">Question</a></span><a class="user_is_author">Ana</a><time>Today</time></div>
                <div class="story_text"><p>Primary body with enough useful text.</p></div>
                <ol class="comments"><li><div class="comment" data-shortid="one"><div class="byline"><a>Ben</a><time>Later</time></div><div class="comment_text"><p>First reply.</p></div></div>
                <ol class="comments"><li><div class="comment" data-shortid="two"><div class="byline"><a>Cy</a></div><div class="comment_text"><p>Nested reply.</p></div></div></li><li><div class="comment" data-shortid="other"><div class="byline"><a>Cy</a></div><div class="comment_text"><p>Nested reply.</p></div></div></li><li><div class="comment" data-shortid="two"><div class="byline"><a>Cy</a></div><div class="comment_text"><p>Nested reply.</p></div></div></li></ol></li></ol>
            </main>"#,
        )
        .unwrap();
        let context = DocumentContext {
            dom: &dom,
            source_uri: None,
        };

        assert!(GenericDiscussionExtractor.matches(&context));
        let result = GenericDiscussionExtractor.extract(&context).unwrap();
        assert_eq!(result.identity, "generic-discussion");
        assert!(result.dom.text(result.root).contains("Primary body"));
        assert_eq!(
            result.dom.text(result.root).matches("Nested reply").count(),
            2
        );
    }

    #[test]
    fn does_not_extract_an_article_comment_widget_or_card_list() {
        for html in [
            r#"<article class="h-entry"><h1><a class="u-url">Article</a></h1><div class="e-content">Body</div></article><div class="comment" data-shortid="one"><div class="comment_text">One comment</div></div>"#,
            r#"<div class="h-entry"><h1><a class="u-url">Cards</a></h1></div><div class="story_text">Intro</div><div class="comment" data-shortid="one"><div>Card one</div></div><div class="comment" data-shortid="two"><div>Card two</div></div>"#,
        ] {
            let dom = Dom::parse_document(html).unwrap();
            let context = DocumentContext {
                dom: &dom,
                source_uri: None,
            };
            assert!(GenericDiscussionExtractor.extract(&context).is_none());
        }
    }
}
