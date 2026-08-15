use std::collections::HashMap;

use thiserror::Error;
use url::Url;

use super::{
    BuildError, Callout, CalloutKind, CodeBlock, DestinationKind, Document, DocumentBuilder,
    DocumentNodeId, FootnoteId, Image, Link, List, ListKind, MathFormat, MathValue, Media,
    NodeKind, Table, TableAlignment, TableCell, TaskMarker, ValidationError, safe_destination,
};
use crate::dom::{AttrName, Dom, NodeId, Tag};

#[derive(Clone, Debug, Default)]
pub(crate) struct CompileContext {
    base_url: Option<Url>,
    resolve_fragment_links: bool,
}

impl CompileContext {
    pub(crate) fn new(base_url: Option<Url>, source_url: Option<&Url>) -> Self {
        let resolve_fragment_links = base_url
            .as_ref()
            .zip(source_url)
            .is_some_and(|(base, source)| base != source);
        Self {
            base_url,
            resolve_fragment_links,
        }
    }

    pub(super) fn link_destination(&self, value: &str) -> Option<Box<str>> {
        if self.resolve_fragment_links && value.trim().starts_with('#') {
            let resolved = self.base_url.as_ref()?.join(value.trim()).ok()?;
            return safe_destination(resolved.as_str(), None, DestinationKind::Link);
        }
        safe_destination(value, self.base_url.as_ref(), DestinationKind::Link)
    }

    pub(super) fn image_destination(&self, value: &str) -> Option<Box<str>> {
        safe_destination(value, self.base_url.as_ref(), DestinationKind::Resource)
    }
}

