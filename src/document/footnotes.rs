use crate::dom::{AttrName, Dom, NodeId, Tag};
use smallvec::SmallVec;
use std::collections::{HashMap, HashSet};

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
    references: Vec<Option<Box<str>>>,
    definitions: Vec<Option<Box<str>>>,
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
            return Self {
                references: Vec::new(),
                definitions: Vec::new(),
                skipped: Vec::new(),
                deferred: Vec::new(),
                trim_start: Vec::new(),
                transparent: Vec::new(),
                available: HashSet::new(),
            };
        }
        let definition_index = DefinitionIndex::analyze(dom, root);
        let definitions = detect_definitions_with_index(dom, root, &definition_index);
        let keys: HashSet<&str> = definitions
            .iter()
            .map(|definition| definition.key.as_str())
            .collect();
        let references = detect_references(dom, root, &keys);
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
        let mut semantic_references: Vec<Option<Box<str>>> = (0..dom.len()).map(|_| None).collect();
        let mut semantic_definitions: Vec<Option<Box<str>>> =
            (0..dom.len()).map(|_| None).collect();
        let mut skipped = vec![false; dom.len()];
        let mut deferred = vec![false; dom.len()];
        for reference in references {
            if let Some(label) = labels.get(&reference.key) {
                semantic_references[reference.node.index()] = Some(label.clone().into());
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
            semantic_definitions[definition.node.index()] = Some(label.clone().into());
            deferred[definition.node.index()] = definition.inline;
            mark_definition_chrome(dom, definition, label, &reference_ids, &mut skipped);
        }
        mark_container_chrome(dom, root, &definition_index, &mut skipped);
        mark_sidenote_controls(dom, root, &mut skipped);
        let mut transparent = vec![false; dom.len()];
        for definition in definitions
            .iter()
            .filter(|definition| semantic_definitions[definition.node.index()].is_some())
        {
            for child in dom.element_children(definition.node) {
                if dom.tag(child) == Some(Tag::Div) && semantic_definitions[child.index()].is_none()
                {
                    transparent[child.index()] = true;
                }
            }
        }
        let mut trim_start = vec![false; dom.len()];
        for definition in definitions
            .iter()
            .filter(|definition| semantic_definitions[definition.node.index()].is_some())
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
        let available = semantic_definitions
            .iter()
            .flatten()
            .map(|label| label.to_string())
            .collect();
        Self {
            references: semantic_references,
            definitions: semantic_definitions,
            skipped,
            deferred,
            trim_start,
            transparent,
            available,
        }
    }

    pub(crate) fn reference(&self, node: NodeId) -> Option<&str> {
        self.references.get(node.index()).and_then(Option::as_deref)
    }

    pub(crate) fn definition(&self, node: NodeId) -> Option<&str> {
        self.definitions
            .get(node.index())
            .and_then(Option::as_deref)
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
}

