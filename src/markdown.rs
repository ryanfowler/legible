//! Markdown serialization for the cleaned extraction DOM.
//!
//! The serializer uses an explicit work stack. Deeply nested input does not use the
//! Rust call stack.

use smallvec::SmallVec;
use std::collections::HashMap;

use crate::dom::{AttrName, Dom, NodeId, Tag};

#[derive(Clone, Copy, Debug)]
pub(crate) struct MarkdownConfig {
    pub(crate) links: bool,
    pub(crate) images: bool,
}

/// Returns true when inline text has a serializer-visible character.
pub(crate) fn has_visible_inline_text(text: &str) -> bool {
    text.chars().any(|character| {
        !character.is_whitespace()
            && !matches!(character, '\u{00a0}' | '\u{200b}' | '\u{2060}' | '\u{feff}')
    })
}

impl Default for MarkdownConfig {
    fn default() -> Self {
        Self {
            links: true,
            images: true,
        }
    }
}

trait MarkdownTree {
    type Node: Copy + Eq + std::hash::Hash;

    fn first_child(&self, node: Self::Node) -> Option<Self::Node>;
    fn next_sibling(&self, node: Self::Node) -> Option<Self::Node>;
    fn parent(&self, node: Self::Node) -> Option<Self::Node>;
    fn tag(&self, node: Self::Node) -> Option<Tag>;
    fn text_node(&self, node: Self::Node) -> Option<&str>;
    fn is_comment(&self, node: Self::Node) -> bool;
    fn attr(&self, node: Self::Node, name: AttrName) -> Option<&str>;
    fn attr_by_local_name(&self, node: Self::Node, name: &str) -> Option<&str>;

    fn has_descendant_tag(&self, root: Self::Node, wanted: Tag) -> bool {
        let mut nodes = SmallVec::<[Self::Node; 16]>::new();
        let mut child = self.first_child(root);
        while let Some(node) = child {
            nodes.push(node);
            child = self.next_sibling(node);
        }
        while let Some(node) = nodes.pop() {
            if self.tag(node) == Some(wanted) {
                return true;
            }
            let mut child = self.first_child(node);
            while let Some(current) = child {
                nodes.push(current);
                child = self.next_sibling(current);
            }
        }
        false
    }

    fn for_each_text(&self, root: Self::Node, mut visit: impl FnMut(&str)) {
        let mut nodes = SmallVec::<[(Self::Node, bool); 16]>::new();
        nodes.push((root, false));
        while let Some((node, include_siblings)) = nodes.pop() {
            if include_siblings && let Some(sibling) = self.next_sibling(node) {
                nodes.push((sibling, true));
            }
            if let Some(text) = self.text_node(node) {
                visit(text);
                continue;
            }
            if self.tag(node) == Some(Tag::Template) {
                continue;
            }
            if let Some(child) = self.first_child(node) {
                nodes.push((child, true));
            }
        }
    }
}

impl MarkdownTree for Dom {
    type Node = NodeId;

    fn first_child(&self, node: NodeId) -> Option<NodeId> {
        self.first_child(node)
    }
    fn next_sibling(&self, node: NodeId) -> Option<NodeId> {
        self.next_sibling(node)
    }
    fn parent(&self, node: NodeId) -> Option<NodeId> {
        self.parent(node)
    }
    fn tag(&self, node: NodeId) -> Option<Tag> {
        self.tag(node)
    }
    fn text_node(&self, node: NodeId) -> Option<&str> {
        self.text_node(node)
    }
    fn is_comment(&self, node: NodeId) -> bool {
        self.is_comment(node)
    }
    fn attr(&self, node: NodeId, name: AttrName) -> Option<&str> {
        self.attr(node, name)
    }
    fn attr_by_local_name(&self, node: NodeId, name: &str) -> Option<&str> {
        self.attr_by_local_name(node, name)
    }
}

/// Serializes the children of `root` as CommonMark.
#[cfg(test)]
pub(crate) fn dom_to_markdown(dom: &Dom, root: NodeId, capacity: usize) -> String {
    render_markdown(dom, root, capacity, MarkdownConfig::default())
}

pub(crate) fn render_markdown(
    dom: &Dom,
    root: NodeId,
    capacity: usize,
    config: MarkdownConfig,
) -> String {
    serialize_markdown(dom, root, capacity, config)
}

fn serialize_markdown<T: MarkdownTree>(
    tree: &T,
    root: T::Node,
    capacity: usize,
    config: MarkdownConfig,
) -> String {
    MarkdownSerializer::new(tree, capacity, config).serialize(root)
}

#[derive(Clone, Copy)]
enum Mode {
    Block,
    Inline,
}

enum InlineNext {
    Text(char),
    Boundary,
    Empty,
}

#[derive(Clone, Copy)]
enum Task<N> {
    Node(N, Mode),
    Siblings(N, Mode),
    InlineRun(N, RunKind),
    FormatChildren(N, RunKind),
    MarkTextBoundary,
    Close(Close<N>),
    ListItems(N, i32, u32),
    ListItem(N, ListMarker),
    ItemParagraph(N),
    FootnoteChildren(N, bool),
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum RunKind {
    Strong,
    Emphasis,
    Code,
}

impl RunKind {
    fn marker(self) -> Option<&'static str> {
        match self {
            Self::Strong => Some("**"),
            Self::Emphasis => Some("*"),
            Self::Code => None,
        }
    }
}

#[derive(Clone, Copy)]
enum ListMarker {
    Bullet,
    Ordered,
    OrderedStart(i32),
}

#[derive(Clone, Copy)]
enum Close<N> {
    Block,
    Marker(RunKind),
    TableCellSeparator,
    TableRow,
    TableHeaderSeparator(usize),
    Table,
    Link(N),
    Quote,
    List,
    ListItem,
    Footnote,
}

struct MarkdownSerializer<'a, T: MarkdownTree> {
    dom: &'a T,
    out: Output,
    tasks: Vec<Task<T::Node>>,
    list_depth: usize,
    table_rows: SmallVec<[usize; 4]>,
    table_gfm: SmallVec<[bool; 4]>,
    visible_content: HashMap<T::Node, bool>,
    formatting_ancestor: HashMap<T::Node, bool>,
    config: MarkdownConfig,
}

impl<'a, T: MarkdownTree> MarkdownSerializer<'a, T> {
    fn new(dom: &'a T, capacity: usize, config: MarkdownConfig) -> Self {
        Self {
            dom,
            out: Output::new(capacity),
            // Most documents stay below this depth. Keep the stack on the heap so
            // adversarial nesting cannot exhaust the call stack.
            tasks: Vec::with_capacity(32),
            list_depth: 0,
            table_rows: SmallVec::new(),
            table_gfm: SmallVec::new(),
            visible_content: HashMap::new(),
            formatting_ancestor: HashMap::new(),
            config,
        }
    }

