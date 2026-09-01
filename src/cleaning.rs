//! DOM preparation and content cleanup.
#![allow(clippy::collapsible_if)]
use crate::constants::{
    PRESENTATIONAL_ATTRIBUTES, has_image_extension, has_image_src, has_image_srcset,
    is_deprecated_size_attribute_elem, parse_b64_data_url,
};
use crate::dom::{AttrName, Dom, NodeId, NodeStats, Tag};
use crate::page_kind::PageKind;
use crate::scoring::{
    get_inner_text, get_link_density_cached, get_normalized_inner_text, get_or_compute_stats,
    has_hidden_utility_class, has_static_hidden_marker, is_hidden_utility_class,
    is_phrasing_content,
};
use crate::tokens::{has_any_token, has_token};
use html5ever::{LocalName, QualName, ns};
use regex::Regex;
use smallvec::SmallVec;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

/// Reusable traversal and scratch storage for one selected fragment.
///
/// A snapshot is valid only for the current mutation epoch and root. Cleanup
/// phases must call `invalidate` after a structural mutation before they ask
/// for another snapshot. This keeps snapshot reuse explicit and prevents a
/// later phase from accidentally observing detached nodes.
#[derive(Default)]
pub(crate) struct FragmentWorkspace {
    mutation_epoch: u32,
    snapshot_epoch: Option<u32>,
    snapshot_root: Option<NodeId>,
    preorder: Vec<NodeId>,
    elements_with_depth: Vec<(NodeId, u32)>,
    snapshot_stack: Vec<(NodeId, u32)>,
    chrome_aggregates: Vec<ChromeAggregate>,
    chrome_aggregates_epoch: Option<u32>,
    chrome_aggregates_root: Option<NodeId>,
    scratch_u32: Vec<u32>,
    scratch_bytes: Vec<u8>,
    scratch_bits: Vec<bool>,
}

pub(crate) struct FragmentScratch {
    pub(crate) u32_values: Vec<u32>,
    pub(crate) bytes: Vec<u8>,
    pub(crate) bits: Vec<bool>,
}

impl FragmentWorkspace {
    /// Drops snapshot validity while retaining all allocated workspace storage.
    pub(crate) fn invalidate(&mut self) {
        self.mutation_epoch = self.mutation_epoch.wrapping_add(1);
        self.snapshot_epoch = None;
        self.snapshot_root = None;
        self.chrome_aggregates_epoch = None;
        self.chrome_aggregates_root = None;
    }

    /// Starts a new fragment epoch without releasing reusable buffers.
    pub(crate) fn reset(&mut self) {
        self.invalidate();
        self.preorder.clear();
        self.elements_with_depth.clear();
        self.snapshot_stack.clear();
        self.chrome_aggregates.clear();
        self.chrome_aggregates_epoch = None;
        self.chrome_aggregates_root = None;
        self.scratch_u32.clear();
        self.scratch_bytes.clear();
        self.scratch_bits.clear();
    }

    /// Returns one DOM-preorder snapshot for the current fragment version.
    pub(crate) fn ensure_snapshot(&mut self, dom: &Dom, root: NodeId) {
        if self.snapshot_epoch == Some(self.mutation_epoch) && self.snapshot_root == Some(root) {
            return;
        }

        self.preorder.clear();
        self.elements_with_depth.clear();
        self.preorder.push(root);
        let Some(first_child) = dom.first_child(root) else {
            self.snapshot_epoch = Some(self.mutation_epoch);
            self.snapshot_root = Some(root);
            return;
        };

        self.snapshot_stack.clear();
        self.snapshot_stack.push((first_child, 1));
        while let Some((node, depth)) = self.snapshot_stack.pop() {
            self.preorder.push(node);
            if dom.is_element(node) {
                self.elements_with_depth.push((node, depth));
            }
            if let Some(sibling) = dom.next_sibling(node) {
                self.snapshot_stack.push((sibling, depth));
            }
            if let Some(child) = dom.first_child(node) {
                self.snapshot_stack.push((child, depth.saturating_add(1)));
            }
        }
        self.snapshot_epoch = Some(self.mutation_epoch);
        self.snapshot_root = Some(root);
    }

    pub(crate) fn preorder(&self) -> &[NodeId] {
        &self.preorder
    }

    pub(crate) fn elements_with_depth(&self) -> &[(NodeId, u32)] {
        &self.elements_with_depth
    }

    fn ensure_chrome_aggregates(&mut self, dom: &Dom, root: NodeId) {
        self.ensure_snapshot(dom, root);
        if self.chrome_aggregates_epoch == Some(self.mutation_epoch)
            && self.chrome_aggregates_root == Some(root)
        {
            return;
        }
        self.chrome_aggregates = chrome_aggregates(dom, &self.elements_with_depth);
        self.chrome_aggregates_epoch = Some(self.mutation_epoch);
        self.chrome_aggregates_root = Some(root);
    }

    fn chrome_aggregates(&self) -> &[ChromeAggregate] {
        &self.chrome_aggregates
    }

    pub(crate) fn take_scratch(&mut self) -> FragmentScratch {
        FragmentScratch {
            u32_values: std::mem::take(&mut self.scratch_u32),
            bytes: std::mem::take(&mut self.scratch_bytes),
            bits: std::mem::take(&mut self.scratch_bits),
        }
    }

    pub(crate) fn restore_scratch(&mut self, scratch: FragmentScratch) {
        self.scratch_u32 = scratch.u32_values;
        self.scratch_bytes = scratch.bytes;
        self.scratch_bits = scratch.bits;
    }

    /// Marks nodes that a pending detach will remove from the next topology
    /// query. The mask reuses workspace storage and avoids copying a fragment
    /// snapshot just to hide nodes from a batched detector.
    pub(crate) fn with_pending_detach_mask<R>(
        &mut self,
        dom: &mut Dom,
        pending: &[NodeId],
        action: impl FnOnce(&mut Dom, &[NodeId], &[bool]) -> R,
    ) -> R {
        if pending.is_empty() {
            self.scratch_bits.clear();
            return action(dom, &self.preorder, &self.scratch_bits);
        }
        self.scratch_bits.resize(dom.len(), false);
        self.scratch_bits.fill(false);
        for &node in pending {
            if node.index() >= self.scratch_bits.len() {
                continue;
            }
            self.scratch_bits[node.index()] = true;
            for descendant in dom.descendants(node) {
                self.scratch_bits[descendant.index()] = true;
            }
        }
        action(dom, &self.preorder, &self.scratch_bits)
    }

    #[cfg(test)]
    pub(crate) fn scratch_u32(&mut self, len: usize) -> &mut [u32] {
        self.scratch_u32.resize(len, 0);
        &mut self.scratch_u32[..len]
    }

    #[cfg(test)]
    pub(crate) fn scratch_bytes(&mut self, len: usize) -> &mut [u8] {
        self.scratch_bytes.resize(len, 0);
        &mut self.scratch_bytes[..len]
    }

    #[cfg(test)]
    pub(crate) fn scratch_bits(&mut self, len: usize) -> &mut [bool] {
        self.scratch_bits.resize(len, false);
        self.scratch_bits[..len].fill(false);
        &mut self.scratch_bits[..len]
    }
}

#[cfg(test)]
pub fn prep_document(dom: &mut Dom) {
    let body = dom.body();
    prep_document_with_body(dom, body);
}

pub(crate) fn prep_document_with_body(dom: &mut Dom, body: Option<NodeId>) {
    // Preserve the required preparation order. Remove inactive subtrees,
    // normalize BR runs, and only then rename deprecated font elements.
    let mut ids: Vec<_> = dom
        .descendants(dom.root())
        .filter(|&id| {
            matches!(dom.tag(id), Some(Tag::Noscript | Tag::Style))
                || dom.tag(id) == Some(Tag::Script)
                    && !dom.attr(id, AttrName::Type).is_some_and(|value| {
                        let value = value.trim().to_ascii_lowercase();
                        value == "math/tex" || value.starts_with("math/tex;") || value == "text/tex"
                    })
        })
        .collect();
    for &id in &ids {
        dom.detach(id);
    }

    ids.clear();
    if let Some(body) = body {
        ids.extend(dom.descendants(body).filter(|&id| {
            dom.tag(id) == Some(Tag::Br)
                && !dom
                    .ancestors(id)
                    .any(|ancestor| matches!(dom.tag(ancestor), Some(Tag::Pre | Tag::Code)))
        }));
    }
    replace_brs(dom, &ids);

    ids.clear();
    ids.extend(
        dom.descendants(dom.root())
            .filter(|&id| dom.tag(id) == Some(Tag::Font)),
    );
    for id in ids {
        dom.rename_html(id, Tag::Span);
    }
}

pub(crate) fn next_non_whitespace_sibling(dom: &Dom, id: NodeId) -> Option<NodeId> {
    let mut n = dom.next_sibling(id);
    while let Some(x) = n {
        if dom.is_element(x) {
            return Some(x);
        }
        if dom.is_text(x) && dom.text_node(x).is_some_and(|t| !t.trim().is_empty()) {
            return None;
        }
        n = dom.next_sibling(x);
    }
    None
}
fn replace_brs(dom: &mut Dom, ids: &[NodeId]) {
    for &br in ids {
        if dom.tag(br) != Some(Tag::Br) {
            continue;
        }
        if dom.parent(br).is_none() {
            continue;
        }
        let mut next = next_non_whitespace_sibling(dom, br);
        let mut replaced = false;
        while let Some(x) = next {
            if dom.tag(x) == Some(Tag::Br) {
                replaced = true;
                next = next_non_whitespace_sibling(dom, x);
                dom.detach(x);
            } else {
                break;
            }
        }
        if !replaced {
            continue;
        }
        dom.rename_html(br, Tag::P);
        let mut n = dom.next_sibling(br);
        while let Some(x) = n {
            if dom.tag(x) == Some(Tag::Br)
                && next_non_whitespace_sibling(dom, x).is_some_and(|y| dom.tag(y) == Some(Tag::Br))
            {
                break;
            }
            if !is_phrasing_content(dom, x) {
                break;
            }
            n = dom.next_sibling(x);
            dom.append_child(br, x);
        }
        while let Some(x) = dom.last_child(br) {
            if crate::scoring::is_whitespace(dom, x) {
                dom.detach(x);
            } else {
                break;
            }
        }
        if let Some(p) = dom.parent(br)
            && dom.tag(p) == Some(Tag::P)
        {
            dom.rename_html(p, Tag::Div);
        }
    }
}
fn has_allowed_media(dom: &Dom, id: NodeId, allowed: &Regex) -> bool {
    if dom
        .attrs(id)
        .iter()
        .any(|attr| allowed.is_match(attr.value.as_ref()))
    {
        return true;
    }
    dom.tag(id) == Some(Tag::Object)
        && dom.descendants(id).any(|node| {
            dom.attrs(node)
                .iter()
                .any(|attr| allowed.is_match(attr.value.as_ref()))
                || dom
                    .text_node(node)
                    .is_some_and(|text| allowed.is_match(text))
        })
}

#[cfg(test)]
fn clean_styles(dom: &mut Dom, root: NodeId, nodes: &mut Vec<NodeId>) {
    let mut semantic_gate = crate::document::SemanticGate::default();
    clean_styles_with_semantic_gate(dom, root, nodes, &mut semantic_gate);
}

/// Removes presentational source attributes while collecting the broad
/// semantic gate during the same traversal. The bit collection is
/// allocation-free; sparse candidate lists allocate only when evidence exists.
#[cfg(test)]
pub(crate) fn clean_styles_with_semantic_gate(
    dom: &mut Dom,
    root: NodeId,
    nodes: &mut Vec<NodeId>,
    semantic_gate: &mut crate::document::SemanticGate,
) {
    let mut workspace = FragmentWorkspace::default();
    clean_styles_with_semantic_gate_in_workspace(dom, root, nodes, semantic_gate, &mut workspace);
}

pub(crate) fn clean_styles_with_semantic_gate_in_workspace(
    dom: &mut Dom,
    root: NodeId,
    nodes: &mut Vec<NodeId>,
    semantic_gate: &mut crate::document::SemanticGate,
    workspace: &mut FragmentWorkspace,
) {
    nodes.clear();
    workspace.ensure_snapshot(dom, root);
    nodes.push(root);
    nodes.extend(
        workspace
            .elements_with_depth()
            .iter()
            .map(|&(node, _)| node),
    );
    for &id in nodes.iter() {
        semantic_gate.observe(dom, id);
        if !dom.is_element(id) || dom.tag(id) == Some(Tag::Svg) {
            continue;
        }
        let has_size = dom.has_attr(id, AttrName::Width) || dom.has_attr(id, AttrName::Height);
        dom.remove_attrs(id, PRESENTATIONAL_ATTRIBUTES);
        if has_size && dom.tag(id).is_some_and(is_deprecated_size_attribute_elem) {
            dom.remove_attrs(id, &[AttrName::Width, AttrName::Height]);
        }
    }
}
#[inline]
fn is_directly_protected(
    dom: &Dom,
    id: NodeId,
    evidence: &crate::document::SourceEvidence,
) -> bool {
    let tag = dom.tag(id);
    if evidence.is_semantic_source(id) {
        return true;
    }
    if matches!(
        tag,
        Some(
            Tag::Pre
                | Tag::Code
                | Tag::Figure
                | Tag::Picture
                | Tag::Blockquote
                | Tag::Details
                | Tag::Math
                | Tag::Dl
        )
    ) {
        return true;
    }
    if tag == Some(Tag::Table) && evidence.data_table(id) {
        return true;
    }
    dom.attrs(id).iter().any(|attribute| {
        attribute.is_named(AttrName::DataFootnote)
            || attribute.is_named(AttrName::DataFootnotes)
            || attribute.is_named(AttrName::DataMath)
    })
}

fn is_protected_content(dom: &Dom, id: NodeId, evidence: &crate::document::SourceEvidence) -> bool {
    std::iter::once(id)
        .chain(dom.ancestors(id))
        .any(|node| is_directly_protected(dom, node, evidence))
}

fn protected_masks<'a>(
    dom: &Dom,
    root: NodeId,
    evidence: &crate::document::SourceEvidence,
    snapshot: &[(NodeId, u32)],
    scratch_u32: &'a mut Vec<u32>,
) -> (&'a [u32], &'a [u32]) {
    let node_count = dom.len();
    scratch_u32.resize(node_count.saturating_mul(3), 0);
    scratch_u32.fill(0);
    let (directly_protected, remaining) = scratch_u32.split_at_mut(node_count);
    let (contains_protected, protected_path) = remaining.split_at_mut(node_count);
    directly_protected[root.index()] = u32::from(is_directly_protected(dom, root, evidence));
    for &(node, _) in snapshot {
        directly_protected[node.index()] =
            u32::from(is_directly_protected(dom, node, evidence) || evidence.accessible_math(node));
    }
    for &(node, _) in snapshot.iter().rev() {
        let mut value = directly_protected[node.index()] != 0;
        for child in dom.element_children(node) {
            value |= contains_protected[child.index()] != 0;
        }
        contains_protected[node.index()] = u32::from(value);
    }

    // Most callers need to know whether the candidate itself or one of its
    // ancestors is protected. Cache that path state too. Without this index,
    // deeply repaired markup makes the same ancestor walk once per candidate.
    let root_protected = directly_protected[root.index()];
    for &(node, _) in snapshot.iter() {
        let parent_protected = dom
            .parent(node)
            .filter(|&parent| parent != root)
            .map_or(root_protected != 0, |parent| {
                protected_path[parent.index()] != 0
            });
        protected_path[node.index()] =
            u32::from(parent_protected || directly_protected[node.index()] != 0);
    }
    (contains_protected, protected_path)
}

fn has_protected_ancestor(
    dom: &Dom,
    id: NodeId,
    root: NodeId,
    evidence: &crate::document::SourceEvidence,
) -> bool {
    dom.ancestors(id)
        .take_while(|&ancestor| ancestor != root)
        .any(|ancestor| is_protected_content(dom, ancestor, evidence))
}

#[derive(Clone, Copy, Default)]
struct TableEvidence {
    has_data_structure: bool,
    has_nested_table: bool,
    rows: u32,
    cols: u32,
}

fn finish_table_evidence(
    open: &mut Vec<(u32, NodeId, TableEvidence)>,
    summaries: &mut Vec<(NodeId, TableEvidence)>,
) {
    let Some((_, table, evidence)) = open.pop() else {
        return;
    };
    summaries.push((table, evidence));
    if let Some((_, _, parent)) = open.last_mut() {
        parent.has_nested_table = true;
        parent.has_data_structure |= evidence.has_data_structure;
        parent.rows = parent.rows.saturating_add(evidence.rows);
        parent.cols = parent.cols.max(evidence.cols);
    }
}

#[cfg(test)]
pub fn mark_data_tables(
    dom: &Dom,
    root: NodeId,
    store: &mut crate::dom::NodeStateStore,
    nodes: &mut Vec<NodeId>,
) {
    let mut workspace = FragmentWorkspace::default();
    mark_data_tables_in_workspace(dom, root, store, nodes, &mut workspace);
}

pub(crate) fn mark_data_tables_in_workspace(
    dom: &Dom,
    root: NodeId,
    store: &mut crate::dom::NodeStateStore,
    nodes: &mut Vec<NodeId>,
    workspace: &mut FragmentWorkspace,
) {
    workspace.ensure_snapshot(dom, root);
    mark_data_tables_from_snapshot(dom, root, workspace.elements_with_depth(), store, nodes);
}

pub(crate) fn mark_data_tables_from_snapshot(
    dom: &Dom,
    root: NodeId,
    snapshot: &[(NodeId, u32)],
    store: &mut crate::dom::NodeStateStore,
    nodes: &mut Vec<NodeId>,
) {
    nodes.clear();
    if dom.tag(root) == Some(Tag::Table) {
        nodes.push(root);
    }
    nodes.extend(
        snapshot
            .iter()
            .map(|&(node, _)| node)
            .filter(|&node| dom.tag(node) == Some(Tag::Table)),
    );

    // Aggregate each table's evidence while one preorder pass visits the
    // document. Closing a nested table merges its summary into its parent, so
    // a deeply nested chain does not rescan each inner subtree.
    let mut summaries = Vec::with_capacity(nodes.len());
    let mut open = Vec::new();
    let source_nodes = std::iter::once((root, 0)).chain(snapshot.iter().copied());
    for (node, depth) in source_nodes {
        while open
            .last()
            .is_some_and(|(table_depth, _, _)| *table_depth >= depth)
        {
            finish_table_evidence(&mut open, &mut summaries);
        }
        if dom.tag(node) == Some(Tag::Table) {
            if let Some((_, _, parent)) = open.last_mut() {
                parent.has_nested_table = true;
            }
            open.push((
                depth,
                node,
                TableEvidence {
                    has_data_structure: dom.has_attr(node, AttrName::Summary),
                    ..TableEvidence::default()
                },
            ));
            continue;
        }
        let Some((_, _, evidence)) = open.last_mut() else {
            continue;
        };
        match dom.tag(node) {
            Some(Tag::Caption) if dom.children(node).next().is_some() => {
                evidence.has_data_structure = true
            }
            Some(Tag::Col | Tag::Colgroup | Tag::Tfoot | Tag::Thead | Tag::Th) => {
                evidence.has_data_structure = true
            }
            Some(Tag::Tr) => {
                evidence.rows = evidence.rows.saturating_add(
                    dom.attr(node, AttrName::RowSpan)
                        .and_then(|value| value.parse().ok())
                        .unwrap_or(1),
                );
                let column_count = dom
                    .element_children(node)
                    .filter(|&cell| matches!(dom.tag(cell), Some(Tag::Td | Tag::Th)))
                    .map(|cell| {
                        dom.attr(cell, AttrName::ColSpan)
                            .and_then(|value| value.parse().ok())
                            .unwrap_or(1)
                    })
                    .fold(0_u32, u32::saturating_add);
                evidence.cols = evidence.cols.max(column_count);
            }
            _ => {}
        }
    }
    while !open.is_empty() {
        finish_table_evidence(&mut open, &mut summaries);
    }
    summaries.sort_unstable_by_key(|&(table, _)| table.index());

    for &id in nodes.iter() {
        if dom.attr(id, AttrName::Role) == Some("presentation")
            || dom.attr(id, AttrName::DataTable) == Some("0")
        {
            store.set_data_table(id, crate::dom::DataTableState::Layout);
            continue;
        }
        let Some((_, evidence)) = summaries
            .binary_search_by_key(&id.index(), |&(table, _)| table.index())
            .ok()
            .map(|index| summaries[index])
        else {
            continue;
        };
        if evidence.has_data_structure {
            store.set_data_table(id, crate::dom::DataTableState::Data);
        } else if evidence.has_nested_table {
            store.set_data_table(id, crate::dom::DataTableState::Layout);
        } else if is_repeated_listing_table(dom, id) {
            store.set_data_table(id, crate::dom::DataTableState::Listing);
        } else {
            store.set_data_table(
                id,
                if evidence.cols == 1
                    || evidence.rows == 1
                    || evidence.rows < 10
                        && evidence.cols <= 4
                        && evidence.rows.saturating_mul(evidence.cols) <= 10
                {
                    crate::dom::DataTableState::Layout
                } else {
                    crate::dom::DataTableState::Data
                },
            );
        }
    }
    store.finish_data_tables();
}

/// Returns true for a conservative, rank-based repeated-content table.
///
/// A rank alone is not sufficient. The table must have several similarly
/// shaped linked rows and must not have explicit data-table semantics.
pub(crate) fn is_repeated_listing_table(dom: &Dom, table: NodeId) -> bool {
    repeated_listing_start(dom, table).is_some()
}

pub(crate) use crate::document::repeated_listing_start;

#[cfg(test)]
pub fn fix_lazy_images(dom: &mut Dom, root: NodeId, nodes: &mut Vec<NodeId>) {
    nodes.clear();
    nodes.extend(
        dom.descendants(root)
            .filter(|&x| matches!(dom.tag(x), Some(Tag::Img | Tag::Picture | Tag::Figure))),
    );
    for &id in nodes.iter() {
        let mut src = false;
        let mut srcset = false;
        let mut lazy = false;
        let mut b64 = false;
        let mut other = false;
        let mut lazy_src = None;
        let mut lazy_srcset = None;
        for a in dom.attrs(id) {
            let v = a.value.as_ref();
            match AttrName::from_local(a.name.local.as_ref()) {
                AttrName::Src => {
                    src = !v.is_empty();
                    if let Some((_, media_type)) = parse_b64_data_url(v)
                        && media_type != "image/svg+xml"
                    {
                        b64 = true;
                    }
                }
                AttrName::Srcset => srcset = !v.is_empty() && v != "null",
                AttrName::DataSrc => {
                    other |= has_image_extension(v);
                    if has_image_src(v) {
                        lazy_src = Some(v.to_string())
                    }
                }
                AttrName::DataSrcset => {
                    other |= has_image_extension(v);
                    if has_image_srcset(v) {
                        lazy_srcset = Some(v.to_string())
                    }
                }
                AttrName::Class => lazy |= has_token(v, "lazy"),
                _ => {
                    other |= has_image_extension(v);
                    if has_image_srcset(v) && lazy_srcset.is_none() {
                        lazy_srcset = Some(v.to_string())
                    } else if has_image_src(v) && lazy_src.is_none() {
                        lazy_src = Some(v.to_string())
                    }
                }
            }
        }
        if b64
            && other
            && let Some(v) = dom.attr(id, AttrName::Src)
            && let Some((end, _)) = parse_b64_data_url(v)
            && v.len().saturating_sub(end) < 133
        {
            dom.remove_attr(id, AttrName::Src);
            src = false;
        }
        if (src || srcset) && !lazy {
            continue;
        }
        let (value, attr) = if let Some(v) = lazy_srcset {
            (v, AttrName::Srcset)
        } else if let Some(v) = lazy_src {
            (v, AttrName::Src)
        } else {
            continue;
        };
        match dom.tag(id) {
            Some(Tag::Img) => dom.set_attr(id, attr, &value),
            Some(Tag::Picture) => {
                let image = dom
                    .first_descendant_by_tag(id, Tag::Img)
                    .or_else(|| dom.create_html_element(Tag::Img).ok());
                if let Some(image) = image {
                    dom.set_attr(image, attr, &value);
                    if dom.parent(image).is_none() {
                        dom.append_child(id, image);
                    }
                }
            }
            Some(Tag::Figure) if !dom.any_descendant_by_tags(id, &[Tag::Img, Tag::Picture]) => {
                if let Ok(image) = dom.create_html_element(Tag::Img) {
                    dom.set_attr(image, attr, &value);
                    dom.append_child(id, image);
                }
            }
            _ => {}
        }
    }
}
fn single_image_fragment(dom: &Dom) -> Option<(NodeId, NodeId)> {
    let mut media = None;
    for node in dom.children(dom.root()) {
        if dom.is_element(node) && media.is_none() {
            media = Some(node);
        } else if !dom
            .text_node(node)
            .is_some_and(|text| text.trim().is_empty())
            && !dom.is_comment(node)
        {
            return None;
        }
    }
    let media = media?;
    let image = single_image_element(dom, media)?;
    matches!(dom.tag(media), Some(Tag::Img | Tag::Picture)).then_some((media, image))
}

fn noscript_media_root(dom: &Dom, noscript: NodeId, image: NodeId) -> NodeId {
    dom.ancestors(image)
        .take_while(|&ancestor| ancestor != noscript)
        .find(|&ancestor| dom.tag(ancestor) == Some(Tag::Picture))
        .unwrap_or(image)
}

fn useful_image(dom: &Dom, id: NodeId) -> bool {
    dom.attrs(id).iter().any(|attribute| {
        let name = attribute.name.local.as_ref();
        matches!(name, "src" | "srcset" | "data-src" | "data-srcset")
            || has_image_extension(attribute.value.as_ref())
    })
}

fn copy_image_attributes(dom: &mut Dom, from: NodeId, to: NodeId) {
    let attrs: Vec<_> = dom
        .attrs(from)
        .iter()
        .filter(|a| {
            !a.value.is_empty()
                && (matches!(
                    AttrName::from_local(a.name.local.as_ref()),
                    AttrName::Src | AttrName::Srcset
                ) || has_image_extension(a.value.as_ref()))
        })
        .map(|a| (a.name.clone(), a.value.clone()))
        .collect();
    for (mut name, value) in attrs {
        if dom.attr_by_local_name(to, name.local.as_ref()) == Some(value.as_ref()) {
            continue;
        }
        if dom.attr_by_local_name(to, name.local.as_ref()).is_some() {
            name = QualName::new(
                None,
                ns!(),
                LocalName::from(format!("data-old-{}", name.local)),
            );
        }
        dom.set_attr_qual(to, name, value);
    }
}

fn copy_missing_image_description(dom: &mut Dom, from: NodeId, to: NodeId) {
    let attrs: SmallVec<[(QualName, String); 3]> = dom
        .attrs(from)
        .iter()
        .filter(|attribute| {
            matches!(
                attribute.name.local.as_ref(),
                "alt" | "aria-label" | "title"
            ) && !attribute.value.trim().is_empty()
                && dom
                    .attr_by_local_name(to, attribute.name.local.as_ref())
                    .is_none_or(|value| value.trim().is_empty())
        })
        .map(|attribute| (attribute.name.clone(), attribute.value.to_string()))
        .collect();
    for (name, value) in attrs {
        dom.set_attr_qual(to, name, value.into());
    }
}

fn previous_element(dom: &Dom, id: NodeId) -> Option<NodeId> {
    let mut previous = dom.prev_sibling(id);
    while let Some(node) = previous {
        if dom.is_element(node) {
            return Some(node);
        }
        previous = dom.prev_sibling(node);
    }
    None
}

fn single_image_element(dom: &Dom, id: NodeId) -> Option<NodeId> {
    if dom.tag(id) == Some(Tag::Img) {
        return Some(id);
    }
    if dom.has_non_whitespace_text(id) {
        return None;
    }
    let mut images = dom
        .descendants(id)
        .filter(|&node| dom.tag(node) == Some(Tag::Img));
    let image = images.next()?;
    images.next().is_none().then_some(image)
}

fn is_tracking_image(dom: &Dom, id: NodeId) -> bool {
    [AttrName::Width, AttrName::Height]
        .into_iter()
        .filter_map(|name| dom.attr(id, name)?.parse::<u32>().ok())
        .any(|size| size <= 1)
}

fn is_placeholder_image(dom: &Dom, id: NodeId) -> bool {
    dom.attr(id, AttrName::Src).is_some_and(|src| {
        contains_ascii_case_insensitive(src, "placeholder")
            || parse_b64_data_url(src).is_some_and(|(end, _)| src.len().saturating_sub(end) < 133)
    })
}

fn images_are_variants(dom: &Dom, first: NodeId, second: NodeId) -> bool {
    let same_nonempty_attr = |name| {
        dom.attr(first, name)
            .filter(|value| !value.is_empty())
            .is_some_and(|value| dom.attr(second, name) == Some(value))
    };
    let url_attrs = [
        AttrName::Src,
        AttrName::Srcset,
        AttrName::DataSrc,
        AttrName::DataSrcset,
    ];
    let mut same_url = false;
    let mut same_basename = false;
    for first_url in url_attrs.iter().filter_map(|&name| dom.attr(first, name)) {
        for second_url in url_attrs.iter().filter_map(|&name| dom.attr(second, name)) {
            same_url |= !first_url.is_empty() && first_url == second_url;
            let first_name = first_url
                .split(['?', '#'])
                .next()
                .and_then(|value| value.rsplit('/').next())
                .filter(|value| !value.is_empty());
            let second_name = second_url
                .split(['?', '#'])
                .next()
                .and_then(|value| value.rsplit('/').next())
                .filter(|value| !value.is_empty());
            same_basename |= first_name.is_some() && first_name == second_name;
        }
    }
    let same_dimensions =
        same_nonempty_attr(AttrName::Width) && same_nonempty_attr(AttrName::Height);
    let same_alt = dom
        .attr(first, AttrName::Alt)
        .filter(|value| !value.is_empty())
        .is_some_and(|value| dom.attr(second, AttrName::Alt) == Some(value));
    same_url || same_dimensions && (same_basename || same_alt)
}

fn previous_useful_image(
    dom: &Dom,
    id: NodeId,
    fallback: NodeId,
) -> (Option<(NodeId, NodeId)>, SmallVec<[NodeId; 1]>) {
    let Some(immediate) = previous_element(dom, id) else {
        return (None, SmallVec::new());
    };
    let Some(immediate_image) = single_image_element(dom, immediate) else {
        return (None, SmallVec::new());
    };
    if useful_image(dom, immediate_image) {
        return if images_are_variants(dom, immediate_image, fallback)
            || is_placeholder_image(dom, immediate_image)
        {
            (Some((immediate, immediate_image)), SmallVec::new())
        } else {
            (None, SmallVec::new())
        };
    }

    // Some lazy-image implementations put an empty hydration image between a
    // low-resolution image and its noscript fallback. Scan past only one such
    // placeholder, and require matching image metadata before merging them.
    if let Some(previous) = previous_element(dom, immediate)
        && let Some(previous_image) = single_image_element(dom, previous)
        && useful_image(dom, previous_image)
        && images_are_variants(dom, previous_image, fallback)
    {
        let mut placeholders = SmallVec::new();
        placeholders.push(immediate);
        return (Some((previous, previous_image)), placeholders);
    }

    (Some((immediate, immediate_image)), SmallVec::new())
}

