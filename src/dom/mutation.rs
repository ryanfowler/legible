#![allow(clippy::collapsible_if)]

use super::{AttrName, Dom, DomError, ElementData, NodeData, NodeId, NodeLink, Tag};
use html5ever::{LocalName, QualName, ns};
use tendril::StrTendril;
impl Dom {
    fn ensure_no_cycle(&self, parent: NodeId, child: NodeId) {
        assert!(parent != child, "DOM cycle");

        // A leaf cannot contain the destination. This is the common case while
        // html5ever builds a document: it appends each new element before the
        // element gets children. Avoid adding a DOM ancestry scan to
        // html5ever's depth-sensitive parsing work.
        if self.first_child(child).is_none() {
            return;
        }

        assert!(!self.ancestors(parent).any(|p| p == child), "DOM cycle");
    }
    pub(crate) fn detach(&mut self, node: NodeId) {
        let p = self.parent(node);
        let prev = self.prev_sibling(node);
        let next = self.next_sibling(node);
        if let Some(p) = p {
            if self.first_child(p) == Some(node) {
                self.node_mut(p).first_child = NodeLink::from_option(next)
            }
            if self.last_child(p) == Some(node) {
                self.node_mut(p).last_child = NodeLink::from_option(prev)
            }
        }
        if let Some(x) = prev {
            self.node_mut(x).next_sibling = NodeLink::from_option(next)
        }
        if let Some(x) = next {
            self.node_mut(x).prev_sibling = NodeLink::from_option(prev)
        }
        let n = self.node_mut(node);
        n.parent = NodeLink::NONE;
        n.prev_sibling = NodeLink::NONE;
        n.next_sibling = NodeLink::NONE;
    }
    pub(crate) fn append_child(&mut self, parent: NodeId, child: NodeId) {
        self.ensure_no_cycle(parent, child);
        if self.parent(child).is_some() {
            self.detach(child)
        }
        let last = self.last_child(parent);
        {
            let n = self.node_mut(child);
            n.parent = NodeLink::from_option(Some(parent));
            n.prev_sibling = NodeLink::from_option(last);
            n.next_sibling = NodeLink::NONE
        }
        if let Some(last) = last {
            self.node_mut(last).next_sibling = NodeLink::from_option(Some(child))
        } else {
            self.node_mut(parent).first_child = NodeLink::from_option(Some(child))
        }
        self.node_mut(parent).last_child = NodeLink::from_option(Some(child));
    }
    #[allow(dead_code)]
    pub(crate) fn prepend_child(&mut self, parent: NodeId, child: NodeId) {
        self.ensure_no_cycle(parent, child);
        if self.parent(child).is_some() {
            self.detach(child)
        }
        let first = self.first_child(parent);
        {
            let n = self.node_mut(child);
            n.parent = NodeLink::from_option(Some(parent));
            n.prev_sibling = NodeLink::NONE;
            n.next_sibling = NodeLink::from_option(first)
        }
        if let Some(first) = first {
            self.node_mut(first).prev_sibling = NodeLink::from_option(Some(child))
        } else {
            self.node_mut(parent).last_child = NodeLink::from_option(Some(child))
        }
        self.node_mut(parent).first_child = NodeLink::from_option(Some(child));
    }
    pub(crate) fn insert_before(&mut self, reference: NodeId, node: NodeId) {
        let parent = self.parent(reference).expect("reference is detached");
        self.ensure_no_cycle(parent, node);
        if node == reference {
            return;
        }
        if self.parent(node).is_some() {
            self.detach(node)
        }
        let prev = self.prev_sibling(reference);
        {
            let n = self.node_mut(node);
            n.parent = NodeLink::from_option(Some(parent));
            n.prev_sibling = NodeLink::from_option(prev);
            n.next_sibling = NodeLink::from_option(Some(reference))
        }
        self.node_mut(reference).prev_sibling = NodeLink::from_option(Some(node));
        if let Some(prev) = prev {
            self.node_mut(prev).next_sibling = NodeLink::from_option(Some(node))
        } else {
            self.node_mut(parent).first_child = NodeLink::from_option(Some(node))
        }
    }
    pub(crate) fn insert_after(&mut self, reference: NodeId, node: NodeId) {
        let parent = self.parent(reference).expect("reference is detached");
        self.ensure_no_cycle(parent, node);
        if node == reference {
            return;
        }
        if self.parent(node).is_some() {
            self.detach(node)
        }
        let next = self.next_sibling(reference);
        {
            let n = self.node_mut(node);
            n.parent = NodeLink::from_option(Some(parent));
            n.prev_sibling = NodeLink::from_option(Some(reference));
            n.next_sibling = NodeLink::from_option(next)
        }
        self.node_mut(reference).next_sibling = NodeLink::from_option(Some(node));
        if let Some(next) = next {
            self.node_mut(next).prev_sibling = NodeLink::from_option(Some(node))
        } else {
            self.node_mut(parent).last_child = NodeLink::from_option(Some(node))
        }
    }
    pub(crate) fn replace_with(&mut self, target: NodeId, replacement: NodeId) {
        if target == replacement {
            return;
        }
        let parent = self.parent(target).expect("target is detached");
        if self.parent(replacement).is_some() {
            self.detach(replacement)
        }
        self.insert_before(target, replacement);
        self.detach(target);
        let _ = parent;
    }
    pub(crate) fn move_children(&mut self, from: NodeId, to: NodeId) {
        let ids: Vec<_> = self.children(from).collect();
        for id in ids {
            self.append_child(to, id)
        }
    }
    pub(crate) fn detach_children(&mut self, parent: NodeId) -> Vec<NodeId> {
        let ids = self.children(parent).collect();
        for &id in &ids {
            self.detach(id)
        }
        ids
    }
    pub(crate) fn rename_html(&mut self, node: NodeId, tag: Tag) {
        if let NodeData::Element(e) = &mut self.node_mut(node).data {
            e.tag = tag;
            e.name = QualName::new(None, ns!(html), LocalName::from(tag.as_lowercase_str()))
        }
    }
    pub(crate) fn set_attr(&mut self, node: NodeId, name: AttrName, value: &str) {
        if let NodeData::Element(e) = &mut self.node_mut(node).data {
            if let Some(a) = e.attrs.iter_mut().find(|a| a.known == name) {
                a.value = StrTendril::from(value)
            } else {
                e.attrs.push(super::Attribute {
                    name: QualName::new(None, ns!(), LocalName::from(name.as_str())),
                    known: name,
                    value: StrTendril::from(value),
                })
            }
        }
    }
    pub(crate) fn set_attr_qual(&mut self, node: NodeId, name: QualName, value: StrTendril) {
        if let NodeData::Element(e) = &mut self.node_mut(node).data {
            if let Some(a) = e.attrs.iter_mut().find(|a| a.name == name) {
                a.value = value
            } else {
                e.attrs.push(super::Attribute {
                    known: AttrName::from_local(name.local.as_ref()),
                    name,
                    value,
                })
            }
        }
    }
    pub(crate) fn remove_attr(&mut self, node: NodeId, name: AttrName) {
        if let NodeData::Element(e) = &mut self.node_mut(node).data {
            e.attrs.retain(|a| a.known != name)
        }
    }
    pub(crate) fn remove_attrs(&mut self, node: NodeId, names: &[AttrName]) {
        if let NodeData::Element(e) = &mut self.node_mut(node).data {
            e.attrs.retain(|a| !names.contains(&a.known))
        }
    }
    #[allow(dead_code)]
    pub(crate) fn retain_attrs_by_local_name(&mut self, node: NodeId, names: &[&str]) {
        if let NodeData::Element(e) = &mut self.node_mut(node).data {
            e.attrs.retain(|a| {
                names
                    .iter()
                    .any(|n| a.name.local.as_ref().eq_ignore_ascii_case(n))
            })
        }
    }
    pub(crate) fn set_inner_html(&mut self, node: NodeId, html: &str) -> Result<(), DomError> {
        let source = Dom::parse_fragment(html, self.tag(node).unwrap_or(Tag::Div))?;
        self.detach_children(node);
        let roots: Vec<_> = source.children(source.root()).collect();
        for id in roots {
            let imported = self.import_subtree(&source, id)?;
            self.append_child(node, imported)
        }
        Ok(())
    }
    pub(crate) fn insert_html_after(
        &mut self,
        node: NodeId,
        html: &str,
    ) -> Result<Vec<NodeId>, DomError> {
        let source = Dom::parse_fragment(
            html,
            self.tag(self.parent(node).unwrap_or(self.root))
                .unwrap_or(Tag::Div),
        )?;
        let roots: Vec<_> = source.children(source.root()).collect();
        let mut out = Vec::new();
        let mut at = node;
        for id in roots {
            let x = self.import_subtree(&source, id)?;
            self.insert_after(at, x);
            at = x;
            out.push(x)
        }
        Ok(out)
    }
    pub(crate) fn import_subtree(
        &mut self,
        source: &Dom,
        source_root: NodeId,
    ) -> Result<NodeId, DomError> {
        fn copy_data(source: &Dom, id: NodeId) -> NodeData {
            match &source.node(id).data {
                NodeData::Element(e) => NodeData::Element(ElementData {
                    name: e.name.clone(),
                    tag: e.tag,
                    attrs: e.attrs.clone(),
                    template_contents: NodeLink::NONE,
                    mathml_annotation_xml_integration_point: e
                        .mathml_annotation_xml_integration_point,
                }),
                data => data.clone(),
            }
        }

        let root = self.create(copy_data(source, source_root))?;
        let mut work = vec![(source_root, root)];
        while let Some((source_id, dest_id)) = work.pop() {
            if let NodeData::Element(element) = &source.node(source_id).data
                && let Some(template) = element.template_contents.get()
            {
                let template_copy = self.create(copy_data(source, template))?;
                if let NodeData::Element(destination) = &mut self.node_mut(dest_id).data {
                    destination.template_contents = NodeLink::from_option(Some(template_copy));
                }
                work.push((template, template_copy));
            }
            for child in source.children(source_id) {
                let child_copy = self.create(copy_data(source, child))?;
                self.append_child(dest_id, child_copy);
                work.push((child, child_copy));
            }
        }
        Ok(root)
    }
    #[cfg(test)]
    pub(crate) fn validate(&self) -> Result<(), DomError> {
        for (i, n) in self.nodes.iter().enumerate() {
            let id = NodeId(i as u32);
            if let Some(p) = n.parent.get() {
                if !self.contains(p) {
                    return Err(DomError("invalid parent".into()));
                }
                if n.prev_sibling.get().is_none() && self.first_child(p) != Some(id) {
                    return Err(DomError("first child link".into()));
                }
            }
            if let Some(c) = n.first_child.get() {
                if !self.contains(c) || self.parent(c) != Some(id) || self.prev_sibling(c).is_some()
                {
                    return Err(DomError("first child invariant".into()));
                }
            }
            if let Some(c) = n.last_child.get() {
                if !self.contains(c) || self.parent(c) != Some(id) || self.next_sibling(c).is_some()
                {
                    return Err(DomError("last child invariant".into()));
                }
            }
            let mut seen = std::collections::HashSet::new();
            let mut cur = n.first_child.get();
            while let Some(c) = cur {
                if !seen.insert(c) {
                    return Err(DomError("duplicate child".into()));
                }
                if self.parent(c) != Some(id) {
                    return Err(DomError("child parent link".into()));
                }
                cur = self.next_sibling(c)
            }
        }
        Ok(())
    }
}
