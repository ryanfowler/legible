use crate::dom::{AttrName, Dom, NodeId, Tag};
use smallvec::SmallVec;
use std::collections::HashSet;

pub(super) fn normalize(dom: &mut Dom, root: NodeId) {
    let definitions: HashSet<String> = dom
        .descendants(root)
        .filter(|&node| is_definition(dom, node))
        .filter_map(|node| dom.attr(node, AttrName::Id).map(str::to_owned))
        .collect();

    let anchors: SmallVec<[NodeId; 16]> = dom
        .descendants(root)
        .filter(|&node| dom.tag(node) == Some(Tag::A))
        .collect();
    for anchor in anchors {
        let Some(target) = fragment_target(dom.attr(anchor, AttrName::Href)).map(str::to_owned)
        else {
            continue;
        };
        let explicit = has_role(dom, anchor, "doc-noteref")
            || dom
                .attr(anchor, AttrName::Rel)
                .is_some_and(|rel| token(rel, "footnote"));
        let parent = dom.parent(anchor);
        let conventional = definitions.contains(target.as_str());
        if !explicit && !conventional {
            continue;
        }
        let reference = parent
            .filter(|&parent| dom.tag(parent) == Some(Tag::Sup))
            .unwrap_or(anchor);
        dom.set_attr(reference, AttrName::DataFootnoteRef, &target);
    }

    let nodes = dom.element_descendants_snapshot_with_depth(root);
    for (node, _) in nodes {
        if dom.parent(node).is_none() {
            continue;
        }
        if is_definition(dom, node) {
            if let Some(id) = dom.attr(node, AttrName::Id).map(str::to_owned) {
                dom.set_attr(node, AttrName::DataFootnote, &id);
            }
            remove_backlinks(dom, node);
        }
        if is_footnote_container(dom, node) {
            dom.set_attr(node, AttrName::DataFootnotes, "");
            if matches!(dom.tag(node), Some(Tag::Div | Tag::Aside)) {
                dom.rename_html(node, Tag::Section);
            }
            if let Some(heading) = dom.element_children(node).find(|&child| {
                matches!(
                    dom.tag(child),
                    Some(Tag::H1 | Tag::H2 | Tag::H3 | Tag::H4 | Tag::H5 | Tag::H6)
                )
            }) {
                dom.detach(heading);
            }
        }
    }
}

pub(crate) struct Definitions(Vec<(String, Dom)>);

pub(crate) fn collect_external(dom: &Dom) -> Definitions {
    Definitions(
        dom.descendants(dom.root())
            .filter(|&node| is_definition(dom, node))
            .filter(|&node| {
                !dom.ancestors(node)
                    .any(|ancestor| is_definition(dom, ancestor))
            })
            .filter_map(|node| {
                let id = dom.attr(node, AttrName::Id)?.to_owned();
                dom.copy_subtree_as_fragment(node)
                    .ok()
                    .map(|copy| (id, copy))
            })
            .collect(),
    )
}

pub(crate) fn adopt_external(definitions: &Definitions, fragment: &mut Dom, fragment_root: NodeId) {
    let referenced: Vec<String> = fragment
        .descendants(fragment_root)
        .filter(|&node| fragment.tag(node) == Some(Tag::A))
        .filter_map(|node| fragment_target(fragment.attr(node, AttrName::Href)).map(str::to_owned))
        .filter(|id| {
            looks_like_footnote_id(id) || definitions.0.iter().any(|(defined, _)| defined == id)
        })
        .scan(HashSet::new(), |seen, id| {
            seen.insert(id.clone()).then_some(id)
        })
        .collect();
    if referenced.is_empty() {
        return;
    }
    let present: HashSet<&str> = fragment
        .descendants(fragment_root)
        .filter_map(|node| fragment.attr(node, AttrName::Id))
        .collect();
    let missing: Vec<&Dom> = referenced
        .into_iter()
        .filter(|id| !present.contains(id.as_str()))
        .filter_map(|id| {
            definitions
                .0
                .iter()
                .find(|(defined, _)| defined == &id)
                .map(|(_, definition)| definition)
        })
        .collect();
    if missing.is_empty() {
        return;
    }
    let Ok(section) = fragment.create_html_element(Tag::Section) else {
        return;
    };
    fragment.set_attr(section, AttrName::DataFootnotes, "");
    for definition in missing {
        let Some(definition_root) = definition.first_child(definition.root()) else {
            continue;
        };
        if let Ok(copy) = fragment.import_subtree(definition, definition_root) {
            if fragment.tag(copy) == Some(Tag::Li) {
                fragment.rename_html(copy, Tag::Div);
            }
            fragment.append_child(section, copy);
        }
    }
    fragment.append_child(fragment_root, section);
}

