use crate::dom::{AttrName, Dom, NodeId, Tag};
use smallvec::SmallVec;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum TableKind {
    Data,
    Layout,
    Listing { start: u32 },
}

#[derive(Clone, Copy, Default)]
struct CellFacts {
    has_content: bool,
    meaningful: bool,
    block_rich: bool,
    text_length: usize,
    phrasing: bool,
    rank: Option<u32>,
    has_content_link: bool,
}

pub(super) struct TableAnalysis {
    kinds: Vec<Option<TableKind>>,
    rows: Vec<Option<Box<[NodeId]>>>,
    cells: Vec<Option<Box<[NodeId]>>>,
    captions: Vec<Option<Box<[NodeId]>>>,
    explicit_structure: Vec<bool>,
    cell_facts: Vec<CellFacts>,
    skipped_nodes: Vec<bool>,
    separator_nodes: Vec<bool>,
    text_replacements: Vec<Option<Box<str>>>,
}

impl TableAnalysis {
    /// Classifies every selected table and indexes listing controls once.
    pub(super) fn analyze(dom: &Dom, nodes: &[NodeId]) -> Self {
        let mut analysis = Self {
            kinds: vec![None; dom.len()],
            rows: vec![None; dom.len()],
            cells: vec![None; dom.len()],
            captions: vec![None; dom.len()],
            explicit_structure: vec![false; dom.len()],
            cell_facts: vec![CellFacts::default(); dom.len()],
            skipped_nodes: vec![false; dom.len()],
            separator_nodes: vec![false; dom.len()],
            text_replacements: vec![None; dom.len()],
        };
        let tables: Vec<_> = nodes
            .iter()
            .copied()
            .filter(|&node| dom.tag(node) == Some(Tag::Table))
            .collect();
        let mut text = String::new();
        let mut subtree_content = vec![false; dom.len()];
        for &table in &tables {
            let direct = dom.table_descendants(table);
            let rows: Vec<_> = direct
                .iter()
                .copied()
                .filter(|&node| dom.tag(node) == Some(Tag::Tr))
                .collect();
            let captions: Vec<_> = direct
                .iter()
                .copied()
                .filter(|&node| dom.tag(node) == Some(Tag::Caption))
                .collect();
            analysis.rows[table.index()] = Some(rows.clone().into_boxed_slice());
            analysis.captions[table.index()] = Some(captions.into_boxed_slice());
            analysis.explicit_structure[table.index()] = direct.iter().any(|&node| {
                matches!(
                    dom.tag(node),
                    Some(
                        Tag::Caption | Tag::Col | Tag::Colgroup | Tag::Thead | Tag::Tfoot | Tag::Th
                    )
                )
            });
            for row in rows {
                let cells: Vec<_> = dom
                    .element_children(row)
                    .filter(|&node| matches!(dom.tag(node), Some(Tag::Td | Tag::Th)))
                    .collect();
                for &cell in &cells {
                    analysis.cell_facts[cell.index()] =
                        analyze_cell(dom, cell, &mut text, &mut subtree_content);
                }
                analysis.cells[row.index()] = Some(cells.into_boxed_slice());
            }
        }

        let mut buffer = String::new();
        for table in tables {
            if super::code::is_gutter_table(dom, table) {
                continue;
            }
            let kind =
                if let Some(start) = repeated_listing_start_from_analysis(dom, table, &analysis) {
                    index_listing_controls(dom, table, &mut analysis, &mut buffer);
                    TableKind::Listing { start }
                } else if is_layout_table(dom, table, &analysis) {
                    TableKind::Layout
                } else {
                    TableKind::Data
                };
            analysis.kinds[table.index()] = Some(kind);
        }
        analysis
    }

    pub(super) fn kind(&self, table: NodeId) -> TableKind {
        self.kinds
            .get(table.index())
            .copied()
            .flatten()
            .unwrap_or(TableKind::Data)
    }

    pub(super) fn is_skipped(&self, node: NodeId) -> bool {
        self.skipped_nodes
            .get(node.index())
            .copied()
            .unwrap_or(false)
    }

    pub(super) fn emits_separator(&self, node: NodeId) -> bool {
        self.separator_nodes
            .get(node.index())
            .copied()
            .unwrap_or(false)
    }

