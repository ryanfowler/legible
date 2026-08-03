#![allow(clippy::collapsible_if)]

use super::{AttrName, Dom, DomError, ElementData, NodeData, NodeId, NodeLink, Tag};
use html5ever::tokenizer::TokenizerOpts;
use html5ever::tree_builder::{ElemName, ElementFlags, NodeOrText, QuirksMode, TreeSink};
use html5ever::{Attribute as HtmlAttribute, ParseOpts, QualName, parse_document, parse_fragment};
use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use tendril::{StrTendril, TendrilSink};

#[derive(Debug, Clone)]
struct OwnedElemName(QualName);
impl ElemName for OwnedElemName {
    fn ns(&self) -> &html5ever::Namespace {
        &self.0.ns
    }
    fn local_name(&self) -> &html5ever::LocalName {
        &self.0.local
    }
}
struct DomSink {
    dom: RefCell<Dom>,
    quirks: Cell<QuirksMode>,
}
impl DomSink {
    fn new(fragment: bool) -> Self {
        Self {
            dom: RefCell::new(Dom::new(if fragment {
                NodeData::Fragment
            } else {
                NodeData::Document
            })),
            quirks: Cell::new(html5ever::tree_builder::NoQuirks),
        }
    }
}
fn opts(drop_doctype: bool) -> ParseOpts {
    ParseOpts {
        tokenizer: TokenizerOpts::default(),
        tree_builder: html5ever::tree_builder::TreeBuilderOpts {
            scripting_enabled: false,
            drop_doctype,
            ..Default::default()
        },
    }
}
impl Dom {
    pub(crate) fn parse_document(html: &str) -> Result<Self, DomError> {
        Ok(parse_document(DomSink::new(false), opts(false)).one(html))
    }
    pub(crate) fn parse_fragment(html: &str, context: Tag) -> Result<Self, DomError> {
        let sink = DomSink::new(true);
        let context = QualName::new(
            None,
            html5ever::ns!(html),
            html5ever::LocalName::from(context.as_lowercase_str()),
        );
        let mut dom = parse_fragment(sink, opts(true), context, Vec::new(), false).one(html);

        // html5ever follows the HTML fragment parsing algorithm and inserts a
        // synthetic <html> element below the sink's document node. The DOM
        // represents fragments with that node directly, so expose the
        // synthetic element's children as the fragment roots instead.
        let root = dom.root();
        if let Some(fragment_root) = dom
            .children(root)
            .find(|&id| dom.tag(id) == Some(Tag::Html))
        {
            let children: Vec<_> = dom.children(fragment_root).collect();
            for child in children {
                dom.insert_before(fragment_root, child)
            }
            dom.detach(fragment_root);
        }
        Ok(dom)
    }
}
impl TreeSink for DomSink {
    type Handle = NodeId;
    type Output = Dom;
    type ElemName<'a>
        = OwnedElemName
    where
        Self: 'a;
    fn finish(self) -> Dom {
        let mut dom = self.dom.into_inner();
        dom.quirks_mode = self.quirks.get();
        dom
    }
    fn parse_error(&self, _msg: Cow<'static, str>) {}
    fn get_document(&self) -> NodeId {
        self.dom.borrow().root()
    }
    fn elem_name<'a>(&'a self, target: &'a NodeId) -> OwnedElemName {
        OwnedElemName(
            self.dom
                .borrow()
                .qual_name(*target)
                .expect("element")
                .clone(),
        )
    }
    fn create_element(
        &self,
        name: QualName,
        attrs: Vec<HtmlAttribute>,
        flags: ElementFlags,
    ) -> NodeId {
        let mut dom = self.dom.borrow_mut();
        let tag = Tag::from_qual_name(&name);
        let id = dom
            .create(NodeData::Element(ElementData {
                name,
                tag,
                attrs: attrs
                    .into_iter()
                    .map(|a| {
                        let known = AttrName::from_local(a.name.local.as_ref());
                        super::Attribute {
                            name: a.name,
                            known,
                            value: a.value,
                        }
                    })
                    .collect(),
                template_contents: NodeLink::NONE,
                mathml_annotation_xml_integration_point: flags
                    .mathml_annotation_xml_integration_point,
            }))
            .expect("DOM node limit");
        if flags.template {
            let f = dom.create(NodeData::Fragment).expect("DOM node limit");
            if let NodeData::Element(e) = &mut dom.node_mut(id).data {
                e.template_contents = NodeLink::from_option(Some(f))
            }
        }
        id
    }
    fn create_comment(&self, text: StrTendril) -> NodeId {
        self.dom
            .borrow_mut()
            .create(NodeData::Comment(text))
            .expect("DOM node limit")
    }
    fn create_pi(&self, target: StrTendril, data: StrTendril) -> NodeId {
        self.dom
            .borrow_mut()
            .create(NodeData::ProcessingInstruction {
                target,
                contents: data,
            })
            .expect("DOM node limit")
    }
    fn append(&self, parent: &NodeId, child: NodeOrText<NodeId>) {
        let mut d = self.dom.borrow_mut();
        match child {
            NodeOrText::AppendNode(id) => d.append_child(*parent, id),
            NodeOrText::AppendText(text) => {
                if let Some(last) = d.last_child(*parent) {
                    if let NodeData::Text(existing) = &mut d.node_mut(last).data {
                        existing.push_tendril(&text);
                        return;
                    }
                }
                let id = d.create(NodeData::Text(text)).expect("DOM node limit");
                d.append_child(*parent, id)
            }
        }
    }
    fn append_based_on_parent_node(
        &self,
        element: &NodeId,
        prev: &NodeId,
        child: NodeOrText<NodeId>,
    ) {
        if self.dom.borrow().parent(*element).is_some() {
            self.append_before_sibling(element, child)
        } else {
            self.append(prev, child)
        }
    }
    fn append_doctype_to_document(
        &self,
        name: StrTendril,
        public_id: StrTendril,
        system_id: StrTendril,
    ) {
        let mut d = self.dom.borrow_mut();
        let id = d
            .create(NodeData::Doctype {
                name,
                _public_id: public_id,
                _system_id: system_id,
            })
            .expect("DOM node limit");
        let root = d.root();
        d.append_child(root, id)
    }
    fn get_template_contents(&self, target: &NodeId) -> NodeId {
        self.dom
            .borrow()
            .node(*target)
            .data
            .element_template()
            .expect("template")
    }
    fn same_node(&self, x: &NodeId, y: &NodeId) -> bool {
        x == y
    }
    fn set_quirks_mode(&self, mode: QuirksMode) {
        self.quirks.set(mode)
    }
    fn append_before_sibling(&self, sibling: &NodeId, child: NodeOrText<NodeId>) {
        let mut d = self.dom.borrow_mut();
        match child {
            NodeOrText::AppendNode(id) => d.insert_before(*sibling, id),
            NodeOrText::AppendText(text) => {
                if let Some(prev) = d.prev_sibling(*sibling) {
                    if let NodeData::Text(existing) = &mut d.node_mut(prev).data {
                        existing.push_tendril(&text);
                        return;
                    }
                }
                let id = d.create(NodeData::Text(text)).expect("DOM node limit");
                d.insert_before(*sibling, id)
            }
        }
    }
    fn add_attrs_if_missing(&self, target: &NodeId, attrs: Vec<HtmlAttribute>) {
        let mut d = self.dom.borrow_mut();
        if let NodeData::Element(e) = &mut d.node_mut(*target).data {
            for a in attrs {
                if !e.attrs.iter().any(|x| x.name == a.name) {
                    let known = AttrName::from_local(a.name.local.as_ref());
                    e.attrs.push(super::Attribute {
                        name: a.name,
                        known,
                        value: a.value,
                    })
                }
            }
        }
    }
    fn remove_from_parent(&self, target: &NodeId) {
        self.dom.borrow_mut().detach(*target)
    }
    fn reparent_children(&self, node: &NodeId, new_parent: &NodeId) {
        let mut d = self.dom.borrow_mut();
        d.move_children(*node, *new_parent)
    }
    fn is_mathml_annotation_xml_integration_point(&self, handle: &NodeId) -> bool {
        match &self.dom.borrow().node(*handle).data {
            NodeData::Element(e) => e.mathml_annotation_xml_integration_point,
            _ => false,
        }
    }
}
trait ElementTemplate {
    fn element_template(&self) -> Option<NodeId>;
}
impl ElementTemplate for NodeData {
    fn element_template(&self) -> Option<NodeId> {
        match self {
            NodeData::Element(e) => e.template_contents.get(),
            _ => None,
        }
    }
}
