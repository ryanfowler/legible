use super::{
    AttrName, Attribute, DomError, ElementData, ElementName, Node, NodeData, NodeId, NodeLink, Tag,
};
use html5ever::{LocalName, QualName, ns};
use tendril::StrTendril;

#[cfg_attr(not(feature = "bench-instrumentation"), derive(Clone))]
pub(crate) struct Dom {
    pub(crate) nodes: Vec<Node>,
    element_names: Vec<Option<QualName>>,
    attribute_names: Vec<Option<QualName>>,
    element_name_free: Vec<usize>,
    attribute_name_free: Vec<usize>,
    pub(crate) root: NodeId,
    pub(crate) quirks_mode: html5ever::tree_builder::QuirksMode,
}

#[cfg(feature = "bench-instrumentation")]
impl Clone for Dom {
    fn clone(&self) -> Self {
        #[cfg(feature = "bench-instrumentation")]
        {
            let bytes = self.nodes.capacity() * std::mem::size_of::<Node>()
                + self
                    .nodes
                    .iter()
                    .filter_map(|node| match &node.data {
                        NodeData::Element(element) => {
                            Some(element.attrs.capacity() * std::mem::size_of::<Attribute>())
                        }
                        _ => None,
                    })
                    .sum::<usize>();
            let bytes = bytes
                + self.element_names.capacity() * std::mem::size_of::<QualName>()
                + self.attribute_names.capacity() * std::mem::size_of::<QualName>()
                + self.element_name_free.capacity() * std::mem::size_of::<usize>()
                + self.attribute_name_free.capacity() * std::mem::size_of::<usize>();
            crate::instrumentation::record_dom_clone(bytes);
        }
        Self {
            nodes: self.nodes.clone(),
            element_names: self.element_names.clone(),
            attribute_names: self.attribute_names.clone(),
            element_name_free: self.element_name_free.clone(),
            attribute_name_free: self.attribute_name_free.clone(),
            root: self.root,
            quirks_mode: self.quirks_mode,
        }
    }
}

