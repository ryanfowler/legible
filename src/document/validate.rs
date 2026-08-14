use std::collections::{HashMap, HashSet};

use super::{Document, DocumentNodeId, FootnoteId, NodeKind, ValidationError};

pub(super) fn validate(document: &Document) -> Result<(), ValidationError> {
    let node_count = document.nodes.len();
    let mut seen = vec![false; node_count];
    let mut claimed_parent = vec![None; node_count];
    let mut is_root = vec![false; node_count];
    let mut stack: Vec<(DocumentNodeId, Option<DocumentNodeId>, bool)> =
        Vec::with_capacity(document.roots.len());

    for &root in &document.roots {
        ensure_id(root, node_count)?;
        if std::mem::replace(&mut is_root[root.index()], true) {
            return Err(ValidationError::new("document root is duplicated"));
        }
        if document.nodes[root.index()].next_sibling.is_some() {
            return Err(ValidationError::new("semantic root has a sibling link"));
        }
        if !valid_root(&document.nodes[root.index()].kind) {
            return Err(ValidationError::new(format!(
                "semantic root {:#?} requires a structural parent",
                document.nodes[root.index()].kind
            )));
        }
    }
    validate_adjacent_text(document, document.roots.iter().copied())?;
    stack.extend(document.roots.iter().rev().map(|&root| (root, None, false)));

    while let Some((id, parent, inside_link)) = stack.pop() {
        ensure_id(id, node_count)?;
        if seen[id.index()] {
            return Err(ValidationError::new(
                "semantic node has multiple parents or a cycle",
            ));
        }
        seen[id.index()] = true;

        let node = &document.nodes[id.index()];
        validate_kind(node.kind(), node.first_child)?;
        if inside_link && matches!(node.kind, NodeKind::Link(_)) {
            return Err(ValidationError::new("semantic links cannot be nested"));
        }
        if let Some(parent) = parent
            && !valid_child(
                &document.nodes[parent.index()].kind,
                &document.nodes[id.index()].kind,
            )
        {
            return Err(ValidationError::new(format!(
                "semantic parent {:#?} cannot contain {:#?}",
                document.nodes[parent.index()].kind,
                document.nodes[id.index()].kind
            )));
        }

        let mut children = Vec::new();
        let mut child = node.first_child;
        let mut sibling_steps = 0usize;
        while let Some(current) = child {
            ensure_id(current, node_count)?;
            sibling_steps += 1;
            if sibling_steps > node_count {
                return Err(ValidationError::new(
                    "semantic sibling chain contains a cycle",
                ));
            }
            if is_root[current.index()] || claimed_parent[current.index()].replace(id).is_some() {
                return Err(ValidationError::new(
                    "semantic node has multiple parents or a sibling cycle",
                ));
            }
            children.push(current);
            child = document.nodes[current.index()].next_sibling;
        }
        validate_adjacent_text(document, children.iter().copied())?;
        validate_child_order(document, node.kind(), &children)?;
        let children_inside_link = inside_link || matches!(node.kind, NodeKind::Link(_));
        stack.extend(
            children
                .into_iter()
                .rev()
                .map(|child| (child, Some(id), children_inside_link)),
        );
    }

    if seen.iter().any(|seen| !seen) {
        return Err(ValidationError::new(
            "semantic arena contains an unreachable node",
        ));
    }
    validate_tables(document)?;
    validate_footnotes(document)
}

fn ensure_id(id: DocumentNodeId, node_count: usize) -> Result<(), ValidationError> {
    if id.index() >= node_count {
        return Err(ValidationError::new(
            "semantic link points outside the arena",
        ));
    }
    Ok(())
}

