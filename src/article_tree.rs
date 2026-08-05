use crate::dom::{Dom, NodeData, NodeId, Tag};
use html5ever::QualName;
use html5ever::serialize::{self, Serialize, Serializer, TraversalScope};
use std::io;

#[derive(Debug)]
pub(crate) struct ArticleTree {
    nodes: Box<[ArticleNode]>,
    root: u32,
}
#[derive(Debug)]
struct ArticleNode {
    kind: Kind,
    first_child: u32,
    next_sibling: u32,
    tag: Tag,
}

const NO_NODE: u32 = u32::MAX;

fn node_index(index: u32) -> usize {
    index as usize
}

fn linked(index: u32) -> Option<u32> {
    (index != NO_NODE).then_some(index)
}
#[derive(Debug)]
enum Kind {
    Root,
    Element {
        name: QualName,
        attrs: Box<[OwnedAttribute]>,
    },
    Text(String),
    Comment(String),
}

impl ArticleTree {
    pub(crate) fn freeze(dom: &Dom, root: NodeId) -> Self {
        let mut nodes = vec![ArticleNode {
            kind: Kind::Root,
            first_child: NO_NODE,
            next_sibling: NO_NODE,
            tag: Tag::Other,
        }];
        let mut stack = dom.children_rev(root).map(|id| (id, 0)).collect::<Vec<_>>();
        let mut last_child = vec![NO_NODE];
        while let Some((id, parent)) = stack.pop() {
            let kind = match &dom.node(id).data {
                NodeData::Document | NodeData::Fragment => {
                    for child in dom.children_rev(id) {
                        stack.push((child, parent));
                    }
                    continue;
                }
                NodeData::Element(e) => {
                    let attrs = e
                        .attrs
                        .iter()
                        .map(|a| OwnedAttribute {
                            name: a.name.clone(),
                            value: a.value.to_string(),
                        })
                        .collect::<Vec<_>>()
                        .into_boxed_slice();
                    Kind::Element {
                        name: e.name.clone(),
                        attrs,
                    }
                }
                NodeData::Text(s) => Kind::Text(s.to_string()),
                NodeData::Comment(s) => Kind::Comment(s.to_string()),
                _ => continue,
            };
            let index = u32::try_from(nodes.len()).expect("article tree exceeds u32 capacity");
            let tag = dom.tag(id).unwrap_or(Tag::Other);
            nodes.push(ArticleNode {
                kind,
                first_child: NO_NODE,
                next_sibling: NO_NODE,
                tag,
            });
            last_child.push(NO_NODE);
            if last_child[node_index(parent)] != NO_NODE {
                let previous = last_child[node_index(parent)];
                nodes[node_index(previous)].next_sibling = index
            } else {
                nodes[node_index(parent)].first_child = index
            }
            last_child[node_index(parent)] = index;
            let child_root = match &dom.node(id).data {
                NodeData::Element(element) => element.template_contents.get().unwrap_or(id),
                _ => id,
            };
            for child in dom.children_rev(child_root) {
                stack.push((child, index));
            }
        }
        Self {
            nodes: nodes.into_boxed_slice(),
            root: 0,
        }
    }
    pub(crate) fn to_html(&self, capacity: usize) -> String {
        self.to_html_filtered(capacity, true, true)
    }
    fn to_html_filtered(
        &self,
        capacity: usize,
        include_links: bool,
        include_images: bool,
    ) -> String {
        let mut bytes = Vec::with_capacity(capacity);
        serialize::serialize(
            &mut bytes,
            &TreeSerializable {
                tree: self,
                include_links,
                include_images,
            },
            serialize::SerializeOpts {
                traversal_scope: TraversalScope::ChildrenOnly(None),
                ..Default::default()
            },
        )
        .expect("writing to Vec cannot fail");
        String::from_utf8(bytes).expect("HTML serialization is UTF-8")
    }
    pub(crate) fn to_text(&self, capacity: usize) -> String {
        enum Task {
            Siblings(u32),
        }
        let mut output = NormalizedOutput::with_capacity(capacity);
        let mut tasks = smallvec::SmallVec::<[Task; 32]>::new();
        if let Some(child) = linked(self.nodes[node_index(self.root)].first_child) {
            tasks.push(Task::Siblings(child));
        }
        while let Some(task) = tasks.pop() {
            let index = match task {
                Task::Siblings(index) => {
                    if let Some(next) = linked(self.nodes[node_index(index)].next_sibling) {
                        tasks.push(Task::Siblings(next));
                    }
                    index
                }
            };
            match &self.nodes[node_index(index)].kind {
                Kind::Text(text) => output.text(text),
                Kind::Element { .. } if self.nodes[node_index(index)].tag == Tag::Template => {}
                Kind::Element { .. } | Kind::Root => {
                    if let Some(child) = linked(self.nodes[node_index(index)].first_child) {
                        tasks.push(Task::Siblings(child));
                    }
                }
                _ => {}
            }
        }
        output.finish()
    }
    pub(crate) fn to_block_text(
        &self,
        capacity: usize,
        block_newlines: bool,
        preserve_breaks: bool,
    ) -> String {
        enum Task {
            Node(u32),
            Siblings(u32),
            BlockEnd,
        }
        let separator = if block_newlines {
            Separator::Newline
        } else {
            Separator::Space
        };
        let mut output = NormalizedOutput::with_capacity(capacity);
        let mut tasks = smallvec::SmallVec::<[Task; 32]>::new();
        if let Some(child) = linked(self.nodes[node_index(self.root)].first_child) {
            tasks.push(Task::Siblings(child));
        }
        while let Some(task) = tasks.pop() {
            match task {
                Task::BlockEnd => output.separator(separator),
                Task::Siblings(index) => {
                    if let Some(next) = linked(self.nodes[node_index(index)].next_sibling) {
                        tasks.push(Task::Siblings(next));
                    }
                    tasks.push(Task::Node(index));
                }
                Task::Node(index) => match &self.nodes[node_index(index)].kind {
                    Kind::Text(text) => output.text(text),
                    Kind::Element { .. } if self.nodes[node_index(index)].tag == Tag::Template => {}
                    Kind::Element { .. }
                        if self.nodes[node_index(index)].tag == Tag::Br && preserve_breaks =>
                    {
                        output.separator(Separator::Newline)
                    }
                    Kind::Element { .. } => {
                        let block = is_text_block(self.nodes[node_index(index)].tag);
                        if block {
                            output.separator(separator);
                            tasks.push(Task::BlockEnd)
                        }
                        if let Some(child) = linked(self.nodes[node_index(index)].first_child) {
                            tasks.push(Task::Siblings(child));
                        }
                    }
                    _ => {}
                },
            }
        }
        output.finish()
    }
    pub(crate) fn to_markdown(&self, capacity: usize) -> String {
        self.to_markdown_filtered(capacity, true, true)
    }
    pub(crate) fn to_markdown_filtered(
        &self,
        capacity: usize,
        include_links: bool,
        include_images: bool,
    ) -> String {
        crate::markdown::tree_to_markdown_filtered(
            self,
            self.root,
            capacity,
            include_links,
            include_images,
        )
    }
    #[cfg(test)]
    pub(crate) fn node_count(&self) -> usize {
        self.nodes.len()
    }
}