pub fn unwrap_noscript_images(dom: &mut Dom) {
    // Inspect noscript fallbacks without first deleting placeholder images.
    // Replace a placeholder only after a usable fallback image is available.
    let candidates: Vec<_> = dom
        .descendants(dom.root())
        .filter(|&id| dom.tag(id) == Some(Tag::Noscript))
        .collect();
    for id in candidates {
        if dom.parent(id).is_none() {
            continue;
        }
        let image_ids: SmallVec<[NodeId; 2]> = dom
            .descendants(id)
            .filter(|&node| dom.tag(node) == Some(Tag::Img))
            .collect();
        if image_ids.len() == 1 && !dom.has_non_whitespace_text(id) {
            let image = image_ids[0];
            if is_tracking_image(dom, image) {
                continue;
            }
            let media = noscript_media_root(dom, id, image);
            let (previous, placeholders) = previous_useful_image(dom, id, image);
            if let Some((previous, previous_image)) = previous {
                copy_image_attributes(dom, previous_image, image);
                copy_missing_image_description(dom, previous_image, image);
                dom.detach(previous);
                for placeholder in placeholders {
                    dom.detach(placeholder);
                }
            }
            dom.insert_before(id, media);
            dom.detach(id);
            continue;
        }
        if !image_ids.is_empty() {
            continue;
        }
        let mut text_nodes = dom.children(id).filter(|&node| {
            dom.is_text(node) && dom.text_node(node).is_some_and(|t| !t.trim().is_empty())
        });
        let Some(text_node) = text_nodes.next() else {
            continue;
        };
        if text_nodes.next().is_some() {
            continue;
        }
        let Some(markup) = dom.text_node(text_node) else {
            continue;
        };
        let Ok(fragment) = Dom::parse_fragment(markup, Tag::Div) else {
            continue;
        };
        let Some((source_media, source_image)) = single_image_fragment(&fragment) else {
            continue;
        };
        if is_tracking_image(&fragment, source_image) {
            continue;
        }
        let Ok(new_media) = dom.import_subtree(&fragment, source_media) else {
            continue;
        };
        let new_image = if dom.tag(new_media) == Some(Tag::Img) {
            new_media
        } else if let Some(image) = dom.first_descendant_by_tag(new_media, Tag::Img) {
            image
        } else {
            continue;
        };
        let (previous, placeholders) = previous_useful_image(dom, id, new_image);
        if let Some((previous, previous_image)) = previous {
            copy_image_attributes(dom, previous_image, new_image);
            copy_missing_image_description(dom, previous_image, new_image);
            dom.detach(previous);
            for placeholder in placeholders {
                dom.detach(placeholder);
            }
        }
        dom.insert_before(id, new_media);
        dom.detach(id);
    }
}
/// Removes content that is not useful in a retained semantic fragment.
///
/// This phase uses only high-confidence rules. It removes executable markup,
/// hidden scaffolding, tracking images, and interactive controls. It keeps the
/// text and structure around removed form controls.
fn preserve_media_from_hidden_variant(dom: &mut Dom, hidden: NodeId) {
    let adjacent_element = |forward: bool| {
        let mut sibling = if forward {
            dom.next_sibling(hidden)
        } else {
            dom.prev_sibling(hidden)
        };
        while sibling.is_some_and(|node| {
            dom.text_node(node)
                .is_some_and(|text| text.trim().is_empty())
        }) {
            sibling = sibling.and_then(|node| {
                if forward {
                    dom.next_sibling(node)
                } else {
                    dom.prev_sibling(node)
                }
            });
        }
        sibling.filter(|&node| dom.is_element(node))
    };
    let sibling = [adjacent_element(false), adjacent_element(true)]
        .into_iter()
        .flatten()
        .find(|&sibling| {
            !has_hidden_utility_class(dom, sibling)
                && dom.any_descendant_by_tags(sibling, &[Tag::Img])
        });
    let hidden_images: SmallVec<[NodeId; 4]> = dom
        .descendants(hidden)
        .filter(|&node| dom.tag(node) == Some(Tag::Img))
        .collect();
    let Some(sibling) = sibling else {
        for image in hidden_images {
            dom.insert_before(hidden, image);
        }
        return;
    };
    let visible_images: SmallVec<[NodeId; 4]> = dom
        .descendants(sibling)
        .filter(|&node| dom.tag(node) == Some(Tag::Img))
        .collect();
    let single_pair = hidden_images.len() == 1 && visible_images.len() == 1;
    for hidden_image in hidden_images {
        let target = visible_images.iter().copied().find(|&visible_image| {
            let hidden_alt = dom.attr(hidden_image, AttrName::Alt);
            hidden_alt.is_some() && hidden_alt == dom.attr(visible_image, AttrName::Alt)
        });
        let target = target.or_else(|| single_pair.then_some(visible_images[0]));
        if let Some(target) = target {
            copy_image_attributes(dom, hidden_image, target);
        } else if useful_image(dom, hidden_image) {
            dom.append_child(sibling, hidden_image);
        }
    }
}

#[cfg(test)]
pub(crate) fn hard_cleanup(
    dom: &mut Dom,
    root: NodeId,
    allowed_media: &Regex,
    relax_static_visibility: bool,
    evidence: &crate::document::SourceEvidence,
    nodes: &mut Vec<NodeId>,
) {
    let mut workspace = FragmentWorkspace::default();
    hard_cleanup_in_workspace(
        dom,
        root,
        allowed_media,
        relax_static_visibility,
        evidence,
        nodes,
        &mut workspace,
    );
}

pub(crate) fn hard_cleanup_in_workspace(
    dom: &mut Dom,
    root: NodeId,
    allowed_media: &Regex,
    relax_static_visibility: bool,
    evidence: &crate::document::SourceEvidence,
    nodes: &mut Vec<NodeId>,
    workspace: &mut FragmentWorkspace,
) {
    nodes.clear();
    workspace.ensure_snapshot(dom, root);
    nodes.extend(
        workspace
            .elements_with_depth()
            .iter()
            .map(|&(node, _)| node),
    );
    let (media_sources, _) = crate::document::media_cleanup_evidence(dom, nodes);
    for &node in nodes.iter().rev() {
        if dom.parent(node).is_none() {
            continue;
        }
        let Some(tag) = dom.tag(node) else { continue };
        let fallback_image = tag == Tag::Img
            && dom
                .attr(node, AttrName::Class)
                .is_some_and(|class| class.contains("fallback-image"));
        let accessible_skip_link = tag == Tag::A
            && dom
                .attr(node, AttrName::Href)
                .is_some_and(|href| href.starts_with('#'))
            && dom.attr(node, AttrName::Class).is_some_and(|classes| {
                has_token(classes, "skip-link")
                    || classes
                        .split_ascii_whitespace()
                        .any(|class| starts_ascii_case_insensitive(class, "skip-to-"))
            });
        let utility_visibility = has_hidden_utility_class(dom, node) && !accessible_skip_link;
        let static_visibility = has_static_hidden_marker(dom, node) || utility_visibility;
        let modal = dom.attr(node, AttrName::AriaModal) == Some("true")
            || dom
                .attr(node, AttrName::Role)
                .is_some_and(|roles| has_any_token(roles, &["dialog", "alertdialog"]))
            || static_visibility
                && dom
                    .attr(node, AttrName::Class)
                    .is_some_and(|classes| has_any_token(classes, &["modal", "dialog"]));
        let math_source = evidence.math(node) || evidence.accessible_math(node);
        let hidden =
            dom.attr(node, AttrName::AriaHidden) == Some("true") && !fallback_image && !math_source
                || !relax_static_visibility && static_visibility && !math_source
                || modal;
        if relax_static_visibility {
            dom.remove_attr(node, AttrName::Hidden);
            if let Some(classes) = dom.attr(node, AttrName::Class) {
                let retained = classes
                    .split_whitespace()
                    .filter(|class| !is_hidden_utility_class(class))
                    .collect::<SmallVec<[&str; 8]>>()
                    .join(" ");
                if retained.is_empty() {
                    dom.remove_attr(node, AttrName::Class);
                } else if retained != classes {
                    dom.set_attr(node, AttrName::Class, &retained);
                }
            }
        }
        let tracking_image = tag == Tag::Img
            && is_tracking_image(dom, node)
            && !has_lazy_image_candidate(dom, node)
            && !picture_has_lazy_source(dom, node);
        let executable = matches!(
            tag,
            Tag::Script | Tag::Style | Tag::Template | Tag::Link | Tag::Meta
        ) && !math_source;
        let content_checkbox = tag == Tag::Input
            && dom
                .attr(node, AttrName::Type)
                .is_some_and(|value| value.eq_ignore_ascii_case("checkbox"))
            && dom
                .ancestors(node)
                .find(|&ancestor| {
                    matches!(dom.tag(ancestor), Some(Tag::Form | Tag::Li))
                        || dom
                            .attr(ancestor, AttrName::Role)
                            .is_some_and(|roles| has_token(roles, "listitem"))
                })
                .is_some_and(|ancestor| {
                    dom.tag(ancestor) == Some(Tag::Li)
                        || dom
                            .attr(ancestor, AttrName::Role)
                            .is_some_and(|roles| has_token(roles, "listitem"))
                });
        if content_checkbox {
            // Keep only the semantic state. The retained control is disabled,
            // so extracted HTML cannot change the source checklist.
            dom.remove_attr(node, AttrName::Other);
            dom.remove_lookup_only_attrs(node);
            dom.remove_attrs(
                node,
                &[
                    AttrName::Class,
                    AttrName::Id,
                    AttrName::Name,
                    AttrName::Style,
                    AttrName::AriaHidden,
                ],
            );
            dom.set_attr(node, AttrName::Disabled, "");
        }
        let control = matches!(
            tag,
            Tag::Input | Tag::Textarea | Tag::Select | Tag::Button | Tag::Datalist | Tag::Option
        ) && !content_checkbox
            && !evidence.is_semantic_source(node);
        let disallowed_embed = matches!(tag, Tag::Object | Tag::Embed | Tag::Iframe)
            && !has_allowed_media(dom, node, allowed_media)
            && !media_sources[node.index()];
        if hidden || tracking_image || executable || control || disallowed_embed {
            if hidden && utility_visibility {
                preserve_media_from_hidden_variant(dom, node);
            }
            dom.detach(node);
        }
    }
    workspace.invalidate();
}

/// Removes clutter only when several independent signals agree.
#[cfg(test)]
pub(crate) fn heuristic_cleanup(
    dom: &mut Dom,
    root: NodeId,
    page_kind: PageKind,
    store: &mut crate::dom::NodeStateStore,
    evidence: &crate::document::SourceEvidence,
    text_buffer: &mut String,
    nodes: &mut Vec<NodeId>,
) {
    let mut workspace = FragmentWorkspace::default();
    heuristic_cleanup_in_workspace(
        dom,
        root,
        page_kind,
        store,
        evidence,
        text_buffer,
        nodes,
        &mut workspace,
    );
}