fn validate_kind(
    kind: &NodeKind,
    first_child: Option<DocumentNodeId>,
) -> Result<(), ValidationError> {
    match kind {
        NodeKind::Text(value) if value.is_empty() => {
            return Err(ValidationError::new("semantic text node is empty"));
        }
        NodeKind::Text(value)
            if super::text::normalize_prose_fragment(value).as_str() != value.as_str() =>
        {
            return Err(ValidationError::new("semantic text is not canonical prose"));
        }
        NodeKind::Heading { level } if !(1..=6).contains(level) => {
            return Err(ValidationError::new(
                "semantic heading level is outside 1 through 6",
            ));
        }
        NodeKind::TableCell(cell) if cell.colspan == 0 || cell.rowspan == 0 => {
            return Err(ValidationError::new(
                "semantic table cell span is less than one",
            ));
        }
        NodeKind::Link(link)
            if !valid_destination(&link.destination, super::uri::DestinationKind::Link) =>
        {
            return Err(ValidationError::new(
                "semantic link has an unsafe destination",
            ));
        }
        NodeKind::Image(image)
            if !valid_destination(&image.source, super::uri::DestinationKind::Resource) =>
        {
            return Err(ValidationError::new(
                "semantic image has an unsafe destination",
            ));
        }
        NodeKind::Media(media)
            if !valid_destination(&media.source, super::uri::DestinationKind::Resource) =>
        {
            return Err(ValidationError::new(
                "semantic media has an unsafe destination",
            ));
        }
        NodeKind::CodeBlock(_)
        | NodeKind::InlineCode(_)
        | NodeKind::Image(_)
        | NodeKind::HardBreak
        | NodeKind::FootnoteReference(_)
        | NodeKind::TaskMarker(_)
        | NodeKind::InlineMath(_)
        | NodeKind::DisplayMath(_)
        | NodeKind::Media(_)
            if first_child.is_some() =>
        {
            return Err(ValidationError::new("semantic leaf node has children"));
        }
        _ => {}
    }
    Ok(())
}

fn valid_destination(value: &str, kind: super::uri::DestinationKind) -> bool {
    super::uri::safe_destination(value, None, kind).as_deref() == Some(value)
}

fn valid_root(kind: &NodeKind) -> bool {
    !matches!(
        kind,
        NodeKind::ListItem
            | NodeKind::TableCaption
            | NodeKind::TableRow
            | NodeKind::TableCell(_)
            | NodeKind::Figcaption
            | NodeKind::Summary
            | NodeKind::DefinitionTerm
            | NodeKind::DefinitionDescription
    )
}

fn valid_child(parent: &NodeKind, child: &NodeKind) -> bool {
    match parent {
        NodeKind::List(_) => matches!(child, NodeKind::ListItem | NodeKind::FootnoteDefinition(_)),
        NodeKind::Table(_) => matches!(child, NodeKind::TableCaption | NodeKind::TableRow),
        NodeKind::TableRow => matches!(child, NodeKind::TableCell(_)),
        NodeKind::DefinitionList => matches!(
            child,
            NodeKind::DefinitionTerm | NodeKind::DefinitionDescription
        ),
        NodeKind::Paragraph => is_inline(child) || matches!(child, NodeKind::DisplayMath(_)),
        NodeKind::Heading { .. }
        | NodeKind::TableCaption
        | NodeKind::DefinitionTerm
        | NodeKind::Emphasis
        | NodeKind::Strong
        | NodeKind::Strikethrough
        | NodeKind::Link(_) => is_inline(child) && !nested_link(parent, child),
        NodeKind::Text(_)
        | NodeKind::CodeBlock(_)
        | NodeKind::InlineCode(_)
        | NodeKind::Image(_)
        | NodeKind::HardBreak
        | NodeKind::FootnoteReference(_)
        | NodeKind::TaskMarker(_)
        | NodeKind::InlineMath(_)
        | NodeKind::DisplayMath(_)
        | NodeKind::Media(_)
        | NodeKind::ThematicBreak => false,
        NodeKind::Details => !requires_special_parent(child) || matches!(child, NodeKind::Summary),
        NodeKind::Figure => {
            !requires_special_parent(child) || matches!(child, NodeKind::Figcaption)
        }
        NodeKind::BlockGroup
        | NodeKind::Summary
        | NodeKind::BlockQuote
        | NodeKind::Figcaption
        | NodeKind::ListItem
        | NodeKind::TableCell(_)
        | NodeKind::DefinitionDescription
        | NodeKind::Callout(_)
        | NodeKind::FootnoteDefinition(_) => !requires_special_parent(child),
    }
}

fn is_inline(kind: &NodeKind) -> bool {
    matches!(
        kind,
        NodeKind::Text(_)
            | NodeKind::Emphasis
            | NodeKind::Strong
            | NodeKind::Strikethrough
            | NodeKind::InlineCode(_)
            | NodeKind::Link(_)
            | NodeKind::Image(_)
            | NodeKind::HardBreak
            | NodeKind::FootnoteReference(_)
            | NodeKind::TaskMarker(_)
            | NodeKind::InlineMath(_)
            | NodeKind::Media(_)
    )
}

