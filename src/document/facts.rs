//! Shared source facts for complex semantic compilation.

use crate::dom::{AttrName, Dom, NodeId, NodeStateStore, Tag};
use std::collections::HashSet;

use super::compiler::{heading_level, is_block_tag};

const HAS_VISIBLE_TEXT: u16 = 1 << 0;
const HAS_BLOCK_DESCENDANT: u16 = 1 << 1;
const HAS_MEANINGFUL_CONTENT: u16 = 1 << 2;
const HAS_MULTILINE_CONTENT: u16 = 1 << 3;
const IS_CODE_BLOCK: u16 = 1 << 4;
const IS_GLYPH_ONLY: u16 = 1 << 5;
const IS_HEADING_PERMALINK: u16 = 1 << 6;
const PERMALINK_SEPARATES_WORDS: u16 = 1 << 7;
const HEADING_HAS_MEANINGFUL_CONTENT: u16 = 1 << 8;
const HEADING_HAS_PERMALINK: u16 = 1 << 9;
const TRIM_HEADING_START: u16 = 1 << 10;
const TRIM_HEADING_END: u16 = 1 << 11;

#[derive(Clone, Copy, Default)]
struct NodeFacts {
    flags: u16,
    heading_level: u8,
    first_visible: Option<char>,
    last_visible: Option<char>,
}

impl NodeFacts {
    fn has(self, flag: u16) -> bool {
        self.flags & flag != 0
    }

    fn set(&mut self, flag: u16, value: bool) {
        if value {
            self.flags |= flag;
        } else {
            self.flags &= !flag;
        }
    }
}

/// Sparse worklists built during the shared preorder scan.
#[derive(Default)]
pub(super) struct FeatureInventory {
    pub(super) owned_code_sources: Vec<(NodeId, NodeId)>,
    pub(super) headings: Vec<NodeId>,
    pub(super) fragment_links: Vec<NodeId>,
    pub(super) images: Vec<NodeId>,
    pub(super) figures: Vec<NodeId>,
    pub(super) media: Vec<NodeId>,
    pub(super) lists: Vec<NodeId>,
    pub(super) tables: Vec<NodeId>,
    pub(super) callouts: Vec<NodeId>,
    pub(super) math: Vec<NodeId>,
    pub(super) footnotes: Vec<NodeId>,
}

/// Per-node feature bits retained in the sparse source worklist.
#[derive(Clone, Copy, Default)]
struct NodeGate(u8);

impl NodeGate {
    fn add(&mut self, bit: u8) {
        self.0 |= bit;
    }

    fn contains(self, bit: u8) -> bool {
        self.0 & bit != 0
    }

    fn is_empty(self) -> bool {
        self.0 == 0
    }
}

/// Small source-gating bits and sparse candidates collected during an existing
/// cleanup traversal.
#[derive(Default)]
pub(crate) struct SemanticGate {
    bits: u8,
    candidates: Vec<(NodeId, NodeGate)>,
    fragment_links: Vec<NodeId>,
    table_nodes: Vec<NodeId>,
}

impl SemanticGate {
    const CALLOUT: u8 = 1 << 0;
    const FOOTNOTE: u8 = 1 << 1;
    const MATH: u8 = 1 << 2;
    const DATA_TABLE: u8 = 1 << 3;

    fn add(&mut self, bit: u8) {
        self.bits |= bit;
    }

    pub(crate) fn add_data_table(&mut self) {
        self.add(Self::DATA_TABLE);
    }

    pub(crate) fn add_data_table_node(&mut self, node: NodeId) {
        self.add_data_table();
        self.table_nodes.push(node);
    }

    fn is_empty(&self) -> bool {
        self.bits == 0
    }

    /// Records broad evidence and sparse candidates. Feature-specific
    /// recognizers run later, and ordinary source does not allocate these
    /// vectors because they remain empty.
    pub(crate) fn observe(&mut self, dom: &Dom, node: NodeId) {
        let mut node_gate = NodeGate::default();
        if may_have_callout_evidence(dom, node) {
            self.add(Self::CALLOUT);
            node_gate.add(Self::CALLOUT);
        }
        if has_footnote_attributes(dom, node) {
            self.add(Self::FOOTNOTE);
            node_gate.add(Self::FOOTNOTE);
        }
        if fragment_target(dom, node).is_some() {
            self.add(Self::FOOTNOTE);
            self.fragment_links.push(node);
        }
        if may_have_math_evidence(dom, node) {
            self.add(Self::MATH);
            node_gate.add(Self::MATH);
        }
        if !node_gate.is_empty() {
            self.candidates.push((node, node_gate));
        }
    }
}

/// Sparse source evidence shared by cleanup and semantic compilation.
///
/// The cheap gate runs before a retained-fragment snapshot. Feature-specific
/// recognizers run only for gated nodes, and their results use sparse node
/// sets. Cleanup only detaches nodes, so these source-relative facts remain
/// valid while the fragment is reduced and compiled.
#[derive(Default)]
pub(crate) struct SourceEvidence {
    callout: HashSet<NodeId>,
    callout_candidate: HashSet<NodeId>,
    footnote: HashSet<NodeId>,
    footnote_candidate: HashSet<NodeId>,
    math: HashSet<NodeId>,
    accessible_math: HashSet<NodeId>,
    contains_semantic: HashSet<NodeId>,
    data_table: HashSet<NodeId>,
}

impl SourceEvidence {
    /// Analyzes source evidence in one pass when no earlier cleanup traversal
    /// supplied a gate.
    pub(crate) fn analyze(dom: &Dom, root: NodeId, store: &NodeStateStore) -> Self {
        Self::analyze_impl(dom, root, store, None)
    }

    /// Completes targeted source analysis after a cleanup pass has collected
    /// the broad gate. This scan is needed only when the gate found a feature
    /// that needs semantic recognition.
    pub(crate) fn analyze_with_gate(
        dom: &Dom,
        root: NodeId,
        store: &NodeStateStore,
        gate: SemanticGate,
    ) -> Self {
        Self::analyze_impl(dom, root, store, Some(gate))
    }

