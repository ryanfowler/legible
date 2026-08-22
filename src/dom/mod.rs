//! A compact, parser-compatible DOM for deterministic content extraction.
mod arena;
mod attr;
mod id;
mod mutation;
mod node;
mod parse;
mod query;
mod state;
mod tag;
mod traversal;

pub(crate) use arena::Dom;
pub(crate) use attr::{AttrName, Attribute};
pub(crate) use id::{DomError, NodeId, NodeLink};
pub(crate) use node::{ElementData, Node, NodeData};
pub(crate) use parse::{ParseError, ParseLimitKind};
pub(crate) use state::{DataTableState, NodeStateStore, NodeStats, ScoreStore};
pub(crate) use tag::Tag;
pub(crate) use traversal::DocumentAnchors;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_html_and_preserves_attributes() {
        let mut dom = Dom::parse_document(
            "<!doctype html><html><body><p title='a &amp; b'>Hello <b>world</b></p></body></html>",
        )
        .unwrap();
        let body = dom.body().unwrap();
        assert_eq!(dom.text(body), "Hello world");
        let paragraph = dom.first_descendant_by_tag(body, Tag::P).unwrap();
        assert_eq!(dom.attr_by_local_name(paragraph, "title"), Some("a & b"));
        dom.set_attr(paragraph, AttrName::Title, "a < b");
        assert_eq!(dom.attr(paragraph, AttrName::Title), Some("a < b"));
        dom.validate().unwrap();
    }

    #[test]
    fn owned_payloads_can_move_text() {
        let mut dom = Dom::parse_fragment("<p title='payload'>large source</p>", Tag::Div).unwrap();
        let paragraph = dom.first_descendant_by_tag(dom.root(), Tag::P).unwrap();
        let text = dom.first_child(paragraph).unwrap();

        let moved_text = dom.take_text(text).unwrap();

        assert_eq!(moved_text.as_ref(), "large source");
        assert_eq!(dom.text_node(text), Some(""));
        assert_eq!(dom.attr(paragraph, AttrName::Title), Some("payload"));
        dom.validate().unwrap();
    }

    #[test]
    fn mutation_preserves_links_and_ids() {
        let mut dom = Dom::parse_document("<div id=a><p>one</p><p>two</p></div>").unwrap();
        let root = dom.first_descendant_by_tag(dom.root(), Tag::Div).unwrap();
        let children: Vec<_> = dom.element_children(root).collect();
        let first = children[0];
        let second = children[1];
        dom.detach(first);
        dom.append_child(root, first);
        assert_eq!(dom.first_child(root), Some(second));
        assert_eq!(dom.last_child(root), Some(first));
        dom.replace_with(second, first);
        dom.validate().unwrap();
        assert!(dom.parent(first).is_some());
    }

    #[test]
    fn fragments_import_and_keep_stable_ids() {
        let mut dom = Dom::parse_document("<div><p>old</p></div>").unwrap();
        let div = dom.first_descendant_by_tag(dom.root(), Tag::Div).unwrap();
        let old = dom.first_descendant_by_tag(div, Tag::P).unwrap();
        dom.set_inner_html(div, "<p>new</p><span>text</span>")
            .unwrap();
        assert!(dom.contains(old));
        assert_eq!(dom.text(div), "newtext");
        dom.validate().unwrap();
    }

    #[test]
    fn set_inner_html_on_body_imports_fragment_children() {
        let mut dom = Dom::parse_document("<html><body></body></html>").unwrap();
        let body = dom.body().unwrap();

        dom.set_inner_html(body, "<div>body content</div>").unwrap();

        let child = dom.first_child(body).unwrap();
        assert_eq!(dom.tag(child), Some(Tag::Div));
        assert!(!dom.children(body).any(|id| dom.tag(id) == Some(Tag::Html)));
        dom.validate().unwrap();
    }

    #[test]
    fn set_inner_html_on_figure_imports_fragment_children() {
        let mut dom = Dom::parse_document("<html><body><figure></figure></body></html>").unwrap();
        let figure = dom
            .first_descendant_by_tag(dom.root(), Tag::Figure)
            .unwrap();

        dom.set_inner_html(figure, "<img src=\"image.jpg\">")
            .unwrap();

        let child = dom.first_child(figure).unwrap();
        assert_eq!(dom.tag(child), Some(Tag::Img));
        assert!(
            !dom.children(figure)
                .any(|id| dom.tag(id) == Some(Tag::Html))
        );
        dom.validate().unwrap();
    }

    #[test]
    fn fragment_contexts_have_exact_structure() {
        let div = Dom::parse_fragment("<b>x</b><!-- note -->", Tag::Div).unwrap();
        let div_children: Vec<_> = div.children(div.root()).collect();
        assert_eq!(div_children.len(), 2);
        assert_eq!(div.tag(div_children[0]), Some(Tag::B));
        assert_eq!(div.text(div_children[0]), "x");
        assert!(div.is_comment(div_children[1]));

        let table = Dom::parse_fragment("<tr><td>x</td></tr>", Tag::Table).unwrap();
        let tbody = table
            .first_descendant_by_tag(table.root(), Tag::Tbody)
            .unwrap();
        let row = table.first_descendant_by_tag(tbody, Tag::Tr).unwrap();
        let cell = table.first_descendant_by_tag(row, Tag::Td).unwrap();
        assert_eq!(table.text(cell), "x");
    }

    #[test]
    fn foreign_namespaces_and_templates_are_retained() {
        let dom = Dom::parse_document(
            "<body><svg viewBox='0 0 1 1'><foreignObject><p>x</p></foreignObject></svg>\
             <math><mi>x</mi></math><template><em>saved</em></template>",
        )
        .unwrap();
        let body = dom.body().unwrap();
        let svg = dom.first_descendant_by_tag(body, Tag::Svg).unwrap();
        assert_eq!(dom.attr_by_local_name(svg, "viewBox"), Some("0 0 1 1"));
        assert_eq!(
            dom.qual_name(svg).unwrap().ns.as_ref(),
            "http://www.w3.org/2000/svg"
        );
        let foreign_object = dom
            .descendants(svg)
            .find(|&node| {
                dom.qual_name(node)
                    .is_some_and(|name| name.local.as_ref() == "foreignObject")
            })
            .unwrap();
        assert_eq!(
            dom.qual_name(foreign_object).unwrap().ns.as_ref(),
            "http://www.w3.org/2000/svg"
        );
        let math = dom.first_descendant_by_tag(body, Tag::Math).unwrap();
        assert_eq!(
            dom.qual_name(math).unwrap().ns.as_ref(),
            "http://www.w3.org/1998/Math/MathML"
        );
        let template = dom.first_descendant_by_tag(body, Tag::Template).unwrap();
        let NodeData::Element(element) = &dom.node(template).data else {
            panic!("template is not an element");
        };
        let template_contents = element.template_contents.get().expect("template contents");
        assert_eq!(
            dom.first_child(template_contents)
                .and_then(|id| dom.tag(id)),
            Some(Tag::Em)
        );
        assert_eq!(dom.text(template_contents), "saved");
    }

    #[test]
    fn malformed_html_has_exact_repaired_structure() {
        let dom = Dom::parse_document("<title>T</title><p>one<div>two</p>three").unwrap();
        let tags: Vec<_> = dom
            .descendants(dom.root())
            .filter_map(|node| dom.tag(node))
            .collect();
        assert_eq!(
            tags,
            [
                Tag::Html,
                Tag::Head,
                Tag::Title,
                Tag::Body,
                Tag::P,
                Tag::Div,
                Tag::P
            ]
        );
        let title = dom.first_descendant_by_tag(dom.root(), Tag::Title).unwrap();
        assert_eq!(dom.text(title), "T");
        let body = dom.body().unwrap();
        let paragraph = dom.first_descendant_by_tag(body, Tag::P).unwrap();
        let div = dom.first_descendant_by_tag(body, Tag::Div).unwrap();
        assert_eq!(dom.text(paragraph), "one");
        assert_eq!(dom.text(div), "twothree");
        assert_eq!(dom.parent(paragraph), Some(body));
        assert_eq!(dom.parent(div), Some(body));
        let empty_paragraph = dom
            .children(div)
            .find(|&node| dom.tag(node) == Some(Tag::P))
            .unwrap();
        assert_eq!(dom.text(empty_paragraph), "");
    }

    #[test]
    fn source_snapshot_caches_repaired_document_anchors() {
        let dom = Dom::parse_document(
            "<base href='https://example.com/base/'><title>T</title><p>content</p>",
        )
        .unwrap();
        let anchors = dom.document_anchors();
        let snapshot = dom.element_descendants_snapshot_with_depth(dom.root());
        let html = dom
            .descendants(dom.root())
            .find(|&node| dom.tag(node) == Some(Tag::Html));

        assert_eq!(anchors.root, dom.root());
        assert_eq!(anchors.html, html);
        assert_eq!(anchors.body, dom.body());
        assert_eq!(
            anchors
                .first_base_with_href
                .and_then(|node| dom.attr(node, AttrName::Href)),
            Some("https://example.com/base/")
        );
        assert!(
            snapshot
                .iter()
                .any(|&(node, _)| node == anchors.body.unwrap())
        );
        assert!(anchors.valid_for(&dom));
    }

    #[test]
    fn source_snapshot_handles_fragments_without_a_body() {
        let dom = Dom::parse_fragment("<h1>deep content</h1>", Tag::Div).unwrap();
        let anchors = dom.document_anchors();
        let snapshot = dom.element_descendants_snapshot_with_depth(dom.root());

        assert_eq!(anchors.root, dom.root());
        assert_eq!(anchors.html, None);
        assert_eq!(anchors.body, None);
        assert_eq!(snapshot.len(), 1);
        assert!(anchors.valid_for(&dom));
    }

    #[test]
    fn source_snapshot_keeps_the_first_repaired_html_and_body() {
        let dom =
            Dom::parse_document("<html><body><p>first</p></body><body><p>second</p></body></html>")
                .unwrap();
        let anchors = dom.document_anchors();
        let html_count = dom
            .descendants(dom.root())
            .filter(|&node| dom.tag(node) == Some(Tag::Html))
            .count();
        let body_count = dom
            .descendants(dom.root())
            .filter(|&node| dom.tag(node) == Some(Tag::Body))
            .count();

        assert_eq!(html_count, 1);
        assert_eq!(body_count, 1);
        assert_eq!(
            anchors.html,
            dom.descendants(dom.root())
                .find(|&node| { dom.tag(node) == Some(Tag::Html) })
        );
        assert_eq!(
            anchors.body,
            dom.descendants(dom.root())
                .find(|&node| { dom.tag(node) == Some(Tag::Body) })
        );
    }

    #[test]
    fn detached_document_anchors_are_rejected() {
        let mut dom = Dom::parse_document("<body><main>content</main></body>").unwrap();
        let anchors = dom.document_anchors();
        dom.detach(anchors.body.unwrap());

        assert!(!anchors.valid_for(&dom));

        let mut moved_dom = Dom::parse_document("<body><main>content</main></body>").unwrap();
        let moved_anchors = moved_dom.document_anchors();
        let detached_parent = moved_dom.create_html_element(Tag::Div).unwrap();
        moved_dom.append_child(detached_parent, moved_anchors.body.unwrap());

        assert!(!moved_anchors.valid_for(&moved_dom));
    }

    #[test]
    fn source_snapshot_preserves_depth_for_repaired_headings() {
        let wrappers = "<div>".repeat(70);
        let closing = "</div>".repeat(70);
        let dom = Dom::parse_document(&format!(
            "<body>{wrappers}<h1>Deep title</h1>{closing}</body>"
        ))
        .unwrap();
        let snapshot = dom.element_descendants_snapshot_with_depth(dom.root());
        let heading = snapshot
            .iter()
            .find(|&&(node, _)| dom.tag(node) == Some(Tag::H1))
            .copied()
            .unwrap();

        assert!(heading.1 > 64);
    }

    #[test]
    fn descendants_snapshot_follows_foster_parented_dom_order() {
        let dom = Dom::parse_document(
            "<body><table id=table><div id=foster>before</div><tr><td id=cell>cell</td></tr></table><p id=after>after</p>",
        )
        .unwrap();
        let ids = |nodes: Vec<NodeId>| {
            nodes
                .into_iter()
                .filter_map(|id| dom.attr(id, AttrName::Id))
                .collect::<Vec<_>>()
        };

        assert_eq!(
            ids(dom.node_ids().collect()),
            ["table", "foster", "cell", "after"]
        );
        assert_eq!(
            ids(dom.descendants_snapshot(dom.root())),
            ["foster", "table", "cell", "after"]
        );
    }

    #[test]
    fn element_snapshot_excludes_non_elements_and_records_depth() {
        let dom = Dom::parse_document(
            "<body><div id=outer>text<!-- comment --><span id=inner>x</span></div><p id=after>y</p></body>",
        )
        .unwrap();
        let body = dom.body().unwrap();
        let nodes = dom.element_descendants_snapshot_with_depth(body);
        let actual: Vec<_> = nodes
            .into_iter()
            .map(|(id, depth)| (dom.attr(id, AttrName::Id).unwrap(), depth))
            .collect();

        assert_eq!(actual, [("outer", 1), ("inner", 2), ("after", 1)]);
    }

    #[test]
    fn descendants_snapshot_follows_misnested_formatting_repair() {
        let dom = Dom::parse_document(
            "<body><p id=p><b id=bold>bold<i id=italic>both</b><span id=inside>italic</span></i><span id=after>after</span></p>",
        )
        .unwrap();
        let ids: Vec<_> = dom
            .descendants_snapshot(dom.root())
            .into_iter()
            .filter_map(|id| dom.attr(id, AttrName::Id))
            .collect();

        assert_eq!(ids, ["p", "bold", "italic", "italic", "inside", "after"]);
        let paragraph = dom.first_descendant_by_tag(dom.root(), Tag::P).unwrap();
        let bold = dom.first_descendant_by_tag(paragraph, Tag::B).unwrap();
        let italic_nodes: Vec<_> = dom
            .descendants(paragraph)
            .filter(|&node| {
                dom.tag(node) == Some(Tag::I) && dom.attr(node, AttrName::Id) == Some("italic")
            })
            .collect();
        let inside = dom
            .descendants(paragraph)
            .find(|&node| dom.attr(node, AttrName::Id) == Some("inside"))
            .unwrap();
        let after = dom
            .descendants(paragraph)
            .find(|&node| dom.attr(node, AttrName::Id) == Some("after"))
            .unwrap();
        assert_eq!(italic_nodes.len(), 2);
        assert_eq!(dom.parent(italic_nodes[0]), Some(bold));
        assert_eq!(dom.parent(italic_nodes[1]), Some(paragraph));
        assert_eq!(dom.parent(inside), Some(italic_nodes[1]));
        assert_eq!(dom.parent(after), Some(paragraph));
    }

    #[test]
    fn capped_normalized_count_is_exact_only_below_threshold() {
        let dom = Dom::parse_document("<p>  one </p><p> two  three </p>").unwrap();
        let body = dom.body().unwrap();

        assert_eq!(dom.normalized_char_count(body), 13);
        assert_eq!(dom.normalized_char_count_below(body, 14), Some(13));
        assert_eq!(dom.normalized_char_count_below(body, 13), None);
        assert_eq!(dom.normalized_char_count_below(body, 1), None);
        assert_eq!(dom.normalized_char_count_below(body, 0), None);
        let (text, length) = dom.normalized_text(body, 4);
        assert_eq!(text, "one two three");
        assert_eq!(length, 13);
    }

    #[test]
    fn limited_text_scans_stop_at_normalized_character_boundaries() {
        let dom = Dom::parse_fragment("<p>  one </p><p> two  three </p>", Tag::Div).unwrap();
        let root = dom.root();
        let mut text = String::new();

        dom.append_normalized_text_limited(root, &mut text, 0);
        assert!(text.is_empty());
        dom.append_normalized_text_limited(root, &mut text, 4);
        assert_eq!(text, "one ");
        text.clear();
        dom.append_normalized_text_limited(root, &mut text, 7);
        assert_eq!(text, "one two");

        let unicode = Dom::parse_fragment("<p>  日本語 \t世界 </p>", Tag::Div).unwrap();
        let mut text = String::new();
        unicode.append_normalized_text_limited(unicode.root(), &mut text, 4);
        assert_eq!(text, "日本語 ");
    }

    #[test]
    fn table_descendants_stop_before_nested_table_contents() {
        let dom = Dom::parse_fragment(
            "<table><tr><td>outer</td><td><table><tr><td>inner</td></tr></table></td></tr></table>",
            Tag::Div,
        )
        .unwrap();
        let outer = dom
            .descendants(dom.root())
            .find(|&node| dom.tag(node) == Some(Tag::Table))
            .unwrap();
        let nodes = dom.table_descendants(outer);
        assert!(nodes.iter().any(|&node| dom.tag(node) == Some(Tag::Table)));
        assert!(
            !nodes
                .iter()
                .any(|&node| dom.text_node(node) == Some("inner"))
        );
    }

    #[test]
    fn deeply_nested_input_is_stack_safe() {
        const DEPTH: usize = 1_000;
        let mut html = "<div>".repeat(DEPTH);
        html.push('x');
        html.push_str(&"</div>".repeat(DEPTH));

        let dom = Dom::parse_document(&html).unwrap();
        assert_eq!(dom.text(dom.body().unwrap()), "x");
        assert_eq!(dom.descendants(dom.body().unwrap()).count(), DEPTH + 1);
        dom.validate().unwrap();
    }

    #[test]
    fn validation_rejects_parent_cycles_and_a_parented_root() {
        let mut cycle = Dom::parse_fragment("<div><span></span></div>", Tag::Div).unwrap();
        let root = cycle.root();
        let div = cycle.first_descendant_by_tag(root, Tag::Div).unwrap();
        let span = cycle.first_descendant_by_tag(div, Tag::Span).unwrap();
        cycle.node_mut(root).first_child = NodeLink::NONE;
        cycle.node_mut(root).last_child = NodeLink::NONE;
        cycle.node_mut(div).parent = NodeLink::from_option(Some(span));
        cycle.node_mut(span).first_child = NodeLink::from_option(Some(div));
        cycle.node_mut(span).last_child = NodeLink::from_option(Some(div));
        assert!(cycle.validate().is_err());

        let mut parented_root = Dom::parse_fragment("<div></div>", Tag::Div).unwrap();
        let root = parented_root.root();
        let div = parented_root
            .first_descendant_by_tag(root, Tag::Div)
            .unwrap();
        parented_root.node_mut(root).parent = NodeLink::from_option(Some(div));
        assert!(parented_root.validate().is_err());
    }
}
