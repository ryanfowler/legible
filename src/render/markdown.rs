//! CommonMark/GFM rendering from the semantic document.
//!
//! The renderer uses an explicit task stack. It has no dependency on the HTML DOM.

use smallvec::SmallVec;
use std::collections::HashMap;

use crate::document::{Document, DocumentNodeId, FootnoteId, ListKind, NodeKind, TableAlignment};

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
enum Mode {
    Block,
    Inline,
}

#[derive(Clone, Copy)]
enum ListMarker {
    Bullet,
    Ordered,
    OrderedStart(i32),
}

enum Task {
    Node(DocumentNodeId, Mode),
    Close(Close),
    ListItem(DocumentNodeId, ListMarker),
    ItemParagraph(DocumentNodeId),
    TableCell(DocumentNodeId),
}

enum Close {
    Block,
    Marker(&'static str),
    Link(DocumentNodeId),
    Quote,
    List,
    ListItem,
    TableRow,
    TableHeader(Vec<Option<TableAlignment>>),
    Table,
    Footnote,
}

pub(crate) fn render_markdown(
    document: &Document,
    capacity: usize,
    config: MarkdownConfig,
) -> String {
    MarkdownRenderer::new(document, capacity, config).render()
}

struct MarkdownRenderer<'a> {
    document: &'a Document,
    out: Output,
    tasks: Vec<Task>,
    list_depth: usize,
    table_depth: usize,
    visible: HashMap<DocumentNodeId, bool>,
    config: MarkdownConfig,
}

impl<'a> MarkdownRenderer<'a> {
    fn new(document: &'a Document, capacity: usize, config: MarkdownConfig) -> Self {
        let visible = compute_visibility(document, config.images);
        Self {
            document,
            out: Output::new(capacity),
            tasks: Vec::with_capacity(32),
            list_depth: 0,
            table_depth: 0,
            visible,
            config,
        }
    }

    fn render(mut self) -> String {
        let roots: SmallVec<[_; 16]> = self.document.root_ids().collect();
        self.tasks.extend(
            roots
                .into_iter()
                .rev()
                .map(|id| Task::Node(id, Mode::Block)),
        );
        while let Some(task) = self.tasks.pop() {
            match task {
                Task::Node(id, mode) => self.node(id, mode),
                Task::Close(close) => self.close(close),
                Task::ListItem(id, marker) => self.list_item(id, marker),
                Task::ItemParagraph(id) => self.item_paragraph(id),
                Task::TableCell(id) => self.table_cell(id),
            }
        }
        self.out.finish()
    }

    fn push_children(&mut self, id: DocumentNodeId, mode: Mode) {
        let children: SmallVec<[_; 16]> = self.document.child_ids(id).collect();
        self.tasks.extend(
            children
                .into_iter()
                .rev()
                .map(|child| Task::Node(child, mode)),
        );
    }

    fn block_contains_only_footnotes(&self, id: DocumentNodeId) -> bool {
        let mut children = self.document.child_ids(id).peekable();
        children.peek().is_some()
            && children.all(|child| {
                matches!(
                    self.document.node(child).map(|node| node.kind()),
                    Some(NodeKind::FootnoteDefinition(_))
                )
            })
    }

    fn visible(&self, root: DocumentNodeId) -> bool {
        self.visible.get(&root).copied().unwrap_or(false)
    }

    fn next_text_char(&self, id: DocumentNodeId) -> Option<char> {
        let mut sibling = self.document.next_sibling(id);
        while let Some(node) = sibling {
            if let Some(value) = self.first_text_char(node) {
                return Some(value);
            }
            sibling = self.document.next_sibling(node);
        }
        None
    }

    fn first_text_char(&self, root: DocumentNodeId) -> Option<char> {
        let mut nodes = SmallVec::<[DocumentNodeId; 8]>::new();
        nodes.push(root);
        while let Some(id) = nodes.pop() {
            let node = self.document.node(id)?;
            match node.kind() {
                NodeKind::Text(text) => {
                    if let Some(ch) = text.chars().next() {
                        return Some(ch);
                    }
                }
                NodeKind::Image(_) if self.config.images => return Some('!'),
                kind if is_block(kind) => return None,
                _ => {
                    let children: SmallVec<[_; 8]> = self.document.child_ids(id).collect();
                    nodes.extend(children.into_iter().rev());
                }
            }
        }
        None
    }

