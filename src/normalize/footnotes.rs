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

pub(super) fn normalize(dom: &mut Dom, root: NodeId) {
    let definitions = detect_definitions(dom, root);
    let keys: HashSet<&str> = definitions
        .iter()
        .map(|definition| definition.key.as_str())
        .collect();
    let references = detect_references(dom, root, &keys);
    canonicalize(dom, root, references, definitions);
}

fn detect_references(dom: &Dom, root: NodeId, definitions: &HashSet<&str>) -> Vec<Reference> {
    let mut references = Vec::new();
    for node in dom.descendants(root) {
        if dom.tag(node) == Some(Tag::A) {
            let Some(key) = fragment_target(dom.attr(node, AttrName::Href)).map(str::to_owned)
            else {
                continue;
            };
            let explicit = is_explicit_reference(dom, node);
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

fn detect_definitions(dom: &Dom, root: NodeId) -> Vec<Definition> {
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
    let mut definitions = Vec::new();
    for (node, _) in dom.element_descendants_snapshot_with_depth(root) {
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
            || dom.attr_by_local_name(node, "data-footnote").is_some()
            || dom
                .attr_by_local_name(node, "data-type")
                .is_some_and(|value| value.eq_ignore_ascii_case("footnote"));
        let contained = matches!(dom.tag(node), Some(Tag::Li | Tag::P | Tag::Aside))
            && dom.attr(node, AttrName::Id).is_some_and(|id| {
                looks_like_footnote_id(id) || potential_reference_targets.contains(id)
            })
            && dom
                .ancestors(node)
                .any(|ancestor| is_footnote_container(dom, ancestor));
        let conventional_id = dom
            .attr(node, AttrName::Id)
            .is_some_and(looks_like_footnote_id)
            && !has_nested_definition_candidate(dom, node)
            && (dom
                .ancestors(node)
                .any(|ancestor| is_footnote_container(dom, ancestor))
                || has_definition_backlink(dom, node));
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
                    dom.attr_by_local_name(node, "data-footnote")
                        .filter(|value| !value.trim().is_empty())
                        .map(str::to_owned)
                })
                .or_else(|| {
                    structural
                        .then(|| nested_definition_key(dom, node))
                        .flatten()
                        .or(word_definition)
                })
        };
        let Some(key) = key else { continue };
        if definitions.iter().any(|definition: &Definition| {
            definition.key == key
                && dom
                    .ancestors(node)
                    .any(|ancestor| ancestor == definition.node)
        }) {
            continue;
        }
        definitions.push(Definition { node, key, inline });
    }
    definitions
}

fn has_nested_definition_candidate(dom: &Dom, node: NodeId) -> bool {
    dom.descendants(node).any(|descendant| {
        matches!(
            dom.tag(descendant),
            Some(Tag::Div | Tag::Li | Tag::P | Tag::Aside)
        ) && dom
            .attr(descendant, AttrName::Id)
            .is_some_and(looks_like_footnote_id)
            && (has_role(dom, descendant, "doc-footnote")
                || dom
                    .attr_by_local_name(descendant, "data-footnote")
                    .is_some()
                || matches!(dom.tag(descendant), Some(Tag::Li | Tag::P | Tag::Aside)))
    })
}

