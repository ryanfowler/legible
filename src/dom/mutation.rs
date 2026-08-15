#![allow(clippy::collapsible_if)]

use super::{AttrName, Dom, DomError, ElementData, NodeData, NodeId, NodeLink, Tag};
use html5ever::{LocalName, QualName, ns};
use smallvec::SmallVec;
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
        if from == to {
            return;
        }
        while let Some(id) = self.first_child(from) {
            self.append_child(to, id)
        }
    }
    pub(crate) fn set_text(&mut self, node: NodeId, text: &str) {
        if let NodeData::Text(value) = &mut self.node_mut(node).data {
            *value = StrTendril::from(text);
        }
    }
    pub(crate) fn rename_html(&mut self, node: NodeId, tag: Tag) {
        if let NodeData::Element(e) = &mut self.node_mut(node).data {
            e.tag = tag;
            e.name = QualName::new(None, ns!(html), LocalName::from(tag.as_lowercase_str()))
        }
    }
    pub(crate) fn set_attr(&mut self, node: NodeId, name: AttrName, value: &str) {
        if let NodeData::Element(e) = &mut self.node_mut(node).data {
            if let Some(a) = e
                .attrs
                .iter_mut()
                .find(|attribute| name.matches_local(attribute.name.local.as_ref()))
            {
                a.value = StrTendril::from(value)
            } else {
                e.attrs.push(super::Attribute {
                    name: QualName::new(None, ns!(), LocalName::from(name.as_str())),
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
                e.attrs.push(super::Attribute { name, value })
            }
        }
    }
    pub(crate) fn remove_attr(&mut self, node: NodeId, name: AttrName) {
        if let NodeData::Element(e) = &mut self.node_mut(node).data {
            e.attrs
                .retain(|attribute| !name.matches_local(attribute.name.local.as_ref()))
        }
    }
    pub(crate) fn remove_attrs(&mut self, node: NodeId, names: &[AttrName]) {
        if let NodeData::Element(e) = &mut self.node_mut(node).data {
            e.attrs.retain(|attribute| {
                !names.contains(&AttrName::from_local(attribute.name.local.as_ref()))
            })
        }
    }
    #[cfg(test)]
    pub(crate) fn set_inner_html(&mut self, node: NodeId, html: &str) -> Result<(), DomError> {
        let source = Dom::parse_fragment(html, self.tag(node).unwrap_or(Tag::Div))?;
        while let Some(child) = self.first_child(node) {
            self.detach(child)
        }
        let roots: SmallVec<[NodeId; 4]> = source.children(source.root()).collect();
        for id in roots {
            let imported = self.import_subtree(&source, id)?;
            self.append_child(node, imported)
        }
        Ok(())
    }
    pub(crate) fn copy_subtree_as_fragment(&self, source_root: NodeId) -> Result<Dom, DomError> {
        // Reserve the attached subtree in one allocation. Template contents can
        // add a small number of extra nodes, but ordinary fragments stay exact.
        let capacity = 2 + self.descendants(source_root).count();
        let mut fragment = Dom::with_capacity(NodeData::Fragment, capacity);
        let copied = fragment.import_subtree(self, source_root)?;
        fragment.append_child(fragment.root(), copied);
        Ok(fragment)
    }
    pub(crate) fn copy_children_as_fragment(&self, source_root: NodeId) -> Result<Dom, DomError> {
        let capacity = 1 + self.descendants(source_root).count();
        let mut fragment = Dom::with_capacity(NodeData::Fragment, capacity);
        for child in self.children(source_root) {
            let copied = fragment.import_subtree(self, child)?;
            fragment.append_child(fragment.root(), copied);
        }
        Ok(fragment)
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
        let mut work = SmallVec::<[(NodeId, NodeId); 16]>::new();
        work.push((source_root, root));
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
    #[cfg(any(test, feature = "fuzzing"))]
    #[allow(dead_code)] // Used by the standalone DOM mutation fuzz target.
    pub(crate) fn validate(&self) -> Result<(), DomError> {
        if self.parent(self.root).is_some() {
            return Err(DomError("root has a parent".into()));
        }
        let mut parent_state = vec![0_u8; self.nodes.len()];
        for start in 0..self.nodes.len() {
            if parent_state[start] != 0 {
                continue;
            }
            let mut path = Vec::new();
            let mut current = Some(NodeId(start as u32));
            while let Some(node) = current {
                if !self.contains(node) {
                    return Err(DomError("invalid parent chain".into()));
                }
                match parent_state[node.index()] {
                    0 => {
                        parent_state[node.index()] = 1;
                        path.push(node);
                        current = self.parent(node);
                    }
                    1 => return Err(DomError("parent cycle".into())),
                    _ => break,
                }
            }
            for node in path {
                parent_state[node.index()] = 2;
            }
        }

        let mut listed_as_child = vec![false; self.nodes.len()];
        for (i, n) in self.nodes.iter().enumerate() {
            let id = NodeId(i as u32);
            if let Some(p) = n.parent.get() {
                if !self.contains(p) {
                    return Err(DomError("invalid parent".into()));
                }
                if n.prev_sibling.get().is_none() && self.first_child(p) != Some(id) {
                    return Err(DomError("first child link".into()));
                }
            } else if n.prev_sibling.get().is_some() || n.next_sibling.get().is_some() {
                return Err(DomError("detached sibling link".into()));
            }
            if let Some(previous) = n.prev_sibling.get()
                && (!self.contains(previous)
                    || self.next_sibling(previous) != Some(id)
                    || self.parent(previous) != n.parent.get())
            {
                return Err(DomError("previous sibling link".into()));
            }
            if let Some(next) = n.next_sibling.get()
                && (!self.contains(next)
                    || self.prev_sibling(next) != Some(id)
                    || self.parent(next) != n.parent.get())
            {
                return Err(DomError("next sibling link".into()));
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
            let mut previous = None;
            while let Some(c) = cur {
                if !seen.insert(c) {
                    return Err(DomError("duplicate child".into()));
                }
                if listed_as_child[c.index()] {
                    return Err(DomError("child appears in multiple lists".into()));
                }
                listed_as_child[c.index()] = true;
                if self.parent(c) != Some(id) {
                    return Err(DomError("child parent link".into()));
                }
                if self.prev_sibling(c) != previous {
                    return Err(DomError("child previous link".into()));
                }
                if let Some(previous) = previous
                    && self.next_sibling(previous) != Some(c)
                {
                    return Err(DomError("child next link".into()));
                }
                previous = Some(c);
                cur = self.next_sibling(c)
            }
            if previous != n.last_child.get() {
                return Err(DomError("last child chain".into()));
            }
        }
        for (i, node) in self.nodes.iter().enumerate() {
            if node.parent.get().is_some() != listed_as_child[i] {
                return Err(DomError("parented node is not in child list".into()));
            }
        }
        Ok(())
    }
}