    fn node(&mut self, id: DocumentNodeId, mode: Mode) {
        let Some(node) = self.document.node(id) else {
            return;
        };
        match node.kind() {
            NodeKind::Text(text) => self.out.text(text, self.next_text_char(id)),
            NodeKind::Heading { level } => {
                if !self.visible(id) {
                    if self.config.images {
                        self.push_children(id, Mode::Block);
                    }
                    return;
                }
                self.out.ensure_blank_line();
                self.out.markup_repeat('#', usize::from(*level));
                self.out.markup(" ");
                self.tasks.push(Task::Close(Close::Block));
                self.push_children(id, Mode::Inline);
            }
            NodeKind::Paragraph => {
                if matches!(mode, Mode::Block) {
                    self.out.ensure_blank_line();
                    self.tasks.push(Task::Close(Close::Block));
                }
                self.push_children(id, Mode::Inline);
            }
            NodeKind::TableCaption
            | NodeKind::Figcaption
            | NodeKind::DefinitionTerm
            | NodeKind::DefinitionDescription
            | NodeKind::Summary => {
                self.out.ensure_blank_line();
                self.tasks.push(Task::Close(Close::Block));
                self.push_children(id, Mode::Inline);
            }
            NodeKind::BlockGroup => {
                if self.block_contains_only_footnotes(id) {
                    self.out.limit_trailing_newlines(3);
                    self.push_children(id, Mode::Block);
                } else {
                    if !self.out.in_empty_list_item() {
                        self.out.ensure_blank_line();
                    }
                    self.tasks.push(Task::Close(Close::Block));
                    self.push_children(id, Mode::Block);
                }
            }
            NodeKind::Figure | NodeKind::Details | NodeKind::DefinitionList => {
                if !self.out.in_empty_list_item() {
                    self.out.ensure_blank_line();
                }
                self.tasks.push(Task::Close(Close::Block));
                self.push_children(id, Mode::Block);
            }
            NodeKind::BlockQuote | NodeKind::Callout(_) => {
                self.out.ensure_blank_line();
                self.out.prefixes.push(Prefix::Quote);
                self.tasks.push(Task::Close(Close::Quote));
                self.push_children(id, Mode::Block);
            }
            NodeKind::HardBreak => self.out.hard_break(),
            NodeKind::ThematicBreak => {
                self.out.ensure_blank_line();
                self.out.mark_list_item_content();
                self.out.markup("---");
                self.out.newline();
            }
            NodeKind::Strong => self.format(id, "**"),
            NodeKind::Emphasis => self.format(id, "*"),
            NodeKind::Strikethrough => self.format(id, "~~"),
            NodeKind::InlineCode(text) => self.code_span(text),
            NodeKind::CodeBlock(code) => self.code_block(code.language.as_deref(), &code.text),
            NodeKind::Link(_) => {
                if self.config.links {
                    self.out.mark_inline_boundary();
                    self.out.open_link();
                    self.tasks.push(Task::Close(Close::Link(id)));
                }
                self.push_children(id, Mode::Inline);
            }
            NodeKind::Image(image) if self.config.images => self.image(image),
            NodeKind::Image(_) => {}
            NodeKind::List(list) => self.list(id, list.kind, list.start),
            NodeKind::ListItem => self.push_children(id, Mode::Inline),
            NodeKind::Table(_) => self.table(id),
            NodeKind::TableRow => self.table_row(id),
            NodeKind::TableCell(_) => self.push_children(id, Mode::Inline),
            NodeKind::FootnoteReference(id) => self.footnote_reference(*id),
            NodeKind::FootnoteDefinition(footnote) => self.footnote_definition(id, *footnote),
            NodeKind::TaskMarker(_) => {}
            NodeKind::InlineMath(math) => self.math(&math.source, false),
            NodeKind::DisplayMath(math) => self.math(&math.source, true),
            NodeKind::Media(media) => {
                let title = media.title.as_deref().unwrap_or(&media.source);
                if self.config.links {
                    self.out.markup("[");
                    self.out.label(title);
                    self.out.markup("](");
                    self.out.destination(&media.source);
                    self.out.markup(")");
                } else {
                    self.out.text(title, None);
                }
            }
        }
    }

