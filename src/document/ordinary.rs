//! Streaming compilation for HTML that has only ordinary semantic structure.

use super::compiler::{
    CompileContext, CompileError, has_single_content_child, heading_level, is_block_tag,
    is_redundant_formatting, semantic_image,
};
use super::{
    CodeBlock, Document, DocumentBuilder, DocumentNodeId, Link, List, ListKind, NodeKind, TextValue,
};
use crate::dom::{AttrName, Dom, NodeId, Tag};

#[derive(Clone, Copy, Default)]
struct ScanContext {
    forbids_blocks: bool,
    inside_code: bool,
    inside_figure: bool,
    inside_heading: bool,
    inside_link: bool,
    inside_pre: bool,
}

enum ScanTask {
    Node { node: NodeId, context: ScanContext },
    End,
}

struct ScanFrame {
    has_content: bool,
    requires_content: bool,
    first_visible: Option<char>,
    inventory_entry: Option<usize>,
}

pub(super) struct Inventory {
    first_visible: Vec<(NodeId, Option<char>)>,
    node_count: usize,
}

/// Inventories source features when they can be emitted without global analysis.
pub(super) fn inventory(dom: &Dom, root: NodeId) -> Option<Inventory> {
    let mut first_visible: Vec<(NodeId, Option<char>)> = Vec::new();
    let mut node_count = 0;
    let mut frames = vec![ScanFrame {
        has_content: false,
        requires_content: false,
        first_visible: None,
        inventory_entry: None,
    }];
    let mut tasks = dom
        .children_rev(root)
        .map(|node| ScanTask::Node {
            node,
            context: ScanContext::default(),
        })
        .collect::<Vec<_>>();

    while let Some(task) = tasks.pop() {
        let ScanTask::Node { node, context } = task else {
            let frame = frames.pop().expect("scan frame must match an element");
            if frame.requires_content && !frame.has_content {
                return None;
            }
            if let Some(entry) = frame.inventory_entry {
                first_visible[entry].1 = frame.first_visible;
            }
            let parent = frames.last_mut().expect("ordinary scan keeps a root frame");
            parent.has_content |= frame.has_content;
            parent.first_visible = parent.first_visible.or(frame.first_visible);
            continue;
        };
        node_count += 1;
        if let Some(text) = dom.text_node(node) {
            if context.inside_code && !context.inside_pre && text.contains('\n') {
                return None;
            }
            if let Some(first) = text.chars().find(|character| !character.is_whitespace()) {
                let frame = frames.last_mut().expect("ordinary scan keeps a root frame");
                frame.has_content = true;
                frame.first_visible = frame.first_visible.or(Some(first));
            }
            continue;
        }
        if dom.is_comment(node) {
            continue;
        }
        let Some(tag) = dom.tag(node) else {
            continue;
        };
        if requires_complex_source(dom, node, tag, context) {
            return None;
        }

        let block = is_block_tag(tag);
        if block && context.forbids_blocks {
            return None;
        }
        let requires_content = !block && !matches!(tag, Tag::Br | Tag::Code | Tag::Img)
            || matches!(
                tag,
                Tag::H1
                    | Tag::H2
                    | Tag::H3
                    | Tag::H4
                    | Tag::H5
                    | Tag::H6
                    | Tag::Strong
                    | Tag::B
                    | Tag::Em
                    | Tag::I
                    | Tag::Del
                    | Tag::A
            );
        let immediate_content = matches!(tag, Tag::Br | Tag::Hr | Tag::Img | Tag::Pre | Tag::Code);
        let inventory_entry = first_visible.len();
        first_visible.push((node, None));
        frames.push(ScanFrame {
            has_content: immediate_content,
            requires_content,
            first_visible: None,
            inventory_entry: Some(inventory_entry),
        });
        tasks.push(ScanTask::End);

        let next = ScanContext {
            forbids_blocks: context.forbids_blocks
                || !block
                || matches!(
                    tag,
                    Tag::Address
                        | Tag::P
                        | Tag::H1
                        | Tag::H2
                        | Tag::H3
                        | Tag::H4
                        | Tag::H5
                        | Tag::H6
                ),
            inside_code: context.inside_code || tag == Tag::Code,
            inside_figure: context.inside_figure || tag == Tag::Figure,
            inside_heading: context.inside_heading
                || matches!(
                    tag,
                    Tag::H1 | Tag::H2 | Tag::H3 | Tag::H4 | Tag::H5 | Tag::H6
                ),
            inside_link: context.inside_link || tag == Tag::A,
            inside_pre: context.inside_pre || tag == Tag::Pre,
        };
        tasks.extend(dom.children_rev(node).map(|child| ScanTask::Node {
            node: child,
            context: next,
        }));
    }
    Some(Inventory {
        first_visible,
        node_count,
    })
}

