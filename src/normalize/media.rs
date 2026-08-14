use crate::dom::{AttrName, Dom, NodeId, Tag};
use crate::scoring::{has_hidden_utility_class, has_static_hidden_marker};

pub(super) fn normalize(dom: &mut Dom, root: NodeId) {
    let nodes = dom.element_descendants_snapshot_with_depth(root);
    for (node, _) in nodes.into_iter().rev() {
        if dom.parent(node).is_none() {
            continue;
        }
        if !matches!(
            dom.tag(node),
            Some(Tag::Iframe | Tag::Video | Tag::Audio | Tag::Object | Tag::Embed)
        ) {
            continue;
        }
        if is_statically_hidden(dom, node) {
            dom.detach(node);
            continue;
        }
        #[cfg(test)]
        let semantic_kind = match dom.tag(node) {
            Some(Tag::Iframe) => "embedded",
            Some(Tag::Video) => "video",
            Some(Tag::Audio) => "audio",
            _ => "",
        };
        let (label, source) = match dom.tag(node) {
            Some(Tag::Iframe) => {
                let source = media_source(dom, node);
                let youtube = source.as_deref().is_some_and(is_youtube);
                (
                    media_label(
                        dom,
                        node,
                        if youtube {
                            "YouTube video"
                        } else {
                            "Embedded content"
                        },
                    ),
                    source.map(|source| {
                        if youtube {
                            canonical_youtube(&source)
                        } else {
                            source
                        }
                    }),
                )
            }
            Some(Tag::Video) => (media_label(dom, node, "Video"), media_source(dom, node)),
            Some(Tag::Audio) => (media_label(dom, node, "Audio"), media_source(dom, node)),
            Some(Tag::Object | Tag::Embed) => {
                dom.detach(node);
                continue;
            }
            _ => continue,
        };
        let Some(source) = source else {
            if let Some(fallback) = media_fallback_link(dom, node) {
                dom.insert_before(node, fallback);
            }
            dom.detach(node);
            continue;
        };
        let Ok(link) = dom.create_html_element(Tag::A) else {
            continue;
        };
        let Ok(text) = dom.create_text(&label) else {
            continue;
        };
        dom.set_attr(link, AttrName::Href, &source);
        // Stage A compiles the normalized DOM only in tests. Keep semantic
        // media identity out of production HTML until the IR replaces it.
        #[cfg(test)]
        dom.set_attr(link, AttrName::DataLegibleKind, semantic_kind);
        dom.append_child(link, text);
        if let Some(fallback) = media_fallback_link(dom, node) {
            let Ok(container) = dom.create_html_element(Tag::Span) else {
                continue;
            };
            dom.append_child(container, link);
            if let Ok(space) = dom.create_text(" ") {
                dom.append_child(container, space);
            }
            dom.append_child(container, fallback);
            dom.replace_with(node, container);
        } else {
            dom.replace_with(node, link);
        }
    }
}

fn is_statically_hidden(dom: &Dom, node: NodeId) -> bool {
    has_static_hidden_marker(dom, node)
        || has_hidden_utility_class(dom, node)
        || dom.attr(node, AttrName::AriaHidden) == Some("true")
}

fn media_source(dom: &Dom, node: NodeId) -> Option<String> {
    let direct_source = |node| {
        [AttrName::Src, AttrName::DataSrc]
            .into_iter()
            .filter_map(|attribute| dom.attr(node, attribute))
            .find_map(|value| safe_uri(value).map(str::to_owned))
    };
    direct_source(node).or_else(|| {
        dom.descendants(node)
            .filter(|&child| dom.tag(child) == Some(Tag::Source))
            .find_map(direct_source)
    })
}

fn media_label(dom: &Dom, node: NodeId, fallback: &str) -> String {
    [AttrName::AriaLabel, AttrName::Title]
        .into_iter()
        .filter_map(|attribute| dom.attr(node, attribute))
        .find_map(normalize_media_label)
        .unwrap_or_else(|| fallback.to_owned())
}