fn populate_heuristic_link_counts(dom: &Dom, snapshot: &[(NodeId, u32)], link_counts: &mut [u8]) {
    for &(node, _) in snapshot.iter().rev() {
        link_counts[node.index()] = u8::from(dom.tag(node) == Some(Tag::A));
        for child in dom.element_children(node) {
            link_counts[node.index()] =
                link_counts[node.index()].saturating_add(link_counts[child.index()]);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn populate_heuristic_aggregates(
    dom: &Dom,
    snapshot: &[(NodeId, u32)],
    link_counts: &mut [u8],
    has_controls: &mut [bool],
    has_images: &mut [bool],
    has_two_images: &mut [bool],
    has_forms: &mut [bool],
    has_social_links: &mut [bool],
    has_action_urls: &mut [bool],
    has_sponsored_links: &mut [bool],
) {
    for &(node, _) in snapshot.iter().rev() {
        let tag = dom.tag(node);
        link_counts[node.index()] = u8::from(tag == Some(Tag::A));
        has_controls[node.index()] = matches!(
            tag,
            Some(Tag::Input | Tag::Textarea | Tag::Select | Tag::Button)
        );
        has_images[node.index()] = tag == Some(Tag::Img);
        has_two_images[node.index()] = false;
        has_forms[node.index()] = tag == Some(Tag::Form);
        has_social_links[node.index()] = tag == Some(Tag::A)
            && dom.attr(node, AttrName::Href).is_some_and(|href| {
                ["facebook.", "twitter.", "x.com/", "linkedin.", "reddit."]
                    .iter()
                    .any(|needle| contains_ascii_case_insensitive(href, needle))
            });
        has_action_urls[node.index()] = tag == Some(Tag::A)
            && dom.attr(node, AttrName::Href).is_some_and(|href| {
                contains_ascii_case_insensitive(href, "/comments")
                    || contains_ascii_case_insensitive(href, "action=share")
                    || contains_ascii_case_insensitive(href, "/reply")
                    || contains_ascii_case_insensitive(href, "dialog=")
            });
        has_sponsored_links[node.index()] = is_sponsored_anchor(dom, node);
        let mut image_count = usize::from(has_images[node.index()]);
        for child in dom.element_children(node) {
            link_counts[node.index()] =
                link_counts[node.index()].saturating_add(link_counts[child.index()]);
            has_controls[node.index()] |= has_controls[child.index()];
            image_count = image_count.saturating_add(if has_two_images[child.index()] {
                2
            } else {
                usize::from(has_images[child.index()])
            });
            has_forms[node.index()] |= has_forms[child.index()];
            has_social_links[node.index()] |= has_social_links[child.index()];
            has_action_urls[node.index()] |= has_action_urls[child.index()];
            has_sponsored_links[node.index()] |= has_sponsored_links[child.index()];
        }
        has_images[node.index()] = image_count > 0;
        has_two_images[node.index()] = image_count >= 2;
        if tag == Some(Tag::Other)
            && dom.attr(node, AttrName::Action).is_some()
            && (dom.qual_name(node).is_some_and(|name| {
                contains_ascii_case_insensitive(name.local.as_ref(), "newsletter-form")
            }) || dom
                .attr(node, AttrName::Class)
                .is_some_and(|value| contains_ascii_case_insensitive(value, "newsletter-form"))
                || dom
                    .attr(node, AttrName::Id)
                    .is_some_and(|value| contains_ascii_case_insensitive(value, "newsletter-form")))
        {
            has_forms[node.index()] = true;
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn heuristic_cleanup_in_workspace(
    dom: &mut Dom,
    root: NodeId,
    page_kind: PageKind,
    store: &mut crate::dom::NodeStateStore,
    evidence: &crate::document::SourceEvidence,
    text_buffer: &mut String,
    nodes: &mut Vec<NodeId>,
    workspace: &mut FragmentWorkspace,
) {
    nodes.clear();
    workspace.ensure_snapshot(dom, root);
    let mut scratch = workspace.take_scratch();
    scratch.bytes.resize(dom.len(), 0);
    scratch.bytes.fill(0);
    scratch.bits.resize(dom.len().saturating_mul(9), false);
    scratch.bits.fill(false);
    let (discovered_boundaries, bits) = scratch.bits.split_at_mut(dom.len());
    let (inspected_subscription, bits) = bits.split_at_mut(dom.len());
    let (has_controls, bits) = bits.split_at_mut(dom.len());
    let (has_images, bits) = bits.split_at_mut(dom.len());
    let (has_two_images, bits) = bits.split_at_mut(dom.len());
    let (has_forms, bits) = bits.split_at_mut(dom.len());
    let (has_social_links, bits) = bits.split_at_mut(dom.len());
    let (has_action_urls, has_sponsored_links) = bits.split_at_mut(dom.len());
    let link_counts = &mut scratch.bytes[..dom.len()];
    store.clear_stats();
    store.enable_link_lengths();
    get_or_compute_stats(dom, root, store);

    // Count links once. The byte index saturates at 255. All cleanup
    // classifiers use threshold checks, and the main classifier gets a useful
    // count instead of the old per-subtree rescans.
    let mut responsive_view_count = 0_u8;
    {
        let snapshot = workspace.elements_with_depth();
        populate_heuristic_aggregates(
            dom,
            snapshot,
            link_counts,
            has_controls,
            has_images,
            has_two_images,
            has_forms,
            has_social_links,
            has_action_urls,
            has_sponsored_links,
        );
        let mut table_depths = SmallVec::<[u32; 8]>::new();
        for &(node, depth) in snapshot {
            responsive_view_count = responsive_view_count
                .saturating_add(u8::from(responsive_visibility(dom, node).is_some()));
            while table_depths
                .last()
                .is_some_and(|&table_depth| table_depth >= depth)
            {
                table_depths.pop();
            }
            let inside_table = !table_depths.is_empty();
            if dom.tag(node) == Some(Tag::Table) {
                table_depths.push(depth);
            }
            if related_heading_signal(dom, node) != RelatedHeadingSignal::None {
                mark_related_heading_boundary(
                    dom,
                    node,
                    root,
                    link_counts,
                    store,
                    discovered_boundaries,
                );
            }
            if is_compact_link_index_heading(dom, node) {
                for ancestor in dom.ancestors(node).take(3) {
                    if dom
                        .element_children(ancestor)
                        .take(16)
                        .any(|child| matches!(dom.tag(child), Some(Tag::Ol | Tag::Ul)))
                    {
                        discovered_boundaries[ancestor.index()] = true;
                        break;
                    }
                }
            }
            if dom.tag(node) == Some(Tag::Form) {
                mark_subscription_boundary(
                    dom,
                    node,
                    root,
                    store,
                    inspected_subscription,
                    discovered_boundaries,
                );
            }
            if is_structural_breadcrumb_candidate(dom, node, inside_table) {
                discovered_boundaries[node.index()] = true;
            }
            if is_structural_peripheral_candidate(dom, node, page_kind, store) {
                discovered_boundaries[node.index()] = true;
            }
        }
    }

    let changed = {
        let snapshot = workspace.elements_with_depth();
        remove_explicit_peripheral_sections(
            dom,
            root,
            snapshot,
            link_counts,
            store,
            responsive_view_count >= 2,
        )
    };
    if changed {
        workspace.invalidate();
        workspace.ensure_snapshot(dom, root);
        populate_heuristic_link_counts(dom, workspace.elements_with_depth(), link_counts);
    }
    let changed = {
        workspace.ensure_snapshot(dom, root);
        let snapshot = workspace.elements_with_depth();
        remove_terminal_taxonomy_before_footnotes(dom, root, snapshot, link_counts, store)
    };
    if changed {
        workspace.invalidate();
    }
    let changed = {
        workspace.ensure_snapshot(dom, root);
        let snapshot = workspace.elements_with_depth();
        remove_job_company_profiles(dom, root, page_kind, snapshot, store)
    };
    if changed {
        workspace.invalidate();
        workspace.ensure_snapshot(dom, root);
        populate_heuristic_link_counts(dom, workspace.elements_with_depth(), link_counts);
    }
    let changed = {
        workspace.ensure_snapshot(dom, root);
        let snapshot = workspace.elements_with_depth();
        remove_direct_peripheral_siblings(dom, root, snapshot, link_counts, store, evidence)
    };
    if changed {
        workspace.invalidate();
    }
    workspace.ensure_snapshot(dom, root);
    let snapshot = workspace.elements_with_depth();
    populate_heuristic_aggregates(
        dom,
        snapshot,
        link_counts,
        has_controls,
        has_images,
        has_two_images,
        has_forms,
        has_social_links,
        has_action_urls,
        has_sponsored_links,
    );
    let root_length = get_or_compute_stats(dom, root, store).text_length.max(1);
    let protected_masks = snapshot
        .iter()
        .any(|&(_, depth)| depth > 64)
        .then(|| protected_masks(dom, root, evidence, snapshot, &mut scratch.u32_values));
    let mut name_buffer = String::new();

    // Keep only outermost candidates. A classifier can inspect the complete
    // subtree once instead of rescanning every nested wrapper.
    let mut boundary_depth = None;
    for &(node, depth) in snapshot {
        if let Some(outer_depth) = boundary_depth {
            let nested_author_boundary =
                depth > outer_depth && is_author_contribution_boundary(dom, node);
            if depth > outer_depth
                && !discovered_boundaries[node.index()]
                && !nested_author_boundary
            {
                continue;
            }
            if depth <= outer_depth {
                boundary_depth = None;
            }
            if nested_author_boundary {
                nodes.push(node);
                boundary_depth = Some(depth);
                continue;
            }
        }
        if dom.parent(node).is_some() {
            if discovered_boundaries[node.index()] {
                nodes.push(node);
                continue;
            }
            if is_heuristic_boundary(dom, node) {
                nodes.push(node);
                boundary_depth = Some(depth);
            }
        }
    }
    for &node in nodes.iter().rev() {
        if dom.parent(node).is_none()
            || protected_masks
                .as_ref()
                .is_some_and(|(_, path)| path[node.index()] != 0)
            || protected_masks.is_none()
                && (is_protected_content(dom, node, evidence)
                    || has_protected_ancestor(dom, node, root, evidence))
        {
            continue;
        }
        let stats = get_or_compute_stats(dom, node, store);
        get_inner_text(dom, node, text_buffer);
        text_buffer.make_ascii_lowercase();
        let text = text_buffer.trim();
        append_node_name(dom, node, &mut name_buffer);
        let name = name_buffer.as_str();
        let links = usize::from(link_counts[node.index()]);
        let controls = usize::from(has_controls[node.index()]);
        let images =
            usize::from(has_images[node.index()]) + usize::from(has_two_images[node.index()]);
        let protected = protected_masks.as_ref().map_or_else(
            || {
                dom.descendants(node)
                    .any(|descendant| is_protected_content(dom, descendant, evidence))
            },
            |(subtrees, _)| subtrees[node.index()] != 0,
        );
        let link_density = get_link_density_cached(dom, node, stats.text_length, store);
        let short = stats.text_length < 350 || stats.text_length * 5 < root_length;

        // Empty boundaries cannot contain useful positional clutter evidence.
        // Skipping them also avoids long sibling scans on empty form shells.
        let (at_start, at_end) = if stats.text_length == 0 {
            (false, false)
        } else {
            (
                near_content_start(dom, node, root, store),
                near_content_end(dom, node, root, store),
            )
        };
        let has_form = has_forms[node.index()];
        let metrics = PeripheralMetrics {
            name,
            text,
            stats,
            links,
            controls,
            images,
            has_form,
            link_density,
            at_start,
            at_end,
            short,
        };
        if at_start && has_compact_content_identity(dom, node, links) {
            continue;
        }
        let related = is_related_content(dom, node, &metrics);

        let social_name = contains_any(name, &["share", "social", "sharedaddy"]);
        let social_links = usize::from(has_social_links[node.index()]);
        let social = social_name && (social_links > 0 || links >= 2) && short;

        let signup = is_newsletter_cta(&metrics);

        let breadcrumb = is_breadcrumb(dom, node, &metrics);
        let navigation_semantic = dom.tag(node) == Some(Tag::Nav)
            || dom
                .attr(node, AttrName::Role)
                .is_some_and(|role| has_token(role, "navigation"));
        let menu_name = contains_any(name, &["menu", "navigation", "breadcrumb"]);
        let navigation_label = dom
            .attr(node, AttrName::AriaLabel)
            .is_some_and(|label| contains_ascii_case_insensitive(label, "navigation"));
        let documentation_toc = dom.attr(node, AttrName::AriaLabel).is_some_and(|label| {
            equals_any_ascii_case_insensitive(
                label.trim(),
                &["on this page", "table of contents", "contents"],
            )
        }) || contains_any(
            name,
            &["table-of-contents", "table_of_contents", "docs-toc"],
        );
        let navigation = navigation_semantic
            && !documentation_toc
            && !breadcrumb
            && (menu_name || navigation_label || links >= 3)
            && link_density >= if navigation_label { 0.35 } else { 0.6 }
            && stats.text_length < 500;

        let author_name = contains_any(name, &["author-bio", "author_bio", "profile", "bio"]);
        let inside_article_toc = std::iter::once(node)
            .chain(dom.ancestors(node))
            .any(|ancestor| dom.attr(ancestor, AttrName::DataArticleToc).is_some());
        let author_card = (author_name || inside_article_toc)
            && short
            && (at_start || at_end)
            && links >= 1
            && images >= 1;
        let author = author_name && short && (social_links > 0 || links >= 2 || author_card);
        let author_metadata = contains_any(
            name,
            &[
                "author-list",
                "author_list",
                "author-roles",
                "author_roles",
                "contributors",
            ],
        ) && links >= 2
            && stats.text_length < 1_200;
        let repeated_contribution_terms = stats.text_length < 6_000
            && text.matches("roles").count() >= 2
            && text.matches("affiliation").count() >= 2
            && links >= 2;
        let author_contribution_metadata = (stats.text_length < 1_000
            && text.contains("roles")
            && text.contains("affiliation")
            && contains_any(
                name,
                &[
                    "author-meta",
                    "author_meta",
                    "authroles",
                    "authaffiliations",
                ],
            ))
            || (repeated_contribution_terms
                && std::iter::once(node)
                    .chain(dom.ancestors(node))
                    .any(|ancestor| has_author_region_name(dom, ancestor))
                && !matches!(dom.tag(node), Some(Tag::Ol | Tag::Ul))
                && !dom.descendants(node).any(|descendant| {
                    matches!(
                        dom.tag(descendant),
                        Some(Tag::H1 | Tag::H2 | Tag::H3 | Tag::H4 | Tag::H5 | Tag::H6)
                    )
                }));
        let author_promotion = is_author_promotion(dom, node, &metrics);
        let audio_controls = is_audio_controls(&metrics);
        let job_profile = is_job_profile_content(dom, node, page_kind, &metrics);
        let collection_promotion = is_collection_promotion(dom, node, &metrics);
        let share_prompt = is_standalone_share_prompt(dom, node, &metrics);
        let sponsored_content = !is_inside_primary_content_container(dom, node)
            && is_sponsored_content(&metrics, has_sponsored_links[node.index()]);
        let revision_history =
            !is_inside_primary_content_container(dom, node) && is_revision_history(&metrics);

        let advertisement = strong_ad_name(name) && short && (links > 0 || stats.text_length < 100);
        let consent = contains_name_or_text(
            name,
            text,
            &["cookie consent", "cookie-banner", "consent-banner"],
        ) && short;
        let account = contains_any(name, &["login", "sign-in", "signin"])
            && (controls > 0 || links > 0)
            && short;
        let comment_ui = name.contains("comment")
            && stats.text_length < 180
            && (text.starts_with("comments")
                && text.contains("login")
                && text.contains("0 comments")
                || text.starts_with("login") && text.contains("0 comments")
                || text.starts_with("share: 0 comments")
                || text == "0 comments subscribe rss");

        let action_label = [
            "leave a comment",
            "share",
            "reply",
            "rate this",
            "answer this",
        ]
        .iter()
        .any(|label| text == *label || contains_followed_by_space(text, label));
        let action_url = has_action_urls[node.index()];
        let interaction_name = contains_any(
            name,
            &[
                "toolbar",
                "article-actions",
                "post-actions",
                "feedback",
                "share",
            ],
        );
        let interaction_signals =
            usize::from(action_label) + usize::from(action_url) + usize::from(interaction_name);
        let terminal_action = links > 0
            && stats.text_length < 160
            && link_density >= 0.55
            && interaction_signals >= 2
            && near_content_end(dom, node, root, store);

        let taxonomy_name = name
            .split(|character: char| !character.is_ascii_alphanumeric())
            .any(|token| {
                matches!(
                    token,
                    "taxonomy"
                        | "tags"
                        | "entities"
                        | "entitylist"
                        | "taglist"
                        | "subject"
                        | "subjects"
                        | "subjectarea"
                        | "subjectareas"
                )
            })
            || contains_any(
                name,
                &[
                    "company-portals",
                    "company_portals",
                    "entity-list",
                    "entity_list",
                    "tag-list",
                    "tag_list",
                ],
            );
        let terminal_taxonomy = taxonomy_name
            && links >= 2
            && stats.text_length < 300
            && link_density >= 0.45
            && near_content_end_ignoring_footnotes(dom, node, root, store);
        let subject_feedback =
            contains_any(name, &["subject-area", "subject_areas", "subjectareas"])
                && links >= 2
                && stats.text_length < 1_200
                && text.contains("feedback");
        let peripheral_panel_name = name
            .split(|character: char| !character.is_ascii_alphanumeric())
            .any(|token| matches!(token, "sidebar" | "comments" | "commentlist"));
        let terminal_peripheral_panel = peripheral_panel_name
            && links >= 3
            && short
            && link_density >= 0.2
            && (at_end || text.starts_with("comments") && text.contains("subscribe"));
        let card_rail = links >= 3
            && images >= 2
            && has_repeated_link_cards(dom, node)
            && matches!(dom.tag(node), Some(Tag::Aside | Tag::Div | Tag::Section))
            && at_end
            && !dom
                .descendants(node)
                .any(|descendant| dom.tag(descendant) == Some(Tag::Article))
            && !is_inside_article_container(dom, node);
        let print_citation = links >= 2
            && short
            && contains_any(name, &["print-citation", "story-footer"])
            && text.contains("appears in print");
        let redundant_document_toc = is_redundant_document_toc(dom, node, root, &metrics);
        let layout_side_rail = is_compact_layout_side_rail(dom, node, &metrics, protected);
        let inline_promotion = is_compact_inline_promotion(dom, node, &metrics, protected);
        let link_index_rail = is_link_index_rail(dom, node, &metrics);
        let document_maintenance = is_document_maintenance(dom, node, &metrics);

        if related
            || social
            || signup
            || breadcrumb
            || navigation
            || author
            || author_metadata
            || author_contribution_metadata
            || author_promotion
            || audio_controls
            || job_profile
            || collection_promotion
            || share_prompt
            || sponsored_content
            || revision_history
            || advertisement
            || consent
            || account
            || comment_ui
            || terminal_action
            || terminal_taxonomy
            || subject_feedback
            || terminal_peripheral_panel
            || card_rail
            || print_citation
            || redundant_document_toc
            || layout_side_rail
            || inline_promotion
            || link_index_rail
            || document_maintenance
        {
            if protected
                && !author_card
                && !author_contribution_metadata
                && !author_promotion
                && !job_profile
                && !collection_promotion
                && !share_prompt
                && !sponsored_content
                && !revision_history
                && !redundant_document_toc
                && !inline_promotion
                && !link_index_rail
            {
                hoist_protected_children(dom, node, store, evidence);
            }
            detach_and_invalidate_stats(dom, node, store);
        }
    }

    workspace.invalidate();
    remove_contextual_boilerplate_in_workspace(
        dom,
        root,
        store,
        evidence,
        text_buffer,
        nodes,
        workspace,
    );
    workspace.restore_scratch(scratch);
}

/// Removes small utility controls and media credits that survive root
/// selection. These elements often sit beside useful article content rather
/// than in a removable navigation subtree.
pub(crate) fn remove_inline_chrome_controls_in_workspace(
    dom: &mut Dom,
    root: NodeId,
    store: &mut crate::dom::NodeStateStore,
    evidence: &crate::document::SourceEvidence,
    workspace: &mut FragmentWorkspace,
) -> bool {
    workspace.ensure_snapshot(dom, root);
    let snapshot = workspace.elements_with_depth();
    let mut remove = vec![false; dom.len()];
    let mut text = String::new();

    for &(node, _) in snapshot {
        if node == root || dom.parent(node).is_none() {
            continue;
        }
        if is_redundant_disclosure_label(dom, node, &mut text) {
            remove[node.index()] = true;
            continue;
        }
        if is_protected_content(dom, node, evidence) {
            continue;
        }
        let at_start = near_content_start(dom, node, root, store);
        if !at_start {
            continue;
        }

        if is_small_navigation_media_link(dom, node, &mut text) {
            remove[node.index()] = true;
            continue;
        }

        if is_media_credit(dom, node, &mut text) {
            remove[node.index()] = true;
        }
    }

    let mut changed = false;
    for &(node, _) in snapshot {
        if !remove[node.index()] || dom.parent(node).is_none() {
            continue;
        }
        detach_and_invalidate_stats(dom, node, store);
        changed = true;
    }
    if changed {
        workspace.invalidate();
    }
    changed
}

fn is_redundant_disclosure_label(dom: &Dom, node: NodeId, text: &mut String) -> bool {
    if dom.tag(node) != Some(Tag::Summary) {
        return false;
    }
    get_normalized_inner_text(dom, node, text);
    let label = text.trim();
    if !equals_any_ascii_case_insensitive(
        label,
        &[
            "expand description",
            "collapse description",
            "show description",
            "hide description",
        ],
    ) {
        return false;
    }
    dom.parent(node).is_some_and(|parent| {
        dom.tag(parent) == Some(Tag::Details)
            && dom
                .element_children(parent)
                .any(|child| child != node && dom.has_non_whitespace_text(child))
    })
}

/// Removes separator text that only exists to imitate a Markdown rule. The
/// adjacent HTML horizontal rule is the semantic source for that separator.
pub(crate) fn remove_decorative_separator_paragraphs_in_workspace(
    dom: &mut Dom,
    root: NodeId,
    store: &mut crate::dom::NodeStateStore,
    evidence: &crate::document::SourceEvidence,
    workspace: &mut FragmentWorkspace,
) -> bool {
    workspace.ensure_snapshot(dom, root);
    let snapshot = workspace.elements_with_depth();
    let mut changed = false;
    for &(node, _) in snapshot {
        if dom.parent(node).is_none()
            || dom.tag(node) != Some(Tag::P)
            || is_protected_content(dom, node, evidence)
        {
            continue;
        }
        let mut text = String::new();
        dom.append_normalized_text_limited(node, &mut text, 32);
        if !is_decorative_separator_text(text.trim()) {
            continue;
        }
        let adjacent_rule = previous_element(dom, node)
            .is_some_and(|sibling| dom.tag(sibling) == Some(Tag::Hr))
            || next_element_sibling(dom, node)
                .is_some_and(|sibling| dom.tag(sibling) == Some(Tag::Hr));
        if adjacent_rule {
            detach_and_invalidate_stats(dom, node, store);
            changed = true;
        }
    }
    if changed {
        workspace.invalidate();
    }
    changed
}

fn is_small_navigation_media_link(dom: &Dom, node: NodeId, text: &mut String) -> bool {
    if dom.tag(node) != Some(Tag::A) {
        return false;
    }
    let images = dom
        .descendants(node)
        .filter(|&descendant| dom.tag(descendant) == Some(Tag::Img))
        .count();
    if images != 1
        || dom.descendants(node).any(|descendant| {
            matches!(
                dom.tag(descendant),
                Some(Tag::Pre | Tag::Table | Tag::Video | Tag::Audio)
            )
        })
    {
        return false;
    }
    get_normalized_inner_text(dom, node, text);
    let label = text.trim().to_ascii_lowercase();
    let title = dom.attr(node, AttrName::Title).unwrap_or_default();
    let alt = dom
        .descendants(node)
        .find(|&descendant| dom.tag(descendant) == Some(Tag::Img))
        .and_then(|image| dom.attr(image, AttrName::Alt))
        .unwrap_or_default();
    let utility = [label.as_str(), title, alt]
        .into_iter()
        .map(str::trim)
        .any(|value| {
            contains_any(
                &value.to_ascii_lowercase(),
                &["back", "return", "previous", "go to", "archive"],
            )
        });
    utility && dom.normalized_char_count(node) <= 96
}

fn is_media_credit(dom: &Dom, node: NodeId, text: &mut String) -> bool {
    if !matches!(
        dom.tag(node),
        Some(Tag::Div | Tag::P | Tag::Small | Tag::Span)
    ) {
        return false;
    }
    get_normalized_inner_text(dom, node, text);
    let value = text.trim();
    if value.is_empty() || value.chars().count() > 140 {
        return false;
    }
    let lower = value.to_ascii_lowercase();
    let credit = lower.starts_with("copyright")
        || lower.starts_with("©")
        || lower.starts_with("photo credit")
        || lower.starts_with("photo by")
        || lower.starts_with("image credit");
    if !credit
        || dom.descendants(node).any(|descendant| {
            matches!(
                dom.tag(descendant),
                Some(
                    Tag::H1
                        | Tag::H2
                        | Tag::H3
                        | Tag::H4
                        | Tag::H5
                        | Tag::H6
                        | Tag::Pre
                        | Tag::Table
                )
            )
        })
    {
        return false;
    }
    let Some(parent) = dom.parent(node) else {
        return false;
    };
    let mut previous = dom.prev_sibling(node);
    while let Some(sibling) = previous {
        if dom.is_comment(sibling) {
            previous = dom.prev_sibling(sibling);
            continue;
        }
        return dom
            .descendants(sibling)
            .any(|descendant| dom.tag(descendant) == Some(Tag::Img));
    }
    dom.element_children(parent)
        .any(|sibling| dom.tag(sibling) == Some(Tag::Img))
}

/// Removes document-level navigation and footer material that survives root
/// selection. These regions often sit inside a broad `main` wrapper, so root
/// semantics alone cannot separate them from the useful page.
///
/// A name or element tag is only one signal. Removal also requires a terminal
/// or leading position, link-heavy low-prose structure, and either semantic or
/// repeated structural evidence. This keeps pricing cards, article dates, and
/// short company content out of the global-chrome bucket.
pub(crate) fn remove_global_chrome_in_workspace(
    dom: &mut Dom,
    root: NodeId,
    store: &mut crate::dom::NodeStateStore,
    evidence: &crate::document::SourceEvidence,
    workspace: &mut FragmentWorkspace,
) -> bool {
    workspace.ensure_snapshot(dom, root);
    if workspace.elements_with_depth().is_empty() {
        return false;
    }

    store.enable_link_lengths();
    get_or_compute_stats(dom, root, store);

    workspace.ensure_chrome_aggregates(dom, root);
    let mut scratch = workspace.take_scratch();
    scratch.bits.resize(dom.len(), false);
    scratch.bits.fill(false);
    let remove = &mut scratch.bits[..dom.len()];
    let aggregates = workspace.chrome_aggregates();
    let mut signatures = HashMap::<u64, u8>::new();
    for &(node, _) in workspace.elements_with_depth() {
        let aggregate = aggregates[node.index()];
        if aggregate.link_count >= 2 {
            let count = signatures.entry(aggregate.signature).or_default();
            *count = count.saturating_add(1).min(3);
        }
    }
    let mut text_buffer = String::new();
    let mut name_buffer = String::new();
    let mut parent_content = vec![None; dom.len()];
    let mut substantive_children = vec![None; dom.len()];
    for &(node, depth) in workspace.elements_with_depth() {
        if node == root || dom.parent(node).is_none() {
            continue;
        }
        let aggregate = aggregates[node.index()];
        append_node_name(dom, node, &mut name_buffer);
        let name = name_buffer.as_str();
        let semantic_navigation = matches!(dom.tag(node), Some(Tag::Aside | Tag::Nav))
            || dom.tag(node) == Some(Tag::Footer)
            || dom.tag(node) == Some(Tag::Header) && aggregate.link_count >= 3
            || dom
                .attr(node, AttrName::Role)
                .is_some_and(|roles| has_any_token(roles, &["navigation", "contentinfo"]));
        let named_chrome = contains_any(
            name,
            &[
                "navigation",
                "navbar",
                "menu",
                "breadcrumb",
                "breadcrumbs",
                "footer",
                "contact",
                "legal",
                "newsletter",
                "recommendation",
                "recommendations",
                "recommended",
                "related-links",
                "related_links",
                "site-links",
                "site_links",
            ],
        );
        let repeated = aggregate.link_count >= 2
            && signatures
                .get(&aggregate.signature)
                .is_some_and(|count| *count >= 2);
        let metadata_candidate = has_metadata_name(name)
            || matches!(dom.tag(node), Some(Tag::Address | Tag::Time))
            || aggregate.has_time && aggregate.link_count >= 1
            || aggregate.link_count == 1
                && dom
                    .parent(node)
                    .is_some_and(|parent| dom.tag(parent) == Some(Tag::Header));
        let leading_frame_candidate = depth <= 3 && aggregate.link_count >= 3;
        let named_share = contains_any(name, &["sharebar", "sharecta", "share-bar", "share-cta"]);
        let revision_marker = is_revision_history_marker(dom, node);
        let sponsored_marker = aggregate.has_sponsored_link;
        if !semantic_navigation
            && !named_chrome
            && !repeated
            && !metadata_candidate
            && !leading_frame_candidate
            && !is_explicit_document_utility(name)
            && !named_share
            && !revision_marker
            && !sponsored_marker
        {
            continue;
        }
        if evidence.contains_semantic(node) {
            continue;
        }
        let stats = get_or_compute_stats(dom, node, store);
        if stats.text_length == 0 {
            continue;
        }
        let links = usize::from(aggregate.link_count);
        let link_density = get_link_density_cached(dom, node, stats.text_length, store);
        let non_link_chars = f64::from(stats.text_length) * (1.0 - link_density);
        let at_start = near_content_start(dom, node, root, store);
        let at_end = near_content_end(dom, node, root, store);
        if is_explicit_document_utility(name)
            && !is_inside_explicit_article_body(dom, node, root)
            && !is_inside_primary_content_container(dom, node)
            && stats.text_length < 2_400
            && aggregate.link_count >= 2
            && (at_start || at_end || name.contains("extra-services"))
        {
            remove[node.index()] = true;
            continue;
        }
        let adjacent_content =
            has_substantive_content_sibling(dom, node, store, &mut substantive_children);
        let dominant_content_sibling = leading_frame_candidate
            && stats.text_length <= 800
            && has_dominant_content_sibling(dom, node, stats, store, &mut parent_content);
        let leading_frame_position = at_start
            || leading_frame_candidate
                && has_only_header_or_empty_siblings_before(dom, node, store);
        get_normalized_inner_text(dom, node, &mut text_buffer);
        text_buffer.make_ascii_lowercase();
        let text = text_buffer.trim();

        let explicit_share = named_share
            && !is_inside_explicit_article_body(dom, node, root)
            && !is_inside_primary_content_container(dom, node)
            && stats.text_length < 700
            && (at_start || at_end || aggregate.link_count >= 2)
            && contains_any(
                text,
                &[
                    "know someone who should see",
                    "share this",
                    "pass it on",
                    "send them the link",
                ],
            );
        let explicit_sponsored = sponsored_marker
            && !is_inside_explicit_article_body(dom, node, root)
            && !is_inside_primary_content_container(dom, node)
            && stats.text_length < 700
            && text.contains("sponsored");
        let explicit_revision = revision_marker
            && !is_inside_explicit_article_body(dom, node, root)
            && !is_inside_primary_content_container(dom, node)
            && stats.text_length < 700
            && (at_start || at_end);
        if explicit_share || explicit_sponsored || explicit_revision {
            remove[node.index()] = true;
            continue;
        }

        let metrics = ChromeMetrics {
            name,
            text,
            stats,
            links,
            link_density,
            non_link_chars,
            repeated,
            has_meaningful_media: aggregate.has_meaningful_media,
            has_content_structure: aggregate.has_content_structure,
            at_start,
            at_end,
            adjacent_content,
        };
        if leading_frame_position && has_compact_content_identity(dom, node, links) {
            continue;
        }
        let leading_metadata = is_leading_metadata(dom, node, &metrics, dominant_content_sibling);
        let leading_frame = is_leading_page_frame(
            dom,
            node,
            &metrics,
            dominant_content_sibling,
            leading_frame_position,
        );
        if (semantic_navigation || named_chrome || repeated || leading_metadata || leading_frame)
            && (is_global_navigation(dom, node, root, &metrics)
                || is_global_footer(dom, node, root, &metrics)
                || leading_metadata
                || leading_frame)
        {
            remove[node.index()] = true;
        }
    }

    let mut changed = false;
    for &(node, _) in workspace.elements_with_depth() {
        if !remove[node.index()] || dom.parent(node).is_none() {
            continue;
        }
        if dom.ancestors(node).any(|ancestor| remove[ancestor.index()]) {
            continue;
        }
        if dom.tag(node) == Some(Tag::Header) {
            hoist_header_identity(dom, node);
        } else if dom.tag(node) == Some(Tag::Footer)
            || dom
                .attr(node, AttrName::Role)
                .is_some_and(|roles| has_token(roles, "contentinfo"))
            || node_name(dom, node).contains("footer")
        {
            hoist_footer_identity(dom, node);
        }
        detach_and_invalidate_stats(dom, node, store);
        changed = true;
    }
    if changed {
        workspace.invalidate();
    }
    workspace.restore_scratch(scratch);
    changed
}

fn is_explicit_document_utility(name: &str) -> bool {
    contains_any(
        name,
        &[
            "browse-context",
            "extra-services",
            "labs-display",
            "labstabs",
            "recommender",
            "revision-history",
        ],
    )
}

/// Removes non-discussion comment threads and repeated content blocks from a
/// selected article fragment.
///
/// Comment widgets can be descendants of an article element, so the global
/// chrome pass cannot reliably remove them. The duplicate pass uses a compact
/// normalized-text fingerprint built while walking the fragment once. It
/// keeps the first occurrence in the primary article region and removes later
/// copies, which also handles responsive or duplicated documentation blocks.
pub(crate) fn remove_repeated_and_discussion_content_in_workspace(
    dom: &mut Dom,
    root: NodeId,
    page_kind: PageKind,
    store: &mut crate::dom::NodeStateStore,
    evidence: &crate::document::SourceEvidence,
    workspace: &mut FragmentWorkspace,
) -> bool {
    workspace.ensure_snapshot(dom, root);
    let snapshot = workspace.elements_with_depth();
    if snapshot.is_empty() {
        return false;
    }

    let mut changed = false;
    if page_kind != PageKind::Discussion && has_possible_comment_content(dom, snapshot) {
        let snapshot = workspace.elements_with_depth();
        let comment_aggregates = comment_aggregates(dom, snapshot);
        let mut remove = vec![false; dom.len()];
        for &(node, _) in snapshot {
            if node == root
                || dom.parent(node).is_none()
                || is_protected_content(dom, node, evidence)
            {
                continue;
            }
            if is_comment_region(dom, node, root, store, comment_aggregates[node.index()]) {
                remove[node.index()] = true;
            }
        }
        for &(node, _) in snapshot {
            if node == root || dom.parent(node).is_none() || remove[node.index()] {
                continue;
            }
            if is_comment_control(dom, node, root, store) {
                remove[node.index()] = true;
            }
        }
        for &(node, _) in snapshot {
            if !remove[node.index()] || dom.parent(node).is_none() {
                continue;
            }
            if dom.ancestors(node).any(|ancestor| remove[ancestor.index()]) {
                continue;
            }
            detach_and_invalidate_stats(dom, node, store);
            changed = true;
        }
        if changed {
            workspace.invalidate();
            workspace.ensure_snapshot(dom, root);
        }
    }

    if page_kind != PageKind::Discussion {
        workspace.ensure_chrome_aggregates(dom, root);
        let snapshot = workspace.elements_with_depth();
        let aggregates = workspace.chrome_aggregates();
        let mut remove = vec![false; dom.len()];
        let mut text = String::new();
        for &(node, _) in snapshot {
            if node == root
                || dom.parent(node).is_none()
                || !matches!(
                    dom.tag(node),
                    Some(
                        Tag::Article
                            | Tag::Aside
                            | Tag::Div
                            | Tag::Footer
                            | Tag::Header
                            | Tag::Main
                            | Tag::Nav
                            | Tag::Ol
                            | Tag::P
                            | Tag::Section
                            | Tag::Ul
                    )
                )
                || is_protected_content(dom, node, evidence)
            {
                continue;
            }
            let stats = get_or_compute_stats(dom, node, store);
            if stats.text_length == 0 || stats.text_length >= 900 {
                continue;
            }
            if near_content_start(dom, node, root, store)
                && has_compact_content_identity(
                    dom,
                    node,
                    usize::from(aggregates[node.index()].link_count),
                )
            {
                continue;
            }
            if is_explicit_terminal_promotion(dom, node, aggregates[node.index()].link_count)
                && near_terminal_peripheral_end(dom, node, root, store)
            {
                remove[node.index()] = true;
                continue;
            }
            text.clear();
            append_bounded_text(dom, node, 96, &mut text);
            text.make_ascii_lowercase();
            if is_terminal_peripheral_region(
                dom,
                node,
                root,
                store,
                TerminalRegionSignals {
                    stats,
                    links: aggregates[node.index()].link_count,
                    name: node_name(dom, node).as_ref(),
                    text: text.trim(),
                },
            ) {
                remove[node.index()] = true;
            }
        }
        for &(node, _) in snapshot {
            if !remove[node.index()] || dom.parent(node).is_none() {
                continue;
            }
            if dom.ancestors(node).any(|ancestor| remove[ancestor.index()]) {
                continue;
            }
            detach_and_invalidate_stats(dom, node, store);
            changed = true;
        }
        if changed {
            workspace.invalidate();
            workspace.ensure_snapshot(dom, root);
        }
    }

    let snapshot = workspace.elements_with_depth();
    if page_kind != PageKind::Discussion {
        let mut repeated_links = HashMap::<(String, String), Vec<NodeId>>::new();
        let mut text = String::new();
        for &(node, _) in snapshot {
            if dom.tag(node) != Some(Tag::A)
                || dom.ancestors(node).any(|ancestor| {
                    matches!(
                        dom.tag(ancestor),
                        Some(
                            Tag::Li
                                | Tag::Ol
                                | Tag::Table
                                | Tag::Tbody
                                | Tag::Td
                                | Tag::Th
                                | Tag::Tr
                                | Tag::Ul
                        )
                    )
                })
            {
                continue;
            }
            text.clear();
            get_normalized_inner_text(dom, node, &mut text);
            let label = text.trim();
            if label.is_empty() || label.chars().count() > 48 {
                continue;
            }
            let href = dom.attr(node, AttrName::Href).unwrap_or_default().trim();
            if href.is_empty() || href.starts_with('#') {
                continue;
            }
            repeated_links
                .entry((label.to_ascii_lowercase(), href.to_ascii_lowercase()))
                .or_default()
                .push(node);
        }
        let mut remove = vec![false; dom.len()];
        for nodes in repeated_links.into_values().filter(|nodes| nodes.len() > 1) {
            for &node in nodes.iter().skip(1) {
                if near_content_start(dom, node, root, store)
                    || near_content_end(dom, node, root, store)
                {
                    remove[node.index()] = true;
                }
            }
        }
        for &(node, _) in snapshot {
            if !remove[node.index()] || dom.parent(node).is_none() {
                continue;
            }
            detach_and_invalidate_stats(dom, node, store);
            changed = true;
        }
        if changed {
            workspace.invalidate();
            workspace.ensure_snapshot(dom, root);
        }
    }

    let snapshot = workspace.elements_with_depth();
    if has_repeated_block_hint(dom, snapshot) {
        let fingerprints = normalized_fingerprints(dom, root, snapshot);
        let mut positions = vec![usize::MAX; dom.len()];
        positions[root.index()] = 0;
        for (position, &(node, _)) in snapshot.iter().enumerate() {
            positions[node.index()] = position.saturating_add(1);
        }
        let mut groups = HashMap::<(u64, u64, u64, u32, u16, u8, usize), Vec<NodeId>>::new();
        for node in std::iter::once(root).chain(snapshot.iter().map(|&(node, _)| node)) {
            let fingerprint = fingerprints[node.index()];
            if fingerprint.text_chars >= 48 && fingerprint.block_count > 0 {
                let hash = if fingerprint.normalized_hash != 0 {
                    fingerprint.normalized_hash
                } else {
                    fingerprint.hash
                };
                let parent = dom.parent(node).map_or(node.index(), NodeId::index);
                groups
                    .entry((
                        hash,
                        fingerprint.first_leaf_hash,
                        fingerprint.last_leaf_hash,
                        fingerprint.text_chars,
                        fingerprint.block_count,
                        fingerprint.role,
                        parent,
                    ))
                    .or_default()
                    .push(node);
            }
        }

        let mut remove = vec![false; dom.len()];
        for nodes in groups.into_values().filter(|nodes| nodes.len() > 1) {
            let retained = nodes
                .iter()
                .copied()
                .min_by_key(|&node| {
                    (
                        !is_primary_article_region(dom, node, root),
                        positions[node.index()],
                    )
                })
                .unwrap_or(nodes[0]);
            for node in nodes {
                if node == retained
                    || node == root
                    || !is_duplicate_region_candidate(dom, node)
                    || dom.parent(node).is_none()
                    || is_protected_content(dom, node, evidence)
                    || dom.ancestors(retained).any(|ancestor| ancestor == node)
                    || fingerprints[node.index()].strong_hash
                        != fingerprints[retained.index()].strong_hash
                {
                    continue;
                }
                remove[node.index()] = true;
            }
        }

        for &(node, _) in snapshot {
            if !remove[node.index()] || dom.parent(node).is_none() {
                continue;
            }
            if dom.ancestors(node).any(|ancestor| remove[ancestor.index()]) {
                continue;
            }
            detach_and_invalidate_stats(dom, node, store);
            changed = true;
        }
    }
    if changed {
        workspace.invalidate();
    }
    changed
}

#[derive(Clone, Copy, Default)]
struct NormalizedFingerprint {
    hash: u64,
    strong_hash: u64,
    normalized_hash: u64,
    first_leaf_hash: u64,
    last_leaf_hash: u64,
    leaf_count: u16,
    text_chars: u32,
    block_count: u16,
    role: u8,
}

fn has_repeated_block_hint(dom: &Dom, snapshot: &[(NodeId, u32)]) -> bool {
    let mut fingerprints = HashMap::<(u64, usize, usize), u8>::new();
    let mut text = String::new();
    for &(node, _) in snapshot {
        if !matches!(
            dom.tag(node),
            Some(Tag::H1 | Tag::H2 | Tag::H3 | Tag::H4 | Tag::H5 | Tag::H6 | Tag::P)
        ) {
            continue;
        }
        text.clear();
        dom.append_normalized_text_limited(node, &mut text, 512);
        text.make_ascii_lowercase();
        let text = text.trim();
        let text_chars = if text.is_ascii() {
            text.len()
        } else {
            text.chars().count()
        };
        if text_chars < 24
            && !matches!(
                dom.tag(node),
                Some(Tag::H1 | Tag::H2 | Tag::H3 | Tag::H4 | Tag::H5 | Tag::H6)
            )
        {
            continue;
        }
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut hasher);
        // The duplicate-region pass groups block candidates by parent as
        // well. Keep that boundary for paragraphs so repeated prose in
        // separate sections does not trigger a full fingerprint walk. A
        // repeated heading remains a useful document-level hint because it
        // commonly identifies duplicated titled regions whose paragraphs have
        // different wrappers.
        let parent = if dom.tag(node) == Some(Tag::P) {
            dom.parent(node).map_or(node.index(), NodeId::index)
        } else {
            usize::MAX
        };
        let key = (hasher.finish(), text_chars, parent);
        let count = fingerprints.entry(key).or_default();
        *count = count.saturating_add(1);
        if *count >= 2 {
            return true;
        }
    }
    false
}

fn normalized_fingerprints(
    dom: &Dom,
    root: NodeId,
    snapshot: &[(NodeId, u32)],
) -> Vec<NormalizedFingerprint> {
    const HASH_OFFSET: u64 = 14_695_981_039_346_656_037;
    let mut fingerprints = vec![NormalizedFingerprint::default(); dom.len()];
    for node in std::iter::once(root)
        .chain(snapshot.iter().map(|&(node, _)| node))
        .rev()
    {
        let mut hash = HASH_OFFSET;
        let mut strong_hasher = std::collections::hash_map::DefaultHasher::new();
        strong_hasher.write_u8(0xa5);
        strong_hasher.write_u8(fingerprint_role(dom, node));
        let mut normalized_hash = 0_u64;
        let mut first_leaf_hash = 0_u64;
        let mut last_leaf_hash = 0_u64;
        let mut leaf_count = 0_u16;
        let mut text_chars = 0_u32;
        let mut block_count = u16::from(is_fingerprint_block(dom, node));
        for child in dom.children(node) {
            if let Some(text) = dom.text_node(child) {
                let (leaf_hash, leaf_chars) =
                    hash_normalized_text(text, &mut hash, &mut text_chars, &mut strong_hasher);
                if leaf_chars > 0 {
                    normalized_hash = normalized_hash.wrapping_add(leaf_hash);
                    if leaf_count == 0 {
                        first_leaf_hash = leaf_hash;
                    }
                    last_leaf_hash = leaf_hash;
                    leaf_count = leaf_count.saturating_add(1);
                }
            } else if dom.is_element(child) {
                let child_fingerprint = fingerprints[child.index()];
                mix_fingerprint_boundary(&mut hash);
                mix_hash(&mut hash, child_fingerprint.hash);
                strong_hasher.write_u8(0x1f);
                strong_hasher.write_u64(child_fingerprint.strong_hash);
                normalized_hash = normalized_hash.wrapping_add(child_fingerprint.normalized_hash);
                if child_fingerprint.leaf_count > 0 {
                    if leaf_count == 0 {
                        first_leaf_hash = child_fingerprint.first_leaf_hash;
                    }
                    last_leaf_hash = child_fingerprint.last_leaf_hash;
                    leaf_count = leaf_count.saturating_add(child_fingerprint.leaf_count);
                }
                text_chars = text_chars.saturating_add(child_fingerprint.text_chars);
                block_count = block_count.saturating_add(child_fingerprint.block_count);
            }
        }
        fingerprints[node.index()] = NormalizedFingerprint {
            hash,
            strong_hash: strong_hasher.finish(),
            normalized_hash,
            first_leaf_hash,
            last_leaf_hash,
            leaf_count,
            text_chars,
            block_count,
            role: fingerprint_role(dom, node),
        };
    }
    fingerprints
}

fn is_duplicate_region_candidate(dom: &Dom, node: NodeId) -> bool {
    matches!(
        dom.tag(node),
        Some(Tag::Article | Tag::Aside | Tag::Div | Tag::Main | Tag::Section)
    ) && dom
        .element_children(node)
        .any(|child| is_fingerprint_block(dom, child))
}

fn fingerprint_role(dom: &Dom, node: NodeId) -> u8 {
    match dom.tag(node) {
        Some(Tag::Article) => 1,
        Some(Tag::Aside) => 2,
        Some(Tag::Div) => 3,
        Some(Tag::Main) => 4,
        Some(Tag::Section) => 5,
        _ => 0,
    }
}

fn hash_normalized_text(
    text: &str,
    hash: &mut u64,
    text_chars: &mut u32,
    strong_hasher: &mut std::collections::hash_map::DefaultHasher,
) -> (u64, u32) {
    const HASH_OFFSET: u64 = 14_695_981_039_346_656_037;
    let mut leaf_hash = HASH_OFFSET;
    let mut leaf_chars = 0_u32;
    let mut pending_space = false;
    if text.is_ascii() {
        for &byte in text.as_bytes() {
            if byte.is_ascii_whitespace() {
                pending_space = true;
                continue;
            }
            if pending_space && *text_chars > 0 {
                mix_byte(hash, b' ');
                strong_hasher.write_u8(b' ');
                *text_chars = (*text_chars).saturating_add(1);
            }
            if pending_space && leaf_chars > 0 {
                mix_byte(&mut leaf_hash, b' ');
                leaf_chars = leaf_chars.saturating_add(1);
            }
            pending_space = false;
            let lowercase = byte.to_ascii_lowercase();
            mix_byte(hash, lowercase);
            mix_byte(&mut leaf_hash, lowercase);
            strong_hasher.write_u8(lowercase);
            *text_chars = (*text_chars).saturating_add(1);
            leaf_chars = leaf_chars.saturating_add(1);
        }
        return (leaf_hash, leaf_chars);
    }
    for character in text.chars() {
        if character.is_whitespace() {
            pending_space = true;
            continue;
        }
        if pending_space && *text_chars > 0 {
            mix_byte(hash, b' ');
            strong_hasher.write_u8(b' ');
            *text_chars = (*text_chars).saturating_add(1);
        }
        if pending_space && leaf_chars > 0 {
            mix_byte(&mut leaf_hash, b' ');
            leaf_chars = leaf_chars.saturating_add(1);
        }
        pending_space = false;
        for lowercase in character.to_lowercase() {
            mix_u32(hash, lowercase as u32);
            let mut encoded = [0; 4];
            strong_hasher.write(lowercase.encode_utf8(&mut encoded).as_bytes());
            mix_u32(&mut leaf_hash, lowercase as u32);
        }
        *text_chars = (*text_chars).saturating_add(1);
        leaf_chars = leaf_chars.saturating_add(1);
    }
    (leaf_hash, leaf_chars)
}

fn mix_fingerprint_boundary(hash: &mut u64) {
    mix_byte(hash, 0x1f);
}

fn mix_hash(hash: &mut u64, value: u64) {
    for byte in value.to_le_bytes() {
        mix_byte(hash, byte);
    }
}

fn mix_u32(hash: &mut u64, value: u32) {
    for byte in value.to_le_bytes() {
        mix_byte(hash, byte);
    }
}

fn mix_byte(hash: &mut u64, byte: u8) {
    *hash = hash
        .wrapping_mul(1_099_511_628_211)
        .wrapping_add(u64::from(byte));
}

fn is_fingerprint_block(dom: &Dom, node: NodeId) -> bool {
    matches!(
        dom.tag(node),
        Some(
            Tag::Article
                | Tag::Blockquote
                | Tag::Figure
                | Tag::H1
                | Tag::H2
                | Tag::H3
                | Tag::H4
                | Tag::H5
                | Tag::H6
                | Tag::Li
                | Tag::P
                | Tag::Pre
                | Tag::Section
                | Tag::Table
        )
    )
}

fn is_primary_article_region(dom: &Dom, node: NodeId, root: NodeId) -> bool {
    for ancestor in std::iter::once(node).chain(dom.ancestors(node)) {
        if is_primary_article_element(dom, ancestor) {
            return true;
        }
        if ancestor == root {
            break;
        }
    }
    false
}

fn is_primary_article_element(dom: &Dom, node: NodeId) -> bool {
    dom.tag(node) == Some(Tag::Article)
        || dom
            .attr(node, AttrName::ItemProp)
            .is_some_and(|value| has_any_token(value, &["articleBody", "text"]))
        || dom
            .attr(node, AttrName::Role)
            .is_some_and(|roles| has_token(roles, "article"))
}

#[derive(Clone, Copy, Default)]
struct CommentAggregate {
    comment_items: u8,
    reply_links: u8,
}

fn comment_aggregates(dom: &Dom, snapshot: &[(NodeId, u32)]) -> Vec<CommentAggregate> {
    let mut aggregates = vec![CommentAggregate::default(); dom.len()];
    for &(node, _) in snapshot.iter().rev() {
        let name = node_name(dom, node);
        let mut comment_items = u8::from(has_comment_token(&name));
        let mut reply_links = u8::from(is_reply_link(dom, node));
        for child in dom.element_children(node) {
            let child_aggregate = aggregates[child.index()];
            comment_items = comment_items
                .saturating_add(child_aggregate.comment_items)
                .min(3);
            reply_links = reply_links
                .saturating_add(child_aggregate.reply_links)
                .min(3);
        }
        aggregates[node.index()] = CommentAggregate {
            comment_items,
            reply_links,
        };
    }
    aggregates
}

/// Returns whether the comment-specific cleanup pass can find any candidate.
///
/// Most article fragments have no comment markers. Avoid the aggregate build
/// and per-element name normalization in that common case. This is only an
/// early-out check; all detailed comment classification remains unchanged.
fn has_possible_comment_content(dom: &Dom, snapshot: &[(NodeId, u32)]) -> bool {
    for &(node, _) in snapshot {
        let named = dom.tag(node) == Some(Tag::Other)
            && dom.qual_name(node).is_some_and(|name| {
                let local = name.local.as_ref();
                contains_ascii_case_insensitive(local, "comment")
                    || contains_ascii_case_insensitive(local, "discussion")
                    || contains_ascii_case_insensitive(local, "repl")
            });
        let class_or_id = [AttrName::Class, AttrName::Id]
            .into_iter()
            .filter_map(|name| dom.attr(node, name))
            .any(|value| {
                contains_ascii_case_insensitive(value, "comment")
                    || contains_ascii_case_insensitive(value, "discussion")
                    || contains_ascii_case_insensitive(value, "repl")
            });
        let semantic = dom
            .attr(node, AttrName::Role)
            .is_some_and(|value| has_any_token(value, &["comment", "discussion"]));
        let reply_link = dom.tag(node) == Some(Tag::A)
            && (dom.attr(node, AttrName::Href).is_some_and(|href| {
                contains_ascii_case_insensitive(href, "comment")
                    || contains_ascii_case_insensitive(href, "reply")
            }) || dom.normalized_text_eq_ignore_ascii_case(node, b"reply"));
        if named || class_or_id || semantic || reply_link {
            return true;
        }
    }
    false
}

fn has_comment_token(name: &str) -> bool {
    name.split(|character: char| !character.is_ascii_alphanumeric())
        .any(|token| matches!(token, "comment" | "comments" | "discussion" | "replies"))
}

fn is_reply_link(dom: &Dom, node: NodeId) -> bool {
    dom.tag(node) == Some(Tag::A)
        && (dom.normalized_text_eq_ignore_ascii_case(node, b"reply")
            || dom
                .attr(node, AttrName::Href)
                .is_some_and(|href| contains_ascii_case_insensitive(href, "reply")))
}

fn is_comment_region(
    dom: &Dom,
    node: NodeId,
    root: NodeId,
    store: &mut crate::dom::NodeStateStore,
    aggregate: CommentAggregate,
) -> bool {
    let name = node_name(dom, node);
    let named = has_comment_token(&name);
    let semantic = dom
        .attr(node, AttrName::Role)
        .is_some_and(|roles| has_any_token(roles, &["comment", "discussion"]));
    if !(named || semantic) || matches!(dom.tag(node), Some(Tag::P | Tag::A | Tag::Span)) {
        return false;
    }
    let terminal = near_content_end(dom, node, root, store)
        || near_terminal_peripheral_end(dom, node, root, store);
    semantic
        || aggregate.comment_items >= 2
        || aggregate.reply_links >= 2
        || dom
            .attr(node, AttrName::Id)
            .is_some_and(|id| id.eq_ignore_ascii_case("comments"))
        || name.contains("comment-thread")
        || named && terminal
}

fn is_comment_control(
    dom: &Dom,
    node: NodeId,
    root: NodeId,
    store: &mut crate::dom::NodeStateStore,
) -> bool {
    if dom.tag(node) != Some(Tag::A) {
        return false;
    }
    let href = dom.attr(node, AttrName::Href).unwrap_or_default();
    if !contains_ascii_case_insensitive(href, "comment") {
        return false;
    }
    if dom.ancestors(node).any(|ancestor| {
        matches!(
            dom.tag(ancestor),
            Some(
                Tag::Li | Tag::Ol | Tag::Table | Tag::Tbody | Tag::Td | Tag::Th | Tag::Tr | Tag::Ul
            )
        )
    }) {
        return false;
    }
    let mut text = String::new();
    get_normalized_inner_text(dom, node, &mut text);
    let text = text.trim();
    let comment_label = contains_ascii_case_insensitive(text, "comment")
        || text.bytes().all(|byte| byte.is_ascii_digit());
    comment_label
        && (near_content_start(dom, node, root, store) || near_content_end(dom, node, root, store))
}

struct TerminalRegionSignals<'a> {
    stats: NodeStats,
    links: u8,
    name: &'a str,
    text: &'a str,
}

fn is_terminal_peripheral_region(
    dom: &Dom,
    node: NodeId,
    root: NodeId,
    store: &mut crate::dom::NodeStateStore,
    signals: TerminalRegionSignals<'_>,
) -> bool {
    let TerminalRegionSignals {
        stats,
        links,
        name,
        text,
    } = signals;
    let named = contains_any(
        name,
        &[
            "article-footer",
            "comments",
            "comment-thread",
            "more-stories",
            "newsletter",
            "post-footer",
            "previous-post",
            "read-more",
            "related",
            "story-footer",
            "subscribe",
        ],
    );
    let labelled = starts_with_any(
        text,
        &[
            "here's a preview of a related post",
            "next post",
            "previous post",
            "read more",
            "related topics",
            "monthly newsletter",
            "more from ",
            "you may also like",
        ],
    );
    let footer = matches!(dom.tag(node), Some(Tag::Footer));
    if !(named || labelled || footer) {
        return false;
    }
    let link_density = get_link_density_cached(dom, node, stats.text_length, store);
    if is_inside_article_container(dom, node) && has_meaningful_region_content(dom, node) {
        return false;
    }
    if !labelled && !name.contains("newsletter") && has_meaningful_heading(dom, node) {
        return false;
    }
    let at_end = near_content_end(dom, node, root, store)
        || near_terminal_peripheral_end(dom, node, root, store);
    let at_start = near_content_start(dom, node, root, store);
    let low_prose = stats.sentence_end_count <= 8 && stats.text_length < 900;
    let link_cluster = links > 0 && link_density >= 0.15;
    if links == 0
        && stats.sentence_end_count > 0
        && !labelled
        && !name.contains("newsletter")
        && !name.contains("comments")
    {
        return false;
    }
    low_prose
        && ((at_end
            && (named || labelled || footer)
            && (link_cluster || !has_substantive_prose(dom, node)))
            || at_start && labelled && link_cluster)
}

fn near_terminal_peripheral_end(
    dom: &Dom,
    node: NodeId,
    root: NodeId,
    store: &mut crate::dom::NodeStateStore,
) -> bool {
    let mut current = node;
    let mut trailing_chars = 0_usize;
    loop {
        let mut sibling = dom.next_sibling(current);
        while let Some(next) = sibling {
            let stats = get_or_compute_stats(dom, next, store);
            if stats.text_length >= 360 || stats.sentence_end_count > 2 {
                return false;
            }
            trailing_chars = trailing_chars.saturating_add(stats.text_length as usize);
            if trailing_chars > 700 {
                return false;
            }
            sibling = dom.next_sibling(next);
        }
        if current == root {
            return true;
        }
        let Some(parent) = dom.parent(current) else {
            return true;
        };
        current = parent;
    }
}

#[derive(Clone, Copy, Default)]
struct ChromeAggregate {
    link_count: u8,
    signature: u64,
    has_meaningful_media: bool,
    has_content_structure: bool,
    has_time: bool,
    has_sponsored_link: bool,
}

fn chrome_aggregates(dom: &Dom, snapshot: &[(NodeId, u32)]) -> Vec<ChromeAggregate> {
    const HASH_OFFSET: u64 = 14_695_981_039_346_656_037;
    let mut aggregates = vec![ChromeAggregate::default(); dom.len()];
    for &(node, _) in snapshot.iter().rev() {
        let mut signature = HASH_OFFSET;
        signature = signature
            .wrapping_mul(1_099_511_628_211)
            .wrapping_add(dom.tag(node).map_or(0, |tag| tag as u64 + 1));
        let mut link_count = u8::from(dom.tag(node) == Some(Tag::A));
        let mut has_meaningful_media = own_meaningful_media(dom, node);
        let mut has_content_structure = matches!(
            dom.tag(node),
            Some(Tag::H1 | Tag::H2 | Tag::H3 | Tag::H4 | Tag::H5 | Tag::H6 | Tag::P)
        );
        let mut has_time = dom.tag(node) == Some(Tag::Time);
        let mut has_sponsored_link = is_sponsored_anchor(dom, node);
        for child in dom.element_children(node) {
            let child_aggregate = aggregates[child.index()];
            link_count = link_count
                .saturating_add(child_aggregate.link_count)
                .min(32);
            has_meaningful_media |= child_aggregate.has_meaningful_media;
            has_content_structure |= child_aggregate.has_content_structure;
            has_time |= child_aggregate.has_time;
            has_sponsored_link |= child_aggregate.has_sponsored_link;
            signature = signature
                .wrapping_mul(1_099_511_628_211)
                .wrapping_add(child_aggregate.signature);
        }
        aggregates[node.index()] = ChromeAggregate {
            link_count,
            signature,
            has_meaningful_media,
            has_content_structure,
            has_time,
            has_sponsored_link,
        };
    }
    aggregates
}

fn own_meaningful_media(dom: &Dom, node: NodeId) -> bool {
    if dom.tag(node) == Some(Tag::Figure)
        && dom.descendants(node).any(|child| {
            dom.tag(child) == Some(Tag::Figcaption) && dom.has_non_whitespace_text(child)
        })
    {
        return true;
    }
    dom.tag(node) == Some(Tag::Img)
        && dom.attr(node, AttrName::Alt).is_some_and(|alt| {
            let alt = alt.trim();
            alt.chars().count() >= 12
                && !["logo", "icon", "avatar", "placeholder"]
                    .iter()
                    .any(|needle| contains_ascii_case_insensitive(alt, needle))
        })
}

fn has_substantive_prose(dom: &Dom, node: NodeId) -> bool {
    dom.descendants(node).any(|descendant| {
        matches!(dom.tag(descendant), Some(Tag::Blockquote | Tag::P))
            && dom.normalized_char_count(descendant) >= 80
    })
}

fn is_brand_identity_link(dom: &Dom, link: NodeId) -> bool {
    let named = [AttrName::Class, AttrName::Id]
        .into_iter()
        .filter_map(|attribute| dom.attr(link, attribute))
        .flat_map(|value| value.split(|character: char| !character.is_ascii_alphanumeric()))
        .any(|token| {
            equals_any_ascii_case_insensitive(
                token,
                &[
                    "brand",
                    "branding",
                    "logo",
                    "masthead",
                    "wordmark",
                    "sitetitle",
                ],
            )
        });
    if named {
        return true;
    }
    let Some(href) = dom.attr(link, AttrName::Href) else {
        return false;
    };
    let root_link = href.trim() == "/" || href.trim().is_empty();
    if !root_link {
        return false;
    }
    let mut text_buffer = String::new();
    let text = get_normalized_inner_text(dom, link, &mut text_buffer);
    (2..=80).contains(&text.chars().count())
        && !equals_any_ascii_case_insensitive(
            text.trim(),
            &["home", "menu", "menu button", "skip to content"],
        )
}

fn hoist_header_identity(dom: &mut Dom, header: NodeId) {
    let identity = dom
        .descendants(header)
        .filter(|&node| dom.tag(node) == Some(Tag::A))
        .find(|&link| is_brand_identity_link(dom, link));
    if let Some(identity) = identity {
        dom.insert_before(header, identity);
    }
}

fn has_substantive_content_sibling(
    dom: &Dom,
    node: NodeId,
    store: &mut crate::dom::NodeStateStore,
    substantive_children: &mut [Option<u16>],
) -> bool {
    let Some(parent) = dom.parent(node) else {
        return false;
    };
    let count = if let Some(count) = substantive_children[parent.index()] {
        count
    } else {
        let count = dom
            .element_children(parent)
            .filter(|&child| is_substantive_content_child(dom, child, store))
            .count()
            .min(usize::from(u16::MAX)) as u16;
        substantive_children[parent.index()] = Some(count);
        count
    };
    count > u16::from(is_substantive_content_child(dom, node, store))
}

fn is_substantive_content_child(
    dom: &Dom,
    node: NodeId,
    store: &mut crate::dom::NodeStateStore,
) -> bool {
    if matches!(
        dom.tag(node),
        Some(Tag::Aside | Tag::Footer | Tag::Header | Tag::Nav)
    ) {
        return false;
    }
    let stats = get_or_compute_stats(dom, node, store);
    stats.text_length >= 80
        && (stats.sentence_end_count > 0
            || dom.descendants(node).any(|descendant| {
                matches!(
                    dom.tag(descendant),
                    Some(
                        Tag::Article
                            | Tag::H1
                            | Tag::H2
                            | Tag::H3
                            | Tag::H4
                            | Tag::H5
                            | Tag::H6
                            | Tag::P
                    )
                )
            }))
}

fn has_dominant_content_sibling(
    dom: &Dom,
    node: NodeId,
    node_stats: NodeStats,
    store: &mut crate::dom::NodeStateStore,
    parent_content: &mut [Option<(u32, u32)>],
) -> bool {
    let Some(parent) = dom.parent(node) else {
        return false;
    };
    let minimum = node_stats.text_length.saturating_mul(4).max(600);
    let (mut text_length, mut sentence_ends) = if let Some(total) = parent_content[parent.index()] {
        total
    } else {
        let mut total = (0_u32, 0_u32);
        for sibling in dom.element_children(parent) {
            if matches!(
                dom.tag(sibling),
                Some(Tag::Aside | Tag::Footer | Tag::Header | Tag::Nav)
            ) {
                continue;
            }
            let stats = get_or_compute_stats(dom, sibling, store);
            total.0 = total.0.saturating_add(stats.text_length);
            total.1 = total.1.saturating_add(stats.sentence_end_count);
        }
        parent_content[parent.index()] = Some(total);
        total
    };
    if !matches!(
        dom.tag(node),
        Some(Tag::Aside | Tag::Footer | Tag::Header | Tag::Nav)
    ) {
        text_length = text_length.saturating_sub(node_stats.text_length);
        sentence_ends = sentence_ends.saturating_sub(node_stats.sentence_end_count);
    }
    text_length >= minimum && sentence_ends >= 4
}

fn has_only_header_or_empty_siblings_before(
    dom: &Dom,
    node: NodeId,
    store: &mut crate::dom::NodeStateStore,
) -> bool {
    let mut sibling = dom.prev_sibling(node);
    let mut saw_header = false;
    while let Some(previous) = sibling {
        if dom.tag(previous) == Some(Tag::Header) {
            saw_header = true;
        } else if get_or_compute_stats(dom, previous, store).text_length > 0 {
            return false;
        }
        sibling = dom.prev_sibling(previous);
    }
    saw_header
}

struct ChromeMetrics<'a> {
    name: &'a str,
    text: &'a str,
    stats: NodeStats,
    links: usize,
    link_density: f64,
    non_link_chars: f64,
    repeated: bool,
    has_meaningful_media: bool,
    has_content_structure: bool,
    at_start: bool,
    at_end: bool,
    adjacent_content: bool,
}

fn has_meaningful_heading(dom: &Dom, node: NodeId) -> bool {
    dom.descendants(node).any(|descendant| {
        matches!(
            dom.tag(descendant),
            Some(Tag::H1 | Tag::H2 | Tag::H3 | Tag::H4 | Tag::H5 | Tag::H6)
        ) && {
            let mut heading = String::new();
            dom.append_normalized_text_limited(descendant, &mut heading, 256);
            !equals_any_ascii_case_insensitive(
                heading.trim(),
                &["menu", "navigation", "sections", "contents", "on this page"],
            )
        }
    })
}

fn is_global_navigation(
    dom: &Dom,
    node: NodeId,
    root: NodeId,
    metrics: &ChromeMetrics<'_>,
) -> bool {
    if !matches!(
        dom.tag(node),
        Some(Tag::Aside | Tag::Div | Tag::Header | Tag::Nav | Tag::Ol | Tag::Section | Tag::Ul)
    ) || dom.tag(node) == Some(Tag::Header)
        && (has_meaningful_heading(dom, node)
            || contains_any(metrics.name, &["page-head", "article-head", "post-head"]))
    {
        return false;
    }
    if is_inside_explicit_article_body(dom, node, root)
        || is_meaningful_article_region(dom, node, metrics)
        || is_content_relative_navigation(dom, node)
        || is_document_toc(dom, node, metrics.name)
        || is_within_pricing_region(dom, node)
        || has_pricing_content(dom, node, metrics.text)
        || metrics.has_meaningful_media
        || has_meaningful_heading(dom, node)
    {
        return false;
    }
    let semantic = dom.tag(node) == Some(Tag::Nav)
        || dom.tag(node) == Some(Tag::Header) && metrics.links >= 3
        || dom
            .attr(node, AttrName::Role)
            .is_some_and(|roles| has_token(roles, "navigation"));
    let named = contains_any(
        metrics.name,
        &[
            "global-nav",
            "global_navigation",
            "site-nav",
            "site_navigation",
            "navbar",
            "navigation",
            "main-menu",
            "main_menu",
            "site-menu",
            "site_menu",
            "sidebar-nav",
            "sidebar_navigation",
            "docs-nav",
            "docs_navigation",
            "menu",
        ],
    );
    let low_prose = metrics.stats.sentence_end_count <= 4 && metrics.non_link_chars <= 360.0;
    let pagination = metrics.at_end
        && metrics.links >= 1
        && (dom
            .attr(node, AttrName::AriaLabel)
            .is_some_and(is_pagination_label)
            || [
                "previous post",
                "next post",
                "previous article",
                "next article",
            ]
            .iter()
            .any(|label| metrics.text.contains(label)));
    let compact = metrics.links >= 3 && metrics.link_density >= 0.35
        || metrics.repeated && metrics.links >= 2 && metrics.link_density >= 0.45;
    let positioned = metrics.at_start || metrics.at_end || metrics.adjacent_content;
    let navigation_name = contains_any(
        metrics.name,
        &[
            "navigation",
            "navbar",
            "nav-",
            "nav_",
            "menu",
            "sidebar",
            "site-links",
        ],
    );
    let structural = (semantic || named) && positioned
        || metrics.repeated
            && navigation_name
            && !metrics.has_meaningful_media
            && !metrics.has_content_structure
            && (metrics.at_start || metrics.at_end);
    structural && (compact || pagination) && low_prose
}

fn is_global_footer(dom: &Dom, node: NodeId, root: NodeId, metrics: &ChromeMetrics<'_>) -> bool {
    if !metrics.at_end
        || !matches!(
            dom.tag(node),
            Some(Tag::Aside | Tag::Div | Tag::Footer | Tag::Header | Tag::Other | Tag::Section)
        )
        || matches!(dom.tag(node), Some(Tag::Article | Tag::Main))
        || is_inside_explicit_article_body(dom, node, root)
        || is_meaningful_article_region(dom, node, metrics)
        || is_within_pricing_region(dom, node)
        || has_pricing_content(dom, node, metrics.text)
        || metrics.has_meaningful_media
    {
        return false;
    }
    let semantic = dom.tag(node) == Some(Tag::Footer)
        || dom
            .attr(node, AttrName::Role)
            .is_some_and(|roles| has_token(roles, "contentinfo"));
    let named_contact = metrics.name.contains("contact")
        && !has_meaningful_heading(dom, node)
        && !has_substantive_prose(dom, node);
    let named = metrics.name.contains("footer")
        || named_contact
        || metrics.name.contains("legal")
        || metrics.name.contains("site-links")
        || metrics.name.contains("site_links");
    let footer_text = contains_any(
        metrics.text,
        &[
            "privacy",
            "terms of service",
            "terms and conditions",
            "cookie policy",
            "all rights reserved",
            "copyright",
            "contact us",
            "follow us",
            "sitemap",
        ],
    );
    let low_prose = metrics.stats.sentence_end_count <= 4 && metrics.non_link_chars <= 480.0;
    let link_cluster = metrics.links >= 2 && metrics.link_density >= 0.2;
    let mut contact_text = String::new();
    let contact_link = dom.descendants(node).any(|descendant| {
        if dom.tag(descendant) != Some(Tag::A) {
            return false;
        }
        contact_text.clear();
        contains_ascii_case_insensitive(
            get_normalized_inner_text(dom, descendant, &mut contact_text),
            "contact",
        )
    });
    let global_structure = semantic || named;
    low_prose
        && ((global_structure && (link_cluster || footer_text || contact_link))
            || (footer_text && metrics.links >= 2 && metrics.link_density >= 0.15))
}

fn is_leading_metadata(
    dom: &Dom,
    node: NodeId,
    metrics: &ChromeMetrics<'_>,
    dominant_content_sibling: bool,
) -> bool {
    let inside_article_header = is_inside_article_header(dom, node);
    if (!metrics.at_start && !inside_article_header)
        || metrics.stats.text_length > 420
        || metrics.stats.sentence_end_count > 3
        || metrics.has_meaningful_media
        || has_substantive_prose(dom, node)
        || std::iter::once(node)
            .chain(dom.descendants(node))
            .any(|descendant| matches!(dom.tag(descendant), Some(Tag::Pre | Tag::Table)))
    {
        return false;
    }
    let named = has_metadata_name(metrics.name);
    let structural = matches!(dom.tag(node), Some(Tag::Address | Tag::Time))
        || metrics.links >= 1
            && dom
                .descendants(node)
                .any(|descendant| dom.tag(descendant) == Some(Tag::Time));
    let header_navigation_label = metrics.links == 1
        && metrics.stats.word_count <= 4
        && matches!(metrics.text, "blog" | "changelog" | "news")
        && dom
            .parent(node)
            .is_some_and(|parent| dom.tag(parent) == Some(Tag::Header));
    (named || structural || header_navigation_label)
        && (dominant_content_sibling
            || metrics.adjacent_content
            || inside_article_header
            || dom.parent(node).is_some_and(|parent| {
                matches!(dom.tag(parent), Some(Tag::Header | Tag::Aside))
                    || node_name(dom, parent).contains("header")
            }))
}

fn is_inside_article_header(dom: &Dom, node: NodeId) -> bool {
    dom.ancestors(node).take(8).any(|ancestor| {
        dom.tag(ancestor) == Some(Tag::Header)
            || contains_any(
                &node_name(dom, ancestor),
                &[
                    "article-head",
                    "article_head",
                    "articleheader",
                    "page-head",
                    "pageheader",
                    "post-head",
                    "postheader",
                ],
            )
    })
}

fn has_metadata_name(name: &str) -> bool {
    name.split(|character: char| !character.is_ascii_alphanumeric())
        .any(|token| {
            matches!(
                token,
                "author"
                    | "authors"
                    | "byline"
                    | "category"
                    | "categories"
                    | "date"
                    | "datepublished"
                    | "dates"
                    | "kicker"
                    | "meta"
                    | "metadata"
                    | "postmeta"
                    | "published"
                    | "taxonomy"
            )
        })
}

fn is_leading_page_frame(
    dom: &Dom,
    node: NodeId,
    metrics: &ChromeMetrics<'_>,
    dominant_content_sibling: bool,
    leading_position: bool,
) -> bool {
    if dom.tag(node) == Some(Tag::Header)
        || contains_any(
            metrics.name,
            &[
                "article-head",
                "article_head",
                "articleheader",
                "page-head",
                "pageheader",
                "post-head",
                "postheader",
            ],
        )
        || has_pricing_content(dom, node, metrics.text)
        || is_document_toc(dom, node, metrics.name)
        || has_substantive_prose(dom, node)
        || std::iter::once(node)
            .chain(dom.ancestors(node))
            .any(|ancestor| matches!(dom.tag(ancestor), Some(Tag::Code | Tag::Pre)))
        || metrics
            .name
            .split(|character: char| !character.is_ascii_alphanumeric())
            .any(|token| token == "pre")
        || std::iter::once(node)
            .chain(dom.descendants(node))
            .any(|descendant| {
                matches!(
                    dom.tag(descendant),
                    Some(Tag::Figure | Tag::Pre | Tag::Table)
                )
            })
    {
        return false;
    }
    leading_position
        && dominant_content_sibling
        && metrics.stats.text_length <= 800
        && metrics.stats.sentence_end_count <= 6
        && metrics.links >= 3
        && metrics.non_link_chars <= 520.0
        && (metrics.link_density >= 0.2 || metrics.links >= 5)
}

fn has_compact_content_identity(dom: &Dom, node: NodeId, links: usize) -> bool {
    if links > 5 {
        return false;
    }
    let mut has_heading = false;
    let mut has_described_image = false;
    let mut has_summary = false;
    for descendant in std::iter::once(node).chain(dom.descendants(node)).take(128) {
        has_heading |= matches!(dom.tag(descendant), Some(Tag::H1 | Tag::H2));
        has_described_image |= dom.tag(descendant) == Some(Tag::Img)
            && dom
                .attr(descendant, AttrName::Alt)
                .is_some_and(|alt| !alt.trim().is_empty());
        if dom.tag(descendant) == Some(Tag::P) {
            let mut text = String::new();
            dom.append_normalized_text_limited(descendant, &mut text, 281);
            has_summary |= (10..=280).contains(&text.trim().chars().count());
        }
        if has_summary && (has_heading || has_described_image) {
            return true;
        }
    }
    has_summary && (has_heading || has_described_image)
}

fn is_content_relative_navigation(dom: &Dom, node: NodeId) -> bool {
    let name = node_name(dom, node);
    name.split(|character: char| !character.is_ascii_alphanumeric())
        .any(|token| token == "hlist" || token.starts_with("navbox") || token.starts_with("portal"))
        || dom
            .ancestors(node)
            .any(|ancestor| dom.tag(ancestor) == Some(Tag::Table))
        || dom
            .descendants(node)
            .any(|descendant| dom.tag(descendant) == Some(Tag::Table))
}

fn is_document_toc(dom: &Dom, node: NodeId, name: &str) -> bool {
    let labelled = dom.attr(node, AttrName::AriaLabel).is_some_and(|label| {
        equals_any_ascii_case_insensitive(
            label.trim(),
            &["on this page", "table of contents", "contents", "toc"],
        )
    });
    let named = contains_any(
        name,
        &["table-of-contents", "table_of_contents", "docs-toc", "toc"],
    );
    if labelled || named {
        return true;
    }
    // One traversal answers both remaining questions: whether a heading says
    // this is a table of contents, and whether every link is a fragment.
    let mut heading_matches = false;
    let mut links = 0usize;
    let mut non_fragment_link = false;
    for (index, descendant) in dom.descendants(node).enumerate() {
        if index >= 512 {
            return false;
        }
        let tag = dom.tag(descendant);
        if !heading_matches && matches!(tag, Some(Tag::H2 | Tag::H3 | Tag::H4 | Tag::H5 | Tag::H6))
        {
            let mut text = String::new();
            dom.append_normalized_text_limited(descendant, &mut text, 128);
            heading_matches = equals_any_ascii_case_insensitive(
                text.trim(),
                &[
                    "contents",
                    "on this page",
                    "table of contents",
                    "in this article",
                ],
            );
            if heading_matches {
                return true;
            }
        }
        if tag == Some(Tag::A) {
            links += 1;
            non_fragment_link |= !dom
                .attr(descendant, AttrName::Href)
                .is_some_and(|href| href.trim_start().starts_with('#'));
        }
    }
    links > 0 && !non_fragment_link
}

fn has_explicit_document_toc_label(dom: &Dom, node: NodeId, name: &str) -> bool {
    dom.attr(node, AttrName::AriaLabel).is_some_and(|label| {
        equals_any_ascii_case_insensitive(
            label.trim(),
            &[
                "on this page",
                "table of contents",
                "contents",
                "toc",
                "in this article",
            ],
        )
    }) || contains_any(
        name,
        &[
            "table-of-contents",
            "table_of_contents",
            "docs-toc",
            "reference-toc",
        ],
    ) || name_has_token(name, "toc")
        || dom
            .descendants(node)
            .take(48)
            .any(|descendant| is_document_toc_heading(dom, descendant))
}

fn is_document_toc_heading(dom: &Dom, node: NodeId) -> bool {
    matches!(
        dom.tag(node),
        Some(Tag::H2 | Tag::H3 | Tag::H4 | Tag::H5 | Tag::H6)
    ) && {
        let mut text = String::new();
        dom.append_normalized_text_limited(node, &mut text, 128);
        equals_any_ascii_case_insensitive(
            text.trim(),
            &[
                "contents",
                "on this page",
                "table of contents",
                "in this article",
            ],
        )
    }
}

fn is_compact_link_index_heading(dom: &Dom, node: NodeId) -> bool {
    matches!(dom.tag(node), Some(Tag::H3 | Tag::H4 | Tag::H5 | Tag::H6)) && {
        let mut text = String::new();
        dom.append_normalized_text_limited(node, &mut text, 96);
        equals_any_ascii_case_insensitive(
            text.trim(),
            &[
                "news",
                "recent posts",
                "latest articles",
                "popular posts",
                "most read",
            ],
        )
    }
}

fn is_redundant_document_toc(
    dom: &Dom,
    node: NodeId,
    root: NodeId,
    metrics: &PeripheralMetrics<'_>,
) -> bool {
    if metrics.links < 2
        || metrics.stats.text_length >= 4_000
        || metrics.link_density < 0.15
        || dom
            .attr(root, AttrName::Class)
            .is_some_and(|class| has_token(class, "mw-parser-output"))
        || !has_explicit_document_toc_label(dom, node, metrics.name)
        || !is_document_toc(dom, node, metrics.name)
        || !std::iter::once(node)
            .chain(dom.descendants(node))
            .take(256)
            .any(|descendant| matches!(dom.tag(descendant), Some(Tag::Nav | Tag::Ol | Tag::Ul)))
    {
        return false;
    }

    let mut headings = 0_u8;
    for (index, candidate) in dom.descendants(root).enumerate() {
        if index >= 512 {
            return false;
        }
        if matches!(
            dom.tag(candidate),
            Some(Tag::H1 | Tag::H2 | Tag::H3 | Tag::H4 | Tag::H5 | Tag::H6)
        ) && candidate != node
            && is_proven_outside_subtree(dom, candidate, node)
        {
            headings += 1;
            if headings >= 2 {
                return true;
            }
        }
    }
    false
}

fn is_proven_outside_subtree(dom: &Dom, candidate: NodeId, subtree: NodeId) -> bool {
    for (index, ancestor) in dom.ancestors(candidate).enumerate() {
        if ancestor == subtree {
            return false;
        }
        if index >= 63 {
            return false;
        }
    }
    true
}

fn is_compact_layout_side_rail(
    dom: &Dom,
    node: NodeId,
    metrics: &PeripheralMetrics<'_>,
    protected: bool,
) -> bool {
    dom.tag(node) == Some(Tag::Aside)
        && !protected
        && !is_inside_primary_content_container_bounded(dom, node)
        && (!dom
            .descendants(node)
            .take(256)
            .any(|descendant| dom.tag(descendant) == Some(Tag::P))
            || contains_any(metrics.name, &["sidebar", "side-rail", "side_rail", "rail"]))
        && (metrics.at_start || metrics.at_end)
        && metrics.stats.text_length < 700
        && metrics.stats.sentence_end_count <= 2
        && !has_substantive_prose_bounded(dom, node)
        && !std::iter::once(node)
            .chain(dom.descendants(node))
            .take(256)
            .any(|descendant| {
                matches!(
                    dom.tag(descendant),
                    Some(Tag::Blockquote | Tag::Figure | Tag::Pre | Tag::Table)
                )
            })
}

fn is_compact_inline_promotion(
    dom: &Dom,
    node: NodeId,
    metrics: &PeripheralMetrics<'_>,
    protected: bool,
) -> bool {
    matches!(dom.tag(node), Some(Tag::Aside | Tag::Div | Tag::Section))
        && !protected
        && contains_any(metrics.name, &["promotion", "promo", "paywall", "upsell"])
        && metrics.stats.text_length < 500
        && metrics.stats.sentence_end_count <= 2
        && (metrics.controls > 0 || metrics.links > 0 || metrics.has_form)
        && !has_substantive_prose_bounded(dom, node)
}

fn is_link_index_rail(dom: &Dom, node: NodeId, metrics: &PeripheralMetrics<'_>) -> bool {
    let layout_peer = has_substantive_layout_peer(dom, node);
    let inside_primary = is_inside_primary_content_container_bounded(dom, node);
    let explicit_peripheral = ["sidebar", "rail"]
        .iter()
        .any(|token| name_has_token(metrics.name, token));
    if !matches!(dom.tag(node), Some(Tag::Aside | Tag::Div | Tag::Section))
        || inside_primary && !explicit_peripheral
        || !(metrics.at_start || metrics.at_end || layout_peer)
        || metrics.links < 4
        || metrics.stats.text_length >= 1_200
        || metrics.link_density < 0.55
        || metrics.stats.sentence_end_count > 3
    {
        return false;
    }
    dom.descendants(node).take(48).any(|heading| {
        matches!(
            dom.tag(heading),
            Some(Tag::H2 | Tag::H3 | Tag::H4 | Tag::H5 | Tag::H6)
        ) && {
            let mut text = String::new();
            dom.append_normalized_text_limited(heading, &mut text, 96);
            equals_any_ascii_case_insensitive(
                text.trim(),
                &[
                    "news",
                    "recent posts",
                    "latest articles",
                    "popular posts",
                    "most read",
                ],
            )
        }
    })
}

fn has_substantive_layout_peer(dom: &Dom, node: NodeId) -> bool {
    let mut branch = node;
    for parent in dom.ancestors(node).take(3) {
        for (index, sibling) in dom.element_children(parent).enumerate() {
            if index >= 16 {
                return false;
            }
            if sibling == branch {
                continue;
            }
            for (descendant_index, descendant) in dom.descendants(sibling).enumerate() {
                if descendant_index >= 256 {
                    return false;
                }
                if dom.tag(descendant) == Some(Tag::P)
                    && dom.normalized_char_count(descendant) >= 120
                {
                    return true;
                }
            }
        }
        branch = parent;
    }
    false
}

fn has_substantive_prose_bounded(dom: &Dom, node: NodeId) -> bool {
    for (index, descendant) in dom.descendants(node).enumerate() {
        if index >= 256 {
            return true;
        }
        if matches!(dom.tag(descendant), Some(Tag::Blockquote | Tag::P))
            && dom.normalized_char_count(descendant) >= 80
        {
            return true;
        }
    }
    false
}

fn is_inside_primary_content_container_bounded(dom: &Dom, node: NodeId) -> bool {
    for (index, ancestor) in std::iter::once(node).chain(dom.ancestors(node)).enumerate() {
        if index >= 64 {
            return true;
        }
        if matches!(dom.tag(ancestor), Some(Tag::Article | Tag::Main))
            || dom
                .attr(ancestor, AttrName::Role)
                .is_some_and(|roles| has_any_token(roles, &["article", "main"]))
        {
            return true;
        }
    }
    false
}

fn has_pricing_heading(dom: &Dom, node: NodeId) -> bool {
    dom.descendants(node).any(|descendant| {
        matches!(
            dom.tag(descendant),
            Some(Tag::H1 | Tag::H2 | Tag::H3 | Tag::H4 | Tag::H5 | Tag::H6)
        ) && {
            let mut heading = String::new();
            dom.append_normalized_text_limited(descendant, &mut heading, 256);
            let heading = heading.trim();
            equals_any_ascii_case_insensitive(heading, &["pricing", "plans", "pricing plans"])
                || ends_ascii_case_insensitive(heading, " plans")
                || contains_ascii_case_insensitive(heading, "pricing")
        }
    })
}

fn has_pricing_content(dom: &Dom, node: NodeId, text: &str) -> bool {
    let pricing_heading = has_pricing_heading(dom, node);
    if pricing_heading {
        return true;
    }
    // Pricing content always shows a currency symbol next to digits. Check
    // that cheap signal first so unrelated nodes skip every substring scan.
    let bytes = text.as_bytes();
    let has_currency = if text.is_ascii() {
        memchr::memchr(b'$', bytes).is_some()
    } else {
        text.chars()
            .any(|character| matches!(character, '$' | '€' | '£' | '¥'))
    } && bytes.iter().any(|byte| byte.is_ascii_digit());
    if !has_currency {
        return false;
    }
    let price_words = ["pricing", "price", "plan", "monthly", "annual", "per month"]
        .into_iter()
        .filter(|word| text.contains(word))
        .count();
    let price_period = ["/month", "/year", "/mo", "/yr", "per month", "per year"]
        .into_iter()
        .any(|period| text.contains(period));
    price_words >= 2 || price_words >= 1 && price_period
}

fn is_within_pricing_region(dom: &Dom, node: NodeId) -> bool {
    std::iter::once(node)
        .chain(dom.ancestors(node))
        .take(8)
        .any(|ancestor| {
            let name = node_name(dom, ancestor);
            contains_any(&name, &["pricing", "price-table", "plan-grid", "plans"])
                || dom.element_children(ancestor).any(|child| {
                    matches!(
                        dom.tag(child),
                        Some(Tag::H1 | Tag::H2 | Tag::H3 | Tag::H4 | Tag::H5 | Tag::H6)
                    ) && {
                        let mut heading = String::new();
                        dom.append_normalized_text_limited(child, &mut heading, 256);
                        let heading = heading.trim();
                        equals_any_ascii_case_insensitive(
                            heading,
                            &["pricing", "plans", "pricing plans"],
                        ) || ends_ascii_case_insensitive(heading, " plans")
                            || contains_ascii_case_insensitive(heading, "pricing")
                    }
                })
        })
}

fn is_inside_explicit_article_body(dom: &Dom, node: NodeId, _root: NodeId) -> bool {
    std::iter::once(node)
        .chain(dom.ancestors(node))
        .any(|ancestor| {
            dom.attr(ancestor, AttrName::ItemProp)
                .is_some_and(|value| has_any_token(value, &["articleBody", "text"]))
        })
}

fn is_meaningful_article_region(dom: &Dom, node: NodeId, metrics: &ChromeMetrics<'_>) -> bool {
    is_inside_article_container(dom, node)
        && (metrics.has_meaningful_media || has_meaningful_region_content(dom, node))
}

fn is_inside_article_container(dom: &Dom, node: NodeId) -> bool {
    std::iter::once(node)
        .chain(dom.ancestors(node))
        .any(|ancestor| {
            dom.tag(ancestor) == Some(Tag::Article)
                || dom
                    .attr(ancestor, AttrName::Role)
                    .is_some_and(|roles| has_token(roles, "article"))
        })
}

fn is_inside_primary_content_container(dom: &Dom, node: NodeId) -> bool {
    std::iter::once(node)
        .chain(dom.ancestors(node))
        .any(|ancestor| {
            matches!(dom.tag(ancestor), Some(Tag::Article | Tag::Main))
                || dom
                    .attr(ancestor, AttrName::Role)
                    .is_some_and(|roles| has_any_token(roles, &["article", "main"]))
        })
}

fn has_meaningful_region_content(dom: &Dom, node: NodeId) -> bool {
    has_substantive_prose(dom, node)
        || std::iter::once(node)
            .chain(dom.descendants(node))
            .any(|descendant| {
                matches!(
                    dom.tag(descendant),
                    Some(Tag::Blockquote | Tag::Figure | Tag::Pre | Tag::Table)
                )
            })
}

fn hoist_footer_identity(dom: &mut Dom, footer: NodeId) {
    let children: Vec<_> = dom.children(footer).collect();
    for child in children {
        if is_inside_pagination_navigation(dom, child, footer) {
            continue;
        }
        if dom.is_text(child) {
            if dom.text_node(child).is_some_and(is_footer_identity_text) {
                dom.insert_before(footer, child);
                return;
            }
            continue;
        }
        if dom
            .descendants(child)
            .all(|descendant| dom.tag(descendant) != Some(Tag::A))
            && is_footer_identity_node(dom, child)
        {
            dom.insert_before(footer, child);
            return;
        }
    }

    let links: Vec<_> = dom
        .descendants(footer)
        .filter(|&node| {
            dom.tag(node) == Some(Tag::A) && !is_inside_pagination_navigation(dom, node, footer)
        })
        .collect();
    let mut text_buffer = String::new();
    if let Some(link) = links.into_iter().find(|&link| {
        let text = get_normalized_inner_text(dom, link, &mut text_buffer);
        is_footer_identity_text(text)
    }) {
        dom.insert_before(footer, link);
    }
}

fn is_pagination_label(label: &str) -> bool {
    label
        .split(|character: char| !character.is_ascii_alphanumeric())
        .any(|token| equals_any_ascii_case_insensitive(token, &["pagination", "pager"]))
}

fn is_pagination_navigation_node(dom: &Dom, node: NodeId) -> bool {
    dom.attr(node, AttrName::AriaLabel)
        .is_some_and(is_pagination_label)
        || contains_any(&node_name(dom, node), &["pagination", "pager"])
}

fn is_inside_pagination_navigation(dom: &Dom, node: NodeId, footer: NodeId) -> bool {
    for ancestor in std::iter::once(node).chain(dom.ancestors(node)) {
        if is_pagination_navigation_node(dom, ancestor) {
            return true;
        }
        if ancestor == footer {
            break;
        }
    }
    false
}

fn is_footer_identity_node(dom: &Dom, node: NodeId) -> bool {
    if matches!(
        dom.tag(node),
        Some(Tag::H1 | Tag::H2 | Tag::H3 | Tag::H4 | Tag::H5 | Tag::H6)
    ) {
        return false;
    }
    let mut text_buffer = String::new();
    let text = get_normalized_inner_text(dom, node, &mut text_buffer);
    is_footer_identity_text(text)
}

fn is_footer_identity_text(text: &str) -> bool {
    let text = text.trim();
    let ui_label = equals_any_ascii_case_insensitive(
        text,
        &[
            "about",
            "about me",
            "home",
            "privacy",
            "privacy policy",
            "terms",
            "terms of service",
            "contact",
            "contact us",
            "copyright",
            "cookie policy",
            "sitemap",
            "imprint",
            "presskit",
            "faq",
            "rss",
            "jobs",
            "subscribe",
            "newsletter",
            "follow us",
            "learn more",
            "read more",
            "view details",
            "more",
        ],
    );
    let boilerplate = [
        "all rights reserved",
        "privacy policy",
        "terms of service",
        "cookie policy",
    ]
    .iter()
    .any(|needle| contains_ascii_case_insensitive(text, needle));
    (2..=100).contains(&text.chars().count())
        && !ui_label
        && !boilerplate
        && text.split_ascii_whitespace().count() <= 8
}

fn remove_explicit_peripheral_sections(
    dom: &mut Dom,
    root: NodeId,
    snapshot: &[(NodeId, u32)],
    link_counts: &[u8],
    store: &mut crate::dom::NodeStateStore,
    has_responsive_pair: bool,
) -> bool {
    let terminal_related = snapshot
        .iter()
        .enumerate()
        .rev()
        .find_map(|(index, &(node, _))| {
            let name = node_name(dom, node);
            let strong_heading =
                related_heading_signal_in(dom, node) == RelatedHeadingSignal::Strong;
            let explicit_name = name.contains("related")
                && contains_any(&name, &["articles", "cards", "grid", "stories"]);
            (strong_heading
                && (explicit_name || has_repeated_link_cards(dom, node))
                && link_counts[node.index()] >= 3
                && near_content_end(dom, node, root, store))
            .then_some((index, node))
        });

    let mut changed = false;
    let mut detached_depth = None;
    for (index, &(node, depth)) in snapshot.iter().enumerate() {
        if detached_depth.is_some_and(|outer_depth| depth > outer_depth) {
            continue;
        }
        detached_depth = None;
        if dom.parent(node).is_none() {
            continue;
        }

        let name = node_name(dom, node);
        let article_toc = dom.attr(node, AttrName::DataArticleToc).is_some();
        let related_component = name.contains("related-content-tout")
            || name.contains("related")
                && contains_any(&name, &["articles", "cards", "grid", "stories"]);
        let audio_player = contains_any(&name, &["audio-player", "audio_player"]);
        let article_meta = name.contains("article-hero__meta-aside");
        let share_controls = name.contains("share-dropdown");
        let newsletter_component = name.contains("article-newsletter");
        if !article_toc
            && !related_component
            && !audio_player
            && !article_meta
            && !share_controls
            && !newsletter_component
        {
            continue;
        }
        let stats = get_or_compute_stats(dom, node, store);
        let mut text = String::new();
        append_bounded_text(dom, node, 256, &mut text);
        text.make_ascii_lowercase();
        let text = text.trim();
        let action_link = dom.descendants(node).any(|descendant| {
            dom.attr(descendant, AttrName::Href)
                .is_some_and(|href| !href.trim().is_empty())
        });
        let image = dom
            .descendants(node)
            .any(|descendant| dom.tag(descendant) == Some(Tag::Img));
        let audio = dom
            .descendants(node)
            .any(|descendant| matches!(dom.tag(descendant), Some(Tag::Audio | Tag::Source)));
        let custom_form = dom.descendants(node).any(|descendant| {
            dom.tag(descendant) == Some(Tag::Other)
                && dom.attr(descendant, AttrName::Action).is_some()
                && node_name(dom, descendant).contains("newsletter-form")
        });

        let author_promotion = article_toc
            && stats.text_length < 1_200
            && action_link
            && image
            && text.contains("the latest from ")
            && text.contains("monthly")
            && contains_any(text, &["news", "updates", "newsletter"]);
        let collection = stats.text_length < 800
            && name.contains("related-content-tout")
            && action_link
            && (text.starts_with("collection ") || text == "collection")
            && terminal_related.is_some_and(|(related_index, related_node)| {
                related_index > index
                    && starts_terminal_peripheral_sequence(dom, node, related_node, root, store)
            });
        let related_cards = terminal_related.is_some_and(|(_, related_node)| {
            related_node == node
                || related_heading_signal_in(dom, node) == RelatedHeadingSignal::Strong
                    && link_counts[node.index()] >= 2
                    && has_repeated_link_cards(dom, node)
                    && starts_terminal_peripheral_sequence(dom, node, related_node, root, store)
        });
        let audio_controls = audio_player
            && stats.text_length < 500
            && (audio || action_link)
            && contains_any(
                text,
                &[
                    "listen to article",
                    "listen to this article",
                    "[[duration]]",
                ],
            );
        let meta_artifact =
            article_meta && stats.text_length < 100 && text.contains('|') && looks_like_date(text);
        let share = share_controls
            && stats.text_length < 500
            && text.starts_with("share")
            && (link_counts[node.index()] >= 2 || text == "share");
        let newsletter = newsletter_component
            && stats.text_length < 1_200
            && custom_form
            && text.contains("in your inbox")
            && text.contains("sign up for our newsletter");

        if author_promotion
            || collection
            || related_cards
            || audio_controls
            || meta_artifact
            || share
            || newsletter
        {
            detach_and_invalidate_stats(dom, node, store);
            changed = true;
            detached_depth = Some(depth);
        }
    }
    let responsive_changed =
        has_responsive_pair && remove_responsive_duplicate_views(dom, snapshot, store);
    changed | responsive_changed
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum ResponsiveVisibility {
    Narrow,
    Wide,
}

fn responsive_visibility(dom: &Dom, node: NodeId) -> Option<ResponsiveVisibility> {
    let class = dom.attr(node, AttrName::Class)?;
    let mut hidden = false;
    let mut narrow_hidden = false;
    let mut wide_display = false;
    for token in class.split_ascii_whitespace() {
        hidden |= token == "hidden";
        narrow_hidden |= responsive_variant(token, ":hidden");
        wide_display |= [":block", ":table", ":flex", ":grid"]
            .iter()
            .any(|suffix| responsive_variant(token, suffix));
    }
    if hidden && wide_display {
        Some(ResponsiveVisibility::Wide)
    } else if narrow_hidden && !hidden {
        Some(ResponsiveVisibility::Narrow)
    } else {
        None
    }
}

fn responsive_variant(token: &str, suffix: &str) -> bool {
    token.strip_suffix(suffix).is_some_and(|prefix| {
        matches!(prefix, "sm" | "md" | "lg" | "xl" | "2xl")
            || prefix.starts_with("min-[")
            || prefix.starts_with("max-[")
    })
}

fn next_element_sibling(dom: &Dom, node: NodeId) -> Option<NodeId> {
    let mut sibling = dom.next_sibling(node);
    while let Some(candidate) = sibling {
        if dom.is_element(candidate) {
            return Some(candidate);
        }
        sibling = dom.next_sibling(candidate);
    }
    None
}

fn remove_responsive_duplicate_views(
    dom: &mut Dom,
    snapshot: &[(NodeId, u32)],
    store: &mut crate::dom::NodeStateStore,
) -> bool {
    let mut changed = false;
    for &(first, _) in snapshot {
        if dom.parent(first).is_none() {
            continue;
        }
        let Some(first_visibility) = responsive_visibility(dom, first) else {
            continue;
        };
        let Some(second) = next_element_sibling(dom, first) else {
            continue;
        };
        let Some(second_visibility) = responsive_visibility(dom, second) else {
            continue;
        };
        if first_visibility == second_visibility {
            continue;
        }
        let (alternate, table_view) = if dom.any_descendant_by_tags(first, &[Tag::Table])
            && !dom.any_descendant_by_tags(second, &[Tag::Table])
        {
            (second, first)
        } else if dom.any_descendant_by_tags(second, &[Tag::Table])
            && !dom.any_descendant_by_tags(first, &[Tag::Table])
        {
            (first, second)
        } else {
            continue;
        };
        if !responsive_table_matches_alternate(dom, table_view, alternate) {
            continue;
        }
        detach_and_invalidate_stats(dom, alternate, store);
        changed = true;
    }
    changed
}

fn responsive_table_matches_alternate(dom: &Dom, table_view: NodeId, alternate: NodeId) -> bool {
    let Some(table) = dom
        .descendants(table_view)
        .find(|&node| dom.tag(node) == Some(Tag::Table))
    else {
        return false;
    };
    let mut keys = SmallVec::<[String; 6]>::new();
    let mut text = String::new();
    for row in dom
        .table_descendants(table)
        .into_iter()
        .filter(|&node| dom.tag(node) == Some(Tag::Tr))
    {
        if dom
            .element_children(row)
            .any(|cell| dom.tag(cell) == Some(Tag::Th))
        {
            continue;
        }
        let Some(cell) = dom
            .element_children(row)
            .find(|&cell| dom.tag(cell) == Some(Tag::Td))
        else {
            continue;
        };
        text.clear();
        dom.append_normalized_text_limited(cell, &mut text, 80);
        let key = text.trim();
        if key.chars().count() >= 3 && !keys.iter().any(|known| known == key) {
            keys.push(key.to_owned());
        }
        if keys.len() == 6 {
            break;
        }
    }
    if keys.len() < 4 {
        return false;
    }
    text.clear();
    dom.append_normalized_text_limited(alternate, &mut text, 8_192);
    keys.iter().all(|key| text.contains(key))
}

fn starts_terminal_peripheral_sequence(
    dom: &Dom,
    first: NodeId,
    last: NodeId,
    root: NodeId,
    store: &mut crate::dom::NodeStateStore,
) -> bool {
    let mut first_path = vec![false; dom.len()];
    first_path[first.index()] = true;
    for ancestor in dom.ancestors(first) {
        first_path[ancestor.index()] = true;
    }
    let common_parent = std::iter::once(last)
        .chain(dom.ancestors(last))
        .find(|node| first_path[node.index()])
        .unwrap_or(root);
    let first_branch = std::iter::once(first)
        .chain(dom.ancestors(first))
        .take_while(|&node| node != common_parent)
        .last()
        .unwrap_or(first);
    let last_branch = std::iter::once(last)
        .chain(dom.ancestors(last))
        .take_while(|&node| node != common_parent)
        .last()
        .unwrap_or(last);
    if first_branch == last_branch {
        return first == last;
    }

    let mut trailing_chars = 0_usize;
    let mut sibling = dom.next_sibling(first_branch);
    while let Some(next) = sibling {
        if next == last_branch {
            return true;
        }
        if is_terminal_sequence_bridge(dom, next, store) {
            sibling = dom.next_sibling(next);
            continue;
        }
        trailing_chars = trailing_chars
            .saturating_add(get_or_compute_stats(dom, next, store).text_length as usize);
        if trailing_chars > 100 {
            return false;
        }
        sibling = dom.next_sibling(next);
    }
    false
}

fn is_terminal_sequence_bridge(
    dom: &Dom,
    node: NodeId,
    store: &mut crate::dom::NodeStateStore,
) -> bool {
    let name = node_name(dom, node);
    if name.contains("footnotes") {
        return true;
    }
    if get_or_compute_stats(dom, node, store).text_length >= 1_200 {
        return false;
    }
    contains_any(
        &name,
        &[
            "article-newsletter",
            "article-tags",
            "audio-player",
            "newsletter-form",
            "share-dropdown",
            "tag-list",
            "taxonomy",
        ],
    ) || dom.descendants(node).take(64).any(|descendant| {
        contains_any(
            &node_name(dom, descendant),
            &["article-newsletter", "newsletter-form"],
        )
    })
}

fn looks_like_date(text: &str) -> bool {
    text.bytes().any(|byte| byte.is_ascii_digit())
        && contains_any(
            text,
            &[
                "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
            ],
        )
}

fn remove_terminal_taxonomy_before_footnotes(
    dom: &mut Dom,
    root: NodeId,
    snapshot: &[(NodeId, u32)],
    link_counts: &[u8],
    store: &mut crate::dom::NodeStateStore,
) -> bool {
    for &(node, _) in snapshot {
        if dom.parent(node).is_none() || !node_name(dom, node).contains("article-tags") {
            continue;
        }
        let stats = get_or_compute_stats(dom, node, store);
        if stats.text_length < 300
            && link_counts[node.index()] >= 2
            && near_content_end_ignoring_footnotes(dom, node, root, store)
        {
            detach_and_invalidate_stats(dom, node, store);
            return true;
        }
    }
    false
}

fn remove_job_company_profiles(
    dom: &mut Dom,
    root: NodeId,
    page_kind: PageKind,
    snapshot: &[(NodeId, u32)],
    store: &mut crate::dom::NodeStateStore,
) -> bool {
    if page_kind != PageKind::JobListing {
        return false;
    }

    let mut field_counts = vec![0_u8; dom.len()];
    let mut founder_counts = vec![0_u8; dom.len()];
    let mut image_counts = vec![0_u8; dom.len()];
    for &(node, _) in snapshot.iter().rev() {
        if dom.element_children(node).next().is_none() && is_profile_field_label(dom, node) {
            field_counts[node.index()] = 1;
        }
        if dom.element_children(node).next().is_none() {
            let mut text = String::new();
            dom.append_normalized_text(node, &mut text);
            founder_counts[node.index()] =
                u8::from(text.trim().to_ascii_lowercase().contains("founder"));
        }
        image_counts[node.index()] = u8::from(dom.tag(node) == Some(Tag::Img));
        if let Some(parent) = dom.parent(node) {
            field_counts[parent.index()] = field_counts[parent.index()]
                .saturating_add(field_counts[node.index()])
                .min(4);
            founder_counts[parent.index()] = founder_counts[parent.index()]
                .saturating_add(founder_counts[node.index()])
                .min(2);
            image_counts[parent.index()] = image_counts[parent.index()]
                .saturating_add(image_counts[node.index()])
                .min(2);
        }
    }

    // Responsive job cards often repeat one founder profile for desktop and
    // mobile. Remove the smallest shared profile wrapper. Keep the nearby
    // application copy and link.
    let mut changed = false;
    let mut blocked_ancestors = vec![false; dom.len()];
    for &(node, _) in snapshot.iter().rev() {
        if dom.parent(node).is_none()
            || blocked_ancestors[node.index()]
            || !matches!(dom.tag(node), Some(Tag::Aside | Tag::Div | Tag::Section))
            || founder_counts[node.index()] < 2
            || image_counts[node.index()] < 2 && !has_repeated_profile_identity(dom, node)
        {
            continue;
        }
        if get_or_compute_stats(dom, node, store).text_length < 800 {
            for ancestor in dom.ancestors(node) {
                blocked_ancestors[ancestor.index()] = true;
            }
            detach_and_invalidate_stats(dom, node, store);
            changed = true;
        }
    }

    // Inspect outer candidates first. A job sidebar can contain one company
    // card followed by founder cards. Remove their shared terminal boundary.
    for &(node, _) in snapshot {
        if dom.parent(node).is_none()
            || !matches!(dom.tag(node), Some(Tag::Aside | Tag::Div | Tag::Section))
            || field_counts[node.index()] < 3
        {
            continue;
        }
        let stats = get_or_compute_stats(dom, node, store);
        if stats.text_length >= 1_600 || !near_content_end(dom, node, root, store) {
            continue;
        }
        detach_and_invalidate_stats(dom, node, store);
        return true;
    }
    changed
}

fn has_repeated_profile_identity(dom: &Dom, node: NodeId) -> bool {
    let mut identities = SmallVec::<[String; 8]>::new();
    for descendant in dom.descendants(node) {
        if dom.element_children(descendant).next().is_some() {
            continue;
        }
        let mut text = String::new();
        dom.append_normalized_text(descendant, &mut text);
        let text = text.trim();
        if !(3..=80).contains(&text.len()) || text.eq_ignore_ascii_case("founder") {
            continue;
        }
        if identities
            .iter()
            .any(|identity| identity.eq_ignore_ascii_case(text))
        {
            return true;
        }
        identities.push(text.to_owned());
    }
    false
}

fn remove_direct_peripheral_siblings(
    dom: &mut Dom,
    root: NodeId,
    snapshot: &[(NodeId, u32)],
    link_counts: &[u8],
    store: &mut crate::dom::NodeStateStore,
    evidence: &crate::document::SourceEvidence,
) -> bool {
    let mut seen = vec![false; dom.len()];
    seen[root.index()] = true;
    let mut parents = vec![root];
    for child in dom.element_children(root) {
        if matches!(dom.tag(child), Some(Tag::Article | Tag::Main))
            && !std::mem::replace(&mut seen[child.index()], true)
        {
            parents.push(child);
        }
    }
    for &(node, _) in snapshot {
        if (related_heading_signal(dom, node) == RelatedHeadingSignal::Strong
            || is_document_toc_heading(dom, node)
            || is_compact_link_index_heading(dom, node)
            || dom.tag(node) == Some(Tag::Form)
            || is_terminal_sequence_candidate(dom, node)
                && terminal_sequence_score(dom, node, link_counts[node.index()]) > 0)
            && let Some(parent) = dom.parent(node)
            && !std::mem::replace(&mut seen[parent.index()], true)
        {
            parents.push(parent);
        }
    }

    let mut changed = false;
    for parent in parents {
        if dom.parent(parent).is_none() && parent != root {
            continue;
        }
        if is_protected_content(dom, parent, evidence)
            || has_protected_ancestor(dom, parent, root, evidence)
        {
            continue;
        }
        changed |= remove_direct_peripheral_children(dom, parent, link_counts, store, evidence);
    }
    changed
}

fn is_terminal_sequence_candidate(dom: &Dom, node: NodeId) -> bool {
    if matches!(dom.tag(node), Some(Tag::Footer | Tag::Form | Tag::Nav)) {
        return true;
    }
    matches!(dom.tag(node), Some(Tag::Aside | Tag::Div | Tag::Section))
        && contains_any(
            &node_name(dom, node),
            &[
                "banner",
                "keep-reading",
                "keep_reading",
                "navbox",
                "newsletter",
                "portal",
                "post-footer",
                "promotion",
                "recommendation",
                "related",
                "subscribe",
            ],
        )
}

fn remove_direct_peripheral_children(
    dom: &mut Dom,
    parent: NodeId,
    link_counts: &[u8],
    store: &mut crate::dom::NodeStateStore,
    evidence: &crate::document::SourceEvidence,
) -> bool {
    let parent_name = node_name(dom, parent);
    let children: Vec<_> = dom.element_children(parent).collect();
    let mut remove = vec![false; children.len()];

    for (index, &child) in children.iter().enumerate() {
        if related_heading_signal(dom, child) != RelatedHeadingSignal::Strong
            || related_name_signal(&parent_name)
        {
            continue;
        }
        let mut links = 0_u8;
        let mut end = index;
        for (offset, &sibling) in children[index + 1..].iter().take(12).enumerate() {
            if matches!(
                dom.tag(sibling),
                Some(Tag::H1 | Tag::H2 | Tag::H3 | Tag::H4 | Tag::H5 | Tag::H6)
            ) {
                break;
            }
            let stats = get_or_compute_stats(dom, sibling, store);
            let sibling_links = link_counts[sibling.index()];
            if is_protected_content(dom, sibling, evidence)
                || sibling_links == 0 && stats.has_non_whitespace()
            {
                break;
            }
            links = links.saturating_add(sibling_links).min(3);
            end = index + offset + 1;
        }
        if links >= 2
            && (!has_heading_text(dom, child, "next steps")
                || has_repeated_link_cards_in_nodes(dom, &children[index + 1..=end]))
        {
            remove[index..=end].fill(true);
        }
    }

    for (index, &child) in children.iter().enumerate() {
        if dom.tag(child) != Some(Tag::Form) || has_explicit_newsletter_name(&parent_name) {
            continue;
        }
        let mut start = index;
        let mut text = String::new();
        for &previous in children[..index].iter().rev().take(3) {
            let tag = dom.tag(previous);
            if !matches!(
                tag,
                Some(Tag::H2 | Tag::H3 | Tag::H4 | Tag::H5 | Tag::H6 | Tag::P)
            ) {
                break;
            }
            start -= 1;
            if matches!(tag, Some(Tag::H2 | Tag::H3 | Tag::H4 | Tag::H5 | Tag::H6)) {
                break;
            }
        }
        if start == index {
            continue;
        }
        for &sibling in &children[start..=index] {
            dom.append_normalized_text(sibling, &mut text);
            text.push(' ');
        }
        let name = node_name(dom, child);
        text.make_ascii_lowercase();
        if has_newsletter_evidence(&name, text.as_str()) {
            remove[start..=index].fill(true);
        }
    }

    mark_terminal_peripheral_sequence(dom, &children, link_counts, store, evidence, &mut remove);

    let mut changed = false;
    for (&node, remove) in children.iter().zip(remove) {
        if remove && dom.parent(node).is_some() {
            if is_protected_content(dom, node, evidence) {
                continue;
            }
            let protected = dom
                .descendants(node)
                .any(|descendant| is_protected_content(dom, descendant, evidence));
            if protected {
                hoist_protected_children(dom, node, store, evidence);
            }
            detach_and_invalidate_stats(dom, node, store);
            changed = true;
        }
    }
    changed
}

fn mark_terminal_peripheral_sequence(
    dom: &Dom,
    children: &[NodeId],
    link_counts: &[u8],
    store: &mut crate::dom::NodeStateStore,
    evidence: &crate::document::SourceEvidence,
    remove: &mut [bool],
) {
    let mut start = children.len();
    let mut signal_nodes = 0_u8;
    let mut saw_signal = false;
    let mut explicit_promotion = false;
    for (index, &child) in children.iter().enumerate().rev() {
        let child_promotion =
            is_explicit_terminal_promotion(dom, child, link_counts[child.index()]);
        if index + 1 == children.len()
            && is_terminal_content_navigation(dom, child, link_counts[child.index()], store)
            && children[..index]
                .iter()
                .map(|&node| get_or_compute_stats(dom, node, store).text_length as usize)
                .sum::<usize>()
                >= 200
        {
            remove[index] = true;
            continue;
        }
        let meaningful_region = is_content_relative_navigation(dom, child)
            || is_inside_article_container(dom, child)
                && has_meaningful_region_content(dom, child)
                && !child_promotion;
        if is_protected_content(dom, child, evidence) || meaningful_region {
            break;
        }
        let child_score = terminal_sequence_score(dom, child, link_counts[child.index()]);
        if child_score > 0 {
            signal_nodes = signal_nodes.saturating_add(1);
            explicit_promotion |= child_promotion;
            saw_signal = true;
            start = index;
        } else if terminal_sequence_bridge(dom, child, store) {
            if saw_signal {
                start = index;
            }
        } else {
            break;
        }
    }
    let single_explicit_related_heading = signal_nodes > 0
        && related_heading_signal_in(dom, children[start]) == RelatedHeadingSignal::Strong
        && link_counts[children[start].index()] >= 2;
    if signal_nodes < 2 && !explicit_promotion && !single_explicit_related_heading
        || start == 0
        || start == children.len()
    {
        return;
    }
    let preceding_text = children[..start]
        .iter()
        .map(|&node| get_or_compute_stats(dom, node, store).text_length as usize)
        .sum::<usize>();
    if preceding_text < 300 {
        return;
    }
    remove[start..].fill(true);
}

fn is_terminal_content_navigation(
    dom: &Dom,
    node: NodeId,
    links: u8,
    store: &mut crate::dom::NodeStateStore,
) -> bool {
    let named = node_name(dom, node)
        .split(|character: char| !character.is_ascii_alphanumeric())
        .any(|token| matches!(token, "navbox" | "portal"));
    if !named || links == 0 {
        return false;
    }
    let stats = get_or_compute_stats(dom, node, store);
    stats.text_length < 600 && get_link_density_cached(dom, node, stats.text_length, store) >= 0.35
}

fn is_explicit_terminal_promotion(dom: &Dom, node: NodeId, links: u8) -> bool {
    if links < 2
        || !std::iter::once(node)
            .chain(dom.descendants(node))
            .any(|descendant| {
                contains_any(
                    &node_name(dom, descendant),
                    &["banner", "promo", "promotion"],
                )
            })
        || !dom
            .descendants(node)
            .any(|descendant| matches!(dom.tag(descendant), Some(Tag::Img | Tag::Table)))
    {
        return false;
    }
    let mut text = String::new();
    dom.append_normalized_text_limited(node, &mut text, 160);
    text.make_ascii_lowercase();
    starts_with_any(
        text.trim(),
        &["our products", "sponsored", "try my ", "try our "],
    )
}

fn terminal_sequence_score(dom: &Dom, node: NodeId, links: u8) -> u8 {
    let mut score = u8::from(matches!(
        dom.tag(node),
        Some(Tag::Footer | Tag::Nav | Tag::Form)
    ));
    let name = node_name(dom, node);
    if contains_any(
        &name,
        &[
            "article-footer",
            "banner",
            "keep-reading",
            "keep_reading",
            "navbox",
            "newsletter",
            "portal",
            "post-footer",
            "promotion",
            "recommendation",
            "related",
            "subscribe",
        ],
    ) {
        score = score.saturating_add(1);
    }
    let mut text = String::new();
    dom.append_normalized_text_limited(node, &mut text, 240);
    text.make_ascii_lowercase();
    let text = text.trim();
    if starts_with_any(
        text,
        &[
            "continue reading",
            "get new posts",
            "here's a preview of a related post",
            "more from ",
            "other posts",
            "preview of a related post",
            "read next",
            "related posts",
            "related stories",
            "subscribe",
            "try my software",
            "you may also like",
        ],
    ) || text.contains("posts you might find similar")
    {
        score = score.saturating_add(1);
    }
    if links > 0
        && (dom.tag(node) == Some(Tag::Blockquote) && text.contains("continue reading")
            || (name.contains("banner") || text.starts_with("try my software"))
                && dom
                    .descendants(node)
                    .any(|descendant| matches!(dom.tag(descendant), Some(Tag::Img | Tag::Table))))
    {
        score = score.saturating_add(1);
    }
    score
}

fn terminal_sequence_bridge(
    dom: &Dom,
    node: NodeId,
    store: &mut crate::dom::NodeStateStore,
) -> bool {
    if dom.tag(node) == Some(Tag::Hr) {
        return true;
    }
    let stats = get_or_compute_stats(dom, node, store);
    stats.text_length == 0
}

#[derive(Clone, Copy, Eq, PartialEq, Ord, PartialOrd)]
enum RelatedHeadingSignal {
    None,
    Ambiguous,
    Strong,
}

struct PeripheralMetrics<'a> {
    name: &'a str,
    text: &'a str,
    stats: NodeStats,
    links: usize,
    controls: usize,
    images: usize,
    has_form: bool,
    link_density: f64,
    at_start: bool,
    at_end: bool,
    short: bool,
}

fn related_heading_signal(dom: &Dom, node: NodeId) -> RelatedHeadingSignal {
    // Heading tags and ARIA headings are common. Do not assemble the
    // lower-cased tag/class/id name for every ordinary element just to learn
    // that it is not a heading. This matters on repaired documents with many
    // nested wrappers.
    let tag_heading = matches!(
        dom.tag(node),
        Some(Tag::H2 | Tag::H3 | Tag::H4 | Tag::H5 | Tag::H6)
    );
    if !tag_heading && dom.attrs(node).is_empty() && dom.tag(node) != Some(Tag::Other) {
        return RelatedHeadingSignal::None;
    }
    let aria_heading = dom
        .attr(node, AttrName::Role)
        .is_some_and(|role| has_token(role, "heading"));
    let named_section_title = if tag_heading || aria_heading {
        false
    } else if dom.attr(node, AttrName::Class).is_some()
        || dom.attr(node, AttrName::Id).is_some()
        || dom.tag(node) == Some(Tag::Other)
    {
        contains_any(&node_name(dom, node), &["section-title", "sectiontitle"])
    } else {
        false
    };
    let semantic_heading = tag_heading || aria_heading || named_section_title;
    if !semantic_heading {
        return RelatedHeadingSignal::None;
    }
    let mut text = String::new();
    dom.append_normalized_text_limited(node, &mut text, 128);
    let text = text.trim();
    if equals_any_ascii_case_insensitive(
        text,
        &[
            "related articles",
            "related content",
            "related posts",
            "related stories",
            "recommended",
            "recommended reading",
            "read next",
            "next steps",
            "more stories",
            "more context",
            "more articles",
            "more posts",
            "you may also like",
            "you might also like",
            "collection",
        ],
    ) || starts_ascii_case_insensitive(text, "more from ")
        && text["more from ".len()..].split_whitespace().count() <= 6
    {
        RelatedHeadingSignal::Strong
    } else if equals_any_ascii_case_insensitive(
        text,
        &["related", "further reading", "see also", "read more"],
    ) {
        RelatedHeadingSignal::Ambiguous
    } else {
        // In particular, keep academic sections such as "Related Work".
        RelatedHeadingSignal::None
    }
}

fn mark_related_heading_boundary(
    dom: &Dom,
    heading: NodeId,
    root: NodeId,
    link_counts: &[u8],
    store: &crate::dom::NodeStateStore,
    boundaries: &mut [bool],
) {
    for candidate in dom
        .ancestors(heading)
        .take_while(|&node| node != root)
        .take(8)
    {
        if !matches!(
            dom.tag(candidate),
            Some(Tag::Aside | Tag::Div | Tag::Footer | Tag::Section)
        ) {
            continue;
        }
        // A heading is discovery evidence only. Keep long reference sections
        // out of the candidate set before the detailed classifier runs.
        if link_counts[candidate.index()] >= 2
            && store
                .get_stats(candidate)
                .is_some_and(|stats| stats.text_length < 1_200)
        {
            boundaries[candidate.index()] = true;
            break;
        }
    }
}

fn append_bounded_text(dom: &Dom, root: NodeId, node_limit: usize, output: &mut String) {
    for node in std::iter::once(root)
        .chain(dom.descendants(root))
        .take(node_limit)
    {
        let Some(text) = dom
            .text_node(node)
            .map(str::trim)
            .filter(|text| !text.is_empty())
        else {
            continue;
        };
        if !output.is_empty() {
            output.push(' ');
        }
        output.push_str(text);
    }
}

fn mark_subscription_boundary(
    dom: &Dom,
    form: NodeId,
    root: NodeId,
    store: &crate::dom::NodeStateStore,
    inspected: &mut [bool],
    boundaries: &mut [bool],
) {
    for candidate in dom.ancestors(form).take_while(|&node| node != root).take(4) {
        if !matches!(
            dom.tag(candidate),
            Some(Tag::Aside | Tag::Div | Tag::Footer | Tag::Section)
        ) {
            continue;
        }
        if std::mem::replace(&mut inspected[candidate.index()], true) {
            if boundaries[candidate.index()] {
                return;
            }
            continue;
        }
        if store
            .get_stats(candidate)
            .is_none_or(|stats| stats.text_length >= 800)
        {
            continue;
        }
        let name = node_name(dom, candidate);
        let mut text = String::new();
        append_bounded_text(dom, candidate, 128, &mut text);
        text.make_ascii_lowercase();
        if !has_newsletter_evidence(&name, &text) {
            continue;
        }
        let has_direct_copy = dom.element_children(candidate).take(12).any(|child| {
            matches!(
                dom.tag(child),
                Some(Tag::H2 | Tag::H3 | Tag::H4 | Tag::H5 | Tag::H6 | Tag::P)
            ) && {
                let mut child_text = String::new();
                dom.append_normalized_text(child, &mut child_text);
                child_text.make_ascii_lowercase();
                has_newsletter_cta_text(&child_text)
            }
        });
        if has_explicit_newsletter_name(&name) || has_direct_copy {
            boundaries[candidate.index()] = true;
            return;
        }
    }
}

fn is_structural_breadcrumb_candidate(dom: &Dom, node: NodeId, inside_table: bool) -> bool {
    let semantic_navigation = dom.tag(node) == Some(Tag::Nav)
        || dom
            .attr(node, AttrName::Role)
            .is_some_and(|roles| has_token(roles, "navigation"));
    if dom.attr(node, AttrName::AriaLabel).is_some_and(|label| {
        equals_any_ascii_case_insensitive(label.trim(), &["breadcrumb", "breadcrumbs"])
    }) {
        return true;
    }
    if !matches!(dom.tag(node), Some(Tag::Div | Tag::Nav | Tag::P)) {
        return false;
    }
    if inside_table {
        return false;
    }
    if dom.element_children(node).any(|child| {
        matches!(
            dom.tag(child),
            Some(Tag::Article | Tag::Div | Tag::P | Tag::Section)
        )
    }) {
        return false;
    }

    // Inspect only a small, shallow prefix. This finds ordinary unlabelled
    // trails without turning candidate discovery into nested subtree scans.
    let mut stack = SmallVec::<[(NodeId, u8); 24]>::new();
    stack.extend(dom.children(node).map(|child| (child, 0)));
    let mut visited = 0usize;
    let mut links = 0usize;
    let mut separator = 0usize;
    while let Some((current, depth)) = stack.pop() {
        visited += 1;
        if visited > 32 {
            return false;
        }
        links += usize::from(dom.tag(current) == Some(Tag::A));
        separator =
            separator.saturating_add(dom.text_node(current).map_or(0, breadcrumb_separator_count));
        if depth < 2 {
            stack.extend(dom.children(current).map(|child| (child, depth + 1)));
        }
    }
    links >= 2 && separator >= 2 && semantic_navigation
}

fn is_structural_peripheral_candidate(
    dom: &Dom,
    node: NodeId,
    page_kind: PageKind,
    store: &crate::dom::NodeStateStore,
) -> bool {
    if !matches!(
        dom.tag(node),
        Some(Tag::Aside | Tag::Div | Tag::Footer | Tag::Header | Tag::Other | Tag::Section)
    ) || store
        .get_stats(node)
        .is_some_and(|stats| stats.text_length >= 1_600)
    {
        return false;
    }

    let name = node_name(dom, node);
    let structural_name = contains_any(
        &name,
        &[
            "audio-player",
            "audio_player",
            "card-grid",
            "card_grid",
            "collection-grid",
            "collection_grid",
            "company-profile",
            "company_profile",
            "founder",
            "news",
            "profile-grid",
            "profile_grid",
            "story-grid",
            "story_grid",
        ],
    );
    let job_name = page_kind == PageKind::JobListing
        && contains_any(
            &name,
            &["card", "company", "employer", "founder", "profile"],
        );
    let promotional_heading = dom.descendants(node).take(48).any(|descendant| {
        if !matches!(
            dom.tag(descendant),
            Some(Tag::H2 | Tag::H3 | Tag::H4 | Tag::H5 | Tag::H6)
        ) {
            return false;
        }
        let mut text = String::new();
        dom.append_normalized_text_limited(descendant, &mut text, 256);
        let text = text.trim();
        equals_any_ascii_case_insensitive(text, &["collection", "company profile", "founders"])
            || starts_ascii_case_insensitive(text, "the latest from ")
            || equals_any_ascii_case_insensitive(
                text,
                &[
                    "news",
                    "recent posts",
                    "latest articles",
                    "popular posts",
                    "most read",
                ],
            ) && dom
                .descendants(node)
                .filter(|&child| dom.tag(child) == Some(Tag::A))
                .take(4)
                .count()
                >= 4
    });

    structural_name || job_name || promotional_heading
}

fn has_breadcrumb_name(dom: &Dom, node: NodeId, name: &str) -> bool {
    name.split(|character: char| !character.is_ascii_alphanumeric())
        .any(|token| matches!(token, "breadcrumb" | "breadcrumbs"))
        || dom.attr(node, AttrName::AriaLabel).is_some_and(|label| {
            equals_any_ascii_case_insensitive(label.trim(), &["breadcrumb", "breadcrumbs"])
        })
}

fn breadcrumb_separator_count(text: &str) -> usize {
    text.chars()
        .filter(|&character| matches!(character, '>' | '/' | '›' | '»'))
        .take(3)
        .count()
}

fn is_breadcrumb(dom: &Dom, node: NodeId, metrics: &PeripheralMetrics<'_>) -> bool {
    let explicit = has_breadcrumb_name(dom, node, metrics.name);
    if !metrics.at_start
        || metrics.links < if explicit { 1 } else { 2 }
        || metrics.stats.text_length > 280
        || metrics.stats.sentence_end_count > 1
    {
        return false;
    }
    let all_fragment_links = dom
        .descendants(node)
        .filter(|&descendant| dom.tag(descendant) == Some(Tag::A))
        .all(|link| {
            dom.attr(link, AttrName::Href)
                .is_some_and(|href| href.starts_with('#'))
        });
    if all_fragment_links {
        return false;
    }

    let navigation = dom.tag(node) == Some(Tag::Nav)
        || dom
            .attr(node, AttrName::Role)
            .is_some_and(|roles| has_token(roles, "navigation"));
    let separator = breadcrumb_separator_count(metrics.text) >= 2;
    let linked_list_items = dom
        .descendants(node)
        .filter(|&descendant| dom.tag(descendant) == Some(Tag::Li))
        .filter(|&item| {
            dom.descendants(item)
                .any(|descendant| dom.tag(descendant) == Some(Tag::A))
        })
        .take(2)
        .count();
    let list_shape = linked_list_items >= 2;
    let compact_links = metrics.stats.text_length <= (metrics.links as u32).saturating_mul(70);

    compact_links
        && metrics.link_density >= if explicit { 0.15 } else { 0.4 }
        && (explicit || separator || navigation && list_shape)
}

fn has_explicit_newsletter_name(name: &str) -> bool {
    name.split(|character: char| !character.is_ascii_alphanumeric())
        .any(|token| token == "newsletter")
}

fn has_subscription_name(name: &str) -> bool {
    name.contains("sign-up")
        || name
            .split(|character: char| !character.is_ascii_alphanumeric())
            .any(|token| matches!(token, "subscribe" | "subscription" | "signup"))
}

fn has_newsletter_evidence(name: &str, text: &str) -> bool {
    has_explicit_newsletter_name(name)
        || text.contains("newsletter")
        || has_subscription_name(name) && text.contains("email")
        || has_newsletter_cta_text(text)
            && (text.contains("inbox")
                || text.trim_start().starts_with("subscribe")
                || text.trim_start().starts_with("join"))
}

fn has_newsletter_cta_text(text: &str) -> bool {
    let text = text.trim();
    starts_with_any(
        text,
        &[
            "subscribe",
            "sign up",
            "join our newsletter",
            "join the newsletter",
            "get updates",
            "stay informed",
            "enter your email",
        ],
    ) || contains_any(
        text,
        &[
            "subscribe to our newsletter",
            "sign up for our newsletter",
            "get updates in your inbox",
            "enter your email",
        ],
    )
}

fn is_newsletter_cta(metrics: &PeripheralMetrics<'_>) -> bool {
    let action = metrics.has_form || metrics.controls > 0 || metrics.links > 0;
    let explicit_newsletter =
        has_explicit_newsletter_name(metrics.name) || metrics.text.contains("newsletter");
    let boundary = metrics.at_start || metrics.at_end || explicit_newsletter;
    let short_copy = metrics.stats.text_length < 800
        && metrics.stats.word_count <= 120
        && metrics.stats.sentence_end_count <= 8
        && metrics.short;
    let explicit_subscription_cta = has_subscription_name(metrics.name)
        && metrics.links > 0
        && metrics.text.trim_start().starts_with("subscribe");
    boundary
        && action
        && short_copy
        && (has_newsletter_evidence(metrics.name, metrics.text) || explicit_subscription_cta)
}

fn is_author_promotion(dom: &Dom, node: NodeId, metrics: &PeripheralMetrics<'_>) -> bool {
    if !(metrics.at_start || metrics.at_end) || !metrics.short || metrics.stats.text_length >= 700 {
        return false;
    }
    let latest_label = metrics.text.contains("the latest from ")
        || metrics.text.starts_with("latest from ")
        || metrics.text.contains("latest articles from ");
    let profile_shape = matches!(dom.tag(node), Some(Tag::Header | Tag::Aside | Tag::Section))
        || contains_any(metrics.name, &["author", "profile", "bio", "promotion"]);
    let author_media = metrics.images > 0 && metrics.links > 0;
    let promotional_copy = metrics.text.contains("monthly")
        && contains_any(metrics.text, &["news", "updates", "newsletter"]);
    latest_label && profile_shape && (author_media || promotional_copy)
}

fn is_collection_promotion(dom: &Dom, node: NodeId, metrics: &PeripheralMetrics<'_>) -> bool {
    if !metrics.at_end || !metrics.short || metrics.stats.text_length >= 800 || metrics.links == 0 {
        return false;
    }
    let labelled = metrics.text.starts_with("collection ")
        || metrics.text == "collection"
        || has_heading_text(dom, node, "collection");
    let explicit_tout = contains_any(metrics.name, &["related-content-tout", "collection"]);
    let promotional_shape = explicit_tout
        || metrics.images > 0 && matches!(dom.tag(node), Some(Tag::Aside | Tag::Section));
    labelled && promotional_shape
}

fn is_audio_controls(metrics: &PeripheralMetrics<'_>) -> bool {
    let named = contains_any(metrics.name, &["audio", "player", "listen"]);
    let labelled = metrics.text.contains("listen to article")
        || metrics.text.contains("listen to this article")
        || metrics.text.contains("[[duration]]");
    metrics.short && metrics.stats.text_length < 240 && named && labelled
}

fn is_profile_field_label(dom: &Dom, node: NodeId) -> bool {
    let mut text = String::new();
    dom.append_normalized_text_limited(node, &mut text, 128);
    let text = text.trim().trim_end_matches(':').trim();
    equals_any_ascii_case_insensitive(text, &["founded", "batch", "team size", "status"])
}

fn is_job_profile_content(
    dom: &Dom,
    node: NodeId,
    page_kind: PageKind,
    metrics: &PeripheralMetrics<'_>,
) -> bool {
    if page_kind != PageKind::JobListing || metrics.stats.text_length >= 1_600 {
        return false;
    }
    let named_profile = contains_any(metrics.name, &["founder", "profile-card", "profile_card"]);
    let repeated_founder_card = metrics.name.contains("card")
        && metrics.images >= 2
        && metrics.text.match_indices("founder").take(2).count() >= 2;
    metrics.at_end && named_profile && (metrics.images >= 2 || has_repeated_link_cards(dom, node))
        || repeated_founder_card
}

fn is_standalone_share_prompt(dom: &Dom, node: NodeId, metrics: &PeripheralMetrics<'_>) -> bool {
    if is_inside_article_container(dom, node)
        || !metrics.short
        || !contains_any(
            metrics.name,
            &["sharebar", "sharecta", "share-bar", "share-cta"],
        )
        || metrics.controls == 0 && metrics.links < 2
    {
        return false;
    }
    contains_any(
        metrics.text,
        &[
            "know someone who should see",
            "share this",
            "pass it on",
            "send them the link",
        ],
    )
}

fn is_sponsored_anchor(dom: &Dom, node: NodeId) -> bool {
    dom.tag(node) == Some(Tag::A)
        && dom.attr(node, AttrName::Rel).is_some_and(|rel| {
            rel.split_ascii_whitespace()
                .any(|token| token.eq_ignore_ascii_case("sponsored"))
        })
}

fn is_sponsored_content(metrics: &PeripheralMetrics<'_>, has_sponsored_link: bool) -> bool {
    metrics.short && metrics.at_start && has_sponsored_link && metrics.text.contains("sponsored")
}

fn is_revision_history(metrics: &PeripheralMetrics<'_>) -> bool {
    if !metrics.at_start || !metrics.short {
        return false;
    }
    let metadata_name = contains_any(metrics.name, &["note-changes", "revision", "history"])
        || metrics.name.contains(" meta");
    let explicit_metadata =
        metrics.text.starts_with("recent changes") || metrics.text.starts_with("recently updated");
    (metadata_name || explicit_metadata)
        && contains_any(metrics.text, &["recent changes", "recently updated"])
        && contains_any(metrics.text, &["last updated", "created", "today"])
}

fn is_decorative_separator_text(text: &str) -> bool {
    matches!(text, "###" | "***" | "___")
}

fn related_name_signal(name: &str) -> bool {
    contains_any(
        name,
        &[
            "related",
            "recommend",
            "more-stories",
            "more_stories",
            "read-next",
            "collection",
        ],
    )
}

fn related_text_signal(text: &str) -> bool {
    starts_with_any(
        text,
        &[
            "related",
            "recommended",
            "more stories",
            "more context",
            "you may also like",
            "collection",
        ],
    )
}

fn related_heading_signal_in(dom: &Dom, node: NodeId) -> RelatedHeadingSignal {
    dom.descendants(node)
        .map(|descendant| related_heading_signal(dom, descendant))
        .max()
        .unwrap_or(RelatedHeadingSignal::None)
}

fn heading_text_equals(dom: &Dom, node: NodeId, expected: &str) -> bool {
    matches!(
        dom.tag(node),
        Some(Tag::H2 | Tag::H3 | Tag::H4 | Tag::H5 | Tag::H6)
    ) && {
        let mut text = String::new();
        dom.append_normalized_text_limited(node, &mut text, 128);
        text.trim().eq_ignore_ascii_case(expected)
    }
}

fn has_heading_text(dom: &Dom, node: NodeId, expected: &str) -> bool {
    std::iter::once(node)
        .chain(dom.descendants(node))
        .any(|descendant| heading_text_equals(dom, descendant, expected))
}

fn has_academic_related_heading(dom: &Dom, node: NodeId) -> bool {
    has_heading_text(dom, node, "related work")
}

fn linked_short_child_count(dom: &Dom, parent: NodeId) -> usize {
    dom.element_children(parent)
        .filter(|&child| {
            !matches!(
                dom.tag(child),
                Some(Tag::H1 | Tag::H2 | Tag::H3 | Tag::H4 | Tag::H5 | Tag::H6)
            ) && dom.normalized_char_count(child) <= 240
                && (dom.tag(child) == Some(Tag::A)
                    || dom
                        .descendants(child)
                        .any(|descendant| dom.tag(descendant) == Some(Tag::A)))
        })
        .take(3)
        .count()
}

fn has_repeated_link_cards(dom: &Dom, node: NodeId) -> bool {
    if linked_short_child_count(dom, node) >= 2 {
        return true;
    }
    dom.element_children(node).any(|container| {
        matches!(
            dom.tag(container),
            Some(Tag::Div | Tag::Ol | Tag::Section | Tag::Ul)
        ) && linked_short_child_count(dom, container) >= 2
    })
}

fn has_repeated_link_cards_in_nodes(dom: &Dom, nodes: &[NodeId]) -> bool {
    let direct_anchors = |parent| {
        dom.element_children(parent)
            .filter(|&child| {
                dom.tag(child) == Some(Tag::A) && dom.normalized_char_count(child) <= 240
            })
            .take(3)
            .count()
    };
    nodes
        .iter()
        .filter(|&&node| dom.tag(node) == Some(Tag::A) && dom.normalized_char_count(node) <= 240)
        .take(3)
        .count()
        >= 2
        || nodes.iter().any(|&container| {
            matches!(
                dom.tag(container),
                Some(Tag::Div | Tag::Ol | Tag::Section | Tag::Ul)
            ) && direct_anchors(container) >= 2
        })
}

fn has_next_steps_card_siblings(dom: &Dom, node: NodeId) -> bool {
    std::iter::once(node)
        .chain(dom.descendants(node))
        .filter(|&heading| heading_text_equals(dom, heading, "next steps"))
        .any(|heading| {
            let Some(parent) = dom.parent(heading) else {
                return false;
            };
            let siblings: SmallVec<[NodeId; 16]> = dom.element_children(parent).collect();
            let Some(start) = siblings.iter().position(|&sibling| sibling == heading) else {
                return false;
            };
            let end = siblings[start + 1..]
                .iter()
                .position(|&sibling| {
                    matches!(
                        dom.tag(sibling),
                        Some(Tag::H1 | Tag::H2 | Tag::H3 | Tag::H4 | Tag::H5 | Tag::H6)
                    )
                })
                .map_or(siblings.len(), |offset| start + 1 + offset);
            has_repeated_link_cards_in_nodes(dom, &siblings[start + 1..end])
        })
}

fn is_related_content(dom: &Dom, node: NodeId, metrics: &PeripheralMetrics<'_>) -> bool {
    if metrics.links < 2 || has_academic_related_heading(dom, node) {
        return false;
    }
    let heading = related_heading_signal_in(dom, node);
    let named = related_name_signal(metrics.name)
        || dom
            .descendants(node)
            .any(|descendant| related_name_signal(&node_name(dom, descendant)));
    if heading == RelatedHeadingSignal::None && !named && !related_text_signal(metrics.text) {
        return false;
    }
    let repeated_cards = has_repeated_link_cards(dom, node);
    let terminal_card_grid = metrics.at_end
        && heading == RelatedHeadingSignal::Strong
        && metrics.links >= 3
        && (metrics.images >= 2 || repeated_cards);
    if terminal_card_grid {
        return true;
    }
    // "Next Steps" is also a common instructional heading. Require card
    // structure before treating a short section with this heading as related
    // content. Other explicit related labels keep the established behavior.
    if metrics.short && metrics.link_density >= 0.2 {
        if has_heading_text(dom, node, "next steps") {
            return has_next_steps_card_siblings(dom, node);
        }
        return true;
    }
    if metrics.stats.text_length >= 1_200 {
        return false;
    }
    let non_link_chars = f64::from(metrics.stats.text_length) * (1.0 - metrics.link_density);
    let sparse_text = non_link_chars <= 320.0 && metrics.stats.sentence_end_count <= 7;

    if !sparse_text {
        return false;
    }

    if metrics.at_end {
        return match heading {
            RelatedHeadingSignal::Strong => repeated_cards || metrics.link_density >= 0.45,
            RelatedHeadingSignal::Ambiguous => repeated_cards && metrics.link_density >= 0.45,
            RelatedHeadingSignal::None => named && repeated_cards && metrics.link_density >= 0.3,
        };
    }
    if metrics.at_start {
        return named && repeated_cards && metrics.link_density >= 0.2;
    }

    // Mid-article removal needs every strong signal. This keeps ordinary link
    // sections while removing a clear card interruption between prose blocks.
    heading == RelatedHeadingSignal::Strong
        && repeated_cards
        && metrics.link_density >= 0.35
        && metrics.stats.text_length < 700
        && non_link_chars <= 240.0
}

/// Removes short textual controls that do not always have useful class names.
///
/// Phrase matches are deliberately narrow. A match also needs document-boundary
/// or control evidence, except for labels that are complete conventional UI
/// phrases. This keeps the same words when they occur in normal prose.
fn remove_contextual_boilerplate_in_workspace(
    dom: &mut Dom,
    root: NodeId,
    store: &mut crate::dom::NodeStateStore,
    evidence: &crate::document::SourceEvidence,
    text_buffer: &mut String,
    nodes: &mut Vec<NodeId>,
    workspace: &mut FragmentWorkspace,
) {
    workspace.ensure_snapshot(dom, root);
    let snapshot = workspace.elements_with_depth();
    let mut has_nested_boundary = vec![false; dom.len()];
    for &(node, _) in snapshot.iter().rev() {
        if (is_contextual_text_boundary(dom, node) || has_nested_boundary[node.index()])
            && let Some(parent) = dom.parent(node)
        {
            has_nested_boundary[parent.index()] = true;
        }
    }
    nodes.clear();
    nodes.extend(snapshot.iter().map(|&(node, _)| node).filter(|&node| {
        is_contextual_text_boundary(dom, node) && !has_nested_boundary[node.index()]
    }));

    // Reuse unchanged subtree statistics. Detach helpers invalidate only the
    // affected ancestor chain, so rebuilding the root keeps this pass local.
    get_or_compute_stats(dom, root, store);

    for &node in nodes.iter().rev() {
        if dom.parent(node).is_none() {
            continue;
        }
        if store
            .get_stats(node)
            .is_none_or(|stats| stats.text_length > 140)
        {
            continue;
        }
        get_inner_text(dom, node, text_buffer);
        text_buffer.make_ascii_lowercase();
        let text = text_buffer.trim();
        if text.is_empty() {
            continue;
        }
        let name = node_name(dom, node);
        let link_or_control = dom.tag(node) == Some(Tag::Form)
            || dom.descendants(node).any(|descendant| {
                matches!(
                    dom.tag(descendant),
                    Some(Tag::A | Tag::Button | Tag::Input | Tag::Select | Tag::Textarea)
                )
            });
        let at_start = near_content_start(dom, node, root, store);
        let at_end = near_content_end(dom, node, root, store);

        let reading_time = is_reading_time_label(text)
            && (at_start
                || contains_any(
                    &name,
                    &[
                        "read-time",
                        "read_time",
                        "reading-time",
                        "metadata",
                        "byline",
                    ],
                ));
        let advertisement = matches!(
            text,
            "advertisement"
                | "advertisement continues below"
                | "reg ad"
                | "sponsored"
                | "sponsored content"
        ) && (at_start || at_end || strong_ad_name(&name));
        let action = matches!(
            text,
            "share"
                | "share this"
                | "share this article"
                | "share this story"
                | "read more"
                | "leave a comment"
        ) && link_or_control
            && (at_end || contains_any(&name, &["share", "action", "button", "toolbar"]));
        let subscription = (text.starts_with("sign up for our newsletter")
            || text.starts_with("subscribe to our newsletter")
            || text.starts_with("subscribe for updates"))
            && link_or_control
            && (at_start
                || at_end
                || contains_any(&name, &["newsletter", "subscribe", "signup", "sign-up"]));
        let copy_confirmation = matches!(text, "copied" | "copied to clipboard" | "copy complete")
            && (dom.attr(node, AttrName::AriaLive).is_some()
                || contains_any(&name, &["clipboard", "copy-status", "copy_status"]));
        let promotional_prompt = text.split_ascii_whitespace().count() <= 12
            && (text.starts_with("unlock the full ") || text.starts_with("start your free trial"))
            && (contains_any(&name, &["promotion", "promo", "paywall", "upsell"])
                || dom.ancestors(node).take(3).any(|ancestor| {
                    contains_any(
                        &node_name(dom, ancestor),
                        &["promotion", "promo", "paywall", "upsell"],
                    )
                }));

        if is_protected_content(dom, node, evidence) && !copy_confirmation && !promotional_prompt {
            continue;
        }

        if reading_time
            || advertisement
            || action
            || subscription
            || copy_confirmation
            || promotional_prompt
        {
            detach_and_invalidate_stats(dom, node, store);
        }
    }
    workspace.invalidate();
}

fn is_contextual_text_boundary(dom: &Dom, node: NodeId) -> bool {
    matches!(
        dom.tag(node),
        Some(
            Tag::Aside
                | Tag::Div
                | Tag::Footer
                | Tag::H2
                | Tag::H3
                | Tag::H4
                | Tag::H5
                | Tag::H6
                | Tag::P
                | Tag::Section
                | Tag::Small
                | Tag::Span
        )
    )
}

fn is_reading_time_label(text: &str) -> bool {
    let text = text
        .strip_prefix("reading time:")
        .map(str::trim)
        .unwrap_or(text);
    let mut words = text.split_ascii_whitespace();
    let Some(amount) = words.next() else {
        return false;
    };
    let Some(unit) = words.next() else {
        return false;
    };
    let Some(read) = words.next() else {
        return false;
    };
    amount.bytes().all(|byte| byte.is_ascii_digit())
        && matches!(unit, "min" | "mins" | "minute" | "minutes")
        && read == "read"
        && words.next().is_none()
}

fn hoist_protected_children(
    dom: &mut Dom,
    wrapper: NodeId,
    store: &mut crate::dom::NodeStateStore,
    evidence: &crate::document::SourceEvidence,
) {
    let protected: SmallVec<[NodeId; 4]> = dom
        .descendants(wrapper)
        .filter(|&node| {
            is_protected_content(dom, node, evidence)
                && !dom
                    .ancestors(node)
                    .take_while(|&ancestor| ancestor != wrapper)
                    .any(|ancestor| is_protected_content(dom, ancestor, evidence))
        })
        .collect();
    for node in protected {
        invalidate_stats_for_ancestors(dom, node, store);
        dom.insert_before(wrapper, node);
    }
}

fn detach_and_invalidate_stats(
    dom: &mut Dom,
    node: NodeId,
    store: &mut crate::dom::NodeStateStore,
) {
    invalidate_stats_for_ancestors(dom, node, store);
    dom.detach(node);
}

fn invalidate_stats_for_ancestors(dom: &Dom, node: NodeId, store: &mut crate::dom::NodeStateStore) {
    for ancestor in dom.ancestors(node) {
        store.invalidate_stats(ancestor);
        if store.link_lengths_enabled() {
            store.set_link_length(ancestor, 0.0);
        }
    }
}

fn has_lazy_image_candidate(dom: &Dom, image: NodeId) -> bool {
    dom.attrs(image).iter().any(|attribute| {
        let name = attribute.name.local.as_ref();
        name.starts_with("data-")
            && (has_image_src(attribute.value.as_ref())
                || has_image_srcset(attribute.value.as_ref()))
    })
}

fn is_author_contribution_boundary(dom: &Dom, node: NodeId) -> bool {
    if !matches!(dom.tag(node), Some(Tag::Ol | Tag::Ul)) {
        return false;
    }
    if !has_author_region_name(dom, node) {
        return false;
    }
    let mut text = String::new();
    dom.append_normalized_text_limited(node, &mut text, 6_000);
    text.make_ascii_lowercase();
    text.matches("roles").count() >= 2
        && text.matches("affiliation").count() >= 2
        && dom
            .descendants(node)
            .any(|descendant| dom.tag(descendant) == Some(Tag::A))
}

fn has_author_region_name(dom: &Dom, node: NodeId) -> bool {
    [AttrName::Class, AttrName::Id]
        .into_iter()
        .filter_map(|attribute| dom.attr(node, attribute))
        .any(|value| {
            [
                "author",
                "byline",
                "contributor",
                "title-authors",
                "title_authors",
                "authroles",
                "authaffiliation",
            ]
            .iter()
            .any(|needle| contains_ascii_case_insensitive(value, needle))
        })
}

fn picture_has_lazy_source(dom: &Dom, image: NodeId) -> bool {
    let Some(picture) = dom
        .ancestors(image)
        .find(|&ancestor| dom.tag(ancestor) == Some(Tag::Picture))
    else {
        return false;
    };
    if dom
        .attr(picture, AttrName::DataSrc)
        .is_some_and(has_image_src)
        || dom
            .attr(picture, AttrName::DataSrcset)
            .is_some_and(has_image_srcset)
    {
        return true;
    }
    dom.descendants(picture).any(|source| {
        dom.tag(source) == Some(Tag::Source)
            && (dom
                .attr(source, AttrName::DataSrc)
                .or_else(|| dom.attr(source, AttrName::Src))
                .is_some_and(has_image_src)
                || dom
                    .attr(source, AttrName::DataSrcset)
                    .or_else(|| dom.attr(source, AttrName::Srcset))
                    .is_some_and(has_image_srcset))
    })
}

fn is_heuristic_boundary(dom: &Dom, node: NodeId) -> bool {
    if matches!(
        dom.tag(node),
        Some(Tag::Aside | Tag::Footer | Tag::Form | Tag::Header | Tag::Nav)
    ) {
        return true;
    }
    let name = node_name(dom, node);
    let author_title_wrapper = contains_any(&name, &["title-authors", "title_authors"])
        && dom
            .element_children(node)
            .any(|child| dom.tag(child) == Some(Tag::H1))
        && dom.descendants(node).any(|descendant| {
            contains_any(&node_name(dom, descendant), &["author-list", "author_list"])
        });
    matches!(
        dom.tag(node),
        Some(Tag::Div | Tag::Ol | Tag::P | Tag::Section | Tag::Ul)
    ) && (contains_any(
        &name,
        &[
            "related",
            "recommend",
            "share",
            "sharebar",
            "sharecta",
            "social",
            "newsletter",
            "subscribe",
            "signup",
            "menu",
            "navigation",
            "breadcrumb",
            "author",
            "author-contribution",
            "author_contribution",
            "contributor",
            "profile",
            "collection",
            "audio",
            "player",
            "founder",
            "company-profile",
            "company_profile",
            "bio",
            "advert",
            "sponsor",
            "cookie",
            "consent",
            "login",
            "signin",
            "sign-in",
            "sidebar",
            "toolbar",
            "actions",
            "feedback",
            "footer",
            "contact",
            "comment",
            "button-wrapper",
            "taxonomy",
            "subject",
            "subject-area",
            "subject_areas",
            "subjectareas",
            "company-portals",
            "entity-list",
            "entity_list",
            "tag-list",
            "tag_list",
        ],
    ) || has_document_maintenance_name(&name))
        && !author_title_wrapper
}

fn is_document_maintenance(dom: &Dom, node: NodeId, metrics: &PeripheralMetrics<'_>) -> bool {
    if !metrics.at_end || metrics.stats.text_length > 1_200 {
        return false;
    }
    let maintenance_text = metrics.text.contains("was this page helpful")
        || metrics.text.contains("this page was last modified")
        || metrics.text.contains("help us improve this page")
        || metrics.text.starts_with("last modified ")
        || metrics.text.starts_with("last updated ");
    if !maintenance_text {
        return false;
    }
    let named_maintenance = has_document_maintenance_name(metrics.name);
    let specific_name = has_specific_document_maintenance_name(metrics.name);
    let interactive = metrics.links > 0 || metrics.controls > 0;
    specific_name && (interactive || metrics.stats.text_length < 180)
        || named_maintenance
            && (interactive
                || metrics.text.contains("was this page helpful")
                || metrics.text.contains("help us improve this page"))
        || matches!(dom.tag(node), Some(Tag::Footer))
            && metrics.controls > 0
            && (metrics.text.contains("was this page helpful")
                || metrics.text.contains("help us improve this page"))
}

fn has_specific_document_maintenance_name(name: &str) -> bool {
    let has = |expected: &str| {
        name.split(|character: char| !character.is_ascii_alphanumeric())
            .any(|token| token == expected)
    };
    has("feedback")
        || has("lastmod")
        || has("last") && has("modified")
        || has("page") && has("maintenance")
}

fn has_document_maintenance_name(name: &str) -> bool {
    let has = |expected: &str| {
        name.split(|character: char| !character.is_ascii_alphanumeric())
            .any(|token| token == expected)
    };
    has("feedback")
        || has("lastmod")
        || has("last") && has("modified")
        || has("page") && (has("footer") || has("maintenance"))
        || has("article") && has("footer")
        || has("pre") && has("footer")
}

fn is_revision_history_marker(dom: &Dom, node: NodeId) -> bool {
    let name = node_name(dom, node);
    let mut text = String::new();
    append_bounded_text(dom, node, 1_024, &mut text);
    text.make_ascii_lowercase();
    let text = text.trim();
    let named_metadata = contains_any(&name, &["note-changes", "revision", "history", "meta"]);
    let explicit_metadata =
        text.starts_with("recent changes") || text.starts_with("recently updated");
    (named_metadata || explicit_metadata)
        && contains_any(text, &["recent changes", "recently updated"])
        && contains_any(text, &["last updated", "created", "today"])
}

fn near_content_end(
    dom: &Dom,
    node: NodeId,
    root: NodeId,
    store: &mut crate::dom::NodeStateStore,
) -> bool {
    let mut current = node;
    let mut trailing_chars = 0_usize;
    loop {
        let mut sibling = dom.next_sibling(current);
        while let Some(next) = sibling {
            // Reuse the cached normalized character count. The previous code
            // rebuilt every following subtree for each heuristic boundary.
            trailing_chars = trailing_chars.saturating_add(
                crate::scoring::get_or_compute_stats(dom, next, store).text_length as usize,
            );
            if trailing_chars > 100 {
                return false;
            }
            sibling = dom.next_sibling(next);
        }
        if current == root {
            return true;
        }
        let Some(parent) = dom.parent(current) else {
            return true;
        };
        current = parent;
    }
}

fn near_content_end_ignoring_footnotes(
    dom: &Dom,
    node: NodeId,
    root: NodeId,
    store: &mut crate::dom::NodeStateStore,
) -> bool {
    let mut current = node;
    let mut trailing_chars = 0_usize;
    loop {
        let mut sibling = dom.next_sibling(current);
        while let Some(next) = sibling {
            if !node_name(dom, next).contains("footnotes") {
                trailing_chars = trailing_chars
                    .saturating_add(get_or_compute_stats(dom, next, store).text_length as usize);
                if trailing_chars > 100 {
                    return false;
                }
            }
            sibling = dom.next_sibling(next);
        }
        if current == root {
            return true;
        }
        let Some(parent) = dom.parent(current) else {
            return true;
        };
        current = parent;
    }
}

fn near_content_start(
    dom: &Dom,
    node: NodeId,
    root: NodeId,
    store: &mut crate::dom::NodeStateStore,
) -> bool {
    let mut current = node;
    let mut leading_chars = 0_usize;
    loop {
        let mut sibling = dom.prev_sibling(current);
        while let Some(previous) = sibling {
            leading_chars = leading_chars.saturating_add(
                crate::scoring::get_or_compute_stats(dom, previous, store).text_length as usize,
            );
            if leading_chars > 100 {
                return false;
            }
            sibling = dom.prev_sibling(previous);
        }
        if current == root {
            return true;
        }
        let Some(parent) = dom.parent(current) else {
            return true;
        };
        current = parent;
    }
}

enum NodeName<'a> {
    Borrowed(&'a str),
    Owned(SmallVec<[u8; 64]>),
}

impl NodeName<'_> {
    #[inline]
    fn as_str(&self) -> &str {
        match self {
            Self::Borrowed(value) => value,
            Self::Owned(value) => std::str::from_utf8(value).expect("node names remain UTF-8"),
        }
    }
}

impl AsRef<str> for NodeName<'_> {
    #[inline]
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl std::ops::Deref for NodeName<'_> {
    type Target = str;

    #[inline]
    fn deref(&self) -> &Self::Target {
        self.as_str()
    }
}

fn node_name<'a>(dom: &'a Dom, node: NodeId) -> NodeName<'a> {
    if dom.tag(node) != Some(Tag::Other) && dom.attrs(node).is_empty() {
        return NodeName::Borrowed("");
    }
    let tag_name = (dom.tag(node) == Some(Tag::Other))
        .then(|| dom.qual_name(node).map(|name| name.local.as_ref()))
        .flatten();
    let class = dom.attr(node, AttrName::Class);
    let id = dom.attr(node, AttrName::Id);
    let mut count = 0;
    let mut single = None;
    for part in [tag_name, class, id].into_iter().flatten() {
        count += 1;
        single = Some(part);
    }
    if count == 0 {
        return NodeName::Borrowed("");
    }
    if count == 1
        && let Some(part) = single
        && part.is_ascii()
        && part.bytes().all(|byte| !byte.is_ascii_uppercase())
    {
        return NodeName::Borrowed(part);
    }

    // Names are usually short. Keep the joined, lower-case value inline so
    // repeated cleanup classifiers do not allocate one String per node.
    let mut value = SmallVec::<[u8; 64]>::new();
    for (index, part) in [tag_name, class, id].into_iter().flatten().enumerate() {
        if index > 0 {
            value.push(b' ');
        }
        value.extend_from_slice(part.as_bytes());
    }
    value.make_ascii_lowercase();
    NodeName::Owned(value)
}

