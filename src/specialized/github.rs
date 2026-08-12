//! GitHub issue and pull request discussion extraction.

use super::{
    DocumentContext, SpecializedExtractor, SpecializedResult, append_text, class_contains,
    create_element, has_class, import_children, new_output,
};
use crate::dom::{AttrName, Dom, NodeId, Tag};
use crate::page_kind::PageKind;

pub(super) struct GitHubExtractor;

impl SpecializedExtractor for GitHubExtractor {
    fn matches(&self, context: &DocumentContext<'_>) -> bool {
        let host_matches = context
            .source_uri
            .and_then(|url| url.host_str())
            .is_some_and(|host| host.eq_ignore_ascii_case("github.com"));
        let has_title = find_title(context.dom).is_some();
        let has_discussion = context
            .dom
            .descendants(context.dom.root())
            .any(|node| is_discussion_body(context.dom, node));
        let has_github_signature = context.dom.descendants(context.dom.root()).any(|node| {
            has_class(context.dom, node, "js-issue-title")
                || context
                    .dom
                    .attr_by_local_name(node, "data-turbo-body")
                    .is_some()
                || context
                    .dom
                    .attr_by_local_name(node, "data-testid")
                    .is_some_and(|value| {
                        matches!(value, "issue-body" | "issue-comment" | "issue-title")
                    })
        });
        has_title && has_discussion && (host_matches || has_github_signature)
    }

    fn extract(&self, context: &DocumentContext<'_>) -> Option<SpecializedResult> {
        let source = context.dom;
        let title = find_title(source)?;
        let bodies: Vec<_> = discussion_bodies(source).collect();
        if bodies.is_empty() {
            return None;
        }

        let (mut dom, root) = new_output()?;
        let heading = create_element(&mut dom, root, Tag::H1)?;
        if source.tag(title) == Some(Tag::H1) {
            if !import_children(source, title, &mut dom, heading) {
                return None;
            }
        } else {
            append_text(&mut dom, heading, source.text(title).trim());
        }

        append_entry(source, &mut dom, root, bodies[0], false)?;
        if bodies.len() > 1 {
            let comments_heading = create_element(&mut dom, root, Tag::H2)?;
            append_text(&mut dom, comments_heading, "Discussion");
            for &entry in &bodies[1..] {
                append_entry(source, &mut dom, root, entry, true)?;
            }
        }

        Some(SpecializedResult {
            dom,
            root,
            kind: PageKind::Discussion,
            identity: "github",
        })
    }
}

fn find_title(dom: &Dom) -> Option<NodeId> {
    dom.descendants(dom.root())
        .find(|&node| {
            has_class(dom, node, "js-issue-title")
                || dom.attr_by_local_name(node, "data-testid") == Some("issue-title")
        })
        .or_else(|| {
            dom.descendants(dom.root()).find(|&node| {
                dom.tag(node) == Some(Tag::H1)
                    && (class_contains(dom, node, "gh-header-title")
                        || dom.descendants(node).any(|child| {
                            has_class(dom, child, "js-issue-title")
                                || has_class(dom, child, "markdown-title")
                        }))
            })
        })
}

fn discussion_bodies(dom: &Dom) -> impl Iterator<Item = NodeId> {
    let nodes = dom.element_descendants_snapshot_with_depth(dom.root());
    let mut subtree_has_match = vec![false; dom.len()];
    let mut bodies = Vec::new();
    for &(node, _) in nodes.iter().rev() {
        let matches = is_discussion_body(dom, node);
        if matches && !subtree_has_match[node.index()] {
            bodies.push(node);
        }
        if (matches || subtree_has_match[node.index()])
            && let Some(parent) = dom.parent(node)
        {
            subtree_has_match[parent.index()] = true;
        }
    }
    bodies.reverse();
    bodies.into_iter()
}

fn is_discussion_body(dom: &Dom, node: NodeId) -> bool {
    has_class(dom, node, "js-comment-body")
        || has_class(dom, node, "comment-body") && has_class(dom, node, "markdown-body")
        || has_class(dom, node, "review-comment-contents")
        || dom
            .attr_by_local_name(node, "data-testid")
            .is_some_and(|value| {
                matches!(
                    value,
                    "issue-body" | "comment-body" | "pull-request-review-body"
                )
            })
}

fn append_entry(
    source: &Dom,
    output: &mut Dom,
    root: NodeId,
    body: NodeId,
    comment: bool,
) -> Option<()> {
    let container = entry_container(source, body);
    if let Some(author) = find_author(source, container) {
        let heading = create_element(output, root, if comment { Tag::H3 } else { Tag::P })?;
        if comment {
            append_text(output, heading, source.text(author).trim());
        } else {
            let strong = create_element(output, heading, Tag::Strong)?;
            append_text(output, strong, source.text(author).trim());
        }
        if let Some(time) = source
            .descendants(container)
            .find(|&node| source.tag(node) == Some(Tag::Time))
        {
            let value = source.text(time);
            let value = value.trim();
            if !value.is_empty() {
                append_text(output, heading, " · ");
                append_text(output, heading, value);
            }
        }
    } else if comment {
        let heading = create_element(output, root, Tag::H3)?;
        append_text(output, heading, "Comment");
    }

    let article = create_element(output, root, Tag::Article)?;
    if !import_children(source, body, output, article) {
        return None;
    }
    Some(())
}

fn entry_container(dom: &Dom, body: NodeId) -> NodeId {
    dom.ancestors(body)
        .find(|&node| {
            has_class(dom, node, "timeline-comment")
                || has_class(dom, node, "js-timeline-item")
                || dom
                    .attr_by_local_name(node, "data-testid")
                    .is_some_and(|value| {
                        matches!(
                            value,
                            "issue-comment" | "issue-body-container" | "pull-request-review"
                        )
                    })
        })
        .unwrap_or(body)
}

fn find_author(dom: &Dom, container: NodeId) -> Option<NodeId> {
    dom.descendants(container).find(|&node| {
        dom.tag(node) == Some(Tag::A)
            && (has_class(dom, node, "author")
                || dom.attr(node, AttrName::Rel).is_some_and(|rel| {
                    rel.split_whitespace()
                        .any(|token| token.eq_ignore_ascii_case("author"))
                })
                || dom.attr_by_local_name(node, "data-testid") == Some("comment-header-author"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_one_innermost_body_through_deep_wrappers() {
        let depth = 1_000;
        let mut html = String::from("<h1><bdi class='js-issue-title'>Title</bdi></h1>");
        html.push_str(&"<div data-testid='issue-body'>".repeat(depth));
        html.push_str("<div class='js-comment-body'><p>Content</p></div>");
        html.push_str(&"</div>".repeat(depth));
        let dom = Dom::parse_document(&html).unwrap();
        let bodies: Vec<_> = discussion_bodies(&dom).collect();

        assert_eq!(bodies.len(), 1);
        assert!(has_class(&dom, bodies[0], "js-comment-body"));
    }
}
