#[cfg(test)]
use super::DomError;
use super::{Dom, NodeData, NodeId};
#[cfg(test)]
use html5ever::QualName;
#[cfg(test)]
use html5ever::serialize::{self, Serialize, Serializer, TraversalScope};
#[cfg(test)]
use smallvec::SmallVec;
#[cfg(test)]
use std::io;

/// Serializes the children of `root` as an HTML fragment.
pub(crate) fn render_html(dom: &Dom, root: NodeId, capacity: usize) -> String {
    FastHtmlSerializer::new(dom, capacity).serialize_children(root)
}

struct FastHtmlSerializer<'a> {
    dom: &'a Dom,
    out: String,
    ops: Vec<FastOp>,
}

#[derive(Clone, Copy)]
enum FastOp {
    Open(NodeId, bool),
    Close(NodeId),
}

impl<'a> FastHtmlSerializer<'a> {
    fn new(dom: &'a Dom, capacity: usize) -> Self {
        Self {
            dom,
            out: String::with_capacity(capacity),
            ops: Vec::with_capacity(32),
        }
    }

    fn serialize_children(mut self, root: NodeId) -> String {
        self.ops.extend(
            self.dom
                .children_rev(root)
                .map(|id| FastOp::Open(id, false)),
        );
        while let Some(op) = self.ops.pop() {
            match op {
                FastOp::Open(id, raw_text) => self.open(id, raw_text),
                FastOp::Close(id) => self.close(id),
            }
        }
        self.out
    }

    fn open(&mut self, id: NodeId, raw_text: bool) {
        match &self.dom.node(id).data {
            NodeData::Element(element) => {
                let name = element.name.local.as_ref();
                self.out.push('<');
                self.out.push_str(name);
                for attr in &element.attrs {
                    self.out.push(' ');
                    let namespace = attr.name.ns.as_ref();
                    match namespace {
                        "" => {}
                        "http://www.w3.org/XML/1998/namespace" => self.out.push_str("xml:"),
                        "http://www.w3.org/2000/xmlns/" if attr.name.local.as_ref() != "xmlns" => {
                            self.out.push_str("xmlns:")
                        }
                        "http://www.w3.org/2000/xmlns/" => {}
                        "http://www.w3.org/1999/xlink" => self.out.push_str("xlink:"),
                        _ => self.out.push_str("unknown_namespace:"),
                    }
                    self.out.push_str(attr.name.local.as_ref());
                    self.out.push_str("=\"");
                    push_escaped(&mut self.out, attr.value.as_ref(), true);
                    self.out.push('"');
                }
                self.out.push('>');

                let is_html = element.name.ns.as_ref() == "http://www.w3.org/1999/xhtml";
                if is_html && is_void_html_element(name) {
                    return;
                }
                let child_raw_text = is_html && is_raw_text_element(name);
                self.ops.push(FastOp::Close(id));
                if let Some(template) = element.template_contents.get() {
                    self.ops.push(FastOp::Open(template, child_raw_text));
                }
                self.ops.extend(
                    self.dom
                        .children_rev(id)
                        .map(|child| FastOp::Open(child, child_raw_text)),
                );
            }
            NodeData::Document | NodeData::Fragment => {
                self.ops.extend(
                    self.dom
                        .children_rev(id)
                        .map(|child| FastOp::Open(child, raw_text)),
                );
            }
            NodeData::Doctype { name, .. } => {
                self.out.push_str("<!DOCTYPE ");
                self.out.push_str(name);
                self.out.push('>');
            }
            NodeData::Text(text) => {
                if raw_text {
                    self.out.push_str(text);
                } else {
                    push_escaped(&mut self.out, text, false);
                }
            }
            NodeData::Comment(text) => {
                self.out.push_str("<!--");
                self.out.push_str(text);
                self.out.push_str("-->");
            }
            NodeData::ProcessingInstruction { target, contents } => {
                self.out.push_str("<?");
                self.out.push_str(target);
                self.out.push(' ');
                self.out.push_str(contents);
                self.out.push('>');
            }
        }
    }