    pub(super) fn replacement_text(&self, node: NodeId) -> Option<&str> {
        self.text_replacements
            .get(node.index())
            .and_then(Option::as_deref)
    }

    pub(super) fn rows(&self, table: NodeId) -> &[NodeId] {
        self.rows
            .get(table.index())
            .and_then(Option::as_deref)
            .unwrap_or_default()
    }

    pub(super) fn cells(&self, row: NodeId) -> &[NodeId] {
        self.cells
            .get(row.index())
            .and_then(Option::as_deref)
            .unwrap_or_default()
    }

    pub(super) fn captions(&self, table: NodeId) -> &[NodeId] {
        self.captions
            .get(table.index())
            .and_then(Option::as_deref)
            .unwrap_or_default()
    }

    pub(super) fn row_has_rank(&self, row: NodeId) -> bool {
        self.cells(row)
            .first()
            .is_some_and(|cell| self.cell_facts[cell.index()].rank.is_some())
    }

    pub(super) fn row_has_content(&self, row: NodeId) -> bool {
        self.cells(row)
            .iter()
            .any(|cell| self.cell_facts[cell.index()].has_content)
    }

    pub(super) fn meaningful_cell(&self, cell: NodeId) -> bool {
        self.cell_facts
            .get(cell.index())
            .is_some_and(|facts| facts.meaningful)
    }

    pub(super) fn cell_is_phrasing(&self, cell: NodeId) -> bool {
        self.cell_facts
            .get(cell.index())
            .is_some_and(|facts| facts.phrasing)
    }

    pub(super) fn flattened_count(&self) -> usize {
        self.kinds
            .iter()
            .filter(|kind| matches!(kind, Some(TableKind::Layout)))
            .count()
    }

    pub(super) fn semantic_table_count(&self) -> usize {
        self.kinds
            .iter()
            .filter(|kind| matches!(kind, Some(TableKind::Data)))
            .count()
    }
}

pub(super) fn table_rows(dom: &Dom, table: NodeId) -> SmallVec<[NodeId; 32]> {
    dom.table_descendants(table)
        .into_iter()
        .filter(|&node| dom.tag(node) == Some(Tag::Tr))
        .collect()
}

pub(super) fn row_cells(dom: &Dom, row: NodeId) -> SmallVec<[NodeId; 8]> {
    dom.element_children(row)
        .filter(|&node| matches!(dom.tag(node), Some(Tag::Td | Tag::Th)))
        .collect()
}

fn analyze_cell(
    dom: &Dom,
    cell: NodeId,
    normalized_text: &mut String,
    subtree_content: &mut [bool],
) -> CellFacts {
    normalized_text.clear();
    let mut facts = CellFacts {
        phrasing: children_are_phrasing(dom, cell),
        ..CellFacts::default()
    };
    let mut pending_whitespace = false;
    let nodes = dom.table_descendants(cell);
    for &node in &nodes {
        if let Some(text) = dom.text_node(node) {
            facts.has_content |= !text.trim().is_empty();
            facts.text_length += text.trim().chars().count();
            for character in text.chars() {
                if character.is_whitespace() {
                    pending_whitespace |= !normalized_text.is_empty();
                } else {
                    if pending_whitespace {
                        normalized_text.push(' ');
                        pending_whitespace = false;
                    }
                    normalized_text.push(character);
                }
            }
        }
        let tag = dom.tag(node);
        facts.has_content |= tag == Some(Tag::Table);
        facts.meaningful |= matches!(tag, Some(Tag::Img | Tag::Picture | Tag::Audio | Tag::Video));
        facts.block_rich |= matches!(
            tag,
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
        );
        subtree_content[node.index()] = dom.tag(node) == Some(Tag::Table)
            || dom
                .text_node(node)
                .is_some_and(|text| !text.trim().is_empty());
    }
    for &node in nodes.iter().rev() {
        if subtree_content[node.index()]
            && let Some(parent) = dom.parent(node)
            && parent != cell
        {
            subtree_content[parent.index()] = true;
        }
        facts.has_content_link |= dom.tag(node) == Some(Tag::A) && subtree_content[node.index()];
    }
    for node in nodes {
        subtree_content[node.index()] = false;
    }
    facts.meaningful |= facts.has_content;
    facts.rank = parse_rank_text(normalized_text);
    facts
}

