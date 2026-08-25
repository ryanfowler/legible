use crate::dom::{AttrName, Dom, NodeId, NodeLink, Tag};
use crate::tokens::{has_any_token, has_token};
use smallvec::SmallVec;
use std::collections::{HashMap, HashSet};

use super::sparse::SparseNodeValues;

#[inline]
fn trim_text(value: &str) -> &str {
    if value.is_ascii() {
        let bytes = value.as_bytes();
        let mut start = 0;
        while start < bytes.len()
            && (bytes[start] == b' ' || (b'\t'..=b'\r').contains(&bytes[start]))
        {
            start += 1;
        }
        let mut end = bytes.len();
        while end > start && (bytes[end - 1] == b' ' || (b'\t'..=b'\r').contains(&bytes[end - 1])) {
            end -= 1;
        }
        &value[start..end]
    } else {
        value.trim()
    }
}

#[derive(Clone)]
struct Reference {
    node: NodeId,
    key: String,
    label: Option<String>,
}

#[derive(Clone)]
struct Definition {
    node: NodeId,
    key: String,
    inline: bool,
}

pub(crate) struct FootnoteAnalysis {
    references: SparseNodeValues<Box<str>>,
    definitions: SparseNodeValues<Box<str>>,
    reference_slots: Vec<u32>,
    definition_slots: Vec<u32>,
    skipped: Vec<bool>,
    deferred: Vec<bool>,
    trim_start: Vec<bool>,
    transparent: Vec<bool>,
    available: HashSet<String>,
}

impl FootnoteAnalysis {
    pub(crate) fn analyze(dom: &Dom, root: NodeId) -> Self {
        if !std::iter::once(root)
            .chain(dom.descendants(root))
            .any(|node| has_possible_footnote_evidence(dom, node))
        {
            return Self::empty();
        }
        let nodes = source_nodes(dom, root);
        let elements = element_nodes_with_depth(dom, root, &nodes);
        Self::analyze_detected(dom, root, &nodes, &elements)
    }

    pub(crate) fn analyze_with_inventory(
        dom: &Dom,
        root: NodeId,
        candidates: &[NodeId],
        nodes: &[NodeId],
    ) -> Self {
        if candidates.is_empty() {
            Self::empty()
        } else {
            let elements = element_nodes_with_depth(dom, root, nodes);
            Self::analyze_detected(dom, root, nodes, &elements)
        }
    }

    fn empty() -> Self {
        Self {
            references: SparseNodeValues::new(),
            definitions: SparseNodeValues::new(),
            reference_slots: Vec::new(),
            definition_slots: Vec::new(),
            skipped: Vec::new(),
            deferred: Vec::new(),
            trim_start: Vec::new(),
            transparent: Vec::new(),
            available: HashSet::new(),
        }
    }

    fn analyze_detected(
        dom: &Dom,
        root: NodeId,
        nodes: &[NodeId],
        elements: &[(NodeId, u32)],
    ) -> Self {
        let definition_index = DefinitionIndex::analyze(dom, root, nodes, elements);
        let definitions =
            detect_definitions_with_index(dom, root, &definition_index, nodes, elements);
        let keys: HashSet<&str> = definitions
            .iter()
            .map(|definition| definition.key.as_str())
            .collect();
        let references = detect_references(dom, root, &keys, nodes);
        let mut labels = HashMap::<String, String>::new();
        let mut used_labels = HashSet::<String>::new();
        let mut generated_label = 1usize;
        for reference in &references {
            if !labels.contains_key(&reference.key) {
                let desired = reference.label.clone().unwrap_or_else(|| {
                    let label = if special_footnote_key(&reference.key) {
                        numeric_suffix(&reference.key)
                            .map(str::to_owned)
                            .unwrap_or_else(|| generated_label.to_string())
                    } else {
                        reference.key.clone()
                    };
                    generated_label += 1;
                    label
                });
                labels.insert(
                    reference.key.clone(),
                    reserve_label(desired, &mut used_labels),
                );
            }
        }
        for definition in &definitions {
            if !labels.contains_key(&definition.key) {
                labels.insert(
                    definition.key.clone(),
                    reserve_label(definition.key.clone(), &mut used_labels),
                );
            }
        }

        let mut selected = HashMap::<String, usize>::new();
        for (index, definition) in definitions.iter().enumerate() {
            selected
                .entry(definition.key.clone())
                .and_modify(|current| {
                    if definitions[*current].inline && !definition.inline {
                        *current = index;
                    }
                })
                .or_insert(index);
        }
        let reference_ids: HashSet<String> = references
            .iter()
            .flat_map(|reference| {
                std::iter::once(reference.node)
                    .chain(dom.descendants(reference.node))
                    .filter_map(|node| dom.attr(node, AttrName::Id).map(str::to_owned))
            })
            .collect();
        let mut semantic_references: SparseNodeValues<Box<str>> =
            SparseNodeValues::with_capacity(references.len());
        let mut semantic_definitions: SparseNodeValues<Box<str>> =
            SparseNodeValues::with_capacity(definitions.len());
        let mut selected_definitions = vec![false; dom.len()];
        let mut skipped = vec![false; dom.len()];
        let mut deferred = vec![false; dom.len()];
        for reference in &references {
            if let Some(label) = labels.get(&reference.key) {
                semantic_references.push(reference.node, label.clone().into());
            }
        }
        for (index, definition) in definitions.iter().enumerate() {
            if selected.get(&definition.key) != Some(&index) {
                skipped[definition.node.index()] = true;
                continue;
            }
            let Some(label) = labels.get(&definition.key) else {
                continue;
            };
            semantic_definitions.push(definition.node, label.clone().into());
            selected_definitions[definition.node.index()] = true;
            deferred[definition.node.index()] = definition.inline;
            mark_definition_chrome(dom, definition, label, &reference_ids, &mut skipped);
        }
        mark_container_chrome(dom, root, &definition_index, &mut skipped);
        mark_sidenote_controls(dom, root, &mut skipped);
        let mut transparent = vec![false; dom.len()];
        for definition in definitions
            .iter()
            .filter(|definition| selected_definitions[definition.node.index()])
        {
            for child in dom.element_children(definition.node) {
                if dom.tag(child) == Some(Tag::Div) && !selected_definitions[child.index()] {
                    transparent[child.index()] = true;
                }
            }
        }
        let mut trim_start = vec![false; dom.len()];
        for definition in definitions
            .iter()
            .filter(|definition| selected_definitions[definition.node.index()])
        {
            if let Some(text) = dom.descendants(definition.node).find(|&node| {
                dom.text_node(node).is_some()
                    && !dom
                        .ancestors(node)
                        .take_while(|&ancestor| ancestor != definition.node)
                        .any(|ancestor| skipped[ancestor.index()])
            }) {
                trim_start[text.index()] = true;
            }
        }
        semantic_references.sort();
        semantic_definitions.sort();
        let reference_slots = dense_slots(&semantic_references, dom.len());
        let definition_slots = dense_slots(&semantic_definitions, dom.len());

        let available = semantic_definitions
            .iter()
            .map(|(_, label)| label.to_string())
            .collect();
        Self {
            references: semantic_references,
            definitions: semantic_definitions,
            reference_slots,
            definition_slots,
            skipped,
            deferred,
            trim_start,
            transparent,
            available,
        }
    }

