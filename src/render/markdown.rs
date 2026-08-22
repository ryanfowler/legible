//! CommonMark/GFM rendering from the semantic document.
//
// The renderer interprets the private semantic tape in source order. It keeps
// only formatting and structural state that is needed while writing output.

use std::fmt;

use crate::document::{
    Document, EventOp, FootnoteId, HAS_VISIBLE_IMAGE, HAS_VISIBLE_TEXT, ListKind, OperationKind,
    SemanticItemView as Item, TableAlignment,
};
use smallvec::SmallVec;

#[derive(Clone, Copy, Debug)]
pub(crate) struct MarkdownConfig {
    pub(crate) links: bool,
    pub(crate) images: bool,
}

impl Default for MarkdownConfig {
    fn default() -> Self {
        Self {
            links: true,
            images: true,
        }
    }
}

#[derive(Clone, Copy)]
enum CloseAction {
    None,
    Block,
    Quote,
    List,
    ListItem,
    Marker(&'static str),
    Link(bool),
    Caption { in_table: bool },
    Table { active: bool },
    TableRow { header: bool, top_level: bool },
    Footnote,
}

struct Frame {
    index: usize,
    kind: OperationKind,
    close: CloseAction,
    direct_children: usize,
    cells: usize,
    first_row_seen: bool,
    list_items: usize,
}

impl Frame {
    fn new(index: usize, kind: OperationKind, close: CloseAction) -> Self {
        Self {
            index,
            kind,
            close,
            direct_children: 0,
            cells: 0,
            first_row_seen: false,
            list_items: 0,
        }
    }
}

enum ListMarker {
    Bullet,
    Ordered,
    OrderedStart(i32),
}

pub(crate) fn render_markdown(
    document: &Document,
    capacity: usize,
    config: MarkdownConfig,
) -> String {
    let mut output = String::with_capacity(capacity.max(512));
    write_markdown(document, &mut output, config).expect("writing to a String cannot fail");
    output
}

pub(crate) fn write_markdown<W: fmt::Write>(
    document: &Document,
    writer: &mut W,
    config: MarkdownConfig,
) -> fmt::Result {
    MarkdownRenderer::new(document, config, writer).render()
}

struct MarkdownRenderer<'document, 'writer> {
    document: &'document Document,
    out: Output<'writer>,
    frames: Vec<Frame>,
    list_depth: usize,
    table_depth: usize,
    config: MarkdownConfig,
}

impl<'document, 'writer> MarkdownRenderer<'document, 'writer> {
    fn new<W: fmt::Write>(
        document: &'document Document,
        config: MarkdownConfig,
        writer: &'writer mut W,
    ) -> Self {
        Self {
            document,
            out: Output::new(writer),
            frames: Vec::with_capacity(32),
            list_depth: 0,
            table_depth: 0,
            config,
        }
    }

    fn render(mut self) -> fmt::Result {
        let mut index = 0;
        while index < self.document.operations().len() {
            let Some(operation) = self.document.operations().get(index).copied() else {
                break;
            };
            if operation.is_close() {
                self.close(operation);
                index += 1;
                continue;
            }

            let Some(node) = self.document.operation_view(index) else {
                index += 1;
                continue;
            };
            if let Some(next) = self.open(index, operation, node) {
                index = next;
            } else {
                index += 1;
            }
        }
        self.out.finish()
    }

    fn open(&mut self, index: usize, operation: EventOp, node: Item<'_>) -> Option<usize> {
        let kind = self.document.operation_kind(index)?;
        if let Some(parent) = self.frames.last_mut() {
            parent.direct_children += 1;
            if parent.kind == OperationKind::List && kind == OperationKind::ListItem {
                parent.list_items += 1;
            }
        }

        let end = self.document.operation_end(index);
        let parent_kind = self.frames.last().map(|frame| frame.kind);

        match node {
            Item::Text(text) => {
                // Most text leaves end in ordinary prose characters. Only a
                // small set of trailing punctuation can form a Markdown
                // construct with the next semantic leaf, so avoid a forward
                // tape scan for the common case.
                let next = needs_next_text_char(text)
                    .then(|| self.next_text_char(index))
                    .flatten();
                self.out.text(text, next);
            }
            Item::Heading { level } => {
                if !self.visible(operation) {
                    if !self.config.images {
                        return Some(end.saturating_add(1));
                    }
                    self.push_frame(index, kind, CloseAction::None);
                    return None;
                }
                self.out.ensure_blank_line();
                self.out.markup_repeat('#', usize::from(level));
                self.out.markup(" ");
                self.push_frame(index, kind, CloseAction::Block);
            }
            Item::Paragraph => {
                let in_list_item = parent_kind == Some(OperationKind::ListItem);
                let in_table_cell = parent_kind == Some(OperationKind::TableCell);
                let in_first_footnote_paragraph = parent_kind
                    == Some(OperationKind::FootnoteDefinition)
                    && self
                        .frames
                        .last()
                        .is_some_and(|frame| frame.direct_children == 1);
                if in_list_item {
                    self.start_item_paragraph();
                } else if !in_first_footnote_paragraph && !in_table_cell {
                    self.out.ensure_blank_line();
                }
                self.push_frame(
                    index,
                    kind,
                    if in_first_footnote_paragraph || in_table_cell {
                        CloseAction::None
                    } else {
                        CloseAction::Block
                    },
                );
            }
            Item::TableCaption => {
                self.out.ensure_blank_line();
                self.push_frame(
                    index,
                    kind,
                    CloseAction::Caption {
                        in_table: parent_kind == Some(OperationKind::Table),
                    },
                );
            }
            Item::Figcaption
            | Item::DefinitionTerm
            | Item::DefinitionDescription
            | Item::Summary => {
                self.out.ensure_blank_line();
                self.push_frame(index, kind, CloseAction::Block);
            }
            Item::BlockGroup => {
                if self.block_contains_only_footnotes(index) {
                    self.out.limit_trailing_newlines(3);
                    self.push_frame(index, kind, CloseAction::None);
                } else {
                    if !self.out.in_empty_list_item() {
                        self.out.ensure_blank_line();
                    }
                    self.push_frame(index, kind, CloseAction::Block);
                }
            }
            Item::Figure | Item::Details | Item::DefinitionList => {
                if !self.out.in_empty_list_item() {
                    self.out.ensure_blank_line();
                }
                self.push_frame(index, kind, CloseAction::Block);
            }
            Item::BlockQuote | Item::Callout(_) => {
                self.out.ensure_blank_line();
                self.out.prefixes.push(Prefix::Quote);
                self.push_frame(index, kind, CloseAction::Quote);
            }
            Item::HardBreak => self.out.hard_break(),
            Item::ThematicBreak => {
                self.out.ensure_blank_line();
                self.out.mark_list_item_content();
                self.out.markup("---");
                self.out.newline();
            }
            Item::Strong => self.format(index, kind, operation, "**"),
            Item::Emphasis => self.format(index, kind, operation, "*"),
            Item::Strikethrough => self.format(index, kind, operation, "~~"),
            Item::InlineCode(text) => self.code_span(text),
            Item::CodeBlock(code) => self.code_block(code.language(), code.text()),
            Item::Link(link) => {
                if self.config.links {
                    self.out.mark_inline_boundary();
                    self.out.open_link();
                }
                self.push_frame(index, kind, CloseAction::Link(self.config.links));
                let _ = link;
            }
            Item::Image(image) if self.config.images => self.image(image),
            Item::Image(_) => {}
            Item::List(list) => self.list(index, kind, list.kind, list.start),
            Item::ListItem => self.list_item(index, kind),
            Item::Table(_) => self.table(index, kind),
            Item::TableRow => self.table_row(index, kind),
            Item::TableCell(_) => {
                if self.start_table_cell(index) {
                    return Some(end.saturating_add(1));
                }
                self.push_frame(index, kind, CloseAction::None);
            }
            Item::FootnoteReference(id) => self.footnote_reference(id),
            Item::FootnoteDefinition(id) => {
                let Some(label) = self.document.footnote_label(id) else {
                    return Some(end.saturating_add(1));
                };
                self.out.ensure_blank_line();
                self.out.markup("[^");
                self.out.footnote_label(label);
                self.out.markup("]: ");
                self.out.prefixes.push(Prefix::Indent(4));
                self.push_frame(index, kind, CloseAction::Footnote);
            }
            Item::TaskMarker(_) => {}
            Item::InlineMath(math) => self.math(math.source(), false),
            Item::DisplayMath(math) => self.math(math.source(), true),
            Item::Media(media) => {
                let title = media.title().unwrap_or(media.source());
                if self.config.links {
                    self.out.markup("[");
                    self.out.label(title);
                    self.out.markup("](");
                    self.out.destination(media.source());
                    self.out.markup(")");
                } else {
                    self.out.text(title, None);
                }
            }
            Item::Invalid => {}
        }
        None
    }

