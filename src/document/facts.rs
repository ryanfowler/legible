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

/// Dense facts that serve more than one complex semantic concern.
pub(super) struct SemanticFacts {
    nodes: Vec<NodeId>,
    facts: Vec<NodeFacts>,
    inventory: FeatureInventory,
}

impl SemanticFacts {
    /// Scans source evidence in preorder and propagates generic facts in reverse order.
    pub(super) fn analyze(dom: &Dom, root: NodeId) -> Self {
        let mut nodes = Vec::with_capacity(dom.len());
        nodes.push(root);
        nodes.extend(dom.descendants(root));
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
}