    fn close(&mut self, id: NodeId) {
        let NodeData::Element(element) = &self.dom.node(id).data else {
            return;
        };
        self.out.push_str("</");
        self.out.push_str(element.name.local.as_ref());
        self.out.push('>');
    }
}

fn push_escaped(out: &mut String, value: &str, attr_mode: bool) {
    let bytes = value.as_bytes();
    let mut copied = 0;
    let mut index = 0;
    while index < bytes.len() {
        let replacement = match bytes[index] {
            b'&' => Some("&amp;"),
            b'"' if attr_mode => Some("&quot;"),
            b'<' => Some("&lt;"),
            b'>' => Some("&gt;"),
            0xC2 if bytes.get(index + 1) == Some(&0xA0) => {
                index += 1;
                Some("&nbsp;")
            }
            _ => None,
        };
        if let Some(replacement) = replacement {
            let special_start = if bytes[index] == 0xA0 {
                index - 1
            } else {
                index
            };
            out.push_str(&value[copied..special_start]);
            out.push_str(replacement);
            copied = index + 1;
        }
        index += 1;
    }
    out.push_str(&value[copied..]);
}

fn is_void_html_element(name: &str) -> bool {
    matches!(
        name,
        "area"
            | "base"
            | "basefont"
            | "bgsound"
            | "br"
            | "col"
            | "embed"
            | "frame"
            | "hr"
            | "img"
            | "input"
            | "keygen"
            | "link"
            | "meta"
            | "param"
            | "source"
            | "track"
            | "wbr"
    )
}

fn is_raw_text_element(name: &str) -> bool {
    matches!(
        name,
        "style" | "script" | "xmp" | "iframe" | "noembed" | "noframes" | "plaintext" | "noscript"
    )
}
#[cfg(test)]
struct Serializable<'a> {
    dom: &'a Dom,
    node: NodeId,
}
#[cfg(test)]
enum Op {
    Open(NodeId),
    Close(QualName),
}
#[cfg(test)]
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
    #[cfg(test)]
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
    #[cfg(test)]
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
    #[cfg(test)]
    pub(crate) fn html(&self, node: NodeId) -> Result<String, DomError> {
        let mut s = String::new();
        self.serialize_node(node, &mut s)?;
        Ok(s)
    }
    #[cfg(test)]
    pub(crate) fn inner_html(&self, node: NodeId) -> Result<String, DomError> {
        let mut s = String::new();
        self.serialize_children(node, &mut s)?;
        Ok(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dom::Tag;

    fn assert_matches_html5ever(html: &str) {
        let dom = Dom::parse_fragment(html, Tag::Div).unwrap();
        let root = dom.root();
        let actual = render_html(&dom, root, 0);
        let mut expected = String::new();
        dom.serialize_children(root, &mut expected).unwrap();
        assert_eq!(actual, expected, "source fragment: {html}");
    }

    #[test]
    fn final_renderer_matches_html5ever_for_escaping_and_node_types() {
        for html in [
            "text &amp; &lt; &gt; \u{00a0}<a title='a &quot; b &amp; c \u{00a0}'>link</a>",
            "before<!-- comment -->after",
            "<p>before<br>after<img src='x&amp;y'><hr></p>",
            "<template><p>template &amp; text</p><!-- nested --></template>",
        ] {
            assert_matches_html5ever(html);
        }
    }

    #[test]
    fn final_renderer_matches_html5ever_for_raw_text_and_namespaces() {
        for html in [
            "<script>if (a < b && c > d) x = '&';</script>",
            "<style>a > b { content: '&'; }</style>",
            "<noscript>raw &amp; text</noscript>",
            r##"<svg viewBox="0 0 1 1"><a xlink:href="#target"><text>&amp;</text></a></svg>"##,
            r#"<math><mi definitionURL="x&amp;y">a</mi></math>"#,
        ] {
            assert_matches_html5ever(html);
        }
    }
}