fn media_fallback_link(dom: &Dom, node: NodeId) -> Option<NodeId> {
    dom.descendants(node)
        .filter(|&child| dom.tag(child) == Some(Tag::A))
        .find(|&link| {
            dom.attr(link, AttrName::Href)
                .and_then(safe_uri)
                .is_some_and(|_| dom.has_non_whitespace_text(link))
        })
}

fn normalize_media_label(value: &str) -> Option<String> {
    let value = value.split_whitespace().collect::<Vec<_>>().join(" ");
    let value = value.chars().take(200).collect::<String>();
    (!value.is_empty()).then_some(value)
}

fn safe_uri(value: &str) -> Option<&str> {
    let value = value.trim_matches(|ch: char| ch.is_ascii_whitespace() || ch.is_control());
    if value.is_empty()
        || value.eq_ignore_ascii_case("null")
        || value.eq_ignore_ascii_case("undefined")
        || value.chars().any(char::is_control)
    {
        return None;
    }
    let colon = value
        .bytes()
        .position(|byte| matches!(byte, b':' | b'/' | b'?' | b'#'));
    if colon.is_some_and(|index| value.as_bytes()[index] == b':') {
        let scheme = &value[..colon.unwrap()];
        if !matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https") {
            return None;
        }
    }
    Some(value)
}

fn canonical_youtube(value: &str) -> String {
    let Some((_prefix, id)) = value.split_once("/embed/") else {
        return value.to_owned();
    };
    if !is_youtube(value) {
        return value.to_owned();
    }
    let id = id.split(['?', '#']).next().unwrap_or(id);
    format!("https://www.youtube.com/watch?v={id}")
}

