//! Static old-Reddit submission and comment extraction.

use super::{
    DocumentContext, SpecializedExtractor, SpecializedResult, discussion::DiscussionBuilder,
    has_class,
};
use crate::dom::{Dom, NodeId};
use url::Url;

pub(super) struct RedditExtractor;

impl SpecializedExtractor for RedditExtractor {
    fn matches(&self, context: &DocumentContext<'_>) -> bool {
        let Some(submission) = find_submission(context.dom) else {
            return false;
        };
        let comments = comment_nodes(context.dom, submission);
        let has_submission_body = find_submission_body(context.dom, submission).is_some();
        let has_comment_body = comments
            .iter()
            .any(|&comment| find_comment_body(context.dom, comment).is_some());
        if !has_submission_body && !has_comment_body {
            return false;
        }

        let url_matches = context.source_uri.is_some_and(is_reddit_thread_url);
        let markup_matches = has_old_reddit_marker(context.dom);
        url_matches || markup_matches
    }

    fn extract(&self, context: &DocumentContext<'_>) -> Option<SpecializedResult> {
        let source = context.dom;
        let submission = find_submission(source)?;
        let title = find_submission_title(source, submission)?;
        let comments = comment_nodes(source, submission);
        let body = find_submission_body(source, submission);
        let has_renderable_comments = comments
            .iter()
            .any(|&comment| find_comment_body(source, comment).is_some());
        if body.is_none() && !has_renderable_comments {
            return None;
        }

        let mut builder = DiscussionBuilder::new()?;
        if !builder.set_title(source, title) {
            return None;
        }
        let (author, time) = entry_metadata(source, submission);
        if !builder.append_primary_byline(author.as_deref(), time.as_deref()) {
            return None;
        }
        if let Some(body) = body
            && !builder.append_primary_body(source, body)
        {
            return None;
        }

        if has_renderable_comments {
            if !builder.set_reply_heading("Comments") {
                return None;
            }
            for comment in comments {
                let (author, time) = entry_metadata(source, comment);
                builder.append_reply(
                    source,
                    comment_depth(source, comment),
                    author.as_deref(),
                    time.as_deref(),
                    find_comment_body(source, comment),
                )?;
            }
        }

        Some(builder.finish("reddit"))
    }
}

fn is_reddit_thread_url(url: &Url) -> bool {
    let Some(host) = url.host_str() else {
        return false;
    };
    let host = host.strip_prefix("www.").unwrap_or(host);
    (host == "reddit.com" || host.ends_with(".reddit.com"))
        && url
            .path_segments()
            .into_iter()
            .flatten()
            .any(|segment| segment == "comments")
}

fn has_old_reddit_marker(dom: &Dom) -> bool {
    dom.descendants(dom.root()).any(|node| {
        dom.attr(node, crate::dom::AttrName::Id) == Some("siteTable")
            || has_class(dom, node, "linklisting")
            || has_class(dom, node, "nestedlisting")
    })
}

fn find_submission(dom: &Dom) -> Option<NodeId> {
    dom.descendants(dom.root()).find(|&node| {
        has_class(dom, node, "thing")
            && has_class(dom, node, "link")
            && dom
                .attr_by_local_name(node, "data-fullname")
                .is_some_and(|value| value.starts_with("t3_"))
            && find_submission_title(dom, node).is_some()
    })
}

fn find_submission_title(dom: &Dom, submission: NodeId) -> Option<NodeId> {
    dom.descendants(submission)
        .find(|&node| has_class(dom, node, "title") && dom.tag(node) == Some(crate::dom::Tag::A))
}

fn comment_nodes(dom: &Dom, submission: NodeId) -> Vec<NodeId> {
    let scope = dom
        .descendants(dom.root())
        .find(|&node| {
            has_class(dom, node, "nestedlisting")
                && dom
                    .descendants(node)
                    .any(|descendant| is_comment(dom, descendant))
        })
        .or_else(|| {
            dom.ancestors(submission).find(|&ancestor| {
                dom.attr_by_local_name(ancestor, "id") == Some("siteTable")
                    && dom
                        .descendants(ancestor)
                        .any(|descendant| is_comment(dom, descendant))
            })
        })
        .or_else(|| {
            dom.parent(submission).filter(|&parent| {
                dom.descendants(parent)
                    .any(|descendant| is_comment(dom, descendant))
            })
        });

    let Some(scope) = scope else {
        return Vec::new();
    };

    dom.descendants(scope)
        .filter(|&node| is_comment(dom, node))
        .collect()
}

fn is_comment(dom: &Dom, node: NodeId) -> bool {
    has_class(dom, node, "thing")
        && has_class(dom, node, "comment")
        && (dom
            .attr_by_local_name(node, "data-fullname")
            .is_some_and(|value| value.starts_with("t1_"))
            || find_comment_body(dom, node).is_some())
}

fn find_submission_body(dom: &Dom, entry: NodeId) -> Option<NodeId> {
    dom.descendants(entry)
        .find(|&node| {
            has_class(dom, node, "usertext-body")
                && dom
                    .ancestors(node)
                    .find(|&ancestor| is_submission(dom, ancestor))
                    == Some(entry)
        })
        .and_then(|body| {
            dom.descendants(body)
                .find(|&node| has_class(dom, node, "md"))
        })
        .filter(|&body| is_renderable_body(dom, body))
}

fn find_comment_body(dom: &Dom, comment: NodeId) -> Option<NodeId> {
    let body = dom.descendants(comment).find(|&node| {
        has_class(dom, node, "usertext-body")
            && dom
                .ancestors(node)
                .find(|&ancestor| is_comment_container(dom, ancestor))
                == Some(comment)
    })?;
    let body = dom
        .descendants(body)
        .find(|&node| has_class(dom, node, "md"))?;
    if !is_renderable_body(dom, body) {
        None
    } else {
        Some(body)
    }
}