    fn serialize(mut self, root: T::Node) -> String {
        self.push_children(root, Mode::Block);
        while let Some(task) = self.tasks.pop() {
            match task {
                Task::Node(id, mode) => self.node(id, mode),
                Task::Siblings(id, mode) => {
                    if let Some(kind) = self.run_kind(id) {
                        let mut after = self.dom.next_sibling(id);
                        while after.is_some_and(|node| self.run_kind(node) == Some(kind)) {
                            after = after.and_then(|node| self.dom.next_sibling(node));
                        }
                        if let Some(after) = after {
                            self.tasks.push(Task::Siblings(after, mode));
                        }
                        self.tasks.push(Task::InlineRun(id, kind));
                    } else {
                        if let Some(sibling) = self.dom.next_sibling(id) {
                            self.tasks.push(Task::Siblings(sibling, mode));
                        }
                        self.tasks.push(Task::Node(id, mode));
                    }
                }
                Task::InlineRun(id, kind) => self.inline_run(id, kind),
                Task::FormatChildren(id, kind) => self.format_children(id, kind),
                Task::MarkTextBoundary => self.out.mark_inline_boundary(),
                Task::Close(close) => self.close(close),
                Task::ListItems(id, start, index) => self.list_items(id, start, index),
                Task::ListItem(id, marker) => self.list_item(id, marker),
                Task::ItemParagraph(id) => self.item_paragraph(id),
                Task::FootnoteChildren(id, first) => self.footnote_children(id, first),
            }
        }
        self.out.finish()
    }

    fn has_visible_heading_content(&self, root: T::Node) -> bool {
        let mut nodes = SmallVec::<[T::Node; 16]>::new();
        nodes.push(root);
        while let Some(node) = nodes.pop() {
            if self
                .dom
                .text_node(node)
                .is_some_and(has_visible_inline_text)
            {
                return true;
            }
            if self.config.images
                && self.dom.tag(node) == Some(Tag::Img)
                && self
                    .dom
                    .attr_by_local_name(node, "alt")
                    .is_some_and(has_visible_inline_text)
            {
                return true;
            }
            let mut child = self.dom.first_child(node);
            while let Some(current) = child {
                nodes.push(current);
                child = self.dom.next_sibling(current);
            }
        }
        false
    }

    fn has_visible_inline_content(&mut self, root: T::Node) -> bool {
        if let Some(&visible) = self.visible_content.get(&root) {
            return visible;
        }

        let mut nodes = SmallVec::<[(T::Node, bool); 32]>::new();
        nodes.push((root, false));
        while let Some((node, visited)) = nodes.pop() {
            if self.visible_content.contains_key(&node) {
                continue;
            }
            if visited {
                let mut visible = self
                    .dom
                    .text_node(node)
                    .is_some_and(has_visible_inline_text);
                if !visible && self.dom.tag(node) != Some(Tag::Template) {
                    let mut child = self.dom.first_child(node);
                    while let Some(current) = child {
                        visible |= self.visible_content.get(&current).copied().unwrap_or(false);
                        child = self.dom.next_sibling(current);
                    }
                }
                self.visible_content.insert(node, visible);
            } else {
                nodes.push((node, true));
                if self.dom.tag(node) != Some(Tag::Template) {
                    let mut child = self.dom.first_child(node);
                    while let Some(current) = child {
                        if !self.visible_content.contains_key(&current) {
                            nodes.push((current, false));
                        }
                        child = self.dom.next_sibling(current);
                    }
                }
            }
        }
        self.visible_content.get(&root).copied().unwrap_or(false)
    }

    fn has_formatting_ancestor(&mut self, node: T::Node) -> bool {
        if let Some(&formatting) = self.formatting_ancestor.get(&node) {
            return formatting;
        }

        let mut path = SmallVec::<[T::Node; 16]>::new();
        let mut ancestor = self.dom.parent(node);
        let mut formatting = false;
        while let Some(id) = ancestor {
            if let Some(&cached) = self.formatting_ancestor.get(&id) {
                formatting = cached;
                break;
            }
            path.push(id);
            if matches!(
                self.dom.tag(id),
                Some(Tag::Strong | Tag::B | Tag::Em | Tag::I | Tag::Code | Tag::Sub | Tag::Sup)
            ) {
                formatting = true;
                break;
            }
            ancestor = self.dom.parent(id);
        }
        self.formatting_ancestor.insert(node, formatting);
        for id in path {
            self.formatting_ancestor.insert(id, formatting);
        }
        formatting
    }

    fn inline_container(&mut self, id: T::Node) {
        let boundary = self.has_visible_inline_content(id) && !self.has_formatting_ancestor(id);
        if boundary {
            self.out.mark_inline_boundary();
            self.tasks.push(Task::MarkTextBoundary);
        }
        self.push_children(id, Mode::Inline);
    }

    fn push_children(&mut self, id: T::Node, mode: Mode) {
        if let Some(child) = self.dom.first_child(id) {
            self.tasks.push(Task::Siblings(child, mode));
        }
    }

    fn next_inline_text_char(&self, node: T::Node) -> Option<char> {
        let mut current = node;
        loop {
            if let Some(sibling) = self.dom.next_sibling(current) {
                match self.first_inline_text_char(sibling) {
                    InlineNext::Text(ch) => return Some(ch),
                    InlineNext::Boundary => return None,
                    InlineNext::Empty => current = sibling,
                }
                continue;
            }
            let parent = self.dom.parent(current)?;
            if matches!(
                self.dom.tag(parent),
                Some(
                    Tag::Address
                        | Tag::Blockquote
                        | Tag::Caption
                        | Tag::Dd
                        | Tag::Div
                        | Tag::Dl
                        | Tag::Dt
                        | Tag::Figure
                        | Tag::Figcaption
                        | Tag::Footer
                        | Tag::Form
                        | Tag::H1
                        | Tag::H2
                        | Tag::H3
                        | Tag::H4
                        | Tag::H5
                        | Tag::H6
                        | Tag::Header
                        | Tag::Hr
                        | Tag::Main
                        | Tag::Nav
                        | Tag::P
                        | Tag::Pre
                        | Tag::Section
                        | Tag::Table
                        | Tag::Td
                        | Tag::Th
                        | Tag::Tr
                        | Tag::Ul
                        | Tag::Ol
                        | Tag::Br
                )
            ) {
                return None;
            }
            current = parent;
        }
    }

    fn first_inline_text_char(&self, root: T::Node) -> InlineNext {
        let mut nodes = SmallVec::<[T::Node; 8]>::new();
        nodes.push(root);
        while let Some(node) = nodes.pop() {
            if let Some(text) = self.dom.text_node(node) {
                if let Some(ch) = text.chars().next() {
                    return InlineNext::Text(ch);
                }
                continue;
            }
            if self.dom.is_comment(node) {
                continue;
            }
            let Some(tag) = self.dom.tag(node) else {
                continue;
            };
            if matches!(tag, Tag::Head | Tag::Script | Tag::Style | Tag::Template) {
                continue;
            }
            if matches!(
                tag,
                Tag::Address
                    | Tag::Blockquote
                    | Tag::Caption
                    | Tag::Dd
                    | Tag::Div
                    | Tag::Dl
                    | Tag::Dt
                    | Tag::Figure
                    | Tag::Figcaption
                    | Tag::Footer
                    | Tag::Form
                    | Tag::H1
                    | Tag::H2
                    | Tag::H3
                    | Tag::H4
                    | Tag::H5
                    | Tag::H6
                    | Tag::Header
                    | Tag::Hr
                    | Tag::Main
                    | Tag::Nav
                    | Tag::P
                    | Tag::Pre
                    | Tag::Section
                    | Tag::Table
                    | Tag::Td
                    | Tag::Th
                    | Tag::Tr
                    | Tag::Ul
                    | Tag::Ol
                    | Tag::Br
            ) {
                return InlineNext::Boundary;
            }
            if tag == Tag::Img && self.config.images {
                return InlineNext::Text('!');
            }
            let mut children = SmallVec::<[T::Node; 8]>::new();
            let mut child = self.dom.first_child(node);
            while let Some(current) = child {
                children.push(current);
                child = self.dom.next_sibling(current);
            }
            nodes.extend(children.into_iter().rev());
        }
        InlineNext::Empty
    }

