//! Specialized extraction for pages whose structure is not article prose.

mod ai_conversation;
mod discourse;
mod discussion;
mod github;
mod hacker_news;
mod reddit;

use crate::dom::{Dom, NodeId};
use crate::page_kind::PageKind;
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

static EXTRACTORS: [&dyn SpecializedExtractor; 5] = [
    &hacker_news::HackerNewsExtractor,
    &github::GitHubExtractor,
    &discourse::DiscourseExtractor,
    &reddit::RedditExtractor,
    &ai_conversation::AiConversationExtractor,
];

/// Runs the first high-confidence extractor in registry order.
pub(crate) fn extract(context: &DocumentContext<'_>) -> Option<SpecializedResult> {
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

pub(super) fn has_class(dom: &Dom, node: NodeId, expected: &str) -> bool {
    dom.attr(node, crate::dom::AttrName::Class)
        .is_some_and(|classes| {
            if classes.is_ascii() {
                classes
                    .split_ascii_whitespace()
                    .any(|class| class == expected)
            } else {
                classes.split_whitespace().any(|class| class == expected)
            }
        })
}

pub(super) fn class_contains(dom: &Dom, node: NodeId, needle: &str) -> bool {
    dom.attr(node, crate::dom::AttrName::Class)
        .is_some_and(|classes| {
            if classes.is_ascii() {
                classes
                    .split_ascii_whitespace()
                    .any(|class| class.contains(needle))
            } else {
                classes
                    .split_whitespace()
                    .any(|class| class.contains(needle))
            }
        })
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
}
