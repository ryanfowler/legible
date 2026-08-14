use std::collections::{HashMap, HashSet};

use thiserror::Error;
use url::Url;

use super::{
    BuildError, Callout, CalloutKind, CodeBlock, DestinationKind, Document, DocumentBuilder,
    DocumentNodeId, FootnoteId, Image, Link, List, ListKind, MathFormat, MathValue, Media,
    MediaKind, NodeKind, Table, TableAlignment, TableCell, TaskMarker, ValidationError,
    safe_destination,
};
use crate::dom::{AttrName, Dom, NodeId, Tag};

#[derive(Clone, Debug, Default)]
pub(crate) struct CompileContext {
    base_url: Option<Url>,
}

impl CompileContext {
    pub(crate) fn new(base_url: Option<Url>) -> Self {
        Self { base_url }
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

#[derive(Clone, Copy)]
struct Task {
    node: NodeId,
    scope: Scope,
}

#[derive(Default)]
struct TableAnalysis {
    current_width: u32,
    maximum_width: u32,
    has_rowspan: bool,
}

/// Compiles the children of a normalized extraction root into semantic nodes.
pub(crate) fn compile_document(
    dom: &Dom,
    root: NodeId,
    context: &CompileContext,
) -> Result<Document, CompileError> {
    let mut block_descendants = vec![false; dom.len()];
    let mut meaningful_content = vec![false; dom.len()];
    let mut visible_text_content = vec![false; dom.len()];
    let mut first_visible = vec![None; dom.len()];
    let mut last_visible = vec![None; dom.len()];
    let nodes: Vec<_> = std::iter::once(root).chain(dom.descendants(root)).collect();
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
            dom.tag(child).is_some_and(is_block_tag) || block_descendants[child.index()]
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
                matches!(tag, Tag::Br | Tag::Code | Tag::Hr | Tag::Img)
                    || dom.attr(node, AttrName::DataMath).is_some()
                    || dom.attr(node, AttrName::DataFootnoteRef).is_some()
            })
            || dom
                .children(node)
                .any(|child| meaningful_content[child.index()]);
    }

    let mut nearest_list_item = vec![None; dom.len()];
    for &node in &nodes {
        nearest_list_item[node.index()] = if dom.tag(node) == Some(Tag::Li) {
            Some(node)
        } else {
            dom.parent(node)
                .and_then(|parent| nearest_list_item[parent.index()])
        };
    }

    let mut builder = DocumentBuilder::with_capacity(dom.len());
    let available_footnotes: HashSet<&str> = dom
        .descendants(root)
        .filter_map(|node| dom.attr(node, AttrName::DataFootnote))
        .collect();
    let mut footnote_ids = HashMap::<String, FootnoteId>::new();
    let mut table_layouts = HashMap::<DocumentNodeId, TableAnalysis>::new();
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
    let mut tasks = Vec::new();
    tasks.extend(dom.children_rev(root).map(|node| Task { node, scope }));

    while let Some(Task { node, scope }) = tasks.pop() {
        if let Some(text) = dom.text_node(node) {
            let whitespace_only = text.chars().all(char::is_whitespace);
            if !whitespace_only
                && !text.chars().next().is_some_and(char::is_whitespace)
                && inline_word_boundary_before(dom, node, &first_visible, &last_visible)
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
        ) {
            continue;
        }

        if let Some(kind) = dom.attr(node, AttrName::DataMath) {
            let source = dom
                .attr(node, AttrName::DataLatex)
                .unwrap_or_default()
                .trim();
            if !source.is_empty() {
                let value = MathValue {
                    source: source.into(),
                    format: MathFormat::Tex,
                    fallback_text: nonempty(dom.text(node)).map(Into::into),
                };
                builder.append(
                    scope.parent,
                    if kind.eq_ignore_ascii_case("block") {
                        NodeKind::DisplayMath(value)
                    } else {
                        NodeKind::InlineMath(value)
                    },
                )?;
                continue;
            }
        }

        if let Some(label) = dom.attr(node, AttrName::DataFootnoteRef) {
            if available_footnotes.contains(label) {
                let id = footnote_id(&mut footnote_ids, label)?;
                builder.append(scope.parent, NodeKind::FootnoteReference(id))?;
            } else {
                push_children(dom, node, scope, &mut tasks);
            }
            continue;
        }

        if let Some(label) = dom.attr(node, AttrName::DataFootnote) {
            let id = footnote_id(&mut footnote_ids, label)?;
            let definition = builder.append(scope.parent, NodeKind::FootnoteDefinition(id))?;
            builder.define_footnote(id, label, definition)?;
            push_children(
                dom,
                node,
                Scope {
                    parent: Some(definition),
                    ..scope
                },
                &mut tasks,
            );
            continue;
        }

        if tag == Tag::Pre {
            let code = CodeBlock {
                language: code_language(dom, node).map(Into::into),
                text: super::text::preformatted_text(&dom.text(node)),
            };
            builder.append(scope.parent, NodeKind::CodeBlock(code))?;
            continue;
        }
        if tag == Tag::Code {
            builder.append(
                scope.parent,
                NodeKind::InlineCode(super::text::preformatted_text(&dom.text(node))),
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
            let alt = dom.attr_by_local_name(node, "alt").unwrap_or_default();
            let source = dom.attr(node, AttrName::Src).and_then(|source| {
                safe_destination(source, context.base_url.as_ref(), DestinationKind::Resource)
            });
            if let Some(source) = source {
                builder.append(
                    scope.parent,
                    NodeKind::Image(Image {
                        source,
                        alt: alt.into(),
                        title: dom.attr(node, AttrName::Title).map(Into::into),
                        width: positive_u32(dom.attr(node, AttrName::Width)),
                        height: positive_u32(dom.attr(node, AttrName::Height)),
                    }),
                )?;
            } else {
                builder.append_prose(scope.parent, alt)?;
            }
            continue;
        }

        let mut next_scope = scope;
        let parent_is_block_group = scope
            .parent
            .is_some_and(|parent| matches!(builder.kind(parent), Some(NodeKind::BlockGroup)));
        let semantic = match tag {
            Tag::Caption if scope.table.is_some() => Some(NodeKind::TableCaption),
            Tag::P if block_descendants[node.index()] => Some(NodeKind::BlockGroup),
            Tag::P | Tag::Address | Tag::Caption => Some(NodeKind::Paragraph),
            Tag::H1 | Tag::H2 | Tag::H3 | Tag::H4 | Tag::H5 | Tag::H6
                if block_descendants[node.index()] =>
            {
                Some(NodeKind::BlockGroup)
            }
            Tag::H1 => Some(NodeKind::Heading { level: 1 }),
            Tag::H2 => Some(NodeKind::Heading { level: 2 }),
            Tag::H3 => Some(NodeKind::Heading { level: 3 }),
            Tag::H4 => Some(NodeKind::Heading { level: 4 }),
            Tag::H5 => Some(NodeKind::Heading { level: 5 }),
            Tag::H6 => Some(NodeKind::Heading { level: 6 }),
            Tag::Blockquote => dom
                .attr(node, AttrName::DataCallout)
                .and_then(callout_kind)
                .map(|kind| NodeKind::Callout(Callout { kind, title: None }))
                .or(Some(NodeKind::BlockQuote)),
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
            Tag::Figure => Some(NodeKind::Figure),
            Tag::Figcaption if scope.figure.is_some() => Some(NodeKind::Figcaption),
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
                    && meaningful_content[node.index()]
                    && normalized_media_kind(dom, node).is_some() =>
            {
                dom.attr(node, AttrName::Href).and_then(|source| {
                    safe_destination(source, context.base_url.as_ref(), DestinationKind::Resource)
                        .map(|source| {
                            NodeKind::Media(Media {
                                kind: normalized_media_kind(dom, node)
                                    .expect("guard validated media kind"),
                                source,
                                title: nonempty(dom.text(node)).map(Into::into),
                            })
                        })
                })
            }
            Tag::A
                if scope.link.is_none()
                    && !block_descendants[node.index()]
                    && meaningful_content[node.index()] =>
            {
                dom.attr(node, AttrName::Href).and_then(|destination| {
                    safe_destination(
                        destination,
                        context.base_url.as_ref(),
                        DestinationKind::Link,
                    )
                    .map(|destination| {
                        NodeKind::Link(Link {
                            destination,
                            title: dom.attr(node, AttrName::Title).map(Into::into),
                        })
                    })
                })
            }
            _ if is_block_tag(tag)
                && !(tag == Tag::Div
                    && parent_is_block_group
                    && dom.children(node).count() == 1) =>
            {
                Some(NodeKind::BlockGroup)
            }
            _ => None,
        };

        let Some(kind) = semantic else {
            if !is_block_tag(tag)
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
        match builder.kind_mut(semantic_node) {
            Some(NodeKind::List(_)) => next_scope.list = Some(semantic_node),
            Some(NodeKind::ListItem) => {}
            _ => {
                if !matches!(tag, Tag::Ul | Tag::Ol) {
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
            Tag::Figure => next_scope.figure = Some(semantic_node),
            Tag::Dl => next_scope.definition_list = Some(semantic_node),
            Tag::A => next_scope.link = Some(semantic_node),
            _ => {}
        }
        if !semantic_leaf {
            if tag == Tag::Figure {
                push_figure_children(dom, node, next_scope, &mut tasks);
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
    document.validate()?;
    Ok(document)
}

fn push_children(dom: &Dom, node: NodeId, scope: Scope, tasks: &mut Vec<Task>) {
    tasks.extend(
        dom.children_rev(node)
            .map(|child| Task { node: child, scope }),
    );
}

fn push_figure_children(dom: &Dom, node: NodeId, scope: Scope, tasks: &mut Vec<Task>) {
    let mut content = Vec::new();
    let mut captions = Vec::new();
    for child in dom.children(node) {
        if dom.tag(child) == Some(Tag::Figcaption) {
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
            .map(|child| Task { node: child, scope }),
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

fn normalized_media_kind(dom: &Dom, node: NodeId) -> Option<MediaKind> {
    match dom.attr(node, AttrName::DataLegibleKind)? {
        "audio" => Some(MediaKind::Audio),
        "video" => Some(MediaKind::Video),
        "embedded" => Some(MediaKind::Embedded),
        _ => None,
    }
}

fn is_block_tag(tag: Tag) -> bool {
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

fn code_language(dom: &Dom, pre: NodeId) -> Option<&str> {
    dom.element_children(pre)
        .find(|&child| dom.tag(child) == Some(Tag::Code))
        .and_then(|code| dom.attr(code, AttrName::DataLanguage))
        .or_else(|| dom.attr(pre, AttrName::DataLanguage))
        .filter(|language| {
            !language.is_empty()
                && language.len() <= 32
                && language.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'#' | b'-' | b'_' | b'.')
                })
        })
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

fn nonempty(value: String) -> Option<String> {
    (!value.trim().is_empty()).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compile(html: &str, base: Option<&str>) -> Document {
        let dom = Dom::parse_fragment(html, Tag::Div).unwrap();
        let context = CompileContext::new(base.map(|value| Url::parse(value).unwrap()));
        compile_document(&dom, dom.root(), &context).unwrap()
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
    fn preserves_spaces_from_empty_inline_wrappers() {
        let document = compile("<p>a<em> </em><span> </span>b</p>", None);
        assert_eq!(document.debug_tree(), "Paragraph\n  Text(\"a b\")\n");
    }

    #[test]
    fn compiles_normalized_media_and_rowspan_table_widths() {
        let document = compile(
            r#"<p><a href="movie.mp4" data-legible-kind="video">Interview recording</a></p><table><tr><td rowspan="2">A</td><td>B</td></tr><tr><td>C</td><td>D</td></tr></table>"#,
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