    pub(crate) fn reference(&self, node: NodeId) -> Option<&str> {
        sparse_slot(&self.reference_slots, node)
            .and_then(|slot| self.references.get_at(slot))
            .map(|label| label.as_ref())
    }

    pub(crate) fn definition(&self, node: NodeId) -> Option<&str> {
        sparse_slot(&self.definition_slots, node)
            .and_then(|slot| self.definitions.get_at(slot))
            .map(|label| label.as_ref())
    }

    pub(crate) fn is_skipped(&self, node: NodeId) -> bool {
        self.skipped.get(node.index()).copied().unwrap_or(false)
    }

    pub(crate) fn is_deferred(&self, node: NodeId) -> bool {
        self.deferred.get(node.index()).copied().unwrap_or(false)
    }

    pub(crate) fn should_trim_start(&self, node: NodeId) -> bool {
        self.trim_start.get(node.index()).copied().unwrap_or(false)
    }

    pub(crate) fn is_transparent(&self, node: NodeId) -> bool {
        self.transparent.get(node.index()).copied().unwrap_or(false)
    }

    pub(crate) fn has_definition(&self, label: &str) -> bool {
        self.available.contains(label)
    }

    // Used by the benchmark-only complex storage report.
    #[allow(dead_code)]
    pub(crate) fn storage_bytes(&self) -> usize {
        self.references
            .allocated_bytes()
            .saturating_add(self.definitions.allocated_bytes())
            .saturating_add(
                self.reference_slots
                    .capacity()
                    .saturating_add(self.definition_slots.capacity())
                    .saturating_mul(std::mem::size_of::<u32>()),
            )
            .saturating_add(
                self.skipped
                    .capacity()
                    .saturating_add(self.deferred.capacity())
                    .saturating_add(self.trim_start.capacity())
                    .saturating_add(self.transparent.capacity())
                    .saturating_mul(std::mem::size_of::<bool>()),
            )
    }
}

fn dense_slots<T>(values: &SparseNodeValues<T>, length: usize) -> Vec<u32> {
    let mut slots = vec![u32::MAX; length];
    for (slot, (node, _)) in values.iter().enumerate() {
        slots[node.index()] = slot as u32;
    }
    slots
}

fn sparse_slot(slots: &[u32], node: NodeId) -> Option<usize> {
    let slot = *slots.get(node.index())?;
    (slot != u32::MAX).then_some(slot as usize)
}

pub(crate) fn has_possible_footnote_evidence(dom: &Dom, node: NodeId) -> bool {
    let tag_evidence = match dom.tag(node) {
        Some(Tag::A) => {
            is_explicit_reference(dom, node)
                || fragment_target(dom.attr(node, AttrName::Href)).is_some()
        }
        Some(Tag::Label) => has_any_class(dom, node, &["footref", "sidenote-number"]),
        _ => false,
    };
    tag_evidence
        || is_source_evidence(dom, node)
        || is_footnote_container(dom, node)
        || dom
            .attr(node, AttrName::Id)
            .is_some_and(looks_like_footnote_id)
        || dom
            .attr_by_local_name(node, "data-type")
            .is_some_and(|value| {
                value.eq_ignore_ascii_case("footnote") || value.eq_ignore_ascii_case("noteref")
            })
}