fn canonicalize(
    dom: &mut Dom,
    root: NodeId,
    references: Vec<Reference>,
    definitions: Vec<Definition>,
) {
    let reference_ids: HashSet<String> = references
        .iter()
        .flat_map(|reference| {
            std::iter::once(reference.node)
                .chain(dom.descendants(reference.node))
                .filter_map(|node| dom.attr(node, AttrName::Id).map(str::to_owned))
        })
        .collect();
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
            let label = reserve_label(desired, &mut used_labels);
            labels.insert(reference.key.clone(), label);
        }
        dom.set_attr(
            reference.node,
            AttrName::DataFootnoteRef,
            &labels[&reference.key],
        );
    }
    for definition in &definitions {
        if !labels.contains_key(&definition.key) {
            let label = reserve_label(definition.key.clone(), &mut used_labels);
            labels.insert(definition.key.clone(), label);
        }
    }

    // Prefer a separate definition over a duplicate inline sidenote. The separate
    // definition usually has cleaner prose and valid block structure.
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

    let matched_sidenote_keys: HashSet<String> = definitions
        .iter()
        .filter(|definition| definition.inline && labels.contains_key(&definition.key))
        .map(|definition| definition.key.clone())
        .collect();
    let mut inline_definitions = Vec::new();
    for (index, definition) in definitions.iter().enumerate() {
        if dom.parent(definition.node).is_none() {
            continue;
        }
        if selected.get(&definition.key) != Some(&index) {
            dom.detach(definition.node);
            continue;
        }
        let label = &labels[&definition.key];
        dom.set_attr(definition.node, AttrName::DataFootnote, label);
        remove_preceding_separator(dom, definition.node);
        remove_definition_markers(dom, definition.node, &definition.key, label, &reference_ids);
        if definition.inline {
            inline_definitions.push(definition.node);
        }
    }

    if !inline_definitions.is_empty()
        && let Ok(section) = dom.create_html_element(Tag::Section)
    {
        dom.set_attr(section, AttrName::DataFootnotes, "");
        for definition in inline_definitions {
            if dom.parent(definition).is_some() {
                dom.append_child(section, definition);
            }
        }
        dom.append_child(root, section);
    }

    remove_sidenote_controls(dom, root, &matched_sidenote_keys);
    mark_containers(dom, root);
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

fn mark_containers(dom: &mut Dom, root: NodeId) {
    let nodes = dom.element_descendants_snapshot_with_depth(root);
    for (node, _) in nodes {
        if dom.parent(node).is_none() || !is_footnote_container(dom, node) {
            continue;
        }
        dom.set_attr(node, AttrName::DataFootnotes, "");
        if matches!(dom.tag(node), Some(Tag::Div | Tag::Aside)) {
            dom.rename_html(node, Tag::Section);
        }
        if let Some(heading) = dom.element_children(node).find(|&child| {
            matches!(
                dom.tag(child),
                Some(Tag::H1 | Tag::H2 | Tag::H3 | Tag::H4 | Tag::H5 | Tag::H6)
            )
        }) {
            dom.detach(heading);
        }
    }
}

