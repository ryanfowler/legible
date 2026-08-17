//! Streaming compilation for HTML that has only ordinary semantic structure.

use super::compiler::{
    CompileContext, CompileError, has_single_content_child, heading_level, is_block_tag,
    semantic_image,
};
use super::{
    BuildCapacityPlan, CodeBlock, Document, DocumentNodeId, Link, List, ListKind, SemanticKind,
    SemanticTapeBuilder,
};
use crate::dom::{AttrName, Dom, NodeId, Tag};

pub(super) struct OrdinarySourcePlan {
    pub(super) source_node_count: usize,
    pub(super) capacity: BuildCapacityPlan,
}

/// Returns the source-node count when a fragment is safe to try on the ordinary path.
///
/// This gate deliberately checks only broad source evidence. Structural
/// validation belongs to [`compile`], which can reject the fragment while it
/// is already lowering it. Keeping those decisions together removes the old
/// full inventory traversal from ordinary pages.
#[cfg(test)]
pub(super) fn ordinary_source_gate(dom: &Dom, root: NodeId) -> Option<usize> {
    ordinary_source_gate_with_retained_nodes(dom, root, None).map(|plan| plan.source_node_count)
}

/// Runs the ordinary gate over the source-order stream produced by final
/// relevance cleanup when one is available. This keeps routing and lowering on
/// the same retained source without rebuilding a DOM traversal.
pub(super) fn ordinary_source_gate_with_retained_nodes(
    dom: &Dom,
    root: NodeId,
    retained_nodes: Option<&[NodeId]>,
) -> Option<OrdinarySourcePlan> {
    if let Some(nodes) = retained_nodes {
        return ordinary_source_gate_from_nodes(dom, nodes);
    }

    let mut source_node_count = 0;
    let mut capacity = BuildCapacityPlan::default();
    let mut tasks: Vec<(NodeId, bool, bool, bool, usize)> = dom
        .children_rev(root)
        .map(|node| (node, false, false, false, 0))
        .collect();

    while let Some((node, inside_heading, inside_code, inside_pre, depth)) = tasks.pop() {
        source_node_count += 1;
        observe_capacity(dom, node, inside_code, inside_pre, depth, &mut capacity);
        if dom.text_node(node).is_some() || dom.is_comment(node) {
            continue;
        }
        let tag = dom.tag(node)?;
        if has_complex_tag(tag)
            || has_complex_attributes(dom, node)
            || tag == Tag::A
                && dom
                    .attr(node, AttrName::Href)
                    .is_some_and(|value| value.trim().starts_with('#'))
            || tag == Tag::Img
                && (inside_heading
                    || !simple_image_source(dom, node)
                    || super::math::class_is_semantic_evidence(dom, node))
        {
            return None;
        }
        let child_inside_heading = inside_heading || is_heading(tag);
        let child_inside_code = inside_code || tag == Tag::Code;
        let child_inside_pre = inside_pre || tag == Tag::Pre;
        let child_semantic_depth = depth.saturating_add(usize::from(is_capacity_container(tag)));
        tasks.extend(dom.children_rev(node).map(|child| {
            (
                child,
                child_inside_heading,
                child_inside_code,
                child_inside_pre,
                child_semantic_depth,
            )
        }));
    }
    Some(OrdinarySourcePlan {
        source_node_count,
        capacity,
    })
}

fn ordinary_source_gate_from_nodes(dom: &Dom, nodes: &[NodeId]) -> Option<OrdinarySourcePlan> {
    let mut ancestors = Vec::<(NodeId, bool, bool, bool, usize)>::new();
    let mut capacity = BuildCapacityPlan::default();
    let mut source_node_count = 0;
    for &node in nodes {
        let parent = dom.parent(node);
        while ancestors
            .last()
            .is_some_and(|&(ancestor, _, _, _, _)| parent != Some(ancestor))
        {
            ancestors.pop();
        }

        source_node_count += 1;
        let inside_code = ancestors.last().is_some_and(|&(_, _, value, _, _)| value);
        let inside_pre = ancestors.last().is_some_and(|&(_, _, _, value, _)| value);
        let semantic_depth = ancestors.last().map_or(0, |&(_, _, _, _, depth)| depth);
        observe_capacity(
            dom,
            node,
            inside_code,
            inside_pre,
            semantic_depth,
            &mut capacity,
        );
        if dom.text_node(node).is_some() || dom.is_comment(node) {
            continue;
        }
        let tag = dom.tag(node)?;
        let inside_heading = ancestors.last().is_some_and(|&(_, value, _, _, _)| value);
        if has_complex_tag(tag)
            || has_complex_attributes(dom, node)
            || tag == Tag::A
                && dom
                    .attr(node, AttrName::Href)
                    .is_some_and(|value| value.trim().starts_with('#'))
            || tag == Tag::Img
                && (inside_heading
                    || !simple_image_source(dom, node)
                    || super::math::class_is_semantic_evidence(dom, node))
        {
            return None;
        }
        ancestors.push((
            node,
            ancestors.last().is_some_and(|&(_, value, _, _, _)| value) || is_heading(tag),
            inside_code || tag == Tag::Code,
            inside_pre || tag == Tag::Pre,
            semantic_depth.saturating_add(usize::from(is_capacity_container(tag))),
        ));
    }
    Some(OrdinarySourcePlan {
        source_node_count,
        capacity,
    })
}

