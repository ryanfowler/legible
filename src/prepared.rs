//! Immutable facts for the prepared source document.
//!
//! This index is valid only while the prepared source DOM remains immutable.
//! Selected fragments and scoring views must build their own indexes because
//! those trees can be detached, renamed, or otherwise changed.

use crate::document::{has_math_wrapper_class, is_math_root, is_tex_annotation};
use crate::dom::{AttrName, DocumentAnchors, Dom, NodeId, Tag};
use crate::quality::ContentMetrics;
use crate::scoring::{has_hidden_utility_class_for_discovery, is_probably_visible};
use std::collections::HashSet;

const NO_POSITION: u32 = u32::MAX;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct SourceFlags(u16);

impl SourceFlags {
    pub(crate) const ELEMENT: Self = Self(1 << 0);
    pub(crate) const STATIC_HIDDEN: Self = Self(1 << 1);
    pub(crate) const UTILITY_HIDDEN: Self = Self(1 << 2);
    pub(crate) const ARIA_HIDDEN: Self = Self(1 << 8);
    pub(crate) const MODAL_DIALOG: Self = Self(1 << 3);
    pub(crate) const DOCUMENT_CHROME: Self = Self(1 << 4);
    pub(crate) const PRIMARY_REGION: Self = Self(1 << 5);
    pub(crate) const LINK: Self = Self(1 << 6);
    pub(crate) const PRIMARY_HEADING: Self = Self(1 << 7);
    pub(crate) const HAS_NON_WHITESPACE_TEXT: Self = Self(1 << 9);

    fn insert(&mut self, flag: Self) {
        self.0 |= flag.0;
    }

    pub(crate) fn contains(self, flag: Self) -> bool {
        self.0 & flag.0 != 0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SourceEntry {
    pub(crate) node: NodeId,
    pub(crate) subtree_end: u32,
    pub(crate) depth: u32,
    pub(crate) tag: Option<Tag>,
    pub(crate) flags: SourceFlags,
}

impl SourceEntry {
    pub(crate) fn is_element(self) -> bool {
        self.flags.contains(SourceFlags::ELEMENT)
    }
}

pub(crate) struct PreparedSource {
    /// The attached source root and every attached descendant, including text
    /// and comment nodes. Consumers filter on `SourceEntry::is_element` when
    /// they need structural facts.
    pub(crate) entries: Vec<SourceEntry>,
    position_by_node: Vec<u32>,
    pub(crate) anchors: DocumentAnchors,
    pub(crate) element_count: usize,
    has_possible_footnote_reference: bool,
    pub(crate) source_metrics: ContentMetrics,
    pub(crate) relaxed_metrics: Option<ContentMetrics>,
}

#[derive(Clone, Copy)]
pub(crate) enum SourceElements<'a> {
    Prepared(&'a PreparedSource),
    Snapshot(&'a [(NodeId, u32)]),
}

pub(crate) struct SourceElementsIter<'a> {
    inner: SourceElementsIterInner<'a>,
}

enum SourceElementsIterInner<'a> {
    Prepared(std::slice::Iter<'a, SourceEntry>),
    Snapshot(std::slice::Iter<'a, (NodeId, u32)>),
}

impl<'a> SourceElements<'a> {
    pub(crate) fn iter(self) -> SourceElementsIter<'a> {
        let inner = match self {
            Self::Prepared(source) => SourceElementsIterInner::Prepared(source.entries.iter()),
            Self::Snapshot(snapshot) => SourceElementsIterInner::Snapshot(snapshot.iter()),
        };
        SourceElementsIter { inner }
    }
}

impl Iterator for SourceElementsIter<'_> {
    type Item = (NodeId, u32);

    fn next(&mut self) -> Option<Self::Item> {
        match &mut self.inner {
            SourceElementsIterInner::Prepared(entries) => entries
                .find(|entry| entry.is_element())
                .map(|entry| (entry.node, entry.depth)),
            SourceElementsIterInner::Snapshot(entries) => entries.next().copied(),
        }
    }
}

impl DoubleEndedIterator for SourceElementsIter<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        match &mut self.inner {
            SourceElementsIterInner::Prepared(entries) => entries
                .rev()
                .find(|entry| entry.is_element())
                .map(|entry| (entry.node, entry.depth)),
            SourceElementsIterInner::Snapshot(entries) => entries.next_back().copied(),
        }
    }
}

