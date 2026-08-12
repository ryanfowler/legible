use crate::dom::{AttrName, Dom, NodeId, Tag};
use smallvec::SmallVec;

/// Flattens tables that arrange page content instead of representing data.
///
/// Explicit table semantics always win. For unmarked tables, this pass uses
/// shape and cell content together. It keeps regular multi-row grids, but
/// removes the table structure from one-dimensional and block-rich layouts.
pub(super) fn normalize_layout_tables(dom: &mut Dom, root: NodeId) -> usize {
    let tables: SmallVec<[NodeId; 16]> = dom
        .descendants(root)
        .filter(|&node| dom.tag(node) == Some(Tag::Table))
        .collect();

    // Transform inner tables first. This keeps every retained child attached
    // when an outer layout table is flattened afterward.
    let mut flattened = 0;
    for table in tables.into_iter().rev() {
        if dom.parent(table).is_none() || !is_layout_table(dom, table) {
            continue;
        }
        flatten(dom, table);
        flattened += 1;
    }
    flattened
}

fn is_layout_table(dom: &Dom, table: NodeId) -> bool {
    if dom.attr(table, AttrName::Role).is_some_and(|role| {
        role.split_whitespace().any(|value| {
            value.eq_ignore_ascii_case("presentation") || value.eq_ignore_ascii_case("none")
        })
    }) || dom.attr(table, AttrName::DataTable) == Some("0")
    {
        return true;
    }
    if has_explicit_data_semantics(dom, table) {
        return false;
    }

    let direct_nodes = direct_table_descendants(dom, table);
    let rows: SmallVec<[NodeId; 16]> = direct_nodes
        .iter()
        .copied()
        .filter(|&node| dom.tag(node) == Some(Tag::Tr))
        .collect();
    if rows.is_empty() {
        return false;
    }

    let mut cell_count = 0_usize;
    let mut max_columns = 0_u32;
    let mut block_rich_cells = 0_usize;
    let mut long_prose_cells = 0_usize;
    for row in &rows {
        let cells: SmallVec<[NodeId; 8]> = dom
            .element_children(*row)
            .filter(|&node| matches!(dom.tag(node), Some(Tag::Td | Tag::Th)))
            .collect();
        cell_count += cells.len();
        max_columns = max_columns.max(cells.iter().fold(0_u32, |columns, &cell| {
            columns.saturating_add(
                dom.attr(cell, AttrName::ColSpan)
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(1),
            )
        }));
        for cell in cells {
            if cell_has_block_content(dom, cell) {
                block_rich_cells += 1;
            }
            if direct_text_length(dom, cell) >= 160 {
                long_prose_cells += 1;
            }
        }
    }

    let layout_shape =
        rows.len() == 1 || max_columns <= 1 || block_rich_cells > 0 || long_prose_cells > 0;
    let named_layout = has_layout_name(dom, table) && layout_shape;
    cell_count == 1 || named_layout
}

fn has_explicit_data_semantics(dom: &Dom, table: NodeId) -> bool {
    dom.has_attr(table, AttrName::Summary)
        || dom
            .attr(table, AttrName::DataTable)
            .is_some_and(|value| value != "0")
        || dom.attr(table, AttrName::Role).is_some_and(|role| {
            role.split_whitespace().any(|value| {
                value.eq_ignore_ascii_case("table")
                    || value.eq_ignore_ascii_case("grid")
                    || value.eq_ignore_ascii_case("treegrid")
            })
        })
        || direct_table_descendants(dom, table)
            .into_iter()
            .any(|node| {
                matches!(
                    dom.tag(node),
                    Some(
                        Tag::Caption | Tag::Col | Tag::Colgroup | Tag::Thead | Tag::Tfoot | Tag::Th
                    )
                )
            })
}

fn has_layout_name(dom: &Dom, table: NodeId) -> bool {
    [AttrName::Class, AttrName::Id]
        .into_iter()
        .filter_map(|attribute| dom.attr(table, attribute))
        .flat_map(|value| value.split(|character: char| !character.is_alphanumeric()))
        .any(|token| {
            matches!(
                token.to_ascii_lowercase().as_str(),
                "layout" | "presentation" | "wrapper" | "columns" | "column"
            )
        })
}

fn direct_table_descendants(dom: &Dom, root: NodeId) -> SmallVec<[NodeId; 64]> {
    let mut nodes = SmallVec::new();
    let mut pending: SmallVec<[NodeId; 64]> = dom.children_rev(root).collect();
    while let Some(node) = pending.pop() {
        nodes.push(node);
        if dom.tag(node) != Some(Tag::Table) {
            pending.extend(dom.children_rev(node));
        }
    }
    nodes
}

fn cell_has_block_content(dom: &Dom, cell: NodeId) -> bool {
    direct_table_descendants(dom, cell).into_iter().any(|node| {
        matches!(
            dom.tag(node),
            Some(
                Tag::Address
                    | Tag::Article
                    | Tag::Aside
                    | Tag::Blockquote
                    | Tag::Div
                    | Tag::Dl
                    | Tag::Figure
                    | Tag::Footer
                    | Tag::Form
                    | Tag::H1
                    | Tag::H2
                    | Tag::H3
                    | Tag::H4
                    | Tag::H5
                    | Tag::H6
                    | Tag::Header
                    | Tag::Main
                    | Tag::Nav
                    | Tag::Ol
                    | Tag::P
                    | Tag::Pre
                    | Tag::Section
                    | Tag::Ul
            )
        )
    })
}

