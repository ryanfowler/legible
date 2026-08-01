use super::{Dom, DomError, NodeData, NodeId};
use html5ever::QualName;
use html5ever::serialize::{self, Serialize, Serializer, TraversalScope};
use smallvec::SmallVec;
use std::io;
struct Serializable<'a> {
    dom: &'a Dom,
    node: NodeId,
}
enum Op {
    Open(NodeId),
    Close(QualName),
}
impl Serialize for Serializable<'_> {
    fn serialize<S>(&self, ser: &mut S, scope: TraversalScope) -> io::Result<()>
    where
        S: Serializer,
    {
        let mut ops = SmallVec::<[Op; 16]>::new();
        match scope {
            TraversalScope::IncludeNode => ops.push(Op::Open(self.node)),
            TraversalScope::ChildrenOnly(_) => {
                for id in self.dom.children_rev(self.node) {
                    ops.push(Op::Open(id));
                }
            }
        }
        while let Some(op) = ops.pop() {
            match op {
                Op::Open(id) => match &self.dom.node(id).data {
                    NodeData::Element(e) => {
                        ser.start_elem(
                            e.name.clone(),
                            e.attrs.iter().map(|a| (&a.name, a.value.as_ref())),
                        )?;
                        ops.push(Op::Close(e.name.clone()));
                        if let Some(t) = e.template_contents.get() {
                            ops.push(Op::Open(t))
                        }
                        for child in self.dom.children_rev(id) {
                            ops.push(Op::Open(child))
                        }
                    }
                    NodeData::Document | NodeData::Fragment => {
                        for child in self.dom.children_rev(id) {
                            ops.push(Op::Open(child))
                        }
                    }
                    NodeData::Doctype { name, .. } => ser.write_doctype(name)?,
                    NodeData::Text(t) => ser.write_text(t)?,
                    NodeData::Comment(t) => ser.write_comment(t)?,
                    NodeData::ProcessingInstruction { target, contents } => {
                        ser.write_processing_instruction(target, contents)?
                    }
                },
                Op::Close(name) => ser.end_elem(name)?,
            }
        }
        Ok(())
    }
}
impl Dom {
    pub(crate) fn serialize_node(&self, node: NodeId, out: &mut String) -> Result<(), DomError> {
        let mut bytes = Vec::new();
        serialize::serialize(
            &mut bytes,
            &Serializable { dom: self, node },
            serialize::SerializeOpts {
                traversal_scope: TraversalScope::IncludeNode,
                ..Default::default()
            },
        )
        .map_err(|e| DomError(e.to_string()))?;
        *out = String::from_utf8(bytes).map_err(|e| DomError(e.to_string()))?;
        Ok(())
    }
    pub(crate) fn serialize_children(
        &self,
        node: NodeId,
        out: &mut String,
    ) -> Result<(), DomError> {
        let mut bytes = Vec::new();
        serialize::serialize(
            &mut bytes,
            &Serializable { dom: self, node },
            serialize::SerializeOpts {
                traversal_scope: TraversalScope::ChildrenOnly(None),
                ..Default::default()
            },
        )
        .map_err(|e| DomError(e.to_string()))?;
        *out = String::from_utf8(bytes).map_err(|e| DomError(e.to_string()))?;
        Ok(())
    }
    pub(crate) fn html(&self, node: NodeId) -> Result<String, DomError> {
        let mut s = String::new();
        self.serialize_node(node, &mut s)?;
        Ok(s)
    }
    pub(crate) fn inner_html(&self, node: NodeId) -> Result<String, DomError> {
        let mut s = String::new();
        self.serialize_children(node, &mut s)?;
        Ok(s)
    }
}
