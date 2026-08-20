use super::{AttrName, Dom, NodeId, Tag};
use html5ever::ns;
use smallvec::SmallVec;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct DocumentAnchors {
    pub(crate) root: NodeId,
    pub(crate) html: Option<NodeId>,
    pub(crate) body: Option<NodeId>,
    pub(crate) first_base_with_href: Option<NodeId>,
}

impl DocumentAnchors {
    pub(crate) fn new(root: NodeId) -> Self {
        Self {
            root,
            html: None,
            body: None,
            first_base_with_href: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn valid_for(&self, dom: &Dom) -> bool {
        let attached = |node: NodeId| {
            node == dom.root() || dom.ancestors(node).any(|ancestor| ancestor == dom.root())
        };
        self.root == dom.root()
            && self
                .html
                .is_none_or(|node| attached(node) && dom.tag(node) == Some(Tag::Html))
            && self
                .body
                .is_none_or(|node| attached(node) && dom.tag(node) == Some(Tag::Body))
            && self.first_base_with_href.is_none_or(|node| {
                attached(node)
                    && is_html_base(dom, node)
                    && dom.attr(node, AttrName::Href).is_some()
            })
    }
}

fn is_html_base(dom: &Dom, node: NodeId) -> bool {
    dom.tag(node) == Some(Tag::Other)
        && dom
            .qual_name(node)
            .is_some_and(|name| name.ns == ns!(html) && name.local.as_ref() == "base")
}

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

    pub(crate) fn record_document_anchor(&self, node: NodeId, anchors: &mut DocumentAnchors) {
        if anchors.html.is_none() && self.tag(node) == Some(Tag::Html) {
            anchors.html = Some(node);
        }
        if anchors.body.is_none() && self.tag(node) == Some(Tag::Body) {
            anchors.body = Some(node);
        }
        if anchors.first_base_with_href.is_none()
            && is_html_base(self, node)
            && self.attr(node, AttrName::Href).is_some()
        {
            anchors.first_base_with_href = Some(node);
        }
    }

    /// Finds the document-level handles used by immutable source phases.
    ///
    /// The handles are valid only while the corresponding tree remains
    /// attached and its relevant nodes are not renamed or detached.
    pub(crate) fn document_anchors(&self) -> DocumentAnchors {
        crate::instrumentation::record_source_full_scan();
        let root = self.root();
        let mut anchors = DocumentAnchors::new(root);
        for node in std::iter::once(root).chain(self.descendants(root)) {
            self.record_document_anchor(node, &mut anchors);
        }
        anchors
    }

    /// Records attached element descendants and their depths in DOM preorder.
    ///
    /// Depth is relative to `id`, so its direct children have depth 1. The
    /// returned snapshot lets mutation passes skip a removed subtree without
    /// walking the current ancestor chain of each descendant.
    pub(crate) fn element_descendants_snapshot_with_depth(&self, id: NodeId) -> Vec<(NodeId, u32)> {
        crate::instrumentation::record_source_full_scan();
        crate::instrumentation::record_source_element_snapshot();
        // Documents usually alternate elements and text nodes. Start near the
        // expected element count and grow once for markup-only documents.
        let mut out = Vec::with_capacity((self.len() / 2).max(16));
        let Some(first_child) = self.first_child(id) else {
            return out;
        };
        let mut pending = SmallVec::<[(NodeId, u32); 16]>::new();
        pending.push((first_child, 1));
        while let Some((node, depth)) = pending.pop() {
            if self.is_element(node) {
                out.push((node, depth));
            }
            // Keep one continuation per active depth. This stays bounded by
            // nesting depth instead of buffering every sibling of a wide node.
            if let Some(sibling) = self.next_sibling(node) {
                pending.push((sibling, depth));
            }
            if let Some(child) = self.first_child(node) {
                pending.push((child, depth + 1));
            }
        }
        out
    }

    /// Records the attached descendants in DOM preorder.
    ///
    /// The returned IDs do not borrow the DOM, so callers can mutate the tree
    /// without changing the order or adding newly created nodes to the pass.
    #[cfg(test)]
    pub(crate) fn descendants_snapshot(&self, id: NodeId) -> Vec<NodeId> {
        crate::instrumentation::record_source_full_scan();
        self.descendants(id).collect()
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
    pub(crate) fn append_text(&self, root: NodeId, out: &mut String) {
        if let Some(t) = self.text_node(root) {
            out.push_str(t);
            return;
        }
        if let Some(child) = self.first_child(root)
            && self.next_sibling(child).is_none()
            && let Some(text) = self.text_node(child)
        {
            out.push_str(text);
            return;
        }
        for id in self.descendants(root) {
            if let Some(t) = self.text_node(id) {
                out.push_str(t)
            }
        }
    }
    pub(crate) fn append_text_limited(&self, root: NodeId, out: &mut String, limit: usize) {
        if limit == 0 {
            return;
        }
        if let Some(child) = self.first_child(root)
            && self.next_sibling(child).is_none()
            && let Some(text) = self.text_node(child)
        {
            append_text_chunk_limited(text, out, limit);
            return;
        }
        let mut remaining = limit;
        for id in std::iter::once(root).chain(self.descendants(root)) {
            let Some(text) = self.text_node(id) else {
                continue;
            };
            if remaining == 0 {
                break;
            }
            let taken = append_text_chunk_limited(text, out, remaining);
            remaining -= taken;
        }
    }
    pub(crate) fn text(&self, root: NodeId) -> String {
        let mut s = String::new();
        self.append_text(root, &mut s);
        s
    }
    pub(crate) fn append_normalized_text(&self, root: NodeId, out: &mut String) {
        if let Some(text) = self.text_node(root) {
            if text.is_ascii() {
                append_normalized_ascii_text(text, out);
            } else {
                append_normalized_text_chunk(text, out);
            }
            return;
        }
        if let Some(child) = self.first_child(root)
            && self.next_sibling(child).is_none()
            && let Some(text) = self.text_node(child)
            && text.is_ascii()
        {
            append_normalized_ascii_text(text, out);
            return;
        }
        let mut pending_whitespace = false;
        for id in std::iter::once(root).chain(self.descendants(root)) {
            let Some(text) = self.text_node(id) else {
                continue;
            };
            if text.is_ascii() {
                let bytes = text.as_bytes();
                let mut index = 0;
                while index < bytes.len() {
                    let start = index;
                    while index < bytes.len() && !bytes[index].is_ascii_whitespace() {
                        index += 1;
                    }
                    if start != index {
                        if pending_whitespace {
                            out.push(' ');
                            pending_whitespace = false;
                        }
                        out.push_str(&text[start..index]);
                    }
                    while index < bytes.len() && bytes[index].is_ascii_whitespace() {
                        pending_whitespace |= !out.is_empty();
                        index += 1;
                    }
                }
            } else {
                for c in text.chars() {
                    if c.is_whitespace() {
                        pending_whitespace |= !out.is_empty();
                    } else {
                        if pending_whitespace {
                            out.push(' ');
                            pending_whitespace = false;
                        }
                        out.push(c);
                    }
                }
            }
        }
    }
    pub(crate) fn append_normalized_text_limited(
        &self,
        root: NodeId,
        out: &mut String,
        limit: usize,
    ) {
        if limit != usize::MAX
            && let Some(child) = self.first_child(root)
            && self.next_sibling(child).is_none()
            && let Some(text) = self.text_node(child)
            && text.len() <= limit
            && text.is_ascii()
        {
            append_normalized_text_chunk(text, out);
            return;
        }
        let mut pending_whitespace = false;
        let mut remaining = limit;
        for id in std::iter::once(root).chain(self.descendants(root)) {
            let Some(text) = self.text_node(id) else {
                continue;
            };
            if text.is_ascii() {
                let bytes = text.as_bytes();
                let mut index = 0;
                while index < bytes.len() {
                    let start = index;
                    while index < bytes.len() && !bytes[index].is_ascii_whitespace() {
                        index += 1;
                    }
                    if start != index {
                        if pending_whitespace {
                            if remaining == 0 {
                                return;
                            }
                            out.push(' ');
                            remaining -= 1;
                            pending_whitespace = false;
                        }
                        let take = (index - start).min(remaining);
                        out.push_str(&text[start..start + take]);
                        remaining -= take;
                        if take != index - start {
                            return;
                        }
                    }
                    while index < bytes.len() && bytes[index].is_ascii_whitespace() {
                        pending_whitespace |= !out.is_empty();
                        index += 1;
                    }
                    if remaining == 0 {
                        return;
                    }
                }
            } else {
                for c in text.chars() {
                    if remaining == 0 {
                        return;
                    }
                    if c.is_whitespace() {
                        pending_whitespace |= !out.is_empty();
                    } else {
                        if pending_whitespace {
                            if remaining == 0 {
                                return;
                            }
                            out.push(' ');
                            remaining -= 1;
                            pending_whitespace = false;
                        }
                        if remaining == 0 {
                            return;
                        }
                        out.push(c);
                        remaining -= 1;
                        if remaining == 0 {
                            return;
                        }
                    }
                }
            }
        }
    }

    /// Returns descendants up to, but not inside, nested tables.
    ///
    /// The nested table node itself remains in the result. This lets callers
    /// preserve it as content while assigning each table's inner nodes to one
    /// table walk.
    pub(crate) fn table_descendants(&self, root: NodeId) -> Vec<NodeId> {
        let mut nodes = Vec::new();
        let mut pending: Vec<NodeId> = self.children_rev(root).collect();
        while let Some(node) = pending.pop() {
            nodes.push(node);
            if self.tag(node) != Some(super::Tag::Table) {
                pending.extend(self.children_rev(node));
            }
        }
        nodes
    }

    pub(crate) fn normalized_char_count(&self, root: NodeId) -> usize {
        if let Some(text) = self.text_node(root) {
            return normalized_ascii_char_count(text).unwrap_or_else(|| {
                text.split_whitespace()
                    .map(|word| word.chars().count())
                    .sum::<usize>()
                    + text.split_whitespace().count().saturating_sub(1)
            });
        }
        if let Some(child) = self.first_child(root)
            && self.next_sibling(child).is_none()
            && let Some(text) = self.text_node(child)
        {
            if let Some(count) = normalized_ascii_char_count(text) {
                return count;
            }
            let mut count = 0;
            let mut pending_whitespace = false;
            for character in text.chars() {
                if character.is_whitespace() {
                    pending_whitespace = true;
                } else {
                    count += usize::from(pending_whitespace);
                    pending_whitespace = false;
                    count += 1;
                }
            }
            return count;
        }
        let mut count = 0;
        let mut has_text = false;
        let mut pending_whitespace = false;
        for id in std::iter::once(root).chain(self.descendants(root)) {
            let Some(text) = self.text_node(id) else {
                continue;
            };
            if text.is_ascii() {
                for &byte in text.as_bytes() {
                    if byte.is_ascii_whitespace() {
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
            } else {
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
        if let Some(text) = self.text_node(root) {
            return normalized_ascii_char_count_below(text, threshold).or_else(|| {
                let count = text
                    .split_whitespace()
                    .map(|word| word.chars().count())
                    .sum::<usize>()
                    + text.split_whitespace().count().saturating_sub(1);
                (count < threshold).then_some(count)
            });
        }
        if let Some(child) = self.first_child(root)
            && self.next_sibling(child).is_none()
            && let Some(text) = self.text_node(child)
            && let Some(count) = normalized_ascii_char_count_below(text, threshold)
        {
            return Some(count);
        }
        let mut count = 0;
        let mut has_text = false;
        let mut pending_whitespace = false;
        for id in std::iter::once(root).chain(self.descendants(root)) {
            let Some(text) = self.text_node(id) else {
                continue;
            };
            if text.is_ascii() {
                for &byte in text.as_bytes() {
                    if byte.is_ascii_whitespace() {
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
            } else {
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
        }
        Some(count)
    }
    #[cfg(test)]
    pub(crate) fn normalized_text(&self, root: NodeId, initial_capacity: usize) -> (String, usize) {
        let mut out = String::with_capacity(initial_capacity);
        self.append_normalized_text(root, &mut out);
        let char_count = out.chars().count();
        (out, char_count)
    }
    pub(crate) fn has_non_whitespace_text(&self, root: NodeId) -> bool {
        fn has_text(text: &str) -> bool {
            !text.trim().is_empty()
        }
        if self.text_node(root).is_some_and(has_text) {
            return true;
        }
        self.descendants(root)
            .any(|id| self.text_node(id).is_some_and(has_text))
    }
}

fn append_text_chunk_limited(text: &str, out: &mut String, limit: usize) -> usize {
    let mut end = 0;
    let mut count = 0;
    for (index, character) in text.char_indices() {
        if count == limit {
            break;
        }
        end = index + character.len_utf8();
        count += 1;
    }
    out.push_str(&text[..end]);
    count
}

fn append_normalized_text_chunk(text: &str, out: &mut String) {
    let mut pending_whitespace = false;
    for character in text.chars() {
        if character.is_whitespace() {
            pending_whitespace |= !out.is_empty();
        } else {
            if pending_whitespace {
                out.push(' ');
                pending_whitespace = false;
            }
            out.push(character);
        }
    }
}

#[inline]
fn append_normalized_ascii_text(text: &str, out: &mut String) {
    let bytes = text.as_bytes();
    let mut index = 0;
    let mut pending_whitespace = false;
    while index < bytes.len() {
        let start = index;
        while index < bytes.len() && !bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        if start != index {
            if pending_whitespace {
                out.push(' ');
                pending_whitespace = false;
            }
            out.push_str(&text[start..index]);
        }
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            pending_whitespace |= !out.is_empty();
            index += 1;
        }
    }
}

#[inline]
fn normalized_ascii_char_count(text: &str) -> Option<usize> {
    if !text.is_ascii() {
        return None;
    }
    let mut count = 0;
    let mut has_text = false;
    let mut pending_whitespace = false;
    for &byte in text.as_bytes() {
        if byte.is_ascii_whitespace() {
            pending_whitespace |= has_text;
        } else {
            count += usize::from(pending_whitespace);
            pending_whitespace = false;
            count += 1;
            has_text = true;
        }
    }
    Some(count)
}

#[inline]
fn normalized_ascii_char_count_below(text: &str, threshold: usize) -> Option<usize> {
    if !text.is_ascii() {
        return None;
    }
    let mut count = 0;
    let mut has_text = false;
    let mut pending_whitespace = false;
    for &byte in text.as_bytes() {
        if byte.is_ascii_whitespace() {
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
    Some(count)
}
