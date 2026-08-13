//! Static extractors for shared AI conversations.

use super::{
    DocumentContext, SpecializedExtractor, SpecializedResult, discussion::DiscussionBuilder,
    has_class,
};
use crate::dom::{Dom, NodeId, Tag};
use url::Url;

pub(super) struct AiConversationExtractor;

impl SpecializedExtractor for AiConversationExtractor {
    fn matches(&self, context: &DocumentContext<'_>) -> bool {
        let Some(url) = context.source_uri else {
            return false;
        };
        if !is_share_url(url) {
            return false;
        }

        let turns = turn_nodes(context.dom);
        !turns.is_empty()
            && find_title(context.dom).is_some()
            && turns.iter().any(|&turn| {
                find_body(context.dom, turn)
                    .is_some_and(|body| has_retained_content(context.dom, body))
            })
    }

    fn extract(&self, context: &DocumentContext<'_>) -> Option<SpecializedResult> {
        let source = context.dom;
        let title = find_title(source)?;
        let turns: Vec<_> = turn_nodes(source)
            .into_iter()
            .filter_map(|turn| {
                let body =
                    find_body(source, turn).filter(|&body| has_retained_content(source, body))?;
                Some((turn, body))
            })
            .collect();
        let (primary_turn, primary_body) = *turns.first()?;

        let mut builder = DiscussionBuilder::new()?;
        if !builder.set_title(source, title) {
            return None;
        }
        let (author, time) = turn_metadata(source, primary_turn);
        if !builder.append_primary_byline(author.as_deref(), time.as_deref())
            || !builder.append_primary_body_filtered(source, primary_body, |node| {
                !is_peripheral_container(source, node)
            })
        {
            return None;
        }

        if turns.len() > 1 {
            if !builder.set_reply_heading("Conversation") {
                return None;
            }
            for &(turn, body) in &turns[1..] {
                let (author, time) = turn_metadata(source, turn);
                builder.append_reply_filtered(
                    source,
                    0,
                    author.as_deref(),
                    time.as_deref(),
                    Some(body),
                    |node| !is_peripheral_container(source, node),
                )?;
            }
        }

        Some(builder.finish(identity(context.source_uri?)))
    }
}

fn is_share_url(url: &Url) -> bool {
    let Some(host) = url.host_str() else {
        return false;
    };
    let host = host.strip_prefix("www.").unwrap_or(host);
    let known_host = matches!(
        host,
        "chatgpt.com" | "chat.openai.com" | "claude.ai" | "gemini.google.com" | "grok.com"
    );
    let mut segments = url.path_segments().into_iter().flatten();
    known_host
        && segments
            .next()
            .is_some_and(|segment| segment.eq_ignore_ascii_case("share"))
        && segments.next().is_some_and(|segment| !segment.is_empty())
        && segments.next().is_none()
}

fn identity(url: &Url) -> &'static str {
    match url
        .host_str()
        .unwrap_or_default()
        .trim_start_matches("www.")
    {
        "chatgpt.com" | "chat.openai.com" => "chatgpt-share",
        "claude.ai" => "claude-share",
        "gemini.google.com" => "gemini-share",
        "grok.com" => "grok-share",
        _ => "ai-conversation-share",
    }
}

fn turn_nodes(dom: &Dom) -> Vec<NodeId> {
    let snapshot = dom
        .element_descendants_snapshot_with_depth(dom.root())
        .into_iter()
        .map(|(node, _)| node);
    snapshot
        .filter(|&node| is_turn(dom, node))
        .filter(|&node| !dom.ancestors(node).any(|ancestor| is_turn(dom, ancestor)))
        .collect()
}

fn is_turn(dom: &Dom, node: NodeId) -> bool {
    has_role_marker(dom, node)
        || (dom.attr_by_local_name(node, "data-testid") == Some("conversation-turn")
            && top_level_role_marker_count(dom, node) == 1)
}