impl PreparedSource {
    #[cfg(test)]
    pub(crate) fn build(dom: &Dom) -> Self {
        Self::build_with_semantic_counts(dom, true)
    }

    pub(crate) fn build_with_semantic_counts(dom: &Dom, include_semantic_counts: bool) -> Self {
        crate::instrumentation::record_source_full_scan();
        crate::instrumentation::record_prepared_source_build();

        let mut entries = Vec::with_capacity(dom.len());
        let mut position_by_node = vec![NO_POSITION; dom.len()];
        let mut open: Vec<usize> = Vec::new();
        let mut anchors = DocumentAnchors::new(dom.root());
        let mut element_count = 0;
        let mut has_possible_footnote_reference = false;

        for node in std::iter::once(dom.root()).chain(dom.descendants(dom.root())) {
            dom.record_document_anchor(node, &mut anchors);
            let tag = dom.tag(node);
            let depth = if node == dom.root() {
                0
            } else {
                dom.parent(node)
                    .and_then(|parent| position_by_node.get(parent.index()).copied())
                    .filter(|&position| position != NO_POSITION)
                    .and_then(|position| entries.get(position as usize))
                    .map_or(0, |parent: &SourceEntry| parent.depth.saturating_add(1))
            };
            let position = entries.len();
            while open
                .last()
                .is_some_and(|&parent| entries[parent].depth >= depth)
            {
                let parent = open.pop().expect("open source entry");
                entries[parent].subtree_end = position as u32;
            }

            let flags = source_flags(dom, node, tag);
            has_possible_footnote_reference |= possible_footnote_reference(dom, node, tag);
            if flags.contains(SourceFlags::ELEMENT) {
                element_count += 1;
            }
            entries.push(SourceEntry {
                node,
                subtree_end: 0,
                depth,
                tag,
                flags,
            });
            if let Some(slot) = position_by_node.get_mut(node.index()) {
                *slot = position as u32;
            }
            open.push(position);
        }
        let end = entries.len() as u32;
        for position in open {
            entries[position].subtree_end = end;
        }

        // Cache subtree text presence while the source index is already in
        // reverse preorder. Candidate discovery uses this to reject empty
        // structural wrappers without rescanning each descendant subtree.
        for position in (0..entries.len()).rev() {
            let node = entries[position].node;
            if dom
                .text_node(node)
                .is_some_and(|text| !text.trim().is_empty())
            {
                entries[position]
                    .flags
                    .insert(SourceFlags::HAS_NON_WHITESPACE_TEXT);
            }
            if entries[position]
                .flags
                .contains(SourceFlags::HAS_NON_WHITESPACE_TEXT)
                && let Some(parent) = dom.parent(node)
                && let Some(parent_position) = position_by_node
                    .get(parent.index())
                    .copied()
                    .filter(|&position| position != NO_POSITION)
            {
                entries[parent_position as usize]
                    .flags
                    .insert(SourceFlags::HAS_NON_WHITESPACE_TEXT);
            }
        }

        let mut source = Self {
            entries,
            position_by_node,
            anchors,
            element_count,
            has_possible_footnote_reference,
            source_metrics: ContentMetrics::default(),
            relaxed_metrics: None,
        };
        if let Some(body) = source.anchors.body {
            // Reuse the exclusion mask when a relaxed visibility view is
            // needed. The metric pass owns the mask only for the duration of
            // source preparation, so it adds no retained source state.
            let mut excluded = Vec::new();
            source.source_metrics = ContentMetrics::measure_source_prepared(
                dom,
                &source,
                body,
                false,
                include_semantic_counts,
                &mut excluded,
            );
            if source.has_relaxable_hidden_content(body) {
                source.relaxed_metrics = Some(ContentMetrics::measure_source_prepared(
                    dom,
                    &source,
                    body,
                    true,
                    include_semantic_counts,
                    &mut excluded,
                ));
            }
        }
        crate::instrumentation::record_prepared_source_entries(source.entries.len());
        source
    }