fn append_node_name(dom: &Dom, node: NodeId, output: &mut String) {
    output.clear();
    if dom.tag(node) != Some(Tag::Other) && dom.attrs(node).is_empty() {
        return;
    }
    let tag_name = (dom.tag(node) == Some(Tag::Other))
        .then(|| dom.qual_name(node).map(|name| name.local.as_ref()))
        .flatten();
    let class = dom.attr(node, AttrName::Class);
    let id = dom.attr(node, AttrName::Id);
    let mut first = true;
    for part in [tag_name, class, id].into_iter().flatten() {
        if !first {
            output.push(' ');
        }
        output.push_str(part);
        first = false;
    }
    output.make_ascii_lowercase();
}

fn contains_any(value: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| value.contains(needle))
}

fn name_has_token(value: &str, expected: &str) -> bool {
    value
        .split(|character: char| !character.is_ascii_alphanumeric())
        .any(|token| token.eq_ignore_ascii_case(expected))
}

fn contains_name_or_text(name: &str, text: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| {
        text.contains(needle)
            || name.contains(needle)
            || needle.split_once(' ').is_some_and(|(first, rest)| {
                let Some(prefix) = name.strip_suffix(first) else {
                    return false;
                };
                prefix
                    .chars()
                    .next_back()
                    .is_none_or(|character| !character.is_ascii_alphanumeric())
                    && text.starts_with(rest)
            })
    })
}