    fn node(&mut self, id: T::Node, mode: Mode) {
        if let Some(text) = self.dom.text_node(id) {
            self.out.text(text, self.next_inline_text_char(id));
            return;
        }
        if self.dom.is_comment(id) {
            return;
        }
        let Some(tag) = self.dom.tag(id) else {
            return;
        };

        if let Some(latex) = self.dom.attr(id, AttrName::DataLatex)
            && let Some(kind) = self.dom.attr(id, AttrName::DataMath)
        {
            self.math(latex, kind == "block");
            return;
        }
        if let Some(label) = self.dom.attr(id, AttrName::DataFootnoteRef) {
            self.out.markup("[^");
            self.out.footnote_label(label);
            self.out.markup("]");
            return;
        }
        if let Some(label) = self.dom.attr(id, AttrName::DataFootnote) {
            self.footnote_definition(id, label);
            return;
        }

        match tag {
            Tag::H1 | Tag::H2 | Tag::H3 | Tag::H4 | Tag::H5 | Tag::H6 => {
                if !self.has_visible_heading_content(id) {
                    if self.config.images {
                        self.push_children(id, Mode::Block);
                    }
                    return;
                }
                self.out.ensure_blank_line();
                let marker = match tag {
                    Tag::H1 => "#",
                    Tag::H2 => "##",
                    Tag::H3 => "###",
                    Tag::H4 => "####",
                    Tag::H5 => "#####",
                    Tag::H6 => "######",
                    _ => unreachable!("heading match contains only headings"),
                };
                self.out.markup(marker);
                self.out.markup(" ");
                self.tasks.push(Task::Close(Close::Block));
                self.push_children(id, Mode::Inline);
            }
            Tag::P => {
                if matches!(mode, Mode::Block) {
                    self.out.ensure_blank_line();
                    self.tasks.push(Task::Close(Close::Block));
                }
                self.push_children(id, Mode::Inline);
            }
            Tag::Address | Tag::Caption | Tag::Dd | Tag::Dt | Tag::Figcaption => {
                self.out.ensure_blank_line();
                self.tasks.push(Task::Close(Close::Block));
                self.push_children(id, Mode::Inline);
            }
            Tag::Article
            | Tag::Aside
            | Tag::Body
            | Tag::Details
            | Tag::Div
            | Tag::Dl
            | Tag::Fieldset
            | Tag::Figure
            | Tag::Footer
            | Tag::Form
            | Tag::Header
            | Tag::Main
            | Tag::Nav
            | Tag::Section => {
                if !self.out.in_empty_list_item() {
                    self.out.ensure_blank_line();
                }
                self.tasks.push(Task::Close(Close::Block));
                self.push_children(id, Mode::Block);
            }
            Tag::Br => self.out.hard_break(),
            Tag::Hr => {
                self.out.ensure_blank_line();
                self.out.mark_list_item_content();
                self.out.markup("---");
                self.out.newline();
            }
            Tag::Blockquote => {
                self.out.ensure_blank_line();
                self.out.prefixes.push(Prefix::Quote);
                self.tasks.push(Task::Close(Close::Quote));
                self.push_children(id, Mode::Block);
            }
            Tag::Ul | Tag::Ol => self.list(id, tag == Tag::Ol),
            Tag::Pre => self.code_block(id),
            Tag::A => {
                let linked = self.config.links
                    && self
                        .dom
                        .attr(id, AttrName::Href)
                        .and_then(|href| safe_destination(href, DestinationKind::Link))
                        .is_some();
                if linked {
                    if self.has_visible_inline_content(id) && !self.has_formatting_ancestor(id) {
                        self.out.mark_inline_boundary();
                    }
                    self.out.open_link();
                    self.tasks.push(Task::Close(Close::Link(id)));
                }
                self.push_children(id, Mode::Inline);
            }
            Tag::Img if self.config.images => self.image(id),
            Tag::Img => {}
            Tag::Strong | Tag::B => self.format(id, RunKind::Strong),
            Tag::Em | Tag::I => self.format(id, RunKind::Emphasis),
            Tag::Code => self.code_span(id),
            Tag::Span => self.inline_container(id),
            Tag::Sub | Tag::Sup => self.push_children(id, Mode::Inline),
            Tag::Table => {
                self.out.ensure_blank_line();
                self.table_rows.push(0);
                self.table_gfm.push(self.table_gfm.is_empty());
                self.tasks.push(Task::Close(Close::Table));
                self.push_children(id, Mode::Block);
            }
            Tag::Tr => self.table_row(id),
            Tag::Td | Tag::Th => self.push_children(id, Mode::Inline),
            Tag::Head | Tag::Script | Tag::Style | Tag::Template => {}
            _ => self.push_children(id, mode),
        }
    }

    fn format(&mut self, id: T::Node, kind: RunKind) {
        let marker = kind.marker().expect("formatting element has a marker");
        let boundary = self.has_visible_inline_content(id);
        if boundary {
            self.out.mark_inline_boundary();
        }
        self.out.open_marker(marker);
        self.tasks.push(Task::Close(Close::Marker(kind)));
        if boundary {
            self.tasks.push(Task::MarkTextBoundary);
        }
        if let Some(child) = self.dom.first_child(id) {
            self.tasks.push(Task::FormatChildren(child, kind));
        }
    }

    fn math(&mut self, latex: &str, block: bool) {
        if block {
            self.out.ensure_blank_line();
            self.out.markup("$$");
            self.out.newline();
            self.out.verbatim(latex.trim());
            self.out.newline();
            self.out.markup("$$");
            self.out.newline();
        } else {
            self.out.markup("$");
            self.out.verbatim(latex.trim());
            self.out.markup("$");
        }
    }

    fn footnote_definition(&mut self, id: T::Node, label: &str) {
        self.out.ensure_blank_line();
        self.out.markup("[^");
        self.out.footnote_label(label);
        self.out.markup("]: ");
        self.out.prefixes.push(Prefix::Indent(4));
        self.tasks.push(Task::Close(Close::Footnote));
        if let Some(child) = self.dom.first_child(id) {
            self.tasks.push(Task::FootnoteChildren(child, true));
        }
    }

    fn footnote_children(&mut self, id: T::Node, first: bool) {
        if let Some(sibling) = self.dom.next_sibling(id) {
            self.tasks.push(Task::FootnoteChildren(sibling, false));
        }
        self.tasks.push(Task::Node(
            id,
            if first && self.dom.tag(id) == Some(Tag::P) {
                Mode::Inline
            } else {
                Mode::Block
            },
        ));
    }