fn nested_link(parent: &NodeKind, child: &NodeKind) -> bool {
    matches!((parent, child), (NodeKind::Link(_), NodeKind::Link(_)))
}

fn requires_special_parent(kind: &NodeKind) -> bool {
    matches!(
        kind,
        NodeKind::ListItem
            | NodeKind::TableCaption
            | NodeKind::TableRow
            | NodeKind::TableCell(_)
            | NodeKind::Figcaption
            | NodeKind::Summary
            | NodeKind::DefinitionTerm
            | NodeKind::DefinitionDescription
    )
}

fn validate_child_order(
    document: &Document,
    parent: &NodeKind,
    children: &[DocumentNodeId],
) -> Result<(), ValidationError> {
    let positions = |matches: fn(&NodeKind) -> bool| {
        children
            .iter()
            .enumerate()
            .filter_map(|(index, id)| matches(&document.nodes[id.index()].kind).then_some(index))
            .collect::<Vec<_>>()
    };
    match parent {
        NodeKind::Table(_) => {
            let captions = positions(|kind| matches!(kind, NodeKind::TableCaption));
            if captions.len() > 1 || captions.first().is_some_and(|&index| index != 0) {
                return Err(ValidationError::new(
                    "semantic table caption must be unique and first",
                ));
            }
        }
        NodeKind::Details => {
            let summaries = positions(|kind| matches!(kind, NodeKind::Summary));
            if summaries.len() > 1 || summaries.first().is_some_and(|&index| index != 0) {
                return Err(ValidationError::new(
                    "semantic details summary must be unique and first",
                ));
            }
        }
        NodeKind::Figure => {
            let captions = positions(|kind| matches!(kind, NodeKind::Figcaption));
            if captions.len() > 1
                || captions
                    .first()
                    .is_some_and(|&index| index != 0 && index + 1 != children.len())
            {
                return Err(ValidationError::new(
                    "semantic figure caption must be unique and first or last",
                ));
            }
        }
        _ => {}
    }
    Ok(())
}

fn validate_adjacent_text(
    document: &Document,
    nodes: impl Iterator<Item = DocumentNodeId>,
) -> Result<(), ValidationError> {
    let mut nodes = nodes.peekable();
    let mut previous = None;
    while let Some(id) = nodes.next() {
        ensure_id(id, document.nodes.len())?;
        let kind = &document.nodes[id.index()].kind;
        if previous.is_some_and(|id: DocumentNodeId| {
            matches!(document.nodes[id.index()].kind, NodeKind::Text(_))
        }) && matches!(kind, NodeKind::Text(_))
        {
            return Err(ValidationError::new(format!(
                "adjacent semantic text nodes were not merged: {:#?} then {:#?}",
                previous.map(|id: DocumentNodeId| &document.nodes[id.index()].kind),
                kind
            )));
        }
        if let NodeKind::Text(value) = kind
            && value.chars().all(char::is_whitespace)
        {
            let bounded_by_inline = previous
                .map(|id: DocumentNodeId| is_inline(&document.nodes[id.index()].kind))
                .unwrap_or(false)
                && nodes
                    .peek()
                    .map(|id| is_inline(&document.nodes[id.index()].kind))
                    .unwrap_or(false);
            if value.as_str() != " " || !bounded_by_inline {
                return Err(ValidationError::new(format!(
                    "semantic whitespace text is not an inline separator between {:#?} and {:#?}",
                    previous.map(|id: DocumentNodeId| &document.nodes[id.index()].kind),
                    nodes.peek().map(|id| &document.nodes[id.index()].kind)
                )));
            }
        }
        previous = Some(id);
    }
    Ok(())
}