fn contains_followed_by_space(text: &str, needle: &str) -> bool {
    text.match_indices(needle)
        .any(|(index, _)| text[index + needle.len()..].starts_with(' '))
}

fn contains_ascii_case_insensitive(value: &str, needle: &str) -> bool {
    let needle = needle.as_bytes();
    !needle.is_empty()
        && value.as_bytes().windows(needle.len()).any(|window| {
            window
                .iter()
                .zip(needle)
                .all(|(&left, &right)| left.eq_ignore_ascii_case(&right))
        })
}

fn starts_ascii_case_insensitive(value: &str, prefix: &str) -> bool {
    value
        .as_bytes()
        .get(..prefix.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(prefix.as_bytes()))
}

fn ends_ascii_case_insensitive(value: &str, suffix: &str) -> bool {
    value
        .as_bytes()
        .get(value.len().saturating_sub(suffix.len())..)
        .is_some_and(|tail| tail.eq_ignore_ascii_case(suffix.as_bytes()))
}

fn equals_any_ascii_case_insensitive(value: &str, values: &[&str]) -> bool {
    values
        .iter()
        .any(|expected| value.eq_ignore_ascii_case(expected))
}

fn starts_with_any(value: &str, needles: &[&str]) -> bool {
    let value = value.trim_start();
    needles.iter().any(|needle| value.starts_with(needle))
}