    fn format_children(&mut self, id: T::Node, kind: RunKind) {
        if let Some(sibling) = self.dom.next_sibling(id) {
            self.tasks.push(Task::FormatChildren(sibling, kind));
        }
        if self.run_kind(id) == Some(kind) {
            if let Some(child) = self.dom.first_child(id) {
                self.tasks.push(Task::FormatChildren(child, kind));
            }
        } else {
            self.tasks.push(Task::Node(id, Mode::Inline));
        }
    }

    fn run_kind(&self, id: T::Node) -> Option<RunKind> {
        match self.dom.tag(id)? {
            Tag::Strong | Tag::B => Some(RunKind::Strong),
            Tag::Em | Tag::I => Some(RunKind::Emphasis),
            Tag::Code => Some(RunKind::Code),
            _ => None,
        }
    }

    fn inline_run(&mut self, first: T::Node, kind: RunKind) {
        if kind == RunKind::Code {
            let mut nodes = SmallVec::<[T::Node; 4]>::new();
            let mut current = Some(first);
            while let Some(id) = current.filter(|&id| self.run_kind(id) == Some(kind)) {
                nodes.push(id);
                current = self.dom.next_sibling(id);
            }
            self.emit_code_span(&nodes);
            return;
        }

        let mut nodes = SmallVec::<[T::Node; 4]>::new();
        let mut current = Some(first);
        while let Some(id) = current.filter(|&id| self.run_kind(id) == Some(kind)) {
            nodes.push(id);
            current = self.dom.next_sibling(id);
        }
        let boundary = nodes
            .iter()
            .copied()
            .any(|id| self.has_visible_inline_content(id));
        let marker = kind.marker().expect("formatting run has a marker");
        if boundary {
            self.out.mark_inline_boundary();
        }
        self.out.open_marker(marker);
        self.tasks.push(Task::Close(Close::Marker(kind)));
        if boundary {
            self.tasks.push(Task::MarkTextBoundary);
        }
        for id in nodes.into_iter().rev() {
            if let Some(child) = self.dom.first_child(id) {
                self.tasks.push(Task::FormatChildren(child, kind));
            }
        }
    }

    fn list(&mut self, id: T::Node, ordered: bool) {
        if self.out.has_current_line_content() {
            self.out.newline();
        }
        if self.list_depth == 0 {
            self.out.ensure_blank_line();
        }
        self.list_depth += 1;
        self.tasks.push(Task::Close(Close::List));

        let start = self
            .dom
            .attr_by_local_name(id, "start")
            .and_then(|value| value.parse::<i32>().ok())
            .filter(|value| (1..=999_999_999).contains(value))
            .unwrap_or(1);
        if let Some(child) = self.dom.first_child(id) {
            self.tasks.push(Task::ListItems(
                child,
                if ordered { start } else { i32::MIN },
                0,
            ));
        }
    }

    fn list_items(&mut self, id: T::Node, start: i32, index: u32) {
        let is_item = self.dom.tag(id) == Some(Tag::Li);
        let is_footnote = is_item && self.dom.attr(id, AttrName::DataFootnote).is_some();
        if let Some(sibling) = self.dom.next_sibling(id) {
            self.tasks.push(Task::ListItems(
                sibling,
                start,
                index + u32::from(is_item && !is_footnote),
            ));
        }
        if is_footnote {
            self.tasks.push(Task::Node(id, Mode::Block));
        } else if is_item {
            let marker = if start == i32::MIN {
                ListMarker::Bullet
            } else if index == 0 && start != 1 {
                ListMarker::OrderedStart(start)
            } else {
                // Any number continues a CommonMark ordered list.
                ListMarker::Ordered
            };
            self.tasks.push(Task::ListItem(id, marker));
        } else {
            self.tasks.push(Task::Node(id, Mode::Inline));
        }
    }

