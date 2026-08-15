//! Shared source facts for complex semantic compilation.

use crate::dom::{AttrName, Dom, NodeId, Tag};

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
        Self::analyze_with_source_facts(dom, root, None)
    }

    /// Scans source evidence while reusing facts computed before final cleanup.
    pub(super) fn analyze_with_source_facts(
        dom: &Dom,
        root: NodeId,
        source_facts: Option<&SemanticSourceFacts>,
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
            if matches!(tag, Tag::Aside | Tag::Div | Tag::Section)
                && super::callouts::class_is_semantic_evidence(dom, node)
            {
                inventory.callouts.push(node);
            }
            if super::math::is_source_evidence(dom, node)
                || super::math::class_is_semantic_evidence(dom, node)
            {
                inventory.math.push(node);
            }
            if super::footnotes::has_possible_footnote_evidence(dom, node) {
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