fn detect_references(
    dom: &Dom,
    root: NodeId,
    definitions: &HashSet<&str>,
    nodes: &[NodeId],
) -> Vec<Reference> {
    let mut references = Vec::new();
    let starts_with_root = nodes.first() == Some(&root);
    for &node in nodes.iter().skip(usize::from(starts_with_root)) {
        if let Some(label) = dom.attr(node, AttrName::DataFootnoteRef) {
            references.push(Reference {
                node,
                key: label.to_owned(),
                label: Some(label.to_owned()),
            });
        } else if dom.tag(node) == Some(Tag::A) {
            let explicit = is_explicit_reference(dom, node);
            let key = fragment_target(dom.attr(node, AttrName::Href));
            let Some(key) = key.map(str::to_owned) else {
                continue;
            };
            if !explicit && !definitions.contains(key.as_str()) {
                continue;
            }
            let reference = dom
                .parent(node)
                .filter(|&parent| {
                    dom.tag(parent) == Some(Tag::Sup)
                        && dom
                            .descendants(parent)
                            .filter(|&descendant| dom.tag(descendant) == Some(Tag::A))
                            .take(2)
                            .count()
                            == 1
                })
                .unwrap_or(node);
            let dialect_label = reference_convention(dom, node)
                || dom.attr_by_local_name(node, "data-footnote-ref").is_some()
                || dom
                    .attr_by_local_name(node, "data-type")
                    .is_some_and(|value| value.eq_ignore_ascii_case("noteref"));
            let label = dialect_label
                .then(|| reference_label(dom, reference))
                .flatten();
            references.push(Reference {
                node: reference,
                key,
                label,
            });
        } else if dom.tag(node) == Some(Tag::Label)
            && has_any_class(dom, node, &["footref", "sidenote-number"])
            && let Some(key) = dom
                .attr_by_local_name(node, "for")
                .filter(|value| definitions.contains(*value))
        {
            references.push(Reference {
                node,
                key: key.to_owned(),
                label: reference_label(dom, node),
            });
        }
    }
    references
}

fn source_nodes(dom: &Dom, root: NodeId) -> Vec<NodeId> {
    std::iter::once(root).chain(dom.descendants(root)).collect()
}

fn element_nodes_with_depth(dom: &Dom, root: NodeId, nodes: &[NodeId]) -> Vec<(NodeId, u32)> {
    let mut elements = Vec::with_capacity(nodes.len() / 2);
    let mut ancestors = Vec::<(NodeId, u32)>::new();
    for &node in nodes {
        let parent = dom.parent(node);
        while ancestors
            .last()
            .is_some_and(|&(ancestor, _)| parent != Some(ancestor))
        {
            ancestors.pop();
        }
        let depth = ancestors
            .last()
            .map_or(0, |&(_, depth)| depth.saturating_add(1));
        if dom.is_element(node) {
            if node == root {
                debug_assert_eq!(depth, 0);
            }
            elements.push((node, depth));
        }
        ancestors.push((node, depth));
    }
    elements
}

struct DefinitionIndex {
    flags: Vec<u8>,
    first_descendant_id: Vec<NodeLink>,
}

const OWN_CANDIDATE: u8 = 1 << 0;
const NESTED_CANDIDATE: u8 = 1 << 1;
const CONTAINER: u8 = 1 << 2;
const INSIDE_CONTAINER: u8 = 1 << 3;
const DEFINITION_BACKLINK: u8 = 1 << 4;

impl DefinitionIndex {
    fn analyze(dom: &Dom, _root: NodeId, _nodes: &[NodeId], elements: &[(NodeId, u32)]) -> Self {
        let mut flags = vec![0_u8; dom.len()];
        for &(node, _) in elements {
            if matches!(
                dom.tag(node),
                Some(Tag::Div | Tag::Li | Tag::P | Tag::Aside)
            ) && dom
                .attr(node, AttrName::Id)
                .is_some_and(looks_like_footnote_id)
                && (has_role(dom, node, "doc-footnote")
                    || dom.attr_by_local_name(node, "data-footnote").is_some()
                    || matches!(dom.tag(node), Some(Tag::Li | Tag::P | Tag::Aside)))
            {
                flags[node.index()] |= OWN_CANDIDATE;
            }
        }
        let mut first_descendant_id = vec![NodeLink::NONE; dom.len()];
        for &(node, _) in elements.iter().rev() {
            let mut nested = false;
            let mut descendant_id = NodeLink::NONE;
            for child in dom.children(node) {
                nested |= flags[child.index()] & (OWN_CANDIDATE | NESTED_CANDIDATE) != 0;
                if descendant_id.get().is_none() {
                    descendant_id = if dom.attr(child, AttrName::Id).is_some() {
                        NodeLink::from_option(Some(child))
                    } else {
                        first_descendant_id[child.index()]
                    };
                }
            }
            if nested {
                flags[node.index()] |= NESTED_CANDIDATE;
            }
            first_descendant_id[node.index()] = descendant_id;
        }
        for &(node, _) in elements {
            let is_container = is_footnote_container(dom, node)
                || has_any_class(dom, node, &["references"])
                    && flags[node.index()] & NESTED_CANDIDATE != 0;
            if is_container {
                flags[node.index()] |= CONTAINER;
            }
            if is_container
                || dom
                    .parent(node)
                    .is_some_and(|parent| flags[parent.index()] & INSIDE_CONTAINER != 0)
            {
                flags[node.index()] |= INSIDE_CONTAINER;
            }
        }
        let mut preorder = vec![usize::MAX; dom.len()];
        let mut subtree_end = vec![elements.len(); dom.len()];
        let mut open = Vec::<(NodeId, u32)>::new();
        for (position, &(node, depth)) in elements.iter().enumerate() {
            while open
                .last()
                .is_some_and(|(_, open_depth)| *open_depth >= depth)
            {
                let (closed, _) = open.pop().unwrap();
                subtree_end[closed.index()] = position;
            }
            preorder[node.index()] = position;
            open.push((node, depth));
        }
        let mut targets = HashMap::<String, Vec<usize>>::new();
        for &(node, _) in elements {
            if dom.tag(node) == Some(Tag::A)
                && let Some(target) = href_fragment(dom.attr(node, AttrName::Href))
            {
                targets
                    .entry(target.to_ascii_lowercase())
                    .or_default()
                    .push(preorder[node.index()]);
            }
        }
        for &(node, _) in elements {
            let Some(id) = dom.attr(node, AttrName::Id) else {
                continue;
            };
            let start = preorder[node.index()] + 1;
            let end = subtree_end[node.index()];
            if conventional_backlink_targets(id).iter().any(|target| {
                targets.get(target).is_some_and(|positions| {
                    let index = positions.partition_point(|position| *position < start);
                    positions.get(index).is_some_and(|position| *position < end)
                })
            }) {
                flags[node.index()] |= DEFINITION_BACKLINK;
            }
        }
        Self {
            flags,
            first_descendant_id,
        }
    }