fn top_level_role_marker_count(dom: &Dom, node: NodeId) -> usize {
    dom.descendants(node)
        .filter(|&candidate| has_role_marker(dom, candidate))
        .filter(|&candidate| {
            !dom.ancestors(candidate)
                .any(|ancestor| ancestor != node && has_role_marker(dom, ancestor))
        })
        .count()
}

fn has_role_marker(dom: &Dom, node: NodeId) -> bool {
    ["data-message-author-role", "data-role"]
        .iter()
        .any(|name| {
            dom.attr_by_local_name(node, name)
                .is_some_and(is_known_role)
        })
}

fn is_known_role(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "user" | "assistant" | "model" | "system" | "tool"
    )
}

fn find_title(dom: &Dom) -> Option<NodeId> {
    dom.descendants(dom.root())
        .find(|&node| dom.tag(node) == Some(Tag::H1) && has_content(dom, node))
        .or_else(|| {
            dom.descendants(dom.root())
                .find(|&node| dom.tag(node) == Some(Tag::Title) && has_content(dom, node))
        })
}

fn find_body(dom: &Dom, turn: NodeId) -> Option<NodeId> {
    dom.descendants(turn)
        .find(|&node| {
            is_body_marker(dom, node)
                && nearest_turn(dom, node) == Some(turn)
                && has_retained_content(dom, node)
        })
        .or_else(|| {
            let candidates: Vec<_> = dom
                .descendants(turn)
                .filter(|&node| {
                    dom.tag(node).is_some_and(is_content_container)
                        && has_retained_content(dom, node)
                        && nearest_turn(dom, node) == Some(turn)
                        && !is_peripheral_container(dom, node)
                        && !dom
                            .descendants(node)
                            .any(|child| is_peripheral_container(dom, child))
                })
                .collect();
            let roots: Vec<_> = candidates
                .iter()
                .copied()
                .filter(|&node| {
                    !candidates.iter().copied().any(|candidate| {
                        candidate != node
                            && dom.ancestors(node).any(|ancestor| ancestor == candidate)
                    })
                })
                .collect();
            if roots.len() > 1 {
                Some(turn)
            } else {
                roots.into_iter().max_by_key(|&node| body_score(dom, node))
            }
        })
}

fn is_body_marker(dom: &Dom, node: NodeId) -> bool {
    dom.attr_by_local_name(node, "data-message-content")
        .is_some()
        || matches!(
            dom.attr_by_local_name(node, "data-testid"),
            Some("message-content" | "conversation-content" | "response-content")
        )
        || [
            "markdown",
            "prose",
            "message-content",
            "text-message",
            "whitespace-pre-wrap",
        ]
        .iter()
        .any(|class| has_class(dom, node, class))
}

fn is_peripheral_container(dom: &Dom, node: NodeId) -> bool {
    matches!(
        dom.tag(node),
        Some(Tag::Header | Tag::Footer | Tag::Nav | Tag::Button)
    ) || [
        "header", "action", "toolbar", "footer", "nav", "menu", "control", "button",
    ]
    .iter()
    .any(|needle| super::class_contains(dom, node, needle))
}

fn body_score(dom: &Dom, node: NodeId) -> usize {
    normalized_text(dom, node).len()
        + dom
            .descendants(node)
            .filter(|&child| is_meaningful_media(dom, child))
            .count()
            .saturating_mul(64)
}

fn is_content_container(tag: Tag) -> bool {
    matches!(
        tag,
        Tag::Div
            | Tag::P
            | Tag::Article
            | Tag::Section
            | Tag::Blockquote
            | Tag::Pre
            | Tag::Ul
            | Tag::Ol
    )
}

fn nearest_turn(dom: &Dom, node: NodeId) -> Option<NodeId> {
    dom.ancestors(node)
        .filter(|&ancestor| is_turn(dom, ancestor))
        .last()
}

