#![allow(dead_code)]

use super::{AttrName, Attribute, NodeLink, Tag};
use html5ever::QualName;
use tendril::StrTendril;

#[derive(Clone, Debug)]
pub(crate) enum NodeData {
    Document,
    Fragment,
    Doctype {
        name: StrTendril,
        _public_id: StrTendril,
        _system_id: StrTendril,
    },
    Text(StrTendril),
    Comment(StrTendril),
    Element(ElementData),
    ProcessingInstruction {
        target: StrTendril,
        contents: StrTendril,
    },
}
#[derive(Clone, Debug)]
pub(crate) struct ElementData {
    pub(crate) name: QualName,
    pub(crate) tag: Tag,
    pub(crate) attrs: Vec<Attribute>,
    pub(crate) template_contents: NodeLink,
    pub(crate) mathml_annotation_xml_integration_point: bool,
}
#[derive(Clone, Debug)]
pub(crate) struct Node {
    pub(crate) parent: NodeLink,
    pub(crate) prev_sibling: NodeLink,
    pub(crate) next_sibling: NodeLink,
    pub(crate) first_child: NodeLink,
    pub(crate) last_child: NodeLink,
    pub(crate) data: NodeData,
}
impl Node {
    pub(crate) fn new(data: NodeData) -> Self {
        Self {
            parent: NodeLink::NONE,
            prev_sibling: NodeLink::NONE,
            next_sibling: NodeLink::NONE,
            first_child: NodeLink::NONE,
            last_child: NodeLink::NONE,
            data,
        }
    }
}
impl ElementData {
    #[inline]
    pub(crate) fn attr(&self, name: AttrName) -> Option<&str> {
        self.attrs
            .iter()
            .find(|attribute| attribute.is_named(name))
            .map(|attribute| attribute.value.as_ref())
    }
    #[inline]
    pub(crate) fn attr_local(&self, name: &str) -> Option<&str> {
        // These dynamic names are queried often but are not part of the
        // typed attribute set. Avoid running the full attribute-name matcher
        // before scanning the small per-element attribute list.
        if matches!(
            name,
            "action"
                | "alt"
                | "aria-level"
                | "data-callout"
                | "data-fn"
                | "data-footnote"
                | "data-footnote-ref"
                | "data-footnotes"
                | "data-type"
                | "for"
        ) {
            return self
                .attrs
                .iter()
                .find(|attribute| attribute.name.local.as_ref().eq_ignore_ascii_case(name))
                .map(|attribute| attribute.value.as_ref());
        }
        let kind = AttrName::from_local(name);
        if kind != AttrName::Other {
            return self
                .attrs
                .iter()
                .find(|attribute| attribute.is_named(kind))
                .map(|attribute| attribute.value.as_ref());
        }
        self.attrs
            .iter()
            .find(|attribute| {
                let local = attribute.name.local.as_ref();
                local == name || local.eq_ignore_ascii_case(name)
            })
            .map(|attribute| attribute.value.as_ref())
    }
}
