use crate::dom::{AttrName, Attribute as DomAttribute, Dom, NodeData, NodeId};
use html5ever::QualName;
use html5ever::serialize::{self, Serialize, Serializer, TraversalScope};
use std::io;
use tendril::StrTendril;

#[derive(Debug)]
pub(crate) struct ArticleTree {
    nodes: Box<[ArticleNode]>,
    root: usize,
}
#[derive(Debug)]
struct ArticleNode {
    kind: Kind,
    first_child: Option<usize>,
    next_sibling: Option<usize>,
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
            first_child: None,
            next_sibling: None,
        }];
        let mut stack: Vec<(NodeId, usize)> = dom.children(root).map(|id| (id, 0)).collect();
        stack.reverse();
        let mut last_child: Vec<Option<usize>> = vec![None];
        while let Some((id, parent)) = stack.pop() {
            let kind = match &dom.node(id).data {
                NodeData::Document | NodeData::Fragment => {
                    let mut children: Vec<_> = dom.children(id).collect();
                    children.reverse();
                    for child in children {
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
            let index = nodes.len();
            nodes.push(ArticleNode {
                kind,
                first_child: None,
                next_sibling: None,
            });
            last_child.push(None);
            if let Some(previous) = last_child[parent] {
                nodes[previous].next_sibling = Some(index)
            } else {
                nodes[parent].first_child = Some(index)
            }
            last_child[parent] = Some(index);
            let mut children: Vec<_> = match &dom.node(id).data {
                NodeData::Element(element) if element.template_contents.get().is_some() => dom
                    .children(
                        element
                            .template_contents
                            .get()
                            .expect("checked template root"),
                    )
                    .collect(),
                _ => dom.children(id).collect(),
            };
            children.reverse();
            for child in children {
                stack.push((child, index));
            }
        }
        Self {
            nodes: nodes.into_boxed_slice(),
            root: 0,
        }
    }
    pub(crate) fn to_html(&self) -> String {
        self.to_html_filtered(true, true)
    }
    fn to_html_filtered(&self, include_links: bool, include_images: bool) -> String {
        let mut bytes = Vec::new();
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
    pub(crate) fn to_text(&self) -> String {
        let mut output = NormalizedOutput::default();
        let mut tasks = Vec::new();
        push_tree_children(&mut tasks, self, self.root);
        while let Some(index) = tasks.pop() {
            match &self.nodes[index].kind {
                Kind::Text(text) => output.text(text),
                Kind::Element { name, .. } if name.local.as_ref() == "template" => {}
                Kind::Element { .. } | Kind::Root => push_tree_children(&mut tasks, self, index),
                _ => {}
            }
        }
        output.finish()
    }
    pub(crate) fn to_block_text(&self, block_newlines: bool, preserve_breaks: bool) -> String {
        enum Task {
            Node(usize),
            BlockEnd,
        }
        let separator = if block_newlines {
            Separator::Newline
        } else {
            Separator::Space
        };
        let mut output = NormalizedOutput::default();
        let mut tasks = Vec::new();
        let mut roots = Vec::new();
        let mut node = self.nodes[self.root].first_child;
        while let Some(index) = node {
            roots.push(index);
            node = self.nodes[index].next_sibling
        }
        for index in roots.into_iter().rev() {
            tasks.push(Task::Node(index))
        }
        while let Some(task) = tasks.pop() {
            match task {
                Task::BlockEnd => output.separator(separator),
                Task::Node(index) => match &self.nodes[index].kind {
                    Kind::Text(text) => output.text(text),
                    Kind::Element { name, .. } if name.local.as_ref() == "template" => {}
                    Kind::Element { name, .. }
                        if name.local.as_ref() == "br" && preserve_breaks =>
                    {
                        output.separator(Separator::Newline)
                    }
                    Kind::Element { name, .. } => {
                        let block = is_text_block(name.local.as_ref());
                        if block {
                            output.separator(separator);
                            tasks.push(Task::BlockEnd)
                        }
                        let mut children = Vec::new();
                        let mut child = self.nodes[index].first_child;
                        while let Some(child_index) = child {
                            children.push(child_index);
                            child = self.nodes[child_index].next_sibling
                        }
                        for child in children.into_iter().rev() {
                            tasks.push(Task::Node(child))
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
        let Some(dom) = self.to_dom_filtered(include_links, include_images) else {
            return String::new();
        };
        crate::markdown::dom_to_markdown(&dom, dom.root(), capacity)
    }
    fn to_dom_filtered(&self, include_links: bool, include_images: bool) -> Option<Dom> {
        let mut dom = Dom::new(NodeData::Fragment);
        let root = dom.root();
        let mut tasks = Vec::new();
        let mut roots = Vec::new();
        let mut node = self.nodes[self.root].first_child;
        while let Some(index) = node {
            roots.push(index);
            node = self.nodes[index].next_sibling
        }
        for index in roots.into_iter().rev() {
            tasks.push((index, root))
        }
        while let Some((index, parent)) = tasks.pop() {
            let created = match &self.nodes[index].kind {
                Kind::Root => continue,
                Kind::Text(text) => dom
                    .create(NodeData::Text(StrTendril::from(text.as_str())))
                    .ok()?,
                Kind::Comment(text) => dom
                    .create(NodeData::Comment(StrTendril::from(text.as_str())))
                    .ok()?,
                Kind::Element { name, .. } if name.local.as_ref() == "img" && !include_images => {
                    continue;
                }
                Kind::Element { name, .. } if name.local.as_ref() == "a" && !include_links => {
                    push_dom_children(&mut tasks, self, index, parent);
                    continue;
                }
                Kind::Element { name, attrs } => {
                    let id = dom.create_element(name.clone()).ok()?;
                    if let NodeData::Element(element) = &mut dom.node_mut(id).data {
                        element.attrs = attrs
                            .iter()
                            .map(|attr| DomAttribute {
                                name: attr.name.clone(),
                                known: AttrName::from_local(attr.name.local.as_ref()),
                                value: StrTendril::from(attr.value.as_str()),
                            })
                            .collect()
                    }
                    id
                }
            };
            dom.append_child(parent, created);
            push_dom_children(&mut tasks, self, index, created);
        }
        Some(dom)
    }
    #[cfg(test)]
    pub(crate) fn node_count(&self) -> usize {
        self.nodes.len()
    }
}

fn push_tree_children(tasks: &mut Vec<usize>, tree: &ArticleTree, parent: usize) {
    let mut children = Vec::new();
    let mut child = tree.nodes[parent].first_child;
    while let Some(index) = child {
        children.push(index);
        child = tree.nodes[index].next_sibling
    }
    tasks.extend(children.into_iter().rev());
}

fn is_text_block(name: &str) -> bool {
    matches!(
        name,
        "p" | "div"
            | "article"
            | "section"
            | "li"
            | "blockquote"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
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
    fn text(&mut self, text: &str) {
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
    Open(usize),
    Close(QualName),
}
fn push_dom_children(
    tasks: &mut Vec<(usize, NodeId)>,
    tree: &ArticleTree,
    parent: usize,
    dom_parent: NodeId,
) {
    let mut children = Vec::new();
    let mut node = tree.nodes[parent].first_child;
    while let Some(index) = node {
        children.push(index);
        node = tree.nodes[index].next_sibling
    }
    for index in children.into_iter().rev() {
        tasks.push((index, dom_parent))
    }
}
fn push_children(ops: &mut Vec<Op>, tree: &ArticleTree, parent: usize) {
    let mut children = Vec::new();
    let mut node = tree.nodes[parent].first_child;
    while let Some(index) = node {
        children.push(index);
        node = tree.nodes[index].next_sibling;
    }
    for index in children.into_iter().rev() {
        ops.push(Op::Open(index))
    }
}
impl Serialize for TreeSerializable<'_> {
    fn serialize<S: Serializer>(&self, ser: &mut S, _: TraversalScope) -> io::Result<()> {
        let mut ops = Vec::new();
        let mut roots = Vec::new();
        let mut n = self.tree.nodes[self.tree.root].first_child;
        while let Some(i) = n {
            roots.push(i);
            n = self.tree.nodes[i].next_sibling
        }
        for i in roots.into_iter().rev() {
            ops.push(Op::Open(i))
        }
        while let Some(op) = ops.pop() {
            match op {
                Op::Close(name) => ser.end_elem(name)?,
                Op::Open(i) => match &self.tree.nodes[i].kind {
                    Kind::Root => {}
                    Kind::Text(s) => ser.write_text(s)?,
                    Kind::Comment(s) => ser.write_comment(s)?,
                    Kind::Element { name, .. }
                        if name.local.as_ref() == "img" && !self.include_images => {}
                    Kind::Element { name, .. }
                        if name.local.as_ref() == "a" && !self.include_links =>
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
            tree.to_html(),
            "<div>visible<template><em>saved</em></template></div>"
        );
        assert_eq!(tree.to_text(), "visible");
        assert_eq!(tree.to_text().chars().count(), expected_count);
        assert!(!tree.to_markdown(expected_count).contains("saved"));
    }
}