fn is_definition(dom: &Dom, node: NodeId) -> bool {
    has_role(dom, node, "doc-footnote")
        || dom.attr(node, AttrName::Class).is_some_and(|classes| {
            classes.split_whitespace().any(|class| {
                matches!(
                    class.to_ascii_lowercase().as_str(),
                    "sidenote" | "side-note" | "marginnote" | "margin-note"
                )
            })
        })
        || dom.attr(node, AttrName::Id).is_some_and(|id| {
            looks_like_footnote_id(id)
                && dom
                    .ancestors(node)
                    .any(|ancestor| is_footnote_container(dom, ancestor))
        })
}

fn is_footnote_container(dom: &Dom, node: NodeId) -> bool {
    has_role(dom, node, "doc-endnotes")
        || [AttrName::Class, AttrName::Id]
            .into_iter()
            .filter_map(|name| dom.attr(node, name))
            .any(|value| {
                value.split_whitespace().any(|part| {
                    matches!(
                        part.to_ascii_lowercase().as_str(),
                        "footnotes" | "footnote-list" | "endnotes"
                    ) || part.eq_ignore_ascii_case("references")
                        && has_footnote_definitions(dom, node)
                })
            })
}

fn has_footnote_definitions(dom: &Dom, node: NodeId) -> bool {
    dom.descendants(node).any(|descendant| {
        has_role(dom, descendant, "doc-footnote")
            || dom
                .attr(descendant, AttrName::Id)
                .is_some_and(looks_like_footnote_id)
    })
}

fn remove_backlinks(dom: &mut Dom, definition: NodeId) {
    let links: SmallVec<[NodeId; 4]> = dom
        .descendants(definition)
        .filter(|&node| dom.tag(node) == Some(Tag::A))
        .collect();
    for link in links {
        let rel = dom
            .attr(link, AttrName::Rel)
            .is_some_and(|value| token(value, "backlink"));
        let text = dom.text(link);
        let label = text.trim().to_ascii_lowercase();
        let conventional = dom
            .attr(link, AttrName::Href)
            .is_some_and(|href| href.starts_with('#'))
            && (matches!(label.as_str(), "back" | "back to content" | "return")
                || label.chars().all(|ch| matches!(ch, '↩' | '↵' | '↑' | ' ')));
        if rel || conventional {
            dom.detach(link);
        }
    }
}

fn fragment_target(href: Option<&str>) -> Option<&str> {
    href?.strip_prefix('#').filter(|value| !value.is_empty())
}

fn looks_like_footnote_id(value: &str) -> bool {
    let value = value.to_ascii_lowercase();
    value.starts_with("fn")
        || value.starts_with("footnote")
        || value.starts_with("note-")
        || value.starts_with("sn")
        || value.starts_with("sidenote")
        || value.starts_with("cite_note")
}

fn has_role(dom: &Dom, node: NodeId, role: &str) -> bool {
    dom.attr(node, AttrName::Role)
        .is_some_and(|value| token(value, role))
}

