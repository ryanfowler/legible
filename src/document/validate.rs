use std::collections::{HashMap, HashSet};

use super::{
    Document, DocumentNodeId, FootnoteId, NodeKindView as NodeKind, OP_CLOSE, OperationKind,
    ValidationError,
};

pub(super) fn validate(document: &Document) -> Result<(), ValidationError> {
    validate_text_arena(document)?;
    validate_tape(document)?;

    let mut seen = vec![false; document.ops.len()];
    let mut roots_seen = HashSet::new();
    let mut stack: Vec<(DocumentNodeId, Option<DocumentNodeId>, bool)> = Vec::new();
    for &root in &document.roots {
        ensure_id(document, root)?;
        if !roots_seen.insert(root) {
            return Err(ValidationError::new("document root is duplicated"));
        }
        let kind = document
            .node(root)
            .ok_or_else(|| ValidationError::new("invalid root"))?
            .kind();
        if !valid_root(kind) {
            return Err(ValidationError::new(format!(
                "semantic root {kind:#?} requires a structural parent"
            )));
        }
        stack.push((root, None, false));
    }

    while let Some((id, parent, inside_link)) = stack.pop() {
        ensure_id(document, id)?;
        if std::mem::replace(&mut seen[id.index()], true) {
            return Err(ValidationError::new(
                "semantic node has multiple parents or a cycle",
            ));
        }
        let kind = document
            .node(id)
            .ok_or_else(|| ValidationError::new("semantic node is not an opening operation"))?
            .kind();
        validate_kind(kind)?;
        if inside_link && matches!(kind, NodeKind::Link(_)) {
            return Err(ValidationError::new("semantic links cannot be nested"));
        }
        if let Some(parent) = parent {
            let parent_kind = document.node(parent).unwrap().kind();
            if !valid_child(parent_kind, kind) {
                return Err(ValidationError::new(format!(
                    "semantic parent {parent_kind:#?} cannot contain {kind:#?}"
                )));
            }
        }

        let children: Vec<_> = document.child_ids(id).collect();
        validate_adjacent_text(document, children.iter().copied())?;
        validate_child_order(document, kind, &children)?;
        let children_inside_link = inside_link || matches!(kind, NodeKind::Link(_));
        stack.extend(
            children
                .into_iter()
                .rev()
                .map(|child| (child, Some(id), children_inside_link)),
        );
    }

    let open_count = document
        .ops
        .iter()
        .filter(|operation| operation.opcode & OP_CLOSE == 0)
        .count();
    if seen.iter().filter(|value| **value).count() != open_count {
        return Err(ValidationError::new(
            "semantic tape contains an unreachable node",
        ));
    }
    validate_tables(document)?;
    validate_footnotes(document)
}

fn validate_text_arena(document: &Document) -> Result<(), ValidationError> {
    for reference in &document.text_refs {
        if document.text.get(reference.range()).is_none() {
            return Err(ValidationError::new(
                "semantic text range is outside the text arena",
            ));
        }
    }
    Ok(())
}

fn validate_tape(document: &Document) -> Result<(), ValidationError> {
    if document.ops.len() != document.ends.len() {
        return Err(ValidationError::new("semantic tape index length mismatch"));
    }
    let mut open_stack = Vec::new();
    for (index, operation) in document.ops.iter().copied().enumerate() {
        let index = u32::try_from(index).map_err(|_| ValidationError::new("tape is too large"))?;
        let Some(kind) = OperationKind::from_opcode(operation.opcode) else {
            return Err(ValidationError::new(
                "semantic tape contains an unknown operation",
            ));
        };
        if operation.is_close() {
            let Some(open) = open_stack.pop() else {
                return Err(ValidationError::new(
                    "semantic tape closes without an opener",
                ));
            };
            if operation.payload != open || document.ends[open as usize] != index {
                return Err(ValidationError::new(
                    "semantic tape close operation is misplaced",
                ));
            }
            if operation.opcode & !OP_CLOSE != document.ops[open as usize].opcode {
                return Err(ValidationError::new(
                    "semantic tape close kind does not match opener",
                ));
            }
        } else if matches!(kind, OperationKind::Heading) && !(1..=6).contains(&operation.aux) {
            return Err(ValidationError::new(
                "semantic heading level is outside 1 through 6",
            ));
        } else if kind.is_container() {
            open_stack.push(index);
            if document.ends[index as usize] <= index {
                return Err(ValidationError::new(
                    "container has no valid close operation",
                ));
            }
        } else if document.ends[index as usize] != index {
            return Err(ValidationError::new("leaf has a close-operation index"));
        }
    }
    if !open_stack.is_empty() {
        return Err(ValidationError::new(
            "semantic tape has an unclosed container",
        ));
    }
    Ok(())
}

