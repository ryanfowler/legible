use super::{Dom, NodeId};

#[cfg(test)]
pub(crate) struct NodeIds {
    next: usize,
    end: usize,
}
#[cfg(test)]
impl Iterator for NodeIds {
    type Item = NodeId;
    fn next(&mut self) -> Option<NodeId> {
        if self.next == self.end {
            return None;
        }
        let id = NodeId(self.next as u32);
        self.next += 1;
        Some(id)
    }
}
pub(crate) struct Children<'a> {
    dom: &'a Dom,
    next: Option<NodeId>,
}
impl Iterator for Children<'_> {
    type Item = NodeId;
    fn next(&mut self) -> Option<NodeId> {
        let id = self.next?;
        self.next = self.dom.next_sibling(id);
        Some(id)
    }
}
pub(crate) struct ChildrenRev<'a> {
    dom: &'a Dom,
    next: Option<NodeId>,
}
impl Iterator for ChildrenRev<'_> {
    type Item = NodeId;
    fn next(&mut self) -> Option<NodeId> {
        let id = self.next?;
        self.next = self.dom.prev_sibling(id);
        Some(id)
    }
}
pub(crate) struct ElementChildren<'a> {
    inner: Children<'a>,
    dom: &'a Dom,
}
impl Iterator for ElementChildren<'_> {
    type Item = NodeId;
    fn next(&mut self) -> Option<NodeId> {
        self.inner.by_ref().find(|id| self.dom.is_element(*id))
    }
}
pub(crate) struct Descendants<'a> {
    dom: &'a Dom,
    root: NodeId,
    next: Option<NodeId>,
}
impl Iterator for Descendants<'_> {
    type Item = NodeId;
    fn next(&mut self) -> Option<NodeId> {
        let id = self.next?;
        self.next = next_preorder(self.dom, self.root, id);
        Some(id)
    }
}
fn next_preorder(dom: &Dom, root: NodeId, id: NodeId) -> Option<NodeId> {
    if let Some(c) = dom.first_child(id) {
        return Some(c);
    }
    let mut cur = id;
    loop {
        if let Some(s) = dom.next_sibling(cur) {
            return Some(s);
        }
        let p = dom.parent(cur)?;
        if p == root {
            return None;
        }
        cur = p;
    }
}
pub(crate) struct Ancestors<'a> {
    dom: &'a Dom,
    next: Option<NodeId>,
}
impl Iterator for Ancestors<'_> {
    type Item = NodeId;
    fn next(&mut self) -> Option<NodeId> {
        let id = self.next?;
        self.next = self.dom.parent(id);
        Some(id)
    }
}
impl Dom {
    /// Iterates the nodes that exist when this method is called in arena order.
    ///
    /// Arena order is allocation order, not DOM preorder. The iterator does not
    /// borrow the DOM, so callers can mutate the arena while newly created nodes
    /// remain excluded from the iteration.
    #[cfg(test)]
    pub(crate) fn node_ids(&self) -> NodeIds {
        NodeIds {
            next: 0,
            end: self.len(),
        }
    }
    pub(crate) fn children(&self, id: NodeId) -> Children<'_> {
        Children {
            dom: self,
            next: self.first_child(id),
        }
    }
    pub(crate) fn children_rev(&self, id: NodeId) -> ChildrenRev<'_> {
        ChildrenRev {
            dom: self,
            next: self.last_child(id),
        }
    }
    pub(crate) fn element_children(&self, id: NodeId) -> ElementChildren<'_> {
        ElementChildren {
            inner: self.children(id),
            dom: self,
        }
    }
    pub(crate) fn descendants(&self, id: NodeId) -> Descendants<'_> {
        Descendants {
            dom: self,
            root: id,
            next: self.first_child(id),
        }
    }
    /// Records the attached descendants in DOM preorder.
    ///
    /// The returned IDs do not borrow the DOM, so callers can mutate the tree
    /// without changing the order or adding newly created nodes to the pass.
    #[cfg(test)]
    pub(crate) fn descendants_snapshot(&self, id: NodeId) -> Vec<NodeId> {
        self.descendants(id).collect()
    }
    /// Records attached element descendants and their depths in DOM preorder.
    ///
    /// Depth is relative to `id`, so its direct children have depth 1. The
    /// returned snapshot lets mutation passes skip a removed subtree without
    /// walking the current ancestor chain of each descendant.
    pub(crate) fn element_descendants_snapshot_with_depth(&self, id: NodeId) -> Vec<(NodeId, u32)> {
        let mut out = Vec::new();
        let Some(mut current) = self.first_child(id) else {
            return out;
        };
        let mut depth = 1;

        'preorder: loop {
            if self.is_element(current) {
                out.push((current, depth));
            }
            if let Some(child) = self.first_child(current) {
                current = child;
                depth += 1;
                continue;
            }
            loop {
                if let Some(sibling) = self.next_sibling(current) {
                    current = sibling;
                    break;
                }
                let Some(parent) = self.parent(current) else {
                    break 'preorder;
                };
                if parent == id {
                    break 'preorder;
                }
                current = parent;
                depth -= 1;
            }
        }
        out
    }
    pub(crate) fn ancestors(&self, id: NodeId) -> Ancestors<'_> {
        Ancestors {
            dom: self,
            next: self.parent(id),
        }
    }
    pub(crate) fn body(&self) -> Option<NodeId> {
        self.descendants(self.root)
            .find(|&id| self.tag(id) == Some(super::Tag::Body))
    }
    pub(crate) fn html_element(&self) -> Option<NodeId> {
        self.descendants(self.root)
            .find(|&id| self.tag(id) == Some(super::Tag::Html))
    }
    pub(crate) fn append_text(&self, root: NodeId, out: &mut String) {
        if let Some(t) = self.text_node(root) {
            out.push_str(t);
            return;
        }
        for id in self.descendants(root) {
            if let Some(t) = self.text_node(id) {
                out.push_str(t)
            }
        }
    }
    pub(crate) fn text(&self, root: NodeId) -> String {
        let mut s = String::new();
        self.append_text(root, &mut s);
        s
    }
    pub(crate) fn normalized_char_count(&self, root: NodeId) -> usize {
        let mut count = 0;
        let mut has_text = false;
        let mut pending_whitespace = false;
        for id in std::iter::once(root).chain(self.descendants(root)) {
            let Some(text) = self.text_node(id) else {
                continue;
            };
            for c in text.chars() {
                if c.is_whitespace() {
                    pending_whitespace |= has_text;
                } else {
                    if pending_whitespace {
                        count += 1;
                        pending_whitespace = false;
                    }
                    count += 1;
                    has_text = true;
                }
            }
        }
        count
    }
    /// Returns the exact normalized length when it is below `threshold`.
    /// Stops as soon as the content reaches the threshold.
    pub(crate) fn normalized_char_count_below(
        &self,
        root: NodeId,
        threshold: usize,
    ) -> Option<usize> {
        if threshold == 0 {
            return None;
        }
        let mut count = 0;
        let mut has_text = false;
        let mut pending_whitespace = false;
        for id in std::iter::once(root).chain(self.descendants(root)) {
            let Some(text) = self.text_node(id) else {
                continue;
            };
            for c in text.chars() {
                if c.is_whitespace() {
                    pending_whitespace |= has_text;
                    continue;
                }
                if pending_whitespace {
                    count += 1;
                    if count >= threshold {
                        return None;
                    }
                    pending_whitespace = false;
                }
                count += 1;
                if count >= threshold {
                    return None;
                }
                has_text = true;
            }
        }
        Some(count)
    }
    #[cfg(test)]
    pub(crate) fn normalized_text(&self, root: NodeId, initial_capacity: usize) -> (String, usize) {
        let mut out = String::with_capacity(initial_capacity);
        let mut char_count = 0;
        let mut pending_whitespace = false;
        for id in std::iter::once(root).chain(self.descendants(root)) {
            let Some(text) = self.text_node(id) else {
                continue;
            };
            for c in text.chars() {
                if c.is_whitespace() {
                    pending_whitespace |= !out.is_empty();
                } else {
                    if pending_whitespace {
                        out.push(' ');
                        char_count += 1;
                        pending_whitespace = false;
                    }
                    out.push(c);
                    char_count += 1;
                }
            }
        }
        (out, char_count)
    }
    pub(crate) fn has_non_whitespace_text(&self, root: NodeId) -> bool {
        if self
            .text_node(root)
            .is_some_and(|s| s.chars().any(|c| !c.is_whitespace()))
        {
            return true;
        }
        self.descendants(root).any(|id| {
            self.text_node(id)
                .is_some_and(|s| s.chars().any(|c| !c.is_whitespace()))
        })
    }
    pub(crate) fn collect_descendants_by_tag(
        &self,
        root: NodeId,
        tag: super::Tag,
        out: &mut Vec<NodeId>,
    ) {
        out.clear();
        out.extend(
            self.descendants(root)
                .filter(|&id| self.tag(id) == Some(tag)),
        )
    }
}

pub(crate) fn build_match_string(dom: &Dom, node: NodeId, buf: &mut String) {
    buf.clear();
    if let Some(v) = dom.attr(node, super::AttrName::Class) {
        buf.push_str(v)
    }
    buf.push(' ');
    if let Some(v) = dom.attr(node, super::AttrName::Id) {
        buf.push_str(v)
    }
}