fn turn_metadata(dom: &Dom, turn: NodeId) -> (Option<String>, Option<String>) {
    let author = dom
        .attr_by_local_name(turn, "data-message-author-role")
        .or_else(|| dom.attr_by_local_name(turn, "data-role"))
        .or_else(|| {
            dom.descendants(turn).find_map(|node| {
                ["data-message-author-role", "data-role"]
                    .iter()
                    .find_map(|name| dom.attr_by_local_name(node, name))
                    .filter(|value| is_known_role(value))
            })
        })
        .map(display_role)
        .filter(|value| !value.is_empty());
    let time = dom
        .descendants(turn)
        .find(|&node| dom.tag(node) == Some(Tag::Time) && nearest_turn(dom, node) == Some(turn))
        .map(|node| normalized_text(dom, node))
        .filter(|value| !value.is_empty());
    (author, time)
}

fn display_role(role: &str) -> String {
    let role = role.trim().to_ascii_lowercase();
    match role.as_str() {
        "user" => "User".to_owned(),
        "assistant" | "model" => "Assistant".to_owned(),
        "system" => "System".to_owned(),
        "tool" => "Tool".to_owned(),
        _ => String::new(),
    }
}

fn has_content(dom: &Dom, node: NodeId) -> bool {
    !normalized_text(dom, node).is_empty()
        || dom
            .descendants(node)
            .any(|child| is_meaningful_media(dom, child))
}

fn has_retained_content(dom: &Dom, node: NodeId) -> bool {
    let mut pending = vec![node];
    while let Some(current) = pending.pop() {
        if is_peripheral_container(dom, current) {
            continue;
        }
        if is_meaningful_media(dom, current)
            || (dom.tag(current).is_none() && !dom.text(current).trim().is_empty())
        {
            return true;
        }
        pending.extend(dom.children(current));
    }
    false
}