fn direct_text_length(dom: &Dom, cell: NodeId) -> usize {
    direct_table_descendants(dom, cell)
        .into_iter()
        .filter_map(|node| dom.text_node(node))
        .map(str::trim)
        .map(str::chars)
        .map(Iterator::count)
        .sum()
}

fn flatten(dom: &mut Dom, table: NodeId) {
    let direct_nodes = direct_table_descendants(dom, table);
    let captions: SmallVec<[NodeId; 2]> = direct_nodes
        .iter()
        .copied()
        .filter(|&node| dom.tag(node) == Some(Tag::Caption))
        .collect();
    let cells: SmallVec<[NodeId; 32]> = direct_nodes
        .into_iter()
        .filter(|&node| dom.tag(node) == Some(Tag::Tr))
        .flat_map(|row| {
            dom.element_children(row)
                .filter(|&node| matches!(dom.tag(node), Some(Tag::Td | Tag::Th)))
        })
        .collect();
    if cells.is_empty() {
        dom.rename_html(table, Tag::Div);
        return;
    }

    for caption in captions {
        let children: SmallVec<[NodeId; 4]> = dom.children(caption).collect();
        for child in children {
            dom.insert_before(table, child);
        }
    }
    for cell in cells {
        let phrasing = dom
            .children(cell)
            .all(|child| crate::scoring::is_phrasing_content(dom, child));
        if phrasing {
            let Ok(paragraph) = dom.create_html_element(Tag::P) else {
                continue;
            };
            dom.move_children(cell, paragraph);
            dom.insert_before(table, paragraph);
        } else {
            let children: SmallVec<[NodeId; 8]> = dom.children(cell).collect();
            for child in children {
                dom.insert_before(table, child);
            }
        }
    }

    dom.detach(table);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn normalized_table_count(html: &str) -> usize {
        let mut dom = Dom::parse_fragment(html, Tag::Div).unwrap();
        let root = dom.root();
        normalize_layout_tables(&mut dom, root);
        dom.descendants(root)
            .filter(|&node| dom.tag(node) == Some(Tag::Table))
            .count()
    }

    #[test]
    fn preserves_small_and_block_wrapped_data_grids() {
        assert_eq!(
            normalized_table_count(
                "<table><tr><td><p>Alpha</p></td><td><p>A long explanation that is still a data value. It contains enough detail to exceed the prose threshold without changing the regular row schema or turning the table into page layout. The explanation continues with supporting context.</p></td></tr><tr><td><p>Beta</p></td><td><p>A shorter value.</p></td></tr></table>"
            ),
            1
        );
        assert_eq!(
            normalized_table_count(
                "<table><tr><td>2</td></tr><tr><td>3</td></tr><tr><td>5</td></tr></table>"
            ),
            1
        );
    }

    #[test]
    fn unicode_prose_threshold_counts_characters() {
        let value = "日本語の値".repeat(20);
        let html = format!(
            "<table class='layout'><tr><td>{value}</td><td>1</td></tr><tr><td>短い値</td><td>2</td></tr><tr><td>別の値</td><td>3</td></tr></table>"
        );
        assert!(value.len() > 160);
        assert!(value.chars().count() < 160);
        assert_eq!(normalized_table_count(&html), 1);
    }

    #[test]
    fn flattening_preserves_a_presentation_caption() {
        let mut dom = Dom::parse_fragment(
            "<table role='presentation'><caption>Diagram summary</caption><tr><td><p>Left</p></td><td><p>Right</p></td></tr></table>",
            Tag::Div,
        )
        .unwrap();
        let root = dom.root();
        normalize_layout_tables(&mut dom, root);
        assert!(dom.descendants(root).any(|node| {
            dom.text_node(node)
                .is_some_and(|text| text == "Diagram summary")
        }));
    }

    #[test]
    fn deeply_wrapped_cell_content_is_stack_safe() {
        let depth = 2_000;
        let wrappers = "<div>".repeat(depth);
        let closing = "</div>".repeat(depth);
        let html = format!(
            "<table><tr><td>{wrappers}Deep value{closing}</td><td>1</td></tr><tr><td>B</td><td>2</td></tr><tr><td>C</td><td>3</td></tr></table>"
        );
        assert_eq!(normalized_table_count(&html), 1);
    }

    #[test]
    fn role_none_marks_a_layout_table() {
        assert_eq!(
            normalized_table_count(
                "<table role='none'><tr><td>Left</td><td>Right</td></tr></table>"
            ),
            0
        );
    }

    #[test]
    fn nested_tables_need_independent_layout_evidence() {
        assert_eq!(
            normalized_table_count(
                "<table><tr><td>A</td><td><table><tr><th>Part</th><th>Value</th></tr><tr><td>X</td><td>1</td></tr></table></td></tr><tr><td>B</td><td>Summary</td></tr><tr><td>C</td><td>Summary</td></tr></table>"
            ),
            2
        );
        assert_eq!(
            normalized_table_count(
                "<table role='presentation'><tr><td><table><tr><th>Part</th><th>Value</th></tr><tr><td>X</td><td>1</td></tr></table></td></tr></table>"
            ),
            1
        );
    }
}
