//! Hacker News listing and discussion extraction.

use super::{
    DocumentContext, SpecializedExtractor, SpecializedResult, append_text, create_element,
    discussion::DiscussionBuilder, has_class, new_output,
};
use crate::dom::{AttrName, Dom, NodeId, Tag};
use crate::page_kind::PageKind;

pub(super) struct HackerNewsExtractor;

impl SpecializedExtractor for HackerNewsExtractor {
    fn matches(&self, context: &DocumentContext<'_>) -> bool {
        let host_matches = context
            .source_uri
            .and_then(|url| url.host_str())
            .is_some_and(|host| host.eq_ignore_ascii_case("news.ycombinator.com"));
        let has_shell = context.dom.descendants(context.dom.root()).any(|node| {
            context.dom.tag(node) == Some(Tag::Table)
                && context.dom.attr(node, AttrName::Id) == Some("hnmain")
        });
        let has_story = story_rows(context.dom).next().is_some();
        has_story && (host_matches || has_shell)
    }

    fn extract(&self, context: &DocumentContext<'_>) -> Option<SpecializedResult> {
        let comments: Vec<_> = context
            .dom
            .descendants(context.dom.root())
            .filter(|&node| {
                context.dom.tag(node) == Some(Tag::Tr) && has_class(context.dom, node, "comtr")
            })
            .collect();
        let item_url = context.source_uri.is_some_and(|url| {
            url.path() == "/item" && url.query_pairs().any(|(name, _)| name == "id")
        });
        let has_submission_text = context
            .dom
            .descendants(context.dom.root())
            .any(|node| has_class(context.dom, node, "toptext"));
        if item_url || has_submission_text || !comments.is_empty() {
            extract_discussion(context, &comments)
        } else {
            extract_listing(context)
        }
    }
}

fn story_rows(dom: &Dom) -> impl Iterator<Item = NodeId> + '_ {
    dom.descendants(dom.root()).filter(|&node| {
        dom.tag(node) == Some(Tag::Tr)
            && has_class(dom, node, "athing")
            && find_descendant_with_class(dom, node, "titleline").is_some()
    })
}

fn extract_listing(context: &DocumentContext<'_>) -> Option<SpecializedResult> {
    let source = context.dom;
    let stories: Vec<_> = story_rows(source).collect();
    if stories.is_empty() {
        return None;
    }
    let (mut dom, root) = new_output()?;
    let heading = create_element(&mut dom, root, Tag::H1)?;
    append_text(&mut dom, heading, "Hacker News");
    let list = create_element(&mut dom, root, Tag::Ol)?;

    for story in stories {
        let titleline = find_descendant_with_class(source, story, "titleline")?;
        let title_link = source
            .descendants(titleline)
            .find(|&node| source.tag(node) == Some(Tag::A))?;
        let item = create_element(&mut dom, list, Tag::Li)?;
        let link = dom.import_subtree(source, title_link).ok()?;
        dom.append_child(item, link);
        if let Some(metadata) = story_metadata(source, story) {
            let details = create_element(&mut dom, item, Tag::P)?;
            append_text(&mut dom, details, &metadata);
        }
    }

    Some(SpecializedResult {
        dom,
        root,
        kind: PageKind::Listing,
        identity: "hacker-news",
    })
}

fn extract_discussion(
    context: &DocumentContext<'_>,
    comments: &[NodeId],
) -> Option<SpecializedResult> {
    let source = context.dom;
    let story = story_rows(source).next()?;
    let titleline = find_descendant_with_class(source, story, "titleline")?;
    let title_link = source
        .descendants(titleline)
        .find(|&node| source.tag(node) == Some(Tag::A))?;
    let mut builder = DiscussionBuilder::new()?;
    if !builder.set_title(source, title_link) {
        return None;
    }

    if let Some(metadata) = story_metadata(source, story)
        && !builder.append_primary_text(&metadata)
    {
        return None;
    }
    if let Some(top_text) = source
        .descendants(source.root())
        .find(|&node| has_class(source, node, "toptext"))
        && !builder.append_primary_body(source, top_text)
    {
        return None;
    }

    let has_renderable_comments = comments
        .iter()
        .any(|&comment| find_descendant_with_class(source, comment, "commtext").is_some());
    if has_renderable_comments {
        if !builder.set_reply_heading("Comments") {
            return None;
        }
        append_comments(source, &mut builder, comments)?;
    }

    Some(builder.finish("hacker-news"))
}

fn append_comments(
    source: &Dom,
    builder: &mut DiscussionBuilder,
    comments: &[NodeId],
) -> Option<()> {
    for &comment in comments {
        let depth = comment_depth(source, comment);
        let Some(body) = find_descendant_with_class(source, comment, "commtext") else {
            builder.append_reply(source, depth, None, None, None)?;
            continue;
        };
        let author = find_descendant_with_class(source, comment, "hnuser")
            .map(|node| normalized_text(source, node));
        let age = find_descendant_with_class(source, comment, "age")
            .map(|node| normalized_text(source, node));
        builder.append_reply(source, depth, author.as_deref(), age.as_deref(), Some(body))?;
    }
    Some(())
}

fn comment_depth(dom: &Dom, comment: NodeId) -> usize {
    let width = dom
        .descendants(comment)
        .find(|&node| dom.tag(node) == Some(Tag::Td) && has_class(dom, node, "ind"))
        .and_then(|node| {
            dom.attr(node, AttrName::Width).or_else(|| {
                dom.descendants(node)
                    .find(|&child| dom.tag(child) == Some(Tag::Img))
                    .and_then(|image| dom.attr(image, AttrName::Width))
            })
        })
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    width / 40
}

fn story_metadata(dom: &Dom, story: NodeId) -> Option<String> {
    let mut sibling = dom.next_sibling(story);
    let row = loop {
        let node = sibling?;
        if dom.is_element(node) {
            break node;
        }
        sibling = dom.next_sibling(node);
    };
    let subtext = find_descendant_with_class(dom, row, "subtext")?;
    let score =
        find_descendant_with_class(dom, subtext, "score").map(|node| normalized_text(dom, node));
    let author =
        find_descendant_with_class(dom, subtext, "hnuser").map(|node| normalized_text(dom, node));
    let age =
        find_descendant_with_class(dom, subtext, "age").map(|node| normalized_text(dom, node));
    let comments = dom
        .descendants(subtext)
        .filter(|&node| dom.tag(node) == Some(Tag::A))
        .map(|node| normalized_text(dom, node))
        .find(|text| {
            let text = text.to_ascii_lowercase();
            text == "discuss" || text.contains("comment")
        });

    let mut metadata = Vec::new();
    if let Some(score) = score.filter(|value| !value.is_empty()) {
        metadata.push(score);
    }
    if let Some(author) = author.filter(|value| !value.is_empty()) {
        metadata.push(format!("by {author}"));
    }
    if let Some(age) = age.filter(|value| !value.is_empty()) {
        metadata.push(age);
    }
    if let Some(comments) = comments.filter(|value| !value.is_empty()) {
        metadata.push(format!("· {comments}"));
    }
    (!metadata.is_empty()).then(|| metadata.join(" "))
}

fn normalized_text(dom: &Dom, node: NodeId) -> String {
    dom.text(node)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn find_descendant_with_class(dom: &Dom, root: NodeId, class: &str) -> Option<NodeId> {
    dom.descendants(root)
        .find(|&node| has_class(dom, node, class))
}