    fn list_item(&mut self, id: T::Node, marker: ListMarker) {
        if self.out.has_current_line_content() {
            self.out.newline();
        }
        let task_state = self.list_item_task_state(id);
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
        if let Some((checked, fallback_label)) = task_state {
            self.out.markup(if checked { "[x]" } else { "[ ]" });
            self.out.pending_space = true;
            if let Some(label) = fallback_label {
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

        let mut children = SmallVec::<[T::Node; 8]>::new();
        let mut child = self.dom.first_child(id);
        while let Some(node) = child {
            children.push(node);
            child = self.dom.next_sibling(node);
        }
        for child in children.into_iter().rev() {
            if self.dom.tag(child) == Some(Tag::P) {
                self.tasks.push(Task::ItemParagraph(child));
            } else {
                self.tasks.push(Task::Node(child, Mode::Inline));
            }
        }
    }

    fn list_item_task_state(&self, item: T::Node) -> Option<(bool, Option<String>)> {
        let has_visible_label = self.list_item_has_visible_label(item);
        let mut nodes = SmallVec::<[T::Node; 8]>::new();
        self.push_nodes_in_reverse(item, &mut nodes);
        while let Some(node) = nodes.pop() {
            if self
                .dom
                .text_node(node)
                .is_some_and(has_visible_inline_text)
            {
                return None;
            }
            match self.dom.tag(node) {
                Some(Tag::Input)
                    if self
                        .dom
                        .attr(node, AttrName::Type)
                        .is_some_and(|value| value.eq_ignore_ascii_case("checkbox")) =>
                {
                    let fallback_label = (!has_visible_label)
                        .then(|| {
                            self.dom
                                .attr(node, AttrName::AriaLabel)
                                .or_else(|| self.dom.attr(node, AttrName::Title))
                        })
                        .flatten()
                        .filter(|label| has_visible_inline_text(label))
                        .map(str::to_owned);
                    return Some((
                        self.dom.attr(node, AttrName::Checked).is_some(),
                        fallback_label,
                    ));
                }
                Some(Tag::Form | Tag::Input | Tag::Img | Tag::Ol | Tag::Ul) => return None,
                _ => self.push_nodes_in_reverse(node, &mut nodes),
            }
        }
        None
    }

    fn list_item_has_visible_label(&self, item: T::Node) -> bool {
        let mut nodes = SmallVec::<[T::Node; 8]>::new();
        self.push_nodes_in_reverse(item, &mut nodes);
        while let Some(node) = nodes.pop() {
            if self
                .dom
                .text_node(node)
                .is_some_and(has_visible_inline_text)
            {
                return true;
            }
            if matches!(self.dom.tag(node), Some(Tag::Ol | Tag::Ul)) {
                continue;
            }
            self.push_nodes_in_reverse(node, &mut nodes);
        }
        false
    }

    fn push_nodes_in_reverse(&self, parent: T::Node, nodes: &mut SmallVec<[T::Node; 8]>) {
        let mut children = SmallVec::<[T::Node; 8]>::new();
        let mut child = self.dom.first_child(parent);
        while let Some(node) = child {
            children.push(node);
            child = self.dom.next_sibling(node);
        }
        nodes.extend(children.into_iter().rev());
    }

    fn item_paragraph(&mut self, id: T::Node) {
        if !self.out.in_empty_list_item() {
            if self.out.has_current_line_content() {
                self.out.newline();
            }
            self.out.newline();
        }
        self.tasks.push(Task::Close(Close::Block));
        self.push_children(id, Mode::Inline);
    }

    fn table_row(&mut self, id: T::Node) {
        if self.out.has_current_line_content() {
            self.out.newline();
        }
        let mut cells = SmallVec::<[T::Node; 16]>::new();
        let mut child = self.dom.first_child(id);
        while let Some(node) = child {
            if matches!(self.dom.tag(node), Some(Tag::Td | Tag::Th)) {
                cells.push(node);
            }
            child = self.dom.next_sibling(node);
        }
        if cells.is_empty() {
            self.push_children(id, Mode::Inline);
            self.tasks.push(Task::Close(Close::Block));
            return;
        }
        let gfm = self.table_gfm.last().copied().unwrap_or(false)
            && !self.dom.has_descendant_tag(id, Tag::Table);
        let first_row = gfm && self.table_rows.last().is_some_and(|rows| *rows == 0);
        if let Some(rows) = self.table_rows.last_mut() {
            *rows += 1;
        }
        if gfm {
            self.out.markup("| ");
            if first_row {
                self.tasks
                    .push(Task::Close(Close::TableHeaderSeparator(cells.len())));
            }
            self.tasks.push(Task::Close(Close::TableRow));
        } else {
            self.tasks.push(Task::Close(Close::Block));
        }
        for (index, cell) in cells.into_iter().enumerate().rev() {
            self.tasks.push(Task::Node(cell, Mode::Inline));
            if index != 0 {
                self.tasks.push(Task::Close(Close::TableCellSeparator));
            }
        }
    }

    fn code_span(&mut self, id: T::Node) {
        self.emit_code_span(&[id]);
    }

    fn emit_code_span(&mut self, nodes: &[T::Node]) {
        let mut scan = CollapsedText::default();
        for &id in nodes {
            self.dom.for_each_text(id, |text| scan.scan(text));
        }

        self.out.mark_list_item_content();
        let fence_len = scan.longest_backtick_run + 1;
        let pad = scan.starts_with_backtick || scan.ends_with_backtick;
        if scan.has_content {
            self.out.mark_inline_boundary();
            self.out.prepare_inline_boundary(scan.first_char);
        }
        self.out.markup_repeat('`', fence_len);
        if pad {
            self.out.verbatim(" ");
        }
        let mut writer = CollapsedTextWriter::default();
        self.out.begin_verbatim();
        for &id in nodes {
            self.dom
                .for_each_text(id, |text| writer.write(&mut self.out.value, text));
        }
        if writer.empty && writer.pending_whitespace {
            self.out.value.push(' ');
        }
        if pad {
            self.out.verbatim(" ");
        }
        self.out.markup_repeat('`', fence_len);
        if !writer.empty {
            self.out.last_text_char = writer.last_char;
            self.out.mark_inline_boundary();
        }
    }

    fn code_block(&mut self, id: T::Node) {
        self.out.ensure_blank_line();
        self.out.mark_list_item_content();
        let mut longest = 0;
        let mut current = 0;
        let mut remaining = 0;
        let mut ends_with_newline = false;
        self.dom.for_each_text(id, |text| {
            remaining += text.len();
            if !text.is_empty() {
                ends_with_newline = text.ends_with('\n');
            }
            scan_longest_run(text.as_bytes(), b'`', &mut longest, &mut current);
        });
        let fence_len = 3.max(longest + 1);
        self.out.markup_repeat('`', fence_len);
        if let Some(language) = self.code_block_language(id).map(str::to_owned) {
            self.out.markup(&language);
        }
        self.out.newline();
        self.dom.for_each_text(id, |text| {
            let omit_last = !text.is_empty() && remaining == text.len() && ends_with_newline;
            let text = if omit_last {
                &text[..text.len() - 1]
            } else {
                text
            };
            self.out.verbatim(text);
            remaining -= text.len() + usize::from(omit_last);
        });
        self.out.newline();
        self.out.markup_repeat('`', fence_len);
        self.out.newline();
    }

    fn code_block_language(&self, pre: T::Node) -> Option<&str> {
        let mut child = self.dom.first_child(pre);
        let mut code = None;
        while let Some(node) = child {
            if self.dom.tag(node) == Some(Tag::Code) {
                code = Some(node);
                break;
            }
            child = self.dom.next_sibling(node);
        }
        code.and_then(|node| self.dom.attr(node, AttrName::DataLanguage))
            .or_else(|| self.dom.attr(pre, AttrName::DataLanguage))
            .filter(|language| {
                !language.is_empty()
                    && language.len() <= 32
                    && language.bytes().all(|byte| {
                        byte.is_ascii_alphanumeric()
                            || matches!(byte, b'+' | b'#' | b'-' | b'_' | b'.')
                    })
            })
    }

    fn image(&mut self, id: T::Node) {
        let alt = self.dom.attr_by_local_name(id, "alt").unwrap_or("");
        let Some(src) = self
            .dom
            .attr(id, AttrName::Src)
            .and_then(|src| safe_destination(src, DestinationKind::Image))
        else {
            self.out.text(alt, None);
            return;
        };
        self.out.mark_list_item_content();
        self.out.markup("![");
        self.out.label(alt);
        self.out.markup("](");
        self.out.destination(src);
        if let Some(title) = self.dom.attr_by_local_name(id, "title") {
            self.out.markup(" \"");
            self.out.link_title(title);
            self.out.markup("\"");
        }
        self.out.markup(")");
    }

    fn close(&mut self, close: Close<T::Node>) {
        match close {
            Close::Block => self.out.newline(),
            Close::Marker(kind) => {
                let marker = kind.marker().expect("formatting close has a marker");
                self.out.close_marker(marker, marker);
            }
            Close::TableCellSeparator => {
                self.out.markup(" | ");
                self.out.last_text_char = None;
            }
            Close::TableRow => {
                self.out.markup(" |");
                self.out.newline();
            }
            Close::TableHeaderSeparator(columns) => {
                self.out.markup("| ");
                for column in 0..columns {
                    if column > 0 {
                        self.out.markup(" | ");
                    }
                    self.out.markup("---");
                }
                self.out.markup(" |");
                self.out.newline();
            }
            Close::Table => {
                self.table_rows.pop();
                self.table_gfm.pop();
            }
            Close::Link(id) => {
                if self.out.close_marker("[", "](") {
                    // HTML whitespace at the end of the label belongs after
                    // the complete Markdown link, not in its destination.
                    let trailing_space = std::mem::take(&mut self.out.pending_space);
                    let href = self
                        .dom
                        .attr(id, AttrName::Href)
                        .and_then(|href| safe_destination(href, DestinationKind::Link))
                        .expect("validated link destination changed during serialization");
                    self.out.destination(href);
                    if let Some(title) = self.dom.attr_by_local_name(id, "title") {
                        self.out.markup(" \"");
                        self.out.link_title(title);
                        self.out.markup("\"");
                    }
                    self.out.markup(")");
                    self.out.pending_space = trailing_space;
                    if self.has_visible_inline_content(id) && !self.has_formatting_ancestor(id) {
                        self.out.mark_inline_boundary();
                    }
                }
            }
            Close::Quote => {
                if self.out.has_current_line_content() {
                    self.out.newline();
                }
                debug_assert!(matches!(self.out.prefixes.last(), Some(Prefix::Quote)));
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
                debug_assert!(matches!(
                    self.out.prefixes.last(),
                    Some(Prefix::ListItem { .. })
                ));
                self.out.prefixes.pop();
            }
            Close::Footnote => {
                if self.out.has_current_line_content() {
                    self.out.newline();
                }
                debug_assert!(matches!(self.out.prefixes.last(), Some(Prefix::Indent(4))));
                self.out.prefixes.pop();
            }
        }
    }
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
        for marker in &mut self.markers {
            if !marker.opened {
                self.value.push_str(marker.value);
                marker.opened = true;
                self.line_text_state = LineTextState::Other;
            }
        }
    }

    fn close_marker(&mut self, opening: &str, closing: &str) -> bool {
        let marker = self.markers.pop().expect("marker close without an open");
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

#[derive(Clone, Copy)]
enum DestinationKind {
    Link,
    Image,
}

/// Returns only destinations that cannot invoke an active or local URI scheme.
/// Relative URLs remain valid because callers can extract without a base URL.
fn safe_destination(value: &str, kind: DestinationKind) -> Option<&str> {
    let value = value.trim_matches(|ch: char| ch.is_ascii_whitespace() || ch.is_control());
    if value.is_empty() || value.chars().any(char::is_control) {
        return None;
    }

    let Some(end) = scheme_end(value) else {
        return Some(value);
    };
    let scheme = &value[..end];
    if scheme.is_empty()
        || !scheme.as_bytes()[0].is_ascii_alphabetic()
        || !scheme
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'-' | b'.'))
    {
        return None;
    }

    let allowed = match kind {
        DestinationKind::Link => {
            matches_ignore_ascii_case(scheme, &["http", "https", "mailto", "tel"])
        }
        DestinationKind::Image => matches_ignore_ascii_case(scheme, &["http", "https"]),
    };
    allowed.then_some(value)
}

fn matches_ignore_ascii_case(value: &str, allowed: &[&str]) -> bool {
    allowed.iter().any(|item| value.eq_ignore_ascii_case(item))
}

fn scheme_end(value: &str) -> Option<usize> {
    let mut offset = 0;
    while let Some(ch) = value[offset..].chars().next() {
        match ch {
            ':' => return Some(offset),
            '/' | '?' | '#' => return None,
            '&' if is_colon_entity(&value[offset..]) => return Some(offset),
            _ => offset += ch.len_utf8(),
        }
    }
    None
}

fn is_colon_entity(value: &str) -> bool {
    if value.starts_with("&colon;") {
        return true;
    }
    let Some(mut value) = value.strip_prefix("&#") else {
        return false;
    };
    let (radix, value_after_prefix) = value
        .strip_prefix('x')
        .or_else(|| value.strip_prefix('X'))
        .map_or((10, value), |value| (16, value));
    value = value_after_prefix;
    let digit_len = value
        .bytes()
        .take_while(|&byte| match radix {
            10 => byte.is_ascii_digit(),
            16 => byte.is_ascii_hexdigit(),
            _ => false,
        })
        .count();
    digit_len > 0 && u32::from_str_radix(&value[..digit_len], radix) == Ok(':' as u32)
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

    fn markdown(html: &str) -> String {
        let dom = Dom::parse_fragment(html, Tag::Div).unwrap();
        dom_to_markdown(&dom, dom.root(), 0)
    }

    #[test]
    fn preserves_checkbox_state_in_list_items() {
        assert_eq!(
            markdown(
                r#"<ul><li><label><input type="checkbox"> Unchecked</label></li><li><label><input type="checkbox" checked> Completed</label></li><li>Optional <input type="checkbox"></li><li><input type="checkbox" checked> First <input type="checkbox"> second</li><li><form><input type="checkbox"> Form option</form></li><li><input type="checkbox" aria-label="Deploy release"></li></ul>"#
            ),
            "- [ ] Unchecked\n- [x] Completed\n- Optional\n- [x] First second\n-  Form option\n- [ ] Deploy release\n"
        );
    }

    #[test]
    fn extraction_preserves_checklist_state_and_accessible_labels() {
        let page = crate::extract(
            r#"<article><h1>Release procedure</h1><p>This procedure explains each release step and provides enough context for the complete checklist.</p><ul><li><label><input type="checkbox"> Prepare build</label></li><li><input type="checkbox" checked aria-label="Deploy release" onclick="bad()"></li></ul><p>Verify the deployment after all required steps are complete.</p></article>"#,
            None,
        )
        .unwrap();
        let markdown = page.markdown();
        assert!(markdown.contains("- [ ] Prepare build"), "{markdown}");
        assert!(markdown.contains("- [x] Deploy release"), "{markdown}");
        let html = page.html();
        assert!(html.contains("aria-label=\"Deploy release\""), "{html}");
        assert!(html.contains("disabled=\"\""), "{html}");
        assert!(!html.contains("onclick"), "{html}");
    }

    #[test]
    fn basic_blocks_and_inline_content() {
        assert_eq!(
            markdown("<h1>Title</h1><p><strong>bold</strong> and <em>italic</em></p>"),
            "# Title\n\n**bold** and *italic*\n"
        );
        assert_eq!(
            markdown("<p>Line one<br>Line two</p><hr>"),
            "Line one  \nLine two\n\n---\n"
        );
    }

    #[test]
    fn headings_paragraphs_and_transparent_containers() {
        assert_eq!(
            markdown("<h1>A</h1><h2>B</h2><h3>C</h3><h4>D</h4><h5>E</h5><h6>F</h6>"),
            "# A\n\n## B\n\n### C\n\n#### D\n\n##### E\n\n###### F\n"
        );
        assert_eq!(
            markdown("<article><section><div><p>One</p><p>Two</p></div></section></article>"),
            "One\n\nTwo\n"
        );
        assert_eq!(
            markdown("<div>First</div><div>Second</div><section>Third</section>"),
            "First\n\nSecond\n\nThird\n"
        );
        assert_eq!(
            markdown("text<!-- hidden --><custom> more</custom>"),
            "text more\n"
        );
        assert_eq!(markdown("<div><span></span></div>"), "");
    }

    #[test]
    fn image_only_heading_without_alt_text_keeps_the_image() {
        assert_eq!(
            markdown(r#"<h2><img src="diagram.png"></h2><p>Explanation.</p>"#),
            "![](diagram.png)\n\nExplanation.\n"
        );
    }

    #[test]
    fn image_only_heading_is_empty_when_images_are_disabled() {
        let dom = Dom::parse_fragment(
            r#"<h2><img src="diagram.png" alt="Diagram"></h2><p>Explanation.</p>"#,
            Tag::Div,
        )
        .unwrap();
        assert_eq!(
            render_markdown(
                &dom,
                dom.root(),
                0,
                MarkdownConfig {
                    links: true,
                    images: false,
                },
            ),
            "Explanation.\n"
        );
    }

    #[test]
    fn inline_formatting_and_empty_elements() {
        assert_eq!(
            markdown("<p><b>bold <i>italic</i></b> <strong>strong</strong> <em>em</em></p>"),
            "**bold *italic*** **strong** *em*\n"
        );
        assert_eq!(
            markdown(
                "<p><strong>one</strong><b>two</b><em>three</em><i>four</i><code>a</code><code>b</code></p>"
            ),
            "**onetwo** *threefour* `ab`\n"
        );
        assert_eq!(
            markdown("<p>before <b></b><i> </i> after</p>"),
            "before after\n"
        );
        assert_eq!(
            markdown("<p><abbr>abbr</abbr> <del>deleted</del> H<sub>2</sub>O x<sup>2</sup></p>"),
            "abbr deleted H2O x2\n"
        );
    }

    #[test]
    fn inserts_spaces_at_word_boundaries_between_inline_nodes() {
        assert_eq!(
            markdown(concat!(
                "<p><strong>Move anything</strong><span>Drag the headline out of place.</span></p>",
                "<p><a href=\"/home\">Home</a><a href=\"/journal\">Journal</a><a href=\"/careers\">Careers</a></p>",
                "<p>Use <code>DES AAPL</code><span>or</span><code>TOP</code>.</p>",
                "<p>Keep foo<span>!</span>bar and <a href=\"/search?q=one+two\">results</a> together.</p>",
                "<p>foo<span></span>bar and <strong>joined<span>text</span></strong>.</p>",
                "<p>H<sub>2<span>x</span></sub>O remains compact.</p>",
                "<div><span>Card title</span><span>Card description</span></div>",
                "<div>Next tile</div>",
            )),
            concat!(
                "**Move anything** Drag the headline out of place.\n\n",
                "[Home](/home) [Journal](/journal) [Careers](/careers)\n\n",
                "Use `DES AAPL` or `TOP`.\n\n",
                "Keep foo!bar and [results](/search?q=one+two) together.\n\n",
                "foobar and **joinedtext**.\n\n",
                "H2xO remains compact.\n\n",
                "Card title Card description\n\n",
                "Next tile\n",
            )
        );
    }

    #[test]
    fn links_images_and_markdown_are_escaped() {
        assert_eq!(
            markdown(
                r#"<p>[text] <a href="https://x.test/a_(b)" title="say &quot;hi&quot;">link</a> <img src="a(b).png" alt="[alt]"></p>"#
            ),
            r#"\[text\] [link](https://x.test/a_\(b\) "say \"hi\"") ![\[alt\]](a\(b\).png)
"#
        );
    }

    #[test]
    fn link_and_image_fallbacks_and_attributes() {
        assert_eq!(
            markdown(
                r#"<p><a href="">plain</a> <a href="relative path" title="line&#10;title">relative</a></p><img alt="missing"><img src="image.png" alt="photo" title="A &quot;title&quot;">"#
            ),
            "plain [relative](relative%20path \"line title\")\nmissing![photo](image.png \"A \\\"title\\\"\")\n"
        );
        assert_eq!(
            markdown(r#"<p>before<a href="/x"> link </a>after <a href="/empty"> </a>end</p>"#),
            "before [link](/x) after end\n"
        );
    }

    #[test]
    fn lists_are_structural() {
        assert_eq!(
            markdown("<ul><li>One</li><li>Two<ul><li>Nested</li></ul></li></ul>"),
            "- One\n- Two\n  - Nested\n"
        );
        assert_eq!(
            markdown("<ul><li># title</li><li>- item</li><li>1. item</li></ul>"),
            "- \\# title\n- \\- item\n- 1\\. item\n"
        );
        assert_eq!(
            markdown("<ol start=3><li>Three</li><li>Four</li></ol>"),
            "3. Three\n1. Four\n"
        );
        assert_eq!(
            markdown("<ol start=\"+3\"><li>Plus</li></ol><ol start=\"0003\"><li>Zeros</li></ol>"),
            "3. Plus\n\n3. Zeros\n"
        );
        assert_eq!(
            markdown("<ol start=12><li>Outer<ul><li>Inner</li></ul></li></ol>"),
            "12. Outer\n    - Inner\n"
        );
        assert_eq!(
            markdown("<ul><li><p>First paragraph</p><p>Second paragraph</p></li></ul>"),
            "- First paragraph\n\n  Second paragraph\n"
        );
        assert_eq!(markdown("<li>orphan</li>"), "orphan\n");
        assert_eq!(
            markdown("<ul>intro<li>item</li><div>outro</div></ul>"),
            "intro\n- item\n\noutro\n"
        );
        assert_eq!(
            markdown("<ul><li>before<p>after</p>between<p>last</p></li></ul>"),
            "- before\n\n  after\n  between\n\n  last\n"
        );
        assert_eq!(
            markdown("<ul><li><div>block</div><p>paragraph</p></li></ul>"),
            "- block\n\n  paragraph\n"
        );
        assert_eq!(
            markdown("<ul><li><p>one<br>two</p></li></ul>"),
            "- one  \n  two\n"
        );
        assert_eq!(
            markdown("<ul><li>item<blockquote><p>quote</p></blockquote></li></ul>"),
            "- item\n\n  > quote\n"
        );
        assert_eq!(
            markdown("<ul><li>item<pre>code\n</pre></li></ul>"),
            "- item\n\n  ```\n  code\n  ```\n"
        );
    }

    #[test]
    fn blockquotes_and_code_choose_safe_fences() {
        assert_eq!(
            markdown(concat!(
                "<pre> ",
                "\n<code data-language=\"rust\">fn main() {}</code></pre>"
            )),
            "```rust\n \nfn main() {}\n```\n"
        );
        assert_eq!(
            markdown("<blockquote><p>One</p><p>Two</p></blockquote>"),
            "> One\n>\n> Two\n"
        );
        assert_eq!(
            markdown("<pre><code>```\nx\n</code></pre>"),
            "````\n```\nx\n````\n"
        );
        assert_eq!(markdown("<p><code>`x`</code></p>"), "`` `x` ``\n");
        assert_eq!(markdown("<p><code>   </code></p>"), "` `\n");
        assert_eq!(markdown("<p><code> x</code></p>"), "` x`\n");
        assert_eq!(markdown("<p><code> `x</code></p>"), "`` `x``\n");
        assert_eq!(
            markdown("<p><code>`<span>`</span> x</code><code> y`</code></p>"),
            "``` `` x y` ```\n"
        );
        assert_eq!(
            markdown("<pre>a<span>``</span>`\n</pre>"),
            "````\na```\n````\n"
        );
        assert_eq!(
            markdown("<blockquote><blockquote><p>Nested</p></blockquote></blockquote>"),
            "> > Nested\n"
        );
        assert_eq!(
            markdown("<pre>no final newline</pre>"),
            "```\nno final newline\n```\n"
        );
    }

    #[test]
    fn whitespace_around_emphasis_stays_outside_markers() {
        assert_eq!(markdown("<p>A <b> bold </b> word</p>"), "A **bold** word\n");
    }

    #[test]
    fn text_that_looks_like_markdown_stays_text() {
        assert_eq!(
            markdown(
                "<p>- item</p><p>+ item</p><p>1. item</p><p>2) item</p><p># title</p><p>***</p>"
            ),
            "\\- item\n\n\\+ item\n\n1\\. item\n\n2\\) item\n\n\\# title\n\n\\*\\*\\*\n"
        );
        assert_eq!(
            markdown(
                "<p>literal [link](javascript:x), ![image](data:x), `code`, &lt;tag&gt;, a_b, and a|b</p>"
            ),
            "literal \\[link\\](javascript:x), \\!\\[image\\](data:x), \\`code\\`, \\<tag\\>, a\\_b, and a\\|b\n"
        );
    }

    #[test]
    fn whitespace_is_collapsed_except_in_preformatted_code() {
        assert_eq!(
            markdown("<p>  one\n\t two <span> three </span> </p><pre> a\n  b </pre>"),
            "one two three\n\n```\n a\n  b \n```\n"
        );
    }

    #[test]
    fn code_blocks_preserve_indentation_tabs_and_blank_lines() {
        assert_eq!(
            markdown(concat!(
                "<pre><code>",
                "    indented\n",
                "\n",
                "\twith_tab\n",
                "  leading and trailing  \n",
                "</code></pre>"
            )),
            concat!(
                "```\n",
                "    indented\n",
                "\n",
                "\twith_tab\n",
                "  leading and trailing  \n",
                "```\n"
            )
        );
    }

    #[test]
    fn code_blocks_canonicalize_only_the_fence_boundary_newline() {
        // A fenced block needs a line break before its closing fence. Therefore,
        // source with no final newline and source with one final newline have the
        // same Markdown representation. Additional final newlines remain code.
        assert_eq!(
            markdown("<pre><code>value</code></pre>"),
            "```\nvalue\n```\n"
        );
        assert_eq!(
            markdown("<pre><code>value\n</code></pre>"),
            "```\nvalue\n```\n"
        );
        assert_eq!(
            markdown("<pre><code>value\n\n</code></pre>"),
            "```\nvalue\n\n```\n"
        );
        assert_eq!(
            markdown("<pre><code>value  \n</code></pre>"),
            "```\nvalue  \n```\n"
        );
    }

    #[test]
    fn tables_use_valid_gfm_syntax() {
        assert_eq!(
            markdown("<table><tr><th>A</th><th>B</th></tr><tr><td>1</td><td>2</td></tr></table>"),
            "| A | B |\n| --- | --- |\n| 1 | 2 |\n"
        );
        assert_eq!(
            markdown(
                "<table><tbody><tr><td>A|B</td><td><strong>C</strong></td></tr></tbody></table>"
            ),
            "| A\\|B | **C** |\n| --- | --- |\n"
        );
        assert_eq!(
            markdown(
                "<table><tr><td>Outer<table><tr><td>Inner</td></tr></table></td></tr></table>"
            ),
            "Outer\n\nInner\n"
        );
    }

    #[test]
    fn unsafe_destinations_are_rendered_as_plain_text() {
        assert_eq!(
            markdown(
                r#"<p><a href="jav&#x61;script:alert(1)">script</a> <a href="VBScript:x">vb</a> <a href="file:///secret">file</a> <a href="data:text/html,x">data</a></p><p><img src="data:image/svg+xml,x" alt="unsafe image"></p>"#
            ),
            "script vb file data\n\nunsafe image\n"
        );
        assert_eq!(
            markdown(
                r#"<p><a href="HTTPS://example.com">web</a> <a href="mailto:a@example.com">mail</a> <a href="/relative">relative</a></p><img src="https://example.com/image.png" alt="safe">"#
            ),
            "[web](HTTPS://example.com) [mail](mailto:a@example.com) [relative](/relative)\n![safe](https://example.com/image.png)\n"
        );
        assert_eq!(
            markdown(
                r#"<p>!<a href="mailto:a@example.com">mail</a> !<a href="tel:123">tel</a> !<a href="/x">relative</a></p>"#
            ),
            "\\![mail](mailto:a@example.com) \\![tel](tel:123) \\![relative](/x)\n"
        );
        assert_eq!(
            markdown(r#"<p>!<a href="/bracket">[x]</a></p>"#),
            "\\![\\[x\\]](/bracket)\n"
        );
        assert_eq!(
            markdown(
                r#"<p><a href="javascript&amp;colon;alert(1)">named</a> <a href="javascript&amp;#58;alert(1)">decimal</a> <a href="javascript&amp;#x3a;alert(1)">hex</a> <a href="javascript&amp;#X3a;alert(1)">upper</a> <a href="javascript&amp;#x00003A;alert(1)">padded</a> <a href="javascript&amp;#58alert(1)">no-semi</a></p><img src="data&amp;colon;image/svg+xml,x" alt="image"> <img src="data&amp;#58image/svg+xml,x" alt="no-semi-image">"#
            ),
            "named decimal hex upper padded no-semi\nimage no-semi-image\n"
        );
    }

    #[test]
    fn destination_allowlist_handles_edge_cases() {
        for value in [
            "http://example.com",
            "HTTPS://example.com",
            "mailto:a@example.com",
            "tel:+12025550123",
            "#fragment",
            "/relative",
            "../relative",
            "//example.com/path",
            "?query=value",
        ] {
            assert_eq!(safe_destination(value, DestinationKind::Link), Some(value));
        }
        for value in [
            "javascript:x",
            "JaVaScRiPt:x",
            "java\nscript:x",
            "vbscript:x",
            "data:text/html,x",
            "file:///etc/passwd",
            "ftp://example.com",
            "blob:https://example.com/id",
            "custom:x",
            ":x",
            "",
        ] {
            assert_eq!(safe_destination(value, DestinationKind::Link), None);
        }
        assert_eq!(
            safe_destination("data:image/png;base64,x", DestinationKind::Image),
            None
        );
        assert_eq!(
            safe_destination("mailto:a@example.com", DestinationKind::Image),
            None
        );
        assert_eq!(
            safe_destination(" https://example.com ", DestinationKind::Image),
            Some("https://example.com")
        );
    }

    #[test]
    fn html_like_text_is_escaped_and_active_elements_are_ignored() {
        assert_eq!(
            markdown(
                "<p>&lt;img src=x onerror=alert(1)&gt;</p><script>alert(1)</script><style>x</style>"
            ),
            "\\<img src=x onerror=alert(1)\\>\n"
        );
    }

    #[test]
    fn extracted_page_uses_the_final_cleaned_dom() {
        let html = "<html><head><title>Test</title></head><body><article><p>This is enough article text for extraction and Markdown output.</p><p><a href=\"javascript:alert(1)\">Unsafe link</a></p></article></body></html>";
        let page = crate::extract(html, Some("https://example.com")).unwrap();
        let markdown = page.markdown();
        assert!(markdown.contains("This is enough article text"));
        assert!(markdown.contains("Unsafe link"));
        assert!(!markdown.contains("javascript:"));
        assert!(!markdown.contains('<'));
    }

    #[test]
    fn code_block_ignores_empty_text_after_its_final_newline() {
        let mut dom = Dom::parse_fragment("<pre>code\n</pre>", Tag::Div).unwrap();
        let pre = dom
            .descendants(dom.root())
            .find(|&node| dom.tag(node) == Some(Tag::Pre))
            .unwrap();
        let empty = dom.create_text("").unwrap();
        dom.append_child(pre, empty);
        let root = dom.root();
        assert_eq!(dom_to_markdown(&dom, root, 0), "```\ncode\n```\n");
    }

    #[test]
    fn deeply_nested_inline_content_does_not_recurse() {
        let depth = 20_000;
        let mut html = "<span>".repeat(depth);
        html.push_str("text");
        html.push_str(&"</span>".repeat(depth));
        assert_eq!(markdown(&html), "text\n");
    }

    #[test]
    fn deeply_nested_lists_do_not_recurse_or_rescan_subtrees() {
        let depth = 500;
        let mut html = "<ul><li>".repeat(depth);
        html.push_str("leaf");
        html.push_str(&"</li></ul>".repeat(depth));
        let output = markdown(&html);
        assert!(output.contains("leaf"));
        assert_eq!(output.lines().count(), depth);
    }
}