fn strong_ad_name(value: &str) -> bool {
    value
        .split(|character: char| !character.is_ascii_alphanumeric())
        .any(|token| {
            matches!(
                token,
                "ad" | "ads" | "advert" | "advertisement" | "sponsor" | "sponsored"
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dom::{AttrName, NodeStateStore};

    #[test]
    fn normalizes_br_runs_before_renaming_fonts() {
        let mut dom = Dom::parse_document("<body><div><br><br><font>text</font></div>").unwrap();

        prep_document(&mut dom);

        let body = dom.body().unwrap();
        let paragraph = dom.first_descendant_by_tag(body, Tag::P).unwrap();
        let span = dom.first_descendant_by_tag(body, Tag::Span).unwrap();
        assert_eq!(dom.parent(paragraph), dom.parent(span));
        assert!(!dom.descendants(paragraph).any(|id| id == span));
    }

    #[test]
    fn fragment_workspace_reuses_snapshot_until_invalidation() {
        let mut dom = Dom::parse_fragment(
            "<section><p>first</p><div><p>second</p></div></section>",
            Tag::Div,
        )
        .unwrap();
        let root = dom.root();
        let section = dom.first_descendant_by_tag(root, Tag::Section).unwrap();
        let paragraph = dom.first_descendant_by_tag(section, Tag::P).unwrap();
        let mut workspace = FragmentWorkspace::default();

        workspace.ensure_snapshot(&dom, section);
        assert_eq!(workspace.preorder()[0], section);
        assert_eq!(workspace.elements_with_depth().len(), 3);
        let first_capacity = workspace.elements_with_depth().as_ptr();
        workspace.ensure_snapshot(&dom, section);
        assert_eq!(workspace.elements_with_depth().as_ptr(), first_capacity);

        dom.detach(paragraph);
        workspace.invalidate();
        workspace.ensure_snapshot(&dom, section);
        assert!(
            !workspace
                .elements_with_depth()
                .iter()
                .any(|&(node, _)| node == paragraph)
        );
    }

    #[test]
    fn fragment_workspace_reuses_scratch_capacity() {
        let mut workspace = FragmentWorkspace::default();
        let u32_ptr = workspace.scratch_u32(16).as_mut_ptr();
        let bytes_ptr = workspace.scratch_bytes(16).as_mut_ptr();
        let bits_ptr = workspace.scratch_bits(16).as_mut_ptr();

        workspace.reset();
        assert_eq!(workspace.scratch_u32(4).as_mut_ptr(), u32_ptr);
        assert_eq!(workspace.scratch_bytes(4).as_mut_ptr(), bytes_ptr);
        assert_eq!(workspace.scratch_bits(4).as_mut_ptr(), bits_ptr);
    }

    #[test]
    fn preserves_code_line_breaks_during_document_preparation() {
        let mut dom = Dom::parse_document(
            "<body><pre><code>one<br><br>two</code></pre><code>three<br><br>four</code></body>",
        )
        .unwrap();

        prep_document(&mut dom);

        let body = dom.body().unwrap();
        assert_eq!(
            dom.descendants(body)
                .filter(|&node| dom.tag(node) == Some(Tag::Br))
                .count(),
            4
        );
    }

    #[test]
    fn replaces_short_base64_image_placeholders() {
        let mut dom = Dom::parse_document(
            r#"<img src="data:image/png;base64,AAAA" data-src="https://example.com/image.jpg">"#,
        )
        .unwrap();
        let root = dom.root();
        let image = dom.first_descendant_by_tag(root, Tag::Img).unwrap();
        fix_lazy_images(&mut dom, root, &mut Vec::new());
        assert_eq!(
            dom.attr(image, AttrName::Src),
            Some("https://example.com/image.jpg")
        );
    }

    #[test]
    fn preserves_unmatched_image_placeholders() {
        let mut dom = Dom::parse_document(
            r#"<main><img id="placeholder"><noscript>Image unavailable</noscript></main>"#,
        )
        .unwrap();
        let placeholder = dom.first_descendant_by_tag(dom.root(), Tag::Img).unwrap();

        unwrap_noscript_images(&mut dom);

        assert!(dom.parent(placeholder).is_some());
    }

    #[test]
    fn does_not_merge_a_fallback_with_an_unrelated_adjacent_image() {
        let mut dom = Dom::parse_document(
            r#"<main><img src="first.jpg"><noscript><img src="second.jpg"></noscript></main>"#,
        )
        .unwrap();

        unwrap_noscript_images(&mut dom);

        let sources: Vec<_> = dom
            .descendants(dom.root())
            .filter(|&id| dom.tag(id) == Some(Tag::Img))
            .filter_map(|id| dom.attr(id, AttrName::Src))
            .collect();
        assert_eq!(sources, ["first.jpg", "second.jpg"]);
    }

    #[test]
    fn does_not_merge_distinct_images_with_equal_dimensions() {
        let mut dom = Dom::parse_document(
            r#"<main><img src="first.jpg" width="300" height="200"><noscript><img src="second.jpg" width="300" height="200"></noscript></main>"#,
        )
        .unwrap();

        unwrap_noscript_images(&mut dom);

        let sources: Vec<_> = dom
            .descendants(dom.root())
            .filter(|&id| dom.tag(id) == Some(Tag::Img))
            .filter_map(|id| dom.attr(id, AttrName::Src))
            .collect();
        assert_eq!(sources, ["first.jpg", "second.jpg"]);
    }

    #[test]
    fn does_not_merge_distinct_images_with_the_same_basename() {
        let mut dom = Dom::parse_document(
            r#"<main><img src="/first/image.jpg"><noscript><img src="/second/image.jpg"></noscript></main>"#,
        )
        .unwrap();

        unwrap_noscript_images(&mut dom);

        let sources: Vec<_> = dom
            .descendants(dom.root())
            .filter(|&id| dom.tag(id) == Some(Tag::Img))
            .filter_map(|id| dom.attr(id, AttrName::Src))
            .collect();
        assert_eq!(sources, ["/first/image.jpg", "/second/image.jpg"]);
    }

    #[test]
    fn does_not_merge_a_fallback_with_an_unrelated_earlier_image() {
        let mut dom = Dom::parse_document(
            r#"<main><img src="first.jpg"><img><noscript><img src="second.jpg"></noscript></main>"#,
        )
        .unwrap();

        unwrap_noscript_images(&mut dom);

        let sources: Vec<_> = dom
            .descendants(dom.root())
            .filter(|&id| dom.tag(id) == Some(Tag::Img))
            .filter_map(|id| dom.attr(id, AttrName::Src))
            .collect();
        assert_eq!(sources, ["first.jpg", "second.jpg"]);
    }

    #[test]
    fn unwraps_only_single_image_noscripts_and_replaces_placeholder() {
        let mut dom = Dom::parse_document(
            r#"<img src="placeholder.jpg"><noscript><img src="real.jpg"></noscript>"#,
        )
        .unwrap();
        unwrap_noscript_images(&mut dom);
        let images: Vec<_> = dom
            .descendants(dom.root())
            .filter(|&id| dom.tag(id) == Some(Tag::Img))
            .collect();
        assert_eq!(images.len(), 1);
        assert_eq!(dom.attr(images[0], AttrName::Src), Some("real.jpg"));
    }

    #[test]
    fn promotes_a_standalone_direct_noscript_image() {
        let mut dom =
            Dom::parse_document(r#"<main><noscript><img src="standalone.jpg"></noscript></main>"#)
                .unwrap();

        unwrap_noscript_images(&mut dom);

        let image = dom.first_descendant_by_tag(dom.root(), Tag::Img).unwrap();
        assert_eq!(dom.attr(image, AttrName::Src), Some("standalone.jpg"));
        assert!(
            !dom.descendants(dom.root())
                .any(|id| dom.tag(id) == Some(Tag::Noscript))
        );
    }

    #[test]
    fn parses_escaped_noscript_image_text_without_serializing() {
        let mut dom = Dom::parse_document(
            r#"<img src="placeholder.jpg"><noscript>&lt;img src="real.jpg" data-id="1"&gt;</noscript>"#,
        )
        .unwrap();

        unwrap_noscript_images(&mut dom);

        let images: Vec<_> = dom
            .descendants(dom.root())
            .filter(|&id| dom.tag(id) == Some(Tag::Img))
            .collect();
        assert_eq!(images.len(), 1);
        assert_eq!(dom.attr(images[0], AttrName::Src), Some("real.jpg"));
        assert_eq!(dom.attr_by_local_name(images[0], "data-id"), Some("1"));
    }

    #[test]
    fn promotes_a_standalone_escaped_noscript_image() {
        let mut dom = Dom::parse_document(
            r#"<main><noscript>&lt;img src="standalone.jpg"&gt;</noscript></main>"#,
        )
        .unwrap();

        unwrap_noscript_images(&mut dom);

        let image = dom.first_descendant_by_tag(dom.root(), Tag::Img).unwrap();
        assert_eq!(dom.attr(image, AttrName::Src), Some("standalone.jpg"));
        assert!(
            !dom.descendants(dom.root())
                .any(|id| dom.tag(id) == Some(Tag::Noscript))
        );
    }

    #[test]
    fn preserves_an_escaped_noscript_picture_and_placeholder_description() {
        let mut dom = Dom::parse_document(
            r#"<main><img src="placeholder.gif" alt="Recovered chart"><noscript>&lt;picture&gt;&lt;source srcset="small.webp 1x, large.webp 2x"&gt;&lt;img src="fallback.jpg"&gt;&lt;/picture&gt;</noscript></main>"#,
        )
        .unwrap();

        unwrap_noscript_images(&mut dom);

        let root = dom.root();
        let image = dom.first_descendant_by_tag(root, Tag::Img).unwrap();
        assert_eq!(dom.attr(image, AttrName::Src), Some("fallback.jpg"));
        assert_eq!(
            dom.attr_by_local_name(image, "alt"),
            Some("Recovered chart")
        );
        assert!(dom.first_descendant_by_tag(root, Tag::Picture).is_some());
        assert!(dom.first_descendant_by_tag(root, Tag::Source).is_some());
        assert_eq!(
            dom.descendants(root)
                .filter(|&node| dom.tag(node) == Some(Tag::Img))
                .count(),
            1
        );
    }

    #[test]
    fn prefers_data_src_over_image_like_alt_text() {
        let mut dom = Dom::parse_document(
            r#"<img class="lazy" alt="placeholder.jpg" data-src="https://example.com/article.jpg">"#,
        )
        .unwrap();
        let root = dom.root();
        let image = dom.first_descendant_by_tag(root, Tag::Img).unwrap();

        fix_lazy_images(&mut dom, root, &mut Vec::new());

        assert_eq!(
            dom.attr(image, AttrName::Src),
            Some("https://example.com/article.jpg")
        );
    }

    #[test]
    fn applies_lazy_picture_source_to_its_image() {
        let mut dom = Dom::parse_document(
            r#"<picture data-src="photo.jpg"><source srcset="photo.webp"><img alt="Photo"></picture>"#,
        )
        .unwrap();
        let root = dom.root();
        let image = dom.first_descendant_by_tag(root, Tag::Img).unwrap();

        fix_lazy_images(&mut dom, root, &mut Vec::new());

        assert_eq!(dom.attr(image, AttrName::Src), Some("photo.jpg"));
        let picture = dom.first_descendant_by_tag(root, Tag::Picture).unwrap();
        assert_eq!(dom.attr(picture, AttrName::Src), None);
    }

    #[test]
    fn adds_lazy_figure_image_without_removing_caption() {
        let mut dom = Dom::parse_document(
            r#"<figure data-src="image.jpg?x=1&amp;y=2"><figcaption>old</figcaption></figure>"#,
        )
        .unwrap();
        let root = dom.root();
        let figure = dom.first_descendant_by_tag(root, Tag::Figure).unwrap();

        fix_lazy_images(&mut dom, root, &mut Vec::new());

        let image = dom.first_descendant_by_tag(figure, Tag::Img).unwrap();
        assert_eq!(dom.attr(image, AttrName::Src), Some("image.jpg?x=1&y=2"));
        assert_eq!(dom.element_children(figure).count(), 2);
        assert!(
            dom.first_descendant_by_tag(figure, Tag::Figcaption)
                .is_some()
        );
    }

    #[test]
    fn removes_small_navigation_media_links_and_media_credits() {
        let mut dom = Dom::parse_fragment(
            r#"<article><header><a href="/news" title="Back to news"><img src="back.svg" alt="Back to news"></a><div><img src="hero.jpg" alt="Hero"></div><div class="credit">© Photographer</div></header><p>The useful article remains.</p></article>"#,
            Tag::Div,
        )
        .unwrap();
        let root = dom.root();
        let store = &mut NodeStateStore::new();
        let evidence = crate::document::SourceEvidence::analyze(&dom, root, store);
        let mut workspace = FragmentWorkspace::default();
        assert!(remove_inline_chrome_controls_in_workspace(
            &mut dom,
            root,
            store,
            &evidence,
            &mut workspace,
        ));
        let text = dom.text(root);
        assert!(!text.contains("Back to news"));
        assert!(!text.contains("Photographer"));
        assert!(text.contains("useful article remains"));
    }

    #[test]
    fn removes_only_redundant_disclosure_labels() {
        let mut dom = Dom::parse_fragment(
            r#"<article><details open><summary>Expand description</summary><div><p>The complete API description remains available.</p></div></details><details><summary>Compatibility notes</summary><p>The authored compatibility notes remain too.</p></details></article>"#,
            Tag::Div,
        )
        .unwrap();
        let root = dom.root();
        let store = &mut NodeStateStore::new();
        let evidence = crate::document::SourceEvidence::analyze(&dom, root, store);
        let mut workspace = FragmentWorkspace::default();

        assert!(remove_inline_chrome_controls_in_workspace(
            &mut dom,
            root,
            store,
            &evidence,
            &mut workspace,
        ));
        let text = dom.text(root);
        assert!(!text.contains("Expand description"), "{text}");
        assert!(text.contains("complete API description"), "{text}");
        assert!(text.contains("Compatibility notes"), "{text}");
        assert!(text.contains("authored compatibility notes"), "{text}");
    }

    fn clean_fragment(html: &str) -> String {
        let mut dom = Dom::parse_fragment(html, Tag::Div).unwrap();
        let root = dom.root();
        let mut nodes = Vec::new();
        let mut store = NodeStateStore::new();
        let mut text = String::new();
        let allowed = Regex::new("video\\.example").unwrap();
        clean_styles(&mut dom, root, &mut nodes);
        mark_data_tables(&dom, root, &mut store, &mut nodes);
        let evidence = crate::document::SourceEvidence::analyze(&dom, root, &store);
        hard_cleanup(&mut dom, root, &allowed, false, &evidence, &mut nodes);
        heuristic_cleanup(
            &mut dom,
            root,
            PageKind::Unknown,
            &mut store,
            &evidence,
            &mut text,
            &mut nodes,
        );
        remove_repeated_and_discussion_content_in_workspace(
            &mut dom,
            root,
            PageKind::Unknown,
            &mut store,
            &evidence,
            &mut FragmentWorkspace::default(),
        );
        dom.text(root)
    }

    #[test]
    fn extreme_table_spans_do_not_overflow() {
        let dom = Dom::parse_fragment(
            r#"<table><tr><td colspan="4294967295">A</td><td colspan="4294967295">B</td></tr><tr><td colspan="4294967295">C</td><td colspan="4294967295">D</td></tr></table>"#,
            Tag::Div,
        )
        .unwrap();
        let mut store = NodeStateStore::new();
        let mut tables = Vec::new();
        mark_data_tables(&dom, dom.root(), &mut store, &mut tables);
        let table = dom.first_descendant_by_tag(dom.root(), Tag::Table).unwrap();
        assert_eq!(store.is_data_table(table), Some(true));
    }

    #[test]
    fn nested_table_evidence_is_aggregated_without_rescanning() {
        let dom = Dom::parse_fragment(
            "<table><tr><td>outer</td><td><table><tr><th>Field</th><th>Value</th></tr><tr><td>A</td><td>B</td></tr></table></td></tr></table>",
            Tag::Div,
        )
        .unwrap();
        let mut store = NodeStateStore::new();
        let mut tables = Vec::new();
        mark_data_tables(&dom, dom.root(), &mut store, &mut tables);
        let table_ids: Vec<_> = dom
            .descendants(dom.root())
            .filter(|&node| dom.tag(node) == Some(Tag::Table))
            .collect();
        assert_eq!(table_ids.len(), 2);
        assert_eq!(store.is_data_table(table_ids[0]), Some(true));
        assert_eq!(store.is_data_table(table_ids[1]), Some(true));
    }

    #[test]
    fn workspace_classifies_a_table_fragment_root() {
        let dom = Dom::parse_fragment(
            "<table><thead><tr><th>Field</th></tr></thead><tbody><tr><td>Value</td></tr></tbody></table>",
            Tag::Div,
        )
        .unwrap();
        let table = dom.first_descendant_by_tag(dom.root(), Tag::Table).unwrap();
        let mut store = NodeStateStore::new();
        let mut tables = Vec::new();
        let mut workspace = FragmentWorkspace::default();
        mark_data_tables_in_workspace(&dom, table, &mut store, &mut tables, &mut workspace);

        assert_eq!(tables, vec![table]);
        assert_eq!(store.is_data_table(table), Some(true));
    }

    #[test]
    fn explicit_data_table_semantics_prevent_listing_classification() {
        for attribute in [r#"role="table""#, r#"datatable="1""#] {
            let html = format!(
                r#"<table {attribute}><tr><td>1.</td><td><a href='/a'>A</a></td></tr><tr><td></td><td>A details</td></tr><tr><td>2.</td><td><a href='/b'>B</a></td></tr><tr><td></td><td>B details</td></tr><tr><td>3.</td><td><a href='/c'>C</a></td></tr><tr><td></td><td>C details</td></tr></table>"#
            );
            let dom = Dom::parse_fragment(&html, Tag::Div).unwrap();
            let table = dom.first_descendant_by_tag(dom.root(), Tag::Table).unwrap();
            assert!(repeated_listing_start(&dom, table).is_none());
        }
    }

    #[test]
    fn hard_cleanup_preserves_only_content_checkboxes() {
        let mut dom = Dom::parse_fragment(
            r#"<ul><li><label><input class="control" onclick="bad()" alt="bad" action="bad" aria-level="2" data-type="bad" data-footnote="bad" type="checkbox" checked> Done</label></li><li><form><input type="checkbox"> Option</form></li></ul><form><input type="checkbox"><button>Search</button></form>"#,
            Tag::Div,
        )
        .unwrap();
        let root = dom.root();
        let evidence = crate::document::SourceEvidence::analyze(&dom, root, &NodeStateStore::new());
        hard_cleanup(
            &mut dom,
            root,
            &Regex::new("$").unwrap(),
            false,
            &evidence,
            &mut Vec::new(),
        );
        let inputs: Vec<_> = dom
            .descendants(root)
            .filter(|&node| dom.tag(node) == Some(Tag::Input))
            .collect();
        assert_eq!(inputs.len(), 1);
        assert!(dom.has_attr(inputs[0], AttrName::Checked));
        assert!(dom.has_attr(inputs[0], AttrName::Disabled));
        assert_eq!(dom.attr(inputs[0], AttrName::Class), None);
        for name in [
            "onclick",
            "alt",
            "action",
            "aria-level",
            "data-type",
            "data-footnote",
        ] {
            assert_eq!(dom.attr_by_local_name(inputs[0], name), None, "{name}");
        }
    }

    #[test]
    fn hard_cleanup_removes_controls_and_keeps_form_text() {
        let text = clean_fragment(
            r#"<form><p>Configuration details remain useful.</p><label>Name<input></label><button>Submit</button></form><script>bad()</script>"#,
        );
        assert!(text.contains("Configuration details"), "{text}");
        assert!(!text.contains("Submit"), "{text}");
        assert!(!text.contains("bad"), "{text}");
    }

    #[test]
    fn heuristic_cleanup_removes_strong_clutter() {
        let text = clean_fragment(
            r#"<main><p>Primary documentation remains.</p>
            <nav class="menu"><a href="/a">A</a><a href="/b">B</a><a href="/c">C</a></nav>
            <aside class="related"><h2>Related stories</h2><a href="/1">One story</a><a href="/2">Two story</a></aside>
            <div class="social-share"><a href="https://twitter.com/share">Twitter</a><a href="https://facebook.com/share">Facebook</a></div>
            <aside class="newsletter"><p>Subscribe to our newsletter</p><form><input><button>Join</button></form></aside>
            <aside class="author-bio"><p>About the author</p><a href="/author">Profile</a><a href="https://x.com/a">Social</a></aside>
            <div class="advertisement"><a href="/buy">Sponsored</a></div></main>"#,
        );
        assert!(text.contains("Primary documentation"), "{text}");
        for clutter in [
            "One story",
            "Twitter",
            "Subscribe",
            "About the author",
            "Sponsored",
        ] {
            assert!(!text.contains(clutter), "retained {clutter}: {text}");
        }
    }

    #[test]
    fn removes_decorative_separator_paragraphs_next_to_rules() {
        let mut dom = Dom::parse_fragment(
            "<article><p>Opening content.</p><p>###</p><hr><p>Closing content.</p></article>",
            Tag::Div,
        )
        .unwrap();
        let root = dom.root();
        let mut store = NodeStateStore::new();
        let evidence = crate::document::SourceEvidence::analyze(&dom, root, &store);
        let mut workspace = FragmentWorkspace::default();

        assert!(remove_decorative_separator_paragraphs_in_workspace(
            &mut dom,
            root,
            &mut store,
            &evidence,
            &mut workspace,
        ));
        assert!(!dom.text(root).contains("###"));
        assert!(dom.text(root).contains("Opening content"));
        assert!(dom.text(root).contains("Closing content"));
    }

    #[test]
    fn removes_named_revision_history_from_global_chrome() {
        let mut dom = Dom::parse_fragment(
            "<div class='note-changes meta'>Recent changes: Last updated today.</div><article><p>Retained article content.</p></article>",
            Tag::Div,
        )
        .unwrap();
        let root = dom.root();
        let mut store = NodeStateStore::new();
        let evidence = crate::document::SourceEvidence::analyze(&dom, root, &store);
        let mut workspace = FragmentWorkspace::default();

        assert!(remove_global_chrome_in_workspace(
            &mut dom,
            root,
            &mut store,
            &evidence,
            &mut workspace,
        ));
        assert!(!dom.text(root).contains("Recent changes"));
        assert!(dom.text(root).contains("Retained article content"));
    }

    #[test]
    fn global_chrome_keeps_a_dated_article_header_with_hero_media() {
        let text = clean_fragment(
            r#"<article><header><figure><img src="hero.jpg" alt="A complete benchmark result"><figcaption>The benchmark setup before the first run.</figcaption></figure><p class="byline"><a href="/author">A. Writer</a> · <time>August 19, 2026</time></p></header><p>The report describes the measured system in enough detail for another operator to repeat the complete experiment.</p><p>The results use the same input data, environment, and validation procedure for every recorded run.</p></article>"#,
        );

        assert!(text.contains("benchmark setup"), "{text}");
        assert!(text.contains("measured system"), "{text}");
    }

    #[test]
    fn global_chrome_keeps_a_compact_readme_identity_block() {
        let text = clean_fragment(
            r#"<article><div align="center"><a href="/logo"><img src="logo.png" alt="Project logo"></a><p>A compact tool for useful work.</p><p><a href="/start">Quick Start</a> · <a href="/guide">Guide</a> · <a href="/chat">Chat</a></p></div><hr><p>The project documentation explains the implementation and gives enough useful detail for readers.</p><p>A second paragraph describes how to install, configure, and validate the complete tool.</p></article>"#,
        );

        assert!(text.contains("compact tool"), "{text}");
        assert!(text.contains("Quick Start"), "{text}");
    }

    #[test]
    fn global_footer_hoists_identity_beside_pagination() {
        let mut dom = Dom::parse_fragment(
            r#"<div><p>The retained document contains enough prose to make the footer peripheral and to preserve the useful page identity.</p><footer class="site-footer"><a class="pagination-link" aria-label="Pagination navigation" href="/previous">Previous article</a><a class="pagination-link" href="/next">Next article</a><div class="site-identity"><a href="/">Example Docs</a></div></footer></div>"#,
            Tag::Div,
        )
        .unwrap();
        let root = dom.root();
        let mut store = NodeStateStore::new();
        let evidence = crate::document::SourceEvidence::analyze(&dom, root, &store);
        let mut workspace = FragmentWorkspace::default();

        assert!(remove_global_chrome_in_workspace(
            &mut dom,
            root,
            &mut store,
            &evidence,
            &mut workspace,
        ));
        let text = dom.text(root);
        assert!(text.contains("Example Docs"), "{text}");
        assert!(!text.contains("Previous article"), "{text}");
        assert!(!text.contains("Next article"), "{text}");
    }

    #[test]
    fn terminal_peripheral_sequence_removes_weak_sibling_blocks() {
        let text = clean_fragment(
            r#"<article><h1>Measured extraction results</h1><p>The report explains the complete extraction method, the input corpus, and the checks that verify each result. It gives enough detail for another person to repeat the work with the same inputs and compare every output.</p><p>The next section records the observed behavior for all tested documents. It keeps the source facts, the measured values, and the limits of the evaluation so the conclusion remains useful.</p><hr><p>Subscribe to get new posts by email.</p><p>Preview of a related post</p><div class="related-card"><a href="/next">Continue reading another report</a></div><footer><a href="/about">About</a></footer></article>"#,
        );

        assert!(text.contains("observed behavior"), "{text}");
        assert!(!text.contains("Subscribe"), "{text}");
        assert!(!text.contains("Continue reading"), "{text}");
        assert!(!text.contains("About"), "{text}");
    }

    #[test]
    fn terminal_peripheral_sequence_removes_one_explicit_related_section() {
        let text = clean_fragment(
            r#"<article><p>The report explains the complete extraction method, the input corpus, and the checks that verify each result. It gives enough detail for another person to repeat the work with the same inputs and compare every output.</p><section><h2>Read next</h2><div><a href="/one">First related report</a><a href="/two">Second related report</a></div></section></article>"#,
        );

        assert!(text.contains("complete extraction method"), "{text}");
        assert!(!text.contains("First related report"), "{text}");
        assert!(!text.contains("Second related report"), "{text}");
    }

    #[test]
    fn terminal_peripheral_sequence_recognizes_custom_related_heading() {
        let text = clean_fragment(
            r#"<article><p>The report explains the complete extraction method, the input corpus, and the checks that verify each result. It gives enough detail for another person to repeat the work with the same inputs and compare every output.</p><section><section-title>Related articles</section-title><div><a href="/one">First related report</a><a href="/two">Second related report</a></div></section></article>"#,
        );

        assert!(text.contains("complete extraction method"), "{text}");
        assert!(!text.contains("First related report"), "{text}");
        assert!(!text.contains("Second related report"), "{text}");
    }

    #[test]
    fn terminal_peripheral_sequence_keeps_a_single_related_reference() {
        let text = clean_fragment(
            r#"<article><h1>Measured extraction results</h1><p>The report explains the complete extraction method, the input corpus, and the checks that verify each result. It gives enough detail for another person to repeat the work with the same inputs and compare every output.</p><p>The next section records the observed behavior for all tested documents. It keeps the source facts, the measured values, and the limits of the evaluation so the conclusion remains useful.</p><p>Related work: <a href="/prior">the prior analysis</a> supplies the baseline for this result.</p></article>"#,
        );

        assert!(text.contains("Related work"), "{text}");
        assert!(text.contains("prior analysis"), "{text}");
    }

    #[test]
    fn terminal_peripheral_sequence_keeps_linked_concluding_prose() {
        let text = clean_fragment(
            r#"<article><h1>Measured extraction results</h1><p>The report explains the complete extraction method, the input corpus, and the checks that verify each result. It gives enough detail for another person to repeat the work with the same inputs and compare every output.</p><p>The next section records the observed behavior for all tested documents. It keeps the source facts, the measured values, and the limits of the evaluation so the conclusion remains useful.</p><p>The complete source remains in the <a href="/archive">public archive</a>.</p><footer class="post-footer"><a href="/about">About</a></footer></article>"#,
        );

        assert!(text.contains("complete source remains"), "{text}");
        assert!(text.contains("public archive"), "{text}");
    }

    #[test]
    fn terminal_peripheral_sequence_keeps_content_navigation_and_footer() {
        let text = clean_fragment(
            r##"<article><h1>Measured extraction results</h1><p>The report explains the complete extraction method, the input corpus, and the checks that verify each result. It gives enough detail for another person to repeat the work with the same inputs and compare every output.</p><p>The next section records the observed behavior for all tested documents. It keeps the source facts, the measured values, and the limits of the evaluation so the conclusion remains useful.</p><nav class="hlist"><a href="#method">Method</a><a href="#results">Results</a><a href="#limits">Limits</a></nav><footer class="related post-footer"><p>The source material remains available under the <a href="/license">project license</a>, with notes that explain the collection method and its limits.</p></footer></article>"##,
        );

        assert!(text.contains("Method"), "{text}");
        assert!(text.contains("source material remains available"), "{text}");
    }

    #[test]
    fn repeated_cleanup_keeps_an_inline_terminal_about_link() {
        let text = clean_fragment(
            r#"<article><h1>Profile links in prose</h1><p>The report explains the complete extraction method, the input corpus, and the checks that verify each result. It gives enough detail for another person to repeat the work with the same inputs and compare every output.</p><p>The final paragraph tells readers where they can learn <a href="/author">About me</a>.</p></article>"#,
        );

        assert!(text.contains("About me"), "{text}");
    }

    #[test]
    fn global_chrome_keeps_meaningful_terminal_article_prose() {
        let text = clean_fragment(
            r#"<article><p>The report explains how the implementation processes each record and validates the final result.</p><p>The method repeats the operation with stable inputs so another operator can verify the result.</p><footer class="related post-footer"><p>The source material remains available under the <a href="/license">project license</a>. Read the <a href="/sources">source notes</a> for the collection method and the limits that apply to this published result.</p></footer></article>"#,
        );

        assert!(text.contains("source material remains available"), "{text}");
        assert!(text.contains("collection method"), "{text}");
    }

    #[test]
    fn heuristic_cleanup_removes_trailing_audio_and_collection_cards() {
        let text = clean_fragment(
            r#"<article><p>The product report explains the complete result and the purchase options.</p>
            <div class="article-audio-player"><time>August 13, 2026</time><span> | </span><p>Listen to article</p><span>[[duration]] minutes</span><button>Play</button></div>
            <section class="collection-grid"><h2>Collection</h2><p>Made by Google 2026</p><div class="cards"><a href="/one"><img src="one.jpg" alt="First story">First story</a><a href="/two"><img src="two.jpg" alt="Second story">Second story</a><a href="/three"><img src="three.jpg" alt="Third story">Third story</a></div></section></article>"#,
        );
        assert!(text.contains("product report"), "{text}");
        for clutter in [
            "August 13",
            "Listen to article",
            "[[duration]]",
            "Collection",
            "Made by Google",
            "First story",
        ] {
            assert!(!text.contains(clutter), "retained {clutter}: {text}");
        }
    }

    #[test]
    fn heuristic_cleanup_removes_consecutive_terminal_recommendation_sections() {
        let text = clean_fragment(
            r#"<article><p>The report explains the measured result and its effect in enough detail for readers to understand the complete finding.</p>
            <section><h2>Related</h2><div class="cards"><a href="/one"><img src="one.jpg" alt="One"><h3>First related report</h3><p>A short summary.</p></a><a href="/two"><img src="two.jpg" alt="Two"><h3>Second related report</h3><p>Another summary.</p></a></div></section>
            <section><h2>More from the publisher</h2><div class="cards"><a href="/three"><img src="three.jpg" alt="Three"><h3>Third report</h3><p>A short summary.</p></a><a href="/four"><img src="four.jpg" alt="Four"><h3>Fourth report</h3><p>Another summary.</p></a><a href="/five"><img src="five.jpg" alt="Five"><h3>Fifth report</h3><p>A final summary.</p></a></div></section></article>"#,
        );
        assert!(text.contains("measured result"), "{text}");
        for clutter in ["First related report", "Third report", "Fifth report"] {
            assert!(!text.contains(clutter), "retained {clutter}: {text}");
        }
    }

    #[test]
    fn heuristic_cleanup_prefers_a_table_over_its_responsive_card_duplicate() {
        let text = clean_fragment(
            r#"<main><p>The benchmark compares the validated result for each model.</p><div>
            <div class="md:hidden"><article>1 Alpha Model 2,700</article><article>2 Beta Model 2,800</article><article>3 Gamma Model 2,900</article><article>4 Delta Model 3,000</article></div>
            <div class="hidden overflow-x-auto md:block"><table><thead><tr><th>Model</th><th>Record</th></tr></thead><tbody><tr><td>Alpha Model</td><td>2,700</td></tr><tr><td>Beta Model</td><td>2,800</td></tr><tr><td>Gamma Model</td><td>2,900</td></tr><tr><td>Delta Model</td><td>3,000</td></tr></tbody></table></div>
            </div></main>"#,
        );
        assert!(text.contains("benchmark compares"), "{text}");
        for model in ["Alpha Model", "Beta Model", "Gamma Model", "Delta Model"] {
            assert_eq!(text.matches(model).count(), 1, "{model}: {text}");
        }
    }

    #[test]
    fn heuristic_cleanup_keeps_distinct_responsive_views() {
        let text = clean_fragment(
            r#"<main><p>The report contains two useful views.</p><div><div class="md:hidden"><p>A compact explanation for small screens remains useful.</p></div><div class="hidden md:block"><table><thead><tr><th>Model</th><th>Record</th></tr></thead><tbody><tr><td>Alpha</td><td>1</td></tr><tr><td>Beta</td><td>2</td></tr><tr><td>Gamma</td><td>3</td></tr><tr><td>Delta</td><td>4</td></tr></tbody></table></div></div></main>"#,
        );
        assert!(text.contains("compact explanation"), "{text}");
        assert!(text.contains("Alpha"), "{text}");
    }

    #[test]
    fn heuristic_cleanup_does_not_treat_interaction_states_as_breakpoints() {
        let dom = Dom::parse_fragment(
            r#"<div><div class="hover:hidden">Interactive card</div><div class="hidden hover:block">Interactive table</div></div>"#,
            Tag::Div,
        )
        .unwrap();
        let variants: Vec<_> = dom
            .descendants(dom.root())
            .filter(|&node| dom.attr(node, AttrName::Class).is_some())
            .collect();

        assert!(responsive_visibility(&dom, variants[0]).is_none());
        assert!(responsive_visibility(&dom, variants[1]).is_none());
    }

    #[test]
    fn heuristic_cleanup_distinguishes_next_step_cards_from_instructions() {
        let cards = clean_fragment(
            r#"<article><p>Primary documentation remains.</p><h2>Next Steps</h2><div class="card-group"><a href="/one">Setup guide</a><a href="/two">Deployment guide</a></div></article>"#,
        );
        assert!(cards.contains("Primary documentation"), "{cards}");
        assert!(!cards.contains("Setup guide"), "{cards}");

        let instructions = clean_fragment(
            r#"<article><p>Primary documentation remains.</p><section><h2>Next Steps</h2><p>Follow the <a href="/setup">setup procedure</a>.</p><p>Then use the <a href="/deploy">deployment procedure</a>.</p></section></article>"#,
        );
        assert!(instructions.contains("Next Steps"), "{instructions}");
        assert!(instructions.contains("setup procedure"), "{instructions}");
        assert!(
            instructions.contains("deployment procedure"),
            "{instructions}"
        );

        let unrelated_cards = clean_fragment(
            r#"<article><div class="card-group"><a href="/one">Earlier card</a><a href="/two">Another card</a></div><p>Primary documentation remains.</p><h2>Next Steps</h2><p>Follow the <a href="/setup">setup procedure</a>.</p><p>Then use the <a href="/deploy">deployment procedure</a>.</p></article>"#,
        );
        assert!(unrelated_cards.contains("Next Steps"), "{unrelated_cards}");
        assert!(
            unrelated_cards.contains("setup procedure"),
            "{unrelated_cards}"
        );

        for list in ["ol", "ul"] {
            let html = format!(
                r#"<article><p>Primary documentation remains.</p><section><h2>Next Steps</h2><{list}><li>Open the <a href="/setup">setup guide</a>.</li><li>Run the <a href="/deploy">deployment procedure</a>.</li></{list}></section></article>"#
            );
            let instructions = clean_fragment(&html);
            assert!(
                instructions.contains("Next Steps"),
                "{list}: {instructions}"
            );
            assert!(
                instructions.contains("setup guide"),
                "{list}: {instructions}"
            );
            assert!(
                instructions.contains("deployment procedure"),
                "{list}: {instructions}"
            );
        }
    }

    #[test]
    fn heuristic_cleanup_removes_terminal_action_paragraphs() {
        let text = clean_fragment(
            r#"<article><p>This substantive article paragraph explains the complete result and gives useful context to readers.</p><p class="button-wrapper"><a href="/story/comments">Leave a comment</a></p><p class="button-wrapper"><a href="/story?action=share">Share</a></p></article>"#,
        );
        assert!(text.contains("substantive article"), "{text}");
        assert!(!text.contains("Leave a comment"), "{text}");
        assert!(!text.contains("Share"), "{text}");
    }

    #[test]
    fn heuristic_cleanup_removes_terminal_taxonomy_name_variants() {
        for class in ["entity-list", "entity_list", "tag-list", "tag_list"] {
            let html = format!(
                r#"<article><p>This substantive article paragraph explains the complete result and gives useful context to readers.</p><div class="{class}"><a href="/a">Alpha</a><a href="/b">Beta</a></div></article>"#
            );
            let text = clean_fragment(&html);
            assert!(text.contains("substantive article"), "{class}: {text}");
            assert!(!text.contains("Alpha"), "{class}: {text}");
        }
    }

    #[test]
    fn heuristic_cleanup_removes_contextual_text_boilerplate() {
        let text = clean_fragment(
            r#"<article><p class="reading-time">5 min read</p><p>This substantive article paragraph explains the complete result and gives useful context to readers.</p><span class="ad-label">REG AD</span><p><a href="/more">Read more</a></p><p>Advertisement</p></article>"#,
        );
        assert!(text.contains("substantive article"), "{text}");
        for clutter in ["5 min read", "REG AD", "Read more", "Advertisement"] {
            assert!(!text.contains(clutter), "retained {clutter}: {text}");
        }
    }

    #[test]
    fn heuristic_cleanup_keeps_boilerplate_words_in_prose() {
        let text = clean_fragment(
            r#"<article><p>The advertisement changed television forever.</p><p>This guide takes five minutes to read more carefully, and it explains why people share this article in class.</p></article>"#,
        );
        assert!(text.contains("advertisement changed television"), "{text}");
        assert!(text.contains("read more carefully"), "{text}");
        assert!(text.contains("share this article in class"), "{text}");
    }

    #[test]
    fn heuristic_cleanup_keeps_ui_phrases_in_authored_content() {
        let text = clean_fragment(
            r##"<article><p>The files were <span>copied</span> successfully.</p><h2><a href="#ownership">Unlock the full potential of Rust</a></h2><p>This section explains ownership and borrowing.</p></article>"##,
        );
        assert!(text.contains("files were copied successfully"), "{text}");
        assert!(text.contains("Unlock the full potential of Rust"), "{text}");
    }

    #[test]
    fn heuristic_cleanup_removes_terminal_document_maintenance() {
        let text = clean_fragment(
            r#"<article><p>This guide explains the complete procedure and the checks that confirm the final result for each supported input.</p><div id="pre-footer"><h2>Feedback</h2><p>Was this page helpful?</p><button>Yes</button><button>No</button><p>Last modified August 19, 2026.</p><a href="/edit">Edit this page</a></div></article>"#,
        );
        assert!(text.contains("complete procedure"), "{text}");
        assert!(!text.contains("Was this page helpful"), "{text}");
        assert!(!text.contains("Last modified"), "{text}");
    }

    #[test]
    fn heuristic_cleanup_keeps_maintenance_words_in_authored_prose() {
        let text = clean_fragment(
            r#"<article><p>This report asks whether the page was helpful because that question is part of the research method.</p><footer><p>The last updated model remains the subject of this authored conclusion, and the report explains why.</p></footer><section class="lastmodel-results"><p>Last updated model results remain part of the authored analysis.</p><a href="/results">Review the model results</a></section></article>"#,
        );
        assert!(text.contains("part of the research method"), "{text}");
        assert!(text.contains("authored conclusion"), "{text}");
        assert!(text.contains("Last updated model results"), "{text}");
    }

    #[test]
    fn heuristic_cleanup_keeps_authored_update_language_in_a_footer() {
        let text = clean_fragment(
            r#"<article><p>This report explains the study and its findings.</p><footer class="article-footer"><p>Last updated estimates show that treatment improved outcomes for the full study group.</p></footer></article>"#,
        );
        assert!(text.contains("treatment improved outcomes"), "{text}");
    }

    #[test]
    fn heuristic_cleanup_keeps_an_authored_news_link_section() {
        for container in ["section", "aside"] {
            let html = format!(
                r#"<article><p>This digest explains the changes that affect the project and gives readers enough context to choose the relevant report.</p><{container}><h3>News</h3><ul><li><a href="/one">Compiler release</a></li><li><a href="/two">Library release</a></li><li><a href="/three">Tooling release</a></li><li><a href="/four">Community report</a></li></ul></{container}></article>"#,
            );
            let text = clean_fragment(&html);
            assert!(text.contains("News"), "{container}: {text}");
            assert!(text.contains("Compiler release"), "{container}: {text}");
            assert!(text.contains("Community report"), "{container}: {text}");
        }
    }

    #[test]
    fn heuristic_cleanup_keeps_protected_content_in_promo_named_regions() {
        let text = clean_fragment(
            r#"<article><h1>Promotion API</h1><div class="promo-example"><a href="/api">API reference</a><pre><code>promotion.enable()</code></pre></div><p>The method enables a promotion for the selected account.</p></article>"#,
        );
        assert!(text.contains("promotion.enable()"), "{text}");
        assert!(text.contains("API reference"), "{text}");
    }

    #[test]
    fn heuristic_cleanup_removes_a_redundant_explicit_toc() {
        let text = clean_fragment(
            r##"<article><h1>Client guide</h1><p>This guide explains the complete client workflow and the checks that validate each operation.</p><nav id="toc"><h2>Contents</h2><ol><li><a href="#setup">Setup</a></li><li><a href="#usage">Usage</a></li></ol></nav><h2 id="setup">Setup</h2><p>Install the client and set its endpoint.</p><h2 id="usage">Usage</h2><p>Run the client and inspect each result.</p></article>"##,
        );
        assert!(!text.contains("Contents"), "{text}");
        assert!(text.contains("Install the client"), "{text}");
        assert!(text.contains("inspect each result"), "{text}");
    }

    #[test]
    fn heuristic_cleanup_does_not_match_toc_inside_an_unrelated_name() {
        let text = clean_fragment(
            r##"<article><h1>Market report</h1><p>This report explains the complete market result and its measured effect.</p><div id="stock-list"><ol><li><a href="#alpha">Alpha stock</a></li><li><a href="#beta">Beta stock</a></li></ol></div><h2 id="alpha">Alpha</h2><p>The Alpha result remains stable.</p><h2 id="beta">Beta</h2><p>The Beta result increased.</p></article>"##,
        );
        assert!(text.contains("Alpha stock"), "{text}");
        assert!(text.contains("Beta stock"), "{text}");
    }

    #[test]
    fn heuristic_cleanup_bounds_many_compact_rail_probes() {
        let mut html = String::from(
            "<main><article><p>The primary report contains enough detailed prose to remain the selected content after cleanup.</p></article>",
        );
        for index in 0..500 {
            html.push_str(&format!(
                r#"<aside><h3>News</h3><ul><li><a href="/{index}/1">One</a></li><li><a href="/{index}/2">Two</a></li><li><a href="/{index}/3">Three</a></li><li><a href="/{index}/4">Four</a></li></ul></aside>"#,
            ));
        }
        html.push_str("</main>");

        let text = clean_fragment(&html);
        assert!(text.contains("primary report"), "{text}");
    }

    #[test]
    fn removes_repeated_blocks_and_comment_regions() {
        let text = clean_fragment(
            r#"<article><p>Primary content remains useful and complete.</p>
            <section class="faq"><h2>FAQ</h2><p>The same answer explains the stable behavior in enough detail for readers.</p></section>
            <section class="faq"><h2>FAQ</h2><p>The same answer explains the stable behavior in enough detail for readers.</p></section>
            <section id="comments"><h2>Comments</h2><p>A reader reply with useful-looking text.</p><a href="/reply">Reply</a><a href="/reply-two">Reply</a></section></article>"#,
        );
        assert!(text.contains("Primary content remains"), "{text}");
        assert_eq!(
            text.matches("The same answer explains").count(),
            1,
            "{text}"
        );
        assert!(!text.contains("reader reply"), "{text}");
    }

    #[test]
    fn preserves_repeated_standalone_prose() {
        let text = clean_fragment(
            r#"<article><p>The same wording can be valid when it appears in two separate paragraphs with different context.</p><p>The same wording can be valid when it appears in two separate paragraphs with different context.</p></article>"#,
        );
        assert_eq!(
            text.matches("The same wording can be valid").count(),
            2,
            "{text}"
        );
    }

    #[test]
    fn preserves_comment_regions_for_discussion_pages() {
        let mut dom = Dom::parse_fragment(
            r#"<main><article><p>Primary post.</p><section id="comments"><p>Reply body.</p><a href="/reply">Reply</a><a href="/reply-two">Reply</a></section></article></main>"#,
            Tag::Div,
        )
        .unwrap();
        let root = dom.root();
        let mut store = NodeStateStore::new();
        let evidence = crate::document::SourceEvidence::analyze(&dom, root, &store);
        let mut workspace = FragmentWorkspace::default();
        remove_repeated_and_discussion_content_in_workspace(
            &mut dom,
            root,
            PageKind::Discussion,
            &mut store,
            &evidence,
            &mut workspace,
        );
        assert!(dom.text(root).contains("Reply body"));
    }

    #[test]
    fn heuristic_cleanup_invalidates_stats_after_nested_removal() {
        let advertisements = "<p>Advertisement</p>".repeat(10);
        let html = format!(
            "<article><p>Useful article content.</p><p><a href=\"/more\">Read more</a></p><div>{advertisements}</div></article>"
        );
        let text = clean_fragment(&html);
        assert!(text.contains("Useful article content"), "{text}");
        assert!(!text.contains("Read more"), "{text}");
        assert!(!text.contains("Advertisement"), "{text}");
    }

    #[test]
    fn heuristic_cleanup_keeps_substantial_callouts_and_documentation_toc() {
        let text = clean_fragment(
            r##"<main>
            <aside class="sidebar callout"><h2>Compatibility note</h2><p>This callout contains substantial guidance. It explains supported systems, migration constraints, failure behavior, recovery steps, and validation requirements.</p><pre><code>cargo test</code></pre></aside>
            <nav aria-label="On this page"><h2>Contents</h2><a href="#one">Installation and configuration reference</a><a href="#two">Detailed API behavior and examples</a><a href="#three">Troubleshooting and recovery guidance</a></nav>
            <p>The primary guide provides complete instructions.</p></main>"##,
        );
        assert!(text.contains("Compatibility note"), "{text}");
        assert!(text.contains("cargo test"), "{text}");
        assert!(text.contains("Installation and configuration"), "{text}");
    }

    #[test]
    fn hard_cleanup_preserves_lazy_tracking_placeholders() {
        let mut dom = Dom::parse_fragment(
            r#"<img width="1" height="1" src="blank.gif" data-src="photo.jpg" alt="Photo"><picture data-src="other.jpg"><img width="1" alt="Other"></picture><img width="1" data-lazy-src="lazy.jpg"><img height="1" data-original="original.jpg">"#,
            Tag::Div,
        )
        .unwrap();
        let root = dom.root();
        let image = dom.first_descendant_by_tag(root, Tag::Img).unwrap();
        let evidence = crate::document::SourceEvidence::analyze(&dom, root, &NodeStateStore::new());
        hard_cleanup(
            &mut dom,
            root,
            &Regex::new("$").unwrap(),
            false,
            &evidence,
            &mut Vec::new(),
        );
        assert!(dom.parent(image).is_some());
        let picture_image = dom
            .descendants(root)
            .filter(|&node| dom.tag(node) == Some(Tag::Img))
            .nth(1)
            .unwrap();
        assert!(dom.parent(picture_image).is_some());
        let remaining_images = dom
            .descendants(root)
            .filter(|&node| dom.tag(node) == Some(Tag::Img))
            .count();
        assert_eq!(remaining_images, 4);
    }

    #[test]
    fn hard_cleanup_preserves_math_fallback_images() {
        let mut dom = Dom::parse_fragment(
            r#"<math><mi>x</mi></math><img class="mwe-math-fallback-image-inline" aria-hidden="true" src="equation.svg" alt="x">"#,
            Tag::Div,
        )
        .unwrap();
        let root = dom.root();
        let evidence = crate::document::SourceEvidence::analyze(&dom, root, &NodeStateStore::new());
        hard_cleanup(
            &mut dom,
            root,
            &Regex::new("$").unwrap(),
            false,
            &evidence,
            &mut Vec::new(),
        );
        assert!(dom.first_descendant_by_tag(root, Tag::Img).is_some());
    }

    #[test]
    fn heuristic_cleanup_scans_nested_boundaries_once() {
        let depth = 2_000;
        let mut html = "<div class=\"sidebar\">".repeat(depth);
        html.push_str("Retained documentation.");
        html.push_str(&"</div>".repeat(depth));
        let text = clean_fragment(&html);
        assert!(text.contains("Retained documentation"));
    }

    #[test]
    fn heuristic_cleanup_indexes_many_forms_and_related_sections_once() {
        let mut html = String::from("<main><p>Retained documentation.</p>");
        for index in 0..2_000 {
            html.push_str(&format!(
                "<form></form><section><p>Get updates in your inbox.</p><form><label>Email<input></label></form></section><aside class=\"related-links\"><h2>Related</h2><a href=\"/{index}/a\">A</a><a href=\"/{index}/b\">B</a></aside>"
            ));
        }
        html.push_str("</main>");

        let text = clean_fragment(&html);

        assert_eq!(text, "Retained documentation.");
    }

    #[test]
    fn preserves_svg_presentation_attributes() {
        let mut dom = Dom::parse_document(
            r#"<svg width="10" height="10"><path fill="red" stroke="blue"/></svg>"#,
        )
        .unwrap();
        let root = dom.root();
        let svg = dom.first_descendant_by_tag(root, Tag::Svg).unwrap();
        let path = dom.first_descendant_by_tag(svg, Tag::Svg).unwrap();
        clean_styles(&mut dom, root, &mut Vec::new());
        assert_eq!(dom.attr_by_local_name(svg, "width"), Some("10"));
        assert_eq!(dom.attr_by_local_name(path, "fill"), Some("red"));
    }
}