pub(crate) fn inner_text<'a>(dom: &Dom, root: NodeId, out: &'a mut String) -> &'a str {
    out.clear();
    let mut pending_whitespace = false;
    for node in dom.table_descendants(root) {
        let Some(text) = dom.text_node(node) else {
            continue;
        };
        for character in text.chars() {
            if character.is_whitespace() {
                pending_whitespace |= !out.is_empty();
            } else {
                if pending_whitespace {
                    out.push(' ');
                    pending_whitespace = false;
                }
                out.push(character);
            }
        }
    }
    out.trim()
}

pub(crate) fn has_content(dom: &Dom, root: NodeId) -> bool {
    dom.table_descendants(root).into_iter().any(|node| {
        dom.tag(node) == Some(Tag::Table)
            || dom
                .text_node(node)
                .is_some_and(|text| !text.trim().is_empty())
    })
}

/// Returns the first rank for a conservative repeated listing table.
pub(crate) fn repeated_listing_start(dom: &Dom, table: NodeId) -> Option<u32> {
    if dom.tag(table) != Some(Tag::Table)
        || dom.has_attr(table, AttrName::Summary)
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
        || dom.table_descendants(table).into_iter().any(|node| {
            matches!(
                dom.tag(node),
                Some(Tag::Caption | Tag::Th | Tag::Thead | Tag::Tfoot)
            )
        })
    {
        return None;
    }

    let rows = table_rows(dom, table);
    if rows.len() < 6 {
        return None;
    }
    let mut ranked_rows = 0_usize;
    let mut linked_ranked_rows = 0_usize;
    let mut metadata_rows = 0_usize;
    let mut expect_metadata = false;
    let mut outside_text_after_rank = None;
    let mut first_rank = None;
    let mut previous_rank: Option<u32> = None;
    let mut common_columns = None;
    let mut common_shape = 0_usize;
    let mut buffer = String::new();
    for row in rows.iter().copied() {
        let cells = row_cells(dom, row);
        let rank = cells
            .first()
            .and_then(|&cell| parse_rank_text(inner_text(dom, cell, &mut buffer)));
        let Some(rank) = rank else {
            if has_content(dom, row) {
                if expect_metadata {
                    metadata_rows += 1;
                    expect_metadata = false;
                } else if outside_text_after_rank.replace(ranked_rows).is_some() {
                    return None;
                }
            }
            continue;
        };
        if cells.len() < 2
            || previous_rank.is_some_and(|previous| previous.checked_add(1) != Some(rank))
        {
            return None;
        }
        first_rank.get_or_insert(rank);
        previous_rank = Some(rank);
        ranked_rows += 1;
        expect_metadata = true;
        if dom
            .table_descendants(row)
            .into_iter()
            .any(|node| dom.tag(node) == Some(Tag::A) && has_content(dom, node))
        {
            linked_ranked_rows += 1;
        }
        match common_columns {
            Some(columns) if columns == cells.len() => common_shape += 1,
            None => {
                common_columns = Some(cells.len());
                common_shape = 1;
            }
            _ => {}
        }
    }

    (ranked_rows >= 3
        && linked_ranked_rows == ranked_rows
        && metadata_rows + 1 >= ranked_rows
        && outside_text_after_rank.is_none_or(|position| position == ranked_rows)
        && common_shape * 4 >= ranked_rows * 3
        && ranked_rows * 4 >= rows.len())
    .then_some(first_rank?)
}