    fn analyze_impl(
        dom: &Dom,
        root: NodeId,
        store: &NodeStateStore,
        precollected_gate: Option<SemanticGate>,
    ) -> Self {
        if precollected_gate
            .as_ref()
            .is_some_and(SemanticGate::is_empty)
        {
            return Self::default();
        }
        let gate_was_precollected = precollected_gate.is_some();
        let mut gate = precollected_gate.unwrap_or_default();
        if !gate_was_precollected {
            for node in std::iter::once(root).chain(dom.descendants(root)) {
                gate.observe(dom, node);
                if dom.tag(node) == Some(Tag::Table) && store.is_data_table(node) == Some(true) {
                    gate.add_data_table_node(node);
                }
            }
        }

        let mut gated = std::mem::take(&mut gate.candidates);
        let table_nodes = std::mem::take(&mut gate.table_nodes);
        let fragment_node_ids = std::mem::take(&mut gate.fragment_links);
        let mut fragment_targets = None;
        let mut fragment_links = Vec::with_capacity(fragment_node_ids.len());
        for node in fragment_node_ids {
            if let Some(target) = fragment_target(dom, node) {
                fragment_targets
                    .get_or_insert_with(HashSet::new)
                    .insert(target);
                fragment_links.push((node, target));
            }
        }

        // Resolve only the small set of fragment targets. Do not build an
        // index for every source ID, and do not allocate anything when the
        // source has no fragment references. The cleanup pass already
        // recorded all feature candidates, so this pass only handles targets.
        if let Some(targets) = fragment_targets {
            let mut resolved_targets = HashSet::new();
            let mut footnote_nodes = HashSet::new();
            for node in std::iter::once(root).chain(dom.descendants(root)) {
                if let Some(id) = dom.attr(node, AttrName::Id)
                    && targets.contains(id)
                {
                    resolved_targets.insert(id);
                    footnote_nodes.insert(node);
                }
            }
            for (node, target) in fragment_links {
                if resolved_targets.contains(target) {
                    footnote_nodes.insert(node);
                }
            }
            if !footnote_nodes.is_empty() {
                gate.add(SemanticGate::FOOTNOTE);
                for (node, node_gate) in &mut gated {
                    if footnote_nodes.contains(node) {
                        node_gate.add(SemanticGate::FOOTNOTE);
                    }
                }
                let mut gated_nodes: HashSet<NodeId> =
                    gated.iter().map(|&(node, _)| node).collect();
                for node in footnote_nodes {
                    if gated_nodes.insert(node) {
                        let mut node_gate = NodeGate::default();
                        node_gate.add(SemanticGate::FOOTNOTE);
                        gated.push((node, node_gate));
                    }
                }
            }
        }

        if gate.is_empty() {
            return Self::default();
        }
        // A fragment link can set the broad footnote bit before its target is
        // resolved. Avoid allocating semantic state when the gated scan found
        // neither a target nor a feature candidate.
        if gated.is_empty() && table_nodes.is_empty() {
            return Self::default();
        }

        let mut nodes = Vec::with_capacity(dom.len());
        nodes.push((root, 0));
        nodes.extend(dom.element_descendants_snapshot_with_depth(root));

        let has_math_candidate = gated
            .iter()
            .any(|&(_, node_gate)| node_gate.contains(SemanticGate::MATH));
        let has_callout_candidate = gated
            .iter()
            .any(|&(_, node_gate)| node_gate.contains(SemanticGate::CALLOUT));
        let has_footnote_candidate = gated
            .iter()
            .any(|&(_, node_gate)| node_gate.contains(SemanticGate::FOOTNOTE));
        let callout_capacity = gated
            .iter()
            .filter(|(_, node_gate)| node_gate.contains(SemanticGate::CALLOUT))
            .count();
        let footnote_capacity = gated
            .iter()
            .filter(|(_, node_gate)| node_gate.contains(SemanticGate::FOOTNOTE))
            .count();
        let math_capacity = gated
            .iter()
            .filter(|(_, node_gate)| node_gate.contains(SemanticGate::MATH))
            .count();
        let mut callout = if has_callout_candidate {
            HashSet::with_capacity(callout_capacity)
        } else {
            HashSet::new()
        };
        let mut callout_candidate = if has_callout_candidate {
            HashSet::with_capacity(callout_capacity)
        } else {
            HashSet::new()
        };
        let mut footnote = if has_footnote_candidate {
            HashSet::with_capacity(footnote_capacity)
        } else {
            HashSet::new()
        };
        let mut footnote_candidate = if has_footnote_candidate {
            HashSet::with_capacity(footnote_capacity)
        } else {
            HashSet::new()
        };
        let mut math = if has_math_candidate {
            HashSet::with_capacity(math_capacity)
        } else {
            HashSet::new()
        };
        let mut data_table = HashSet::with_capacity(table_nodes.len());

        for (node, node_gate) in gated {
            if node_gate.contains(SemanticGate::CALLOUT) {
                let (source, candidate) = super::callouts::source_evidence(dom, node);
                let explicit = dom.attr(node, AttrName::DataCallout).is_some()
                    || dom.attr_by_local_name(node, "data-callout").is_some();
                if source || explicit {
                    callout.insert(node);
                }
                if candidate || explicit {
                    callout_candidate.insert(node);
                }
            }
            if node_gate.contains(SemanticGate::FOOTNOTE) {
                let explicit = dom.attr(node, AttrName::DataFootnote).is_some()
                    || dom.attr(node, AttrName::DataFootnoteRef).is_some()
                    || dom.attr(node, AttrName::DataFootnotes).is_some()
                    || dom.attr_by_local_name(node, "data-footnote").is_some()
                    || dom.attr_by_local_name(node, "data-footnote-ref").is_some()
                    || dom.attr_by_local_name(node, "data-footnotes").is_some();
                if super::footnotes::is_source_evidence(dom, node) || explicit {
                    footnote.insert(node);
                }
                if super::footnotes::has_possible_footnote_evidence(dom, node) || explicit {
                    footnote_candidate.insert(node);
                }
            }
            if node_gate.contains(SemanticGate::MATH) && super::math::is_source_evidence(dom, node)
            {
                math.insert(node);
            }
        }
        for node in table_nodes {
            if store.is_data_table(node) == Some(true) {
                data_table.insert(node);
            }
        }

        let accessible_math = if has_math_candidate {
            super::math::accessible_math_nodes(dom, &nodes)
        } else {
            HashSet::new()
        };
        let mut contains_semantic = HashSet::new();
        for &(node, _) in nodes.iter().rev() {
            let value = callout.contains(&node)
                || footnote.contains(&node)
                || math.contains(&node)
                || accessible_math.contains(&node)
                || data_table.contains(&node)
                || dom
                    .element_children(node)
                    .any(|child| contains_semantic.contains(&child));
            if value {
                contains_semantic.insert(node);
            }
        }
        Self {
            callout,
            callout_candidate,
            footnote,
            footnote_candidate,
            math,
            accessible_math,
            contains_semantic,
            data_table,
        }
    }

