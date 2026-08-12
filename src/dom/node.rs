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
            .find(|attribute| name.matches_local(attribute.name.local.as_ref()))
            .map(|attribute| attribute.value.as_ref())
    }
    #[inline]
    pub(crate) fn attr_local(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|a| a.name.local.as_ref().eq_ignore_ascii_case(name))
            .map(|a| a.value.as_ref())
    }
}