    fn close(&mut self, operation: EventOp) {
        let opening = self.document.operation_opening_index(operation);
        let Some(frame) = self.frames.pop() else {
            return;
        };
        debug_assert_eq!(frame.index, opening);
        match frame.close {
            CloseAction::None => {}
            CloseAction::Block => self.out.newline(),
            CloseAction::Marker(marker) => {
                if self.out.close_marker(marker, marker) {
                    self.out.mark_inline_boundary();
                }
            }
            CloseAction::Link(enabled) => {
                if !enabled {
                    return;
                }
                let Some(Item::Link(link)) = self.document.operation_view(opening) else {
                    return;
                };
                if self.out.close_marker("[", "](") {
                    let trailing = std::mem::take(&mut self.out.pending_space);
                    self.out.destination(link.destination());
                    if let Some(title) = link.title() {
                        self.out.markup(" \"");
                        self.out.link_title(title);
                        self.out.markup("\"");
                    }
                    self.out.markup(")");
                    self.out.pending_space = trailing;
                    self.out.mark_inline_boundary();
                }
            }
            CloseAction::Caption { in_table } => {
                // A table caption closes its own block and then the table's
                // surrounding block boundary. Keep both boundaries to match
                // the former caption task ordering.
                self.out.newline();
                if in_table {
                    self.out.newline();
                }
            }
            CloseAction::Quote => {
                if self.out.has_current_line_content() {
                    self.out.newline();
                }
                self.out.prefixes.pop();
            }
            CloseAction::List => {
                if self.out.has_current_line_content() {
                    self.out.newline();
                }
                self.list_depth -= 1;
            }
            CloseAction::ListItem => {
                if self.out.has_current_line_content() {
                    self.out.newline();
                }
                self.out.prefixes.pop();
            }
            CloseAction::Table { active } => {
                if active {
                    self.table_depth -= 1;
                }
            }
            CloseAction::TableRow { header, top_level } => {
                if top_level {
                    self.out.markup(" |");
                }
                self.out.newline();
                if header {
                    self.table_header(opening);
                }
            }
            CloseAction::Footnote => {
                if self.out.has_current_line_content() {
                    self.out.newline();
                }
                self.out.prefixes.pop();
            }
        }
    }

    fn push_frame(&mut self, index: usize, kind: OperationKind, close: CloseAction) {
        self.frames.push(Frame::new(index, kind, close));
    }

    fn visible(&self, operation: EventOp) -> bool {
        let flags = operation.flags();
        flags & HAS_VISIBLE_TEXT != 0 || self.config.images && flags & HAS_VISIBLE_IMAGE != 0
    }

    fn next_text_char(&self, index: usize) -> Option<char> {
        let mut nested = 0usize;
        let mut next = index.saturating_add(1);
        while let Some(operation) = self.document.operations().get(next).copied() {
            if operation.is_close() {
                if nested == 0 {
                    return None;
                }
                nested -= 1;
                next += 1;
                continue;
            }
            let kind = self.document.operation_kind(next)?;
            if is_block_operation(kind) {
                next = self.document.operation_end(next).saturating_add(1);
                continue;
            }
            let node = self.document.operation_view(next)?;
            match node {
                Item::Text(text) => return text.chars().next(),
                Item::Image(_) if self.config.images => return Some('!'),
                _ => {
                    if kind.is_container() {
                        nested += 1;
                    }
                }
            }
            next += 1;
        }
        None
    }

    fn format(
        &mut self,
        index: usize,
        kind: OperationKind,
        operation: EventOp,
        marker: &'static str,
    ) {
        if self.visible(operation) {
            self.out.mark_inline_boundary();
        }
        self.out.open_marker(marker);
        self.push_frame(index, kind, CloseAction::Marker(marker));
    }

    fn code_span(&mut self, text: &str) {
        let table_text = (self.table_depth == 1).then(|| text.replace('|', "\\|"));
        let text = table_text.as_deref().unwrap_or(text);
        let mut scan = CollapsedText::default();
        scan.scan(text);
        let fence = scan.longest_backtick_run + 1;
        let pad = scan.starts_with_backtick || scan.ends_with_backtick;
        self.out.mark_list_item_content();
        if scan.has_content {
            self.out.mark_inline_boundary();
            self.out.prepare_inline_boundary(scan.first_char);
        }
        self.out.markup_repeat(char::from(96), fence);
        if pad {
            self.out.verbatim(" ");
        }
        let mut writer = CollapsedTextWriter::default();
        self.out.begin_verbatim();
        writer.write(&mut self.out.value, text);
        if writer.empty && writer.pending_whitespace {
            self.out.value.push(' ');
        }
        if pad {
            self.out.verbatim(" ");
        }
        self.out.markup_repeat(char::from(96), fence);
        if !writer.empty {
            self.out.last_text_char = writer.last_char;
            self.out.mark_inline_boundary();
        }
    }

    fn code_block(&mut self, language: Option<&str>, text: &str) {
        self.out.ensure_blank_line();
        self.out.mark_list_item_content();
        let mut longest = 0;
        let mut current = 0;
        scan_longest_run(text.as_bytes(), b'\x60', &mut longest, &mut current);
        let fence = 3.max(longest + 1);
        self.out.markup_repeat(char::from(96), fence);
        if let Some(language) = language {
            self.out.markup(language);
        }
        self.out.newline();
        self.out.verbatim(text.strip_suffix('\n').unwrap_or(text));
        self.out.newline();
        self.out.markup_repeat(char::from(96), fence);
        self.out.newline();
    }

    fn image(&mut self, image: &crate::document::Image) {
        self.out.mark_list_item_content();
        self.out.markup("![");
        if self.table_depth == 1 {
            self.out.table_label(image.alt());
        } else {
            self.out.label(image.alt());
        }
        self.out.markup("](");
        self.out.destination(image.source());
        if let Some(title) = image.title() {
            self.out.markup(" \"");
            self.out.link_title(title);
            self.out.markup("\"");
        }
        self.out.markup(")");
    }

    fn math(&mut self, source: &str, block: bool) {
        let source = escape_math_source(source.trim());
        if block {
            self.out.ensure_blank_line();
            self.out.markup("$$");
            self.out.newline();
            self.out.verbatim(&source);
            self.out.newline();
            self.out.markup("$$");
            self.out.newline();
        } else {
            self.out.markup("$");
            self.out.verbatim(&source);
            self.out.markup("$");
        }
    }

    fn list(
        &mut self,
        index: usize,
        kind: OperationKind,
        _list_kind: ListKind,
        _start: Option<i64>,
    ) {
        if self.out.has_current_line_content() {
            self.out.newline();
        }
        if self.list_depth == 0 {
            self.out.ensure_blank_line();
        }
        self.list_depth += 1;
        self.push_frame(index, kind, CloseAction::List);
    }