    pub(crate) fn callout(&self, node: NodeId) -> bool {
        self.callout.contains(&node)
    }

    pub(crate) fn callout_candidate(&self, node: NodeId) -> bool {
        self.callout_candidate.contains(&node)
    }

    pub(crate) fn footnote(&self, node: NodeId) -> bool {
        self.footnote.contains(&node)
    }

    pub(crate) fn footnote_candidate(&self, node: NodeId) -> bool {
        self.footnote_candidate.contains(&node)
    }

    pub(crate) fn math(&self, node: NodeId) -> bool {
        self.math.contains(&node)
    }

    pub(crate) fn accessible_math(&self, node: NodeId) -> bool {
        self.accessible_math.contains(&node)
    }

    pub(crate) fn contains_semantic(&self, node: NodeId) -> bool {
        self.contains_semantic.contains(&node)
    }

    pub(crate) fn data_table(&self, node: NodeId) -> bool {
        self.data_table.contains(&node)
    }

    pub(crate) fn is_semantic_source(&self, node: NodeId) -> bool {
        self.callout(node) || self.footnote(node) || self.math(node) || self.accessible_math(node)
    }
}

fn may_have_callout_evidence(dom: &Dom, node: NodeId) -> bool {
    dom.attr(node, AttrName::DataCallout).is_some()
        || dom.attr_by_local_name(node, "data-callout").is_some()
        || matches!(dom.tag(node), Some(Tag::Aside | Tag::Div | Tag::Section))
            && (dom
                .attr(node, AttrName::Role)
                .is_some_and(likely_callout_name)
                || [AttrName::Class, AttrName::Id]
                    .into_iter()
                    .filter_map(|attribute| dom.attr(node, attribute))
                    .any(likely_callout_name))
}

fn has_footnote_attributes(dom: &Dom, node: NodeId) -> bool {
    dom.attr(node, AttrName::DataFootnote).is_some()
        || dom.attr(node, AttrName::DataFootnoteRef).is_some()
        || dom.attr(node, AttrName::DataFootnotes).is_some()
        || dom.attr_by_local_name(node, "data-footnote").is_some()
        || dom.attr_by_local_name(node, "data-footnotes").is_some()
        || dom.attr_by_local_name(node, "data-footnote-ref").is_some()
        || dom
            .attr_by_local_name(node, "data-type")
            .is_some_and(likely_footnote_name)
        || [AttrName::Class, AttrName::Id, AttrName::Role, AttrName::Rel]
            .into_iter()
            .filter_map(|attribute| dom.attr(node, attribute))
            .any(likely_footnote_name)
}

fn fragment_target(dom: &Dom, node: NodeId) -> Option<&str> {
    let href = dom
        .tag(node)
        .is_some_and(|tag| tag == Tag::A)
        .then(|| dom.attr(node, AttrName::Href))
        .flatten()?
        .trim();
    let (prefix, target) = href.rsplit_once('#')?;
    let has_scheme = prefix.find(':').is_some_and(|colon| {
        colon > 0
            && prefix[..colon]
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'-' | b'.'))
    });
    if target.is_empty()
        || (!prefix.is_empty() && (has_scheme || prefix.starts_with('/') || prefix.contains('?')))
    {
        return None;
    }
    Some(target)
}

fn may_have_math_evidence(dom: &Dom, node: NodeId) -> bool {
    dom.tag(node) == Some(Tag::Math)
        || dom.tag(node) == Some(Tag::Script)
            && dom.attr(node, AttrName::Type).is_some_and(|value| {
                let value = value.trim().to_ascii_lowercase();
                value == "text/tex" || value == "math/tex" || value.starts_with("math/tex;")
            })
        || dom.tag(node) == Some(Tag::Img)
            && ([AttrName::Class, AttrName::Id]
                .into_iter()
                .filter_map(|attribute| dom.attr(node, attribute))
                .any(likely_math_name)
                || dom
                    .attr(node, AttrName::Src)
                    .is_some_and(likely_math_source))
        || dom.qual_name(node).is_some_and(|name| {
            let local = name.local.as_ref();
            local.eq_ignore_ascii_case("annotation") || local.eq_ignore_ascii_case("mjx-container")
        })
        || dom.attr_by_local_name(node, "data-latex").is_some()
        || dom.attr_by_local_name(node, "data-tex").is_some()
        || dom.attr(node, AttrName::DataMath).is_some()
        || dom.attr_by_local_name(node, "data-math").is_some()
        || dom.attr_by_local_name(node, "data-formula").is_some()
        || [AttrName::Class, AttrName::Id]
            .into_iter()
            .filter_map(|attribute| dom.attr(node, attribute))
            .any(likely_math_name)
}

fn has_name_token(value: &str, names: &[&str]) -> bool {
    value
        .split(|character: char| !character.is_ascii_alphanumeric())
        .any(|token| {
            !token.is_empty()
                && names.iter().any(|name| {
                    token.eq_ignore_ascii_case(name)
                        || token
                            .get(..name.len())
                            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(name))
                })
        })
}

fn likely_callout_name(value: &str) -> bool {
    has_name_token(
        value,
        &[
            "admonition",
            "callout",
            "alert",
            "note",
            "warning",
            "caution",
            "important",
            "danger",
            "tip",
            "info",
        ],
    )
}

fn likely_footnote_name(value: &str) -> bool {
    has_name_token(
        value,
        &[
            "foot",
            "fn",
            "sn",
            "note",
            "noteref",
            "backref",
            "endnote",
            "sidenote",
            "marginnote",
            "reference",
            "refnote",
            "cite",
            "ftn",
            "ftnt",
        ],
    )
}

fn likely_math_name(value: &str) -> bool {
    has_name_token(
        value,
        &[
            "math", "katex", "mathjax", "tex2jax", "equation", "formula", "latex",
        ],
    )
}

fn likely_math_source(value: &str) -> bool {
    let value = value.to_ascii_lowercase();
    value.contains("equation") || value.contains("formula")
}

/// Source facts that cleanup and semantic compilation can share.
///
/// Cleanup only detaches nodes. When it does so, this type updates the derived
/// facts for the affected ancestor chain so the compiler can use the same
/// analysis after cleanup without trusting stale subtree state.
pub(crate) struct SemanticSourceFacts {
    root: NodeId,
    source_root_live: bool,
    preorder: Vec<NodeId>,
    positions: Vec<usize>,
    subtree_ends: Vec<usize>,
    removed_ranges: Vec<(usize, usize)>,
    multiline_content: Vec<bool>,
    code_blocks: Vec<bool>,
    dirty: Vec<bool>,
    has_dirty: bool,
}