    fn contains(&self, node: NodeId, flag: u8) -> bool {
        self.flags[node.index()] & flag != 0
    }

    fn has_container_ancestor(&self, dom: &Dom, node: NodeId) -> bool {
        dom.parent(node)
            .is_some_and(|parent| self.contains(parent, INSIDE_CONTAINER))
    }
}

fn detect_definitions(dom: &Dom, root: NodeId) -> Vec<Definition> {
    let nodes = source_nodes(dom, root);
    let elements = element_nodes_with_depth(dom, root, &nodes);
    let index = DefinitionIndex::analyze(dom, root, &nodes, &elements);
    detect_definitions_with_index(dom, root, &index, &nodes, &elements)
}

fn detect_definitions_with_index(
    dom: &Dom,
    root: NodeId,
    index: &DefinitionIndex,
    nodes: &[NodeId],
    elements: &[(NodeId, u32)],
) -> Vec<Definition> {
    let mut potential_reference_targets = HashSet::new();
    let mut named_sidenote_targets = HashSet::new();
    for &node in nodes.iter().skip(usize::from(nodes.first() == Some(&root))) {
        if dom.tag(node) != Some(Tag::A) {
            continue;
        }
        if is_explicit_reference(dom, node)
            && let Some(target) = fragment_target(dom.attr(node, AttrName::Href))
        {
            potential_reference_targets.insert(target.to_owned());
        }
        if dom
            .parent(node)
            .is_some_and(|parent| dom.tag(parent) == Some(Tag::Sup))
            && let Some(target) = fragment_target(dom.attr(node, AttrName::Href))
        {
            named_sidenote_targets.insert(target.to_owned());
        }
    }
    let mut definitions: Vec<Definition> = Vec::new();
    let mut nearest_definition: Vec<Option<usize>> = vec![None; dom.len()];
    for &(node, _) in elements.iter().skip(usize::from(
        elements.first().is_some_and(|&(node, _)| node == root),
    )) {
        nearest_definition[node.index()] = dom
            .parent(node)
            .and_then(|parent| nearest_definition[parent.index()]);
        let sidenote = has_any_class(
            dom,
            node,
            &["sidenote", "side-note", "marginnote", "margin-note"],
        );
        let inline = sidenote && dom.attr(node, AttrName::Id).is_none();
        let named_sidenote = sidenote
            && dom
                .attr(node, AttrName::Id)
                .is_some_and(|id| named_sidenote_targets.contains(id));
        let structural = has_role(dom, node, "doc-footnote")
            || inline
            || named_sidenote
            || has_any_class(dom, node, &["footnote-definition", "footdef"])
            || dom.attr(node, AttrName::DataFootnote).is_some()
            || dom.attr_by_local_name(node, "data-footnote").is_some()
            || dom
                .attr_by_local_name(node, "data-type")
                .is_some_and(|value| value.eq_ignore_ascii_case("footnote"));
        let contained = (matches!(dom.tag(node), Some(Tag::Li | Tag::P | Tag::Aside))
            || dom.tag(node) == Some(Tag::Div)
                && index.contains(node, CONTAINER)
                && !index.contains(node, NESTED_CANDIDATE))
            && dom.attr(node, AttrName::Id).is_some_and(|id| {
                looks_like_footnote_id(id) || potential_reference_targets.contains(id)
            })
            && (index.contains(node, CONTAINER) || index.has_container_ancestor(dom, node));
        let conventional_id = dom
            .attr(node, AttrName::Id)
            .is_some_and(looks_like_footnote_id)
            && !index.contains(node, NESTED_CANDIDATE)
            && (index.has_container_ancestor(dom, node)
                || index.contains(node, DEFINITION_BACKLINK));
        let word_definition = word_definition_key(dom, node);
        if !inline && !structural && !contained && !conventional_id && word_definition.is_none() {
            continue;
        }
        let key = if inline {
            inline_sidenote_key(dom, node)
        } else {
            dom.attr(node, AttrName::Id)
                .map(str::to_owned)
                .or_else(|| {
                    dom.attr(node, AttrName::DataFootnote)
                        .filter(|value| !trim_text(value).is_empty())
                        .map(str::to_owned)
                })
                .or_else(|| {
                    dom.attr_by_local_name(node, "data-footnote")
                        .filter(|value| !trim_text(value).is_empty())
                        .map(str::to_owned)
                })
                .or_else(|| {
                    structural
                        .then(|| {
                            index.first_descendant_id[node.index()]
                                .get()
                                .and_then(|descendant| dom.attr(descendant, AttrName::Id))
                                .map(str::to_owned)
                        })
                        .flatten()
                        .or(word_definition)
                })
        };
        let Some(key) = key else { continue };
        if nearest_definition[node.index()].is_some() {
            continue;
        }
        nearest_definition[node.index()] = Some(definitions.len());
        definitions.push(Definition { node, key, inline });
    }
    definitions
}