    pub(crate) fn position(&self, node: NodeId) -> Option<usize> {
        self.position_by_node
            .get(node.index())
            .copied()
            .filter(|&position| position != NO_POSITION)
            .map(|position| position as usize)
    }

    pub(crate) fn entry(&self, node: NodeId) -> Option<&SourceEntry> {
        self.position(node)
            .and_then(|position| self.entries.get(position))
    }

    #[allow(dead_code)]
    pub(crate) fn contains(&self, ancestor: NodeId, descendant: NodeId) -> bool {
        let Some(ancestor) = self.entry(ancestor) else {
            return false;
        };
        let Some(descendant) = self.position(descendant) else {
            return false;
        };
        let start = self.position(ancestor.node).unwrap_or(usize::MAX);
        start <= descendant && descendant < ancestor.subtree_end as usize
    }

    pub(crate) fn subtree_range(&self, node: NodeId) -> Option<std::ops::Range<usize>> {
        let entry = self.entry(node)?;
        let start = self.position(node)?;
        Some(start..entry.subtree_end as usize)
    }

    #[allow(dead_code)]
    pub(crate) fn depth(&self, node: NodeId) -> Option<u32> {
        self.entry(node).map(|entry| entry.depth)
    }

    pub(crate) fn elements(&self) -> impl DoubleEndedIterator<Item = &SourceEntry> {
        self.entries.iter().filter(|entry| entry.is_element())
    }

    pub(crate) fn elements_in(&self, root: NodeId) -> impl Iterator<Item = &SourceEntry> {
        let range = self.subtree_range(root).unwrap_or(0..0);
        self.entries[range]
            .iter()
            .filter(|entry| entry.is_element())
    }

    pub(crate) fn entries_in(&self, root: NodeId) -> impl Iterator<Item = &SourceEntry> {
        let range = self.subtree_range(root).unwrap_or(0..0);
        self.entries[range].iter()
    }

    pub(crate) fn has_possible_footnote_reference(&self) -> bool {
        self.has_possible_footnote_reference
    }

    pub(crate) fn has_relaxable_hidden_content(&self, root: NodeId) -> bool {
        self.elements_in(root).any(|entry| {
            matches!(
                entry.tag,
                Some(Tag::Article | Tag::Aside | Tag::Div | Tag::Main | Tag::Nav | Tag::Section)
            ) && entry.flags.contains(SourceFlags::STATIC_HIDDEN)
                && !entry.flags.contains(SourceFlags::MODAL_DIALOG)
        })
    }

    pub(crate) fn accessible_math_nodes(&self, dom: &Dom) -> HashSet<NodeId> {
        let annotations: Vec<_> = self
            .elements()
            .map(|entry| entry.node)
            .filter(|&node| is_tex_annotation(dom, node))
            .collect();
        if annotations.is_empty() {
            return HashSet::new();
        }

        let mut relevant = HashSet::new();
        for annotation in annotations {
            let mut node = Some(annotation);
            while let Some(current) = node {
                relevant.insert(current);
                if current == self.anchors.root {
                    break;
                }
                node = dom.parent(current);
            }
        }

        let mut inside_wrapper = HashSet::new();
        let mut accessible = HashSet::new();
        for entry in self.elements() {
            let node = entry.node;
            if !relevant.contains(&node) {
                continue;
            }
            let inherited = dom
                .parent(node)
                .is_some_and(|parent| inside_wrapper.contains(&parent));
            let wrapper = inherited || has_math_wrapper_class(dom, node);
            if wrapper {
                inside_wrapper.insert(node);
            }
            if is_math_root(dom, node) || wrapper {
                accessible.insert(node);
            }
        }
        accessible
    }
}

fn has_local_fragment_target(href: &str) -> bool {
    let href = href.trim();
    let Some((prefix, target)) = href.rsplit_once('#') else {
        return false;
    };
    let has_scheme = prefix.find(':').is_some_and(|colon| {
        colon > 0
            && prefix[..colon]
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'-' | b'.'))
    });
    !target.is_empty()
        && (prefix.is_empty() || !has_scheme && !prefix.starts_with('/') && !prefix.contains('?'))
}