impl SemanticSourceFacts {
    /// Builds shared facts from the cleanup pass's multiline arrays.
    pub(crate) fn from_precomputed(
        dom: &Dom,
        root: NodeId,
        nodes: &[NodeId],
        multiline_content: Vec<bool>,
        code_blocks: Vec<bool>,
    ) -> Self {
        let mut preorder = Vec::with_capacity(nodes.len() + 1);
        preorder.push(root);
        preorder.extend(nodes.iter().copied());
        let mut code_blocks = code_blocks;
        for &node in &preorder {
            if dom.tag(node) == Some(Tag::Pre) {
                code_blocks[node.index()] = true;
            }
        }
        Self::from_precomputed_preorder(dom, root, preorder, multiline_content, code_blocks)
    }

    pub(crate) fn analyze(dom: &Dom, root: NodeId) -> Self {
        let mut preorder = Vec::with_capacity(dom.len());
        preorder.push(root);
        preorder.extend(dom.descendants(root));
        Self::analyze_preorder(dom, root, preorder)
    }

    fn analyze_preorder(dom: &Dom, root: NodeId, preorder: Vec<NodeId>) -> Self {
        let mut multiline_content = vec![false; dom.len()];
        let mut code_blocks = vec![false; dom.len()];
        for &node in preorder.iter().rev() {
            let multiline = dom.tag(node) == Some(Tag::Br)
                || dom.text_node(node).is_some_and(|text| text.contains('\n'))
                || dom
                    .children(node)
                    .any(|child| multiline_content[child.index()]);
            multiline_content[node.index()] = multiline;
            code_blocks[node.index()] = dom.tag(node) == Some(Tag::Pre)
                || super::code::is_multiline_orphan_with_evidence(dom, node, multiline);
        }
        Self::from_precomputed_preorder(dom, root, preorder, multiline_content, code_blocks)
    }

    fn from_precomputed_preorder(
        dom: &Dom,
        root: NodeId,
        preorder: Vec<NodeId>,
        multiline_content: Vec<bool>,
        code_blocks: Vec<bool>,
    ) -> Self {
        let mut positions = vec![usize::MAX; dom.len()];
        for (position, &node) in preorder.iter().enumerate() {
            positions[node.index()] = position;
        }
        let mut subtree_ends: Vec<_> = (0..preorder.len()).map(|position| position + 1).collect();
        for position in (1..preorder.len()).rev() {
            let node = preorder[position];
            if let Some(parent) = dom.parent(node) {
                let parent_position = positions[parent.index()];
                if parent_position < position {
                    subtree_ends[parent_position] =
                        subtree_ends[parent_position].max(subtree_ends[position]);
                }
            }
        }

        Self {
            root,
            source_root_live: true,
            preorder,
            positions,
            subtree_ends,
            removed_ranges: Vec::new(),
            multiline_content,
            code_blocks,
            dirty: vec![false; dom.len()],
            has_dirty: false,
        }
    }

    pub(crate) fn nodes(&self) -> &[NodeId] {
        &self.preorder[1..]
    }

    /// Returns the original preorder snapshot without detached subtrees.
    pub(crate) fn live_nodes(&self, root: NodeId) -> Vec<NodeId> {
        // Cleanup visits the preorder snapshot in reverse, so reverse the
        // recorded ranges without sorting them. This keeps retry finalization
        // linear even when many subtrees are detached.
        let mut nodes = Vec::with_capacity(self.preorder.len());
        let include_source_root = self.source_root_live && self.preorder[0] != root;
        let mut next = usize::from(!include_source_root);
        let mut ranges = self.removed_ranges.iter().rev().copied();
        let mut range = ranges.next();
        while next < self.preorder.len() {
            if let Some((start, end)) = range {
                if next < start {
                    nodes.extend_from_slice(&self.preorder[next..start]);
                    next = start;
                }
                next = next.max(end);
                range = ranges.next();
            } else {
                nodes.extend_from_slice(&self.preorder[next..]);
                break;
            }
        }
        nodes
    }

    pub(crate) fn multiline_content(&self, node: NodeId) -> bool {
        self.multiline_content[node.index()]
    }

    pub(crate) fn is_code_block(&self, node: NodeId) -> bool {
        self.code_blocks[node.index()]
    }

    /// Records a cleanup detach for a later linear fact refresh.
    pub(crate) fn node_detached(&mut self, node: NodeId, parent: Option<NodeId>) {
        self.multiline_content[node.index()] = false;
        self.code_blocks[node.index()] = false;
        self.dirty[node.index()] = false;
        let position = self.positions[node.index()];
        if position != usize::MAX {
            if position == 0 {
                self.source_root_live = false;
            }
            self.removed_ranges
                .push((position, self.subtree_ends[position]));
        }
        if let Some(parent) = parent {
            self.dirty[parent.index()] = true;
            self.has_dirty = true;
        }
    }

    /// Repairs derived facts after cleanup has detached all selected nodes.
    pub(crate) fn refresh_after_cleanup(&mut self, dom: &Dom) {
        if !self.has_dirty {
            return;
        }
        for index in (1..self.preorder.len()).rev() {
            let node = self.preorder[index];
            self.refresh_node(dom, node);
        }
        self.refresh_node(dom, self.root);
        self.has_dirty = false;
    }

    /// Moves the source-fact boundary to a new retained fragment root.
    pub(crate) fn rebase_root(&mut self, dom: &Dom, root: NodeId) {
        self.root = root;
        self.source_root_live = dom.parent(self.preorder[0]).is_some();
        self.multiline_content[root.index()] = dom.tag(root) == Some(Tag::Br)
            || dom.text_node(root).is_some_and(|text| text.contains('\n'))
            || dom
                .children(root)
                .any(|child| self.multiline_content[child.index()]);
        self.code_blocks[root.index()] = dom.tag(root) == Some(Tag::Pre)
            || super::code::is_multiline_orphan_with_evidence(
                dom,
                root,
                self.multiline_content[root.index()],
            );
    }