fn reserve_label(desired: String, used: &mut HashSet<String>) -> String {
    if used.insert(desired.clone()) {
        return desired;
    }
    for suffix in 2.. {
        let candidate = format!("{desired}-{suffix}");
        if used.insert(candidate.clone()) {
            return candidate;
        }
    }
    unreachable!("an unbounded numeric suffix always finds a label")
}

fn mark_container_chrome(dom: &Dom, root: NodeId, index: &DefinitionIndex, skipped: &mut [bool]) {
    for (node, _) in dom.element_descendants_snapshot_with_depth(root) {
        if !index.contains(node, CONTAINER) {
            continue;
        }
        if let Some(heading) = dom.element_children(node).find(|&child| {
            matches!(
                dom.tag(child),
                Some(Tag::H1 | Tag::H2 | Tag::H3 | Tag::H4 | Tag::H5 | Tag::H6)
            )
        }) {
            skipped[heading.index()] = true;
        }
    }
}

fn mark_sidenote_controls(dom: &Dom, root: NodeId, skipped: &mut [bool]) {
    for node in dom.descendants(root) {
        if dom.tag(node) == Some(Tag::Input)
            && has_any_class(dom, node, &["footref-toggle", "margin-toggle"])
        {
            skipped[node.index()] = true;
        }
    }
}

pub(crate) struct Definitions(Vec<(String, NodeId)>);

pub(crate) fn collect_external(dom: &Dom) -> Definitions {
    let detected = detect_definitions(dom, dom.root());
    let definition_nodes: HashSet<NodeId> =
        detected.iter().map(|definition| definition.node).collect();
    Definitions(
        detected
            .iter()
            .filter(|definition| {
                !dom.ancestors(definition.node)
                    .any(|ancestor| definition_nodes.contains(&ancestor))
            })
            .map(|definition| (definition.key.clone(), definition.node))
            .collect(),
    )
}

pub(crate) fn adopt_external(
    definitions: &Definitions,
    source: &Dom,
    fragment: &mut Dom,
    fragment_root: NodeId,
) {
    let known: HashSet<&str> = definitions.0.iter().map(|(key, _)| key.as_str()).collect();
    let fragment_nodes = source_nodes(fragment, fragment_root);
    let referenced: Vec<String> =
        detect_references(fragment, fragment_root, &known, &fragment_nodes)
            .into_iter()
            .map(|reference| reference.key)
            .scan(HashSet::new(), |seen, key| {
                seen.insert(key.clone()).then_some(key)
            })
            .collect();
    if referenced.is_empty() {
        return;
    }
    let present: HashSet<String> = detect_definitions(fragment, fragment_root)
        .into_iter()
        .map(|definition| definition.key)
        .collect();
    let missing: Vec<(&str, NodeId)> = referenced
        .into_iter()
        .filter(|key| !present.contains(key))
        .filter_map(|key| {
            definitions
                .0
                .iter()
                .find(|(defined, _)| defined == &key)
                .map(|(defined, definition)| (defined.as_str(), *definition))
        })
        .collect();
    if missing.is_empty() {
        return;
    }
    let Ok(section) = fragment.create_html_element(Tag::Section) else {
        return;
    };
    fragment.set_attr(section, AttrName::DataFootnotes, "");
    for (key, definition) in missing {
        if let Ok(copy) = fragment.import_subtree(source, definition) {
            if fragment.tag(copy) == Some(Tag::Li) {
                fragment.rename_html(copy, Tag::Div);
            }
            if fragment.attr(copy, AttrName::Id).is_none()
                && has_any_class(
                    fragment,
                    copy,
                    &["sidenote", "side-note", "marginnote", "margin-note"],
                )
            {
                fragment.set_attr(copy, AttrName::Id, key);
            }
            fragment.append_child(section, copy);
        }
    }
    fragment.append_child(fragment_root, section);
}