fn remove_sidenote_controls(dom: &mut Dom, root: NodeId, matched_keys: &HashSet<String>) {
    let controls: SmallVec<[NodeId; 8]> = dom
        .descendants(root)
        .filter(|&node| {
            dom.tag(node) == Some(Tag::Input)
                && has_any_class(dom, node, &["footref-toggle", "margin-toggle"])
                && dom
                    .attr(node, AttrName::Id)
                    .is_some_and(|id| matched_keys.contains(id))
        })
        .collect();
    for control in controls {
        dom.detach(control);
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

fn nested_definition_key(dom: &Dom, node: NodeId) -> Option<String> {
    dom.descendants(node)
        .find_map(|descendant| dom.attr(descendant, AttrName::Id).map(str::to_owned))
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

fn remove_preceding_separator(dom: &mut Dom, definition: NodeId) {
    let mut previous = dom.prev_sibling(definition);
    while let Some(node) = previous {
        if dom.tag(node) == Some(Tag::Hr) {
            dom.detach(node);
            return;
        }
        if dom
            .text_node(node)
            .is_some_and(|text| text.trim().is_empty())
        {
            previous = dom.prev_sibling(node);
            continue;
        }
        return;
    }
}

fn remove_definition_markers(
    dom: &mut Dom,
    definition: NodeId,
    definition_key: &str,
    label: &str,
    reference_ids: &HashSet<String>,
) {
    let leading_sup = first_significant_child(dom, definition).filter(|&node| {
        if dom.tag(node) != Some(Tag::Sup) {
            return false;
        }
        let text = dom.text(node);
        let marker = text
            .trim()
            .trim_matches(|character| matches!(character, '[' | ']'));
        !marker.is_empty()
            && marker.chars().count() <= 4
            && marker.chars().all(|character| character.is_ascii_digit())
            && (marker == label || numeric_suffix(definition_key) == Some(marker))
    });
    if let Some(marker) = leading_sup {
        dom.detach(marker);
    }

    let wrappers: SmallVec<[NodeId; 4]> = dom
        .descendants(definition)
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
        if dom.parent(wrapper).is_some() {
            dom.detach(wrapper);
        }
    }

    let links: SmallVec<[NodeId; 4]> = dom
        .descendants(definition)
        .filter(|&node| dom.tag(node) == Some(Tag::A))
        .collect();
    for link in links {
        if dom.parent(link).is_none() {
            continue;
        }
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
        let parent = dom.parent(link);
        dom.detach(link);
        if let Some(parent) = parent
            && dom.parent(parent).is_some()
            && dom.tag(parent) == Some(Tag::Sup)
            && !dom.has_non_whitespace_text(parent)
        {
            dom.detach(parent);
        }
    }

    if let Some(text_node) = std::iter::once(definition)
        .chain(dom.descendants(definition))
        .find(|&node| dom.text_node(node).is_some())
        && let Some(trimmed) = dom.text_node(text_node).map(str::trim_start)
        && trimmed.len() != dom.text_node(text_node).unwrap_or("").len()
    {
        let trimmed = trimmed.to_owned();
        dom.set_text(text_node, &trimmed);
    }
}

fn first_significant_child(dom: &Dom, node: NodeId) -> Option<NodeId> {
    dom.children(node).find(|&child| {
        !dom.text_node(child)
            .is_some_and(|text| text.trim().is_empty())
    })
}

fn is_conventional_backlink_target(definition_key: &str, target: &str) -> bool {
    let key = definition_key.to_ascii_lowercase();
    let target = target.to_ascii_lowercase();
    if let Some(suffix) = key.strip_prefix("user-content-fn-") {
        return target == format!("user-content-fnref-{suffix}");
    }
    if let Some(suffix) = key.strip_prefix("footnotedef") {
        return target == format!("footnoteref{suffix}")
            || target == format!("footnote-ref{suffix}")
            || target == format!("fnref{suffix}");
    }
    if let Some(suffix) = key.strip_prefix("_ftn") {
        return target == format!("_ftnref{suffix}");
    }
    if let Some(suffix) = key.strip_prefix("ftnt") {
        return target == format!("ftnt_ref{suffix}");
    }
    false
}

fn has_definition_backlink(dom: &Dom, node: NodeId) -> bool {
    let Some(id) = dom.attr(node, AttrName::Id) else {
        return false;
    };
    let id = id.to_ascii_lowercase();
    dom.descendants(node)
        .filter(|&descendant| dom.tag(descendant) == Some(Tag::A))
        .filter_map(|anchor| href_fragment(dom.attr(anchor, AttrName::Href)))
        .any(|target| is_conventional_backlink_target(&id, target))
}

fn is_explicit_reference(dom: &Dom, anchor: NodeId) -> bool {
    has_role(dom, anchor, "doc-noteref")
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
            &["footnote-reference", "footnote-ref", "footnoteref", "fnref"],
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
                    ) || part.eq_ignore_ascii_case("references")
                        && has_footnote_definitions(dom, node)
                })
            })
}