#[cfg(test)]
pub(super) fn supports(dom: &Dom, root: NodeId) -> bool {
    inventory(dom, root).is_some()
}

fn requires_complex_source(dom: &Dom, node: NodeId, tag: Tag, context: ScanContext) -> bool {
    if dom.attr(node, AttrName::Role).is_some() {
        return true;
    }
    let attributes = dom.attrs(node);
    if dom
        .attrs(node)
        .iter()
        .any(|attribute| attribute.name.local.as_ref().starts_with("aria-"))
    {
        return true;
    }
    let has_class_or_id =
        dom.attr(node, AttrName::Class).is_some() || dom.attr(node, AttrName::Id).is_some();
    let has_semantic_data = attributes.iter().any(|attribute| {
        matches!(
            attribute.name.local.as_ref(),
            "data-callout"
                | "data-footnote"
                | "data-footnote-ref"
                | "data-footnotes"
                | "data-formula"
                | "data-latex"
                | "data-legible-callout"
                | "data-legible-footnote"
                | "data-legible-footnote-ref"
                | "data-legible-footnotes"
                | "data-legible-math"
                | "data-math"
                | "data-tex"
                | "data-type"
        )
    });
    let fragment_link = tag == Tag::A
        && dom
            .attr(node, AttrName::Href)
            .is_some_and(|value| value.trim().starts_with('#'));
    if (has_class_or_id || has_semantic_data || fragment_link || tag == Tag::Img)
        && super::semantic_source_evidence(dom, node)
    {
        return true;
    }
    if !matches!(
        tag,
        Tag::Abbr
            | Tag::Address
            | Tag::Article
            | Tag::Aside
            | Tag::A
            | Tag::B
            | Tag::Bdi
            | Tag::Bdo
            | Tag::Blockquote
            | Tag::Br
            | Tag::Cite
            | Tag::Code
            | Tag::Dd
            | Tag::Del
            | Tag::Details
            | Tag::Dfn
            | Tag::Div
            | Tag::Dl
            | Tag::Dt
            | Tag::Em
            | Tag::Figcaption
            | Tag::Figure
            | Tag::Footer
            | Tag::H1
            | Tag::H2
            | Tag::H3
            | Tag::H4
            | Tag::H5
            | Tag::H6
            | Tag::Header
            | Tag::Hr
            | Tag::I
            | Tag::Img
            | Tag::Ins
            | Tag::Kbd
            | Tag::Li
            | Tag::Main
            | Tag::Mark
            | Tag::Nav
            | Tag::Ol
            | Tag::P
            | Tag::Pre
            | Tag::Q
            | Tag::Samp
            | Tag::Section
            | Tag::Small
            | Tag::Span
            | Tag::Strong
            | Tag::Sub
            | Tag::Summary
            | Tag::Sup
            | Tag::Time
            | Tag::U
            | Tag::Ul
            | Tag::Var
            | Tag::Wbr
    ) {
        return true;
    }
    let has_code_evidence_attribute = !attributes.is_empty()
        && (has_class_or_id
            || dom.attr(node, AttrName::DataLanguage).is_some()
            || dom.attr(node, AttrName::DataLang).is_some()
            || dom.attr(node, AttrName::DataCodeLanguage).is_some()
            || dom.attr(node, AttrName::Language).is_some()
            || dom.attr_by_local_name(node, "data-line").is_some()
            || dom
                .attr_by_local_name(node, "data-language-label")
                .is_some());
    if has_code_evidence_attribute && super::code::class_is_semantic_evidence(dom, node) {
        return true;
    }
    if has_class_or_id && super::figures::class_is_semantic_evidence(dom, node) {
        return true;
    }
    match tag {
        Tag::A => context.inside_link || fragment_link || context.inside_heading && has_class_or_id,
        Tag::Code => dom.children(node).any(|child| dom.is_element(child)),
        Tag::Pre => !simple_pre(dom, node),
        Tag::Img => {
            context.inside_heading
                || dom.attr(node, AttrName::Src).is_none()
                || dom
                    .attr(node, AttrName::Src)
                    .is_some_and(|source| !super::images::is_simple_source(source))
                || dom.attr(node, AttrName::Srcset).is_some()
                || dom.attr(node, AttrName::DataSrc).is_some()
                || dom.attr(node, AttrName::DataSrcset).is_some()
                || dom
                    .attrs(node)
                    .iter()
                    .any(|attribute| attribute.name.local.as_ref().starts_with("data-"))
        }
        Tag::Figure => !simple_figure(dom, node),
        Tag::Figcaption => dom.parent(node).and_then(|parent| dom.tag(parent)) != Some(Tag::Figure),
        Tag::Summary => {
            dom.parent(node).and_then(|parent| dom.tag(parent)) != Some(Tag::Details)
                || dom
                    .parent(node)
                    .is_some_and(|parent| dom.element_children(parent).next() != Some(node))
        }
        Tag::Li => !matches!(
            dom.parent(node).and_then(|parent| dom.tag(parent)),
            Some(Tag::Ul | Tag::Ol)
        ),
        Tag::Dt | Tag::Dd => dom.parent(node).and_then(|parent| dom.tag(parent)) != Some(Tag::Dl),
        Tag::Ul | Tag::Ol => !simple_native_list(dom, node),
        Tag::Dl => !simple_definition_list(dom, node),
        _ => false,
    }
}

