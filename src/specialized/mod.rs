//! Specialized extraction for pages whose structure is not article prose.

mod ai_conversation;
mod discourse;
mod discussion;
mod generic_discussion;
mod github;
mod hacker_news;
mod reddit;

use crate::dom::{AttrName, Dom, NodeId};
use crate::page_kind::PageKind;
use crate::tokens::{any_token_contains, has_any_token, has_token};
use url::Url;

/// Read-only document data used for cheap extractor recognition.
pub(crate) struct DocumentContext<'a> {
    pub(crate) dom: &'a Dom,
    pub(crate) source_uri: Option<&'a Url>,
}

/// A canonical subtree produced by a specialized extractor.
pub(crate) struct SpecializedResult {
    pub(crate) dom: Dom,
    pub(crate) root: NodeId,
    pub(crate) kind: PageKind,
    pub(crate) identity: &'static str,
}

/// Converts one recognized page type into canonical semantic HTML.
trait SpecializedExtractor: Sync {
    fn matches(&self, context: &DocumentContext<'_>) -> bool;
    fn extract(&self, context: &DocumentContext<'_>) -> Option<SpecializedResult>;
}

static EXTRACTORS: [&dyn SpecializedExtractor; 6] = [
    &hacker_news::HackerNewsExtractor,
    &github::GitHubExtractor,
    &discourse::DiscourseExtractor,
    &reddit::RedditExtractor,
    &generic_discussion::GenericDiscussionExtractor,
    &ai_conversation::AiConversationExtractor,
];

/// Runs the first high-confidence extractor in registry order.
pub(crate) fn extract(context: &DocumentContext<'_>) -> Option<SpecializedResult> {
    // Ordinary pages are common. Avoid running each specialized recognizer's
    // full DOM scan when the source has none of their identifying markers.
    // Keep share URLs in the scan because AI conversation extraction uses the
    // URL as its primary signature.
    if !has_specialized_signature(context) {
        return None;
    }
    extract_with_registry(context, &EXTRACTORS)
}

fn extract_with_registry(
    context: &DocumentContext<'_>,
    extractors: &[&dyn SpecializedExtractor],
) -> Option<SpecializedResult> {
    extractors
        .iter()
        .find(|extractor| extractor.matches(context))
        .and_then(|extractor| extractor.extract(context))
}

fn has_specialized_signature(context: &DocumentContext<'_>) -> bool {
    if context.source_uri.is_some_and(is_ai_share_url) {
        return true;
    }
    context
        .dom
        .descendants(context.dom.root())
        .any(|node| has_specialized_node_marker(context.dom, node))
}

/// Checks all specialized attributes in one pass. Most source elements have
/// zero or one attribute. Repeated named lookups made ordinary pages scan the
/// same short attribute list up to seven times per element.
fn has_specialized_node_marker(dom: &Dom, node: NodeId) -> bool {
    dom.attrs(node).iter().any(|attribute| {
        let value = attribute.value.as_ref();
        if attribute.is_named(AttrName::Id) {
            return matches!(value, "siteTable" | "hnmain");
        }
        if attribute.is_named(AttrName::Class) {
            return has_any_token(
                value,
                &[
                    "athing",
                    "comtr",
                    "js-issue-title",
                    "js-comment-body",
                    "comment-body",
                    "markdown-body",
                    "review-comment-contents",
                    "topic-post",
                    "cooked",
                    "discourse-application",
                    "thing",
                    "link",
                    "linklisting",
                    "nestedlisting",
                    "comment_text",
                    "story_text",
                    "title",
                ],
            );
        }
        let name = attribute.name.local.as_ref();
        matches!(
            name,
            "data-turbo-body"
                | "data-testid"
                | "data-post-id"
                | "data-discourse-base-url"
                | "data-fullname"
        ) || name.bytes().any(|byte| byte.is_ascii_uppercase())
            && [
                "data-turbo-body",
                "data-testid",
                "data-post-id",
                "data-discourse-base-url",
                "data-fullname",
            ]
            .iter()
            .any(|expected| name.eq_ignore_ascii_case(expected))
    })
}

fn is_ai_share_url(url: &url::Url) -> bool {
    let Some(host) = url.host_str() else {
        return false;
    };
    let host = host.strip_prefix("www.").unwrap_or(host);
    if !matches!(
        host,
        "chatgpt.com" | "chat.openai.com" | "claude.ai" | "gemini.google.com" | "grok.com"
    ) {
        return false;
    }
    let mut segments = url.path_segments().into_iter().flatten();
    segments.next() == Some("share")
        && segments.next().is_some_and(|segment| !segment.is_empty())
        && segments.next().is_none()
}

pub(super) fn has_class(dom: &Dom, node: NodeId, expected: &str) -> bool {
    dom.attr(node, crate::dom::AttrName::Class)
        .is_some_and(|classes| has_token(classes, expected))
}

pub(super) fn class_contains(dom: &Dom, node: NodeId, needle: &str) -> bool {
    dom.attr(node, crate::dom::AttrName::Class)
        .is_some_and(|classes| any_token_contains(classes, needle))
}

pub(super) fn create_element(
    dom: &mut Dom,
    parent: NodeId,
    tag: crate::dom::Tag,
) -> Option<NodeId> {
    let node = dom.create_html_element(tag).ok()?;
    dom.append_child(parent, node);
    Some(node)
}

/// Creates a compact document with one canonical content root.
pub(super) fn new_output() -> Option<(Dom, NodeId)> {
    let dom = Dom::parse_document("<main></main>").ok()?;
    let root = dom.first_descendant_by_tag(dom.root(), crate::dom::Tag::Main)?;
    Some((dom, root))
}

pub(super) fn append_text(dom: &mut Dom, parent: NodeId, value: &str) -> bool {
    let Ok(text) = dom.create_text(value) else {
        return false;
    };
    dom.append_child(parent, text);
    true
}

pub(super) fn import_children(
    source: &Dom,
    source_root: NodeId,
    output: &mut Dom,
    destination: NodeId,
) -> bool {
    for child in source.children(source_root) {
        let Ok(child) = output.import_subtree(source, child) else {
            return false;
        };
        output.append_child(destination, child);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dom::AttrName;

    struct DummyExtractor;
    struct LaterExtractor;

    impl SpecializedExtractor for DummyExtractor {
        fn matches(&self, context: &DocumentContext<'_>) -> bool {
            context
                .dom
                .descendants(context.dom.root())
                .any(|node| context.dom.attr(node, AttrName::Id) == Some("dummy"))
        }

        fn extract(&self, _context: &DocumentContext<'_>) -> Option<SpecializedResult> {
            let (mut dom, root) = new_output()?;
            append_text(&mut dom, root, "Specialized content");
            Some(SpecializedResult {
                dom,
                root,
                kind: PageKind::Listing,
                identity: "dummy",
            })
        }
    }

    impl SpecializedExtractor for LaterExtractor {
        fn matches(&self, _context: &DocumentContext<'_>) -> bool {
            true
        }

        fn extract(&self, _context: &DocumentContext<'_>) -> Option<SpecializedResult> {
            let (mut dom, root) = new_output()?;
            append_text(&mut dom, root, "Later content");
            Some(SpecializedResult {
                dom,
                root,
                kind: PageKind::Discussion,
                identity: "later",
            })
        }
    }

    #[test]
    fn registry_runs_the_first_matching_extractor() {
        let dom = Dom::parse_document("<body><div id='dummy'></div></body>").unwrap();
        let context = DocumentContext {
            dom: &dom,
            source_uri: None,
        };
        let result = extract_with_registry(&context, &[&DummyExtractor, &LaterExtractor]).unwrap();

        assert_eq!(result.kind, PageKind::Listing);
        assert_eq!(result.identity, "dummy");
        assert_eq!(result.dom.text(result.root), "Specialized content");
    }

    #[test]
    fn registry_ignores_non_matching_extractors() {
        let dom = Dom::parse_document("<body><main>Ordinary page</main></body>").unwrap();
        let context = DocumentContext {
            dom: &dom,
            source_uri: None,
        };

        assert!(extract_with_registry(&context, &[&DummyExtractor]).is_none());
    }

    #[test]
    fn class_helpers_match_ascii_case_insensitively() {
        let dom = Dom::parse_document("<body><div class='GH-Header-Title utility'></div></body>")
            .unwrap();
        let node = dom
            .descendants(dom.root())
            .find(|&node| dom.attr(node, AttrName::Class).is_some())
            .unwrap();

        assert!(has_class(&dom, node, "gh-header-title"));
        assert!(class_contains(&dom, node, "HEADER-title"));
    }
}
