//! Static Discourse topic extraction.

use super::{
    DocumentContext, SpecializedExtractor, SpecializedResult, discussion::DiscussionBuilder,
    has_class,
};
use crate::dom::{AttrName, Dom, NodeId, Tag};
use url::Url;

pub(super) struct DiscourseExtractor;

impl SpecializedExtractor for DiscourseExtractor {
    fn matches(&self, context: &DocumentContext<'_>) -> bool {
        let posts = post_nodes(context.dom);
        if posts.is_empty() || find_title(context.dom).is_none() {
            return false;
        }

        let has_topic_body = posts.iter().any(|&post| {
            find_body(context.dom, post).is_some_and(|body| has_content(context.dom, body))
        });
        let url_matches = context.source_uri.is_some_and(is_topic_url);
        let markup_matches = has_discourse_marker(context.dom)
            || posts.iter().any(|&post| {
                context
                    .dom
                    .attr_by_local_name(post, "data-post-id")
                    .is_some()
                    && find_body(context.dom, post).is_some()
            });

        has_topic_body && (url_matches || markup_matches)
    }

    fn extract(&self, context: &DocumentContext<'_>) -> Option<SpecializedResult> {
        let source = context.dom;
        let title = find_title(source)?;
        let posts = post_nodes(source);
        let primary_body =
            find_body(source, *posts.first()?).filter(|&body| has_content(source, body));
        let has_reply_body = posts
            .iter()
            .skip(1)
            .any(|&post| find_body(source, post).is_some_and(|body| has_content(source, body)));
        if primary_body.is_none() && !has_reply_body {
            return None;
        }

        let mut builder = DiscussionBuilder::new()?;
        if !builder.set_title(source, title) {
            return None;
        }
        let (author, time) = post_metadata(source, posts[0]);
        if !builder.append_primary_byline(author.as_deref(), time.as_deref()) {
            return None;
        }
        if let Some(primary_body) = primary_body
            && !builder.append_primary_body(source, primary_body)
        {
            return None;
        }

        if has_reply_body {
            if !builder.set_reply_heading("Replies") {
                return None;
            }
            for &post in posts.iter().skip(1) {
                let (author, time) = post_metadata(source, post);
                let body = find_body(source, post).filter(|&body| has_content(source, body));
                builder.append_reply(source, 0, author.as_deref(), time.as_deref(), body)?;
            }
        }

        Some(builder.finish("discourse"))
    }
}

fn is_topic_url(url: &Url) -> bool {
    let mut segments = url.path_segments().into_iter().flatten();
    segments.next() == Some("t") && segments.next().is_some()
}

fn has_discourse_marker(dom: &Dom) -> bool {
    dom.descendants(dom.root()).any(|node| {
        (dom.tag(node) == Some(Tag::Meta)
            && dom
                .attr(node, AttrName::Name)
                .is_some_and(|name| name.eq_ignore_ascii_case("generator"))
            && dom
                .attr(node, AttrName::Content)
                .is_some_and(|content| content.to_ascii_lowercase().contains("discourse")))
            || dom
                .attr_by_local_name(node, "data-discourse-base-url")
                .is_some()
            || has_class(dom, node, "discourse-application")
    })
}

fn post_nodes(dom: &Dom) -> Vec<NodeId> {
    let snapshot: Vec<_> = dom.descendants(dom.root()).collect();
    snapshot
        .into_iter()
        .filter(|&node| is_post(dom, node))
        .filter(|&node| !dom.ancestors(node).any(|ancestor| is_post(dom, ancestor)))
        .collect()
}

fn is_post(dom: &Dom, node: NodeId) -> bool {
    has_class(dom, node, "topic-post")
        && (dom.attr_by_local_name(node, "data-post-id").is_some()
            || find_body(dom, node).is_some())
}

fn is_post_container(dom: &Dom, node: NodeId) -> bool {
    has_class(dom, node, "topic-post")
}

fn find_title(dom: &Dom) -> Option<NodeId> {
    dom.descendants(dom.root()).find(|&node| {
        dom.tag(node) == Some(Tag::H1)
            && (has_class(dom, node, "fancy-title")
                || has_class(dom, node, "topic-title")
                || dom.attr_by_local_name(node, "data-topic-title").is_some())
    })
}

fn find_body(dom: &Dom, post: NodeId) -> Option<NodeId> {
    dom.descendants(post).find(|&node| {
        has_class(dom, node, "cooked")
            && dom
                .ancestors(node)
                .find(|&ancestor| is_post_container(dom, ancestor))
                == Some(post)
    })
}

fn has_content(dom: &Dom, node: NodeId) -> bool {
    !normalized_text(dom, node).is_empty()
}

fn post_metadata(dom: &Dom, post: NodeId) -> (Option<String>, Option<String>) {
    let author = dom.descendants(post).find(|&node| {
        (has_class(dom, node, "username")
            || has_class(dom, node, "creator")
            || dom.attr_by_local_name(node, "data-user-card").is_some())
            && dom
                .ancestors(node)
                .find(|&ancestor| is_post_container(dom, ancestor))
                == Some(post)
    });
    let author = author
        .map(|node| normalized_text(dom, node))
        .filter(|value| !value.is_empty());
    let time = dom
        .descendants(post)
        .find(|&node| {
            (dom.tag(node) == Some(Tag::Time)
                || has_class(dom, node, "post-time")
                || has_class(dom, node, "relative-date"))
                && dom
                    .ancestors(node)
                    .find(|&ancestor| is_post_container(dom, ancestor))
                    == Some(post)
        })
        .map(|node| normalized_text(dom, node))
        .filter(|value| !value.is_empty());
    (author, time)
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
    fn recognizes_topic_posts_with_static_bodies() {
        let dom = Dom::parse_document(
            "<h1 class='fancy-title'>A topic</h1><article class='topic-post' data-post-id='1'><div class='cooked'><p>Body</p></div></article>",
        )
        .unwrap();
        assert!(DiscourseExtractor.matches(&context(&dom, "https://forum.test/t/topic/1")));
    }

    #[test]
    fn rejects_a_lookalike_without_topic_url_or_discourse_marker() {
        let dom = Dom::parse_document(
            "<h1 class='fancy-title'>A topic</h1><article class='topic-post'><div class='cooked'><p>Body</p></div></article>",
        )
        .unwrap();
        assert!(!DiscourseExtractor.matches(&context(&dom, "https://example.test/articles/topic")));
    }
}