fn repeated_listing_start_from_analysis(
    dom: &Dom,
    table: NodeId,
    analysis: &TableAnalysis,
) -> Option<u32> {
    if dom.has_attr(table, AttrName::Summary)
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
        || analysis.explicit_structure[table.index()]
    {
        return None;
    }
    let rows = analysis.rows(table);
    if rows.len() < 6 {
        return None;
    }

    let mut ranked_rows = 0_usize;
    let mut linked_ranked_rows = 0_usize;
    let mut metadata_rows = 0_usize;
    let mut expect_metadata = false;
    let mut outside_text_after_rank = None;
    let mut first_rank = None;
    let mut previous_rank: Option<u32> = None;
    let mut common_columns = None;
    let mut common_shape = 0_usize;
    for &row in rows {
        let cells = analysis.cells(row);
        let rank = cells
            .first()
            .and_then(|cell| analysis.cell_facts[cell.index()].rank);
        let Some(rank) = rank else {
            if analysis.row_has_content(row) {
                if expect_metadata {
                    metadata_rows += 1;
                    expect_metadata = false;
                } else if outside_text_after_rank.replace(ranked_rows).is_some() {
                    return None;
                }
            }
            continue;
        };
        if cells.len() < 2
            || previous_rank.is_some_and(|previous| previous.checked_add(1) != Some(rank))
        {
            return None;
        }
        first_rank.get_or_insert(rank);
        previous_rank = Some(rank);
        ranked_rows += 1;
        expect_metadata = true;
        if cells
            .iter()
            .any(|cell| analysis.cell_facts[cell.index()].has_content_link)
        {
            linked_ranked_rows += 1;
        }
        match common_columns {
            Some(columns) if columns == cells.len() => common_shape += 1,
            None => {
                common_columns = Some(cells.len());
                common_shape = 1;
            }
            _ => {}
        }
    }

    (ranked_rows >= 3
        && linked_ranked_rows == ranked_rows
        && metadata_rows + 1 >= ranked_rows
        && outside_text_after_rank.is_none_or(|position| position == ranked_rows)
        && common_shape * 4 >= ranked_rows * 3
        && ranked_rows * 4 >= rows.len())
    .then_some(first_rank?)
}

fn parse_rank_text(text: &str) -> Option<u32> {
    let text = text.trim();
    let digits = text.strip_suffix('.').unwrap_or(text);
    (!digits.is_empty() && digits.len() <= 6 && digits.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| digits.parse().ok())
        .flatten()
}

pub(super) fn children_are_phrasing(dom: &Dom, node: NodeId) -> bool {
    dom.children(node)
        .all(|child| is_phrasing_content(dom, child))
}

fn is_phrasing_content(dom: &Dom, node: NodeId) -> bool {
    fn is_phrasing(dom: &Dom, node: NodeId, depth: u8) -> bool {
        if dom.is_text(node) || dom.is_comment(node) {
            return true;
        }
        let Some(tag) = dom.tag(node) else {
            return false;
        };
        if matches!(
            tag,
            Tag::Abbr
                | Tag::Audio
                | Tag::B
                | Tag::Bdo
                | Tag::Br
                | Tag::Button
                | Tag::Cite
                | Tag::Code
                | Tag::Data
                | Tag::Datalist
                | Tag::Dfn
                | Tag::Em
                | Tag::Embed
                | Tag::I
                | Tag::Img
                | Tag::Input
                | Tag::Kbd
                | Tag::Label
                | Tag::Mark
                | Tag::Math
                | Tag::Meter
                | Tag::Noscript
                | Tag::Object
                | Tag::Output
                | Tag::Progress
                | Tag::Q
                | Tag::Ruby
                | Tag::Samp
                | Tag::Script
                | Tag::Select
                | Tag::Small
                | Tag::Span
                | Tag::Strong
                | Tag::Sub
                | Tag::Sup
                | Tag::Textarea
                | Tag::Time
                | Tag::Var
                | Tag::Wbr
        ) {
            return true;
        }
        matches!(tag, Tag::A | Tag::Del | Tag::Ins)
            && depth < 10
            && dom
                .children(node)
                .all(|child| is_phrasing(dom, child, depth + 1))
    }
    is_phrasing(dom, node, 0)
}

fn is_layout_table(dom: &Dom, table: NodeId, analysis: &TableAnalysis) -> bool {
    if dom.attr(table, AttrName::Role).is_some_and(|role| {
        role.split_whitespace().any(|value| {
            value.eq_ignore_ascii_case("presentation") || value.eq_ignore_ascii_case("none")
        })
    }) || dom.attr(table, AttrName::DataTable) == Some("0")
    {
        return true;
    }
    if dom.has_attr(table, AttrName::Summary)
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
        || analysis.explicit_structure[table.index()]
    {
        return false;
    }

    let rows = analysis.rows(table);
    if rows.is_empty() {
        return false;
    }
    let mut cell_count = 0_usize;
    let mut max_columns = 0_u32;
    let mut block_rich_cells = 0_usize;
    let mut long_prose_cells = 0_usize;
    for &row in rows {
        let cells = analysis.cells(row);
        cell_count += cells.len();
        max_columns = max_columns.max(cells.iter().fold(0_u32, |columns, &cell| {
            columns.saturating_add(
                dom.attr(cell, AttrName::ColSpan)
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(1),
            )
        }));
        for &cell in cells {
            let facts = analysis.cell_facts[cell.index()];
            block_rich_cells += usize::from(facts.block_rich);
            long_prose_cells += usize::from(facts.text_length >= 160);
        }
    }

    let layout_shape =
        rows.len() == 1 || max_columns <= 1 || block_rich_cells > 0 || long_prose_cells > 0;
    cell_count == 1 || has_layout_name(dom, table) && layout_shape
}