fn is_youtube(value: &str) -> bool {
    let protocol_relative;
    let value = if value.starts_with("//") {
        protocol_relative = format!("https:{value}");
        &protocol_relative
    } else {
        value
    };
    let Ok(url) = url::Url::parse(value) else {
        return false;
    };
    url.host_str().is_some_and(|host| {
        let host = host.to_ascii_lowercase();
        host == "youtu.be"
            || host == "youtube.com"
            || host.ends_with(".youtube.com")
            || host == "youtube-nocookie.com"
            || host.ends_with(".youtube-nocookie.com")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown::dom_to_markdown;

    #[test]
    fn converts_iframes_and_native_media_to_links() {
        let mut dom = Dom::parse_fragment(r#"<iframe src="https://www.youtube-nocookie.com/embed/abc?rel=0" srcdoc="bad" onload="bad()"></iframe><video poster="poster.jpg"><source src="movie.mp4"></video><audio src="sound.mp3"></audio><iframe src="/frame"></iframe>"#, Tag::Div).unwrap();
        let root = dom.root();
        normalize(&mut dom, root);
        assert_eq!(
            dom_to_markdown(&dom, root, 0),
            "[YouTube video](https://www.youtube.com/watch?v=abc) [Video](movie.mp4) [Audio](sound.mp3) [Embedded content](/frame)\n"
        );
        let html = crate::dom::render_html(&dom, root, 0);
        assert!(!html.contains("srcdoc"));
        assert!(!html.contains("onload"));
    }

    #[test]
    fn uses_media_titles_without_copying_player_fallback_text() {
        let mut dom = Dom::parse_fragment(
            r#"<figure><iframe src="https://video.example.test/embed/42" title="Interview with the bridge engineer">Opaque player chrome</iframe><figcaption><a href="/transcript">Read the complete interview transcript</a></figcaption></figure>"#,
            Tag::Div,
        )
        .unwrap();
        let root = dom.root();
        normalize(&mut dom, root);
        let markdown = dom_to_markdown(&dom, root, 0);
        assert!(
            markdown.contains(
                "[Interview with the bridge engineer](https://video.example.test/embed/42)"
            )
        );
        assert!(markdown.contains("[Read the complete interview transcript](/transcript)"));
        assert!(!markdown.contains("Opaque player chrome"));
    }

    #[test]
    fn removes_unsafe_or_empty_media() {
        let mut dom = Dom::parse_fragment(r#"<iframe src="javascript:bad()"></iframe><video src="data:text/html,bad"></video><audio></audio><object data="movie.swf"></object>"#, Tag::Div).unwrap();
        let root = dom.root();
        normalize(&mut dom, root);
        assert_eq!(dom_to_markdown(&dom, root, 0), "");
        assert!(dom.first_descendant_by_tag(root, Tag::Object).is_none());
    }

    #[test]
    fn deceptive_hosts_are_generic_embeds() {
        let mut dom = Dom::parse_fragment(
            r#"<iframe src="https://notyoutube.com/embed/id"></iframe><iframe src="https://example.test/youtube.com/embed/id"></iframe>"#,
            Tag::Div,
        )
        .unwrap();
        let root = dom.root();
        normalize(&mut dom, root);
        assert_eq!(
            dom_to_markdown(&dom, root, 0),
            "[Embedded content](https://notyoutube.com/embed/id) [Embedded content](https://example.test/youtube.com/embed/id)\n"
        );
    }

    #[test]
    fn uses_the_first_safe_native_source() {
        let mut dom = Dom::parse_fragment(
            r#"<video><source src="javascript:bad"><source src="safe.mp4"></video>"#,
            Tag::Div,
        )
        .unwrap();
        let root = dom.root();
        normalize(&mut dom, root);
        assert_eq!(dom_to_markdown(&dom, root, 0), "[Video](safe.mp4)\n");
    }

    #[test]
    fn recovers_safe_data_sources_after_unsafe_sources() {
        let mut dom = Dom::parse_fragment(
            r#"<iframe src="javascript:bad" data-src="/frame"></iframe><video src="javascript:bad"><source src="javascript:bad" data-src="movie.mp4"></video>"#,
            Tag::Div,
        )
        .unwrap();
        let root = dom.root();
        normalize(&mut dom, root);
        assert_eq!(
            dom_to_markdown(&dom, root, 0),
            "[Embedded content](/frame) [Video](movie.mp4)\n"
        );
    }

    #[test]
    fn skips_sentinel_sources_when_recovering_media() {
        let mut dom = Dom::parse_fragment(
            r#"<iframe src="null" data-src="/frame"></iframe><audio src="undefined" data-src="sound.mp3"></audio>"#,
            Tag::Div,
        )
        .unwrap();
        let root = dom.root();
        normalize(&mut dom, root);
        assert_eq!(
            dom_to_markdown(&dom, root, 0),
            "[Embedded content](/frame) [Audio](sound.mp3)\n"
        );
    }

    #[test]
    fn prefers_an_accessible_media_label_over_a_generic_title() {
        let mut dom = Dom::parse_fragment(
            r#"<iframe src="/frame" title="Video player" aria-label="Bridge engineer interview"></iframe>"#,
            Tag::Div,
        )
        .unwrap();
        let root = dom.root();
        normalize(&mut dom, root);
        assert_eq!(
            dom_to_markdown(&dom, root, 0),
            "[Bridge engineer interview](/frame)\n"
        );
    }

    #[test]
    fn preserves_a_safe_fallback_link_inside_native_media() {
        let mut dom = Dom::parse_fragment(
            r#"<video src="movie.mp4"><a href="/transcript">Read the transcript</a></video>"#,
            Tag::Div,
        )
        .unwrap();
        let root = dom.root();
        normalize(&mut dom, root);
        assert_eq!(
            dom_to_markdown(&dom, root, 0),
            "[Video](movie.mp4) [Read the transcript](/transcript)\n"
        );
    }

    #[test]
    fn removes_hidden_media_instead_of_revealing_links() {
        let mut dom = Dom::parse_fragment(
            r#"<iframe hidden src="https://example.test/frame"></iframe><video aria-hidden="true" src="movie.mp4"></video><audio style="display: none" src="sound.mp3"></audio>"#,
            Tag::Div,
        )
        .unwrap();
        let root = dom.root();
        normalize(&mut dom, root);
        assert_eq!(dom_to_markdown(&dom, root, 0), "");
    }
}