    fn list_item(&mut self, index: usize, kind: OperationKind) {
        let marker = self.next_list_marker();
        if self.out.has_current_line_content() {
            self.out.newline();
        }
        let mut indent = match marker {
            ListMarker::Bullet => {
                self.out.markup("- ");
                2
            }
            ListMarker::Ordered => {
                self.out.markup("1. ");
                3
            }
            ListMarker::OrderedStart(start) => {
                let width = decimal_len(start) + 2;
                self.out.markup_number(start);
                self.out.markup(". ");
                width
            }
        };
        if let Some((checked, label)) = self
            .task_marker(index)
            .map(|(checked, label)| (checked, label.map(str::to_owned)))
        {
            self.out.markup(if checked { "[x]" } else { "[ ]" });
            self.out.pending_space = true;
            if !self.list_item_has_text(index)
                && let Some(label) = label
            {
                self.out.text(&label, None);
            }
            indent += 4;
        }
        self.out.begin_list_item_content();
        self.out.prefixes.push(Prefix::ListItem {
            width: indent,
            has_content: false,
        });
        self.push_frame(index, kind, CloseAction::ListItem);
    }

    fn table(&mut self, index: usize, kind: OperationKind) {
        self.out.ensure_blank_line();
        let active = !self.table_has_spans(index);
        if active {
            self.table_depth += 1;
        }
        self.push_frame(index, kind, CloseAction::Table { active });
    }

    fn table_row(&mut self, index: usize, kind: OperationKind) {
        let top_level = self.table_depth == 1;
        let header = top_level && self.mark_first_table_row();
        if self.out.has_current_line_content() {
            self.out.newline();
        }
        if top_level {
            self.out.markup("| ");
        }
        self.push_frame(index, kind, CloseAction::TableRow { header, top_level });
    }

    fn start_table_cell(&mut self, index: usize) -> bool {
        let Some(row) = self
            .frames
            .last_mut()
            .filter(|frame| frame.kind == OperationKind::TableRow)
        else {
            return false;
        };
        if row.cells > 0 {
            self.out
                .markup(if self.table_depth == 1 { " | " } else { "; " });
            self.out.last_text_char = None;
        }
        row.cells += 1;
        if self.table_cell_requires_flattening(index) {
            let text = self.flatten_table_cell(index);
            self.out.text(text.trim(), None);
            return true;
        }
        false
    }

    fn mark_first_table_row(&mut self) -> bool {
        let Some(table) = self
            .frames
            .iter_mut()
            .rev()
            .find(|frame| frame.kind == OperationKind::Table)
        else {
            return false;
        };
        if !table.first_row_seen {
            table.first_row_seen = true;
            true
        } else {
            false
        }
    }

    fn table_header(&mut self, row: usize) {
        if self.table_depth != 1 {
            return;
        }
        let values = self.table_row_alignments(row);
        self.out.markup("| ");
        for (column, alignment) in values.into_iter().enumerate() {
            if column > 0 {
                self.out.markup(" | ");
            }
            self.out.markup(match alignment {
                Some(TableAlignment::Left) => ":---",
                Some(TableAlignment::Center) => ":---:",
                Some(TableAlignment::Right) => "---:",
                None => "---",
            });
        }
        self.out.markup(" |");
        self.out.newline();
    }

    fn block_contains_only_footnotes(&self, root: usize) -> bool {
        let mut direct = 0;
        let mut depth = 0usize;
        let end = self.document.operation_end(root);
        let mut index = root.saturating_add(1);
        while index < end {
            let Some(operation) = self.document.operations().get(index).copied() else {
                break;
            };
            if operation.is_close() {
                depth = depth.saturating_sub(1);
                index += 1;
                continue;
            }
            if depth == 0 {
                direct += 1;
                if self.document.operation_kind(index) != Some(OperationKind::FootnoteDefinition) {
                    return false;
                }
            }
            if self
                .document
                .operation_kind(index)
                .is_some_and(OperationKind::is_container)
            {
                depth += 1;
            }
            index += 1;
        }
        direct > 0
    }

    fn task_marker(&self, item: usize) -> Option<(bool, Option<&str>)> {
        let end = self.document.operation_end(item);
        let mut index = item.saturating_add(1);
        while index < end {
            let Some(operation) = self.document.operations().get(index).copied() else {
                break;
            };
            if operation.is_close() {
                index += 1;
                continue;
            }
            let Some(node) = self.document.operation_view(index) else {
                index += 1;
                continue;
            };
            match node {
                Item::Text(text) if has_visible_inline_text(text) => return None,
                Item::TaskMarker(marker) => {
                    return Some((marker.is_checked(), marker.fallback_label()));
                }
                Item::List(_) => {
                    index = self.document.operation_end(index).saturating_add(1);
                    continue;
                }
                Item::Image(_) | Item::Media(_) => return None,
                _ => {}
            }
            index += 1;
        }
        None
    }

    fn list_item_has_text(&self, item: usize) -> bool {
        let end = self.document.operation_end(item);
        let mut index = item.saturating_add(1);
        while index < end {
            let Some(operation) = self.document.operations().get(index).copied() else {
                break;
            };
            if operation.is_close() {
                index += 1;
                continue;
            }
            let Some(node) = self.document.operation_view(index) else {
                index += 1;
                continue;
            };
            match node {
                Item::Text(text) if has_visible_inline_text(text) => return true,
                Item::List(_) | Item::TaskMarker(_) => {
                    index = self.document.operation_end(index).saturating_add(1);
                    continue;
                }
                _ => {}
            }
            index += 1;
        }
        false
    }

    fn table_has_spans(&self, table: usize) -> bool {
        let end = self.document.operation_end(table);
        let mut depth = 0usize;
        let mut index = table.saturating_add(1);
        while index < end {
            let Some(operation) = self.document.operations().get(index).copied() else {
                break;
            };
            if operation.is_close() {
                depth = depth.saturating_sub(1);
                index += 1;
                continue;
            }
            if depth == 1
                && self.document.operation_kind(index) == Some(OperationKind::TableCell)
                && let Some(Item::TableCell(cell)) = self.document.operation_view(index)
                && (cell.colspan() > 1 || cell.rowspan() > 1)
            {
                return true;
            }
            if self
                .document
                .operation_kind(index)
                .is_some_and(OperationKind::is_container)
            {
                depth += 1;
            }
            index += 1;
        }
        false
    }

    fn table_row_alignments(&self, row: usize) -> Vec<Option<TableAlignment>> {
        let mut values = Vec::new();
        let end = self.document.operation_end(row);
        let mut depth = 0usize;
        let mut index = row.saturating_add(1);
        while index < end {
            let Some(operation) = self.document.operations().get(index).copied() else {
                break;
            };
            if operation.is_close() {
                depth = depth.saturating_sub(1);
                index += 1;
                continue;
            }
            if depth == 0
                && let Some(Item::TableCell(cell)) = self.document.operation_view(index)
            {
                values.push(cell.alignment());
            }
            if self
                .document
                .operation_kind(index)
                .is_some_and(OperationKind::is_container)
            {
                depth += 1;
            }
            index += 1;
        }
        values
    }

    fn table_cell_requires_flattening(&self, cell: usize) -> bool {
        let end = self.document.operation_end(cell);
        let mut blocks = 0;
        let mut index = cell.saturating_add(1);
        while index < end {
            let Some(operation) = self.document.operations().get(index).copied() else {
                break;
            };
            if operation.is_close() {
                index += 1;
                continue;
            }
            let Some(node) = self.document.operation_view(index) else {
                index += 1;
                continue;
            };
            match node {
                Item::HardBreak
                | Item::CodeBlock(_)
                | Item::List(_)
                | Item::Table(_)
                | Item::DisplayMath(_) => return true,
                kind if is_block(&kind) => {
                    blocks += 1;
                    if blocks > 1 {
                        return true;
                    }
                }
                _ => {}
            }
            index += 1;
        }
        false
    }