    fn format(&mut self, id: DocumentNodeId, marker: &'static str) {
        if self.visible(id) {
            self.out.mark_inline_boundary();
        }
        self.out.open_marker(marker);
        self.tasks.push(Task::Close(Close::Marker(marker)));
        self.push_children(id, Mode::Inline);
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
        self.out.markup_repeat('`', fence);
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
        self.out.markup_repeat('`', fence);
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
        scan_longest_run(text.as_bytes(), b'`', &mut longest, &mut current);
        let fence = 3.max(longest + 1);
        self.out.markup_repeat('`', fence);
        if let Some(language) = language {
            self.out.markup(language);
        }
        self.out.newline();
        self.out.verbatim(text.strip_suffix('\n').unwrap_or(text));
        self.out.newline();
        self.out.markup_repeat('`', fence);
        self.out.newline();
    }

    fn image(&mut self, image: &crate::document::Image) {
        self.out.mark_list_item_content();
        self.out.markup("![");
        if self.table_depth == 1 {
            self.out.table_label(&image.alt);
        } else {
            self.out.label(&image.alt);
        }
        self.out.markup("](");
        self.out.destination(&image.source);
        if let Some(title) = &image.title {
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

    fn list(&mut self, id: DocumentNodeId, kind: ListKind, start: Option<i64>) {
        if self.out.has_current_line_content() {
            self.out.newline();
        }
        if self.list_depth == 0 {
            self.out.ensure_blank_line();
        }
        self.list_depth += 1;
        self.tasks.push(Task::Close(Close::List));
        let start = start
            .and_then(|value| i32::try_from(value).ok())
            .filter(|value| (1..=999_999_999).contains(value))
            .unwrap_or(1);
        let children: SmallVec<[_; 16]> = self.document.child_ids(id).collect();
        let mut index = children
            .iter()
            .filter(|child| {
                matches!(
                    self.document.node(**child).map(|n| n.kind()),
                    Some(NodeKind::ListItem)
                )
            })
            .count();
        for child in children.into_iter().rev() {
            match self.document.node(child).map(|node| node.kind()) {
                Some(NodeKind::ListItem) => {
                    index -= 1;
                    let marker = match kind {
                        ListKind::Unordered => ListMarker::Bullet,
                        ListKind::Ordered if index == 0 && start != 1 => {
                            ListMarker::OrderedStart(start)
                        }
                        ListKind::Ordered => ListMarker::Ordered,
                    };
                    self.tasks.push(Task::ListItem(child, marker));
                }
                _ => self.tasks.push(Task::Node(child, Mode::Block)),
            }
        }
    }

    fn list_item(&mut self, id: DocumentNodeId, marker: ListMarker) {
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
            .task_marker(id)
            .map(|(checked, label)| (checked, label.map(str::to_owned)))
        {
            self.out.markup(if checked { "[x]" } else { "[ ]" });
            self.out.pending_space = true;
            if !self.list_item_has_text(id)
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
        self.tasks.push(Task::Close(Close::ListItem));
        let children: SmallVec<[_; 16]> = self.document.child_ids(id).collect();
        for child in children.into_iter().rev() {
            if matches!(
                self.document.node(child).map(|n| n.kind()),
                Some(NodeKind::TaskMarker(_))
            ) {
                continue;
            }
            if matches!(
                self.document.node(child).map(|n| n.kind()),
                Some(NodeKind::Paragraph)
            ) {
                self.tasks.push(Task::ItemParagraph(child));
            } else {
                self.tasks.push(Task::Node(child, Mode::Inline));
            }
        }
    }

    fn item_paragraph(&mut self, id: DocumentNodeId) {
        if !self.out.in_empty_list_item() {
            if self.out.has_current_line_content() {
                self.out.newline();
            }
            self.out.newline();
        }
        self.tasks.push(Task::Close(Close::Block));
        self.push_children(id, Mode::Inline);
    }

    fn task_marker(&self, item: DocumentNodeId) -> Option<(bool, Option<&str>)> {
        let children: SmallVec<[_; 8]> = self.document.child_ids(item).collect();
        let mut nodes: SmallVec<[DocumentNodeId; 8]> = children.into_iter().rev().collect();
        while let Some(id) = nodes.pop() {
            match self.document.node(id)?.kind() {
                NodeKind::Text(text) if has_visible_inline_text(text) => return None,
                NodeKind::TaskMarker(marker) => {
                    return Some((marker.checked, marker.fallback_label.as_deref()));
                }
                NodeKind::List(_) => {}
                NodeKind::Image(_) | NodeKind::Media(_) => return None,
                _ => {
                    let children: SmallVec<[_; 8]> = self.document.child_ids(id).collect();
                    nodes.extend(children.into_iter().rev());
                }
            }
        }
        None
    }

    fn list_item_has_text(&self, item: DocumentNodeId) -> bool {
        let mut nodes = SmallVec::<[DocumentNodeId; 8]>::new();
        nodes.extend(self.document.child_ids(item));
        while let Some(id) = nodes.pop() {
            match self.document.node(id).map(|node| node.kind()) {
                Some(NodeKind::Text(text)) if has_visible_inline_text(text) => return true,
                Some(NodeKind::List(_)) => {}
                Some(NodeKind::TaskMarker(_)) => {}
                Some(_) => nodes.extend(self.document.child_ids(id)),
                None => {}
            }
        }
        false
    }

    fn table(&mut self, id: DocumentNodeId) {
        self.out.ensure_blank_line();
        let children: SmallVec<[_; 16]> = self.document.child_ids(id).collect();
        if self.table_has_spans(id) {
            for child in children.iter().rev().copied() {
                self.tasks.push(Task::Node(child, Mode::Block));
            }
            return;
        }
        self.table_depth += 1;
        self.tasks.push(Task::Close(Close::Table));
        let rows: SmallVec<[_; 16]> = children
            .iter()
            .copied()
            .filter(|row| {
                matches!(
                    self.document.node(*row).map(|n| n.kind()),
                    Some(NodeKind::TableRow)
                )
            })
            .collect();
        for (index, row) in rows.into_iter().enumerate().rev() {
            let cells: SmallVec<[_; 32]> = self.document.child_ids(row).collect();
            let alignments = cells
                .iter()
                .map(
                    |cell| match self.document.node(*cell).map(|node| node.kind()) {
                        Some(NodeKind::TableCell(value)) => value.alignment,
                        _ => None,
                    },
                )
                .collect();
            if index == 0 && self.table_depth == 1 {
                self.tasks.push(Task::Close(Close::TableHeader(alignments)));
            }
            self.tasks.push(Task::Node(row, Mode::Block));
        }
        if let Some(caption) = children.into_iter().find(|child| {
            matches!(
                self.document.node(*child).map(|node| node.kind()),
                Some(NodeKind::TableCaption)
            )
        }) {
            self.tasks.push(Task::Close(Close::Block));
            self.tasks.push(Task::Node(caption, Mode::Block));
        }
    }

    fn table_row(&mut self, id: DocumentNodeId) {
        if self.out.has_current_line_content() {
            self.out.newline();
        }
        let cells: SmallVec<[_; 16]> = self.document.child_ids(id).collect();
        if self.table_depth == 1 {
            self.out.markup("| ");
        }
        self.tasks.push(Task::Close(Close::TableRow));
        for (index, cell) in cells.into_iter().enumerate().rev() {
            self.tasks.push(Task::TableCell(cell));
            if index != 0 {
                // A close task avoids exposing HTML-specific cell traversal.
                self.tasks
                    .push(Task::Close(Close::Marker(if self.table_depth == 1 {
                        " | "
                    } else {
                        "; "
                    })));
            }
        }
    }

    fn table_has_spans(&self, table: DocumentNodeId) -> bool {
        self.document.child_ids(table).any(|row| {
            self.document.child_ids(row).any(|cell| {
                matches!(
                    self.document.node(cell).map(|node| node.kind()),
                    Some(NodeKind::TableCell(value)) if value.colspan > 1 || value.rowspan > 1
                )
            })
        })
    }

    fn table_cell(&mut self, id: DocumentNodeId) {
        if !self.table_cell_requires_flattening(id) {
            self.push_children(id, Mode::Inline);
            return;
        }

        let mut text = String::new();
        let children: SmallVec<[_; 16]> = self.document.child_ids(id).collect();
        let mut nodes: SmallVec<[DocumentNodeId; 16]> = children.into_iter().rev().collect();
        while let Some(node_id) = nodes.pop() {
            let Some(node) = self.document.node(node_id) else {
                continue;
            };
            match node.kind() {
                NodeKind::Text(value) => text.push_str(value),
                NodeKind::InlineCode(value) => text.push_str(value),
                NodeKind::CodeBlock(code) => {
                    text.push(' ');
                    text.push_str(&code.text);
                    text.push(' ');
                }
                NodeKind::Image(image) if self.config.images => text.push_str(&image.alt),
                NodeKind::HardBreak => text.push(' '),
                NodeKind::FootnoteReference(id) => {
                    if let Some(definition) = self.document.footnote(*id) {
                        text.push_str(definition.label());
                    }
                }
                NodeKind::TaskMarker(marker) => {
                    if let Some(label) = &marker.fallback_label {
                        text.push_str(label);
                    }
                }
                NodeKind::InlineMath(math) | NodeKind::DisplayMath(math) => {
                    text.push_str(math.fallback_text.as_deref().unwrap_or(&math.source))
                }
                NodeKind::Media(media) => {
                    text.push_str(media.title.as_deref().unwrap_or(&media.source));
                }
                kind => {
                    if is_block(kind) {
                        text.push(' ');
                    }
                    let children: SmallVec<[_; 8]> = self.document.child_ids(node_id).collect();
                    nodes.extend(children.into_iter().rev());
                }
            }
        }
        self.out.text(text.trim(), None);
    }

    fn table_cell_requires_flattening(&self, cell: DocumentNodeId) -> bool {
        let mut blocks = 0;
        let mut nodes: SmallVec<[DocumentNodeId; 16]> = self.document.child_ids(cell).collect();
        while let Some(id) = nodes.pop() {
            let Some(node) = self.document.node(id) else {
                continue;
            };
            match node.kind() {
                NodeKind::HardBreak
                | NodeKind::CodeBlock(_)
                | NodeKind::List(_)
                | NodeKind::Table(_)
                | NodeKind::DisplayMath(_) => return true,
                kind if is_block(kind) => {
                    blocks += 1;
                    if blocks > 1 {
                        return true;
                    }
                }
                _ => {}
            }
            nodes.extend(self.document.child_ids(id));
        }
        false
    }

    fn footnote_reference(&mut self, id: FootnoteId) {
        if let Some(definition) = self.document.footnote(id) {
            self.out.markup("[^");
            self.out.footnote_label(definition.label());
            self.out.markup("]");
        }
    }

    fn footnote_definition(&mut self, node: DocumentNodeId, id: FootnoteId) {
        let Some(definition) = self.document.footnote(id) else {
            return;
        };
        self.out.ensure_blank_line();
        self.out.markup("[^");
        self.out.footnote_label(definition.label());
        self.out.markup("]: ");
        self.out.prefixes.push(Prefix::Indent(4));
        self.tasks.push(Task::Close(Close::Footnote));
        let children: SmallVec<[_; 8]> = self.document.child_ids(node).collect();
        for (index, child) in children.into_iter().enumerate().rev() {
            let mode = if index == 0
                && matches!(
                    self.document.node(child).map(|n| n.kind()),
                    Some(NodeKind::Paragraph)
                ) {
                Mode::Inline
            } else {
                Mode::Block
            };
            self.tasks.push(Task::Node(child, mode));
        }
    }

    fn close(&mut self, close: Close) {
        match close {
            Close::Block => self.out.newline(),
            Close::Marker(marker) if matches!(marker, " | " | "; ") => {
                self.out.markup(marker);
                self.out.last_text_char = None;
            }
            Close::Marker(marker) => {
                if self.out.close_marker(marker, marker) {
                    self.out.mark_inline_boundary();
                }
            }
            Close::Link(id) => {
                let Some(NodeKind::Link(link)) = self.document.node(id).map(|node| node.kind())
                else {
                    return;
                };
                if self.out.close_marker("[", "](") {
                    let trailing = std::mem::take(&mut self.out.pending_space);
                    self.out.destination(&link.destination);
                    if let Some(title) = &link.title {
                        self.out.markup(" \"");
                        self.out.link_title(title);
                        self.out.markup("\"");
                    }
                    self.out.markup(")");
                    self.out.pending_space = trailing;
                    self.out.mark_inline_boundary();
                }
            }
            Close::Quote => {
                if self.out.has_current_line_content() {
                    self.out.newline();
                }
                self.out.prefixes.pop();
            }
            Close::List => {
                if self.out.has_current_line_content() {
                    self.out.newline();
                }
                self.list_depth -= 1;
            }
            Close::ListItem => {
                if self.out.has_current_line_content() {
                    self.out.newline();
                }
                self.out.prefixes.pop();
            }
            Close::TableRow => {
                if self.table_depth == 1 {
                    self.out.markup(" |");
                }
                self.out.newline();
            }
            Close::TableHeader(values) => {
                if self.table_depth == 1 {
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
            }
            Close::Table => {
                self.table_depth -= 1;
            }
            Close::Footnote => {
                if self.out.has_current_line_content() {
                    self.out.newline();
                }
                self.out.prefixes.pop();
            }
        }
    }
}

fn compute_visibility(document: &Document, images: bool) -> HashMap<DocumentNodeId, bool> {
    let mut visible = HashMap::with_capacity(document.len());
    let mut tasks = Vec::with_capacity(32);
    tasks.extend(document.root_ids().map(|root| (root, false)));
    while let Some((id, visited)) = tasks.pop() {
        let Some(node) = document.node(id) else {
            continue;
        };
        if !visited {
            tasks.push((id, true));
            tasks.extend(document.child_ids(id).map(|child| (child, false)));
            continue;
        }
        let value = match node.kind() {
            NodeKind::Text(text) => has_visible_inline_text(text),
            NodeKind::InlineCode(text) => has_visible_inline_text(text),
            NodeKind::CodeBlock(code) => has_visible_inline_text(&code.text),
            NodeKind::Image(image) => images && has_visible_inline_text(&image.alt),
            NodeKind::TaskMarker(marker) => marker
                .fallback_label
                .as_deref()
                .is_some_and(has_visible_inline_text),
            NodeKind::InlineMath(_) | NodeKind::DisplayMath(_) | NodeKind::Media(_) => true,
            _ => document
                .child_ids(id)
                .any(|child| visible.get(&child).copied().unwrap_or(false)),
        };
        visible.insert(id, value);
    }
    visible
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

fn is_block(kind: &NodeKind) -> bool {
    matches!(
        kind,
        NodeKind::Paragraph
            | NodeKind::BlockGroup
            | NodeKind::Heading { .. }
            | NodeKind::BlockQuote
            | NodeKind::CodeBlock(_)
            | NodeKind::List(_)
            | NodeKind::ListItem
            | NodeKind::Table(_)
            | NodeKind::TableCaption
            | NodeKind::TableRow
            | NodeKind::TableCell(_)
            | NodeKind::Figure
            | NodeKind::Figcaption
            | NodeKind::Details
            | NodeKind::Summary
            | NodeKind::ThematicBreak
            | NodeKind::DefinitionList
            | NodeKind::DefinitionTerm
            | NodeKind::DefinitionDescription
            | NodeKind::Callout(_)
            | NodeKind::FootnoteDefinition(_)
            | NodeKind::DisplayMath(_)
    )
}

pub(crate) fn has_visible_inline_text(text: &str) -> bool {
    text.chars().any(|character| {
        !character.is_whitespace()
            && !matches!(character, '\u{00a0}' | '\u{200b}' | '\u{2060}' | '\u{feff}')
    })
}

struct Output {
    value: String,
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

impl Output {
    fn new(capacity: usize) -> Self {
        Self {
            value: String::with_capacity(capacity.max(512)),
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

    fn finish(mut self) -> String {
        self.resolve_hash_run(true);
        self.value
            .truncate(self.value.trim_end_matches([' ', '\t', '\r', '\n']).len());
        if !self.value.is_empty() {
            self.value.push('\n');
        }
        self.value
    }

    fn has_current_line_content(&self) -> bool {
        !self.line_start
    }

    fn prefix(&mut self) {
        if !self.line_start {
            return;
        }
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
            self.value.push_str(part);
            if part.ends_with('\n') {
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
        self.value.push('\n');
        self.line_start = true;
        self.line_text_state = LineTextState::Start;
        self.trailing_newlines += 1;
    }

    fn limit_trailing_newlines(&mut self, maximum: usize) {
        while self.trailing_newlines > maximum && self.value.ends_with('\n') {
            self.value.pop();
            self.trailing_newlines -= 1;
        }
    }

    fn ensure_blank_line(&mut self) {
        if self.value.is_empty() {
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
        self.value.push_str("  \n");
        self.line_start = true;
        self.line_text_state = LineTextState::Start;
        self.trailing_newlines = 1;
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
        CodeBlock, DocumentBuilder, Image, List, ListKind, MathFormat, MathValue, NodeKind, Table,
        TableCell, TaskMarker, TextValue,
    };

    #[test]
    fn semantic_code_chooses_safe_delimiters() {
        let mut builder = DocumentBuilder::with_capacity(3);
        let paragraph = builder.append(None, NodeKind::Paragraph).unwrap();
        builder
            .append(Some(paragraph), NodeKind::InlineCode(TextValue::new("a`b")))
            .unwrap();
        builder
            .append(
                None,
                NodeKind::CodeBlock(CodeBlock {
                    language: Some("rust".into()),
                    text: "let fence = ```;\n".into(),
                }),
            )
            .unwrap();
        let document = builder.finish();
        assert_eq!(
            render_markdown(&document, 0, MarkdownConfig::default()),
            "``a`b``\n\n````rust\nlet fence = ```;\n````\n"
        );
    }

    #[test]
    fn image_only_heading_without_alt_degrades_to_an_image() {
        let mut builder = DocumentBuilder::with_capacity(2);
        let heading = builder
            .append(None, NodeKind::Heading { level: 2 })
            .unwrap();
        builder
            .append(
                Some(heading),
                NodeKind::Image(Image {
                    source: "diagram.png".into(),
                    alt: "".into(),
                    title: None,
                    width: None,
                    height: None,
                }),
            )
            .unwrap();
        assert_eq!(
            render_markdown(&builder.finish(), 0, MarkdownConfig::default()),
            "![](diagram.png)\n"
        );
    }

    #[test]
    fn task_markers_must_be_the_first_visible_item_content() {
        let mut builder = DocumentBuilder::with_capacity(8);
        let list = builder
            .append(
                None,
                NodeKind::List(List {
                    kind: ListKind::Unordered,
                    start: None,
                }),
            )
            .unwrap();
        let trailing = builder.append(Some(list), NodeKind::ListItem).unwrap();
        builder.append_prose(Some(trailing), "Optional ").unwrap();
        builder
            .append(
                Some(trailing),
                NodeKind::TaskMarker(TaskMarker {
                    checked: true,
                    fallback_label: None,
                }),
            )
            .unwrap();
        let multiple = builder.append(Some(list), NodeKind::ListItem).unwrap();
        builder
            .append(
                Some(multiple),
                NodeKind::TaskMarker(TaskMarker {
                    checked: false,
                    fallback_label: None,
                }),
            )
            .unwrap();
        builder
            .append(
                Some(multiple),
                NodeKind::TaskMarker(TaskMarker {
                    checked: true,
                    fallback_label: None,
                }),
            )
            .unwrap();
        builder.append_prose(Some(multiple), "First").unwrap();
        assert_eq!(
            render_markdown(&builder.finish(), 0, MarkdownConfig::default()),
            "- Optional\n- [ ] First\n"
        );
    }

    #[test]
    fn math_cannot_emit_raw_html_or_close_its_delimiter() {
        let mut builder = DocumentBuilder::with_capacity(2);
        let paragraph = builder.append(None, NodeKind::Paragraph).unwrap();
        builder
            .append(
                Some(paragraph),
                NodeKind::InlineMath(MathValue {
                    source: "x $ <img src=x onerror=x>".into(),
                    format: MathFormat::Tex,
                    fallback_text: None,
                }),
            )
            .unwrap();
        let markdown = render_markdown(&builder.finish(), 0, MarkdownConfig::default());
        assert_eq!(markdown, "$x \\$ &lt;img src=x onerror=x&gt;$\n");
        assert!(!markdown.contains("<img"));
    }

    #[test]
    fn complex_table_cells_flatten_to_valid_gfm_lines_and_keep_the_caption() {
        let mut builder = DocumentBuilder::with_capacity(10);
        let table = builder
            .append(
                None,
                NodeKind::Table(Table {
                    column_count: Some(1),
                }),
            )
            .unwrap();
        let caption = builder.append(Some(table), NodeKind::TableCaption).unwrap();
        builder.append_prose(Some(caption), "Measurements").unwrap();
        let row = builder.append(Some(table), NodeKind::TableRow).unwrap();
        let cell = builder
            .append(
                Some(row),
                NodeKind::TableCell(TableCell {
                    header: false,
                    colspan: 1,
                    rowspan: 1,
                    alignment: None,
                }),
            )
            .unwrap();
        let first = builder.append(Some(cell), NodeKind::Paragraph).unwrap();
        builder.append_prose(Some(first), "first").unwrap();
        let second = builder.append(Some(cell), NodeKind::Paragraph).unwrap();
        builder.append_prose(Some(second), "second").unwrap();
        let markdown = render_markdown(&builder.finish(), 0, MarkdownConfig::default());
        assert_eq!(markdown, "Measurements\n\n| first second |\n| --- |\n");
    }

    #[test]
    fn table_images_and_code_escape_column_delimiters() {
        let mut builder = DocumentBuilder::with_capacity(6);
        let table = builder
            .append(
                None,
                NodeKind::Table(Table {
                    column_count: Some(2),
                }),
            )
            .unwrap();
        let row = builder.append(Some(table), NodeKind::TableRow).unwrap();
        for kind in [
            NodeKind::Image(Image {
                source: "diagram.png".into(),
                alt: "A|B".into(),
                title: None,
                width: None,
                height: None,
            }),
            NodeKind::InlineCode(TextValue::new("a|b")),
        ] {
            let cell = builder
                .append(
                    Some(row),
                    NodeKind::TableCell(TableCell {
                        header: false,
                        colspan: 1,
                        rowspan: 1,
                        alignment: None,
                    }),
                )
                .unwrap();
            builder.append(Some(cell), kind).unwrap();
        }
        let markdown = render_markdown(&builder.finish(), 0, MarkdownConfig::default());
        assert!(markdown.contains("![A\\|B](diagram.png)"), "{markdown}");
        assert!(markdown.contains("`a\\|b`"), "{markdown}");
    }

    #[test]
    fn spanning_tables_degrade_without_invalid_gfm_columns() {
        let mut builder = DocumentBuilder::with_capacity(5);
        let table = builder
            .append(
                None,
                NodeKind::Table(Table {
                    column_count: Some(2),
                }),
            )
            .unwrap();
        let row = builder.append(Some(table), NodeKind::TableRow).unwrap();
        let cell = builder
            .append(
                Some(row),
                NodeKind::TableCell(TableCell {
                    header: true,
                    colspan: 2,
                    rowspan: 1,
                    alignment: None,
                }),
            )
            .unwrap();
        builder.append_prose(Some(cell), "Wide heading").unwrap();
        let markdown = render_markdown(&builder.finish(), 0, MarkdownConfig::default());
        assert_eq!(markdown, "Wide heading\n");
        assert!(!markdown.contains("| ---"));
    }

    #[test]
    fn deeply_nested_formatting_with_many_text_runs_is_linear() {
        const DEPTH: usize = 5_000;
        let mut builder = DocumentBuilder::with_capacity(DEPTH * 2 + 1);
        let mut parent = Some(builder.append(None, NodeKind::Paragraph).unwrap());
        for _ in 0..DEPTH {
            let formatting = builder.append(parent, NodeKind::Emphasis).unwrap();
            builder.append_prose(Some(formatting), "x ").unwrap();
            parent = Some(formatting);
        }
        builder.append_prose(parent, "end").unwrap();
        let markdown = render_markdown(&builder.finish(), 0, MarkdownConfig::default());
        assert!(markdown.contains("end"));
    }

    #[test]
    fn deeply_nested_formatting_is_linear_and_stack_safe() {
        const DEPTH: usize = 10_000;
        let mut builder = DocumentBuilder::with_capacity(DEPTH + 2);
        let mut parent = Some(builder.append(None, NodeKind::Paragraph).unwrap());
        for _ in 0..DEPTH {
            parent = Some(builder.append(parent, NodeKind::Emphasis).unwrap());
        }
        builder.append_prose(parent, "deep").unwrap();
        let document = builder.finish();
        let markdown = render_markdown(&document, 0, MarkdownConfig::default());
        assert!(markdown.contains("deep"));
    }
}