    fn refresh_node(&mut self, dom: &Dom, node: NodeId) {
        if !self.dirty[node.index()] {
            return;
        }
        self.dirty[node.index()] = false;
        if node != self.root && dom.parent(node).is_none() {
            return;
        }
        let multiline = dom.tag(node) == Some(Tag::Br)
            || dom.text_node(node).is_some_and(|text| text.contains('\n'))
            || dom
                .children(node)
                .any(|child| self.multiline_content[child.index()]);
        self.multiline_content[node.index()] = multiline;
        self.code_blocks[node.index()] = dom.tag(node) == Some(Tag::Pre)
            || super::code::is_multiline_orphan_with_evidence(dom, node, multiline);
        for child in dom.children(node) {
            if dom.tag(child) == Some(Tag::Code) {
                let child_multiline = self.multiline_content[child.index()];
                self.code_blocks[child.index()] =
                    super::code::is_multiline_orphan_with_evidence(dom, child, child_multiline);
            }
        }
        if node != self.root
            && let Some(parent) = dom.parent(node)
        {
            self.dirty[parent.index()] = true;
        }
    }
}

/// Dense facts that serve more than one complex semantic concern.
pub(super) struct SemanticFacts {
    nodes: Vec<NodeId>,
    facts: Vec<NodeFacts>,
    inventory: FeatureInventory,
}

impl SemanticFacts {
    /// Scans source evidence in preorder and propagates generic facts in reverse order.
    pub(super) fn analyze(dom: &Dom, root: NodeId) -> Self {
        Self::analyze_with_source_facts(dom, root, None, None)
    }

    /// Scans source evidence while reusing facts computed before final cleanup.
    pub(super) fn analyze_with_source_facts(
        dom: &Dom,
        root: NodeId,
        source_facts: Option<&SemanticSourceFacts>,
        source_evidence: Option<&SourceEvidence>,
    ) -> Self {
        let mut nodes = Vec::with_capacity(dom.len());
        nodes.push(root);
        if let Some(source_facts) = source_facts {
            nodes.extend(source_facts.live_nodes(root));
        } else {
            nodes.extend(dom.descendants(root));
        }
        let mut facts = vec![NodeFacts::default(); dom.len()];
        let mut inventory = FeatureInventory::default();

        for &node in &nodes {
            let Some(tag) = dom.tag(node) else {
                continue;
            };
            let is_code_block = source_facts.map_or_else(
                || {
                    tag == Tag::Pre
                        || tag == Tag::Code && super::code::is_multiline_orphan(dom, node)
                },
                |facts| facts.is_code_block(node),
            );
            if is_code_block && let Some(source) = super::code::owned_source_candidate(dom, node) {
                inventory.owned_code_sources.push((node, source));
            }
            let level = heading_level(dom, node).unwrap_or(0);
            facts[node.index()].heading_level = level;
            if level != 0 {
                inventory.headings.push(node);
            }
            if tag == Tag::A
                && dom
                    .attr(node, AttrName::Href)
                    .is_some_and(|href| href.trim().starts_with('#'))
            {
                inventory.fragment_links.push(node);
            }
            if matches!(tag, Tag::Img | Tag::Picture | Tag::Figure) {
                inventory.images.push(node);
            }
            if matches!(tag, Tag::Figure | Tag::Figcaption)
                || super::figures::class_is_semantic_evidence(dom, node)
            {
                inventory.figures.push(node);
            }
            if matches!(tag, Tag::Iframe | Tag::Video | Tag::Audio) {
                inventory.media.push(node);
            }
            if matches!(tag, Tag::Ul | Tag::Ol)
                || dom.attr(node, AttrName::Role).is_some_and(|roles| {
                    roles
                        .split_ascii_whitespace()
                        .any(|role| role.eq_ignore_ascii_case("list"))
                })
            {
                inventory.lists.push(node);
            }
            if tag == Tag::Table {
                inventory.tables.push(node);
            }
            if source_evidence.is_some_and(|evidence| evidence.callout_candidate(node))
                || source_evidence.is_none()
                    && matches!(tag, Tag::Aside | Tag::Div | Tag::Section)
                    && super::callouts::class_is_semantic_evidence(dom, node)
            {
                inventory.callouts.push(node);
            }
            if source_evidence.is_some_and(|evidence| evidence.math(node))
                || source_evidence.is_none()
                    && (super::math::is_source_evidence(dom, node)
                        || super::math::class_is_semantic_evidence(dom, node))
            {
                inventory.math.push(node);
            }
            if source_evidence.is_some_and(|evidence| evidence.footnote_candidate(node))
                || source_evidence.is_none()
                    && super::footnotes::has_possible_footnote_evidence(dom, node)
            {
                inventory.footnotes.push(node);
            }
        }

        let needs_permalink_glyphs =
            !inventory.headings.is_empty() && !inventory.fragment_links.is_empty();

        if let Some(source_facts) = source_facts {
            for &node in nodes.iter().rev() {
                let mut first_visible = None;
                let mut last_visible = None;
                let mut has_visible_text = false;
                let mut glyph_only = true;
                let multiline = source_facts.multiline_content(node);
                let mut meaningful = false;

                if let Some(text) = dom.text_node(node) {
                    first_visible = text.chars().find(|character| !character.is_whitespace());
                    last_visible = text
                        .chars()
                        .rev()
                        .find(|character| !character.is_whitespace());
                    has_visible_text = first_visible.is_some();
                    glyph_only = needs_permalink_glyphs
                        && text
                            .chars()
                            .filter(|character| !character.is_whitespace())
                            .all(is_permalink_glyph);
                    meaningful = has_visible_text;
                } else {
                    for child in dom.children(node) {
                        let child_facts = facts[child.index()];
                        first_visible = first_visible.or(child_facts.first_visible);
                        if child_facts.last_visible.is_some() {
                            last_visible = child_facts.last_visible;
                        }
                        if child_facts.has(HAS_VISIBLE_TEXT) {
                            has_visible_text = true;
                            glyph_only &= child_facts.has(IS_GLYPH_ONLY);
                        }
                        meaningful |= child_facts.has(HAS_MEANINGFUL_CONTENT);
                    }
                }

                let tag = dom.tag(node);
                let code_block = source_facts.is_code_block(node);
                let block_descendant = dom.children(node).any(|child| {
                    let child_facts = facts[child.index()];
                    child_facts.has(IS_CODE_BLOCK)
                        || dom.tag(child).is_some_and(is_block_tag)
                        || child_facts.has(HAS_BLOCK_DESCENDANT)
                });
                meaningful |= tag.is_some_and(|tag| {
                    matches!(
                        tag,
                        Tag::Br
                            | Tag::Code
                            | Tag::Hr
                            | Tag::Img
                            | Tag::Iframe
                            | Tag::Video
                            | Tag::Audio
                    )
                });

                let node_facts = &mut facts[node.index()];
                node_facts.first_visible = first_visible;
                node_facts.last_visible = last_visible;
                node_facts.set(HAS_VISIBLE_TEXT, has_visible_text);
                node_facts.set(IS_GLYPH_ONLY, has_visible_text && glyph_only);
                node_facts.set(HAS_MULTILINE_CONTENT, multiline);
                node_facts.set(IS_CODE_BLOCK, code_block);
                node_facts.set(HAS_BLOCK_DESCENDANT, block_descendant);
                node_facts.set(HAS_MEANINGFUL_CONTENT, meaningful);
            }
        } else {
            propagate_without_source_facts(dom, &nodes, &mut facts, needs_permalink_glyphs);
        }

        Self {
            nodes,
            facts,
            inventory,
        }
    }