fn has_possible_footnote_evidence(dom: &Dom, node: NodeId) -> bool {
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

fn detect_references(dom: &Dom, root: NodeId, definitions: &HashSet<&str>) -> Vec<Reference> {
    let mut references = Vec::new();
    for node in dom.descendants(root) {
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

struct DefinitionIndex {
    container: Vec<bool>,
    inside_container: Vec<bool>,
    nested_candidate: Vec<bool>,
    definition_backlink: Vec<bool>,
    first_descendant_id: Vec<Option<NodeId>>,
}

impl DefinitionIndex {
    fn analyze(dom: &Dom, root: NodeId) -> Self {
        let nodes = std::iter::once(root)
            .chain(dom.descendants(root))
            .collect::<Vec<_>>();
        let mut own_candidate = vec![false; dom.len()];
        for &node in &nodes {
            own_candidate[node.index()] = matches!(
                dom.tag(node),
                Some(Tag::Div | Tag::Li | Tag::P | Tag::Aside)
            ) && dom
                .attr(node, AttrName::Id)
                .is_some_and(looks_like_footnote_id)
                && (has_role(dom, node, "doc-footnote")
                    || dom.attr_by_local_name(node, "data-footnote").is_some()
                    || matches!(dom.tag(node), Some(Tag::Li | Tag::P | Tag::Aside)));
        }
        let mut nested_candidate = vec![false; dom.len()];
        let mut first_descendant_id = vec![None; dom.len()];
        for &node in nodes.iter().rev() {
            nested_candidate[node.index()] = dom
                .children(node)
                .any(|child| own_candidate[child.index()] || nested_candidate[child.index()]);
            first_descendant_id[node.index()] = dom.children(node).find_map(|child| {
                dom.attr(child, AttrName::Id)
                    .map(|_| child)
                    .or(first_descendant_id[child.index()])
            });
        }
        let mut container = vec![false; dom.len()];
        let mut inside_container = vec![false; dom.len()];
        for &node in &nodes {
            container[node.index()] = is_footnote_container(dom, node)
                || has_any_class(dom, node, &["references"]) && nested_candidate[node.index()];
            inside_container[node.index()] = container[node.index()]
                || dom
                    .parent(node)
                    .is_some_and(|parent| inside_container[parent.index()]);
        }
        let elements = std::iter::once((root, 0))
            .chain(dom.element_descendants_snapshot_with_depth(root))
            .collect::<Vec<_>>();
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
        for &(node, _) in &elements {
            if dom.tag(node) == Some(Tag::A)
                && let Some(target) = href_fragment(dom.attr(node, AttrName::Href))
            {
                targets
                    .entry(target.to_ascii_lowercase())
                    .or_default()
                    .push(preorder[node.index()]);
            }
        }
        let mut definition_backlink = vec![false; dom.len()];
        for &(node, _) in &elements {
            let Some(id) = dom.attr(node, AttrName::Id) else {
                continue;
            };
            let start = preorder[node.index()] + 1;
            let end = subtree_end[node.index()];
            definition_backlink[node.index()] =
                conventional_backlink_targets(id).iter().any(|target| {
                    targets.get(target).is_some_and(|positions| {
                        let index = positions.partition_point(|position| *position < start);
                        positions.get(index).is_some_and(|position| *position < end)
                    })
                });
        }
        Self {
            container,
            inside_container,
            nested_candidate,
            definition_backlink,
            first_descendant_id,
        }
    }

    fn has_container_ancestor(&self, dom: &Dom, node: NodeId) -> bool {
        dom.parent(node)
            .is_some_and(|parent| self.inside_container[parent.index()])
    }
}

fn detect_definitions(dom: &Dom, root: NodeId) -> Vec<Definition> {
    let index = DefinitionIndex::analyze(dom, root);
    detect_definitions_with_index(dom, root, &index)
}

fn detect_definitions_with_index(
    dom: &Dom,
    root: NodeId,
    index: &DefinitionIndex,
) -> Vec<Definition> {
    let potential_reference_targets: HashSet<String> = dom
        .descendants(root)
        .filter(|&node| dom.tag(node) == Some(Tag::A))
        .filter(|&node| is_explicit_reference(dom, node))
        .filter_map(|node| fragment_target(dom.attr(node, AttrName::Href)).map(str::to_owned))
        .collect();
    let named_sidenote_targets: HashSet<String> = dom
        .descendants(root)
        .filter(|&node| dom.tag(node) == Some(Tag::A))
        .filter(|&node| {
            dom.parent(node)
                .is_some_and(|parent| dom.tag(parent) == Some(Tag::Sup))
        })
        .filter_map(|node| fragment_target(dom.attr(node, AttrName::Href)).map(str::to_owned))
        .collect();
    let mut definitions: Vec<Definition> = Vec::new();
    let mut nearest_definition: Vec<Option<usize>> = vec![None; dom.len()];
    for (node, _) in dom.element_descendants_snapshot_with_depth(root) {
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
                && index.container[node.index()]
                && !index.nested_candidate[node.index()])
            && dom.attr(node, AttrName::Id).is_some_and(|id| {
                looks_like_footnote_id(id) || potential_reference_targets.contains(id)
            })
            && (index.container[node.index()] || index.has_container_ancestor(dom, node));
        let conventional_id = dom
            .attr(node, AttrName::Id)
            .is_some_and(looks_like_footnote_id)
            && !index.nested_candidate[node.index()]
            && (index.has_container_ancestor(dom, node) || index.definition_backlink[node.index()]);
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
                        .filter(|value| !value.trim().is_empty())
                        .map(str::to_owned)
                })
                .or_else(|| {
                    dom.attr_by_local_name(node, "data-footnote")
                        .filter(|value| !value.trim().is_empty())
                        .map(str::to_owned)
                })
                .or_else(|| {
                    structural
                        .then(|| {
                            index.first_descendant_id[node.index()]
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
        if !index.container[node.index()] {
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

pub(crate) struct Definitions(Vec<(String, Dom)>);

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
            .filter_map(|definition| {
                dom.copy_subtree_as_fragment(definition.node)
                    .ok()
                    .map(|copy| (definition.key.clone(), copy))
            })
            .collect(),
    )
}

pub(crate) fn adopt_external(definitions: &Definitions, fragment: &mut Dom, fragment_root: NodeId) {
    let known: HashSet<&str> = definitions.0.iter().map(|(key, _)| key.as_str()).collect();
    let referenced: Vec<String> = detect_references(fragment, fragment_root, &known)
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
    let missing: Vec<(&str, &Dom)> = referenced
        .into_iter()
        .filter(|key| !present.contains(key))
        .filter_map(|key| {
            definitions
                .0
                .iter()
                .find(|(defined, _)| defined == &key)
                .map(|(defined, definition)| (defined.as_str(), definition))
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
        let Some(definition_root) = definition.first_child(definition.root()) else {
            continue;
        };
        if let Ok(copy) = fragment.import_subtree(definition, definition_root) {
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
                .is_some_and(|text| !text.trim().is_empty())
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
            .is_some_and(|text| text.trim().is_empty())
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
            .is_some_and(|text| text.trim().is_empty())
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
                        .is_some_and(|text| text.trim().is_empty())
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
        let marker = text
            .trim()
            .trim_matches(|character| matches!(character, '[' | ']'));
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
        let label = text.trim().to_ascii_lowercase();
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
            .is_some_and(|value| token(value, "backlink"))
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
                        .is_some_and(|text| text.trim().is_empty())
            })
        {
            skipped[parent.index()] = true;
        }
    }
}

fn first_significant_child(dom: &Dom, node: NodeId) -> Option<NodeId> {
    dom.children(node).find(|&child| {
        !dom.text_node(child)
            .is_some_and(|text| text.trim().is_empty())
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
            .is_some_and(|rel| token(rel, "footnote"))
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
                let id = id.to_ascii_lowercase();
                id.starts_with("ftnt_ref")
                    || id.starts_with("user-content-fnref")
                    || id.starts_with("footnoteref")
                    || id.starts_with("footnote-ref")
                    || id.starts_with("fnref")
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
            let id = id.to_ascii_lowercase();
            id.starts_with("cite_ref")
                || id.starts_with("user-content-fnref")
                || id.starts_with("footnoteref")
                || id.starts_with("footnote-ref")
                || id.starts_with("fnref")
        })
}

fn reference_label(dom: &Dom, node: NodeId) -> Option<String> {
    let text = dom.text(node);
    let label = text
        .trim()
        .trim_matches(|character: char| {
            character.is_whitespace()
                || matches!(character, '[' | ']' | '(' | ')' | ':' | '.' | '†' | '*')
        })
        .trim();
    (!label.is_empty() && label.chars().count() <= 16).then(|| label.to_owned())
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
            .any(|value| {
                value.split_whitespace().any(|part| {
                    matches!(
                        part.to_ascii_lowercase().as_str(),
                        "footnotes"
                            | "footnote-list"
                            | "footnote-definitions"
                            | "footnote-container"
                            | "footnotes-container"
                            | "wp-block-footnotes"
                            | "endnotes"
                    )
                })
            })
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
    dom.attr(node, AttrName::Class).is_some_and(|classes| {
        classes.split_whitespace().any(|class| {
            expected
                .iter()
                .any(|value| class.eq_ignore_ascii_case(value))
        })
    })
}

fn has_role(dom: &Dom, node: NodeId, role: &str) -> bool {
    dom.attr(node, AttrName::Role)
        .is_some_and(|value| token(value, role))
}

fn token(value: &str, expected: &str) -> bool {
    value
        .split_whitespace()
        .any(|value| value.eq_ignore_ascii_case(expected))
}

pub(crate) fn is_local_reference(dom: &Dom, node: NodeId, href: &str) -> bool {
    dom.tag(node) == Some(Tag::A)
        && is_explicit_reference(dom, node)
        && fragment_target(Some(href)).is_some()
}

pub(crate) fn is_source_evidence(dom: &Dom, node: NodeId) -> bool {
    is_explicit_reference(dom, node)
        || has_role(dom, node, "doc-footnote")
        || has_role(dom, node, "doc-endnotes")
        || dom.attr_by_local_name(node, "data-footnote").is_some()
        || dom.attr_by_local_name(node, "data-footnotes").is_some()
        || has_any_class(dom, node, &["fn"]) && dom.attr_by_local_name(node, "data-fn").is_some()
        || has_any_class(
            dom,
            node,
            &[
                "footnote-reference",
                "footnote-ref",
                "footnoteref",
                "fnref",
                "footnote-definition",
                "footdef",
                "sidenote",
                "side-note",
                "marginnote",
                "margin-note",
                "footref",
                "sidenote-number",
                "footref-toggle",
                "margin-toggle",
                "footnotes",
                "footnote-list",
                "footnote-definitions",
                "footnote-container",
                "footnotes-container",
                "wp-block-footnotes",
                "endnotes",
            ],
        )
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
