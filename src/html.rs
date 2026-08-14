//! Legacy sanitized HTML output used by differential tests.

#![allow(dead_code)]

use crate::dom::{Dom, NodeId, render_html};

pub(crate) fn render_safe_html(dom: &Dom, root: NodeId, capacity: usize) -> String {
    let Ok(mut safe) = dom.copy_children_as_fragment(root) else {
        return String::new();
    };
    let root = safe.root();
    let nodes = safe.element_descendants_snapshot_with_depth(root);
    let mut removed_depth = None;
    for (node, depth) in nodes {
        if removed_depth.is_some_and(|value| depth > value) {
            continue;
        }
        removed_depth = None;
        let local = safe
            .qual_name(node)
            .map(|name| name.local.to_string())
            .unwrap_or_default();
        if is_blocked_element(&local) {
            safe.detach(node);
            removed_depth = Some(depth);
            continue;
        }
        safe.retain_attrs(node, |attribute| {
            let name = attribute.name.local.as_ref();
            !name.to_ascii_lowercase().starts_with("on")
                && !matches!(name.to_ascii_lowercase().as_str(), "srcdoc" | "style")
                && uri_attribute_is_safe(&local, name, attribute.value.as_ref())
        });
    }
    render_html(&safe, root, capacity)
}

fn is_blocked_element(local: &str) -> bool {
    matches!(
        local.to_ascii_lowercase().as_str(),
        "script"
            | "style"
            | "noscript"
            | "object"
            | "embed"
            | "applet"
            | "base"
            | "frame"
            | "frameset"
            | "iframe"
            | "animate"
            | "animatemotion"
            | "animatetransform"
            | "discard"
            | "foreignobject"
            | "set"
    )
}

fn uri_attribute_is_safe(element: &str, attribute: &str, value: &str) -> bool {
    let attribute = attribute.to_ascii_lowercase();
    let policy = match attribute.as_str() {
        "href" => UriPolicy::Link,
        "src" | "poster" | "background" => UriPolicy::Resource,
        "action" | "formaction" => UriPolicy::Form,
        "srcset" => return srcset_is_safe(value),
        _ => return true,
    };
    if element.eq_ignore_ascii_case("form") && attribute == "action" {
        return false;
    }
    url_is_safe(value, policy)
}

#[derive(Clone, Copy)]
enum UriPolicy {
    Link,
    Resource,
    Form,
}

fn url_is_safe(value: &str, policy: UriPolicy) -> bool {
    let compact: String = value
        .trim_start()
        .chars()
        .take_while(|character| *character != ':' && *character != '/' && *character != '#')
        .filter(|character| !character.is_ascii_whitespace() && !character.is_control())
        .flat_map(char::to_lowercase)
        .collect();
    let has_scheme = value.trim_start().find(':').is_some_and(|colon| {
        let boundary = value.trim_start().find(['/', '#']).unwrap_or(usize::MAX);
        colon < boundary
    });
    if !has_scheme {
        return true;
    }
    matches!(
        (compact.as_str(), policy),
        ("http" | "https", _) | ("mailto" | "tel", UriPolicy::Link)
    )
}

fn srcset_is_safe(value: &str) -> bool {
    value.split(',').all(|candidate| {
        candidate
            .split_ascii_whitespace()
            .next()
            .is_some_and(|url| url_is_safe(url, UriPolicy::Resource))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_obfuscated_schemes() {
        assert!(!url_is_safe(" JaVa\nScRiPt:alert(1)", UriPolicy::Link));
        assert!(!url_is_safe("data:text/html,x", UriPolicy::Resource));
        assert!(!url_is_safe("vbscript:msgbox(1)", UriPolicy::Link));
        assert!(url_is_safe("/safe/path", UriPolicy::Link));
        assert!(url_is_safe("#section", UriPolicy::Link));
        assert!(url_is_safe("mailto:a@example.com", UriPolicy::Link));
    }
}