    fn flatten_table_cell(&self, cell: usize) -> String {
        let mut text = String::new();
        let end = self.document.operation_end(cell);
        let mut index = cell.saturating_add(1);
        while index < end {
            let Some(operation) = self.document.operations().get(index).copied() else {
                break;
            };
            if operation.is_close() {
                index += 1;
                continue;
            }
            let Some(node) = self.document.operation_view(index) else {
                index += 1;
                continue;
            };
            match node {
                Item::Text(value) | Item::InlineCode(value) => text.push_str(value),
                Item::CodeBlock(code) => {
                    text.push(' ');
                    text.push_str(code.text());
                    text.push(' ');
                }
                Item::Image(image) if self.config.images => text.push_str(image.alt()),
                Item::HardBreak => text.push(' '),
                Item::FootnoteReference(id) => {
                    if let Some(label) = self.document.footnote_label(id) {
                        text.push_str(label);
                    }
                }
                Item::TaskMarker(marker) => {
                    if let Some(label) = marker.fallback_label() {
                        text.push_str(label);
                    }
                }
                Item::InlineMath(math) | Item::DisplayMath(math) => {
                    text.push_str(math.fallback_text().unwrap_or(math.source()));
                }
                Item::Media(media) => {
                    text.push_str(media.title().unwrap_or(media.source()));
                }
                kind => {
                    if is_block(&kind) {
                        text.push(' ');
                    }
                }
            }
            index += 1;
        }
        text
    }

    fn start_item_paragraph(&mut self) {
        if !self.out.in_empty_list_item() {
            if self.out.has_current_line_content() {
                self.out.newline();
            }
            self.out.newline();
        }
    }

    fn footnote_reference(&mut self, id: FootnoteId) {
        if let Some(label) = self.document.footnote_label(id) {
            self.out.markup("[^");
            self.out.footnote_label(label);
            self.out.markup("]");
        }
    }

    fn next_list_marker(&self) -> ListMarker {
        let list = self
            .frames
            .iter()
            .rev()
            .find(|frame| frame.kind == OperationKind::List);
        let (kind, start) = match list {
            Some(frame) => {
                if let Some(Item::List(value)) = self.document.operation_view(frame.index) {
                    (value.kind(), value.start())
                } else {
                    (ListKind::Unordered, None)
                }
            }
            None => (ListKind::Unordered, None),
        };
        let item_index = list.map_or(0, |frame| frame.list_items.saturating_sub(1));
        match kind {
            ListKind::Unordered => ListMarker::Bullet,
            ListKind::Ordered if item_index == 0 => {
                let value = start
                    .and_then(|value| i32::try_from(value).ok())
                    .filter(|value| (1..=999_999_999).contains(value))
                    .unwrap_or(1);
                if value == 1 {
                    ListMarker::Ordered
                } else {
                    ListMarker::OrderedStart(value)
                }
            }
            ListKind::Ordered => ListMarker::Ordered,
        }
    }
}
fn needs_next_text_char(text: &str) -> bool {
    text.chars()
        .next_back()
        .is_some_and(|character| matches!(character, '!' | '#' | '-' | '+' | '=' | '~' | '.' | ')'))
}

fn escape_math_source(source: &str) -> String {
    let mut escaped = String::with_capacity(source.len());
    for character in source.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '$' => escaped.push_str("\\$"),
            _ => escaped.push(character),
        }
    }
    escaped
}

fn is_block(kind: &Item) -> bool {
    matches!(
        kind,
        Item::Paragraph
            | Item::BlockGroup
            | Item::Heading { .. }
            | Item::BlockQuote
            | Item::CodeBlock(_)
            | Item::List(_)
            | Item::ListItem
            | Item::Table(_)
            | Item::TableCaption
            | Item::TableRow
            | Item::TableCell(_)
            | Item::Figure
            | Item::Figcaption
            | Item::Details
            | Item::Summary
            | Item::ThematicBreak
            | Item::DefinitionList
            | Item::DefinitionTerm
            | Item::DefinitionDescription
            | Item::Callout(_)
            | Item::FootnoteDefinition(_)
            | Item::DisplayMath(_)
    )
}

fn is_block_operation(kind: OperationKind) -> bool {
    matches!(
        kind,
        OperationKind::Paragraph
            | OperationKind::BlockGroup
            | OperationKind::Heading
            | OperationKind::BlockQuote
            | OperationKind::CodeBlock
            | OperationKind::List
            | OperationKind::ListItem
            | OperationKind::Table
            | OperationKind::TableCaption
            | OperationKind::TableRow
            | OperationKind::TableCell
            | OperationKind::Figure
            | OperationKind::Figcaption
            | OperationKind::Details
            | OperationKind::Summary
            | OperationKind::ThematicBreak
            | OperationKind::DefinitionList
            | OperationKind::DefinitionTerm
            | OperationKind::DefinitionDescription
            | OperationKind::Callout
            | OperationKind::FootnoteDefinition
            | OperationKind::DisplayMath
    )
}

pub(crate) fn has_visible_inline_text(text: &str) -> bool {
    crate::document::stats::has_visible_inline_text(text)
}

struct Output<'writer> {
    writer: &'writer mut dyn fmt::Write,
    value: String,
    error: Option<fmt::Error>,
    has_output: bool,
    pending_newlines: usize,
    pending_space: bool,
    last_text_char: Option<char>,
    inline_boundary: bool,
    line_start: bool,
    trailing_newlines: usize,
    prefixes: SmallVec<[Prefix; 8]>,
    markers: SmallVec<[Marker; 4]>,
    first_unopened_marker: usize,
    line_text_state: LineTextState,
    hash_run_start: Option<usize>,
}

#[derive(Clone, Copy)]
enum LineTextState {
    Start,
    Digits(u8),
    Hashes(u8),
    Other,
    Tildes(u8),
}

#[derive(Clone, Copy)]
enum Prefix {
    Quote,
    Indent(usize),
    ListItem { width: usize, has_content: bool },
}

struct Marker {
    value: &'static str,
    opened: bool,
}

impl<'writer> Output<'writer> {
    fn new<W: fmt::Write>(writer: &'writer mut W) -> Self {
        Self {
            writer,
            value: String::with_capacity(512),
            error: None,
            has_output: false,
            pending_newlines: 0,
            pending_space: false,
            last_text_char: None,
            inline_boundary: false,
            line_start: true,
            trailing_newlines: 0,
            prefixes: SmallVec::new(),
            markers: SmallVec::new(),
            first_unopened_marker: 0,
            line_text_state: LineTextState::Start,
            hash_run_start: None,
        }
    }

    fn finish(mut self) -> fmt::Result {
        self.resolve_hash_run(true);
        self.value
            .truncate(self.value.trim_end_matches([' ', '\t', '\r', '\n']).len());
        self.flush_line();
        self.pending_newlines = 0;
        if self.has_output {
            self.write_str("\n");
        }
        self.error.map_or(Ok(()), Err)
    }

    fn write_str(&mut self, value: &str) {
        if self.error.is_none()
            && let Err(error) = self.writer.write_str(value)
        {
            self.error = Some(error);
        }
    }

    fn flush_pending_newlines(&mut self) {
        for _ in 0..self.pending_newlines {
            self.write_str("\n");
        }
        self.pending_newlines = 0;
    }

    fn flush_line(&mut self) {
        if self.value.is_empty() {
            return;
        }
        self.flush_pending_newlines();
        let value = std::mem::take(&mut self.value);
        self.write_str(&value);
        self.has_output = true;
    }

    fn has_current_line_content(&self) -> bool {
        !self.line_start
    }