fn word_definition_key(dom: &Dom, node: NodeId) -> Option<String> {
    if dom.tag(node) != Some(Tag::P) {
        return None;
    }
    let anchor = dom
        .descendants(node)
        .find(|&descendant| dom.tag(descendant) == Some(Tag::A))?;
    if std::iter::once(node)
        .chain(dom.descendants(node))
        .take_while(|&descendant| descendant != anchor)
        .any(|descendant| {
            dom.text_node(descendant)
                .is_some_and(|text| !trim_text(text).is_empty())
        })
    {
        return None;
    }
    let target = href_fragment(dom.attr(anchor, AttrName::Href))?;
    let lower = target.to_ascii_lowercase();
    let suffix = lower.strip_prefix("_ftnref")?;
    let word_marker = dom
        .attr_by_local_name(anchor, "name")
        .is_some_and(|name| name.eq_ignore_ascii_case(&format!("_ftn{suffix}")))
        || std::iter::once(anchor)
            .chain(
                dom.ancestors(anchor)
                    .take_while(|&ancestor| ancestor != node),
            )
            .any(|marker| {
                dom.tag(marker) == Some(Tag::Sup)
                    || has_any_class(dom, marker, &["msofootnotereference"])
            });
    word_marker.then(|| format!("_ftn{suffix}"))
}

fn inline_sidenote_key(dom: &Dom, node: NodeId) -> Option<String> {
    let previous = previous_significant_sibling(dom, node)?;
    if dom.tag(previous) == Some(Tag::Input)
        && has_any_class(dom, previous, &["footref-toggle", "margin-toggle"])
    {
        let key = dom.attr(previous, AttrName::Id)?;
        let label = previous_significant_sibling(dom, previous)?;
        return (dom.tag(label) == Some(Tag::Label)
            && has_any_class(dom, label, &["footref", "sidenote-number"])
            && dom.attr_by_local_name(label, "for") == Some(key))
        .then(|| key.to_owned());
    }
    std::iter::once(previous)
        .chain(dom.descendants(previous))
        .filter(|&candidate| dom.tag(candidate) == Some(Tag::A))
        .filter(|&anchor| is_explicit_reference(dom, anchor))
        .find_map(|anchor| fragment_target(dom.attr(anchor, AttrName::Href)).map(str::to_owned))
}

fn previous_significant_sibling(dom: &Dom, node: NodeId) -> Option<NodeId> {
    let mut previous = dom.prev_sibling(node);
    while let Some(candidate) = previous {
        if !dom
            .text_node(candidate)
            .is_some_and(|text| trim_text(text).is_empty())
        {
            return Some(candidate);
        }
        previous = dom.prev_sibling(candidate);
    }
    None
}

fn mark_definition_chrome(
    dom: &Dom,
    definition: &Definition,
    label: &str,
    reference_ids: &HashSet<String>,
    skipped: &mut [bool],
) {
    let mut previous = dom.prev_sibling(definition.node);
    while let Some(node) = previous {
        if dom.tag(node) == Some(Tag::Hr) {
            skipped[node.index()] = true;
            break;
        }
        if dom
            .text_node(node)
            .is_some_and(|text| trim_text(text).is_empty())
        {
            previous = dom.prev_sibling(node);
            continue;
        }
        break;
    }

    let definition_key = &definition.key;
    let definition_node = definition.node;
    let leading_marker = first_significant_child(dom, definition_node).filter(|&node| {
        let marker_node = if dom.tag(node) == Some(Tag::Sup) {
            Some(node)
        } else if dom.tag(node) == Some(Tag::P)
            && dom.element_children(node).count() == 1
            && dom.children(node).all(|child| {
                dom.tag(child) == Some(Tag::Sup)
                    || dom
                        .text_node(child)
                        .is_some_and(|text| trim_text(text).is_empty())
            })
        {
            dom.element_children(node).next()
        } else {
            None
        };
        let Some(marker_node) = marker_node else {
            return false;
        };
        let text = dom.text(marker_node);
        let marker = trim_text(&text).trim_matches(|character| matches!(character, '[' | ']'));
        !marker.is_empty()
            && marker.chars().count() <= 4
            && marker.chars().all(|character| character.is_ascii_digit())
            && (marker == label || numeric_suffix(definition_key) == Some(marker))
    });
    if let Some(marker) = leading_marker {
        skipped[marker.index()] = true;
    }

    let wrappers: SmallVec<[NodeId; 4]> = dom
        .descendants(definition_node)
        .filter(|&node| {
            has_any_class(
                dom,
                node,
                &[
                    "mw-cite-backlink",
                    "footnote-definition-label",
                    "sidenote-number",
                ],
            )
        })
        .collect();
    for wrapper in wrappers {
        skipped[wrapper.index()] = true;
    }

    let links: SmallVec<[NodeId; 4]> = dom
        .descendants(definition_node)
        .filter(|&node| dom.tag(node) == Some(Tag::A))
        .collect();
    for link in links {
        let text = dom.text(link);
        let label = trim_text(&text).to_ascii_lowercase();
        let href = dom
            .attr(link, AttrName::Href)
            .unwrap_or("")
            .to_ascii_lowercase();
        let aria = dom
            .attr_by_local_name(link, "aria-label")
            .unwrap_or("")
            .to_ascii_lowercase();
        let backlink = dom
            .attr(link, AttrName::Rel)
            .is_some_and(|value| has_token(value, "backlink"))
            || dom
                .attr_by_local_name(link, "data-footnote-backref")
                .is_some()
            || has_role(dom, link, "doc-backlink")
            || href_fragment(Some(&href)).is_some_and(|target| {
                reference_ids.contains(target)
                    || is_conventional_backlink_target(definition_key, target)
            })
            || aria.contains("footnote reference")
            || (href.starts_with('#')
                && (matches!(label.as_str(), "back" | "back to content" | "return")
                    || label
                        .chars()
                        .all(|ch| matches!(ch, '↩' | '↵' | '↑' | '^' | ' '))));
        if !backlink {
            continue;
        }
        skipped[link.index()] = true;
        if let Some(parent) = dom.parent(link)
            && dom.tag(parent) == Some(Tag::Sup)
            && dom.element_children(parent).all(|child| child == link)
            && dom.children(parent).all(|child| {
                child == link
                    || dom
                        .text_node(child)
                        .is_some_and(|text| trim_text(text).is_empty())
            })
        {
            skipped[parent.index()] = true;
        }
    }
}