fn is_meaningful_media(dom: &Dom, node: NodeId) -> bool {
    match dom.tag(node) {
        Some(Tag::Img) => ["alt", "src", "data-src", "srcset"].iter().any(|name| {
            dom.attr_by_local_name(node, name)
                .is_some_and(|value| !value.trim().is_empty())
        }),
        Some(Tag::Svg) => {
            dom.attr_by_local_name(node, "aria-label").is_some()
                || dom.attr_by_local_name(node, "role") == Some("img")
        }
        Some(Tag::Audio | Tag::Video | Tag::Iframe) => ["src", "poster", "title"]
            .iter()
            .any(|name| dom.attr_by_local_name(node, name).is_some()),
        _ => false,
    }
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
    fn recognizes_static_shared_conversation() {
        let dom = Dom::parse_document(
            "<h1>Shared chat</h1><article data-message-author-role='user'><div data-testid='message-content'><p>Hello</p></div></article>",
        )
        .unwrap();
        assert!(
            AiConversationExtractor
                .matches(&context(&dom, "https://chatgpt.com/share/conversation-1",))
        );
    }

    #[test]
    fn rejects_application_shell_without_turn_content() {
        let dom = Dom::parse_document(
            "<h1>Shared chat</h1><div data-testid='conversation-turn' data-message-author-role='assistant'><div data-testid='message-content'></div></div>",
        )
        .unwrap();
        assert!(
            !AiConversationExtractor
                .matches(&context(&dom, "https://chatgpt.com/share/conversation-1",))
        );
    }

    #[test]
    fn requires_a_known_share_url() {
        let dom = Dom::parse_document(
            "<h1>Shared chat</h1><article data-message-author-role='user'><div data-testid='message-content'>Hello</div></article>",
        )
        .unwrap();
        assert!(
            !AiConversationExtractor
                .matches(&context(&dom, "https://example.test/share/conversation-1",))
        );
    }

    #[test]
    fn requires_the_share_route_at_the_path_root() {
        let dom = Dom::parse_document(
            "<h1>Shared chat</h1><article data-message-author-role='user'><div data-testid='message-content'>Hello</div></article>",
        )
        .unwrap();
        assert!(!AiConversationExtractor.matches(&context(
            &dom,
            "https://chatgpt.com/not-share/conversation-1",
        )));
    }

    #[test]
    fn preserves_a_media_only_turn_body() {
        let dom = Dom::parse_document(
            "<h1>Shared chat</h1><article data-message-author-role='user'><div class='message-wrapper'><div class='message-header'>User</div><div class='content'><img src='diagram.png' alt='Parser flow diagram'></div></div></article>",
        )
        .unwrap();
        let turn = turn_nodes(&dom)[0];
        let body = find_body(&dom, turn).unwrap();
        assert!(has_content(&dom, body));
        assert_eq!(dom.tag(body), Some(Tag::Div));
    }

    #[test]
    fn handles_turn_and_role_markers_on_nested_elements() {
        let dom = Dom::parse_document(
            "<h1>Shared chat</h1><div data-testid='conversation-turn'><div data-message-author-role='user'><div data-testid='message-content'>Hello</div></div></div>",
        )
        .unwrap();
        assert!(
            AiConversationExtractor
                .matches(&context(&dom, "https://chatgpt.com/share/conversation-1",))
        );
    }

    #[test]
    fn ignores_toolbar_text_when_finding_a_fallback_body() {
        let dom = Dom::parse_document(
            "<h1>Shared chat</h1><article data-message-author-role='user'><div class='toolbar'>Copy Regenerate</div></article>",
        )
        .unwrap();
        assert!(
            !AiConversationExtractor
                .matches(&context(&dom, "https://chatgpt.com/share/conversation-1",))
        );
    }

    #[test]
    fn keeps_multiple_role_marked_turns_inside_one_wrapper() {
        let dom = Dom::parse_document(
            "<h1>Shared chat</h1><div data-testid='conversation-turn'><div data-message-author-role='user'><div class='prose'>First</div></div><div data-message-author-role='assistant'><div class='prose'>Second</div></div></div>",
        )
        .unwrap();
        assert_eq!(turn_nodes(&dom).len(), 2);
        assert!(
            AiConversationExtractor
                .matches(&context(&dom, "https://chatgpt.com/share/conversation-1",))
        );
    }

    #[test]
    fn filters_toolbar_without_dropping_sibling_body_blocks() {
        let dom = Dom::parse_document(
            "<h1>Shared chat</h1><article data-message-author-role='user'><div class='toolbar'>Copy Regenerate</div><p>First paragraph.</p><pre><code>let answer = 42;</code></pre><ul><li>Keep this list.</li></ul></article>",
        )
        .unwrap();
        let result = AiConversationExtractor
            .extract(&context(&dom, "https://chatgpt.com/share/conversation-1"))
            .unwrap();
        let html = result.dom.html(result.root).unwrap();
        assert!(html.contains("First paragraph."));
        assert!(html.contains("Keep this list."));
        assert!(!html.contains("Copy Regenerate"));
    }

    #[test]
    fn rejects_a_marked_toolbar_without_retained_message_content() {
        let dom = Dom::parse_document(
            "<h1>Shared chat</h1><article data-message-author-role='assistant'><div data-testid='message-content'><div class='toolbar'>Copy Regenerate</div></div></article>",
        )
        .unwrap();
        assert!(
            !AiConversationExtractor
                .matches(&context(&dom, "https://chatgpt.com/share/conversation-1",))
        );
    }

    #[test]
    fn rejects_a_bare_conversation_turn_marker() {
        let dom = Dom::parse_document(
            "<h1>Unrelated page</h1><div data-testid='conversation-turn'><p>Ordinary page content.</p></div>",
        )
        .unwrap();
        assert!(
            !AiConversationExtractor
                .matches(&context(&dom, "https://chatgpt.com/share/conversation-1",))
        );
    }
}