impl crate::markdown::MarkdownTree for ArticleTree {
    type Node = u32;

    fn first_child(&self, node: u32) -> Option<u32> {
        linked(self.nodes[node_index(node)].first_child)
    }

    fn next_sibling(&self, node: u32) -> Option<u32> {
        linked(self.nodes[node_index(node)].next_sibling)
    }

    fn tag(&self, node: u32) -> Option<Tag> {
        let node = &self.nodes[node_index(node)];
        matches!(node.kind, Kind::Element { .. }).then_some(node.tag)
    }

    fn text_node(&self, node: u32) -> Option<&str> {
        match &self.nodes[node_index(node)].kind {
            Kind::Text(text) => Some(text),
            _ => None,
        }
    }

    fn is_comment(&self, node: u32) -> bool {
        matches!(self.nodes[node_index(node)].kind, Kind::Comment(_))
    }

    fn attr_by_local_name(&self, node: u32, name: &str) -> Option<&str> {
        match &self.nodes[node_index(node)].kind {
            Kind::Element { attrs, .. } => attrs
                .iter()
                .find(|attr| attr.name.local.as_ref().eq_ignore_ascii_case(name))
                .map(|attr| attr.value.as_str()),
            _ => None,
        }
    }
}

fn is_text_block(tag: Tag) -> bool {
    matches!(
        tag,
        Tag::P
            | Tag::Div
            | Tag::Article
            | Tag::Section
            | Tag::Li
            | Tag::Blockquote
            | Tag::H1
            | Tag::H2
            | Tag::H3
            | Tag::H4
            | Tag::H5
            | Tag::H6
    )
}

#[derive(Clone, Copy, Default, Eq, PartialEq)]
enum Separator {
    #[default]
    None,
    Space,
    Newline,
}
#[derive(Default)]
struct NormalizedOutput {
    output: String,
    pending: Separator,
}
impl NormalizedOutput {
    fn with_capacity(capacity: usize) -> Self {
        Self {
            output: String::with_capacity(capacity),
            pending: Separator::None,
        }
    }