fn first_significant_child(dom: &Dom, node: NodeId) -> Option<NodeId> {
    dom.children(node).find(|&child| {
        !dom.text_node(child)
            .is_some_and(|text| trim_text(text).is_empty())
    })
}

fn conventional_backlink_targets(definition_key: &str) -> SmallVec<[String; 3]> {
    let key = definition_key.to_ascii_lowercase();
    let mut targets = SmallVec::new();
    if let Some(suffix) = key.strip_prefix("user-content-fn-") {
        targets.push(format!("user-content-fnref-{suffix}"));
    } else if let Some(suffix) = key.strip_prefix("footnotedef") {
        targets.push(format!("footnoteref{suffix}"));
        targets.push(format!("footnote-ref{suffix}"));
        targets.push(format!("fnref{suffix}"));
    } else if let Some(suffix) = key.strip_prefix("_ftn") {
        targets.push(format!("_ftnref{suffix}"));
    } else if let Some(suffix) = key.strip_prefix("ftnt") {
        targets.push(format!("ftnt_ref{suffix}"));
    }
    targets
}

fn is_conventional_backlink_target(definition_key: &str, target: &str) -> bool {
    let target = target.to_ascii_lowercase();
    conventional_backlink_targets(definition_key)
        .iter()
        .any(|expected| expected == &target)
}

fn is_explicit_reference(dom: &Dom, anchor: NodeId) -> bool {
    has_role(dom, anchor, "doc-noteref")
        || dom.attr(anchor, AttrName::DataFootnoteRef).is_some()
        || dom
            .attr_by_local_name(anchor, "data-footnote-ref")
            .is_some()
        || dom
            .attr(anchor, AttrName::Rel)
            .is_some_and(|rel| has_token(rel, "footnote"))
        || dom
            .attr_by_local_name(anchor, "data-type")
            .is_some_and(|value| value.eq_ignore_ascii_case("noteref"))
        || reference_convention(dom, anchor)
}

fn reference_convention(dom: &Dom, anchor: NodeId) -> bool {
    let parent = dom.parent(anchor);
    parent.is_some_and(|parent| {
        has_any_class(
            dom,
            parent,
            &[
                "footnote-reference",
                "footnote-ref",
                "footnoteref",
                "fnref",
                "reference",
            ],
        ) || has_any_class(dom, parent, &["fn"])
            && dom.attr_by_local_name(parent, "data-fn").is_some()
            || dom.attr(parent, AttrName::Id).is_some_and(|id| {
                starts_with_ignore_case_any(
                    id,
                    &[
                        "ftnt_ref",
                        "user-content-fnref",
                        "footnoteref",
                        "footnote-ref",
                        "fnref",
                    ],
                )
            })
    }) || has_any_class(
        dom,
        anchor,
        &[
            "footnote-reference",
            "footnote-ref",
            "footnote-anchor",
            "footnote-link",
            "footnoteref",
            "fnref",
        ],
    ) || dom
        .attr_by_local_name(anchor, "data-footnote-ref")
        .is_some()
        || dom.attr(anchor, AttrName::Id).is_some_and(|id| {
            starts_with_ignore_case_any(
                id,
                &[
                    "cite_ref",
                    "user-content-fnref",
                    "footnoteref",
                    "footnote-ref",
                    "fnref",
                ],
            )
        })
}

/// ASCII case-insensitive `starts_with` against several prefixes.
/// Equivalent to lowercasing the value first, but without allocating.
fn starts_with_ignore_case_any(value: &str, prefixes: &[&str]) -> bool {
    let bytes = value.as_bytes();
    prefixes.iter().any(|prefix| {
        bytes
            .get(..prefix.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(prefix.as_bytes()))
    })
}

fn reference_label(dom: &Dom, node: NodeId) -> Option<String> {
    let text = dom.text(node);
    let label = trim_reference_label(&text);
    (!label.is_empty() && label.chars().count() <= 16).then(|| label.to_owned())
}

#[inline]
fn trim_reference_label(value: &str) -> &str {
    if value.is_ascii() {
        let bytes = value.as_bytes();
        let is_edge = |byte: u8| {
            byte.is_ascii_whitespace()
                || matches!(byte, b'[' | b']' | b'(' | b')' | b':' | b'.' | b'*')
        };
        let mut start = 0;
        while start < bytes.len() && is_edge(bytes[start]) {
            start += 1;
        }
        let mut end = bytes.len();
        while end > start && is_edge(bytes[end - 1]) {
            end -= 1;
        }
        &value[start..end]
    } else {
        value.trim_matches(|character: char| {
            character.is_whitespace()
                || matches!(character, '[' | ']' | '(' | ')' | ':' | '.' | '†' | '*')
        })
    }
}