fn observe_capacity(
    dom: &Dom,
    node: NodeId,
    inside_code: bool,
    inside_pre: bool,
    depth: usize,
    plan: &mut BuildCapacityPlan,
) {
    plan.semantic_nodes = plan.semantic_nodes.saturating_add(1);
    let semantic_depth =
        depth.saturating_add(dom.tag(node).is_some_and(is_capacity_container).into());
    plan.max_depth = plan.max_depth.max(semantic_depth);

    if let Some(text) = dom.text_node(node) {
        if !inside_code && !inside_pre {
            plan.text_bytes = plan.text_bytes.saturating_add(text.len());
            plan.text_refs = plan.text_refs.saturating_add(1);
        }
        return;
    }

    let Some(tag) = dom.tag(node) else {
        return;
    };
    plan.containers = plan
        .containers
        .saturating_add(usize::from(is_capacity_container(tag)));
    match tag {
        Tag::Pre => plan.code_blocks = plan.code_blocks.saturating_add(1),
        Tag::Code => {
            if !inside_pre {
                plan.text_refs = plan.text_refs.saturating_add(1);
                if let Some(text) = dom.first_child(node).and_then(|child| dom.text_node(child)) {
                    plan.text_bytes = plan.text_bytes.saturating_add(text.len());
                }
            }
        }
        Tag::A => plan.links = plan.links.saturating_add(1),
        Tag::Img => plan.images = plan.images.saturating_add(1),
        Tag::Ol | Tag::Ul => plan.lists = plan.lists.saturating_add(1),
        _ => {}
    }
}

fn is_capacity_container(tag: Tag) -> bool {
    !matches!(
        tag,
        Tag::Br | Tag::Hr | Tag::Img | Tag::Pre | Tag::Iframe | Tag::Video | Tag::Audio
    ) && is_block_tag(tag)
        || matches!(
            tag,
            Tag::A
                | Tag::B
                | Tag::Strong
                | Tag::Em
                | Tag::I
                | Tag::Del
                | Tag::Figure
                | Tag::Figcaption
                | Tag::Details
                | Tag::Summary
                | Tag::Dl
                | Tag::Dt
                | Tag::Dd
        )
}

#[cfg(test)]
pub(super) fn supports(dom: &Dom, root: NodeId) -> bool {
    ordinary_source_gate(dom, root)
        .is_some_and(|count| compile(dom, root, &CompileContext::default(), count).is_ok())
}

fn has_complex_tag(tag: Tag) -> bool {
    !matches!(
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
    )
}

fn is_heading(tag: Tag) -> bool {
    matches!(
        tag,
        Tag::H1 | Tag::H2 | Tag::H3 | Tag::H4 | Tag::H5 | Tag::H6
    )
}

fn has_complex_attributes(dom: &Dom, node: NodeId) -> bool {
    if dom.attr(node, AttrName::Role).is_some() {
        return true;
    }
    let synthetic_boundary = dom.attr(node, AttrName::Id) == Some("legible-content")
        && dom.attr(node, AttrName::Class).is_some_and(|value| {
            value
                .split_whitespace()
                .all(|token| token.eq_ignore_ascii_case("page"))
        });
    if dom.attrs(node).iter().any(|attribute| {
        let name = attribute.name.local.as_ref();
        name.starts_with("aria-") || name.starts_with("data-")
    }) {
        return true;
    }
    if synthetic_boundary {
        return false;
    }
    [AttrName::Class, AttrName::Id]
        .into_iter()
        .filter_map(|attribute| dom.attr(node, attribute))
        .flat_map(str::split_whitespace)
        .any(class_or_id_is_semantic_hint)
}