    fn prefix(&mut self) {
        if !self.line_start {
            return;
        }
        self.flush_pending_newlines();
        for prefix in &self.prefixes {
            match prefix {
                Prefix::Quote => self.value.push_str("> "),
                Prefix::Indent(width) => self.value.extend(std::iter::repeat_n(' ', *width)),
                Prefix::ListItem { width, .. } => {
                    self.value.extend(std::iter::repeat_n(' ', *width));
                }
            }
        }
        self.line_start = false;
        self.trailing_newlines = 0;
    }

    fn flush_space(&mut self) {
        if self.pending_space && !self.line_start {
            self.value.push(' ');
        }
        self.pending_space = false;
    }

    fn text(&mut self, text: &str, next_text_char: Option<char>) {
        if text.is_ascii() {
            self.ascii_text(text, next_text_char);
            return;
        }

        let mut prepared = false;
        for (index, ch) in text.char_indices() {
            if ch.is_whitespace() {
                self.resolve_hash_run(true);
                self.pending_space |= !self.line_start;
                if matches!(
                    self.line_text_state,
                    LineTextState::Digits(_) | LineTextState::Hashes(_) | LineTextState::Tildes(_)
                ) {
                    self.line_text_state = LineTextState::Other;
                }
                continue;
            }
            if matches!(self.line_text_state, LineTextState::Hashes(_)) && ch != '#' {
                self.resolve_hash_run(false);
            }
            if prepared {
                self.flush_space();
            } else {
                self.prepare_text(ch);
                prepared = true;
            }

            let escape = self.should_escape_char(ch, &text[index..], next_text_char);
            if ch == '#' && matches!(self.line_text_state, LineTextState::Start) && !escape {
                self.hash_run_start = Some(self.value.len());
            }
            if escape {
                self.value.push('\\');
            }
            self.value.push(ch);
            self.last_text_char = Some(ch);
            self.advance_line_text_state(ch);
        }
    }

    fn ascii_text(&mut self, text: &str, next_text_char: Option<char>) {
        let bytes = text.as_bytes();
        let mut index = 0;
        let mut prepared = false;
        while index < bytes.len() {
            while index < bytes.len() && bytes[index].is_ascii_whitespace() {
                self.resolve_hash_run(true);
                self.pending_space |= !self.line_start;
                if matches!(
                    self.line_text_state,
                    LineTextState::Digits(_) | LineTextState::Hashes(_) | LineTextState::Tildes(_)
                ) {
                    self.line_text_state = LineTextState::Other;
                }
                index += 1;
            }
            if index == bytes.len() {
                break;
            }
            if matches!(self.line_text_state, LineTextState::Hashes(_)) && bytes[index] != b'#' {
                self.resolve_hash_run(false);
            }

            if prepared {
                self.flush_space();
            } else {
                self.prepare_text(bytes[index] as char);
                prepared = true;
            }
            self.ascii_text_run(text, &mut index, next_text_char);
        }
    }

    /// Writes one non-whitespace run and leaves `index` at its end.
    fn ascii_text_run(&mut self, text: &str, index: &mut usize, next_text_char: Option<char>) {
        let bytes = text.as_bytes();

        while *index < bytes.len() && !bytes[*index].is_ascii_whitespace() {
            // Once the line state is `Other`, ordinary prose cannot affect any
            // Markdown construct. Copy it as one slice instead of processing
            // every byte through the escape and line-state checks.
            if matches!(self.line_text_state, LineTextState::Other) {
                let start = *index;
                while *index < bytes.len()
                    && !bytes[*index].is_ascii_whitespace()
                    && !matches!(
                        bytes[*index],
                        b'!' | b'\\' | b'`' | b'*' | b'_' | b'[' | b']' | b'<' | b'>' | b'|'
                    )
                {
                    *index += 1;
                }
                if start != *index {
                    self.value.push_str(&text[start..*index]);
                    self.last_text_char = Some(bytes[*index - 1] as char);
                    continue;
                }
            }

            let byte = bytes[*index];
            let escape = self.should_escape_char(byte as char, &text[*index..], next_text_char);
            if byte == b'#' && matches!(self.line_text_state, LineTextState::Start) && !escape {
                self.hash_run_start = Some(self.value.len());
            }
            if escape {
                self.value.push('\\');
            }
            self.value.push(byte as char);
            self.last_text_char = Some(byte as char);
            self.advance_line_text_state(byte as char);
            *index += 1;
        }
    }

    fn prepare_text(&mut self, first: char) {
        self.prepare_inline_boundary(Some(first));
        self.prefix();
        self.flush_space();
        self.open_pending_markers();
        self.mark_list_item_content();
    }

    fn prepare_inline_boundary(&mut self, first: Option<char>) {
        if !self.inline_boundary {
            return;
        }
        self.inline_boundary = false;
        if self.pending_space || self.line_start {
            return;
        }
        if let (Some(previous), Some(first)) = (self.last_text_char, first)
            && is_word_like(previous)
            && is_word_like(first)
        {
            self.pending_space = true;
        }
    }

    fn mark_inline_boundary(&mut self) {
        self.inline_boundary = true;
    }

    fn should_escape_char(&self, ch: char, remaining: &str, next_text_char: Option<char>) -> bool {
        let next = remaining[ch.len_utf8()..].chars().next().or(next_text_char);
        match ch {
            '!' => next == Some('['),
            '#' => {
                matches!(self.line_text_state, LineTextState::Start)
                    && is_atx_heading_marker(remaining, next_text_char)
            }
            '-' | '+' => {
                matches!(self.line_text_state, LineTextState::Start)
                    && is_unordered_list_marker(remaining, ch, next_text_char)
            }
            '=' => {
                matches!(self.line_text_state, LineTextState::Start)
                    && is_setext_underline(remaining)
            }
            '~' => {
                (matches!(self.line_text_state, LineTextState::Start) && is_tilde_fence(remaining))
                    || matches!(self.line_text_state, LineTextState::Tildes(count) if count >= 2)
            }
            '.' | ')' => {
                matches!(self.line_text_state, LineTextState::Digits(count) if count <= 9)
                    && next.is_none_or(char::is_whitespace)
            }
            _ if ch.is_ascii() => markdown_escape_byte(ch as u8),
            _ => markdown_escape(ch),
        }
    }

    fn advance_line_text_state(&mut self, ch: char) {
        self.line_text_state = match self.line_text_state {
            LineTextState::Start if ch.is_ascii_digit() => LineTextState::Digits(1),
            LineTextState::Digits(count) if ch.is_ascii_digit() => {
                LineTextState::Digits(count.saturating_add(1))
            }
            LineTextState::Start if ch == '#' => LineTextState::Hashes(1),
            LineTextState::Hashes(count) if ch == '#' => {
                LineTextState::Hashes(count.saturating_add(1))
            }
            LineTextState::Start if ch == '~' => LineTextState::Tildes(1),
            LineTextState::Tildes(count) if ch == '~' => {
                LineTextState::Tildes(count.saturating_add(1))
            }
            _ => LineTextState::Other,
        };
    }

    fn resolve_hash_run(&mut self, escape: bool) {
        let Some(start) = self.hash_run_start.take() else {
            return;
        };
        if escape && matches!(self.line_text_state, LineTextState::Hashes(count) if count <= 6) {
            self.value.insert(start, '\\');
        }
    }

    fn label(&mut self, text: &str) {
        for ch in text.chars() {
            if ch.is_whitespace() {
                if !self.value.ends_with(' ') {
                    self.value.push(' ');
                }
                continue;
            }
            if matches!(ch, '\\' | '`' | '*' | '_' | '[' | ']' | '<' | '>') {
                self.value.push('\\');
            }
            self.value.push(ch);
        }
    }