fn is_submission(dom: &Dom, node: NodeId) -> bool {
    has_class(dom, node, "thing")
        && has_class(dom, node, "link")
        && dom
            .attr_by_local_name(node, "data-fullname")
            .is_some_and(|value| value.starts_with("t3_"))
}

fn is_comment_container(dom: &Dom, node: NodeId) -> bool {
    has_class(dom, node, "thing") && has_class(dom, node, "comment")
}

fn entry_metadata(dom: &Dom, entry: NodeId) -> (Option<String>, Option<String>) {
    let author = dom.descendants(entry).find(|&node| {
        has_class(dom, node, "author")
            && normalized_text(dom, node) != "[deleted]"
            && dom
                .ancestors(node)
                .find(|&ancestor| is_entry_container(dom, ancestor))
                == Some(entry)
    });
    let author = author
        .map(|node| normalized_text(dom, node))
        .filter(|value| !value.is_empty());
    let time = dom
        .descendants(entry)
        .find(|&node| {
            (dom.tag(node) == Some(crate::dom::Tag::Time) || has_class(dom, node, "live-timestamp"))
                && dom
                    .ancestors(node)
                    .find(|&ancestor| is_entry_container(dom, ancestor))
                    == Some(entry)
        })
        .map(|node| normalized_text(dom, node))
        .filter(|value| !value.is_empty());
    (author, time)
}

fn is_entry_container(dom: &Dom, node: NodeId) -> bool {
    is_submission(dom, node) || is_comment_container(dom, node)
}

fn is_unavailable_body(dom: &Dom, body: NodeId) -> bool {
    matches!(
        normalized_text(dom, body).to_ascii_lowercase().as_str(),
        "[deleted]" | "[removed]"
    )
}

fn is_renderable_body(dom: &Dom, body: NodeId) -> bool {
    !normalized_text(dom, body).is_empty() && !is_unavailable_body(dom, body)
}

fn comment_depth(dom: &Dom, comment: NodeId) -> usize {
    dom.ancestors(comment)
        .filter(|&ancestor| is_comment(dom, ancestor))
        .count()
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
    use url::Url;

    fn context<'a>(dom: &'a Dom, url: &'a str) -> DocumentContext<'a> {
        let url = Box::leak(Box::new(Url::parse(url).unwrap()));
        DocumentContext {
            dom,
            source_uri: Some(url),
        }
    }

    #[test]
    fn recognizes_old_reddit_threads() {
        let dom = Dom::parse_document(
            "<div id='siteTable'><div class='thing link' data-fullname='t3_1'><p><a class='title'>Post</a></p><div class='usertext-body'><div class='md'><p>Body</p></div></div></div><div class='thing comment' data-fullname='t1_1'><div class='usertext-body'><div class='md'><p>Reply</p></div></div></div></div>",
        )
        .unwrap();
        assert!(RedditExtractor.matches(&context(
            &dom,
            "https://www.reddit.com/r/rust/comments/1/post/"
        )));
    }

    #[test]
    fn rejects_empty_or_deleted_submission_bodies() {
        for body in ["<div class='md'></div>", "<div class='md'>[deleted]</div>"] {
            let html = format!(
                "<div id='siteTable'><div class='thing link' data-fullname='t3_1'><a class='title'>Post</a><div class='usertext-body'>{body}</div></div></div>"
            );
            let dom = Dom::parse_document(&html).unwrap();
            assert!(!RedditExtractor.matches(&context(
                &dom,
                "https://old.reddit.com/r/rust/comments/1/post/"
            )));
        }
    }

    #[test]
    fn ignores_comments_outside_the_thread_listing() {
        let dom = Dom::parse_document(
            "<div id='siteTable'><div class='thing link' data-fullname='t3_1'><a class='title'>Post</a><div class='usertext-body'><div class='md'>Body</div></div></div></div><div class='sidebar'><div class='thing comment' data-fullname='t1_other'><div class='usertext-body'><div class='md'>Other</div></div></div></div>",
        )
        .unwrap();
        let submission = find_submission(&dom).unwrap();

        assert!(comment_nodes(&dom, submission).is_empty());
    }

    #[test]
    fn does_not_match_a_generic_thing_list() {
        let dom = Dom::parse_document(
            "<div><div class='thing link'><a class='title'>Post</a><div class='usertext-body'><div class='md'><p>Body</p></div></div></div></div>",
        )
        .unwrap();
        assert!(!RedditExtractor.matches(&context(&dom, "https://example.test/r/rust")));
    }

    #[test]
    fn does_not_match_an_empty_reddit_application_shell() {
        let dom = Dom::parse_document(
            "<div id='siteTable'><div class='thing link' data-fullname='t3_1'><a class='title'>Post</a><div class='usertext-body'></div></div></div>",
        )
        .unwrap();
        assert!(!RedditExtractor.matches(&context(
            &dom,
            "https://old.reddit.com/r/rust/comments/1/post/"
        )));
    }

    #[test]
    fn does_not_borrow_nested_comment_metadata() {
        let dom = Dom::parse_document(
            "<div class='thing comment' data-fullname='t1_parent'><div class='child'><div class='thing comment' data-fullname='t1_child'><a class='author'>child</a><time>later</time><div class='usertext-body'><div class='md'><p>Reply</p></div></div></div></div></div>",
        )
        .unwrap();
        let parent = dom
            .descendants(dom.root())
            .find(|&node| dom.attr_by_local_name(node, "data-fullname") == Some("t1_parent"))
            .unwrap();

        assert_eq!(entry_metadata(&dom, parent), (None, None));
    }
}
