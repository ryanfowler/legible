use std::collections::{HashMap, HashSet};

use super::{
    Document, FootnoteId, OP_CLOSE, OperationKind, SemanticItemView as Item, ValidationError,
};

pub(super) fn validate(document: &Document) -> Result<(), ValidationError> {
    validate_text_arena(document)?;
    validate_tape(document)?;

    struct Frame<'a> {
        opening: usize,
        kind: Item<'a>,
        children: Vec<Item<'a>>,
    }

    let mut stack = Vec::<Frame>::new();
    for (index, operation) in document.ops.iter().copied().enumerate() {
        if operation.is_close() {
            let opening = document.operation_opening_index(operation);
            let frame = stack
                .pop()
                .ok_or_else(|| ValidationError::new("semantic tape closes without an opener"))?;
            if frame.opening != opening {
                return Err(ValidationError::new(
                    "semantic tape close nesting is invalid",
                ));
            }
            validate_adjacent_text(frame.children.iter().copied())?;
            validate_child_order(frame.kind, &frame.children)?;
            continue;
        }

        let kind = document
            .operation_view(index)
            .ok_or_else(|| ValidationError::new("semantic payload is missing"))?;
        validate_kind(kind)?;
        let inside_link = stack
            .iter()
            .any(|frame| matches!(frame.kind, Item::Link(_)));
        if inside_link && matches!(kind, Item::Link(_)) {
            return Err(ValidationError::new("semantic links cannot be nested"));
        }
        if let Some(parent) = stack.last_mut() {
            if !valid_child(parent.kind, kind) {
                return Err(ValidationError::new(format!(
                    "semantic parent {:#?} cannot contain {kind:#?}",
                    parent.kind
                )));
            }
            parent.children.push(kind);
        } else if !valid_root(kind) {
            return Err(ValidationError::new(format!(
                "semantic root {kind:#?} requires a structural parent"
            )));
        }

        if operation.kind().is_container() {
            stack.push(Frame {
                opening: index,
                kind,
                children: Vec::new(),
            });
        }
    }
    if !stack.is_empty() {
        return Err(ValidationError::new(
            "semantic tape has an unclosed container",
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

fn validate_kind(kind: Item<'_>) -> Result<(), ValidationError> {
    match kind {
        Item::Invalid => return Err(ValidationError::new("semantic payload is missing")),
        Item::Text("") => {
            return Err(ValidationError::new("semantic text node is empty"));
        }
        Item::Text(value) if super::text::normalize_prose_fragment(value).as_ref() != value => {
            return Err(ValidationError::new("semantic text is not canonical prose"));
        }
        Item::Heading { level } if !(1..=6).contains(&level) => {
            return Err(ValidationError::new(
                "semantic heading level is outside 1 through 6",
            ));
        }
        Item::TableCell(cell) if cell.colspan == 0 || cell.rowspan == 0 => {
            return Err(ValidationError::new(
                "semantic table cell span is less than one",
            ));
        }
        Item::Link(link)
            if !valid_destination(&link.destination, super::uri::DestinationKind::Link) =>
        {
            return Err(ValidationError::new(
                "semantic link has an unsafe destination",
            ));
        }
        Item::Image(image)
            if !valid_destination(&image.source, super::uri::DestinationKind::Resource) =>
        {
            return Err(ValidationError::new(
                "semantic image has an unsafe destination",
            ));
        }
        Item::Media(media)
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

fn valid_root(kind: Item<'_>) -> bool {
    !matches!(
        kind,
        Item::ListItem
            | Item::TableCaption
            | Item::TableRow
            | Item::TableCell(_)
            | Item::Figcaption
            | Item::Summary
            | Item::DefinitionTerm
            | Item::DefinitionDescription
    )
}

fn valid_child(parent: Item<'_>, child: Item<'_>) -> bool {
    match parent {
        Item::List(_) => matches!(child, Item::ListItem | Item::FootnoteDefinition(_)),
        Item::Table(_) => matches!(child, Item::TableCaption | Item::TableRow),
        Item::TableRow => matches!(child, Item::TableCell(_)),
        Item::DefinitionList => matches!(child, Item::DefinitionTerm | Item::DefinitionDescription),
        Item::Paragraph => is_inline(child) || matches!(child, Item::DisplayMath(_)),
        Item::Heading { .. }
        | Item::TableCaption
        | Item::DefinitionTerm
        | Item::Emphasis
        | Item::Strong
        | Item::Strikethrough
        | Item::Link(_) => is_inline(child) && !nested_link(parent, child),
        Item::Text(_)
        | Item::CodeBlock(_)
        | Item::InlineCode(_)
        | Item::Image(_)
        | Item::HardBreak
        | Item::FootnoteReference(_)
        | Item::TaskMarker(_)
        | Item::InlineMath(_)
        | Item::DisplayMath(_)
        | Item::Media(_)
        | Item::ThematicBreak
        | Item::Invalid => false,
        Item::Details => !requires_special_parent(child) || matches!(child, Item::Summary),
        Item::Figure => !requires_special_parent(child) || matches!(child, Item::Figcaption),
        Item::BlockGroup
        | Item::Summary
        | Item::BlockQuote
        | Item::Figcaption
        | Item::ListItem
        | Item::TableCell(_)
        | Item::DefinitionDescription
        | Item::Callout(_)
        | Item::FootnoteDefinition(_) => !requires_special_parent(child),
    }
}

fn is_inline(kind: Item<'_>) -> bool {
    matches!(
        kind,
        Item::Text(_)
            | Item::Emphasis
            | Item::Strong
            | Item::Strikethrough
            | Item::InlineCode(_)
            | Item::Link(_)
            | Item::Image(_)
            | Item::HardBreak
            | Item::FootnoteReference(_)
            | Item::TaskMarker(_)
            | Item::InlineMath(_)
            | Item::Media(_)
    )
}

fn nested_link(parent: Item<'_>, child: Item<'_>) -> bool {
    matches!((parent, child), (Item::Link(_), Item::Link(_)))
}

fn requires_special_parent(kind: Item<'_>) -> bool {
    matches!(
        kind,
        Item::ListItem
            | Item::TableCaption
            | Item::TableRow
            | Item::TableCell(_)
            | Item::Figcaption
            | Item::Summary
            | Item::DefinitionTerm
            | Item::DefinitionDescription
    )
}

fn validate_child_order(parent: Item<'_>, children: &[Item<'_>]) -> Result<(), ValidationError> {
    let positions = |matches: fn(Item<'_>) -> bool| {
        children
            .iter()
            .enumerate()
            .filter_map(|(index, kind)| matches(*kind).then_some(index))
            .collect::<Vec<_>>()
    };
    match parent {
        Item::Table(_) => {
            let captions = positions(|kind| matches!(kind, Item::TableCaption));
            if captions.len() > 1 || captions.first().is_some_and(|&index| index != 0) {
                return Err(ValidationError::new(
                    "semantic table caption must be unique and first",
                ));
            }
        }
        Item::Details => {
            let summaries = positions(|kind| matches!(kind, Item::Summary));
            if summaries.len() > 1 || summaries.first().is_some_and(|&index| index != 0) {
                return Err(ValidationError::new(
                    "semantic details summary must be unique and first",
                ));
            }
        }
        Item::Figure => {
            let captions = positions(|kind| matches!(kind, Item::Figcaption));
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

fn validate_adjacent_text<'a>(
    nodes: impl Iterator<Item = Item<'a>>,
) -> Result<(), ValidationError> {
    let nodes = nodes.collect::<Vec<_>>();
    for (index, &kind) in nodes.iter().enumerate() {
        if index > 0 && matches!(nodes[index - 1], Item::Text(_)) && matches!(kind, Item::Text(_)) {
            return Err(ValidationError::new(
                "adjacent semantic text nodes were not merged",
            ));
        }
        if let Item::Text(value) = kind
            && value.chars().all(char::is_whitespace)
        {
            let bounded_by_inline = index > 0
                && index + 1 < nodes.len()
                && is_inline(nodes[index - 1])
                && is_inline(nodes[index + 1]);
            if value != " " || !bounded_by_inline {
                return Err(ValidationError::new(
                    "semantic whitespace text is not an inline separator",
                ));
            }
        }
    }
    Ok(())
}

fn validate_tables(document: &Document) -> Result<(), ValidationError> {
    struct Frame {
        kind: OperationKind,
        width: u32,
        max_width: u32,
        has_rowspan: bool,
        column_count: Option<u32>,
    }

    let mut stack = Vec::<Frame>::new();
    for (index, operation) in document.ops.iter().copied().enumerate() {
        if operation.is_close() {
            let frame = stack
                .pop()
                .ok_or_else(|| ValidationError::new("table stack is unbalanced"))?;
            match frame.kind {
                OperationKind::TableCell => {
                    if let Some(parent) = stack.last_mut()
                        && parent.kind == OperationKind::TableRow
                    {
                        parent.width = parent.width.checked_add(frame.width).ok_or_else(|| {
                            ValidationError::new("semantic table spans exceed column capacity")
                        })?;
                        parent.has_rowspan |= frame.has_rowspan;
                    }
                }
                OperationKind::TableRow => {
                    if let Some(parent) = stack.last_mut()
                        && parent.kind == OperationKind::Table
                    {
                        parent.max_width = parent.max_width.max(frame.width);
                        parent.has_rowspan |= frame.has_rowspan;
                    }
                }
                OperationKind::Table => {
                    let expected = (!frame.has_rowspan).then_some(frame.max_width);
                    if frame.column_count != expected {
                        return Err(ValidationError::new(
                            "semantic table column count does not match its spans",
                        ));
                    }
                }
                _ => {}
            }
            continue;
        }
        if !operation.kind().is_container() {
            continue;
        }
        let (width, has_rowspan, column_count) = match document.operation_view(index) {
            Some(Item::Table(table)) => (0, false, table.column_count),
            Some(Item::TableCell(cell)) => (cell.colspan, cell.rowspan > 1, None),
            _ => (0, false, None),
        };
        stack.push(Frame {
            kind: operation.kind(),
            width,
            max_width: 0,
            has_rowspan,
            column_count,
        });
    }
    if !stack.is_empty() {
        return Err(ValidationError::new("table stack is unbalanced"));
    }
    Ok(())
}

fn validate_footnotes(document: &Document) -> Result<(), ValidationError> {
    let mut definition_nodes = HashMap::<FootnoteId, usize>::new();
    let mut references = HashSet::<FootnoteId>::new();
    for (index, operation) in document.ops.iter().copied().enumerate() {
        if operation.is_close() {
            continue;
        }
        match document.operation_view(index) {
            Some(Item::FootnoteReference(id)) => {
                references.insert(id);
            }
            Some(Item::FootnoteDefinition(id)) if definition_nodes.insert(id, index).is_some() => {
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
        let node = definition.node.index();
        if document.operation_kind(node) != Some(OperationKind::FootnoteDefinition)
            || document.operation_view(node).is_none_or(
                |kind| !matches!(kind, Item::FootnoteDefinition(id) if id == definition.id),
            )
        {
            return Err(ValidationError::new(
                "footnote index points outside the definition operations",
            ));
        }
        if !indexed_ids.insert(definition.id) || !labels.insert(definition.label.as_ref()) {
            return Err(ValidationError::new(
                "footnote index has duplicate IDs or labels",
            ));
        }
        if definition_nodes.get(&definition.id) != Some(&node) {
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