fn has_footnote_definitions(dom: &Dom, node: NodeId) -> bool {
    dom.descendants(node).any(|descendant| {
        dom.attr(descendant, AttrName::DataFootnote).is_some()
            || dom
                .attr_by_local_name(descendant, "data-footnote")
                .is_some()
            || has_role(dom, descendant, "doc-footnote")
            || dom
                .attr(descendant, AttrName::Id)
                .is_some_and(looks_like_footnote_id)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown::dom_to_markdown;

    fn markdown(html: &str) -> String {
        let mut dom = Dom::parse_fragment(html, Tag::Div).unwrap();
        let root = dom.root();
        normalize(&mut dom, root);
        dom_to_markdown(&dom, root, 0)
    }

    #[test]
    fn normalizes_repeated_references_links_and_backlinks() {
        assert_eq!(
            markdown(
                r##"<p>One<sup><a href="#fn1">1</a></sup> and again <a role="doc-noteref" href="#fn1">1</a>.</p><section class="footnotes"><h2>Notes</h2><ol><li id="fn1" role="doc-footnote"><p>See <a href="https://example.test">source</a>. <a href="#ref" rel="backlink">Back</a></p><p>More detail.</p></li></ol></section>"##
            ),
            "One[^fn1] and again [^fn1].\n\n[^fn1]: See [source](https://example.test).\n\n    More detail.\n"
        );
    }

    #[test]
    fn supports_google_docs_and_word_exports() {
        assert_eq!(
            markdown(
                r##"<p>Google note<sup id="ftnt_ref1"><a href="#ftnt1">[1]</a></sup>.</p><p id="ftnt1"><a href="#ftnt_ref1">[1]</a> Google content.</p><p>Word note<sup><a href="#_ftn2">[2]</a></sup>.</p><p><span class="MsoFootnoteReference"><a name="_ftn2" href="//GUID#_ftnref2">[2]</a></span> Word content.</p>"##
            ),
            "Google note[^1].\n\n[^1]: Google content.\n\nWord note[^2].\n\n[^2]: Word content.\n"
        );
    }

    #[test]
    fn supports_static_site_generator_definitions() {
        assert_eq!(
            markdown(
                r##"<p>Text<sup class="footnote-reference"><a href="#1">1</a></sup>.</p><div class="footnote-definition" id="1"><sup class="footnote-definition-label">1</sup><p>Generated note.</p></div>"##
            ),
            "Text[^1].\n\n[^1]: Generated note.\n"
        );
    }

    #[test]
    fn supports_htmlbook_relative_noterefs() {
        assert_eq!(
            markdown(
                r##"<p>Text<sup><a data-type="noteref" href="chapter.html#fn1">1</a></sup>.</p><div data-type="footnotes"><p data-type="footnote" id="fn1"><sup><a href="chapter.html#fn1-marker">1</a></sup>Book note.</p></div>"##
            ),
            "Text[^1].\n\n[^1]: Book note.\n"
        );
    }

    #[test]
    fn moves_an_inline_sidenote_and_keeps_normal_asides() {
        assert_eq!(
            markdown(
                r##"<p>Text<label class="footref" for="fn.1">1</label><input id="fn.1" class="footref-toggle" type="checkbox"><span class="sidenote"><sup>1</sup> Side note.</span> More.</p><aside><p>Related prose.</p></aside>"##
            ),
            "Text[^1] More.\n\nRelated prose.\n\n[^1]: Side note.\n"
        );
    }

    #[test]
    fn keeps_definition_wrappers_links_and_unmatched_sidenotes_safe() {
        let output = markdown(
            r##"<p>One<a role="doc-noteref" href="#fn1">1</a> and two<a role="doc-noteref" href="#fn2">2</a>, plus three<a role="doc-noteref" href="#fn3">3</a>.</p><section class="footnotes"><p id="intro">Notes introduction.</p><div class="definitions-wrapper"><div id="fn1" role="doc-footnote">First note with <a href="#reference-data">reference data</a>.</div><div id="fn2" role="doc-footnote">Second note uses x<sup>2</sup>.</div><div id="fn3" role="doc-footnote"><sup>2</sup> H<sup>2</sup> stays meaningful.</div></div></section><p>An <a href="#details">ordinary link</a><em>intervening prose</em><span class="sidenote">unmatched annotation</span>.</p><p>See <a href="appendix.html#_ftnref7">the appendix reference</a>, <a href="mailto:user@example.test#fn1">email</a>, and <a href="urn:example#fn99">URN data</a>.</p><p id="note-1">See <a href="#reference-1">numbered reference</a>.</p><p><label class="footref" for="missing">Unmatched label</label></p><aside id="orphan" class="sidenote">Unmatched named sidenote.</aside><aside id="details">Details remain ordinary content.</aside>"##,
        );
        assert!(output.contains("[^fn1]: First note with [reference data](#reference-data)."));
        assert!(output.contains("[^fn2]: Second note uses x2."));
        assert!(output.contains("[^fn3]: 2 H2 stays meaningful."));
        assert!(output.contains("Notes introduction."));
        assert!(!output.contains("[^intro]"));
        assert!(
            output.contains("[ordinary link](#details) *intervening prose* unmatched annotation")
        );
        assert!(output.contains("[the appendix reference](appendix.html#_ftnref7)"));
        assert!(output.contains("[email](mailto:user@example.test#fn1)"));
        assert!(output.contains("URN data"));
        assert!(!output.contains("[^fn99]"));
        assert!(output.contains("[numbered reference](#reference-1)"));
        assert!(!output.contains("[^_ftn7]"));
        assert!(!output.contains("[^note-1]"));
        assert!(output.contains("Unmatched label"));
        assert!(output.contains("Unmatched named sidenote."));
        assert!(!output.contains("[^missing]"));
        assert!(!output.contains("[^orphan]"));
        assert!(output.contains("Details remain ordinary content."));
    }

    #[test]
    fn keeps_unmatched_bibliography_links_and_allocates_unique_labels() {
        let output = markdown(
            r##"<p>A citation<a role="doc-biblioref" href="#book">[Book]</a> and <span class="fn"><a href="#details">see details</a></span>.</p><p>Notes<a href="#_ftn1">[1]</a>, <sup class="reference"><a href="#cite_note-1">[1]</a></sup>, <sup><a rel="footnote" href="#group-a">A</a>, <a rel="footnote" href="#group-b">B</a></sup>, and <a rel="footnote" href="#arbitrary">another note</a>.</p><p><sup><a href="//GUID#_ftnref1">[1]</a></sup>Word note.</p><ol class="references"><li id="cite_note-1">Wiki note.</li><li id="group-a">Grouped A.</li><li id="group-b">Grouped B.</li><li id="arbitrary">Explicit arbitrary note.</li></ol><p id="details">Ordinary details.</p>"##,
        );
        assert!(output.contains(r"[\[Book\]](#book)"), "{output}");
        assert!(output.contains("[see details](#details)"), "{output}");
        assert!(
            output.contains("Notes[^1], [^1-2], [^group-a], [^group-b], and [^arbitrary]."),
            "{output}"
        );
        assert!(output.contains("[^1]: Word note."), "{output}");
        assert!(output.contains("[^1-2]: Wiki note."), "{output}");
        assert!(output.contains("[^group-a]: Grouped A."), "{output}");
        assert!(output.contains("[^group-b]: Grouped B."), "{output}");
        assert!(
            output.contains("[^arbitrary]: Explicit arbitrary note."),
            "{output}"
        );
    }

    #[test]
    fn adopts_only_referenced_external_definitions() {
        let source = Dom::parse_fragment(r##"<article><p>Text<sup><a href="#fn1">1</a></sup> and again <sup><a href="#fn1">1</a></sup>.</p></article><footer class="footnotes"><div id="fn1" role="doc-footnote">Kept note.</div><div id="fn2" role="doc-footnote">Unused note.</div></footer>"##, Tag::Div).unwrap();
        let selected = source
            .first_descendant_by_tag(source.root(), Tag::Article)
            .unwrap();
        let definitions = collect_external(&source);
        let mut fragment = source.copy_subtree_as_fragment(selected).unwrap();
        let root = fragment.root();
        adopt_external(&definitions, &mut fragment, root);
        normalize(&mut fragment, root);
        let markdown = dom_to_markdown(&fragment, root, 0);
        assert!(markdown.contains("[^fn1]: Kept note."), "{markdown}");
        assert_eq!(markdown.matches("[^fn1]:").count(), 1, "{markdown}");
        assert!(!markdown.contains("Unused note"), "{markdown}");
    }

    #[test]
    fn keeps_valid_list_ancestry_and_copies_only_outer_definitions() {
        let source = Dom::parse_fragment(r#"<section class="footnotes"><ol><li id="fn1" role="doc-footnote">Outer <span id="fn-inner" role="doc-footnote">nested marker</span></li></ol></section>"#, Tag::Div).unwrap();
        let definitions = collect_external(&source);
        assert_eq!(definitions.0.len(), 1);
        let mut dom = source;
        let root = dom.root();
        normalize(&mut dom, root);
        let html = crate::dom::render_html(&dom, root, 0);
        assert!(html.contains("<ol><li"), "{html}");
        assert!(!html.contains("<ol><div"), "{html}");
    }

    #[test]
    fn keeps_a_missing_inferred_reference_as_a_link() {
        assert_eq!(
            markdown(r##"<p>Text<sup><a href="#fn404">404</a></sup>.</p>"##),
            "Text[404](#fn404).\n"
        );
    }

    #[test]
    fn keeps_an_ordinary_references_heading() {
        assert_eq!(
            markdown(
                r#"<section id="references"><h2>References</h2><p>Smith, Example Book.</p></section>"#
            ),
            "## References\n\nSmith, Example Book.\n"
        );
    }

    #[test]
    fn keeps_an_unmatched_footnotedef_link_as_an_ordinary_link() {
        assert_eq!(
            markdown(r##"<p>Read the <a href="#footnotedef-overview">ordinary overview</a>.</p>"##),
            "Read the [ordinary overview](#footnotedef-overview).\n"
        );
    }
}