fn simple_pre(dom: &Dom, node: NodeId) -> bool {
    let mut element_children = dom.element_children(node);
    let Some(child) = element_children.next() else {
        return true;
    };
    element_children.next().is_none()
        && dom.tag(child) == Some(Tag::Code)
        && !dom
            .children(child)
            .any(|descendant| dom.is_element(descendant))
}

fn simple_figure(dom: &Dom, node: NodeId) -> bool {
    let mut saw_image = false;
    let mut saw_caption = false;
    for child in dom.children(node) {
        if dom
            .text_node(child)
            .is_some_and(|text| !text.trim().is_empty())
        {
            return false;
        }
        let Some(tag) = dom.tag(child) else {
            continue;
        };
        match tag {
            Tag::Img if !saw_image && !saw_caption => saw_image = true,
            Tag::Figcaption if saw_image && !saw_caption => saw_caption = true,
            _ => return false,
        }
    }
    saw_image && saw_caption
}

fn simple_native_list(dom: &Dom, node: NodeId) -> bool {
    dom.children(node).all(|child| {
        dom.text_node(child)
            .is_some_and(|text| text.trim().is_empty())
            || dom.is_comment(child)
            || dom.tag(child) == Some(Tag::Li)
    })
}

fn simple_definition_list(dom: &Dom, node: NodeId) -> bool {
    dom.children(node).all(|child| {
        dom.text_node(child)
            .is_some_and(|text| text.trim().is_empty())
            || dom.is_comment(child)
            || matches!(dom.tag(child), Some(Tag::Dt | Tag::Dd))
    })
}

impl Inventory {
    fn first_visible(&self, cursor: &mut usize, node: NodeId) -> Option<char> {
        while self
            .first_visible
            .get(*cursor)
            .is_some_and(|(candidate, _)| *candidate != node)
        {
            *cursor += 1;
        }
        let (candidate, first) = self.first_visible.get(*cursor)?;
        if *candidate != node {
            return None;
        }
        *cursor += 1;
        *first
    }
}

#[derive(Clone, Copy, Default)]
struct Scope {
    parent: Option<DocumentNodeId>,
    list: Option<DocumentNodeId>,
    figure: Option<DocumentNodeId>,
    definition_list: Option<DocumentNodeId>,
    link: Option<DocumentNodeId>,
}

enum Visit {
    Node { node: NodeId, scope: Scope },
    End,
}

struct Frame {
    tag: Option<Tag>,
    scope: Scope,
    first_visible: Option<char>,
    last_visible: Option<char>,
    previous_child_inline_element: bool,
    previous_child_inline: bool,
    previous_child_last_visible: Option<char>,
}