    fn table_label(&mut self, text: &str) {
        for ch in text.chars() {
            if ch.is_whitespace() {
                if !self.value.ends_with(' ') {
                    self.value.push(' ');
                }
                continue;
            }
            if matches!(ch, '\\' | '`' | '*' | '_' | '[' | ']' | '<' | '>' | '|') {
                self.value.push('\\');
            }
            self.value.push(ch);
        }
    }

    fn footnote_label(&mut self, text: &str) {
        for ch in text.chars() {
            if ch.is_whitespace() {
                if !self.value.ends_with(' ') {
                    self.value.push(' ');
                }
            } else {
                if matches!(ch, '\\' | ']') {
                    self.value.push('\\');
                }
                self.value.push(ch);
            }
        }
    }

    fn destination(&mut self, value: &str) {
        for ch in value.chars() {
            match ch {
                '\\' | '(' | ')' => {
                    self.value.push('\\');
                    self.value.push(ch);
                }
                ' ' => self.value.push_str("%20"),
                '\n' => self.value.push_str("%0A"),
                '\r' => self.value.push_str("%0D"),
                '\t' => self.value.push_str("%09"),
                '<' => self.value.push_str("%3C"),
                '>' => self.value.push_str("%3E"),
                _ => self.value.push(ch),
            }
        }
    }

    fn link_title(&mut self, value: &str) {
        let mut pending_space = false;
        for ch in value.chars() {
            if ch.is_whitespace() {
                pending_space = true;
                continue;
            }
            if pending_space {
                self.value.push(' ');
                pending_space = false;
            }
            if matches!(ch, '\\' | '"') {
                self.value.push('\\');
            }
            self.value.push(ch);
        }
    }

    fn markup(&mut self, value: &str) {
        self.prepare_markup();
        self.value.push_str(value);
    }

    fn markup_repeat(&mut self, value: char, count: usize) {
        self.prepare_markup();
        self.value.extend(std::iter::repeat_n(value, count));
    }

    fn markup_number(&mut self, value: i32) {
        use std::fmt::Write;

        self.prepare_markup();
        write!(self.value, "{value}").expect("writing to a String cannot fail");
    }

    fn prepare_markup(&mut self) {
        self.resolve_hash_run(false);
        self.flush_space();
        self.prefix();
        self.open_pending_markers();
        self.line_text_state = LineTextState::Other;
    }

    fn open_marker(&mut self, value: &'static str) {
        self.resolve_hash_run(false);
        self.markers.push(Marker {
            value,
            opened: false,
        });
    }

    fn open_link(&mut self) {
        if self.last_text_char == Some('!') && !self.pending_space && self.value.ends_with('!') {
            let bang_index = self.value.len() - 1;
            let backslashes = self.value[..bang_index]
                .bytes()
                .rev()
                .take_while(|&byte| byte == b'\\')
                .count();
            if backslashes % 2 == 0 {
                self.value.insert(bang_index, '\\');
            }
        }
        self.open_marker("[");
    }

    fn begin_list_item_content(&mut self) {
        self.line_text_state = LineTextState::Start;
    }

    fn open_pending_markers(&mut self) {
        while let Some(marker) = self.markers.get_mut(self.first_unopened_marker) {
            self.value.push_str(marker.value);
            marker.opened = true;
            self.first_unopened_marker += 1;
            self.line_text_state = LineTextState::Other;
        }
    }

    fn close_marker(&mut self, opening: &str, closing: &str) -> bool {
        let marker = self.markers.pop().expect("marker close without an open");
        self.first_unopened_marker = self.first_unopened_marker.min(self.markers.len());
        debug_assert_eq!(marker.value, opening);
        if marker.opened {
            self.value.push_str(closing);
        }
        marker.opened
    }

    fn begin_verbatim(&mut self) {
        self.flush_space();
        self.prefix();
    }

    fn verbatim(&mut self, value: &str) {
        self.flush_space();
        for part in value.split_inclusive('\n') {
            self.prefix();
            let line = part.strip_suffix('\n').unwrap_or(part);
            self.value.push_str(line);
            if part.ends_with('\n') {
                self.flush_line();
                self.pending_newlines += 1;
                self.line_start = true;
                self.trailing_newlines += 1;
            }
        }
    }

    fn newline(&mut self) {
        self.resolve_hash_run(true);
        self.pending_space = false;
        self.last_text_char = None;
        self.inline_boundary = false;
        if self.line_start {
            let prefix_start = self.value.len();
            for prefix in &self.prefixes {
                match prefix {
                    Prefix::Quote => self.value.push_str("> "),
                    Prefix::Indent(width) => self.value.extend(std::iter::repeat_n(' ', *width)),
                    Prefix::ListItem { width, .. } => {
                        self.value.extend(std::iter::repeat_n(' ', *width));
                    }
                }
            }
            while self.value.ends_with(' ') {
                self.value.pop();
            }
            if !self.value[prefix_start..].contains('>') {
                self.value.truncate(prefix_start);
            }
        }
        self.flush_line();
        self.pending_newlines += 1;
        self.line_start = true;
        self.line_text_state = LineTextState::Start;
        self.trailing_newlines += 1;
    }

    fn limit_trailing_newlines(&mut self, maximum: usize) {
        self.trailing_newlines = self.trailing_newlines.min(maximum);
        self.pending_newlines = self.pending_newlines.min(maximum);
    }

    fn ensure_blank_line(&mut self) {
        if !self.has_output && self.value.is_empty() {
            return;
        }
        while self.trailing_newlines < 2 {
            self.newline();
        }
    }

    fn in_empty_list_item(&self) -> bool {
        self.prefixes.iter().rev().find_map(|prefix| match prefix {
            Prefix::ListItem { has_content, .. } => Some(!has_content),
            Prefix::Quote => None,
            Prefix::Indent(_) => None,
        }) == Some(true)
    }

    fn mark_list_item_content(&mut self) {
        for prefix in &mut self.prefixes {
            if let Prefix::ListItem { has_content, .. } = prefix {
                *has_content = true;
            }
        }
    }

    fn hard_break(&mut self) {
        self.resolve_hash_run(true);
        self.pending_space = false;
        self.prefix();
        self.value.push_str("  ");
        self.flush_line();
        self.pending_newlines = 1;
        self.line_start = true;
        self.line_text_state = LineTextState::Start;
        self.trailing_newlines += 1;
    }
}

fn is_atx_heading_marker(value: &str, next_text_char: Option<char>) -> bool {
    let hashes = value.chars().take_while(|&ch| ch == '#').count();
    if !(1..=6).contains(&hashes) {
        return false;
    }
    value[hashes..]
        .chars()
        .next()
        .or(next_text_char)
        .is_none_or(char::is_whitespace)
}

fn is_unordered_list_marker(value: &str, marker: char, next_text_char: Option<char>) -> bool {
    let count = value.chars().take_while(|&ch| ch == marker).count();
    let rest = &value[count..];
    if marker == '-' && count >= 3 && rest.chars().all(char::is_whitespace) {
        return true;
    }
    count == 1
        && rest
            .chars()
            .next()
            .or(next_text_char)
            .is_none_or(char::is_whitespace)
}

fn is_setext_underline(value: &str) -> bool {
    let count = value.chars().take_while(|&ch| ch == '=').count();
    count > 0 && value[count..].chars().all(char::is_whitespace)
}

fn is_tilde_fence(value: &str) -> bool {
    value.chars().take_while(|&ch| ch == '~').count() >= 3
}

fn markdown_escape(ch: char) -> bool {
    matches!(ch, '\\' | '`' | '*' | '_' | '[' | ']' | '<' | '>' | '|')
}

fn markdown_escape_byte(byte: u8) -> bool {
    matches!(
        byte,
        b'\\' | b'`' | b'*' | b'_' | b'[' | b']' | b'<' | b'>' | b'|'
    )
}

