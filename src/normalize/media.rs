use crate::dom::{AttrName, Dom, NodeId, Tag};

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
        let (label, source) = match dom.tag(node) {
            Some(Tag::Iframe) => {
                let source = dom
                    .attr(node, AttrName::Src)
                    .and_then(safe_uri)
                    .map(str::to_owned);
                let youtube = source.as_deref().is_some_and(is_youtube);
                (
                    if youtube {
                        "YouTube video"
                    } else {
                        "Embedded content"
                    },
                    source.map(|source| {
                        if youtube {
                            canonical_youtube(&source)
                        } else {
                            source
                        }
                    }),
                )
            }
            Some(Tag::Video) => ("Video", media_source(dom, node)),
            Some(Tag::Audio) => ("Audio", media_source(dom, node)),
            Some(Tag::Object | Tag::Embed) => {
                dom.detach(node);
                continue;
            }
            _ => continue,
        };
        let Some(source) = source else {
            dom.detach(node);
            continue;
        };
        let Ok(link) = dom.create_html_element(Tag::A) else {
            continue;
        };
        let Ok(text) = dom.create_text(label) else {
            continue;
        };
        dom.set_attr(link, AttrName::Href, &source);
        dom.append_child(link, text);
        dom.replace_with(node, link);
    }
}

fn is_statically_hidden(dom: &Dom, node: NodeId) -> bool {
    dom.has_attr(node, AttrName::Hidden)
        || dom.attr(node, AttrName::AriaHidden) == Some("true")
        || dom.attr(node, AttrName::Style).is_some_and(|style| {
            let compact: String = style
                .bytes()
                .filter(|byte| !byte.is_ascii_whitespace())
                .map(char::from)
                .collect();
            let compact = compact.to_ascii_lowercase();
            compact.contains("display:none") || compact.contains("visibility:hidden")
        })
}

fn media_source(dom: &Dom, node: NodeId) -> Option<String> {
    dom.attr(node, AttrName::Src)
        .and_then(safe_uri)
        .map(str::to_owned)
        .or_else(|| {
            dom.descendants(node)
                .filter(|&child| dom.tag(child) == Some(Tag::Source))
                .find_map(|source| {
                    dom.attr(source, AttrName::Src)
                        .and_then(safe_uri)
                        .map(str::to_owned)
                })
        })
}

fn safe_uri(value: &str) -> Option<&str> {
    let value = value.trim_matches(|ch: char| ch.is_ascii_whitespace() || ch.is_control());
    if value.is_empty() || value.chars().any(char::is_control) {
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
            "[YouTube video](https://www.youtube.com/watch?v=abc)[Video](movie.mp4)[Audio](sound.mp3)[Embedded content](/frame)\n"
        );
        let html = crate::dom::render_html(&dom, root, 0);
        assert!(!html.contains("srcdoc"));
        assert!(!html.contains("onload"));
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
            "[Embedded content](https://notyoutube.com/embed/id)[Embedded content](https://example.test/youtube.com/embed/id)\n"
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