/// Compiles an ordinary fragment in one stack-safe source traversal.
pub(super) fn compile(
    dom: &Dom,
    root: NodeId,
    context: &CompileContext,
    inventory: &Inventory,
) -> Result<Document, CompileError> {
    let mut builder = DocumentBuilder::with_capacity(inventory.node_count);
    let mut inventory_cursor = 0;
    let mut frames = vec![Frame {
        tag: None,
        scope: Scope::default(),
        first_visible: None,
        last_visible: None,
        previous_child_inline_element: false,
        previous_child_inline: false,
        previous_child_last_visible: None,
    }];
    let mut tasks = dom
        .children_rev(root)
        .map(|node| Visit::Node {
            node,
            scope: Scope::default(),
        })
        .collect::<Vec<_>>();

    while let Some(task) = tasks.pop() {
        let Visit::Node { node, scope } = task else {
            let frame = frames.pop().expect("compile frame must match an element");
            complete_child(
                frames
                    .last_mut()
                    .expect("ordinary compiler keeps a root frame"),
                frame.tag,
                frame.first_visible,
                frame.last_visible,
            );
            continue;
        };
        if let Some(text) = dom.text_node(node) {
            let first = text.chars().find(|character| !character.is_whitespace());
            let last = text
                .chars()
                .rev()
                .find(|character| !character.is_whitespace());
            let frame = frames
                .last_mut()
                .expect("ordinary compiler keeps a root frame");
            if first.is_none() {
                let next_is_inline = dom
                    .next_sibling(node)
                    .is_some_and(|sibling| is_inline_source(dom, sibling));
                if frame.previous_child_inline && next_is_inline {
                    builder.append_prose(frame.scope.parent, text)?;
                }
                complete_child(frame, None, first, last);
                continue;
            }
            if !text.chars().next().is_some_and(char::is_whitespace)
                && first.is_some_and(char::is_alphanumeric)
                && frame.previous_child_inline_element
                && frame
                    .previous_child_last_visible
                    .is_some_and(char::is_alphanumeric)
            {
                builder.append_prose(frame.scope.parent, " ")?;
            }
            builder.append_prose(frame.scope.parent, text)?;
            complete_child(frame, None, first, last);
            continue;
        }
        if dom.is_comment(node) {
            complete_child(
                frames
                    .last_mut()
                    .expect("ordinary compiler keeps a root frame"),
                None,
                None,
                None,
            );
            continue;
        }
        let Some(tag) = dom.tag(node) else {
            continue;
        };
        let subtree_first = inventory.first_visible(&mut inventory_cursor, node);

        if tag == Tag::Pre {
            let code = super::code::recognize_known_block(dom, node, true)
                .expect("ordinary routing accepts only simple pre blocks");
            let first = subtree_first;
            let last = code
                .text
                .chars()
                .rev()
                .find(|character| !character.is_whitespace());
            builder.append(
                scope.parent,
                NodeKind::CodeBlock(CodeBlock {
                    language: code.language.map(Into::into),
                    text: code.text.into(),
                }),
            )?;
            complete_child(frames.last_mut().unwrap(), Some(tag), first, last);
            continue;
        }
        if tag == Tag::Code {
            let text = dom.text(node);
            let first = subtree_first;
            builder.append(
                scope.parent,
                NodeKind::InlineCode(TextValue::new(text.clone())),
            )?;
            complete_child(
                frames.last_mut().unwrap(),
                Some(tag),
                first,
                text.chars()
                    .rev()
                    .find(|character| !character.is_whitespace()),
            );
            continue;
        }
        if tag == Tag::Img {
            let source = dom
                .attr(node, AttrName::Src)
                .and_then(|value| context.image_destination(value));
            if let Some(source) = source {
                builder.append(
                    scope.parent,
                    NodeKind::Image(semantic_image(dom, node, source)),
                )?;
            } else {
                let alt = super::images::canonical_label(dom.attr_by_local_name(node, "alt"));
                builder.append_prose(scope.parent, &alt)?;
            }
            complete_child(frames.last_mut().unwrap(), Some(tag), None, None);
            continue;
        }

        let mut next = scope;
        let parent_is_block_group = scope
            .parent
            .is_some_and(|parent| matches!(builder.kind(parent), Some(NodeKind::BlockGroup)));
        let kind = match tag {
            Tag::P | Tag::Address => Some(NodeKind::Paragraph),
            Tag::H1 | Tag::H2 | Tag::H3 | Tag::H4 | Tag::H5 | Tag::H6 => Some(NodeKind::Heading {
                level: heading_level(dom, node).unwrap_or(1),
            }),
            Tag::Blockquote => Some(NodeKind::BlockQuote),
            Tag::Ul => Some(NodeKind::List(List {
                kind: ListKind::Unordered,
                start: None,
            })),
            Tag::Ol => Some(NodeKind::List(List {
                kind: ListKind::Ordered,
                start: dom
                    .attr(node, AttrName::Start)
                    .and_then(|value| value.parse().ok()),
            })),
            Tag::Li if scope.list.is_some() => Some(NodeKind::ListItem),
            Tag::Figure => Some(NodeKind::Figure),
            Tag::Figcaption if scope.figure.is_some() => Some(NodeKind::Figcaption),
            Tag::Details => Some(NodeKind::Details),
            Tag::Summary => Some(NodeKind::Summary),
            Tag::Hr => Some(NodeKind::ThematicBreak),
            Tag::Dl => Some(NodeKind::DefinitionList),
            Tag::Dt if scope.definition_list.is_some() => Some(NodeKind::DefinitionTerm),
            Tag::Dd if scope.definition_list.is_some() => Some(NodeKind::DefinitionDescription),
            Tag::Strong | Tag::B => Some(NodeKind::Strong),
            Tag::Em | Tag::I => Some(NodeKind::Emphasis),
            Tag::Del => Some(NodeKind::Strikethrough),
            Tag::Br => Some(NodeKind::HardBreak),
            Tag::A if scope.link.is_none() => dom.attr(node, AttrName::Href).and_then(|value| {
                let trimmed = value.trim_matches(|character: char| {
                    character.is_ascii_whitespace() || character.is_control()
                });
                let fragment_only = trimmed.starts_with('#') && trimmed.len() > 1;
                context.link_destination(value).map(|destination| {
                    NodeKind::Link(Link {
                        destination,
                        title: dom.attr(node, AttrName::Title).map(Into::into),
                        fragment_only,
                    })
                })
            }),
            _ if is_block_tag(tag)
                && !(matches!(tag, Tag::Div | Tag::Section)
                    && parent_is_block_group
                    && has_single_content_child(dom, node)) =>
            {
                Some(NodeKind::BlockGroup)
            }
            _ => None,
        };
        let redundant = kind.as_ref().is_some_and(|kind| {
            is_redundant_formatting(kind, scope.parent.and_then(|parent| builder.kind(parent)))
        });
        let transparent = kind.is_none() || redundant;
        let transparent_inline = transparent && !is_block_tag(tag);
        let boundary_before = transparent_inline
            && frames.last().is_some_and(|frame| {
                frame.previous_child_inline_element
                    && frame
                        .previous_child_last_visible
                        .is_some_and(char::is_alphanumeric)
            });
        if boundary_before && subtree_first.is_some_and(char::is_alphanumeric) {
            builder.append_prose(scope.parent, " ")?;
        }
        let semantic_parent = if tag == Tag::Figcaption {
            scope.figure
        } else {
            scope.parent
        };
        let semantic = if transparent {
            None
        } else {
            Some(builder.append(
                semantic_parent,
                kind.expect("nontransparent node has a kind"),
            )?)
        };
        if let Some(semantic) = semantic {
            next.parent = Some(semantic);
            match tag {
                Tag::Ul | Tag::Ol => next.list = Some(semantic),
                Tag::Figure => next.figure = Some(semantic),
                Tag::Dl => next.definition_list = Some(semantic),
                Tag::A => next.link = Some(semantic),
                _ => {}
            }
        }
        frames.push(Frame {
            tag: Some(tag),
            scope: next,
            first_visible: None,
            last_visible: None,
            previous_child_inline_element: false,
            previous_child_inline: false,
            previous_child_last_visible: None,
        });
        tasks.push(Visit::End);
        if !matches!(tag, Tag::Hr | Tag::Br) {
            tasks.extend(dom.children_rev(node).map(|child| Visit::Node {
                node: child,
                scope: next,
            }));
        }
    }

    let document = builder.finish();
    #[cfg(any(test, debug_assertions))]
    document.validate()?;
    Ok(document)
}

fn complete_child(
    parent: &mut Frame,
    tag: Option<Tag>,
    first_visible: Option<char>,
    last_visible: Option<char>,
) {
    parent.first_visible = parent.first_visible.or(first_visible);
    if last_visible.is_some() {
        parent.last_visible = last_visible;
    }
    parent.previous_child_inline_element = tag.is_some_and(|tag| !is_block_tag(tag));
    parent.previous_child_inline = tag.map_or(last_visible.is_some(), |tag| !is_block_tag(tag));
    parent.previous_child_last_visible = last_visible;
}

fn is_inline_source(dom: &Dom, node: NodeId) -> bool {
    dom.text_node(node)
        .is_some_and(|text| text.chars().any(|character| !character.is_whitespace()))
        || dom.tag(node).is_some_and(|tag| !is_block_tag(tag))
}