fn class_or_id_is_semantic_hint(value: &str) -> bool {
    let value = value.to_ascii_lowercase();
    value.starts_with("language-")
        || value.starts_with("lang-")
        || value.starts_with("highlight-source-")
        || value.starts_with("mathjax_")
        || value.starts_with("fn")
        || value.starts_with("_ftn")
        || value.starts_with("ftnt")
        || value.starts_with("footnote")
        || value.starts_with("note-")
        || value.starts_with("sn")
        || value.starts_with("sidenote")
        || value.starts_with("cite_note")
        || value.starts_with("user-content-fn")
        || value.starts_with("footnotedef")
        || matches!(
            value.as_str(),
            "admonition"
                | "callout"
                | "alert"
                | "note"
                | "warning"
                | "tip"
                | "important"
                | "caution"
                | "info"
                | "information"
                | "reference"
                | "references"
                | "footnote-reference"
                | "footnote-ref"
                | "footnoteref"
                | "fnref"
                | "footnote-definition"
                | "footdef"
                | "footnote-backref"
                | "footnote-body"
                | "reference-text"
                | "mw-cite-backlink"
                | "sidenote"
                | "side-note"
                | "sidenote-number"
                | "footref"
                | "footref-toggle"
                | "margin-toggle"
                | "margin-note"
                | "marginnote"
                | "footnotes"
                | "footnote-list"
                | "footnote-definitions"
                | "footnote-container"
                | "footnotes-container"
                | "wp-block-footnotes"
                | "endnote"
                | "endnotes"
                | "katex"
                | "katex-display"
                | "mathjax"
                | "mathjax-display"
                | "tex2jax_process"
                | "formula"
                | "latex"
                | "tex"
                | "highlight"
                | "codehilite"
                | "sourcecode"
                | "code-block"
                | "codeblock"
                | "syntax-highlight"
                | "highlighttable"
                | "lntable"
                | "rouge-table"
                | "rouge-line-table"
                | "linenos"
                | "rouge-gutter"
                | "gutter"
                | "lnt"
                | "lineno"
                | "line-number"
                | "line-numbers"
                | "line-numbers-rows"
                | "line-number-gutter"
                | "linenodiv"
                | "line"
                | "code-header"
                | "code-language"
                | "code-lang"
                | "language-label"
                | "highlight-header"
                | "codeblock-title"
                | "figure"
                | "image-with-caption"
                | "media-with-caption"
                | "caption"
                | "figcaption"
                | "image-caption"
        )
}

fn simple_image_source(dom: &Dom, node: NodeId) -> bool {
    dom.attr(node, AttrName::Src)
        .is_some_and(super::images::is_simple_source)
        && dom.attr(node, AttrName::Srcset).is_none()
        && dom.attr(node, AttrName::DataSrc).is_none()
        && dom.attr(node, AttrName::DataSrcset).is_none()
        && !dom
            .attrs(node)
            .iter()
            .any(|attribute| attribute.name.local.as_ref().starts_with("data-"))
}