    pub(super) fn nodes(&self) -> &[NodeId] {
        &self.nodes
    }

    pub(super) fn inventory(&self) -> &FeatureInventory {
        &self.inventory
    }

    pub(super) fn first_visible(&self, node: NodeId) -> Option<char> {
        self.facts[node.index()].first_visible
    }

    pub(super) fn last_visible(&self, node: NodeId) -> Option<char> {
        self.facts[node.index()].last_visible
    }

    pub(super) fn has_visible_text(&self, node: NodeId) -> bool {
        self.facts[node.index()].has(HAS_VISIBLE_TEXT)
    }

    pub(super) fn glyph_only(&self, node: NodeId) -> bool {
        self.facts[node.index()].has(IS_GLYPH_ONLY)
    }

    pub(super) fn has_block_descendant(&self, node: NodeId) -> bool {
        self.facts[node.index()].has(HAS_BLOCK_DESCENDANT)
    }

    pub(super) fn has_meaningful_content(&self, node: NodeId) -> bool {
        self.facts[node.index()].has(HAS_MEANINGFUL_CONTENT)
    }

    pub(super) fn is_code_block(&self, node: NodeId) -> bool {
        self.facts[node.index()].has(IS_CODE_BLOCK)
    }

    pub(super) fn heading_level(&self, node: NodeId) -> Option<u8> {
        let level = self.facts[node.index()].heading_level;
        (level != 0).then_some(level)
    }

    pub(super) fn is_heading_permalink(&self, node: NodeId) -> bool {
        self.facts[node.index()].has(IS_HEADING_PERMALINK)
    }

    pub(super) fn permalink_separates_words(&self, node: NodeId) -> bool {
        self.facts[node.index()].has(PERMALINK_SEPARATES_WORDS)
    }

    pub(super) fn heading_has_meaningful_content(&self, node: NodeId) -> bool {
        self.facts[node.index()].has(HEADING_HAS_MEANINGFUL_CONTENT)
    }

    pub(super) fn heading_has_permalink(&self, node: NodeId) -> bool {
        self.facts[node.index()].has(HEADING_HAS_PERMALINK)
    }

    pub(super) fn trims_heading_start(&self, node: NodeId) -> bool {
        self.facts[node.index()].has(TRIM_HEADING_START)
    }

    pub(super) fn trims_heading_end(&self, node: NodeId) -> bool {
        self.facts[node.index()].has(TRIM_HEADING_END)
    }

    pub(super) fn mark_heading_permalink(&mut self, node: NodeId) {
        self.facts[node.index()].set(IS_HEADING_PERMALINK, true);
    }

    pub(super) fn mark_permalink_separator(&mut self, node: NodeId) {
        self.facts[node.index()].set(PERMALINK_SEPARATES_WORDS, true);
    }

    pub(super) fn mark_heading_content(&mut self, node: NodeId, meaningful: bool) {
        self.facts[node.index()].set(HEADING_HAS_MEANINGFUL_CONTENT, meaningful);
    }

    pub(super) fn mark_heading_has_permalink(&mut self, node: NodeId) {
        self.facts[node.index()].set(HEADING_HAS_PERMALINK, true);
    }

    pub(super) fn mark_heading_trim_start(&mut self, node: NodeId) {
        self.facts[node.index()].set(TRIM_HEADING_START, true);
    }

    pub(super) fn mark_heading_trim_end(&mut self, node: NodeId) {
        self.facts[node.index()].set(TRIM_HEADING_END, true);
    }

    /// Adds meaning from recognized semantic leaves and propagates it once.
    pub(super) fn include_semantic_meaning(
        &mut self,
        dom: &Dom,
        mut recognized: impl FnMut(NodeId) -> bool,
    ) {
        for &node in self.nodes.iter().rev() {
            let meaningful = self.facts[node.index()].has(HAS_MEANINGFUL_CONTENT)
                || recognized(node)
                || dom
                    .children(node)
                    .any(|child| self.facts[child.index()].has(HAS_MEANINGFUL_CONTENT));
            self.facts[node.index()].set(HAS_MEANINGFUL_CONTENT, meaningful);
        }
    }
}

fn propagate_without_source_facts(
    dom: &Dom,
    nodes: &[NodeId],
    facts: &mut [NodeFacts],
    needs_permalink_glyphs: bool,
) {
    for &node in nodes.iter().rev() {
        let mut first_visible = None;
        let mut last_visible = None;
        let mut has_visible_text = false;
        let mut glyph_only = true;
        let mut multiline = false;
        let mut meaningful = false;

        if let Some(text) = dom.text_node(node) {
            first_visible = text.chars().find(|character| !character.is_whitespace());
            last_visible = text
                .chars()
                .rev()
                .find(|character| !character.is_whitespace());
            has_visible_text = first_visible.is_some();
            glyph_only = needs_permalink_glyphs
                && text
                    .chars()
                    .filter(|character| !character.is_whitespace())
                    .all(is_permalink_glyph);
            multiline = text.contains('\n');
            meaningful = has_visible_text;
        } else {
            for child in dom.children(node) {
                let child_facts = facts[child.index()];
                first_visible = first_visible.or(child_facts.first_visible);
                if child_facts.last_visible.is_some() {
                    last_visible = child_facts.last_visible;
                }
                if child_facts.has(HAS_VISIBLE_TEXT) {
                    has_visible_text = true;
                    glyph_only &= child_facts.has(IS_GLYPH_ONLY);
                }
                multiline |= child_facts.has(HAS_MULTILINE_CONTENT);
                meaningful |= child_facts.has(HAS_MEANINGFUL_CONTENT);
            }
        }

        let tag = dom.tag(node);
        multiline |= tag == Some(Tag::Br);
        let code_block = tag == Some(Tag::Pre)
            || super::code::is_multiline_orphan_with_evidence(dom, node, multiline);
        let block_descendant = dom.children(node).any(|child| {
            let child_facts = facts[child.index()];
            child_facts.has(IS_CODE_BLOCK)
                || dom.tag(child).is_some_and(is_block_tag)
                || child_facts.has(HAS_BLOCK_DESCENDANT)
        });
        meaningful |= tag.is_some_and(|tag| {
            matches!(
                tag,
                Tag::Br | Tag::Code | Tag::Hr | Tag::Img | Tag::Iframe | Tag::Video | Tag::Audio
            )
        });

        let node_facts = &mut facts[node.index()];
        node_facts.first_visible = first_visible;
        node_facts.last_visible = last_visible;
        node_facts.set(HAS_VISIBLE_TEXT, has_visible_text);
        node_facts.set(IS_GLYPH_ONLY, has_visible_text && glyph_only);
        node_facts.set(HAS_MULTILINE_CONTENT, multiline);
        node_facts.set(IS_CODE_BLOCK, code_block);
        node_facts.set(HAS_BLOCK_DESCENDANT, block_descendant);
        node_facts.set(HAS_MEANINGFUL_CONTENT, meaningful);
    }
}

