use crate::dom::{AttrName, Dom, NodeId, Tag};

pub(super) fn normalize(dom: &mut Dom, root: NodeId) {
    let nodes = dom.element_descendants_snapshot_with_depth(root);
    for (node, _) in nodes {
        if dom.parent(node).is_none() || dom.tag(node) == Some(Tag::Blockquote) {
            continue;
        }
        if !matches!(dom.tag(node), Some(Tag::Aside | Tag::Div | Tag::Section)) {
            continue;
        }
        let Some(kind) = callout_kind(dom, node) else {
            continue;
        };
        dom.rename_html(node, Tag::Blockquote);
        dom.set_attr(node, AttrName::DataCallout, kind);
        emphasize_label(dom, node, kind);
    }
}

fn callout_kind(dom: &Dom, node: NodeId) -> Option<&'static str> {
    let mut structural = false;
    let mut kind = None;
    if dom.attr(node, AttrName::Role).is_some_and(|roles| {
        roles
            .split_whitespace()
            .any(|role| role.eq_ignore_ascii_case("note"))
    }) {
        structural = true;
        kind = Some("note");
    }
    for token in [AttrName::Class, AttrName::Id]
        .into_iter()
        .filter_map(|name| dom.attr(node, name))
        .flat_map(str::split_whitespace)
    {
        let token = token
            .trim_matches(|ch: char| !ch.is_ascii_alphanumeric())
            .to_ascii_lowercase();
        structural |= matches!(token.as_str(), "admonition" | "callout" | "alert");
        if let Some(value) = canonical_kind(&token) {
            kind = Some(value);
        }
    }
    let kind = kind?;
    let label = first_label(dom, node);
    (structural
        || label
            .as_deref()
            .is_some_and(|label| canonical_kind(label) == Some(kind)))
    .then_some(kind)
}

fn canonical_kind(value: &str) -> Option<&'static str> {
    match value
        .trim()
        .trim_end_matches(':')
        .to_ascii_lowercase()
        .as_str()
    {
        "note" => Some("note"),
        "warning" => Some("warning"),
        "tip" => Some("tip"),
        "important" => Some("important"),
        "caution" => Some("caution"),
        "info" | "information" => Some("info"),
        _ => None,
    }
}

fn first_label(dom: &Dom, node: NodeId) -> Option<String> {
    let child = dom.element_children(node).next()?;
    let explicitly_named = [AttrName::Class, AttrName::Id]
        .into_iter()
        .filter_map(|name| dom.attr(child, name))
        .flat_map(str::split_whitespace)
        .any(|token| {
            matches!(
                token.to_ascii_lowercase().as_str(),
                "title" | "admonition-title" | "callout-title" | "label"
            )
        });
    let text = dom.text(child);
    let text = text.trim().trim_end_matches(':');
    (explicitly_named || text.len() <= 16).then(|| text.to_ascii_lowercase())
}

fn emphasize_label(dom: &mut Dom, node: NodeId, kind: &str) {
    let Some(child) = dom.element_children(node).next() else {
        return;
    };
    let text = dom.text(child);
    if canonical_kind(text.trim().trim_end_matches(':')) == Some(kind)
        && !dom
            .element_children(child)
            .any(|element| matches!(dom.tag(element), Some(Tag::Strong | Tag::B)))
    {
        let Ok(strong) = dom.create_html_element(Tag::Strong) else {
            return;
        };
        dom.move_children(child, strong);
        dom.append_child(child, strong);
        return;
    }
    let Ok(label) = dom.create_html_element(Tag::P) else {
        return;
    };
    let Ok(strong) = dom.create_html_element(Tag::Strong) else {
        return;
    };
    let mut title = kind.to_owned();
    if let Some(first) = title.get_mut(0..1) {
        first.make_ascii_uppercase();
    }
    let Ok(text) = dom.create_text(&title) else {
        return;
    };
    dom.append_child(strong, text);
    dom.append_child(label, strong);
    dom.insert_before(child, label);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown::dom_to_markdown;

    #[test]
    fn converts_an_admonition_but_not_a_card() {
        let mut dom = Dom::parse_fragment(r#"<div class="admonition warning"><p class="admonition-title">Warning</p><p>Take care.</p></div><div class="card warning"><p>Release notes</p></div>"#, Tag::Div).unwrap();
        let root = dom.root();
        normalize(&mut dom, root);
        assert_eq!(
            dom_to_markdown(&dom, root, 0),
            "> **Warning**\n>\n> Take care.\n\nRelease notes\n"
        );
    }
}
