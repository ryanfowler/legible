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
use html5ever::{LocalName, QualName, ns};
use regex::Regex;
use smallvec::SmallVec;
use std::collections::HashMap;

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
    }

    /// Starts a new fragment epoch without releasing reusable buffers.
    pub(crate) fn reset(&mut self) {
        self.invalidate();
        self.preorder.clear();
        self.elements_with_depth.clear();
        self.scratch_u32.clear();
        self.scratch_bytes.clear();
        self.scratch_bits.clear();
    }

    /// Returns one DOM-preorder snapshot for the current fragment version.
    pub(crate) fn ensure_snapshot(&mut self, dom: &Dom, root: NodeId) {
        if self.snapshot_epoch == Some(self.mutation_epoch) && self.snapshot_root == Some(root) {
            return;
        }

        crate::instrumentation::record_source_full_scan();
        crate::instrumentation::record_source_element_snapshot();
        self.preorder.clear();
        self.elements_with_depth.clear();
        self.preorder.push(root);
        let Some(first_child) = dom.first_child(root) else {
            self.snapshot_epoch = Some(self.mutation_epoch);
            self.snapshot_root = Some(root);
            return;
        };

        let mut pending = Vec::<(NodeId, u32)>::new();
        pending.push((first_child, 1));
        while let Some((node, depth)) = pending.pop() {
            self.preorder.push(node);
            if dom.is_element(node) {
                self.elements_with_depth.push((node, depth));
            }
            if let Some(sibling) = dom.next_sibling(node) {
                pending.push((sibling, depth));
            }
            if let Some(child) = dom.first_child(node) {
                pending.push((child, depth.saturating_add(1)));
            }
        }
        self.snapshot_epoch = Some(self.mutation_epoch);
        self.snapshot_root = Some(root);
    }

    #[allow(dead_code)]
    pub(crate) fn preorder(&self) -> &[NodeId] {
        &self.preorder
    }

    pub(crate) fn elements_with_depth(&self) -> &[(NodeId, u32)] {
        &self.elements_with_depth
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

    #[allow(dead_code)]
    pub(crate) fn scratch_u32(&mut self, len: usize) -> &mut [u32] {
        self.scratch_u32.resize(len, 0);
        &mut self.scratch_u32[..len]
    }

    #[allow(dead_code)]
    pub(crate) fn scratch_bytes(&mut self, len: usize) -> &mut [u8] {
        self.scratch_bytes.resize(len, 0);
        &mut self.scratch_bytes[..len]
    }

    #[allow(dead_code)]
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
fn is_directly_protected(
    dom: &Dom,
    id: NodeId,
    evidence: &crate::document::SourceEvidence,
) -> bool {
    evidence.is_semantic_source(id)
        || dom.attr(id, AttrName::DataFootnote).is_some()
        || dom.attr(id, AttrName::DataFootnotes).is_some()
        || dom.attr(id, AttrName::DataMath).is_some()
        || matches!(
            dom.tag(id),
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
        )
        || dom.tag(id) == Some(Tag::Table) && evidence.data_table(id)
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
                AttrName::Class => {
                    lazy |= v.split_whitespace().any(|x| x.eq_ignore_ascii_case("lazy"))
                }
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
        src.to_ascii_lowercase().contains("placeholder")
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
        .attr_by_local_name(first, "alt")
        .filter(|value| !value.is_empty())
        .is_some_and(|value| dom.attr_by_local_name(second, "alt") == Some(value));
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
            let hidden_alt = dom.attr_by_local_name(hidden_image, "alt");
            hidden_alt.is_some() && hidden_alt == dom.attr_by_local_name(visible_image, "alt")
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
                classes.split_ascii_whitespace().any(|class| {
                    class.eq_ignore_ascii_case("skip-link")
                        || class.to_ascii_lowercase().starts_with("skip-to-")
                })
            });
        let utility_visibility = has_hidden_utility_class(dom, node) && !accessible_skip_link;
        let static_visibility = has_static_hidden_marker(dom, node) || utility_visibility;
        let modal = dom.attr(node, AttrName::AriaModal) == Some("true")
            || dom.attr(node, AttrName::Role).is_some_and(|roles| {
                roles.split_whitespace().any(|role| {
                    role.eq_ignore_ascii_case("dialog") || role.eq_ignore_ascii_case("alertdialog")
                })
            })
            || static_visibility
                && dom.attr(node, AttrName::Class).is_some_and(|classes| {
                    classes.split_whitespace().any(|class| {
                        class.eq_ignore_ascii_case("modal") || class.eq_ignore_ascii_case("dialog")
                    })
                });
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
                        || dom.attr(ancestor, AttrName::Role).is_some_and(|roles| {
                            roles
                                .split_ascii_whitespace()
                                .any(|role| role.eq_ignore_ascii_case("listitem"))
                        })
                })
                .is_some_and(|ancestor| {
                    dom.tag(ancestor) == Some(Tag::Li)
                        || dom.attr(ancestor, AttrName::Role).is_some_and(|roles| {
                            roles
                                .split_ascii_whitespace()
                                .any(|role| role.eq_ignore_ascii_case("listitem"))
                        })
                });
        if content_checkbox {
            // Keep only the semantic state. The retained control is disabled,
            // so extracted HTML cannot change the source checklist.
            dom.remove_attr(node, AttrName::Other);
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
    scratch.bits.resize(dom.len().saturating_mul(2), false);
    scratch.bits.fill(false);
    let (discovered_boundaries, inspected_subscription) = scratch.bits.split_at_mut(dom.len());
    let link_counts = &mut scratch.bytes[..dom.len()];
    store.clear_stats();
    store.enable_link_lengths();
    get_or_compute_stats(dom, root, store);

    // Count links once. Discovery uses the capped index instead of rescanning
    // each related-content subtree for links.
    {
        let snapshot = workspace.elements_with_depth();
        for &(node, _) in snapshot {
            link_counts[node.index()] = u8::from(dom.tag(node) == Some(Tag::A));
        }
        for &(node, _) in snapshot.iter().rev() {
            if let Some(parent) = dom.parent(node) {
                link_counts[parent.index()] = link_counts[parent.index()]
                    .saturating_add(link_counts[node.index()])
                    .min(3);
            }
        }
        let mut table_depths = SmallVec::<[u32; 8]>::new();
        for &(node, depth) in snapshot {
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
        remove_explicit_peripheral_sections(dom, root, snapshot, link_counts, store)
    };
    if changed {
        workspace.invalidate();
        store.clear_stats();
    }
    let changed = {
        workspace.ensure_snapshot(dom, root);
        let snapshot = workspace.elements_with_depth();
        remove_terminal_taxonomy_before_footnotes(dom, root, snapshot, link_counts, store)
    };
    if changed {
        workspace.invalidate();
        store.clear_stats();
    }
    let changed = {
        workspace.ensure_snapshot(dom, root);
        let snapshot = workspace.elements_with_depth();
        remove_job_company_profiles(dom, root, page_kind, snapshot, store)
    };
    if changed {
        workspace.invalidate();
        store.clear_stats();
    }
    let changed = {
        workspace.ensure_snapshot(dom, root);
        let snapshot = workspace.elements_with_depth();
        remove_direct_peripheral_siblings(dom, root, snapshot, link_counts, store, evidence)
    };
    if changed {
        workspace.invalidate();
        store.clear_stats();
    }
    workspace.ensure_snapshot(dom, root);
    let snapshot = workspace.elements_with_depth();
    let root_length = get_or_compute_stats(dom, root, store).text_length.max(1);
    let protected_masks = snapshot
        .iter()
        .any(|&(_, depth)| depth > 64)
        .then(|| protected_masks(dom, root, evidence, snapshot, &mut scratch.u32_values));

    // Keep only outermost candidates. A classifier can inspect the complete
    // subtree once instead of rescanning every nested wrapper.
    let mut boundary_depth = None;
    for &(node, depth) in snapshot {
        if let Some(outer_depth) = boundary_depth {
            if depth > outer_depth && !discovered_boundaries[node.index()] {
                continue;
            }
            if depth <= outer_depth {
                boundary_depth = None;
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
        let text = get_inner_text(dom, node, text_buffer).to_ascii_lowercase();
        let name = node_name(dom, node);
        let links = dom
            .descendants(node)
            .filter(|&descendant| dom.tag(descendant) == Some(Tag::A))
            .count();
        let controls = dom
            .descendants(node)
            .filter(|&descendant| {
                matches!(
                    dom.tag(descendant),
                    Some(Tag::Input | Tag::Textarea | Tag::Select | Tag::Button)
                )
            })
            .count();
        let images = dom
            .descendants(node)
            .filter(|&descendant| dom.tag(descendant) == Some(Tag::Img))
            .take(4)
            .count();
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
        let has_form = dom.tag(node) == Some(Tag::Form)
            || dom.descendants(node).any(|descendant| {
                dom.tag(descendant) == Some(Tag::Form)
                    || dom.tag(descendant) == Some(Tag::Other)
                        && dom.attr_by_local_name(descendant, "action").is_some()
                        && node_name(dom, descendant).contains("newsletter-form")
            });
        let metrics = PeripheralMetrics {
            name: &name,
            text: &text,
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
        let related = is_related_content(dom, node, &metrics);

        let social_name = contains_any(&name, &["share", "social", "sharedaddy"]);
        let social_links = dom
            .descendants(node)
            .filter(|&descendant| {
                dom.tag(descendant) == Some(Tag::A)
                    && dom.attr(descendant, AttrName::Href).is_some_and(|href| {
                        contains_any(
                            &href.to_ascii_lowercase(),
                            &["facebook.", "twitter.", "x.com/", "linkedin.", "reddit."],
                        )
                    })
            })
            .count();
        let social = social_name && (social_links > 0 || links >= 2) && short;

        let signup = is_newsletter_cta(&metrics);

        let breadcrumb = is_breadcrumb(dom, node, &metrics);
        let navigation_semantic = dom.tag(node) == Some(Tag::Nav)
            || dom.attr(node, AttrName::Role).is_some_and(|role| {
                role.split_whitespace()
                    .any(|value| value.eq_ignore_ascii_case("navigation"))
            });
        let menu_name = contains_any(&name, &["menu", "navigation", "breadcrumb"]);
        let documentation_toc = dom
            .attr_by_local_name(node, "aria-label")
            .is_some_and(|label| {
                let label = label.trim().to_ascii_lowercase();
                label == "on this page" || label == "table of contents" || label == "contents"
            })
            || contains_any(
                &name,
                &["table-of-contents", "table_of_contents", "docs-toc"],
            );
        let navigation = navigation_semantic
            && !documentation_toc
            && !breadcrumb
            && (menu_name || links >= 3)
            && link_density >= 0.6
            && stats.text_length < 500;

        let author_name = contains_any(&name, &["author-bio", "author_bio", "profile", "bio"]);
        let inside_article_toc = std::iter::once(node)
            .chain(dom.ancestors(node))
            .any(|ancestor| {
                dom.attr_by_local_name(ancestor, "data-article-toc")
                    .is_some()
            });
        let author_card = (author_name || inside_article_toc)
            && short
            && (at_start || at_end)
            && links >= 1
            && images >= 1;
        let author = author_name && short && (social_links > 0 || links >= 2 || author_card);
        let author_promotion = is_author_promotion(dom, node, &metrics);
        let audio_controls = is_audio_controls(&metrics);
        let job_profile = is_job_profile_content(dom, node, page_kind, &metrics);
        let collection_promotion = is_collection_promotion(dom, node, &metrics);

        let advertisement =
            strong_ad_name(&name) && short && (links > 0 || stats.text_length < 100);
        let consent = contains_any(
            &format!("{name} {text}"),
            &["cookie consent", "cookie-banner", "consent-banner"],
        ) && short;
        let account = contains_any(&name, &["login", "sign-in", "signin"])
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
        .any(|label| text.trim() == *label || text.contains(&format!("{label} ")));
        let action_url = dom.descendants(node).any(|descendant| {
            dom.tag(descendant) == Some(Tag::A)
                && dom.attr(descendant, AttrName::Href).is_some_and(|href| {
                    let href = href.to_ascii_lowercase();
                    href.contains("/comments")
                        || href.contains("action=share")
                        || href.contains("/reply")
                        || href.contains("dialog=")
                })
        });
        let interaction_name = contains_any(
            &name,
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
                    "taxonomy" | "tags" | "entities" | "entitylist" | "taglist"
                )
            })
            || contains_any(
                &name,
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
        let peripheral_panel_name = name
            .split(|character: char| !character.is_ascii_alphanumeric())
            .any(|token| matches!(token, "sidebar" | "comments" | "commentlist"));
        let terminal_peripheral_panel = peripheral_panel_name
            && links >= 3
            && short
            && link_density >= 0.2
            && (at_end || text.starts_with("comments") && text.contains("subscribe"));
        let print_citation = links >= 2
            && short
            && contains_any(&name, &["print-citation", "story-footer"])
            && text.contains("appears in print");

        if related
            || social
            || signup
            || breadcrumb
            || navigation
            || author
            || author_promotion
            || audio_controls
            || job_profile
            || collection_promotion
            || advertisement
            || consent
            || account
            || comment_ui
            || terminal_action
            || terminal_taxonomy
            || terminal_peripheral_panel
            || print_citation
        {
            if protected
                && !author_card
                && !author_promotion
                && !job_profile
                && !collection_promotion
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

/// Removes document-level navigation and footer material that survives root
/// selection. These regions often sit inside a broad `main` wrapper, so root
/// semantics alone cannot separate them from the useful page.
///
/// A name or element tag is only one signal. Removal also requires a terminal
/// or leading position, link-heavy low-prose structure, and either semantic or
/// repeated structural evidence. This keeps pricing cards, article dates, and
/// short company content out of the global-chrome bucket.
#[cfg(test)]
#[allow(dead_code)]
pub(crate) fn remove_global_chrome(
    dom: &mut Dom,
    root: NodeId,
    store: &mut crate::dom::NodeStateStore,
    evidence: &crate::document::SourceEvidence,
) -> bool {
    let mut workspace = FragmentWorkspace::default();
    remove_global_chrome_in_workspace(dom, root, store, evidence, &mut workspace)
}

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

    store.clear_stats();
    store.enable_link_lengths();
    get_or_compute_stats(dom, root, store);

    let aggregates = {
        let snapshot = workspace.elements_with_depth();
        chrome_aggregates(dom, snapshot)
    };
    let mut signatures = HashMap::<u64, u8>::new();
    for &(node, _) in workspace.elements_with_depth() {
        let aggregate = aggregates[node.index()];
        if aggregate.link_count >= 2 {
            let count = signatures.entry(aggregate.signature).or_default();
            *count = count.saturating_add(1).min(3);
        }
    }

    let mut scratch = workspace.take_scratch();
    scratch.bits.resize(dom.len(), false);
    scratch.bits.fill(false);
    let remove = &mut scratch.bits[..dom.len()];
    let mut text_buffer = String::new();
    for &(node, _) in workspace.elements_with_depth() {
        if node == root || dom.parent(node).is_none() {
            continue;
        }
        let aggregate = aggregates[node.index()];
        let name = node_name(dom, node);
        let semantic_navigation = dom.tag(node) == Some(Tag::Nav)
            || dom.tag(node) == Some(Tag::Footer)
            || dom.tag(node) == Some(Tag::Header) && aggregate.link_count >= 3
            || dom.attr(node, AttrName::Role).is_some_and(|roles| {
                roles.split_whitespace().any(|role| {
                    role.eq_ignore_ascii_case("navigation")
                        || role.eq_ignore_ascii_case("contentinfo")
                })
            });
        let named_chrome = contains_any(
            &name,
            &[
                "navigation",
                "navbar",
                "menu",
                "footer",
                "contact",
                "legal",
                "site-links",
                "site_links",
            ],
        );
        let repeated = aggregate.link_count >= 2
            && signatures
                .get(&aggregate.signature)
                .is_some_and(|count| *count >= 2);
        if !semantic_navigation && !named_chrome && !repeated {
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
        let adjacent_content = has_substantive_content_sibling(dom, node, store);
        let text = get_normalized_inner_text(dom, node, &mut text_buffer).to_ascii_lowercase();

        let metrics = ChromeMetrics {
            name: &name,
            text: &text,
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
        if is_global_navigation(dom, node, root, &metrics)
            || is_global_footer(dom, node, root, &metrics)
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
            || dom.attr(node, AttrName::Role).is_some_and(|roles| {
                roles
                    .split_whitespace()
                    .any(|role| role.eq_ignore_ascii_case("contentinfo"))
            })
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

#[derive(Clone, Copy, Default)]
struct ChromeAggregate {
    link_count: u8,
    signature: u64,
    has_meaningful_media: bool,
    has_content_structure: bool,
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
        for child in dom.element_children(node) {
            let child_aggregate = aggregates[child.index()];
            link_count = link_count
                .saturating_add(child_aggregate.link_count)
                .min(32);
            has_meaningful_media |= child_aggregate.has_meaningful_media;
            has_content_structure |= child_aggregate.has_content_structure;
            signature = signature
                .wrapping_mul(1_099_511_628_211)
                .wrapping_add(child_aggregate.signature);
        }
        aggregates[node.index()] = ChromeAggregate {
            link_count,
            signature,
            has_meaningful_media,
            has_content_structure,
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
        && dom.attr_by_local_name(node, "alt").is_some_and(|alt| {
            let alt = alt.trim();
            alt.chars().count() >= 12
                && !contains_any(
                    &alt.to_ascii_lowercase(),
                    &["logo", "icon", "avatar", "placeholder"],
                )
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
            matches!(
                token.to_ascii_lowercase().as_str(),
                "brand" | "branding" | "logo" | "masthead" | "wordmark" | "sitetitle"
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
        && !matches!(
            text.trim().to_ascii_lowercase().as_str(),
            "home" | "menu" | "menu button" | "skip to content"
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
) -> bool {
    let Some(parent) = dom.parent(node) else {
        return false;
    };
    dom.element_children(parent).any(|sibling| {
        sibling != node
            && !matches!(
                dom.tag(sibling),
                Some(Tag::Aside | Tag::Footer | Tag::Header | Tag::Nav)
            )
            && {
                let stats = get_or_compute_stats(dom, sibling, store);
                stats.text_length >= 80
                    && (stats.sentence_end_count > 0
                        || dom.descendants(sibling).any(|descendant| {
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
    })
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
            !matches!(
                heading.trim().to_ascii_lowercase().as_str(),
                "menu" | "navigation" | "sections" | "contents" | "on this page"
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
    if is_inside_article_content(dom, node, root)
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
        || dom.attr(node, AttrName::Role).is_some_and(|roles| {
            roles
                .split_whitespace()
                .any(|role| role.eq_ignore_ascii_case("navigation"))
        });
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
    structural && compact && low_prose
}

fn is_global_footer(dom: &Dom, node: NodeId, root: NodeId, metrics: &ChromeMetrics<'_>) -> bool {
    if !metrics.at_end
        || !matches!(
            dom.tag(node),
            Some(Tag::Aside | Tag::Div | Tag::Footer | Tag::Header | Tag::Other | Tag::Section)
        )
        || matches!(dom.tag(node), Some(Tag::Article | Tag::Main))
        || is_inside_article_content(dom, node, root)
        || is_within_pricing_region(dom, node)
        || has_pricing_content(dom, node, metrics.text)
        || metrics.has_meaningful_media
    {
        return false;
    }
    let semantic = dom.tag(node) == Some(Tag::Footer)
        || dom.attr(node, AttrName::Role).is_some_and(|roles| {
            roles
                .split_whitespace()
                .any(|role| role.eq_ignore_ascii_case("contentinfo"))
        });
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
    let contact_link = dom.descendants(node).any(|descendant| {
        dom.tag(descendant) == Some(Tag::A)
            && get_normalized_inner_text(dom, descendant, &mut String::new())
                .to_ascii_lowercase()
                .contains("contact")
    });
    let global_structure = semantic || named;
    low_prose
        && ((global_structure && (link_cluster || footer_text || contact_link))
            || (footer_text && metrics.links >= 2 && metrics.link_density >= 0.15))
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
        matches!(
            label.trim().to_ascii_lowercase().as_str(),
            "on this page" | "table of contents" | "contents" | "toc"
        )
    });
    let named = contains_any(
        name,
        &["table-of-contents", "table_of_contents", "docs-toc", "toc"],
    );
    let headings = dom.descendants(node).any(|descendant| {
        matches!(
            dom.tag(descendant),
            Some(Tag::H2 | Tag::H3 | Tag::H4 | Tag::H5 | Tag::H6)
        ) && {
            let mut text = String::new();
            dom.append_normalized_text_limited(descendant, &mut text, 128);
            matches!(
                text.trim().to_ascii_lowercase().as_str(),
                "contents" | "on this page"
            )
        }
    });
    let links: Vec<_> = dom
        .descendants(node)
        .filter(|&descendant| dom.tag(descendant) == Some(Tag::A))
        .collect();
    let all_fragment_links = !links.is_empty()
        && links.iter().all(|&link| {
            dom.attr(link, AttrName::Href)
                .is_some_and(|href| href.trim_start().starts_with('#'))
        });
    labelled || named || headings || all_fragment_links
}

fn has_pricing_heading(dom: &Dom, node: NodeId) -> bool {
    dom.descendants(node).any(|descendant| {
        matches!(
            dom.tag(descendant),
            Some(Tag::H1 | Tag::H2 | Tag::H3 | Tag::H4 | Tag::H5 | Tag::H6)
        ) && {
            let mut heading = String::new();
            dom.append_normalized_text_limited(descendant, &mut heading, 256);
            let heading = heading.trim().to_ascii_lowercase();
            heading == "pricing"
                || heading == "plans"
                || heading == "pricing plans"
                || heading.ends_with(" plans")
                || heading.contains("pricing")
        }
    })
}

fn has_pricing_content(dom: &Dom, node: NodeId, text: &str) -> bool {
    let pricing_heading = has_pricing_heading(dom, node);
    if pricing_heading {
        return true;
    }
    let price_words = ["pricing", "price", "plan", "monthly", "annual", "per month"]
        .into_iter()
        .filter(|word| text.contains(word))
        .count();
    let has_currency = text
        .chars()
        .any(|character| matches!(character, '$' | '€' | '£' | '¥'))
        && text.chars().any(|character| character.is_ascii_digit());
    let price_period = ["/month", "/year", "/mo", "/yr", "per month", "per year"]
        .into_iter()
        .any(|period| text.contains(period));
    has_currency && (price_words >= 2 || price_words >= 1 && price_period)
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
                        let heading = heading.trim().to_ascii_lowercase();
                        heading == "pricing"
                            || heading == "plans"
                            || heading == "pricing plans"
                            || heading.ends_with(" plans")
                            || heading.contains("pricing")
                    }
                })
        })
}

fn is_inside_article_content(dom: &Dom, node: NodeId, _root: NodeId) -> bool {
    std::iter::once(node)
        .chain(dom.ancestors(node))
        .any(|ancestor| {
            dom.tag(ancestor) == Some(Tag::Article)
                || dom.attr(ancestor, AttrName::ItemProp).is_some_and(|value| {
                    value.split_whitespace().any(|item| {
                        item.eq_ignore_ascii_case("articleBody")
                            || item.eq_ignore_ascii_case("text")
                    })
                })
                || dom.attr(ancestor, AttrName::Role).is_some_and(|roles| {
                    roles
                        .split_whitespace()
                        .any(|role| role.eq_ignore_ascii_case("article"))
                })
        })
}

fn hoist_footer_identity(dom: &mut Dom, footer: NodeId) {
    let children: Vec<_> = dom.children(footer).collect();
    for child in children {
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
        .filter(|&node| dom.tag(node) == Some(Tag::A))
        .collect();
    let mut text_buffer = String::new();
    if let Some(link) = links.into_iter().find(|&link| {
        let text = get_normalized_inner_text(dom, link, &mut text_buffer);
        is_footer_identity_text(text)
    }) {
        dom.insert_before(footer, link);
    }
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
    let lower = text.to_ascii_lowercase();
    let ui_label = matches!(
        lower.as_str(),
        "home"
            | "privacy"
            | "privacy policy"
            | "terms"
            | "terms of service"
            | "contact"
            | "contact us"
            | "copyright"
            | "cookie policy"
            | "sitemap"
            | "imprint"
            | "presskit"
            | "faq"
            | "rss"
            | "jobs"
            | "subscribe"
            | "newsletter"
            | "follow us"
            | "learn more"
            | "read more"
            | "view details"
            | "more"
    );
    let boilerplate = contains_any(
        &lower,
        &[
            "all rights reserved",
            "privacy policy",
            "terms of service",
            "cookie policy",
        ],
    );
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
) -> bool {
    let terminal_related = snapshot
        .iter()
        .enumerate()
        .rev()
        .find_map(|(index, &(node, _))| {
            let name = node_name(dom, node);
            (name.contains("related")
                && contains_any(&name, &["articles", "cards", "grid", "stories"])
                && related_heading_signal_in(dom, node) == RelatedHeadingSignal::Strong
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
        let article_toc = dom.attr_by_local_name(node, "data-article-toc").is_some();
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
        let text = text.trim().to_ascii_lowercase();
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
                && dom.attr_by_local_name(descendant, "action").is_some()
                && node_name(dom, descendant).contains("newsletter-form")
        });

        let author_promotion = article_toc
            && stats.text_length < 1_200
            && action_link
            && image
            && text.contains("the latest from ")
            && text.contains("monthly")
            && contains_any(&text, &["news", "updates", "newsletter"]);
        let collection = stats.text_length < 800
            && name.contains("related-content-tout")
            && action_link
            && (text.starts_with("collection ") || text == "collection")
            && terminal_related.is_some_and(|(related_index, related_node)| {
                related_index > index
                    && starts_terminal_peripheral_sequence(dom, node, related_node, root, store)
            });
        let related_cards = terminal_related.is_some_and(|(_, related_node)| related_node == node);
        let audio_controls = audio_player
            && stats.text_length < 500
            && (audio || action_link)
            && contains_any(
                &text,
                &[
                    "listen to article",
                    "listen to this article",
                    "[[duration]]",
                ],
            );
        let meta_artifact =
            article_meta && stats.text_length < 100 && text.contains('|') && looks_like_date(&text);
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
    changed
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
            founder_counts[node.index()] = u8::from(text.trim().eq_ignore_ascii_case("founder"));
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
    for &(node, _) in snapshot {
        if (related_heading_signal(dom, node) == RelatedHeadingSignal::Strong
            || dom.tag(node) == Some(Tag::Form))
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
        if has_newsletter_evidence(&name, &text.to_ascii_lowercase()) {
            remove[start..=index].fill(true);
        }
    }

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
    if !matches!(
        dom.tag(node),
        Some(Tag::H2 | Tag::H3 | Tag::H4 | Tag::H5 | Tag::H6)
    ) {
        return RelatedHeadingSignal::None;
    }
    let mut text = String::new();
    dom.append_normalized_text_limited(node, &mut text, 128);
    let text = text.trim().to_ascii_lowercase();
    if matches!(
        text.as_str(),
        "related articles"
            | "related content"
            | "related posts"
            | "related stories"
            | "recommended"
            | "recommended reading"
            | "read next"
            | "next steps"
            | "more stories"
            | "more articles"
            | "more posts"
            | "you may also like"
            | "you might also like"
            | "collection"
    ) || text
        .strip_prefix("more from ")
        .is_some_and(|suffix| !suffix.is_empty() && suffix.split_whitespace().count() <= 6)
    {
        RelatedHeadingSignal::Strong
    } else if matches!(
        text.as_str(),
        "related" | "further reading" | "see also" | "read more"
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
        let text = text.to_ascii_lowercase();
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
                has_newsletter_cta_text(&child_text.to_ascii_lowercase())
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
        || dom.attr(node, AttrName::Role).is_some_and(|roles| {
            roles
                .split_whitespace()
                .any(|role| role.eq_ignore_ascii_case("navigation"))
        });
    if dom.attr(node, AttrName::AriaLabel).is_some_and(|label| {
        matches!(
            label.trim().to_ascii_lowercase().as_str(),
            "breadcrumb" | "breadcrumbs"
        )
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
        let text = text.trim().to_ascii_lowercase();
        text == "collection"
            || text == "company profile"
            || text == "founders"
            || text.starts_with("the latest from ")
    });

    structural_name || job_name || promotional_heading
}

fn has_breadcrumb_name(dom: &Dom, node: NodeId, name: &str) -> bool {
    name.split(|character: char| !character.is_ascii_alphanumeric())
        .any(|token| matches!(token, "breadcrumb" | "breadcrumbs"))
        || dom.attr(node, AttrName::AriaLabel).is_some_and(|label| {
            matches!(
                label.trim().to_ascii_lowercase().as_str(),
                "breadcrumb" | "breadcrumbs"
            )
        })
}

fn breadcrumb_separator_count(text: &str) -> usize {
    text.chars()
        .filter(|&character| matches!(character, '>' | '/' | '›' | '»'))
        .take(3)
        .count()
}

fn is_breadcrumb(dom: &Dom, node: NodeId, metrics: &PeripheralMetrics<'_>) -> bool {
    if !metrics.at_start
        || metrics.links < 2
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

    let explicit = has_breadcrumb_name(dom, node, metrics.name);
    let navigation = dom.tag(node) == Some(Tag::Nav)
        || dom.attr(node, AttrName::Role).is_some_and(|roles| {
            roles
                .split_whitespace()
                .any(|role| role.eq_ignore_ascii_case("navigation"))
        });
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
        && metrics.link_density >= if explicit { 0.25 } else { 0.4 }
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
    matches!(
        text.to_ascii_lowercase().as_str(),
        "founded" | "batch" | "team size" | "status"
    )
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

    // Populate every current subtree statistic in one bottom-up traversal.
    // The earlier structural pass can detach nodes, so do not reuse its cache.
    store.clear_stats();
    get_or_compute_stats(dom, root, store);

    for &node in nodes.iter().rev() {
        if dom.parent(node).is_none() || is_protected_content(dom, node, evidence) {
            continue;
        }
        if store
            .get_stats(node)
            .is_none_or(|stats| stats.text_length > 140)
        {
            continue;
        }
        let text = get_inner_text(dom, node, text_buffer);
        let text = text.trim().to_ascii_lowercase();
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

        let reading_time = is_reading_time_label(&text)
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
            text.as_str(),
            "advertisement" | "advertisement continues below" | "sponsored" | "sponsored content"
        ) && (at_start || at_end || strong_ad_name(&name));
        let action = matches!(
            text.as_str(),
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

        if reading_time || advertisement || action || subscription {
            detach_and_invalidate_stats(dom, node, store);
        }
    }
    workspace.invalidate();
}

fn is_contextual_text_boundary(dom: &Dom, node: NodeId) -> bool {
    matches!(
        dom.tag(node),
        Some(Tag::Aside | Tag::Div | Tag::Footer | Tag::P | Tag::Section | Tag::Small)
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
    matches!(
        dom.tag(node),
        Some(Tag::Div | Tag::Ol | Tag::P | Tag::Section | Tag::Ul)
    ) && contains_any(
        &node_name(dom, node),
        &[
            "related",
            "recommend",
            "share",
            "social",
            "newsletter",
            "subscribe",
            "signup",
            "menu",
            "navigation",
            "breadcrumb",
            "author",
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
            "company-portals",
            "entity-list",
            "entity_list",
            "tag-list",
            "tag_list",
        ],
    )
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

fn node_name(dom: &Dom, node: NodeId) -> String {
    let mut value = String::new();
    if dom.tag(node) == Some(Tag::Other)
        && let Some(name) = dom.qual_name(node)
    {
        value.push_str(name.local.as_ref());
    }
    for name in [AttrName::Class, AttrName::Id] {
        if let Some(part) = dom.attr(node, name) {
            if !value.is_empty() {
                value.push(' ');
            }
            value.push_str(part);
        }
    }
    value.make_ascii_lowercase();
    value
}

fn contains_any(value: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| value.contains(needle))
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
            r#"<ul><li><label><input class="control" onclick="bad()" type="checkbox" checked> Done</label></li><li><form><input type="checkbox"> Option</form></li></ul><form><input type="checkbox"><button>Search</button></form>"#,
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
        assert_eq!(dom.attr_by_local_name(inputs[0], "onclick"), None);
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
            r#"<article><p class="reading-time">5 min read</p><p>This substantive article paragraph explains the complete result and gives useful context to readers.</p><p><a href="/more">Read more</a></p><p>Advertisement</p></article>"#,
        );
        assert!(text.contains("substantive article"), "{text}");
        for clutter in ["5 min read", "Read more", "Advertisement"] {
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