fn ensure_id(document: &Document, id: DocumentNodeId) -> Result<(), ValidationError> {
    if document.node(id).is_none() {
        return Err(ValidationError::new(
            "semantic link points outside the tape",
        ));
    }
    Ok(())
}

fn validate_kind(kind: NodeKind<'_>) -> Result<(), ValidationError> {
    match kind {
        NodeKind::Invalid => return Err(ValidationError::new("semantic payload is missing")),
        NodeKind::Text("") => {
            return Err(ValidationError::new("semantic text node is empty"));
        }
        NodeKind::Text(value) if super::text::normalize_prose_fragment(value).as_ref() != value => {
            return Err(ValidationError::new("semantic text is not canonical prose"));
        }
        NodeKind::Heading { level } if !(1..=6).contains(&level) => {
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
        _ => {}
    }
    Ok(())
}

fn valid_destination(value: &str, kind: super::uri::DestinationKind) -> bool {
    super::uri::safe_destination(value, None, kind).as_deref() == Some(value)
}

fn valid_root(kind: NodeKind<'_>) -> bool {
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

fn valid_child(parent: NodeKind<'_>, child: NodeKind<'_>) -> bool {
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
        | NodeKind::ThematicBreak
        | NodeKind::Invalid => false,
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

fn is_inline(kind: NodeKind<'_>) -> bool {
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

fn nested_link(parent: NodeKind<'_>, child: NodeKind<'_>) -> bool {
    matches!((parent, child), (NodeKind::Link(_), NodeKind::Link(_)))
}

fn requires_special_parent(kind: NodeKind<'_>) -> bool {
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
    parent: NodeKind<'_>,
    children: &[DocumentNodeId],
) -> Result<(), ValidationError> {
    let positions = |matches: fn(NodeKind<'_>) -> bool| {
        children
            .iter()
            .enumerate()
            .filter_map(|(index, id)| {
                document
                    .node(*id)
                    .is_some_and(|node| matches(node.kind()))
                    .then_some(index)
            })
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
        let kind = document.node(id).unwrap().kind();
        if previous.is_some_and(|previous| {
            matches!(document.node(previous).unwrap().kind(), NodeKind::Text(_))
        }) && matches!(kind, NodeKind::Text(_))
        {
            return Err(ValidationError::new(
                "adjacent semantic text nodes were not merged",
            ));
        }
        if let NodeKind::Text(value) = kind
            && value.chars().all(char::is_whitespace)
        {
            let bounded_by_inline = previous
                .map(|id| is_inline(document.node(id).unwrap().kind()))
                .unwrap_or(false)
                && nodes
                    .peek()
                    .map(|id| is_inline(document.node(*id).unwrap().kind()))
                    .unwrap_or(false);
            if value != " " || !bounded_by_inline {
                return Err(ValidationError::new(
                    "semantic whitespace text is not an inline separator",
                ));
            }
        }
        previous = Some(id);
    }
    Ok(())
}

fn validate_tables(document: &Document) -> Result<(), ValidationError> {
    for (index, operation) in document.ops.iter().copied().enumerate() {
        if operation.is_close() || !matches!(operation.kind(), OperationKind::Table) {
            continue;
        }
        let table_id = DocumentNodeId(index as u32);
        let NodeKind::Table(table) = document.node(table_id).unwrap().kind() else {
            return Err(ValidationError::new("table payload is missing"));
        };
        let mut maximum_width = 0u32;
        let mut has_rowspan = false;
        for row in document.child_ids(table_id) {
            if !matches!(document.node(row).unwrap().kind(), NodeKind::TableRow) {
                continue;
            }
            let mut width = 0u32;
            for cell in document.child_ids(row) {
                let NodeKind::TableCell(cell) = document.node(cell).unwrap().kind() else {
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
    for (index, operation) in document.ops.iter().copied().enumerate() {
        if operation.is_close() {
            continue;
        }
        match document.node(DocumentNodeId(index as u32)).unwrap().kind() {
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
            _ => {}
        }
    }

    let mut indexed_ids = HashSet::new();
    let mut labels = HashSet::new();
    for definition in &document.footnotes {
        ensure_id(document, definition.node)?;
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
    use crate::document::{DocumentBuilder, FootnoteId, Link, List, ListKind, NodeKind};

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
        document
            .roots
            .push(crate::document::DocumentNodeId(u32::MAX));
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
                    fragment_only: false,
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
                    fragment_only: false,
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