pub(crate) fn class_is_semantic_evidence(dom: &Dom, node: NodeId) -> bool {
    dom.tag(node) == Some(Tag::Table)
        && dom
            .attr(node, AttrName::Class)
            .is_some_and(value_has_layout_name)
}

fn has_layout_name(dom: &Dom, table: NodeId) -> bool {
    [AttrName::Class, AttrName::Id]
        .into_iter()
        .filter_map(|attribute| dom.attr(table, attribute))
        .any(value_has_layout_name)
}

fn value_has_layout_name(value: &str) -> bool {
    value
        .split(|character: char| !character.is_alphanumeric())
        .any(|token| {
            matches!(
                token.to_ascii_lowercase().as_str(),
                "layout" | "presentation" | "wrapper" | "columns" | "column"
            )
        })
}

fn index_listing_controls(
    dom: &Dom,
    table: NodeId,
    analysis: &mut TableAnalysis,
    buffer: &mut String,
) {
    for anchor in dom
        .table_descendants(table)
        .into_iter()
        .filter(|&node| dom.tag(node) == Some(Tag::A))
    {
        let text = inner_text(dom, anchor, buffer).to_ascii_lowercase();
        let action_label = matches!(
            text.as_str(),
            "hide" | "vote" | "delete" | "share" | "login" | "sign in" | "subscribe"
        );
        let action_url = dom.attr(anchor, AttrName::Href).is_some_and(|href| {
            let href = href.to_ascii_lowercase();
            href.contains("action=")
                || href.contains("how=")
                || href.starts_with("vote?")
                || href.starts_with("hide?")
                || href.starts_with("delete?")
                || href.contains("/vote?")
                || href.contains("/hide?")
                || href.contains("/delete?")
        });
        let has_media = dom.table_descendants(anchor).into_iter().any(|node| {
            matches!(
                dom.tag(node),
                Some(Tag::Img | Tag::Picture | Tag::Audio | Tag::Video)
            )
        });
        if !(text.is_empty() && action_url && !has_media || action_label && action_url) {
            continue;
        }
        analysis.skipped_nodes[anchor.index()] = true;
        if !action_label {
            continue;
        }
        let previous = separator_replacement(dom, dom.prev_sibling(anchor), true);
        let next = separator_replacement(dom, dom.next_sibling(anchor), false);
        if previous.is_some() || next.is_some() {
            if let Some((node, replacement)) = previous {
                analysis.text_replacements[node.index()] = Some(replacement.into());
            }
            if let Some((node, replacement)) = next {
                analysis.text_replacements[node.index()] = Some(replacement.into());
            }
            analysis.separator_nodes[anchor.index()] = true;
        }
    }
}

fn separator_replacement(
    dom: &Dom,
    node: Option<NodeId>,
    previous: bool,
) -> Option<(NodeId, String)> {
    let node = node?;
    let text = dom.text_node(node)?;
    let replacement = if previous {
        let trimmed = text.trim_end();
        let retained = trimmed.trim_end_matches(is_control_separator_character);
        (retained.len() != trimmed.len()).then(|| retained.trim_end().to_owned())
    } else {
        let trimmed = text.trim_start();
        let retained = trimmed.trim_start_matches(is_control_separator_character);
        (retained.len() != trimmed.len()).then(|| retained.trim_start().to_owned())
    }?;
    Some((node, replacement))
}

fn is_control_separator_character(character: char) -> bool {
    matches!(character, '|' | '·' | '-' | '–' | '—' | '•')
}