fn is_footnote_container(dom: &Dom, node: NodeId) -> bool {
    dom.attr(node, AttrName::DataFootnotes).is_some()
        || dom.attr_by_local_name(node, "data-footnotes").is_some()
        || has_role(dom, node, "doc-endnotes")
        || dom
            .attr_by_local_name(node, "data-type")
            .is_some_and(|value| value.eq_ignore_ascii_case("footnotes"))
        || [AttrName::Class, AttrName::Id]
            .into_iter()
            .filter_map(|name| dom.attr(node, name))
            .any(|value| any_token(value, is_footnote_container_token))
}

fn is_footnote_container_token(part: &str) -> bool {
    // Most class tokens are unrelated. Matching by length avoids comparing
    // them with every known footnote spelling.
    match part.len() {
        8 => part.eq_ignore_ascii_case("endnotes"),
        9 => part.eq_ignore_ascii_case("footnotes"),
        13 => part.eq_ignore_ascii_case("footnote-list"),
        18 => {
            part.eq_ignore_ascii_case("footnote-container")
                || part.eq_ignore_ascii_case("wp-block-footnotes")
        }
        20 => part.eq_ignore_ascii_case("footnote-definitions"),
        19 => part.eq_ignore_ascii_case("footnotes-container"),
        _ => false,
    }
}

fn fragment_target(href: Option<&str>) -> Option<&str> {
    let href = href?;
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

fn href_fragment(href: Option<&str>) -> Option<&str> {
    href?
        .rsplit_once('#')
        .map(|(_, target)| target)
        .filter(|target| !target.is_empty())
}

fn looks_like_footnote_id(value: &str) -> bool {
    let value = value.to_ascii_lowercase();
    value.starts_with("fn")
        || value.starts_with("_ftn")
        || value.starts_with("ftnt")
        || value.starts_with("footnote")
        || value.starts_with("note-")
        || value.starts_with("sn")
        || value.starts_with("sidenote")
        || value.starts_with("cite_note")
        || value.starts_with("user-content-fn")
        || value.starts_with("footnotedef")
}

fn special_footnote_key(value: &str) -> bool {
    let value = value.to_ascii_lowercase();
    value.starts_with("_ftn")
        || value.starts_with("ftnt")
        || value.starts_with("cite_note")
        || value.starts_with("user-content-fn")
        || value.starts_with("footnotedef")
        || value.chars().all(|character| character.is_ascii_digit())
        || value.len() >= 24 && value.contains('-')
}

fn numeric_suffix(value: &str) -> Option<&str> {
    let start = value
        .char_indices()
        .rev()
        .take_while(|(_, character)| character.is_ascii_digit())
        .last()
        .map(|(index, _)| index)?;
    Some(&value[start..])
}

fn has_any_class(dom: &Dom, node: NodeId, expected: &[&str]) -> bool {
    dom.attr(node, AttrName::Class)
        .is_some_and(|classes| has_any_token(classes, expected))
}

fn has_role(dom: &Dom, node: NodeId, role: &str) -> bool {
    dom.attr(node, AttrName::Role)
        .is_some_and(|value| has_token(value, role))
}

#[inline]
fn any_token(value: &str, mut predicate: impl FnMut(&str) -> bool) -> bool {
    if value.is_ascii() {
        value.split_ascii_whitespace().any(&mut predicate)
    } else {
        value.split_whitespace().any(predicate)
    }
}

pub(crate) fn is_source_evidence(dom: &Dom, node: NodeId) -> bool {
    is_explicit_reference(dom, node)
        || has_role(dom, node, "doc-footnote")
        || has_role(dom, node, "doc-endnotes")
        || dom.attr_by_local_name(node, "data-footnote").is_some()
        || dom.attr_by_local_name(node, "data-footnotes").is_some()
        || has_any_class(dom, node, &["fn"]) && dom.attr_by_local_name(node, "data-fn").is_some()
        || has_source_evidence_class(dom, node)
}

fn has_source_evidence_class(dom: &Dom, node: NodeId) -> bool {
    dom.attr(node, AttrName::Class)
        .is_some_and(|classes| any_token(classes, is_source_evidence_class_token))
}

fn is_source_evidence_class_token(class: &str) -> bool {
    let Some(first) = class.as_bytes().first().copied() else {
        return false;
    };
    match first.to_ascii_lowercase() {
        b'e' => class.eq_ignore_ascii_case("endnotes"),
        b'f' => [
            "footnote-reference",
            "footnote-ref",
            "footnoteref",
            "fnref",
            "footnote-definition",
            "footdef",
            "footref",
            "footref-toggle",
            "footnotes",
            "footnote-list",
            "footnote-definitions",
            "footnote-container",
            "footnotes-container",
        ]
        .iter()
        .any(|expected| class.eq_ignore_ascii_case(expected)),
        b'm' => ["marginnote", "margin-note", "margin-toggle"]
            .iter()
            .any(|expected| class.eq_ignore_ascii_case(expected)),
        b's' => ["sidenote", "side-note", "sidenote-number"]
            .iter()
            .any(|expected| class.eq_ignore_ascii_case(expected)),
        b'w' => class.eq_ignore_ascii_case("wp-block-footnotes"),
        _ => false,
    }
}

pub(crate) fn class_is_semantic_evidence(dom: &Dom, node: NodeId) -> bool {
    is_source_evidence(dom, node)
        || has_any_class(
            dom,
            node,
            &[
                "reference",
                "references",
                "fn",
                "footnote-backref",
                "footnote-body",
                "reference-text",
                "mw-cite-backlink",
            ],
        )
}
