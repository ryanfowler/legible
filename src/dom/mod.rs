//! A compact, parser-compatible DOM specialized for Readability extraction.
mod arena;
mod attr;
mod id;
mod mutation;
mod node;
mod parse;
mod query;
mod serialize;
mod state;
mod tag;
mod traversal;

pub(crate) use arena::Dom;
pub(crate) use attr::{AttrName, Attribute};
pub(crate) use id::{DomError, NodeId, NodeLink};
pub(crate) use node::{ElementData, Node, NodeData};
pub(crate) use state::{DataTableState, NodeStateStore, NodeStats};
pub(crate) use tag::Tag;
pub(crate) use traversal::build_match_string;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_serializes_html() {
        let mut dom = Dom::parse_document(
            "<!doctype html><html><body><p title='a &amp; b'>Hello <b>world</b></p></body></html>",
        )
        .unwrap();
        let body = dom.body().unwrap();
        assert_eq!(dom.text(body), "Hello world");
        let paragraph = dom.first_descendant_by_tag(body, Tag::P).unwrap();
        assert_eq!(dom.attr_by_local_name(paragraph, "title"), Some("a & b"));
        dom.set_attr(paragraph, AttrName::Other, "a < b");
        let serialized = dom.html(dom.root()).unwrap();
        assert!(serialized.contains("title=\"a &lt; b\""));
        assert!(!serialized.contains("&amp;lt;"));
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
        assert_eq!(div.inner_html(div.root()).unwrap(), "<b>x</b><!-- note -->");

        let table = Dom::parse_fragment("<tr><td>x</td></tr>", Tag::Table).unwrap();
        assert_eq!(
            table.inner_html(table.root()).unwrap(),
            "<tbody><tr><td>x</td></tr></tbody>"
        );
    }

    #[test]
    fn foreign_namespaces_and_templates_serialize_exactly() {
        let dom = Dom::parse_document(
            "<body><svg viewBox='0 0 1 1'><foreignObject><p>x</p></foreignObject></svg>\
             <math><mi>x</mi></math><template><em>saved</em></template>",
        )
        .unwrap();
        let body = dom.body().unwrap();
        assert_eq!(
            dom.inner_html(body).unwrap(),
            "<svg viewBox=\"0 0 1 1\"><foreignObject><p>x</p></foreignObject></svg>\
             <math><mi>x</mi></math><template><em>saved</em></template>"
        );
    }

    #[test]
    fn malformed_html_has_exact_repaired_structure() {
        let dom = Dom::parse_document("<title>T</title><p>one<div>two</p>three").unwrap();
        assert_eq!(
            dom.html(dom.root()).unwrap(),
            "<html><head><title>T</title></head><body><p>one</p><div>two<p></p>three</div></body></html>"
        );
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
        assert_eq!(
            dom.inner_html(dom.body().unwrap()).unwrap(),
            "<p id=\"p\"><b id=\"bold\">bold<i id=\"italic\">both</i></b><i id=\"italic\"><span id=\"inside\">italic</span></i><span id=\"after\">after</span></p>"
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
}