impl Dom {
    pub(crate) fn with_capacity(data: NodeData, capacity: usize) -> Self {
        let mut nodes = Vec::with_capacity(capacity.max(1));
        nodes.push(Node::new(data));
        Self {
            nodes,
            element_names: Vec::new(),
            attribute_names: Vec::new(),
            element_name_free: Vec::new(),
            attribute_name_free: Vec::new(),
            root: NodeId(0),
            quirks_mode: html5ever::tree_builder::NoQuirks,
        }
    }
    pub(crate) fn len(&self) -> usize {
        self.nodes.len()
    }
    #[cfg(test)]
    pub(crate) fn auxiliary_name_storage_bytes(&self) -> usize {
        self.element_names.capacity() * std::mem::size_of::<Option<QualName>>()
            + self.attribute_names.capacity() * std::mem::size_of::<Option<QualName>>()
            + (self.element_name_free.capacity() + self.attribute_name_free.capacity())
                * std::mem::size_of::<usize>()
    }
    /// Reserves space for a known small set of synthetic nodes.
    ///
    /// Use an exact reservation here. A geometric growth at the end of a
    /// large parsed arena can retain almost one full unused copy of the node
    /// storage for only a few new wrappers.
    pub(crate) fn reserve_additional_nodes_exact(&mut self, additional: usize) {
        self.nodes.reserve_exact(additional);
    }
    pub(crate) fn root(&self) -> NodeId {
        self.root
    }
    #[cfg(any(test, feature = "fuzzing"))]
    #[allow(dead_code)] // Used by the standalone DOM mutation fuzz target.
    pub(crate) fn contains(&self, id: NodeId) -> bool {
        id.index() < self.nodes.len()
    }
    #[inline]
    pub(crate) fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id.index()]
    }
    #[inline]
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
        let tag = Tag::from_qual_name(&name);
        let compact_name = self.compact_element_name(&name, tag);
        self.create(NodeData::Element(ElementData {
            name: compact_name,
            local: name.local.clone(),
            tag,
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

    pub(crate) fn compact_element_name(&mut self, name: &QualName, tag: Tag) -> ElementName {
        if name.ns == ns!(html)
            && name.prefix.is_none()
            && tag != Tag::Other
            && name.local.as_ref() == tag.as_lowercase_str()
        {
            return ElementName::known(tag);
        }
        let index = self
            .element_name_free
            .pop()
            .unwrap_or(self.element_names.len());
        if index == self.element_names.len() {
            self.element_names.push(Some(name.clone()));
        } else {
            self.element_names[index] = Some(name.clone());
        }
        ElementName::foreign(index)
    }

    pub(crate) fn compact_attribute(&mut self, attribute: html5ever::Attribute) -> Attribute {
        Attribute::new(
            attribute.name,
            attribute.value,
            &mut self.attribute_names,
            &mut self.attribute_name_free,
        )
    }

    pub(crate) fn compact_attribute_parts(
        &mut self,
        name: QualName,
        value: StrTendril,
    ) -> Attribute {
        Attribute::new(
            name,
            value,
            &mut self.attribute_names,
            &mut self.attribute_name_free,
        )
    }

    pub(crate) fn clone_element_name_from(&mut self, source: &Dom, id: NodeId) -> ElementName {
        let NodeData::Element(element) = &source.node(id).data else {
            unreachable!("element name requested for a non-element")
        };
        match element.name.foreign_index() {
            Some(index) => {
                let name = source.element_names[index]
                    .as_ref()
                    .expect("live qualified element name")
                    .clone();
                self.compact_element_name(&name, element.tag)
            }
            None => ElementName::known(element.tag),
        }
    }

    pub(crate) fn clone_attribute_from(
        &mut self,
        source: &Dom,
        attribute: &Attribute,
    ) -> Attribute {
        let value = attribute.value.clone();
        match attribute.qualified_name_index() {
            Some(index) => self.compact_attribute_parts(
                source.attribute_names[index]
                    .as_ref()
                    .expect("live qualified attribute name")
                    .clone(),
                value,
            ),
            None => Attribute::known_with_local(
                attribute.known_kind(),
                attribute.local_name().into(),
                value,
            ),
        }
    }

    pub(crate) fn release_element_name(&mut self, name: ElementName) {
        if let Some(index) = name.foreign_index()
            && self.element_names[index].take().is_some()
        {
            self.element_name_free.push(index);
        }
    }

    pub(crate) fn release_attribute_name_index(&mut self, index: usize) {
        if self.attribute_names[index].take().is_some() {
            self.attribute_name_free.push(index);
        }
    }
    pub(crate) fn create_text(&mut self, text: &str) -> Result<NodeId, DomError> {
        self.create(NodeData::Text(StrTendril::from(text)))
    }
    #[inline]
    pub(crate) fn parent(&self, id: NodeId) -> Option<NodeId> {
        self.node(id).parent.get()
    }
    #[inline]
    pub(crate) fn prev_sibling(&self, id: NodeId) -> Option<NodeId> {
        self.node(id).prev_sibling.get()
    }
    #[inline]
    pub(crate) fn next_sibling(&self, id: NodeId) -> Option<NodeId> {
        self.node(id).next_sibling.get()
    }
    #[inline]
    pub(crate) fn first_child(&self, id: NodeId) -> Option<NodeId> {
        self.node(id).first_child.get()
    }
    #[inline]
    pub(crate) fn last_child(&self, id: NodeId) -> Option<NodeId> {
        self.node(id).last_child.get()
    }
    #[inline]
    pub(crate) fn is_element(&self, id: NodeId) -> bool {
        matches!(self.node(id).data, NodeData::Element(_))
    }
    #[inline]
    pub(crate) fn is_text(&self, id: NodeId) -> bool {
        matches!(self.node(id).data, NodeData::Text(_))
    }
    #[inline]
    pub(crate) fn is_comment(&self, id: NodeId) -> bool {
        matches!(self.node(id).data, NodeData::Comment(_))
    }
    #[inline]
    pub(crate) fn tag(&self, id: NodeId) -> Option<Tag> {
        match &self.node(id).data {
            NodeData::Element(e) => Some(e.tag),
            _ => None,
        }
    }
    pub(crate) fn qual_name(&self, id: NodeId) -> Option<QualName> {
        match &self.node(id).data {
            NodeData::Element(e) => Some(match e.name.foreign_index() {
                Some(index) => self.element_names[index]
                    .as_ref()
                    .expect("live qualified element name")
                    .clone(),
                None => QualName::new(None, ns!(html), LocalName::from(e.tag.as_lowercase_str())),
            }),
            _ => None,
        }
    }
    pub(crate) fn local_name(&self, id: NodeId) -> Option<&str> {
        match &self.node(id).data {
            NodeData::Element(e) => Some(e.local.as_ref()),
            _ => None,
        }
    }
    pub(crate) fn is_namespace(&self, id: NodeId, namespace: &html5ever::Namespace) -> bool {
        match &self.node(id).data {
            NodeData::Element(element) => match element.name.foreign_index() {
                Some(index) => {
                    self.element_names[index]
                        .as_ref()
                        .expect("live qualified element name")
                        .ns
                        == *namespace
                }
                None => *namespace == ns!(html),
            },
            _ => false,
        }
    }
    #[inline]
    pub(crate) fn text_node(&self, id: NodeId) -> Option<&str> {
        match &self.node(id).data {
            NodeData::Text(s) => Some(s.as_ref()),
            _ => None,
        }
    }
    #[inline]
    pub(crate) fn attrs(&self, id: NodeId) -> &[super::Attribute] {
        match &self.node(id).data {
            NodeData::Element(e) => &e.attrs,
            _ => &[],
        }
    }
    #[inline]
    pub(crate) fn attr(&self, id: NodeId, name: AttrName) -> Option<&str> {
        match &self.node(id).data {
            NodeData::Element(e) => e.attr(name),
            _ => None,
        }
    }
    #[inline]
    pub(crate) fn attr_by_local_name(&self, id: NodeId, name: &str) -> Option<&str> {
        match &self.node(id).data {
            NodeData::Element(e) => e.attr_local(name),
            _ => None,
        }
    }
    #[inline]
    pub(crate) fn has_attr(&self, id: NodeId, name: AttrName) -> bool {
        self.attr(id, name).is_some()
    }

    #[inline]
    pub(crate) fn attribute_local_name<'a>(&'a self, attribute: &'a Attribute) -> &'a str {
        attribute.local_name()
    }

    #[inline]
    pub(crate) fn attribute_qual_name(&self, attribute: &Attribute) -> QualName {
        attribute.qualified_name(&self.attribute_names)
    }

    #[inline]
    pub(crate) fn attribute_matches(&self, attribute: &Attribute, name: &QualName) -> bool {
        match attribute.qualified_name_index() {
            Some(index) => {
                self.attribute_names[index]
                    .as_ref()
                    .expect("live qualified attribute name")
                    == name
            }
            None => {
                name.ns == ns!()
                    && name.prefix.is_none()
                    && attribute.local_name() == name.local.as_ref()
            }
        }
    }
}