fn possible_footnote_reference(dom: &Dom, node: NodeId, tag: Option<Tag>) -> bool {
    (tag == Some(Tag::A)
        && dom
            .attr(node, AttrName::Href)
            .is_some_and(has_local_fragment_target))
        || tag == Some(Tag::Label)
            && dom.attr(node, AttrName::Class).is_some_and(|classes| {
                classes.split_whitespace().any(|class| {
                    class.eq_ignore_ascii_case("footref")
                        || class.eq_ignore_ascii_case("sidenote-number")
                })
            })
            && dom.attr_by_local_name(node, "for").is_some()
        || dom.attr(node, AttrName::DataFootnoteRef).is_some()
        || dom.attr_by_local_name(node, "data-footnote-ref").is_some()
}

fn source_flags(dom: &Dom, node: NodeId, tag: Option<Tag>) -> SourceFlags {
    let Some(tag) = tag else {
        return SourceFlags::default();
    };
    let mut flags = SourceFlags::ELEMENT;
    let utility_hidden = has_hidden_utility_class_for_discovery(dom, node);
    let static_hidden = dom.attr(node, AttrName::AriaHidden) != Some("true")
        && (!is_probably_visible(dom, node) || utility_hidden);
    if static_hidden {
        flags.insert(SourceFlags::STATIC_HIDDEN);
    }
    if utility_hidden {
        flags.insert(SourceFlags::UTILITY_HIDDEN);
    }
    if dom.attr(node, AttrName::AriaHidden) == Some("true") {
        flags.insert(SourceFlags::ARIA_HIDDEN);
    }
    if is_modal_or_dialog(dom, node, static_hidden, utility_hidden) {
        flags.insert(SourceFlags::MODAL_DIALOG);
    }
    if matches!(tag, Tag::Header | Tag::Footer | Tag::Nav)
        || dom.attr(node, AttrName::Role).is_some_and(|roles| {
            roles.split_whitespace().any(|role| {
                role.eq_ignore_ascii_case("banner") || role.eq_ignore_ascii_case("navigation")
            })
        })
    {
        flags.insert(SourceFlags::DOCUMENT_CHROME);
    }
    if matches!(tag, Tag::Article | Tag::Main)
        || dom.attr(node, AttrName::Role).is_some_and(|roles| {
            roles.split_whitespace().any(|role| {
                role.eq_ignore_ascii_case("article") || role.eq_ignore_ascii_case("main")
            })
        })
    {
        flags.insert(SourceFlags::PRIMARY_REGION);
    }
    if tag == Tag::A {
        flags.insert(SourceFlags::LINK);
    }
    if matches!(
        tag,
        Tag::H1 | Tag::H2 | Tag::H3 | Tag::H4 | Tag::H5 | Tag::H6
    ) || dom.attr(node, AttrName::Role).is_some_and(|role| {
        role.split_whitespace()
            .any(|value| value.eq_ignore_ascii_case("heading"))
    }) || dom.attr_by_local_name(node, "aria-level").is_some()
    {
        flags.insert(SourceFlags::PRIMARY_HEADING);
    }
    flags
}