fn scan_longest_run(bytes: &[u8], needle: u8, longest: &mut usize, current: &mut usize) {
    for &byte in bytes {
        if byte == needle {
            *current += 1;
            *longest = (*longest).max(*current);
        } else {
            *current = 0;
        }
    }
}

#[derive(Clone, Copy, Default)]
struct CollapsedText {
    longest_backtick_run: usize,
    current_backtick_run: usize,
    starts_with_backtick: bool,
    ends_with_backtick: bool,
    has_content: bool,
    first_char: Option<char>,
    pending_whitespace: bool,
}

impl CollapsedText {
    fn scan(&mut self, value: &str) {
        for ch in value.chars() {
            if ch.is_whitespace() {
                self.pending_whitespace = true;
                self.current_backtick_run = 0;
                continue;
            }
            if !self.has_content {
                self.starts_with_backtick = !self.pending_whitespace && ch == '`';
                self.first_char = Some(ch);
                self.has_content = true;
            }
            self.ends_with_backtick = ch == '`';
            if ch == '`' {
                self.current_backtick_run += 1;
                self.longest_backtick_run =
                    self.longest_backtick_run.max(self.current_backtick_run);
            } else {
                self.current_backtick_run = 0;
            }
            self.pending_whitespace = false;
        }
    }
}

struct CollapsedTextWriter {
    empty: bool,
    pending_whitespace: bool,
    last_char: Option<char>,
}

impl Default for CollapsedTextWriter {
    fn default() -> Self {
        Self {
            empty: true,
            pending_whitespace: false,
            last_char: None,
        }
    }
}

impl CollapsedTextWriter {
    fn write(&mut self, out: &mut String, value: &str) {
        for ch in value.chars() {
            if ch.is_whitespace() {
                self.pending_whitespace = true;
                continue;
            }
            if self.pending_whitespace {
                out.push(' ');
            }
            out.push(ch);
            self.empty = false;
            self.last_char = Some(ch);
            self.pending_whitespace = false;
        }
    }
}

fn decimal_len(value: i32) -> usize {
    debug_assert!(value > 0);
    value.ilog10() as usize + 1
}