fn summary_is_first_child(dom: &Dom, node: NodeId) -> bool {
    let Some(parent) = dom.parent(node) else {
        return false;
    };
    if dom.tag(parent) != Some(Tag::Details) {
        return false;
    }
    for child in dom.children(parent) {
        if child == node {
            return true;
        }
        if !dom.is_comment(child)
            && !dom
                .text_node(child)
                .is_some_and(|text| text.trim().is_empty())
        {
            return false;
        }
    }
    false
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

#[derive(Clone, Copy, Default)]
struct Scope {
    parent: Option<DocumentNodeId>,
    list: Option<DocumentNodeId>,
    figure: Option<DocumentNodeId>,
    definition_list: Option<DocumentNodeId>,
    link: Option<DocumentNodeId>,
    forbids_blocks: bool,
    inside_code: bool,
    inside_heading: bool,
    inside_pre: bool,
}

enum Visit {
    Node { node: NodeId },
    End,
}

struct Frame {
    tag: Option<Tag>,
    scope: Scope,
    semantic: Option<DocumentNodeId>,
    has_content: bool,
    requires_content: bool,
    first_visible: Option<char>,
    last_visible: Option<char>,
    previous_child_inline_element: bool,
    previous_child_inline: bool,
    previous_child_last_visible: Option<char>,
    boundary_source: Option<usize>,
    boundary_pending: bool,
}

/// Compiles an ordinary fragment in one stack-safe source traversal.
#[cfg(test)]
pub(super) fn compile(
    dom: &Dom,
    root: NodeId,
    context: &CompileContext,
    source_node_count: usize,
) -> Result<Document, CompileError> {
    compile_with_retained_nodes(dom, root, context, source_node_count, None)
}

/// Compiles an ordinary fragment from the final-cleanup source stream when it
/// is available. The stream contains source nodes in preorder. Leaf semantic
/// elements still skip their source descendants, matching DOM traversal.
#[cfg(test)]
pub(super) fn compile_with_retained_nodes(
    dom: &Dom,
    root: NodeId,
    context: &CompileContext,
    source_node_count: usize,
    retained_nodes: Option<&[NodeId]>,
) -> Result<Document, CompileError> {
    compile_with_retained_capacity_plan(
        dom,
        root,
        context,
        BuildCapacityPlan::from_source_node_count(source_node_count),
        retained_nodes,
    )
}

pub(super) fn compile_with_retained_capacity_plan(
    dom: &Dom,
    root: NodeId,
    context: &CompileContext,
    capacity: BuildCapacityPlan,
    retained_nodes: Option<&[NodeId]>,
) -> Result<Document, CompileError> {
    let mut builder = SemanticTapeBuilder::with_plan(capacity);
    let mut frames = vec![Frame {
        tag: None,
        scope: Scope::default(),
        semantic: None,
        has_content: false,
        requires_content: false,
        first_visible: None,
        last_visible: None,
        previous_child_inline_element: false,
        previous_child_inline: false,
        previous_child_last_visible: None,
        boundary_source: None,
        boundary_pending: false,
    }];
    let mut tasks = retained_nodes.map_or_else(
        || {
            dom.children_rev(root)
                .map(|node| Visit::Node { node })
                .collect::<Vec<_>>()
        },
        |nodes| retained_source_tasks(dom, nodes),
    );

    macro_rules! requires_complex {
        () => {{
            builder.record_abandoned_attempt();
            return Err(CompileError::RequiresComplex);
        }};
    }

    while let Some(task) = tasks.pop() {
        let Visit::Node { node } = task else {
            let frame = frames.pop().expect("compile frame must match an element");
            if frame.requires_content && !frame.has_content {
                requires_complex!();
            }
            if let Some(semantic) = frame.semantic {
                builder.close(semantic)?;
            }
            complete_child(
                frames
                    .last_mut()
                    .expect("ordinary compiler keeps a root frame"),
                frame.tag,
                frame.first_visible,
                frame.last_visible,
                frame.has_content,
            );
            continue;
        };
        let scope = frames
            .last()
            .expect("ordinary compiler keeps a root frame")
            .scope;
        if let Some(text) = dom.text_node(node) {
            if frames
                .last()
                .is_some_and(|frame| frame.scope.inside_code && !frame.scope.inside_pre)
                && text.contains('\n')
            {
                requires_complex!();
            }
            let first = text.chars().find(|character| !character.is_whitespace());
            let last = text
                .chars()
                .rev()
                .find(|character| !character.is_whitespace());
            let frame_index = frames.len() - 1;
            if first.is_none() {
                let next_is_inline = dom
                    .next_sibling(node)
                    .is_some_and(|sibling| is_inline_source(dom, sibling));
                let frame = &mut frames[frame_index];
                if frame.previous_child_inline && next_is_inline {
                    builder.append_prose(frame.scope.parent, text)?;
                }
                complete_child(frame, None, first, last, false);
                continue;
            }
            let frame = &mut frames[frame_index];
            if !text.chars().next().is_some_and(char::is_whitespace)
                && first.is_some_and(char::is_alphanumeric)
                && frame.previous_child_inline_element
                && frame
                    .previous_child_last_visible
                    .is_some_and(char::is_alphanumeric)
            {
                builder.append_normalized_prose(frame.scope.parent, " ")?;
            }
            consume_boundary(&mut frames, &mut builder, frame_index, first)?;
            let parent = frames[frame_index].scope.parent;
            builder.append_prose(parent, text)?;
            complete_child(&mut frames[frame_index], None, first, last, true);
            continue;
        }
        if dom.is_comment(node) {
            let frame = frames
                .last_mut()
                .expect("ordinary compiler keeps a root frame");
            complete_child(frame, None, None, None, false);
            continue;
        }
        let Some(tag) = dom.tag(node) else {
            requires_complex!();
        };
        if has_complex_tag(tag) {
            requires_complex!();
        }
        if is_block_tag(tag) && scope.forbids_blocks {
            requires_complex!();
        }
        if tag == Tag::Pre {
            if !simple_pre(dom, node) {
                requires_complex!();
            }
            let Some(code) = super::code::recognize_known_block(dom, node, true) else {
                requires_complex!();
            };
            let first = code
                .text
                .chars()
                .find(|character| !character.is_whitespace());
            let last = code
                .text
                .chars()
                .rev()
                .find(|character| !character.is_whitespace());
            let frame_index = frames.len() - 1;
            consume_boundary(&mut frames, &mut builder, frame_index, first)?;
            let language = code.language.as_deref().map(Into::into);
            let text = code.into_text(None);
            builder.emit(
                scope.parent,
                SemanticKind::CodeBlock(CodeBlock { language, text }),
            )?;
            complete_child(frames.last_mut().unwrap(), Some(tag), first, last, true);
            continue;
        }
        if tag == Tag::Code {
            if dom.children(node).any(|child| dom.is_element(child)) {
                requires_complex!();
            }
            let text = dom.text(node);
            if !scope.inside_pre && text.contains('\n') {
                requires_complex!();
            }
            let first = text.chars().find(|character| !character.is_whitespace());
            let last = text
                .chars()
                .rev()
                .find(|character| !character.is_whitespace());
            let frame_index = frames.len() - 1;
            consume_boundary(&mut frames, &mut builder, frame_index, first)?;
            builder.append_inline_code(scope.parent, &text)?;
            complete_child(frames.last_mut().unwrap(), Some(tag), first, last, true);
            continue;
        }
        if tag == Tag::Img {
            if scope.inside_heading || !simple_image_source(dom, node) {
                requires_complex!();
            }
            let source = dom
                .attr(node, AttrName::Src)
                .and_then(|value| context.image_destination(value));
            let frame_index = frames.len() - 1;
            if let Some(source) = source {
                let alt = super::images::canonical_label(dom.attr_by_local_name(node, "alt"));
                let first = alt.chars().find(|character| !character.is_whitespace());
                consume_boundary(&mut frames, &mut builder, frame_index, first)?;
                builder.emit(
                    scope.parent,
                    SemanticKind::Image(semantic_image(dom, node, source)),
                )?;
            } else {
                let alt = super::images::canonical_label(dom.attr_by_local_name(node, "alt"));
                let first = alt.chars().find(|character| !character.is_whitespace());
                consume_boundary(&mut frames, &mut builder, frame_index, first)?;
                builder.append_prose(scope.parent, &alt)?;
            }
            complete_child(frames.last_mut().unwrap(), Some(tag), None, None, true);
            continue;
        }

        if tag == Tag::A && scope.link.is_some() {
            requires_complex!();
        }
        if tag == Tag::Figure && !simple_figure(dom, node)
            || tag == Tag::Figcaption
                && dom.parent(node).and_then(|parent| dom.tag(parent)) != Some(Tag::Figure)
            || tag == Tag::Summary
                && (dom.parent(node).and_then(|parent| dom.tag(parent)) != Some(Tag::Details)
                    || !summary_is_first_child(dom, node))
            || tag == Tag::Li
                && !matches!(
                    dom.parent(node).and_then(|parent| dom.tag(parent)),
                    Some(Tag::Ul | Tag::Ol)
                )
            || matches!(tag, Tag::Dt | Tag::Dd)
                && dom.parent(node).and_then(|parent| dom.tag(parent)) != Some(Tag::Dl)
            || matches!(tag, Tag::Ul | Tag::Ol) && !simple_native_list(dom, node)
            || tag == Tag::Dl && !simple_definition_list(dom, node)
        {
            requires_complex!();
        }
        let block = is_block_tag(tag);
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
        let mut next = scope;
        let parent_is_block_group = scope
            .parent
            .is_some_and(|parent| builder.is_block_group(parent));
        let kind = match tag {
            Tag::P | Tag::Address => Some(SemanticKind::Paragraph),
            Tag::H1 | Tag::H2 | Tag::H3 | Tag::H4 | Tag::H5 | Tag::H6 => {
                Some(SemanticKind::Heading {
                    level: heading_level(dom, node).unwrap_or(1),
                })
            }
            Tag::Blockquote => Some(SemanticKind::BlockQuote),
            Tag::Ul => Some(SemanticKind::List(List {
                kind: ListKind::Unordered,
                start: None,
            })),
            Tag::Ol => Some(SemanticKind::List(List {
                kind: ListKind::Ordered,
                start: dom
                    .attr(node, AttrName::Start)
                    .and_then(|value| value.parse().ok()),
            })),
            Tag::Li if scope.list.is_some() => Some(SemanticKind::ListItem),
            Tag::Figure => Some(SemanticKind::Figure),
            Tag::Figcaption if scope.figure.is_some() => Some(SemanticKind::Figcaption),
            Tag::Details => Some(SemanticKind::Details),
            Tag::Summary => Some(SemanticKind::Summary),
            Tag::Hr => Some(SemanticKind::ThematicBreak),
            Tag::Dl => Some(SemanticKind::DefinitionList),
            Tag::Dt if scope.definition_list.is_some() => Some(SemanticKind::DefinitionTerm),
            Tag::Dd if scope.definition_list.is_some() => Some(SemanticKind::DefinitionDescription),
            Tag::Strong | Tag::B => Some(SemanticKind::Strong),
            Tag::Em | Tag::I => Some(SemanticKind::Emphasis),
            Tag::Del => Some(SemanticKind::Strikethrough),
            Tag::Br => Some(SemanticKind::HardBreak),
            Tag::A if scope.link.is_none() => dom.attr(node, AttrName::Href).and_then(|value| {
                let trimmed = value.trim_matches(|character: char| {
                    character.is_ascii_whitespace() || character.is_control()
                });
                let fragment_only = trimmed.starts_with('#') && trimmed.len() > 1;
                context.link_destination(value).map(|destination| {
                    SemanticKind::Link(Link {
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
                Some(SemanticKind::BlockGroup)
            }
            _ => None,
        };
        let redundant = kind
            .as_ref()
            .is_some_and(|kind| builder.is_redundant_formatting(scope.parent, kind));
        let transparent = kind.is_none() || redundant;
        let transparent_inline = transparent && !is_block_tag(tag);
        let inherited_boundary = frames
            .last()
            .and_then(|frame| frame.boundary_source)
            .filter(|&source| frames[source].boundary_pending);
        let boundary_before = transparent_inline
            && inherited_boundary.is_none()
            && frames.last().is_some_and(|frame| {
                frame.previous_child_inline_element
                    && frame
                        .previous_child_last_visible
                        .is_some_and(char::is_alphanumeric)
            });
        let frame_index = frames.len();
        let boundary_source = if boundary_before {
            Some(frame_index)
        } else {
            inherited_boundary
        };
        let semantic_parent = if tag == Tag::Figcaption {
            scope.figure
        } else {
            scope.parent
        };
        let semantic = if transparent {
            None
        } else {
            if let Some(source) = inherited_boundary.filter(|_| !block) {
                let first_visible = first_visible_character(dom, node);
                frames[source].boundary_pending = false;
                if first_visible.is_some_and(char::is_alphanumeric) {
                    builder.append_normalized_prose(frames[source].scope.parent, " ")?;
                }
            }
            let semantic = builder.emit(
                semantic_parent,
                kind.expect("nontransparent node has a kind"),
            )?;
            builder.is_container(semantic).then_some(semantic)
        };
        next.forbids_blocks = scope.forbids_blocks
            || !block
            || matches!(
                tag,
                Tag::Address | Tag::P | Tag::H1 | Tag::H2 | Tag::H3 | Tag::H4 | Tag::H5 | Tag::H6
            );
        next.inside_code = scope.inside_code || tag == Tag::Code;
        next.inside_heading = scope.inside_heading || is_heading(tag);
        next.inside_pre = scope.inside_pre || tag == Tag::Pre;
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
            semantic,
            has_content: matches!(tag, Tag::Br | Tag::Hr),
            requires_content,
            first_visible: None,
            last_visible: None,
            previous_child_inline_element: false,
            previous_child_inline: false,
            previous_child_last_visible: None,
            boundary_source,
            boundary_pending: boundary_before,
        });
        if retained_nodes.is_none() {
            tasks.push(Visit::End);
            if !matches!(tag, Tag::Hr | Tag::Br) {
                tasks.extend(
                    dom.children_rev(node)
                        .map(|child| Visit::Node { node: child }),
                );
            }
        }
    }

    let document = builder.finish()?;
    #[cfg(any(test, debug_assertions))]
    if document.validate().is_err() {
        return Err(CompileError::RequiresComplex);
    }
    Ok(document)
}

fn retained_source_tasks(dom: &Dom, nodes: &[NodeId]) -> Vec<Visit> {
    let mut positions = vec![usize::MAX; dom.len()];
    for (position, &node) in nodes.iter().enumerate() {
        positions[node.index()] = position;
    }
    let mut subtree_ends: Vec<_> = (0..nodes.len()).map(|position| position + 1).collect();
    for position in (0..nodes.len()).rev() {
        let node = nodes[position];
        if let Some(parent) = dom.parent(node) {
            let parent_position = positions[parent.index()];
            if parent_position < position {
                subtree_ends[parent_position] =
                    subtree_ends[parent_position].max(subtree_ends[position]);
            }
        }
    }

    let mut events = Vec::with_capacity(nodes.len());
    let mut open = Vec::new();
    let mut skipped_until = 0;
    for (position, &node) in nodes.iter().enumerate() {
        if position < skipped_until {
            continue;
        }
        while let Some(&ancestor) = open.last() {
            if dom.parent(node) == Some(ancestor) {
                break;
            }
            events.push(Visit::End);
            open.pop();
        }
        events.push(Visit::Node { node });
        if ordinary_frame_node(dom, node) {
            open.push(node);
        }
        if ordinary_leaf_node(dom, node) {
            skipped_until = subtree_ends[position];
        }
    }
    while open.pop().is_some() {
        events.push(Visit::End);
    }
    events.reverse();
    events
}

fn ordinary_frame_node(dom: &Dom, node: NodeId) -> bool {
    dom.tag(node)
        .is_some_and(|tag| !matches!(tag, Tag::Pre | Tag::Code | Tag::Img))
}

fn ordinary_leaf_node(dom: &Dom, node: NodeId) -> bool {
    dom.tag(node)
        .is_some_and(|tag| matches!(tag, Tag::Pre | Tag::Code | Tag::Img | Tag::Br | Tag::Hr))
}

fn complete_child(
    parent: &mut Frame,
    tag: Option<Tag>,
    first_visible: Option<char>,
    last_visible: Option<char>,
    has_content: bool,
) {
    parent.has_content |= has_content;
    parent.first_visible = parent.first_visible.or(first_visible);
    if last_visible.is_some() {
        parent.last_visible = last_visible;
    }
    parent.previous_child_inline_element = tag.is_some_and(|tag| !is_block_tag(tag));
    parent.previous_child_inline = tag.map_or(last_visible.is_some(), |tag| !is_block_tag(tag));
    parent.previous_child_last_visible = last_visible;
}

/// Consumes a deferred boundary once the first meaningful child is known.
fn consume_boundary(
    frames: &mut [Frame],
    builder: &mut SemanticTapeBuilder,
    frame_index: usize,
    first_visible: Option<char>,
) -> Result<(), CompileError> {
    let Some(source) = frames[frame_index].boundary_source else {
        return Ok(());
    };
    if !frames[source].boundary_pending {
        return Ok(());
    }
    frames[source].boundary_pending = false;
    if first_visible.is_some_and(char::is_alphanumeric) {
        builder.append_normalized_prose(frames[source].scope.parent, " ")?;
    }
    Ok(())
}

fn is_inline_source(dom: &Dom, node: NodeId) -> bool {
    dom.text_node(node)
        .is_some_and(|text| text.chars().any(|character| !character.is_whitespace()))
        || dom.tag(node).is_some_and(|tag| !is_block_tag(tag))
}

fn first_visible_character(dom: &Dom, node: NodeId) -> Option<char> {
    let mut tasks = dom.children_rev(node).collect::<Vec<_>>();
    while let Some(node) = tasks.pop() {
        if let Some(text) = dom.text_node(node) {
            if let Some(character) = text.chars().find(|character| !character.is_whitespace()) {
                return Some(character);
            }
            continue;
        }
        if dom.is_comment(node) {
            continue;
        }
        tasks.extend(dom.children_rev(node));
    }
    None
}
