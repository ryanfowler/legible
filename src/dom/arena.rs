use super::{AttrName, DomError, ElementData, Node, NodeData, NodeId, NodeLink, Tag};
use html5ever::{LocalName, QualName, ns};
use tendril::StrTendril;

#[derive(Clone)]
pub(crate) struct Dom {
    pub(crate) nodes: Vec<Node>,
    pub(crate) root: NodeId,
    pub(crate) quirks_mode: html5ever::tree_builder::QuirksMode,
}
impl Dom {
    pub(crate) fn new(data: NodeData) -> Self {
        Self::with_capacity(data, 1)
    }
    pub(crate) fn with_capacity(data: NodeData, capacity: usize) -> Self {
        let mut nodes = Vec::with_capacity(capacity.max(1));
        nodes.push(Node::new(data));
        Self {
            nodes,
            root: NodeId(0),
            quirks_mode: html5ever::tree_builder::NoQuirks,
        }
    }
    pub(crate) fn len(&self) -> usize {
        self.nodes.len()
    }
    pub(crate) fn root(&self) -> NodeId {
        self.root
    }
    #[cfg(test)]
    pub(crate) fn contains(&self, id: NodeId) -> bool {
        id.index() < self.nodes.len()
    }
    pub(crate) fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id.index()]
    }
    pub(crate) fn node_mut(&mut self, id: NodeId) -> &mut Node {
        &mut self.nodes[id.index()]
    }
    pub(crate) fn create(&mut self, data: NodeData) -> Result<NodeId, DomError> {
        let i = u32::try_from(self.nodes.len())
            .map_err(|_| DomError("DOM exceeds NodeId capacity".into()))?;
        self.nodes.push(Node::new(data));
        Ok(NodeId(i))
    }
    pub(crate) fn create_element(&mut self, name: QualName) -> Result<NodeId, DomError> {
        self.create(NodeData::Element(ElementData {
            name: name.clone(),
            tag: Tag::from_qual_name(&name),
            attrs: Vec::new(),
            template_contents: NodeLink::NONE,
            mathml_annotation_xml_integration_point: false,
        }))
    }
    pub(crate) fn create_html_element(&mut self, tag: Tag) -> Result<NodeId, DomError> {
        self.create_element(QualName::new(
            None,
            ns!(html),
            LocalName::from(tag.as_lowercase_str()),
        ))
    }
    pub(crate) fn create_text(&mut self, text: &str) -> Result<NodeId, DomError> {
        self.create(NodeData::Text(StrTendril::from(text)))
    }
    pub(crate) fn parent(&self, id: NodeId) -> Option<NodeId> {
        self.node(id).parent.get()
    }
    pub(crate) fn prev_sibling(&self, id: NodeId) -> Option<NodeId> {
        self.node(id).prev_sibling.get()
    }
    pub(crate) fn next_sibling(&self, id: NodeId) -> Option<NodeId> {
        self.node(id).next_sibling.get()
    }
    pub(crate) fn first_child(&self, id: NodeId) -> Option<NodeId> {
        self.node(id).first_child.get()
    }
    pub(crate) fn last_child(&self, id: NodeId) -> Option<NodeId> {
        self.node(id).last_child.get()
    }
    pub(crate) fn is_element(&self, id: NodeId) -> bool {
        matches!(self.node(id).data, NodeData::Element(_))
    }
    pub(crate) fn is_text(&self, id: NodeId) -> bool {
        matches!(self.node(id).data, NodeData::Text(_))
    }
    pub(crate) fn is_comment(&self, id: NodeId) -> bool {
        matches!(self.node(id).data, NodeData::Comment(_))
    }
    pub(crate) fn tag(&self, id: NodeId) -> Option<Tag> {
        match &self.node(id).data {
            NodeData::Element(e) => Some(e.tag),
            _ => None,
        }
    }
    pub(crate) fn qual_name(&self, id: NodeId) -> Option<&QualName> {
        match &self.node(id).data {
            NodeData::Element(e) => Some(&e.name),
            _ => None,
        }
    }
    pub(crate) fn text_node(&self, id: NodeId) -> Option<&str> {
        match &self.node(id).data {
            NodeData::Text(s) => Some(s.as_ref()),
            _ => None,
        }
    }
    pub(crate) fn attrs(&self, id: NodeId) -> &[super::Attribute] {
        match &self.node(id).data {
            NodeData::Element(e) => &e.attrs,
            _ => &[],
        }
    }
    pub(crate) fn attr(&self, id: NodeId, name: AttrName) -> Option<&str> {
        match &self.node(id).data {
            NodeData::Element(e) => e.attr(name),
            _ => None,
        }
    }
    pub(crate) fn attr_by_local_name(&self, id: NodeId, name: &str) -> Option<&str> {
        match &self.node(id).data {
            NodeData::Element(e) => e.attr_local(name),
            _ => None,
        }
    }
    pub(crate) fn has_attr(&self, id: NodeId, name: AttrName) -> bool {
        self.attr(id, name).is_some()
    }
}