    fn text(&mut self, text: &str) {
        if text.is_ascii() {
            let bytes = text.as_bytes();
            let mut index = 0;
            while index < bytes.len() {
                if bytes[index].is_ascii_whitespace() {
                    if !self.output.is_empty() && self.pending == Separator::None {
                        self.pending = Separator::Space
                    }
                    index += 1;
                    continue;
                }
                self.flush();
                let start = index;
                while index < bytes.len() && !bytes[index].is_ascii_whitespace() {
                    index += 1;
                }
                self.output.push_str(&text[start..index]);
            }
            return;
        }

        for character in text.chars() {
            if character.is_whitespace() {
                if !self.output.is_empty() && self.pending == Separator::None {
                    self.pending = Separator::Space
                }
            } else {
                self.flush();
                self.output.push(character)
            }
        }
    }
    fn separator(&mut self, separator: Separator) {
        if !self.output.is_empty()
            && (separator == Separator::Newline || self.pending == Separator::None)
        {
            self.pending = separator
        }
    }
    fn flush(&mut self) {
        match self.pending {
            Separator::None => {}
            Separator::Space => self.output.push(' '),
            Separator::Newline => self.output.push('\n'),
        }
        self.pending = Separator::None
    }
    fn finish(self) -> String {
        self.output
    }
}

#[derive(Debug)]
struct OwnedAttribute {
    name: QualName,
    value: String,
}
struct TreeSerializable<'a> {
    tree: &'a ArticleTree,
    include_links: bool,
    include_images: bool,
}
enum Op {
    Open(u32),
    Siblings(u32),
    Close(QualName),
}
fn push_children(ops: &mut smallvec::SmallVec<[Op; 32]>, tree: &ArticleTree, parent: u32) {
    if let Some(child) = linked(tree.nodes[node_index(parent)].first_child) {
        ops.push(Op::Siblings(child));
    }
}
impl Serialize for TreeSerializable<'_> {
    fn serialize<S: Serializer>(&self, ser: &mut S, _: TraversalScope) -> io::Result<()> {
        let mut ops = smallvec::SmallVec::<[Op; 32]>::new();
        push_children(&mut ops, self.tree, self.tree.root);
        while let Some(op) = ops.pop() {
            match op {
                Op::Close(name) => ser.end_elem(name)?,
                Op::Siblings(i) => {
                    if let Some(next) = linked(self.tree.nodes[node_index(i)].next_sibling) {
                        ops.push(Op::Siblings(next));
                    }
                    ops.push(Op::Open(i));
                }
                Op::Open(i) => match &self.tree.nodes[node_index(i)].kind {
                    Kind::Root => {}
                    Kind::Text(s) => ser.write_text(s)?,
                    Kind::Comment(s) => ser.write_comment(s)?,
                    Kind::Element { .. }
                        if self.tree.nodes[node_index(i)].tag == Tag::Img
                            && !self.include_images => {}
                    Kind::Element { .. }
                        if self.tree.nodes[node_index(i)].tag == Tag::A && !self.include_links =>
                    {
                        push_children(&mut ops, self.tree, i);
                    }
                    Kind::Element { name, attrs } => {
                        ser.start_elem(
                            name.clone(),
                            attrs.iter().map(|a| (&a.name, a.value.as_ref())),
                        )?;
                        ops.push(Op::Close(name.clone()));
                        push_children(&mut ops, self.tree, i);
                    }
                },
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn freeze_preserves_template_contents() {
        let dom = Dom::parse_document(
            "<body><div>visible<template><em>saved</em></template></div></body>",
        )
        .unwrap();
        let body = dom.body().unwrap();
        let expected_count = dom.normalized_char_count(body);
        let tree = ArticleTree::freeze(&dom, body);
        assert_eq!(
            tree.to_html(expected_count),
            "<div>visible<template><em>saved</em></template></div>"
        );
        assert_eq!(tree.to_text(expected_count), "visible");
        assert_eq!(tree.to_text(expected_count).chars().count(), expected_count);
        assert!(!tree.to_markdown(expected_count).contains("saved"));
    }

    #[test]
    fn direct_renderers_match_the_cleaned_dom() {
        let depth = 1_000;
        let html = format!(
            "<body>{}<p>A <a href='https://example.test/a(b)'>link [x]</a><img src='image.jpg' alt='photo'></p>{}</body>",
            "<section>".repeat(depth),
            "</section>".repeat(depth)
        );
        let dom = Dom::parse_document(&html).unwrap();
        let body = dom.body().unwrap();
        let count = dom.normalized_char_count(body);
        let tree = ArticleTree::freeze(&dom, body);

        assert_eq!(tree.to_html(count), dom.inner_html(body).unwrap());
        assert_eq!(tree.to_text(count), dom.normalized_text(body, count).0);
        assert_eq!(
            tree.to_markdown(count),
            crate::markdown::dom_to_markdown(&dom, body, count)
        );
    }
}