fn is_permalink_glyph(character: char) -> bool {
    matches!(character, '#' | '¶' | '§' | '🔗')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_evidence_is_sparse_and_reuses_semantic_gates() {
        let dom = Dom::parse_fragment(
            r##"<div><p class="ordinary">Text</p><aside class="admonition warning"><p>Warning</p></aside><p data-latex="x^2">x squared</p><p><a role="doc-noteref" href="#note">1</a></p><p><a href="article.html#fn1">2</a></p><aside id="note" role="doc-footnote">A note.</aside><p id="fn1">A second note.</p><table role="table"><tr><th>Value</th></tr></table></div>"##,
            Tag::Div,
        )
        .unwrap();
        let mut store = NodeStateStore::new();
        let mut tables = Vec::new();
        crate::cleaning::mark_data_tables(&dom, dom.root(), &mut store, &mut tables);
        let evidence = SourceEvidence::analyze(&dom, dom.root(), &store);
        let ordinary = dom
            .descendants(dom.root())
            .find(|&node| dom.attr(node, AttrName::Class) == Some("ordinary"))
            .unwrap();
        let warning = dom
            .descendants(dom.root())
            .find(|&node| dom.attr(node, AttrName::Class) == Some("admonition warning"))
            .unwrap();
        let math = dom
            .descendants(dom.root())
            .find(|&node| dom.attr(node, AttrName::DataLatex).is_some())
            .unwrap();
        let conventional_reference = dom
            .descendants(dom.root())
            .find(|&node| dom.attr(node, AttrName::Href) == Some("article.html#fn1"))
            .unwrap();
        let conventional_definition = dom
            .descendants(dom.root())
            .find(|&node| dom.attr(node, AttrName::Id) == Some("fn1"))
            .unwrap();
        let table = dom
            .descendants(dom.root())
            .find(|&node| dom.tag(node) == Some(Tag::Table))
            .unwrap();
        assert!(!evidence.is_semantic_source(ordinary));
        assert!(evidence.callout_candidate(warning));
        assert!(evidence.math(math));
        assert!(evidence.footnote_candidate(conventional_reference));
        assert!(evidence.footnote_candidate(conventional_definition));
        assert!(evidence.data_table(table));
    }

    #[test]
    fn cleanup_gate_matches_direct_source_evidence_analysis() {
        let mut dom = Dom::parse_fragment(
            r##"<div><p class="ordinary">Text</p><aside class="admonition warning"><p>Warning</p></aside><p data-latex="x^2">x squared</p><p><a role="doc-noteref" href="#note">1</a></p><aside id="note" role="doc-footnote">A note.</aside><table role="table"><tr><th>Value</th></tr></table></div>"##,
            Tag::Div,
        )
        .unwrap();
        let mut store = NodeStateStore::new();
        let mut tables = Vec::new();
        crate::cleaning::mark_data_tables(&dom, dom.root(), &mut store, &mut tables);
        let root = dom.root();
        let direct = SourceEvidence::analyze(&dom, root, &store);

        let mut gate = SemanticGate::default();
        let mut nodes = Vec::new();
        crate::cleaning::clean_styles_with_semantic_gate(&mut dom, root, &mut nodes, &mut gate);
        for &node in &tables {
            if store.is_data_table(node) == Some(true) {
                gate.add_data_table_node(node);
            }
        }
        let gated = SourceEvidence::analyze_with_gate(&dom, root, &store, gate);

        for node in std::iter::once(root).chain(dom.descendants(root)) {
            assert_eq!(gated.callout(node), direct.callout(node));
            assert_eq!(
                gated.callout_candidate(node),
                direct.callout_candidate(node)
            );
            assert_eq!(gated.footnote(node), direct.footnote(node));
            assert_eq!(
                gated.footnote_candidate(node),
                direct.footnote_candidate(node)
            );
            assert_eq!(gated.math(node), direct.math(node));
            assert_eq!(gated.accessible_math(node), direct.accessible_math(node));
            assert_eq!(
                gated.contains_semantic(node),
                direct.contains_semantic(node)
            );
            assert_eq!(gated.data_table(node), direct.data_table(node));
        }

        let mut plain = Dom::parse_fragment("<div><p>Ordinary text.</p></div>", Tag::Div).unwrap();
        let plain_root = plain.root();
        let mut plain_gate = SemanticGate::default();
        let mut plain_nodes = Vec::new();
        crate::cleaning::clean_styles_with_semantic_gate(
            &mut plain,
            plain_root,
            &mut plain_nodes,
            &mut plain_gate,
        );
        assert!(plain_gate.is_empty());
        assert!(
            !SourceEvidence::analyze_with_gate(
                &plain,
                plain_root,
                &NodeStateStore::new(),
                plain_gate,
            )
            .is_semantic_source(plain_root)
        );
    }

    #[test]
    fn unrelated_ids_do_not_trigger_fragment_semantic_evidence() {
        let dom = Dom::parse_fragment(
            r#"<div><p id="section">Section text</p><p class="ordinary">More text</p></div>"#,
            Tag::Div,
        )
        .unwrap();
        let evidence = SourceEvidence::analyze(&dom, dom.root(), &NodeStateStore::new());
        let section = dom
            .descendants(dom.root())
            .find(|&node| dom.attr(node, AttrName::Id) == Some("section"))
            .unwrap();
        assert!(!evidence.footnote_candidate(section));
        assert!(!evidence.contains_semantic(dom.root()));
    }

    #[test]
    fn layout_tables_do_not_activate_data_table_evidence() {
        let dom = Dom::parse_fragment(
            "<div><table><tr><td>Layout</td></tr></table><p>Text</p></div>",
            Tag::Div,
        )
        .unwrap();
        let evidence = SourceEvidence::analyze(&dom, dom.root(), &NodeStateStore::new());
        let table = dom
            .descendants(dom.root())
            .find(|&node| dom.tag(node) == Some(Tag::Table))
            .unwrap();
        assert!(!evidence.data_table(table));
        assert!(!evidence.contains_semantic(dom.root()));
    }

    #[test]
    fn unresolved_fragment_targets_do_not_trigger_semantic_evidence() {
        let dom = Dom::parse_fragment(
            r##"<div><p><a href="#missing">Missing</a></p><p id="section">Section text</p></div>"##,
            Tag::Div,
        )
        .unwrap();
        let evidence = SourceEvidence::analyze(&dom, dom.root(), &NodeStateStore::new());
        let reference = dom
            .descendants(dom.root())
            .find(|&node| dom.attr(node, AttrName::Href) == Some("#missing"))
            .unwrap();
        assert!(!evidence.footnote_candidate(reference));
    }

    #[test]
    fn external_and_query_fragments_do_not_trigger_local_evidence() {
        let dom = Dom::parse_fragment(
            r##"<div><p><a href="https://example.test/page#note">Remote</a></p><p id="note">Remote target</p><p><a href="?view=all#query">Query</a></p><p id="query">Query target</p></div>"##,
            Tag::Div,
        )
        .unwrap();
        let evidence = SourceEvidence::analyze(&dom, dom.root(), &NodeStateStore::new());
        for href in ["https://example.test/page#note", "?view=all#query"] {
            let reference = dom
                .descendants(dom.root())
                .find(|&node| dom.attr(node, AttrName::Href) == Some(href))
                .unwrap();
            assert!(!evidence.footnote_candidate(reference));
        }
    }

    #[test]
    fn inventories_features_and_propagates_shared_facts() {
        let dom = Dom::parse_fragment(
            r##"<h2>API <a href="#api">#</a></h2><div><code>a
b</code></div><table><tr><td>value</td></tr></table>"##,
            Tag::Div,
        )
        .unwrap();
        let facts = SemanticFacts::analyze(&dom, dom.root());
        assert_eq!(facts.inventory().headings.len(), 1);
        assert_eq!(facts.inventory().fragment_links.len(), 1);
        assert_eq!(facts.inventory().tables.len(), 1);
        let code = dom
            .descendants(dom.root())
            .find(|&node| dom.tag(node) == Some(Tag::Code))
            .unwrap();
        assert!(facts.is_code_block(code));
        assert!(facts.has_block_descendant(dom.parent(code).unwrap()));
        assert_eq!(facts.first_visible(code), Some('a'));
        assert_eq!(facts.last_visible(code), Some('b'));
    }

    #[test]
    fn keeps_specialized_worklists_sparse_and_propagates_added_meaning() {
        let dom = Dom::parse_fragment(
            "<div><span>ordinary</span><math><mi>x</mi></math><table><tr><td>x<table><tr><td>y</td></tr></table></td></tr></table></div>",
            Tag::Div,
        )
        .unwrap();
        let mut facts = SemanticFacts::analyze(&dom, dom.root());
        assert!(facts.inventory().images.is_empty());
        assert!(facts.inventory().media.is_empty());
        assert!(facts.inventory().lists.is_empty());
        assert_eq!(facts.inventory().math.len(), 1);
        assert_eq!(facts.inventory().tables.len(), 2);

        let math = facts.inventory().math[0];
        facts.include_semantic_meaning(&dom, |node| node == math);
        assert!(facts.has_meaningful_content(math));
        assert!(facts.has_meaningful_content(dom.root()));
    }

    #[test]
    fn deep_shared_fact_analysis_is_stack_safe() {
        let depth = 4_000;
        let html = format!("{}text{}", "<span>".repeat(depth), "</span>".repeat(depth));
        let dom = Dom::parse_fragment(&html, Tag::Div).unwrap();
        let facts = SemanticFacts::analyze(&dom, dom.root());
        assert!(facts.has_visible_text(dom.root()));
        assert_eq!(facts.first_visible(dom.root()), Some('t'));
        assert_eq!(facts.last_visible(dom.root()), Some('t'));
    }

    #[test]
    fn repairs_multiline_code_facts_after_a_cleanup_detach() {
        let mut dom = Dom::parse_fragment("<code><span><br></span></code>", Tag::Div).unwrap();
        let root = dom.root();
        let mut source = SemanticSourceFacts::analyze(&dom, root);
        let code = dom
            .descendants(root)
            .find(|&node| dom.tag(node) == Some(Tag::Code))
            .unwrap();
        let span = dom
            .descendants(root)
            .find(|&node| dom.tag(node) == Some(Tag::Span))
            .unwrap();
        assert!(source.multiline_content(code));
        assert!(source.is_code_block(code));

        let parent = dom.parent(span);
        dom.detach(span);
        source.node_detached(span, parent);
        source.refresh_after_cleanup(&dom);

        assert!(!source.multiline_content(code));
        assert!(!source.is_code_block(code));
    }

    #[test]
    fn refreshes_code_block_status_when_a_sibling_is_detached() {
        let mut dom =
            Dom::parse_fragment("<p><code>a\nb</code><span></span></p>", Tag::Div).unwrap();
        let root = dom.root();
        let mut source = SemanticSourceFacts::analyze(&dom, root);
        let code = dom
            .descendants(root)
            .find(|&node| dom.tag(node) == Some(Tag::Code))
            .unwrap();
        let span = dom
            .descendants(root)
            .find(|&node| dom.tag(node) == Some(Tag::Span))
            .unwrap();
        assert!(!source.is_code_block(code));

        let parent = dom.parent(span);
        dom.detach(span);
        source.node_detached(span, parent);
        source.refresh_after_cleanup(&dom);

        assert!(source.is_code_block(code));
    }

    #[test]
    fn rebased_facts_keep_a_live_source_root_in_the_fragment() {
        let dom = Dom::parse_fragment("<section><p>text</p></section>", Tag::Div).unwrap();
        let root = dom.root();
        let source_root = dom
            .descendants(root)
            .find(|&node| dom.tag(node) == Some(Tag::Section))
            .unwrap();
        let mut source = SemanticSourceFacts::analyze(&dom, source_root);
        source.rebase_root(&dom, root);

        let live = source.live_nodes(root);
        assert_eq!(live.first().copied(), Some(source_root));
        assert!(live.iter().any(|&node| dom.tag(node) == Some(Tag::P)));
    }
}