fn token(value: &str, expected: &str) -> bool {
    value
        .split_whitespace()
        .any(|value| value.eq_ignore_ascii_case(expected))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown::dom_to_markdown;

    #[test]
    fn normalizes_repeated_references_links_and_backlinks() {
        let mut dom = Dom::parse_fragment(r##"<p>One<sup><a href="#fn1">1</a></sup> and again <a role="doc-noteref" href="#fn1">1</a>.</p><section class="footnotes"><h2>Notes</h2><ol><li id="fn1" role="doc-footnote"><p>See <a href="https://example.test">source</a>. <a href="#ref" rel="backlink">Back</a></p><p>More detail.</p></li></ol></section>"##, Tag::Div).unwrap();
        let root = dom.root();
        normalize(&mut dom, root);
        assert_eq!(
            dom_to_markdown(&dom, root, 0),
            "One[^fn1] and again [^fn1].\n\n[^fn1]: See [source](https://example.test).\n\n    More detail.\n"
        );
    }

    #[test]
    fn adopts_only_referenced_external_definitions() {
        let source = Dom::parse_fragment(r##"<article><p>Text<sup><a href="#fn1">1</a></sup> and again <sup><a href="#fn1">1</a></sup>.</p></article><footer class="footnotes"><div id="fn1" role="doc-footnote">Kept note.</div><div id="fn2" role="doc-footnote">Unused note.</div></footer>"##, Tag::Div).unwrap();
        let selected = source
            .first_descendant_by_tag(source.root(), Tag::Article)
            .unwrap();
        let definitions = collect_external(&source);
        let mut fragment = source.copy_subtree_as_fragment(selected).unwrap();
        let root = fragment.root();
        adopt_external(&definitions, &mut fragment, root);
        normalize(&mut fragment, root);
        let markdown = dom_to_markdown(&fragment, root, 0);
        assert!(markdown.contains("[^fn1]: Kept note."), "{markdown}");
        assert_eq!(markdown.matches("[^fn1]:").count(), 1, "{markdown}");
        assert!(!markdown.contains("Unused note"), "{markdown}");
    }

    #[test]
    fn keeps_valid_list_ancestry_and_copies_only_outer_definitions() {
        let source = Dom::parse_fragment(
            r#"<section class="footnotes"><ol><li id="fn1" role="doc-footnote">Outer <span id="fn-inner" role="doc-footnote">nested marker</span></li></ol></section>"#,
            Tag::Div,
        )
        .unwrap();
        let definitions = collect_external(&source);
        assert_eq!(definitions.0.len(), 1);
        let mut dom = source;
        let root = dom.root();
        normalize(&mut dom, root);
        let html = crate::dom::render_html(&dom, root, 0);
        assert!(html.contains("<ol><li"), "{html}");
        assert!(!html.contains("<ol><div"), "{html}");
    }

    #[test]
    fn external_list_definitions_get_a_valid_standalone_root() {
        let source = Dom::parse_fragment(
            r##"<article><p>Text<a role="doc-noteref" href="#fn1">1</a>.</p></article><ol class="footnotes"><li id="fn1" role="doc-footnote">A note.</li></ol>"##,
            Tag::Div,
        )
        .unwrap();
        let article = source
            .first_descendant_by_tag(source.root(), Tag::Article)
            .unwrap();
        let definitions = collect_external(&source);
        let mut fragment = source.copy_subtree_as_fragment(article).unwrap();
        let root = fragment.root();
        adopt_external(&definitions, &mut fragment, root);
        normalize(&mut fragment, root);
        let html = crate::dom::render_html(&fragment, root, 0);
        assert!(html.contains("<section"), "{html}");
        assert!(html.contains("<div id=\"fn1\""), "{html}");
        assert!(
            !html.contains("<section data-legible-footnotes=\"\"><li"),
            "{html}"
        );
    }

    #[test]
    fn keeps_a_missing_inferred_reference_as_a_link() {
        let mut dom = Dom::parse_fragment(
            r##"<p>Text<sup><a href="#fn404">404</a></sup>.</p>"##,
            Tag::Div,
        )
        .unwrap();
        let root = dom.root();
        normalize(&mut dom, root);
        assert_eq!(dom_to_markdown(&dom, root, 0), "Text[404](#fn404).\n");
    }

    #[test]
    fn keeps_an_ordinary_references_heading() {
        let mut dom = Dom::parse_fragment(
            r#"<section id="references"><h2>References</h2><p>Smith, Example Book.</p></section>"#,
            Tag::Div,
        )
        .unwrap();
        let root = dom.root();
        normalize(&mut dom, root);
        assert_eq!(
            dom_to_markdown(&dom, root, 0),
            "## References\n\nSmith, Example Book.\n"
        );
    }
}