#[derive(Debug, Error)]
pub(crate) enum CompileError {
    #[error(transparent)]
    Build(#[from] BuildError),
    #[error(transparent)]
    Validation(#[from] ValidationError),
}

#[derive(Clone, Copy)]
struct Scope {
    parent: Option<DocumentNodeId>,
    list: Option<DocumentNodeId>,
    table: Option<DocumentNodeId>,
    row: Option<DocumentNodeId>,
    figure: Option<DocumentNodeId>,
    definition_list: Option<DocumentNodeId>,
    link: Option<DocumentNodeId>,
    preserve_isolated_whitespace: bool,
}

enum Task {
    Node {
        node: NodeId,
        scope: Scope,
    },
    Prose {
        parent: Option<DocumentNodeId>,
        text: Box<str>,
    },
    HardBreak {
        parent: Option<DocumentNodeId>,
    },
    WrappedChildren {
        node: NodeId,
        scope: Scope,
        kind: NodeKind,
    },
    DeferredFootnote {
        node: NodeId,
        label: Box<str>,
        scope: Scope,
    },
    CalloutTitle {
        node: NodeId,
        scope: Scope,
        already_strong: bool,
    },
}

#[derive(Default)]
struct TableAnalysis {
    current_width: u32,
    maximum_width: u32,
    has_rowspan: bool,
}

/// Compiles the children of a retained source root into semantic nodes.
pub(crate) fn compile_document(
    dom: &Dom,
    root: NodeId,
    context: &CompileContext,
) -> Result<Document, CompileError> {
    if let Some(inventory) = super::ordinary::inventory(dom, root) {
        return super::ordinary::compile(dom, root, context, &inventory);
    }
    compile_complex_document(dom, root, context)
}

fn compile_complex_document(
    dom: &Dom,
    root: NodeId,
    context: &CompileContext,
) -> Result<Document, CompileError> {
    let mut nodes = Vec::with_capacity(dom.len());
    nodes.push(root);
    nodes.extend(dom.descendants(root));
    let mut block_descendants = vec![false; dom.len()];
    let mut code_blocks = vec![false; dom.len()];
    let mut meaningful_content = vec![false; dom.len()];
    let mut visible_text_content = vec![false; dom.len()];
    let mut first_visible = vec![None; dom.len()];
    let mut last_visible = vec![None; dom.len()];
    let heading_permalinks = super::headings::permalink_nodes(dom, &nodes);
    let has_heading_permalinks = heading_permalinks.iter().any(|value| *value);
    let mut heading_levels = vec![None; dom.len()];
    let mut nearest_heading = if has_heading_permalinks {
        vec![None; dom.len()]
    } else {
        Vec::new()
    };
    let mut heading_has_permalink = if has_heading_permalinks {
        vec![false; dom.len()]
    } else {
        Vec::new()
    };
    let mut nearest_heading_permalink = if has_heading_permalinks {
        vec![None; dom.len()]
    } else {
        Vec::new()
    };
    let mut first_heading_text = if has_heading_permalinks {
        vec![None; dom.len()]
    } else {
        Vec::new()
    };
    let mut last_heading_text = if has_heading_permalinks {
        vec![None; dom.len()]
    } else {
        Vec::new()
    };
    for &node in &nodes {
        heading_levels[node.index()] = heading_level(dom, node);
        if !has_heading_permalinks {
            continue;
        }
        nearest_heading[node.index()] = if heading_levels[node.index()].is_some() {
            Some(node)
        } else {
            dom.parent(node)
                .and_then(|parent| nearest_heading[parent.index()])
        };
        nearest_heading_permalink[node.index()] = if heading_permalinks[node.index()] {
            Some(node)
        } else {
            dom.parent(node)
                .and_then(|parent| nearest_heading_permalink[parent.index()])
        };
        if let Some(heading) = nearest_heading[node.index()] {
            if nearest_heading_permalink[node.index()].is_some() {
                heading_has_permalink[heading.index()] = true;
            } else if dom.text_node(node).is_some() {
                first_heading_text[heading.index()].get_or_insert(node);
                last_heading_text[heading.index()] = Some(node);
            }
        }
    }
    let mut permalink_separates_words = if has_heading_permalinks {
        vec![false; dom.len()]
    } else {
        Vec::new()
    };
    if has_heading_permalinks {
        let mut previous_heading_character = vec![None; dom.len()];
        for &node in &nodes {
            let Some(heading) = nearest_heading[node.index()] else {
                continue;
            };
            if heading_permalinks[node.index()] {
                permalink_separates_words[node.index()] =
                    previous_heading_character[heading.index()].is_some_and(char::is_alphanumeric);
            } else if nearest_heading_permalink[node.index()].is_none()
                && let Some(text) = dom.text_node(node)
                && let Some(character) = text
                    .chars()
                    .rev()
                    .find(|character| !character.is_whitespace())
            {
                previous_heading_character[heading.index()] = Some(character);
            }
        }
        let mut next_heading_character = vec![None; dom.len()];
        for &node in nodes.iter().rev() {
            let Some(heading) = nearest_heading[node.index()] else {
                continue;
            };
            if heading_permalinks[node.index()] {
                permalink_separates_words[node.index()] &=
                    next_heading_character[heading.index()].is_some_and(char::is_alphanumeric);
            } else if nearest_heading_permalink[node.index()].is_none()
                && let Some(text) = dom.text_node(node)
                && let Some(character) = text.chars().find(|character| !character.is_whitespace())
            {
                next_heading_character[heading.index()] = Some(character);
            }
        }
    }
    let has_multiline_source = nodes.iter().any(|&node| {
        matches!(dom.tag(node), Some(Tag::Pre | Tag::Br))
            || dom.text_node(node).is_some_and(|text| text.contains('\n'))
    });
    let multiline_content =
        has_multiline_source.then(|| super::code::multiline_content(dom, &nodes));
    let images = super::images::analyze(dom, &nodes, context.base_url.as_ref());
    let mut meaningful_heading_content = vec![false; dom.len()];
    for &node in nodes.iter().rev() {
        if has_heading_permalinks && nearest_heading_permalink[node.index()].is_some() {
            continue;
        }
        meaningful_heading_content[node.index()] = dom
            .text_node(node)
            .is_some_and(|text| text.chars().any(|character| !character.is_whitespace()))
            || dom.tag(node) == Some(Tag::Img) && images.source(node).is_some()
            || dom
                .children(node)
                .any(|child| meaningful_heading_content[child.index()]);
    }
    let (figures, captions) = super::figures::analyze(dom, &nodes, &images);
    let media = super::media::analyze(dom, &nodes, context.base_url.as_ref());
    let lists = super::lists::ListAnalysis::analyze(dom, &nodes);
    let tables = super::tables::TableAnalysis::analyze(dom, &nodes);
    let callouts = super::callouts::CalloutAnalysis::analyze(dom, &nodes);
    let footnotes = super::footnotes::FootnoteAnalysis::analyze(dom, root);
    let math = super::math::MathAnalysis::analyze(dom, &nodes);
    let (media_separators, text_after_media_separators) = if media.is_empty() {
        (vec![false; dom.len()], vec![false; dom.len()])
    } else {
        media_separators(dom, root, &media)
    };
    if let Some(multiline_content) = multiline_content.as_deref() {
        for &node in &nodes {
            code_blocks[node.index()] = dom.tag(node) == Some(Tag::Pre)
                || super::code::is_multiline_orphan_with_evidence(
                    dom,
                    node,
                    multiline_content[node.index()],
                );
        }
    } else {
        for &node in &nodes {
            code_blocks[node.index()] = dom.tag(node) == Some(Tag::Pre);
        }
    }
    for &node in nodes.iter().rev() {
        if let Some(text) = dom.text_node(node) {
            first_visible[node.index()] = text.chars().find(|character| !character.is_whitespace());
            last_visible[node.index()] = text
                .chars()
                .rev()
                .find(|character| !character.is_whitespace());
        } else {
            for child in dom.children(node) {
                first_visible[node.index()] =
                    first_visible[node.index()].or(first_visible[child.index()]);
                if last_visible[child.index()].is_some() {
                    last_visible[node.index()] = last_visible[child.index()];
                }
            }
        }
        block_descendants[node.index()] = dom.children(node).any(|child| {
            code_blocks[child.index()]
                || dom.tag(child).is_some_and(is_block_tag)
                || block_descendants[child.index()]
        });
        visible_text_content[node.index()] = dom
            .text_node(node)
            .is_some_and(|text| text.chars().any(|character| !character.is_whitespace()))
            || dom
                .children(node)
                .any(|child| visible_text_content[child.index()]);
        meaningful_content[node.index()] = dom
            .text_node(node)
            .is_some_and(|text| text.chars().any(|character| !character.is_whitespace()))
            || dom.tag(node).is_some_and(|tag| {
                matches!(
                    tag,
                    Tag::Br
                        | Tag::Code
                        | Tag::Hr
                        | Tag::Img
                        | Tag::Iframe
                        | Tag::Video
                        | Tag::Audio
                ) || math.value(node).is_some()
                    || footnotes.reference(node).is_some()
            })
            || dom
                .children(node)
                .any(|child| meaningful_content[child.index()]);
    }

    let mut nearest_list_item = vec![None; dom.len()];
    for &node in &nodes {
        nearest_list_item[node.index()] = if dom.tag(node) == Some(Tag::Li) || lists.is_item(node) {
            Some(node)
        } else {
            dom.parent(node)
                .and_then(|parent| nearest_list_item[parent.index()])
        };
    }

    let mut builder = DocumentBuilder::with_capacity(dom.len());
    let mut footnote_ids = HashMap::<String, FootnoteId>::new();
    let mut table_layouts = HashMap::<DocumentNodeId, TableAnalysis>::new();
    let mut deferred_footnote_group = None;
    let scope = Scope {
        parent: None,
        list: None,
        table: None,
        row: None,
        figure: None,
        definition_list: None,
        link: None,
        preserve_isolated_whitespace: false,
    };
    let mut tasks = nodes
        .iter()
        .rev()
        .filter_map(|&node| {
            footnotes
                .definition(node)
                .filter(|_| footnotes.is_deferred(node))
                .map(|label| Task::DeferredFootnote {
                    node,
                    label: label.into(),
                    scope,
                })
        })
        .collect::<Vec<_>>();
    tasks.extend(
        dom.children_rev(root)
            .map(|node| Task::Node { node, scope }),
    );

    while let Some(task) = tasks.pop() {
        let Task::Node { node, scope } = task else {
            match task {
                Task::Prose { parent, text } => {
                    builder.append_prose(parent, &text)?;
                }
                Task::HardBreak { parent } => {
                    builder.append(parent, NodeKind::HardBreak)?;
                }
                Task::WrappedChildren { node, scope, kind } => {
                    let parent = builder.append(scope.parent, kind)?;
                    push_children(
                        dom,
                        node,
                        Scope {
                            parent: Some(parent),
                            ..scope
                        },
                        &mut tasks,
                    );
                }
                Task::DeferredFootnote {
                    node,
                    label,
                    mut scope,
                } => {
                    let parent = if let Some(parent) = deferred_footnote_group {
                        parent
                    } else {
                        let parent = builder.append(None, NodeKind::BlockGroup)?;
                        deferred_footnote_group = Some(parent);
                        parent
                    };
                    scope.parent = Some(parent);
                    compile_footnote_definition(
                        dom,
                        node,
                        &label,
                        scope,
                        &mut footnote_ids,
                        &mut builder,
                        &mut tasks,
                    )?;
                }
                Task::CalloutTitle {
                    node,
                    scope,
                    already_strong,
                } => {
                    let paragraph = builder.append(scope.parent, NodeKind::Paragraph)?;
                    let parent = if already_strong {
                        paragraph
                    } else {
                        builder.append(Some(paragraph), NodeKind::Strong)?
                    };
                    push_children(
                        dom,
                        node,
                        Scope {
                            parent: Some(parent),
                            ..scope
                        },
                        &mut tasks,
                    );
                }
                Task::Node { .. } => unreachable!(),
            }
            continue;
        };
        let heading_permalink = has_heading_permalinks
            && nearest_heading[node.index()]
                .is_some_and(|heading| heading != node && heading_permalinks[node.index()]);
        if tables.is_skipped(node)
            || footnotes.is_skipped(node)
            || math.is_skipped(node)
            || footnotes.is_deferred(node)
            || heading_permalink
        {
            if tables.emits_separator(node)
                || heading_permalink && permalink_separates_words[node.index()]
            {
                builder.append_prose(scope.parent, " ")?;
            }
            continue;
        }
        if footnotes.is_transparent(node) {
            push_children(dom, node, scope, &mut tasks);
            continue;
        }
        if let Some(text) = dom.text_node(node) {
            let text = tables
                .replacement_text(node)
                .or_else(|| lists.replacement_text(node))
                .unwrap_or(text);
            let heading = has_heading_permalinks
                .then(|| nearest_heading[node.index()])
                .flatten()
                .filter(|heading| heading_has_permalink[heading.index()]);
            let trim_start = footnotes.should_trim_start(node)
                || heading.is_some_and(|heading| first_heading_text[heading.index()] == Some(node));
            let text = if trim_start { text.trim_start() } else { text };
            let text = if heading
                .is_some_and(|heading| last_heading_text[heading.index()] == Some(node))
            {
                text.trim_end()
            } else {
                text
            };
            let whitespace_only = text.chars().all(char::is_whitespace);
            if !whitespace_only
                && !text.chars().next().is_some_and(char::is_whitespace)
                && (inline_word_boundary_before(dom, node, &first_visible, &last_visible)
                    || text_after_media_separators[node.index()])
            {
                builder.append_prose(scope.parent, " ")?;
            }
            let structural_parent = scope.parent.is_some_and(|parent| {
                Some(parent) == scope.list
                    || Some(parent) == scope.table
                    || Some(parent) == scope.row
                    || Some(parent) == scope.definition_list
            });
            if !(whitespace_only
                && (structural_parent
                    || (!scope.preserve_isolated_whitespace
                        && !meaningful_inline_separator(dom, node, &block_descendants))))
            {
                let parent = if scope.parent.is_some() && scope.parent == scope.list {
                    Some(builder.append(scope.parent, NodeKind::ListItem)?)
                } else {
                    scope.parent
                };
                builder.append_prose(parent, text)?;
            }
            continue;
        }
        if dom.is_comment(node) {
            continue;
        }
        let Some(tag) = dom.tag(node) else {
            continue;
        };
        if matches!(
            tag,
            Tag::Head | Tag::Script | Tag::Style | Tag::Template | Tag::Noscript
        ) && math.value(node).is_none()
        {
            continue;
        }

        if let Some(math) = math.value(node) {
            let value = MathValue {
                source: math.source.clone(),
                format: MathFormat::Tex,
                fallback_text: Some(math.fallback.clone()),
            };
            builder.append(
                scope.parent,
                if math.block {
                    NodeKind::DisplayMath(value)
                } else {
                    NodeKind::InlineMath(value)
                },
            )?;
            continue;
        }

        if let Some(label) = footnotes.reference(node) {
            if footnotes.has_definition(label) {
                let id = footnote_id(&mut footnote_ids, label)?;
                builder.append(scope.parent, NodeKind::FootnoteReference(id))?;
            } else {
                push_children(dom, node, scope, &mut tasks);
            }
            continue;
        }

        if let Some(label) = footnotes.definition(node) {
            compile_footnote_definition(
                dom,
                node,
                label,
                scope,
                &mut footnote_ids,
                &mut builder,
                &mut tasks,
            )?;
            continue;
        }

        if tag == Tag::Table
            && let Some(code) = super::code::recognize_gutter_table(dom, node)
        {
            builder.append(
                scope.parent,
                NodeKind::CodeBlock(CodeBlock {
                    language: code.language.map(Into::into),
                    text: code.text.into(),
                }),
            )?;
            continue;
        }
        if tag == Tag::Table {
            let table_scope = if scope.parent.is_some() && scope.parent == scope.list {
                Scope {
                    parent: Some(builder.append(scope.parent, NodeKind::ListItem)?),
                    ..scope
                }
            } else {
                scope
            };
            match tables.kind(node) {
                super::tables::TableKind::Layout => {
                    compile_layout_table(dom, node, table_scope, &tables, &mut tasks);
                    continue;
                }
                super::tables::TableKind::Listing { start } => {
                    compile_listing_table(
                        dom,
                        node,
                        start,
                        table_scope,
                        &tables,
                        &mut builder,
                        &mut tasks,
                    )?;
                    continue;
                }
                super::tables::TableKind::Data => {}
            }
        }
        if code_blocks[node.index()]
            && let Some(code) = super::code::recognize_known_block(dom, node, true)
        {
            builder.append(
                scope.parent,
                NodeKind::CodeBlock(CodeBlock {
                    language: code.language.map(Into::into),
                    text: code.text.into(),
                }),
            )?;
            continue;
        }
        if tag == Tag::Code {
            builder.append(
                scope.parent,
                NodeKind::InlineCode(super::TextValue::new(dom.text(node))),
            )?;
            continue;
        }
        if tag == Tag::Input
            && dom
                .attr(node, AttrName::Type)
                .is_some_and(|value| value.eq_ignore_ascii_case("checkbox"))
        {
            builder.append(
                scope.parent,
                NodeKind::TaskMarker(TaskMarker {
                    checked: dom.attr(node, AttrName::Checked).is_some(),
                    fallback_label: (!nearest_list_item[node.index()]
                        .is_some_and(|item| visible_text_content[item.index()]))
                    .then(|| {
                        dom.attr(node, AttrName::AriaLabel)
                            .or_else(|| dom.attr(node, AttrName::Title))
                    })
                    .flatten()
                    .filter(|label| !label.trim().is_empty())
                    .map(Into::into),
                }),
            )?;
            continue;
        }
        if tag == Tag::Img {
            let alt = super::images::canonical_label(dom.attr_by_local_name(node, "alt"));
            let source = images.source(node).map(Into::into);
            if let Some(source) = source {
                builder.append(
                    scope.parent,
                    NodeKind::Image(semantic_image(dom, node, source)),
                )?;
            } else {
                builder.append_prose(scope.parent, &alt)?;
            }
            continue;
        }
        if tag == Tag::Picture && images.is_synthetic(node) {
            if let Some(source) = images.source(node).map(Into::into) {
                builder.append(
                    scope.parent,
                    NodeKind::Image(semantic_image(dom, node, source)),
                )?;
            }
            continue;
        }
        if matches!(tag, Tag::Iframe | Tag::Video | Tag::Audio) {
            if let Some(media) = media.item(node) {
                if media_separators[node.index()] {
                    builder.append_prose(scope.parent, " ")?;
                }
                builder.append(
                    scope.parent,
                    NodeKind::Media(Media {
                        kind: media.kind,
                        source: media.source.clone(),
                        title: Some(media.title.clone()),
                    }),
                )?;
                if let Some(fallback) = media.fallback {
                    builder.append_prose(scope.parent, " ")?;
                    tasks.push(Task::Node {
                        node: fallback,
                        scope,
                    });
                }
            }
            continue;
        }

        let mut next_scope = scope;
        let callout = callouts.value(node);
        let parent_is_block_group = scope
            .parent
            .is_some_and(|parent| matches!(builder.kind(parent), Some(NodeKind::BlockGroup)));
        let semantic = if let Some(level) = heading_levels[node.index()] {
            if !meaningful_heading_content[node.index()] {
                None
            } else if block_descendants[node.index()] {
                Some(NodeKind::BlockGroup)
            } else {
                Some(NodeKind::Heading { level })
            }
        } else if let Some(callout) = &callout {
            callout_kind(callout.kind).map(|kind| {
                NodeKind::Callout(Callout {
                    kind,
                    title: Some(callout.title.clone()),
                })
            })
        } else if figures[node.index()] {
            Some(NodeKind::Figure)
        } else if captions[node.index()] && scope.figure.is_some() {
            Some(NodeKind::Figcaption)
        } else if let Some(list) = lists.container(node) {
            Some(NodeKind::List(list))
        } else if lists.is_item(node) && scope.list.is_some() {
            Some(NodeKind::ListItem)
        } else {
            match tag {
                Tag::Caption if scope.table.is_some() => Some(NodeKind::TableCaption),
                Tag::P if block_descendants[node.index()] => Some(NodeKind::BlockGroup),
                Tag::P | Tag::Address | Tag::Caption => Some(NodeKind::Paragraph),
                Tag::Blockquote => dom
                    .attr(node, AttrName::DataCallout)
                    .and_then(callout_kind)
                    .map(|kind| NodeKind::Callout(Callout { kind, title: None }))
                    .or(Some(NodeKind::BlockQuote)),
                Tag::Li if scope.list.is_some() => Some(NodeKind::ListItem),
                Tag::Table => Some(NodeKind::Table(Table {
                    column_count: Some(0),
                })),
                Tag::Tr if scope.table.is_some() => Some(NodeKind::TableRow),
                Tag::Td | Tag::Th if scope.row.is_some() => Some(NodeKind::TableCell(TableCell {
                    header: tag == Tag::Th,
                    colspan: positive_u32(dom.attr(node, AttrName::ColSpan)).unwrap_or(1),
                    rowspan: positive_u32(dom.attr(node, AttrName::RowSpan)).unwrap_or(1),
                    alignment: dom.attr(node, AttrName::Align).and_then(table_alignment),
                })),
                Tag::Details => Some(NodeKind::Details),
                Tag::Summary => Some(NodeKind::Summary),
                Tag::Hr => Some(NodeKind::ThematicBreak),
                Tag::Dl => Some(NodeKind::DefinitionList),
                Tag::Dt if scope.definition_list.is_some() => Some(NodeKind::DefinitionTerm),
                Tag::Dd if scope.definition_list.is_some() => Some(NodeKind::DefinitionDescription),
                Tag::Dt | Tag::Dd => Some(NodeKind::Paragraph),
                Tag::Strong | Tag::B | Tag::Em | Tag::I | Tag::Del
                    if block_descendants[node.index()] =>
                {
                    Some(NodeKind::BlockGroup)
                }
                Tag::Strong | Tag::B | Tag::Em | Tag::I | Tag::Del
                    if !meaningful_content[node.index()] =>
                {
                    None
                }
                Tag::Strong | Tag::B => Some(NodeKind::Strong),
                Tag::Em | Tag::I => Some(NodeKind::Emphasis),
                Tag::Del => Some(NodeKind::Strikethrough),
                Tag::Br => Some(NodeKind::HardBreak),
                Tag::A
                    if scope.link.is_none()
                        && !block_descendants[node.index()]
                        && meaningful_content[node.index()] =>
                {
                    dom.attr(node, AttrName::Href).and_then(|destination| {
                        let trimmed = destination.trim_matches(|character: char| {
                            character.is_ascii_whitespace() || character.is_control()
                        });
                        let fragment_only = trimmed.starts_with('#') && trimmed.len() > 1;
                        context.link_destination(destination).map(|destination| {
                            NodeKind::Link(Link {
                                destination,
                                title: dom.attr(node, AttrName::Title).map(Into::into),
                                fragment_only,
                            })
                        })
                    })
                }
                _ if is_block_tag(tag)
                    && !(matches!(tag, Tag::Div | Tag::Section)
                        && parent_is_block_group
                        && has_single_content_child(dom, node)) =>
                {
                    Some(NodeKind::BlockGroup)
                }
                _ => None,
            }
        };

        let Some(kind) = semantic else {
            if !is_block_tag(tag)
                && tag != Tag::Sup
                && inline_word_boundary_before(dom, node, &first_visible, &last_visible)
            {
                builder.append_prose(scope.parent, " ")?;
            }
            let mut transparent_scope = scope;
            if !is_block_tag(tag) && !meaningful_content[node.index()] {
                transparent_scope.preserve_isolated_whitespace = true;
            }
            push_children(dom, node, transparent_scope, &mut tasks);
            continue;
        };
        if is_redundant_formatting(&kind, scope.parent.and_then(|parent| builder.kind(parent))) {
            push_children(dom, node, scope, &mut tasks);
            continue;
        }
        let semantic_leaf = matches!(kind, NodeKind::Media(_));
        let cell_span = match &kind {
            NodeKind::TableCell(cell) => Some((cell.colspan, cell.rowspan)),
            _ => None,
        };
        let semantic_parent = if matches!(kind, NodeKind::Figcaption) {
            scope.figure
        } else if scope.parent.is_some()
            && scope.parent == scope.list
            && !matches!(kind, NodeKind::ListItem | NodeKind::FootnoteDefinition(_))
        {
            Some(builder.append(scope.parent, NodeKind::ListItem)?)
        } else {
            scope.parent
        };
        let semantic_node = builder.append(semantic_parent, kind)?;
        next_scope.parent = Some(semantic_node);
        if figures[node.index()]
            && images.is_synthetic(node)
            && let Some(source) = images.source(node).map(Into::into)
        {
            builder.append(
                Some(semantic_node),
                NodeKind::Image(semantic_image(dom, node, source)),
            )?;
        }
        match builder.kind_mut(semantic_node) {
            Some(NodeKind::List(_)) => next_scope.list = Some(semantic_node),
            Some(NodeKind::ListItem) => {}
            _ => {
                if lists.container(node).is_none() {
                    next_scope.list = scope.list;
                }
            }
        }
        match tag {
            Tag::Table => {
                next_scope.table = Some(semantic_node);
                next_scope.row = None;
                table_layouts.insert(semantic_node, TableAnalysis::default());
            }
            Tag::Tr => {
                next_scope.row = Some(semantic_node);
                if let Some(table) = scope.table {
                    table_layouts
                        .get_mut(&table)
                        .expect("semantic table scope has an analysis")
                        .current_width = 0;
                }
            }
            Tag::Td | Tag::Th => {
                if let Some(table) = scope.table {
                    let (colspan, rowspan) = cell_span.unwrap_or((1, 1));
                    let analysis = table_layouts
                        .get_mut(&table)
                        .expect("semantic table scope has an analysis");
                    analysis.current_width = analysis
                        .current_width
                        .checked_add(colspan)
                        .ok_or(BuildError::CapacityExceeded)?;
                    analysis.maximum_width = analysis.maximum_width.max(analysis.current_width);
                    analysis.has_rowspan |= rowspan > 1;
                }
            }
            _ if figures[node.index()] => next_scope.figure = Some(semantic_node),
            Tag::Dl => next_scope.definition_list = Some(semantic_node),
            Tag::A => next_scope.link = Some(semantic_node),
            _ => {}
        }
        if !semantic_leaf {
            if let Some(callout) = callout {
                if let Some(title) = callout.title_node {
                    push_callout_children(
                        dom,
                        node,
                        title,
                        callout.title_is_strong,
                        next_scope,
                        &mut tasks,
                    );
                } else {
                    let paragraph = builder.append(Some(semantic_node), NodeKind::Paragraph)?;
                    let strong = builder.append(Some(paragraph), NodeKind::Strong)?;
                    builder.append_prose(Some(strong), &callout.title)?;
                    push_children(dom, node, next_scope, &mut tasks);
                }
            } else if figures[node.index()] {
                push_figure_children(dom, node, next_scope, &captions, &mut tasks);
            } else {
                push_children(dom, node, next_scope, &mut tasks);
            }
        }
    }

    for (table, analysis) in table_layouts {
        if let Some(NodeKind::Table(value)) = builder.kind_mut(table) {
            value.column_count = (!analysis.has_rowspan).then_some(analysis.maximum_width);
        }
    }
    let document = builder.finish();
    #[cfg(any(test, debug_assertions))]
    document.validate()?;
    Ok(document)
}

fn media_separators(
    dom: &Dom,
    root: NodeId,
    media: &super::media::MediaAnalysis,
) -> (Vec<bool>, Vec<bool>) {
    enum Visit {
        Node(NodeId),
        EndBlock,
    }
    #[derive(Clone, Copy)]
    enum PreviousInline {
        None,
        Word,
        Media,
    }

    let mut before_media = vec![false; dom.len()];
    let mut after_media = vec![false; dom.len()];
    let mut previous = PreviousInline::None;
    let mut tasks = Vec::new();
    tasks.extend(dom.children_rev(root).map(Visit::Node));
    while let Some(task) = tasks.pop() {
        let Visit::Node(node) = task else {
            previous = PreviousInline::None;
            continue;
        };
        if let Some(text) = dom.text_node(node) {
            let starts_word = text.chars().next().is_some_and(char::is_alphanumeric);
            after_media[node.index()] = matches!(previous, PreviousInline::Media) && starts_word;
            previous = if text.chars().last().is_some_and(char::is_alphanumeric) {
                PreviousInline::Word
            } else {
                PreviousInline::None
            };
            continue;
        }
        let Some(tag) = dom.tag(node) else {
            continue;
        };
        if media.item(node).is_some() {
            before_media[node.index()] =
                matches!(previous, PreviousInline::Word | PreviousInline::Media);
            previous = PreviousInline::Media;
            continue;
        }
        if is_block_tag(tag) {
            previous = PreviousInline::None;
            tasks.push(Visit::EndBlock);
        } else if matches!(tag, Tag::Br | Tag::Hr | Tag::Img) {
            previous = PreviousInline::None;
        }
        tasks.extend(dom.children_rev(node).map(Visit::Node));
    }
    (before_media, after_media)
}

fn compile_layout_table(
    dom: &Dom,
    table: NodeId,
    scope: Scope,
    analysis: &super::tables::TableAnalysis,
    tasks: &mut Vec<Task>,
) {
    let mut ordered = Vec::new();
    for &caption in analysis.captions(table) {
        if super::tables::children_are_phrasing(dom, caption) {
            ordered.push(Task::WrappedChildren {
                node: caption,
                scope,
                kind: NodeKind::Paragraph,
            });
        } else {
            append_child_tasks(dom, caption, scope, &mut ordered);
        }
    }
    for &row in analysis.rows(table) {
        for &cell in analysis.cells(row) {
            if !analysis.meaningful_cell(cell) {
                continue;
            }
            if analysis.cell_is_phrasing(cell) {
                ordered.push(Task::WrappedChildren {
                    node: cell,
                    scope,
                    kind: NodeKind::Paragraph,
                });
            } else {
                append_child_tasks(dom, cell, scope, &mut ordered);
            }
        }
    }
    tasks.extend(ordered.into_iter().rev());
}

fn compile_listing_table(
    dom: &Dom,
    table: NodeId,
    start: u32,
    scope: Scope,
    analysis: &super::tables::TableAnalysis,
    builder: &mut DocumentBuilder,
    tasks: &mut Vec<Task>,
) -> Result<(), BuildError> {
    let list = builder.append(
        scope.parent,
        NodeKind::List(List {
            kind: ListKind::Ordered,
            start: (start != 1).then_some(i64::from(start)),
        }),
    )?;
    let mut ordered = Vec::new();
    let mut current_item = None;
    let mut expects_metadata = false;
    for &row in analysis.rows(table) {
        let cells = analysis.cells(row);
        if analysis.row_has_rank(row) {
            let item = builder.append(Some(list), NodeKind::ListItem)?;
            append_cell_tasks(
                dom,
                &cells[1..],
                analysis,
                Scope {
                    parent: Some(item),
                    ..scope
                },
                &mut ordered,
            );
            current_item = Some(item);
            expects_metadata = true;
        } else if !analysis.row_has_content(row) {
            continue;
        } else if expects_metadata {
            if let Some(item) = current_item {
                ordered.push(Task::HardBreak { parent: Some(item) });
                append_cell_tasks(
                    dom,
                    cells,
                    analysis,
                    Scope {
                        parent: Some(item),
                        ..scope
                    },
                    &mut ordered,
                );
            }
            expects_metadata = false;
        } else {
            let kind = if cells
                .iter()
                .filter(|&&cell| analysis.meaningful_cell(cell))
                .all(|&cell| analysis.cell_is_phrasing(cell))
            {
                NodeKind::Paragraph
            } else {
                NodeKind::BlockGroup
            };
            let group = builder.append(scope.parent, kind)?;
            append_cell_tasks(
                dom,
                cells,
                analysis,
                Scope {
                    parent: Some(group),
                    ..scope
                },
                &mut ordered,
            );
        }
    }
    tasks.extend(ordered.into_iter().rev());
    Ok(())
}

fn append_cell_tasks(
    dom: &Dom,
    cells: &[NodeId],
    analysis: &super::tables::TableAnalysis,
    scope: Scope,
    ordered: &mut Vec<Task>,
) {
    let mut inserted = false;
    for &cell in cells {
        if !analysis.meaningful_cell(cell) {
            continue;
        }
        if inserted {
            ordered.push(Task::Prose {
                parent: scope.parent,
                text: " ".into(),
            });
        }
        append_child_tasks(dom, cell, scope, ordered);
        inserted = true;
    }
}

fn append_child_tasks(dom: &Dom, node: NodeId, scope: Scope, ordered: &mut Vec<Task>) {
    ordered.extend(dom.children(node).map(|node| Task::Node { node, scope }));
}

fn push_children(dom: &Dom, node: NodeId, scope: Scope, tasks: &mut Vec<Task>) {
    tasks.extend(
        dom.children_rev(node)
            .map(|child| Task::Node { node: child, scope }),
    );
}

fn push_callout_children(
    dom: &Dom,
    node: NodeId,
    title: NodeId,
    title_is_strong: bool,
    scope: Scope,
    tasks: &mut Vec<Task>,
) {
    tasks.extend(dom.children_rev(node).map(|child| {
        if child == title {
            Task::CalloutTitle {
                node: child,
                scope,
                already_strong: title_is_strong,
            }
        } else {
            Task::Node { node: child, scope }
        }
    }));
}

fn compile_footnote_definition(
    dom: &Dom,
    node: NodeId,
    label: &str,
    scope: Scope,
    footnote_ids: &mut HashMap<String, FootnoteId>,
    builder: &mut DocumentBuilder,
    tasks: &mut Vec<Task>,
) -> Result<(), BuildError> {
    let id = footnote_id(footnote_ids, label)?;
    let definition = builder.append(scope.parent, NodeKind::FootnoteDefinition(id))?;
    builder.define_footnote(id, label, definition)?;
    push_children(
        dom,
        node,
        Scope {
            parent: Some(definition),
            ..scope
        },
        tasks,
    );
    Ok(())
}

fn push_figure_children(
    dom: &Dom,
    node: NodeId,
    scope: Scope,
    caption_nodes: &[bool],
    tasks: &mut Vec<Task>,
) {
    let mut content = Vec::new();
    let mut captions = Vec::new();
    for child in dom.children(node) {
        if caption_nodes[child.index()] {
            captions.push(child);
        } else {
            content.push(child);
        }
    }
    content.extend(captions);
    tasks.extend(
        content
            .into_iter()
            .rev()
            .map(|child| Task::Node { node: child, scope }),
    );
}

fn inline_word_boundary_before(
    dom: &Dom,
    node: NodeId,
    first_visible: &[Option<char>],
    last_visible: &[Option<char>],
) -> bool {
    first_visible[node.index()].is_some_and(char::is_alphanumeric)
        && dom.prev_sibling(node).is_some_and(|previous| {
            last_visible[previous.index()].is_some_and(char::is_alphanumeric)
                && dom.tag(previous).is_some_and(|tag| !is_block_tag(tag))
        })
}

fn meaningful_inline_separator(dom: &Dom, node: NodeId, block_descendants: &[bool]) -> bool {
    dom.prev_sibling(node)
        .is_some_and(|sibling| is_inline_dom_node(dom, sibling, block_descendants))
        && dom
            .next_sibling(node)
            .is_some_and(|sibling| is_inline_dom_node(dom, sibling, block_descendants))
}

fn is_inline_dom_node(dom: &Dom, node: NodeId, block_descendants: &[bool]) -> bool {
    dom.text_node(node)
        .is_some_and(|text| text.chars().any(|character| !character.is_whitespace()))
        || dom.tag(node).is_some_and(|tag| {
            !block_descendants[node.index()]
                && !is_block_tag(tag)
                && !matches!(
                    tag,
                    Tag::Head | Tag::Script | Tag::Style | Tag::Template | Tag::Noscript
                )
        })
}

pub(super) fn has_single_content_child(dom: &Dom, node: NodeId) -> bool {
    let mut count = 0_u8;
    for child in dom.children(node) {
        let meaningful = dom.is_element(child)
            || dom
                .text_node(child)
                .is_some_and(|text| !text.trim().is_empty());
        if meaningful {
            count += 1;
            if count > 1 {
                return false;
            }
        }
    }
    count == 1
}

pub(super) fn is_redundant_formatting(kind: &NodeKind, parent: Option<&NodeKind>) -> bool {
    matches!(
        (kind, parent),
        (NodeKind::Strong, Some(NodeKind::Strong))
            | (NodeKind::Emphasis, Some(NodeKind::Emphasis))
            | (NodeKind::Strikethrough, Some(NodeKind::Strikethrough))
    )
}

pub(super) fn is_block_tag(tag: Tag) -> bool {
    matches!(
        tag,
        Tag::Address
            | Tag::Article
            | Tag::Aside
            | Tag::Blockquote
            | Tag::Caption
            | Tag::Dd
            | Tag::Details
            | Tag::Div
            | Tag::Dl
            | Tag::Dt
            | Tag::Fieldset
            | Tag::Figcaption
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
            | Tag::Hr
            | Tag::Li
            | Tag::Main
            | Tag::Nav
            | Tag::Ol
            | Tag::P
            | Tag::Pre
            | Tag::Section
            | Tag::Summary
            | Tag::Table
            | Tag::Tr
            | Tag::Ul
    )
}

fn footnote_id(
    footnotes: &mut HashMap<String, FootnoteId>,
    label: &str,
) -> Result<FootnoteId, BuildError> {
    if let Some(&id) = footnotes.get(label) {
        return Ok(id);
    }
    let id = FootnoteId::from_index(footnotes.len())?;
    footnotes.insert(label.to_owned(), id);
    Ok(id)
}

pub(super) fn semantic_image(dom: &Dom, node: NodeId, source: Box<str>) -> Image {
    Image {
        source,
        alt: super::images::canonical_label(dom.attr_by_local_name(node, "alt")),
        title: dom
            .attr(node, AttrName::Title)
            .map(|title| super::images::canonical_label(Some(title))),
        width: positive_u32(dom.attr(node, AttrName::Width)),
        height: positive_u32(dom.attr(node, AttrName::Height)),
    }
}

fn positive_u32(value: Option<&str>) -> Option<u32> {
    value?.trim().parse().ok().filter(|value| *value > 0)
}

fn table_alignment(value: &str) -> Option<TableAlignment> {
    if value.eq_ignore_ascii_case("left") {
        Some(TableAlignment::Left)
    } else if value.eq_ignore_ascii_case("center") {
        Some(TableAlignment::Center)
    } else if value.eq_ignore_ascii_case("right") {
        Some(TableAlignment::Right)
    } else {
        None
    }
}

pub(super) fn heading_level(dom: &Dom, node: NodeId) -> Option<u8> {
    let native = match dom.tag(node) {
        Some(Tag::H1) => Some(1),
        Some(Tag::H2) => Some(2),
        Some(Tag::H3) => Some(3),
        Some(Tag::H4) => Some(4),
        Some(Tag::H5) => Some(5),
        Some(Tag::H6) => Some(6),
        _ => None,
    };
    native.or_else(|| {
        dom.attr(node, AttrName::Role)
            .filter(|roles| {
                roles
                    .split_ascii_whitespace()
                    .any(|role| role.eq_ignore_ascii_case("heading"))
            })
            .and_then(|_| dom.attr_by_local_name(node, "aria-level"))
            .and_then(|level| level.trim().parse::<u8>().ok())
            .filter(|level| (1..=6).contains(level))
    })
}

fn callout_kind(value: &str) -> Option<CalloutKind> {
    match value.to_ascii_lowercase().as_str() {
        "note" => Some(CalloutKind::Note),
        "warning" => Some(CalloutKind::Warning),
        "tip" => Some(CalloutKind::Tip),
        "important" => Some(CalloutKind::Important),
        "caution" => Some(CalloutKind::Caution),
        "info" => Some(CalloutKind::Info),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compile(html: &str, base: Option<&str>) -> Document {
        let dom = Dom::parse_fragment(html, Tag::Div).unwrap();
        let base = base.map(|value| Url::parse(value).unwrap());
        let context = CompileContext::new(base.clone(), base.as_ref());
        compile_document(&dom, dom.root(), &context).unwrap()
    }

    fn uses_ordinary_compiler(html: &str) -> bool {
        let dom = Dom::parse_fragment(html, Tag::Div).unwrap();
        super::super::ordinary::supports(&dom, dom.root())
    }

    fn compare_ordinary_and_complex(html: &str, base: Option<&str>) {
        let dom = Dom::parse_fragment(html, Tag::Div).unwrap();
        let base = base.map(|value| Url::parse(value).unwrap());
        let context = CompileContext::new(base.clone(), base.as_ref());
        let inventory = super::super::ordinary::inventory(&dom, dom.root())
            .expect("source must support ordinary compilation");
        let ordinary =
            super::super::ordinary::compile(&dom, dom.root(), &context, &inventory).unwrap();
        let complex = compile_complex_document(&dom, dom.root(), &context).unwrap();
        assert_eq!(ordinary.debug_tree(), complex.debug_tree());
    }

    #[test]
    fn ordinary_compiler_handles_common_inline_and_block_semantics() {
        let html = r#"<h2>Read <em>this <strong>guide</strong></em></h2><p>Use <code>x = 1</code> with <del>old</del> and <a href="/relative">relative</a> or <a href="https://elsewhere.test/page">absolute</a> links.</p><blockquote><p>Quoted text.</p></blockquote><ul><li>One</li><li>Two</li></ul><ol start="4"><li>Four</li></ol><pre>plain code
</pre>"#;
        assert!(uses_ordinary_compiler(html));
        let document = compile(html, Some("https://example.test/base/"));
        assert_eq!(
            document.debug_tree(),
            concat!(
                "Heading(level=2)\n",
                "  Text(\"Read \")\n",
                "  Emphasis\n",
                "    Text(\"this \")\n",
                "    Strong\n",
                "      Text(\"guide\")\n",
                "Paragraph\n",
                "  Text(\"Use \")\n",
                "  InlineCode(\"x = 1\")\n",
                "  Text(\" with \")\n",
                "  Strikethrough\n",
                "    Text(\"old\")\n",
                "  Text(\" and \")\n",
                "  Link(destination=\"https://example.test/relative\", title=None)\n",
                "    Text(\"relative\")\n",
                "  Text(\" or \")\n",
                "  Link(destination=\"https://elsewhere.test/page\", title=None)\n",
                "    Text(\"absolute\")\n",
                "  Text(\" links.\")\n",
                "BlockQuote\n",
                "  Paragraph\n",
                "    Text(\"Quoted text.\")\n",
                "List(kind=Unordered, start=None)\n",
                "  ListItem\n",
                "    Text(\"One\")\n",
                "  ListItem\n",
                "    Text(\"Two\")\n",
                "List(kind=Ordered, start=Some(4))\n",
                "  ListItem\n",
                "    Text(\"Four\")\n",
                "CodeBlock(language=None, text=\"plain code\\n\")\n",
            )
        );
    }

    #[test]
    fn ordinary_compiler_handles_figures_details_and_definition_lists() {
        let html = r#"<figure><img src="/chart.png" alt="Chart" width="640" height="320"><figcaption>Quarterly result</figcaption></figure><details><summary>More</summary><p>Details.</p></details><dl><dt>Term</dt><dd>Definition.</dd></dl>"#;
        assert!(uses_ordinary_compiler(html));
        let document = compile(html, Some("https://example.test/docs/"));
        assert_eq!(
            document.debug_tree(),
            concat!(
                "Figure\n",
                "  Image(source=\"https://example.test/chart.png\", alt=\"Chart\", title=None, width=Some(640), height=Some(320))\n",
                "  Figcaption\n",
                "    Text(\"Quarterly result\")\n",
                "Details\n",
                "  Summary\n",
                "    Text(\"More\")\n",
                "  Paragraph\n",
                "    Text(\"Details.\")\n",
                "DefinitionList\n",
                "  DefinitionTerm\n",
                "    Text(\"Term\")\n",
                "  DefinitionDescription\n",
                "    Text(\"Definition.\")\n",
            )
        );
    }

    #[test]
    fn ordinary_compiler_preserves_boundaries_and_uri_policy() {
        let html = r#"<p>one<span>two</span>three <a href="javascript:alert(1)">unsafe</a> <img src="ftp://example.test/image.png" alt="fallback"></p>"#;
        assert!(uses_ordinary_compiler(html));
        let document = compile(html, None);
        assert_eq!(
            document.debug_tree(),
            concat!("Paragraph\n", "  Text(\"onetwo three unsafe fallback\")\n",)
        );
    }

    #[test]
    fn ordinary_compiler_matches_complex_for_article_collections() {
        compare_ordinary_and_complex(
            r#"<article><h1>Archive design</h1><p>The guide explains how the archive stores each record.</p><section class="related-content-tout"><h2>Collection</h2><p>This collection is part of the guide.</p><a href="/archive">Open the archive</a></section><h2>Validation</h2><p>The validation step compares every stored record.</p></article>"#,
            Some("https://example.test/docs/page.html"),
        );
    }

    #[test]
    fn complex_source_evidence_bypasses_the_ordinary_compiler() {
        for html in [
            r##"<p>Note<sup class="footnote-reference"><a href="#fn1">1</a></sup></p>"##,
            r#"<math><mi>x</mi></math>"#,
            r#"<div class="admonition warning"><p>Careful.</p></div>"#,
            r#"<picture><source srcset="large.png"><img src="small.png"></picture>"#,
            r#"<table><tr><td>Cell</td></tr></table>"#,
            r#"<table class="highlighttable"><tr><td class="linenos"><pre>1</pre></td><td><pre>code</pre></td></tr></table>"#,
            r#"<div role="list"><div role="listitem">One</div></div>"#,
            r#"<p><img src="formula.svg" alt="x^2"></p>"#,
        ] {
            assert!(
                !uses_ordinary_compiler(html),
                "unexpected ordinary route: {html}"
            );
        }
    }

    #[test]
    fn ambiguous_native_structures_and_image_sources_use_complex_compilation() {
        for html in [
            "<ul>stray text<li>item</li></ul>",
            "<figure><img src='plot.png'><figcaption>Plot</figcaption>trailing text</figure>",
            "<p><img src='null' alt='fallback'></p>",
            "<p><img src='undefined' alt='fallback'></p>",
        ] {
            assert!(
                !uses_ordinary_compiler(html),
                "unexpected ordinary route: {html}"
            );
        }
        assert_eq!(
            compile("<p><img src='null' alt='fallback'></p>", None).text(),
            "fallback"
        );
    }

    #[test]
    fn ordinary_compiler_keeps_comment_sensitive_inline_boundaries() {
        compare_ordinary_and_complex("<p><span>a</span><!-- marker -->b</p>", None);
        compare_ordinary_and_complex(
            "<p><em>a</em><span><img src='x.png' alt='icon'>b</span></p>",
            None,
        );
        assert_eq!(
            compile("<p><span>a</span><!-- marker -->b</p>", None).text(),
            "ab"
        );
    }

    #[test]
    fn empty_code_uses_the_complex_compiler_boundary_behavior() {
        for html in [
            "<p><em>a</em><span><code></code></span>b</p>",
            "<p><em>a</em><span><code>  </code></span>b</p>",
        ] {
            compare_ordinary_and_complex(html, None);
        }
        assert_eq!(
            compile("<p><em>a</em><span><code></code></span>b</p>", None).text(),
            "ab"
        );
        assert_eq!(
            compile("<p><em>a</em><span><code>  </code></span>b</p>", None).text(),
            "a b"
        );
    }

    #[test]
    fn phrasing_elements_with_block_children_use_complex_compilation() {
        for tag in [
            "abbr", "address", "bdi", "bdo", "cite", "dfn", "kbd", "mark", "q", "samp", "small",
            "span", "sub", "sup", "time", "u", "var",
        ] {
            let html = format!("<{tag}><div>block</div></{tag}>");
            assert!(
                !uses_ordinary_compiler(&html),
                "unexpected ordinary route: {html}"
            );
        }
    }

    #[test]
    fn misplaced_native_structural_elements_use_complex_compilation() {
        for html in [
            "<details><div><summary>Nested</summary></div></details>",
            "<dl>stray text<dt>Term</dt><dd>Definition</dd></dl>",
        ] {
            assert!(
                !uses_ordinary_compiler(html),
                "unexpected ordinary route: {html}"
            );
        }
    }

    #[test]
    fn transparent_boundary_with_deep_image_markup_remains_linear() {
        const DEPTH: usize = 10_000;
        let mut html = String::from("<p>x<span>");
        for index in 0..DEPTH {
            html.push_str(if index % 2 == 0 { "<strong>" } else { "<em>" });
        }
        html.push_str("<img src='image.png' alt='image'>");
        for index in (0..DEPTH).rev() {
            html.push_str(if index % 2 == 0 { "</strong>" } else { "</em>" });
        }
        html.push_str("</span>y</p>");

        compare_ordinary_and_complex(&html, None);
    }

    #[test]
    fn transparent_boundary_with_deep_punctuation_markup_remains_linear() {
        const DEPTH: usize = 10_000;
        let mut html = String::from("<p><em>x</em><span>");
        for index in 0..DEPTH {
            html.push_str(if index % 2 == 0 { "<strong>" } else { "<em>" });
        }
        html.push_str("!value");
        for index in (0..DEPTH).rev() {
            html.push_str(if index % 2 == 0 { "</strong>" } else { "</em>" });
        }
        html.push_str("</span></p>");

        assert!(uses_ordinary_compiler(&html));
        assert_eq!(compile(&html, None).text(), "x!value");
    }

    #[test]
    fn deeply_nested_ordinary_inline_markup_is_stack_safe() {
        const DEPTH: usize = 20_000;
        let mut html = String::from("<p>");
        for _ in 0..DEPTH {
            html.push_str("<span>");
        }
        html.push_str("deep");
        for _ in 0..DEPTH {
            html.push_str("</span>");
        }
        html.push_str("</p>");

        assert!(uses_ordinary_compiler(&html));
        let document = compile(&html, None);
        assert_eq!(document.text(), "deep");
    }

    #[test]
    fn compiles_heading_roles_and_omits_permalink_controls() {
        let document = compile(
            r##"<div role="heading" aria-level="3">Overview <a class="heading-anchor" href="#overview">#</a></div><h2>Release<span><a href="#release">#</a></span>guide</h2><h2><a href="/guide">Read the guide</a></h2>"##,
            None,
        );
        assert_eq!(
            document.debug_tree(),
            concat!(
                "Heading(level=3)\n",
                "  Text(\"Overview\")\n",
                "Heading(level=2)\n",
                "  Text(\"Release guide\")\n",
                "Heading(level=2)\n",
                "  Link(destination=\"/guide\", title=None)\n",
                "    Text(\"Read the guide\")\n",
            )
        );
    }

    #[test]
    fn omits_headings_that_contain_only_permalink_controls() {
        let document = compile(
            r##"<h2><a href="#native">#</a></h2><div role="heading" aria-level="3"><a class="heading-anchor" href="#aria"></a></div><p>Content.</p>"##,
            None,
        );
        assert_eq!(document.debug_tree(), "Paragraph\n  Text(\"Content.\")\n");
    }

    #[test]
    fn collapses_transparent_div_and_section_wrapper_chains() {
        let document = compile(
            "<div> \n <section>\n<div><p>Content.</p></div>\n</section> </div>",
            None,
        );
        assert_eq!(
            document.debug_tree(),
            "BlockGroup\n  Paragraph\n    Text(\"Content.\")\n"
        );
    }

    #[test]
    fn plain_prose_fast_path_does_not_bypass_source_semantics() {
        let document = compile(
            r#"<div class="admonition warning"><p>Warning</p><p>Take care.</p></div><div class="warning"><p>Warning</p><p>Also take care.</p></div><blockquote data-legible-callout="warning"><p>Another warning.</p></blockquote><p data-legible-math="inline" data-latex="x^2">x 2</p><div id="footnotes"><p id="fn1">A note.</p></div>"#,
            None,
        );
        let tree = document.debug_tree();
        assert_eq!(tree.matches("Callout(kind=Warning").count(), 3, "{tree}");
        assert!(tree.contains("DisplayMath(source=\"x^2\""), "{tree}");
        assert!(tree.contains("FootnoteDefinition"), "{tree}");
    }

    #[test]
    fn compiles_common_semantic_shapes() {
        let document = compile(
            r##"<h2>Guide</h2><p>Hello <strong>world</strong><br><a href="/more">more</a>.</p><ol start="3"><li><code>x = 1</code></li></ol><details><summary>More</summary><p>Detail</p></details>"##,
            Some("https://example.test/page"),
        );
        assert_eq!(
            document.debug_tree(),
            concat!(
                "Heading(level=2)\n",
                "  Text(\"Guide\")\n",
                "Paragraph\n",
                "  Text(\"Hello \")\n",
                "  Strong\n",
                "    Text(\"world\")\n",
                "  HardBreak\n",
                "  Link(destination=\"https://example.test/more\", title=None)\n",
                "    Text(\"more\")\n",
                "  Text(\".\")\n",
                "List(kind=Ordered, start=Some(3))\n",
                "  ListItem\n",
                "    InlineCode(\"x = 1\")\n",
                "Details\n",
                "  Summary\n",
                "    Text(\"More\")\n",
                "  Paragraph\n",
                "    Text(\"Detail\")\n",
            )
        );
    }

    #[test]
    fn compiles_hard_semantic_structures() {
        let document = compile(
            r#"<blockquote data-legible-callout="warning"><p><strong>Warning</strong></p></blockquote><pre><code data-language="rust">fn main() {
  run();
}
</code></pre><figure><img src="plot.png" alt="Plot" width="640"><figcaption>Result</figcaption></figure><table><thead><tr><th align="left">Name</th><th>Value</th></tr></thead><tbody><tr><td colspan="2">A</td></tr></tbody></table><p>Equation <span data-legible-math="inline" data-latex="x^2">x 2</span><sup data-legible-footnote-ref="n1">1</sup></p><ol><li data-legible-footnote="n1"><p>Note text.</p></li></ol>"#,
            None,
        );
        assert_eq!(
            document.debug_tree(),
            concat!(
                "Callout(kind=Warning, title=None)\n",
                "  Paragraph\n",
                "    Strong\n",
                "      Text(\"Warning\")\n",
                "CodeBlock(language=Some(\"rust\"), text=\"fn main() {\\n  run();\\n}\\n\")\n",
                "Figure\n",
                "  Image(source=\"plot.png\", alt=\"Plot\", title=None, width=Some(640), height=None)\n",
                "  Figcaption\n",
                "    Text(\"Result\")\n",
                "Table(columns=Some(2))\n",
                "  TableRow\n",
                "    TableCell(header=true, colspan=1, rowspan=1, alignment=Some(Left))\n",
                "      Text(\"Name\")\n",
                "    TableCell(header=true, colspan=1, rowspan=1, alignment=None)\n",
                "      Text(\"Value\")\n",
                "  TableRow\n",
                "    TableCell(header=false, colspan=2, rowspan=1, alignment=None)\n",
                "      Text(\"A\")\n",
                "Paragraph\n",
                "  Text(\"Equation \")\n",
                "  InlineMath(source=\"x^2\", format=Tex, fallback=Some(\"x 2\"))\n",
                "  FootnoteReference(0)\n",
                "List(kind=Ordered, start=None)\n",
                "  FootnoteDefinition(0)\n",
                "    Paragraph\n",
                "      Text(\"Note text.\")\n",
            )
        );
        assert_eq!(document.footnotes().len(), 1);
    }

    #[test]
    fn compiles_source_callout_math_and_footnote_semantics_directly() {
        let document = compile(
            r##"<div class="admonition warning"><p class="admonition-title"><strong>Warning</strong></p><p>Take care.</p></div><input class="footref-toggle" type="checkbox"><p>Equation <math><msup><mi>x</mi><mn>2</mn></msup></math>.<sup class="footnote-reference"><a href="#fn1">1</a></sup></p><section class="footnotes"><ol><li id="fn1"><p>Source note. <a class="footnote-backref" href="#ref1">↩</a></p></li></ol></section>"##,
            None,
        );
        assert_eq!(
            document.debug_tree(),
            concat!(
                "Callout(kind=Warning, title=Some(\"Warning\"))\n",
                "  Paragraph\n",
                "    Strong\n",
                "      Text(\"Warning\")\n",
                "  Paragraph\n",
                "    Text(\"Take care.\")\n",
                "Paragraph\n",
                "  Text(\"Equation \")\n",
                "  InlineMath(source=\"x^{2}\", format=Tex, fallback=Some(\"x 2\"))\n",
                "  Text(\".\")\n",
                "  FootnoteReference(0)\n",
                "BlockGroup\n",
                "  List(kind=Ordered, start=None)\n",
                "    FootnoteDefinition(0)\n",
                "      Paragraph\n",
                "        Text(\"Source note. \")\n",
            )
        );
    }

    #[test]
    fn overriding_base_urls_resolve_fragment_links() {
        let dom = Dom::parse_fragment(r##"<p><a href=" #part ">Part</a></p>"##, Tag::Div).unwrap();
        let source = Url::parse("https://example.test/article").unwrap();
        let base = Url::parse("https://cdn.example.test/content/").unwrap();
        let context = CompileContext::new(Some(base), Some(&source));
        let document = compile_document(&dom, dom.root(), &context).unwrap();
        assert!(
            document
                .debug_tree()
                .contains("destination=\"https://cdn.example.test/content/#part\"")
        );
        assert_eq!(document.stats().link_text_length, 4);
        assert_eq!(document.stats().link_density, 0.3);
    }

    #[test]
    fn external_noteref_urls_remain_links() {
        let document = compile(
            r##"<p><a role="doc-noteref" href="https://example.test/notes#fn1">external note</a></p><aside id="fn1" role="doc-footnote">Local definition.</aside>"##,
            None,
        );
        assert!(matches!(
            document
                .roots()
                .next()
                .and_then(|root| root.children().next())
                .map(|node| node.kind()),
            Some(NodeKind::Link(_))
        ));
    }

    #[test]
    fn compiles_highlighted_code_to_one_semantic_leaf() {
        let document = compile(
            r#"<div class="highlight language-rust"><pre><code><span data-line><span class="line-number">1</span><span>fn main() {</span></span><span data-line><span class="line-number">2</span><span>    run();</span></span><span data-line><span class="line-number">3</span><span>}</span></span></code></pre></div>"#,
            None,
        );
        assert_eq!(
            document.debug_tree(),
            "BlockGroup\n  CodeBlock(language=Some(\"rust\"), text=\"fn main() {\\n    run();\\n}\")\n"
        );
        assert_eq!(document.len(), 2);
    }

    #[test]
    fn preserves_spaces_from_empty_inline_wrappers() {
        let document = compile("<p>a<em> </em><span> </span>b</p>", None);
        assert_eq!(document.debug_tree(), "Paragraph\n  Text(\"a b\")\n");
    }

    #[test]
    fn compiles_normalized_media_and_rowspan_table_widths() {
        let document = compile(
            r#"<p><video src="movie.mp4" aria-label="Interview recording"></video></p><table><tr><td rowspan="2">A</td><td>B</td></tr><tr><td>C</td><td>D</td></tr></table>"#,
            None,
        );
        assert_eq!(
            document.debug_tree(),
            concat!(
                "Paragraph\n",
                "  Media(kind=Video, source=\"movie.mp4\", title=Some(\"Interview recording\"))\n",
                "Table(columns=None)\n",
                "  TableRow\n",
                "    TableCell(header=false, colspan=1, rowspan=2, alignment=None)\n",
                "      Text(\"A\")\n",
                "    TableCell(header=false, colspan=1, rowspan=1, alignment=None)\n",
                "      Text(\"B\")\n",
                "  TableRow\n",
                "    TableCell(header=false, colspan=1, rowspan=1, alignment=None)\n",
                "      Text(\"C\")\n",
                "    TableCell(header=false, colspan=1, rowspan=1, alignment=None)\n",
                "      Text(\"D\")\n",
            )
        );
    }

    #[test]
    fn unsafe_links_flatten_and_unsafe_images_keep_alt_text() {
        let document = compile(
            r#"<p><a href="javascript:alert(1)">safe label</a><img src="data:text/html,x" alt="diagram"></p>"#,
            None,
        );
        assert_eq!(
            document.debug_tree(),
            "Paragraph\n  Text(\"safe labeldiagram\")\n"
        );
    }

    #[test]
    fn image_selection_skips_unsafe_primary_sources() {
        let document = compile(
            r#"<img src="javascript:bad()" data-src="safe.jpg" alt=" diagram   label ">"#,
            Some("https://example.test/article"),
        );
        assert_eq!(
            document.debug_tree(),
            "Image(source=\"https://example.test/safe.jpg\", alt=\"diagram label\", title=None, width=None, height=None)\n"
        );
    }

    #[test]
    fn compiles_lazy_picture_and_figure_sources_without_img_elements() {
        let document = compile(
            r#"<picture data-src="photo.jpg" title="Photo"></picture><figure data-src="chart.png"><figcaption>Chart result</figcaption></figure><div class="image-with-caption"><picture data-src="wrapped.jpg"></picture><p class="caption">Wrapped result</p><p class="caption">Additional note</p></div>"#,
            None,
        );
        assert_eq!(
            document.debug_tree(),
            concat!(
                "Image(source=\"photo.jpg\", alt=\"\", title=Some(\"Photo\"), width=None, height=None)\n",
                "Figure\n",
                "  Image(source=\"chart.png\", alt=\"\", title=None, width=None, height=None)\n",
                "  Figcaption\n",
                "    Text(\"Chart result\")\n",
                "Figure\n",
                "  Image(source=\"wrapped.jpg\", alt=\"\", title=None, width=None, height=None)\n",
                "  Paragraph\n",
                "    Text(\"Additional note\")\n",
                "  Figcaption\n",
                "    Text(\"Wrapped result\")\n",
            )
        );
    }

    #[test]
    fn compiles_aria_lists_without_rewriting_the_dom() {
        let document = compile(
            r#"<div role="list"><div role="listitem">3. Deploy<div role="list"><div role="listitem">Nested</div></div></div><div role="listitem">4. Publish</div></div>"#,
            None,
        );
        assert_eq!(
            document.debug_tree(),
            concat!(
                "List(kind=Ordered, start=Some(3))\n",
                "  ListItem\n",
                "    Text(\"Deploy\")\n",
                "    List(kind=Unordered, start=None)\n",
                "      ListItem\n",
                "        Text(\"Nested\")\n",
                "  ListItem\n",
                "    Text(\"Publish\")\n",
            )
        );
    }

    #[test]
    fn native_lists_keep_aria_items_structural() {
        let document = compile(
            r#"<ol><div role="listitem"><strong>One</strong> detail</div></ol>"#,
            None,
        );
        assert_eq!(
            document.debug_tree(),
            concat!(
                "List(kind=Ordered, start=None)\n",
                "  ListItem\n",
                "    Strong\n",
                "      Text(\"One\")\n",
                "    Text(\" detail\")\n",
            )
        );
    }

    #[test]
    fn orphan_aria_items_inside_lists_stay_non_structural() {
        let document = compile(
            r#"<ul><li>Outer<div><div role="listitem">Nested orphan</div></div></li></ul>"#,
            None,
        );
        document.validate().unwrap();
        assert_eq!(
            document
                .debug_tree()
                .lines()
                .filter(|line| line.trim() == "ListItem")
                .count(),
            1
        );
    }

    #[test]
    fn compiles_layout_and_data_tables_to_distinct_semantics() {
        let document = compile(
            r#"<table role="presentation"><caption><h3>Overview</h3></caption><tr><td><h2>Left</h2><p>Prose</p></td><td>Right</td></tr></table><table><tr><th>Name</th><th>Value</th></tr><tr><td>A</td><td>1</td></tr></table>"#,
            None,
        );
        let tree = document.debug_tree();
        assert_eq!(
            tree.lines()
                .filter(|line| line.starts_with("Table("))
                .count(),
            1
        );
        assert!(
            tree.starts_with("Heading(level=3)\n  Text(\"Overview\")\nHeading(level=2)"),
            "{tree}"
        );
        assert!(tree.contains("Table(columns=Some(2))"));
    }

    #[test]
    fn listing_fallback_rows_accept_block_content() {
        let document = compile(
            r#"<table><tr><td>1.</td><td><a href='/one'>One</a></td></tr><tr><td></td><td>First metadata</td></tr><tr><td>2.</td><td><a href='/two'>Two</a></td></tr><tr><td></td><td>Second metadata</td></tr><tr><td>3.</td><td><a href='/three'>Three</a></td></tr><tr><td></td><td>Third metadata</td></tr><tr><td></td><td><div><p>Trailing explanation</p></div></td></tr></table>"#,
            None,
        );
        document.validate().unwrap();
        let tree = document.debug_tree();
        assert!(tree.starts_with("List(kind=Ordered, start=None)"), "{tree}");
        assert!(tree.contains("BlockGroup\n  Paragraph"), "{tree}");
    }

    #[test]
    fn specialized_tables_under_lists_get_item_boundaries() {
        let native_layout = compile(
            r#"<ul><table role="presentation"><tr><td>Layout text</td></tr></table></ul>"#,
            None,
        );
        native_layout.validate().unwrap();
        assert_eq!(
            native_layout
                .debug_tree()
                .lines()
                .filter(|line| line.trim() == "ListItem")
                .count(),
            1
        );

        let aria_listing = compile(
            r#"<div role="list"><div role="listitem">Intro</div><table><tr><td>1.</td><td><a href='/one'>One</a></td></tr><tr><td></td><td>First metadata</td></tr><tr><td>2.</td><td><a href='/two'>Two</a></td></tr><tr><td></td><td>Second metadata</td></tr><tr><td>3.</td><td><a href='/three'>Three</a></td></tr><tr><td></td><td>Third metadata</td></tr></table></div>"#,
            None,
        );
        aria_listing.validate().unwrap();
        let tree = aria_listing.debug_tree();
        assert!(
            tree.contains("  ListItem\n    List(kind=Ordered, start=None)"),
            "{tree}"
        );
    }

    #[test]
    fn deeply_wrapped_layout_table_cells_are_stack_safe() {
        const DEPTH: usize = 5_000;
        let wrappers = "<div>".repeat(DEPTH);
        let closing = "</div>".repeat(DEPTH);
        let html = format!(
            "<table role='presentation'><tr><td>{wrappers}Deep value{closing}</td></tr></table>"
        );
        let document = compile(&html, None);
        document.validate().unwrap();
        assert!(
            !document
                .debug_tree()
                .lines()
                .any(|line| line.starts_with("Table("))
        );
        assert_eq!(document.text(), "Deep value");
    }

    #[test]
    fn deeply_nested_multiline_code_compiles_in_linear_passes() {
        const DEPTH: usize = 5_000;
        let mut html = "<code>".repeat(DEPTH);
        html.push_str("deep\ncode");
        html.push_str(&"</code>".repeat(DEPTH));
        let document = compile(&html, None);
        assert_eq!(document.len(), 1);
        assert!(matches!(
            document.roots().next().map(|node| node.kind()),
            Some(NodeKind::CodeBlock(_))
        ));
    }

    #[test]
    fn compilation_is_stack_safe_for_deep_transparent_markup() {
        const DEPTH: usize = 5_000;
        let mut html = "<div>".repeat(DEPTH);
        html.push_str("deep");
        html.push_str(&"</div>".repeat(DEPTH));
        let document = compile(&html, None);
        assert_eq!(document.len(), 2);
        document.validate().unwrap();
    }
}