fn is_modal_or_dialog(dom: &Dom, node: NodeId, static_hidden: bool, utility_hidden: bool) -> bool {
    dom.attr(node, AttrName::AriaModal) == Some("true")
        || dom.attr(node, AttrName::Role).is_some_and(|roles| {
            roles.split_whitespace().any(|role| {
                role.eq_ignore_ascii_case("dialog") || role.eq_ignore_ascii_case("alertdialog")
            })
        })
        || (static_hidden || utility_hidden)
            && dom.attr(node, AttrName::Class).is_some_and(|classes| {
                classes.split_whitespace().any(|class| {
                    class.eq_ignore_ascii_case("modal") || class.eq_ignore_ascii_case("dialog")
                })
            })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intervals_match_repaired_dom_ancestry() {
        let dom = Dom::parse_document(
            "<body><main><div><p>one</p><p>two</p></div></main><aside>side</aside></body>",
        )
        .unwrap();
        let source = PreparedSource::build(&dom);
        let main = source
            .anchors
            .body
            .and_then(|body| {
                source
                    .elements_in(body)
                    .find(|entry| entry.tag == Some(Tag::Main))
                    .map(|entry| entry.node)
            })
            .unwrap();
        let paragraph = source
            .elements_in(main)
            .find(|entry| entry.tag == Some(Tag::P))
            .unwrap()
            .node;
        let aside = source
            .elements()
            .find(|entry| entry.tag == Some(Tag::Aside))
            .unwrap()
            .node;

        assert!(source.contains(main, paragraph));
        assert!(!source.contains(main, aside));
        assert_eq!(
            source.depth(paragraph),
            Some(source.depth(main).unwrap() + 2)
        );
        let text = dom
            .children(
                source
                    .elements_in(main)
                    .find(|entry| entry.tag == Some(Tag::P))
                    .unwrap()
                    .node,
            )
            .find(|node| dom.is_text(*node))
            .unwrap();
        assert!(source.contains(main, text));
        assert_eq!(source.subtree_range(main).unwrap().len(), 6);
    }

    #[test]
    fn source_index_keeps_anchor_handles_and_flags() {
        let dom = Dom::parse_document(
            "<html lang='en'><head><base href='/docs'></head><body><header>chrome</header><main><h1>Title</h1><a href='/x'>link</a></main></body></html>",
        )
        .unwrap();
        let source = PreparedSource::build(&dom);
        let html = source.anchors.html.unwrap();
        let body = source.anchors.body.unwrap();
        assert_eq!(dom.attr(html, AttrName::Lang), Some("en"));
        assert!(source.entry(body).unwrap().is_element());
        assert!(
            source
                .elements()
                .any(|entry| entry.flags.contains(SourceFlags::LINK))
        );
        assert_eq!(source.element_count, source.elements().count());
        assert!(!source.has_possible_footnote_reference());
    }

    #[test]
    fn source_index_detects_reference_targets_without_a_second_scan() {
        let dom = Dom::parse_document(
            "<body><main><p><a href='#note'>1</a></p><p data-footnote-ref='note'>2</p><label class='footref' for='note'>3</label></main></body>",
        )
        .unwrap();
        let source = PreparedSource::build(&dom);
        assert!(source.has_possible_footnote_reference());

        let external = Dom::parse_document(
            "<body><main><p><a href='https://example.test/page#section'>section</a></p></main></body>",
        )
        .unwrap();
        assert!(!PreparedSource::build(&external).has_possible_footnote_reference());
    }

    #[test]
    fn detached_nodes_are_not_indexed() {
        let mut dom =
            Dom::parse_document("<body><main><p>kept</p></main><aside>removed</aside></body>")
                .unwrap();
        let aside = dom.first_descendant_by_tag(dom.root(), Tag::Aside).unwrap();
        dom.detach(aside);
        let source = PreparedSource::build(&dom);

        assert!(source.entry(aside).is_none());
        assert!(!source.contains(dom.root(), aside));
        assert!(!source.elements().any(|entry| entry.node == aside));
    }

    #[test]
    fn repaired_dom_preorder_is_preserved_for_documents_and_fragments() {
        let document = Dom::parse_document(
            "<body><table><p>fostered</table><div><b>misnested</div></b></body>",
        )
        .unwrap();
        let fragment = Dom::parse_fragment(
            "<section><p>fragment <em>content</em></p></section>",
            Tag::Div,
        )
        .unwrap();
        let document_source = PreparedSource::build(&document);
        let fragment_source = PreparedSource::build(&fragment);
        let document_nodes: Vec<_> = std::iter::once(document.root())
            .chain(document.descendants(document.root()))
            .collect();
        let fragment_nodes: Vec<_> = std::iter::once(fragment.root())
            .chain(fragment.descendants(fragment.root()))
            .collect();

        assert_eq!(
            document_source
                .entries
                .iter()
                .map(|entry| entry.node)
                .collect::<Vec<_>>(),
            document_nodes
        );
        assert_eq!(
            fragment_source
                .entries
                .iter()
                .map(|entry| entry.node)
                .collect::<Vec<_>>(),
            fragment_nodes
        );
        assert_ne!(
            document_source.entries.as_ptr(),
            fragment_source.entries.as_ptr()
        );
    }
}