fn is_word_like(value: char) -> bool {
    value.is_alphanumeric()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{
        CodeBlock, Image, List, ListKind, MathFormat, MathValue, SemanticKind as Item,
        SemanticTapeBuilder, Table, TableCell, TaskMarker,
    };

    #[test]
    fn semantic_code_chooses_safe_delimiters() {
        let mut builder = SemanticTapeBuilder::with_capacity(3);
        let paragraph = builder.emit(None, Item::Paragraph).unwrap();
        builder.append_inline_code(Some(paragraph), "a`b").unwrap();
        builder.close(paragraph).unwrap();
        builder
            .emit(
                None,
                Item::CodeBlock(CodeBlock {
                    language: Some("rust".into()),
                    text: "let fence = ```;\n".into(),
                }),
            )
            .unwrap();
        let document = builder.finish().unwrap();
        assert_eq!(
            render_markdown(&document, 0, MarkdownConfig::default()),
            "``a`b``\n\n````rust\nlet fence = ```;\n````\n"
        );
    }

    #[test]
    fn sibling_tasks_preserve_nested_inline_order() {
        let mut builder = SemanticTapeBuilder::with_capacity(6);
        let paragraph = builder.emit(None, Item::Paragraph).unwrap();
        builder.append_prose(Some(paragraph), "first").unwrap();
        let emphasis = builder.emit(Some(paragraph), Item::Emphasis).unwrap();
        builder.append_prose(Some(emphasis), "second").unwrap();
        builder.close(emphasis).unwrap();
        builder.append_prose(Some(paragraph), " third").unwrap();
        builder.close(paragraph).unwrap();
        let trailing = builder.emit(None, Item::Paragraph).unwrap();
        builder.append_prose(Some(trailing), "next").unwrap();
        builder.close(trailing).unwrap();

        assert_eq!(
            render_markdown(&builder.finish().unwrap(), 0, MarkdownConfig::default()),
            "first *second* third\n\nnext\n"
        );
    }

    #[test]
    fn image_only_heading_without_alt_degrades_to_an_image() {
        let mut builder = SemanticTapeBuilder::with_capacity(2);
        let heading = builder.emit(None, Item::Heading { level: 2 }).unwrap();
        builder
            .emit(
                Some(heading),
                Item::Image(Image {
                    source: "diagram.png".into(),
                    alt: "".into(),
                    title: None,
                    width: None,
                    height: None,
                }),
            )
            .unwrap();
        builder.close(heading).unwrap();
        assert_eq!(
            render_markdown(&builder.finish().unwrap(), 0, MarkdownConfig::default()),
            "![](diagram.png)\n"
        );
    }

    #[test]
    fn task_markers_must_be_the_first_visible_item_content() {
        let mut builder = SemanticTapeBuilder::with_capacity(8);
        let list = builder
            .emit(
                None,
                Item::List(List {
                    kind: ListKind::Unordered,
                    start: None,
                }),
            )
            .unwrap();
        let trailing = builder.emit(Some(list), Item::ListItem).unwrap();
        builder.append_prose(Some(trailing), "Optional ").unwrap();
        builder
            .emit(
                Some(trailing),
                Item::TaskMarker(TaskMarker {
                    checked: true,
                    fallback_label: None,
                }),
            )
            .unwrap();
        builder.close(trailing).unwrap();
        let multiple = builder.emit(Some(list), Item::ListItem).unwrap();
        builder
            .emit(
                Some(multiple),
                Item::TaskMarker(TaskMarker {
                    checked: false,
                    fallback_label: None,
                }),
            )
            .unwrap();
        builder
            .emit(
                Some(multiple),
                Item::TaskMarker(TaskMarker {
                    checked: true,
                    fallback_label: None,
                }),
            )
            .unwrap();
        builder.append_prose(Some(multiple), "First").unwrap();
        builder.close(multiple).unwrap();
        builder.close(list).unwrap();
        assert_eq!(
            render_markdown(&builder.finish().unwrap(), 0, MarkdownConfig::default()),
            "- Optional\n- [ ] First\n"
        );
    }

    #[test]
    fn math_cannot_emit_raw_html_or_close_its_delimiter() {
        let mut builder = SemanticTapeBuilder::with_capacity(2);
        let paragraph = builder.emit(None, Item::Paragraph).unwrap();
        builder
            .emit(
                Some(paragraph),
                Item::InlineMath(MathValue {
                    source: "x $ <img src=x onerror=x>".into(),
                    format: MathFormat::Tex,
                    fallback_text: None,
                }),
            )
            .unwrap();
        builder.close(paragraph).unwrap();
        let markdown = render_markdown(&builder.finish().unwrap(), 0, MarkdownConfig::default());
        assert_eq!(markdown, "$x \\$ &lt;img src=x onerror=x&gt;$\n");
        assert!(!markdown.contains("<img"));
    }

    #[test]
    fn complex_table_cells_flatten_to_valid_gfm_lines_and_keep_the_caption() {
        let mut builder = SemanticTapeBuilder::with_capacity(10);
        let table = builder
            .emit(
                None,
                Item::Table(Table {
                    column_count: Some(1),
                }),
            )
            .unwrap();
        let caption = builder.emit(Some(table), Item::TableCaption).unwrap();
        builder.append_prose(Some(caption), "Measurements").unwrap();
        builder.close(caption).unwrap();
        let row = builder.emit(Some(table), Item::TableRow).unwrap();
        let cell = builder
            .emit(
                Some(row),
                Item::TableCell(TableCell {
                    header: false,
                    colspan: 1,
                    rowspan: 1,
                    alignment: None,
                }),
            )
            .unwrap();
        let first = builder.emit(Some(cell), Item::Paragraph).unwrap();
        builder.append_prose(Some(first), "first").unwrap();
        builder.close(first).unwrap();
        let second = builder.emit(Some(cell), Item::Paragraph).unwrap();
        builder.append_prose(Some(second), "second").unwrap();
        builder.close(second).unwrap();
        builder.close(cell).unwrap();
        builder.close(row).unwrap();
        builder.close(table).unwrap();
        let markdown = render_markdown(&builder.finish().unwrap(), 0, MarkdownConfig::default());
        assert_eq!(markdown, "Measurements\n\n| first second |\n| --- |\n");
    }

    #[test]
    fn table_images_and_code_escape_column_delimiters() {
        let mut builder = SemanticTapeBuilder::with_capacity(6);
        let table = builder
            .emit(
                None,
                Item::Table(Table {
                    column_count: Some(2),
                }),
            )
            .unwrap();
        let row = builder.emit(Some(table), Item::TableRow).unwrap();
        let image_cell = builder
            .emit(
                Some(row),
                Item::TableCell(TableCell {
                    header: false,
                    colspan: 1,
                    rowspan: 1,
                    alignment: None,
                }),
            )
            .unwrap();
        builder
            .emit(
                Some(image_cell),
                Item::Image(Image {
                    source: "diagram.png".into(),
                    alt: "A|B".into(),
                    title: None,
                    width: None,
                    height: None,
                }),
            )
            .unwrap();
        builder.close(image_cell).unwrap();
        let code_cell = builder
            .emit(
                Some(row),
                Item::TableCell(TableCell {
                    header: false,
                    colspan: 1,
                    rowspan: 1,
                    alignment: None,
                }),
            )
            .unwrap();
        builder.append_inline_code(Some(code_cell), "a|b").unwrap();
        builder.close(code_cell).unwrap();
        builder.close(row).unwrap();
        builder.close(table).unwrap();
        let markdown = render_markdown(&builder.finish().unwrap(), 0, MarkdownConfig::default());
        assert!(markdown.contains("![A\\|B](diagram.png)"), "{markdown}");
        assert!(markdown.contains("`a\\|b`"), "{markdown}");
    }

    #[test]
    fn spanning_tables_degrade_without_invalid_gfm_columns() {
        let mut builder = SemanticTapeBuilder::with_capacity(5);
        let table = builder
            .emit(
                None,
                Item::Table(Table {
                    column_count: Some(2),
                }),
            )
            .unwrap();
        let row = builder.emit(Some(table), Item::TableRow).unwrap();
        let cell = builder
            .emit(
                Some(row),
                Item::TableCell(TableCell {
                    header: true,
                    colspan: 2,
                    rowspan: 1,
                    alignment: None,
                }),
            )
            .unwrap();
        builder.append_prose(Some(cell), "Wide heading").unwrap();
        builder.close(cell).unwrap();
        builder.close(row).unwrap();
        builder.close(table).unwrap();
        let markdown = render_markdown(&builder.finish().unwrap(), 0, MarkdownConfig::default());
        assert_eq!(markdown, "Wide heading\n");
        assert!(!markdown.contains("| ---"));
    }

    #[test]
    fn nested_spanning_tables_do_not_degrade_the_outer_table() {
        let mut builder = SemanticTapeBuilder::with_capacity(10);
        let outer = builder
            .emit(
                None,
                Item::Table(Table {
                    column_count: Some(1),
                }),
            )
            .unwrap();
        let outer_row = builder.emit(Some(outer), Item::TableRow).unwrap();
        let outer_cell = builder
            .emit(
                Some(outer_row),
                Item::TableCell(TableCell {
                    header: false,
                    colspan: 1,
                    rowspan: 1,
                    alignment: None,
                }),
            )
            .unwrap();
        let nested = builder
            .emit(
                Some(outer_cell),
                Item::Table(Table {
                    column_count: Some(2),
                }),
            )
            .unwrap();
        let nested_row = builder.emit(Some(nested), Item::TableRow).unwrap();
        let nested_cell = builder
            .emit(
                Some(nested_row),
                Item::TableCell(TableCell {
                    header: true,
                    colspan: 2,
                    rowspan: 1,
                    alignment: None,
                }),
            )
            .unwrap();
        builder.append_prose(Some(nested_cell), "Nested").unwrap();
        builder.close(nested_cell).unwrap();
        builder.close(nested_row).unwrap();
        builder.close(nested).unwrap();
        builder.close(outer_cell).unwrap();
        builder.close(outer_row).unwrap();
        builder.close(outer).unwrap();

        let markdown = render_markdown(&builder.finish().unwrap(), 0, MarkdownConfig::default());
        assert!(markdown.contains("| Nested |"), "{markdown}");
        assert!(markdown.contains("| --- |"), "{markdown}");
    }

    #[test]
    fn list_item_numbering_ignores_direct_footnote_children() {
        let mut builder = SemanticTapeBuilder::with_capacity(6);
        let list = builder
            .emit(
                None,
                Item::List(List {
                    kind: ListKind::Ordered,
                    start: Some(5),
                }),
            )
            .unwrap();
        let footnote_id = crate::document::FootnoteId::from_index(0).unwrap();
        let definition = builder
            .emit(Some(list), Item::FootnoteDefinition(footnote_id))
            .unwrap();
        let definition_paragraph = builder.emit(Some(definition), Item::Paragraph).unwrap();
        builder
            .append_prose(Some(definition_paragraph), "Note")
            .unwrap();
        builder
            .define_footnote(footnote_id, "note", definition)
            .unwrap();
        builder.close(definition_paragraph).unwrap();
        builder.close(definition).unwrap();
        let item = builder.emit(Some(list), Item::ListItem).unwrap();
        builder.append_prose(Some(item), "Item").unwrap();
        builder.close(item).unwrap();
        builder.close(list).unwrap();

        let markdown = render_markdown(&builder.finish().unwrap(), 0, MarkdownConfig::default());
        assert!(markdown.contains("5. Item"), "{markdown}");
    }

    #[test]
    fn deeply_nested_formatting_with_many_text_runs_is_linear() {
        const DEPTH: usize = 5_000;
        let mut builder = SemanticTapeBuilder::with_capacity(DEPTH * 2 + 1);
        let paragraph = builder.emit(None, Item::Paragraph).unwrap();
        let mut parent = Some(paragraph);
        let mut formatting_nodes = Vec::with_capacity(DEPTH);
        for _ in 0..DEPTH {
            let formatting = builder.emit(parent, Item::Emphasis).unwrap();
            builder.append_prose(Some(formatting), "x ").unwrap();
            formatting_nodes.push(formatting);
            parent = Some(formatting);
        }
        builder.append_prose(parent, "end").unwrap();
        for formatting in formatting_nodes.into_iter().rev() {
            builder.close(formatting).unwrap();
        }
        builder.close(paragraph).unwrap();
        let markdown = render_markdown(&builder.finish().unwrap(), 0, MarkdownConfig::default());
        assert!(markdown.contains("end"));
    }

    #[test]
    fn deeply_nested_formatting_is_linear_and_stack_safe() {
        const DEPTH: usize = 10_000;
        let mut builder = SemanticTapeBuilder::with_capacity(DEPTH + 2);
        let paragraph = builder.emit(None, Item::Paragraph).unwrap();
        let mut parent = Some(paragraph);
        let mut formatting_nodes = Vec::with_capacity(DEPTH);
        for _ in 0..DEPTH {
            let emphasis = builder.emit(parent, Item::Emphasis).unwrap();
            formatting_nodes.push(emphasis);
            parent = Some(emphasis);
        }
        builder.append_prose(parent, "deep").unwrap();
        for formatting in formatting_nodes.into_iter().rev() {
            builder.close(formatting).unwrap();
        }
        builder.close(paragraph).unwrap();
        let document = builder.finish().unwrap();
        let markdown = render_markdown(&document, 0, MarkdownConfig::default());
        assert!(markdown.contains("deep"));
    }
}