fn validate_tables(document: &Document) -> Result<(), ValidationError> {
    for (index, node) in document.nodes.iter().enumerate() {
        let NodeKind::Table(table) = &node.kind else {
            continue;
        };
        let mut maximum_width = 0u32;
        let mut has_rowspan = false;
        for child in document.children(DocumentNodeId(index as u32)) {
            if !matches!(document.nodes[child.index()].kind, NodeKind::TableRow) {
                continue;
            }
            let mut width = 0u32;
            for cell in document.children(child) {
                let NodeKind::TableCell(cell) = &document.nodes[cell.index()].kind else {
                    continue;
                };
                width = width.checked_add(cell.colspan).ok_or_else(|| {
                    ValidationError::new("semantic table spans exceed column capacity")
                })?;
                has_rowspan |= cell.rowspan > 1;
            }
            maximum_width = maximum_width.max(width);
        }
        let expected = (!has_rowspan).then_some(maximum_width);
        if table.column_count != expected {
            return Err(ValidationError::new(
                "semantic table column count does not match its spans",
            ));
        }
    }
    Ok(())
}

fn validate_footnotes(document: &Document) -> Result<(), ValidationError> {
    let mut definition_nodes = HashMap::<FootnoteId, DocumentNodeId>::new();
    let mut references = HashSet::<FootnoteId>::new();
    for (index, node) in document.nodes.iter().enumerate() {
        match node.kind {
            NodeKind::FootnoteReference(id) => {
                references.insert(id);
            }
            NodeKind::FootnoteDefinition(id)
                if definition_nodes
                    .insert(id, DocumentNodeId(index as u32))
                    .is_some() =>
            {
                return Err(ValidationError::new(
                    "footnote has duplicate definition nodes",
                ));
            }
            NodeKind::FootnoteDefinition(_) => {}
            _ => {}
        }
    }

    let mut indexed_ids = HashSet::new();
    let mut labels = HashSet::new();
    for definition in &document.footnotes {
        ensure_id(definition.node, document.nodes.len())?;
        if !indexed_ids.insert(definition.id) || !labels.insert(definition.label.as_ref()) {
            return Err(ValidationError::new(
                "footnote index has duplicate IDs or labels",
            ));
        }
        if definition_nodes.get(&definition.id) != Some(&definition.node) {
            return Err(ValidationError::new(
                "footnote index does not point to its definition node",
            ));
        }
    }
    if definition_nodes.keys().any(|id| !indexed_ids.contains(id)) {
        return Err(ValidationError::new(
            "footnote definition is missing from the index",
        ));
    }
    if references.iter().any(|id| !indexed_ids.contains(id)) {
        return Err(ValidationError::new(
            "footnote reference does not resolve to a definition",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::document::{
        DocumentBuilder, DocumentNodeId, FootnoteId, Link, List, ListKind, NodeKind,
    };

    #[test]
    fn rejects_invalid_structure() {
        let mut builder = DocumentBuilder::with_capacity(1);
        builder.append(None, NodeKind::ListItem).unwrap();
        assert!(builder.finish().validate().is_err());

        let mut builder = DocumentBuilder::with_capacity(2);
        let list = builder
            .append(
                None,
                NodeKind::List(List {
                    kind: ListKind::Unordered,
                    start: None,
                }),
            )
            .unwrap();
        builder.append(Some(list), NodeKind::ListItem).unwrap();
        assert!(builder.finish().validate().is_ok());
    }

    #[test]
    fn rejects_invalid_roots_without_panicking() {
        let mut builder = DocumentBuilder::with_capacity(1);
        builder.append(None, NodeKind::Paragraph).unwrap();
        let mut document = builder.finish();
        document.roots.push(DocumentNodeId(u32::MAX));
        assert!(document.validate().is_err());
    }

    #[test]
    fn rejects_indirectly_nested_links() {
        let mut builder = DocumentBuilder::with_capacity(3);
        let outer = builder
            .append(
                None,
                NodeKind::Link(Link {
                    destination: "https://example.test/outer".into(),
                    title: None,
                }),
            )
            .unwrap();
        let emphasis = builder.append(Some(outer), NodeKind::Emphasis).unwrap();
        builder
            .append(
                Some(emphasis),
                NodeKind::Link(Link {
                    destination: "https://example.test/inner".into(),
                    title: None,
                }),
            )
            .unwrap();
        assert!(builder.finish().validate().is_err());
    }

    #[test]
    fn rejects_unresolved_footnotes() {
        let mut builder = DocumentBuilder::with_capacity(1);
        builder
            .append(
                None,
                NodeKind::FootnoteReference(FootnoteId::from_index(0).unwrap()),
            )
            .unwrap();
        assert!(builder.finish().validate().is_err());
    }
}
