use std::collections::HashMap;

use thiserror::Error;
use url::Url;

use super::{
    BuildCapacityPlan, BuildError, Callout, CalloutKind, CodeBlock, DestinationKind, Document,
    DocumentNodeId, FootnoteId, Image, Link, List, ListKind, MathFormat, MathValue, Media,
    SemanticKind, SemanticTapeBuilder, Table, TableAlignment, TableCell, TaskMarker,
    ValidationError, safe_destination, trim_destination,
};
use crate::dom::{AttrName, Dom, NodeId, NodeStateStore, Tag};
use crate::instrumentation::{Phase, PhaseGuard};
use crate::tokens::has_token;

use super::sparse::SparseNodeSet;

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
    /// The ordinary compiler found source structure that needs rich analysis.
    #[error("ordinary semantic lowering requires the complex compiler")]
    RequiresComplex,
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
    figure_wrapper: Option<DocumentNodeId>,
    definition_list: Option<DocumentNodeId>,
    link: Option<DocumentNodeId>,
    preserve_isolated_whitespace: bool,
}

enum ListingPlan {
    Item {
        primary: Vec<NodeId>,
        metadata: Option<Vec<NodeId>>,
    },
    Group {
        kind: SemanticKind,
        cells: Vec<NodeId>,
    },
}

enum Task {
    Node {
        node: NodeId,
        scope: Scope,
    },
    Prose {
        parent: Option<DocumentNodeId>,
    },
    HardBreak {
        parent: Option<DocumentNodeId>,
    },
    WrappedChildren {
        node: NodeId,
        scope: Scope,
        kind: SemanticKind,
    },
    Close {
        node: DocumentNodeId,
    },
    Listing {
        list: DocumentNodeId,
        plan: ListingPlan,
        scope: Scope,
    },
    DeferredFootnote {
        node: NodeId,
        label: Box<str>,
        scope: Scope,
        close_group: bool,
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

struct OpenTable {
    node: DocumentNodeId,
    analysis: TableAnalysis,
}

struct DeferredCaption {
    node: NodeId,
    scope: Scope,
}

/// Optional precomputed inputs. `Default` runs the compiler's own analysis.
#[derive(Default)]
pub(crate) struct CompileInputs<'a> {
    pub(crate) source_facts: Option<&'a super::facts::SemanticSourceFacts>,
    pub(crate) source_evidence: Option<&'a super::facts::SourceEvidence>,
    pub(crate) retained_stream: Option<&'a super::RetainedStream>,
}

/// Compiles the children of a retained source root into semantic nodes.
pub(crate) fn compile_document(
    dom: &Dom,
    root: NodeId,
    context: &CompileContext,
    inputs: &CompileInputs<'_>,
) -> Result<Document, CompileError> {
    let _phase = PhaseGuard::new(Phase::SemanticCompilation);
    if let Some(result) = try_ordinary_compilation(dom, root, context, inputs.retained_stream) {
        match result {
            Ok(document) => {
                record_document_metrics(&document);
                return Ok(document);
            }
            Err(CompileError::RequiresComplex) => {}
            Err(error) => return Err(error),
        }
    }

    let owned_evidence;
    let source_evidence = if let Some(source_evidence) = inputs.source_evidence {
        source_evidence
    } else {
        owned_evidence = super::facts::SourceEvidence::analyze(dom, root, &NodeStateStore::new());
        &owned_evidence
    };
    compile_complex_document(
        dom,
        root,
        context,
        inputs.source_facts,
        source_evidence,
        inputs.retained_stream,
    )
}

/// Compiles and releases an owned retained-source fragment.
///
/// Production extraction transfers its winning compact fragment here. This
/// keeps borrowed compilation available for diagnostics and tests without a
/// second compiler implementation.
pub(crate) fn compile_document_owned(
    mut dom: Dom,
    root: NodeId,
    context: &CompileContext,
    inputs: CompileInputs<'_>,
) -> Result<Document, CompileError> {
    let _phase = PhaseGuard::new(Phase::SemanticCompilation);
    if let Some(result) = try_ordinary_compilation(&dom, root, context, inputs.retained_stream) {
        match result {
            Ok(document) => {
                record_document_metrics(&document);
                return Ok(document);
            }
            Err(CompileError::RequiresComplex) => {}
            Err(error) => return Err(error),
        }
    }

    let owned_evidence;
    let source_evidence = if let Some(source_evidence) = inputs.source_evidence {
        source_evidence
    } else {
        owned_evidence = super::facts::SourceEvidence::analyze(&dom, root, &NodeStateStore::new());
        &owned_evidence
    };
    let analysis = analyze_complex_document(
        &dom,
        root,
        context,
        inputs.source_facts,
        source_evidence,
        inputs.retained_stream,
    );
    let source_node_count = analysis.facts.nodes().len();
    let mut owned_source_texts = super::code::take_owned_source_texts(
        &mut dom,
        &analysis.facts.inventory().owned_code_sources,
    );
    let result = lower_complex_document(&dom, root, context, analysis, owned_source_texts.as_mut());
    if let Ok(document) = &result {
        crate::instrumentation::record_semantic_source_nodes(source_node_count);
        record_document_metrics(document);
    }
    result
}

fn try_ordinary_compilation(
    dom: &Dom,
    root: NodeId,
    context: &CompileContext,
    retained_stream: Option<&super::RetainedStream>,
) -> Option<Result<Document, CompileError>> {
    let source_plan =
        super::ordinary::ordinary_source_gate_with_retained_nodes(dom, root, retained_stream)?;
    let source_node_count = source_plan.source_node_count;
    let result = super::ordinary::compile_with_retained_capacity_plan(
        dom,
        root,
        context,
        source_plan.capacity,
        retained_stream,
    );
    if result.is_ok() {
        crate::instrumentation::record_semantic_source_nodes(source_node_count);
    }
    Some(result)
}

fn record_document_metrics(document: &Document) {
    crate::instrumentation::record_semantic_operations(document.operations().len());
    crate::instrumentation::record_retained_bytes(document.retained_bytes_estimate());
}

struct ComplexSourceAnalysis {
    facts: super::facts::SemanticFacts,
    images: super::images::ImageAnalysis,
    // These flags are queried for every source node during lowering. Keep the
    // dense form because sparse lookups were slower on the complex benchmarks.
    figures: Vec<bool>,
    captions: Vec<bool>,
    media: super::media::MediaAnalysis,
    lists: super::lists::ListAnalysis,
    tables: super::tables::TableAnalysis,
    callouts: super::callouts::CalloutAnalysis,
    footnotes: super::footnotes::FootnoteAnalysis,
    math: super::math::MathAnalysis,
    media_separators: SparseNodeSet,
    text_after_media_separators: SparseNodeSet,
}

/// Capacity evidence for the analysis state held immediately before lowering.
/// This is benchmark-only evidence. It excludes allocator metadata and the
/// short-lived parser and cleanup buffers.
#[allow(dead_code)]
pub(crate) struct ComplexStorageMetrics {
    pub(crate) source_nodes: usize,
    pub(crate) lowering_passes: usize,
    pub(crate) conditional_separator_passes: usize,
    pub(crate) dense_bytes: usize,
    pub(crate) sparse_bytes: usize,
}

#[allow(dead_code)]
pub(crate) fn complex_storage_metrics_for_benchmark(
    dom: &Dom,
    root: NodeId,
    context: &CompileContext,
    source_facts: Option<&super::facts::SemanticSourceFacts>,
    source_evidence: &super::facts::SourceEvidence,
) -> ComplexStorageMetrics {
    let analysis =
        analyze_complex_document(dom, root, context, source_facts, source_evidence, None);
    let dense_bytes = analysis
        .figures
        .capacity()
        .saturating_add(analysis.captions.capacity())
        .saturating_mul(std::mem::size_of::<bool>());
    let sparse_bytes = analysis
        .images
        .storage_bytes()
        .saturating_add(analysis.media.storage_bytes())
        .saturating_add(analysis.callouts.storage_bytes())
        .saturating_add(analysis.footnotes.storage_bytes())
        .saturating_add(analysis.math.storage_bytes())
        .saturating_add(analysis.media_separators.allocated_bytes())
        .saturating_add(analysis.text_after_media_separators.allocated_bytes());
    ComplexStorageMetrics {
        source_nodes: analysis.facts.nodes().len(),
        lowering_passes: 1,
        conditional_separator_passes: usize::from(!analysis.media.is_empty()),
        dense_bytes,
        sparse_bytes,
    }
}

fn compile_complex_document(
    dom: &Dom,
    root: NodeId,
    context: &CompileContext,
    source_facts: Option<&super::facts::SemanticSourceFacts>,
    source_evidence: &super::facts::SourceEvidence,
    retained_stream: Option<&super::RetainedStream>,
) -> Result<Document, CompileError> {
    let analysis = analyze_complex_document(
        dom,
        root,
        context,
        source_facts,
        source_evidence,
        retained_stream,
    );
    let source_node_count = analysis.facts.nodes().len();
    let result = lower_complex_document(dom, root, context, analysis, None);
    if let Ok(document) = &result {
        crate::instrumentation::record_semantic_source_nodes(source_node_count);
        record_document_metrics(document);
    }
    result
}

fn analyze_complex_document(
    dom: &Dom,
    root: NodeId,
    context: &CompileContext,
    source_facts: Option<&super::facts::SemanticSourceFacts>,
    source_evidence: &super::facts::SourceEvidence,
    retained_stream: Option<&super::RetainedStream>,
) -> ComplexSourceAnalysis {
    let mut facts = super::facts::SemanticFacts::analyze_with_source_facts(
        dom,
        root,
        source_facts,
        Some(source_evidence),
        retained_stream,
    );
    let images = super::images::analyze_with_inventory(
        dom,
        facts.nodes(),
        &facts.inventory().images,
        context.base_url.as_ref(),
    );
    super::headings::analyze_complex(dom, &mut facts, &images);
    let (figures, captions) = super::figures::analyze_with_inventory(
        dom,
        facts.nodes(),
        &facts.inventory().figures,
        &images,
    );
    let media = super::media::analyze_with_facts(dom, &facts, context.base_url.as_ref());
    let lists = super::lists::ListAnalysis::analyze(dom, facts.inventory().lists.as_slice());
    let tables =
        super::tables::TableAnalysis::analyze_candidates(dom, facts.inventory().tables.as_slice());
    let callouts =
        super::callouts::CalloutAnalysis::analyze(dom, facts.nodes(), &facts.inventory().callouts);
    let footnotes = super::footnotes::FootnoteAnalysis::analyze_with_inventory(
        dom,
        root,
        &facts.inventory().footnotes,
        facts.nodes(),
    );
    let math = super::math::MathAnalysis::analyze_with_inventory_and_evidence(
        dom,
        facts.nodes(),
        &facts.inventory().math,
        Some(source_evidence),
    );
    if !facts.inventory().math.is_empty() || !facts.inventory().footnotes.is_empty() {
        facts.include_semantic_meaning(dom, |node| {
            math.value(node).is_some() || footnotes.reference(node).is_some()
        });
    }
    let (media_separators, text_after_media_separators) = if media.is_empty() {
        (SparseNodeSet::new(), SparseNodeSet::new())
    } else {
        media_separators(dom, root, &media)
    };
    ComplexSourceAnalysis {
        facts,
        images,
        figures,
        captions,
        media,
        lists,
        tables,
        callouts,
        footnotes,
        math,
        media_separators,
        text_after_media_separators,
    }
}

fn lower_complex_document(
    dom: &Dom,
    root: NodeId,
    context: &CompileContext,
    analysis: ComplexSourceAnalysis,
    mut owned_source_texts: Option<&mut super::code::OwnedSourceTexts>,
) -> Result<Document, CompileError> {
    let ComplexSourceAnalysis {
        facts,
        images,
        figures,
        captions,
        media,
        lists,
        tables,
        callouts,
        footnotes,
        math,
        media_separators,
        text_after_media_separators,
    } = analysis;
    let capacity = capacity_plan_for_lowering(&facts, &tables);
    let mut builder = SemanticTapeBuilder::with_plan(capacity);
    let mut footnote_ids = HashMap::<String, FootnoteId>::new();
    let mut open_tables: Vec<OpenTable> = Vec::new();
    let mut deferred_footnote_group = None;
    let mut deferred_captions = HashMap::<DocumentNodeId, Vec<DeferredCaption>>::new();
    let scope = Scope {
        parent: None,
        list: None,
        table: None,
        row: None,
        figure: None,
        figure_wrapper: None,
        definition_list: None,
        link: None,
        preserve_isolated_whitespace: false,
    };
    let mut tasks = facts
        .nodes()
        .iter()
        .filter_map(|&node| {
            footnotes
                .definition(node)
                .filter(|_| footnotes.is_deferred(node))
                .map(|label| (node, label))
        })
        .rev()
        .enumerate()
        .map(|(reverse_index, (node, label))| Task::DeferredFootnote {
            node,
            label: label.into(),
            scope,
            close_group: reverse_index == 0,
        })
        .collect::<Vec<_>>();
    tasks.extend(
        dom.children_rev(root)
            .map(|node| Task::Node { node, scope }),
    );

    while let Some(task) = tasks.pop() {
        let Task::Node { node, scope } = task else {
            match task {
                Task::Prose { parent } => {
                    builder.append_normalized_prose(parent, " ")?;
                }
                Task::HardBreak { parent } => {
                    builder.emit(parent, SemanticKind::HardBreak)?;
                }
                Task::WrappedChildren { node, scope, kind } => {
                    let parent = builder.emit(scope.parent, kind)?;
                    tasks.push(Task::Close { node: parent });
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
                Task::Close { node } => {
                    builder.close(node)?;
                    if let Some(table) = open_tables.pop_if(|table| table.node == node)
                        && let Some(value) = builder.table_mut(table.node)
                    {
                        value.column_count =
                            (!table.analysis.has_rowspan).then_some(table.analysis.maximum_width);
                    }
                    if let Some(captions) = deferred_captions.remove(&node) {
                        tasks.extend(captions.into_iter().rev().map(|caption| Task::Node {
                            node: caption.node,
                            scope: caption.scope,
                        }));
                    }
                }
                Task::Listing { list, plan, scope } => {
                    let (parent, children) = match plan {
                        ListingPlan::Item { primary, metadata } => {
                            let item = builder.emit(Some(list), SemanticKind::ListItem)?;
                            let mut children = Vec::new();
                            append_cell_tasks(
                                dom,
                                &primary,
                                Scope {
                                    parent: Some(item),
                                    ..scope
                                },
                                &mut children,
                            );
                            if let Some(metadata) = metadata {
                                children.push(Task::HardBreak { parent: Some(item) });
                                append_cell_tasks(
                                    dom,
                                    &metadata,
                                    Scope {
                                        parent: Some(item),
                                        ..scope
                                    },
                                    &mut children,
                                );
                            }
                            (item, children)
                        }
                        ListingPlan::Group { kind, cells } => {
                            let group = builder.emit(scope.parent, kind)?;
                            let mut children = Vec::new();
                            append_cell_tasks(
                                dom,
                                &cells,
                                Scope {
                                    parent: Some(group),
                                    ..scope
                                },
                                &mut children,
                            );
                            (group, children)
                        }
                    };
                    tasks.push(Task::Close { node: parent });
                    tasks.extend(children.into_iter().rev());
                }
                Task::DeferredFootnote {
                    node,
                    label,
                    mut scope,
                    close_group,
                } => {
                    let parent = if let Some(parent) = deferred_footnote_group {
                        parent
                    } else {
                        let parent = builder.emit(None, SemanticKind::BlockGroup)?;
                        deferred_footnote_group = Some(parent);
                        parent
                    };
                    scope.parent = Some(parent);
                    if close_group {
                        tasks.push(Task::Close { node: parent });
                    }
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
                    let paragraph = builder.emit(scope.parent, SemanticKind::Paragraph)?;
                    let (parent, close_parent) = if already_strong {
                        (paragraph, None)
                    } else {
                        let strong = builder.emit(Some(paragraph), SemanticKind::Strong)?;
                        (strong, Some(strong))
                    };
                    tasks.push(Task::Close { node: paragraph });
                    if let Some(close_parent) = close_parent {
                        tasks.push(Task::Close { node: close_parent });
                    }
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
        if captions[node.index()]
            && let Some(figure) = scope.figure
            && scope.parent != Some(figure)
            && let Some(wrapper) = scope.figure_wrapper
        {
            let mut deferred_scope = scope;
            deferred_scope.parent = Some(figure);
            deferred_scope.figure_wrapper = None;
            deferred_captions
                .entry(wrapper)
                .or_default()
                .push(DeferredCaption {
                    node,
                    scope: deferred_scope,
                });
            continue;
        }
        let heading_permalink = facts.is_heading_permalink(node);
        if tables.is_skipped(node)
            || footnotes.is_skipped(node)
            || math.is_skipped(node)
            || footnotes.is_deferred(node)
            || heading_permalink
        {
            if tables.emits_separator(node)
                || heading_permalink && facts.permalink_separates_words(node)
            {
                builder.append_normalized_prose(scope.parent, " ")?;
            }
            continue;
        }
        if footnotes.is_transparent(node) {
            push_children(dom, node, scope, &mut tasks);
            continue;
        }
        if dom.is_element(node) && super::code::is_code_language_label(dom, node) {
            continue;
        }
        if let Some(text) = dom.text_node(node) {
            if dom
                .parent(node)
                .is_some_and(|parent| super::code::is_code_language_label(dom, parent))
            {
                continue;
            }
            let text = tables
                .replacement_text(node)
                .or_else(|| lists.replacement_text(node))
                .unwrap_or(text);
            let trim_start = footnotes.should_trim_start(node) || facts.trims_heading_start(node);
            let text = if trim_start { text.trim_start() } else { text };
            let text = if facts.trims_heading_end(node) {
                text.trim_end()
            } else {
                text
            };
            let whitespace_only = text.chars().all(char::is_whitespace);
            if whitespace_only {
                let structural_parent = scope.parent.is_some_and(|parent| {
                    Some(parent) == scope.list
                        || Some(parent) == scope.table
                        || Some(parent) == scope.row
                        || Some(parent) == scope.definition_list
                });
                let preserve = !structural_parent
                    && builder.previous_child_is_inline(scope.parent)
                    && (scope.preserve_isolated_whitespace
                        || meaningful_inline_separator(dom, node, &facts));
                if !preserve {
                    continue;
                }
            }
            let text = if whitespace_only { " " } else { text };
            if !whitespace_only
                && !text.chars().next().is_some_and(char::is_whitespace)
                && (inline_word_boundary_before(dom, node, &facts)
                    || text_after_media_separators.contains(node))
            {
                builder.append_normalized_prose(scope.parent, " ")?;
            }
            let list_item = if scope.parent.is_some() && scope.parent == scope.list {
                Some(builder.emit(scope.parent, SemanticKind::ListItem)?)
            } else {
                None
            };
            builder.append_prose(list_item.or(scope.parent), text)?;
            if let Some(list_item) = list_item {
                builder.close(list_item)?;
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
            builder.emit(
                scope.parent,
                if math.block {
                    SemanticKind::DisplayMath(value)
                } else {
                    SemanticKind::InlineMath(value)
                },
            )?;
            continue;
        }

        if let Some(label) = footnotes.reference(node) {
            if footnotes.has_definition(label) {
                let id = footnote_id(&mut footnote_ids, label)?;
                builder.emit(scope.parent, SemanticKind::FootnoteReference(id))?;
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
            builder.emit(
                scope.parent,
                SemanticKind::CodeBlock(CodeBlock {
                    language: code.language.as_deref().map(Into::into),
                    text: code.into_text(None),
                }),
            )?;
            continue;
        }
        if tag == Tag::Table {
            let table_list_item = if scope.parent.is_some() && scope.parent == scope.list {
                Some(builder.emit(scope.parent, SemanticKind::ListItem)?)
            } else {
                None
            };
            if let Some(table_list_item) = table_list_item {
                tasks.push(Task::Close {
                    node: table_list_item,
                });
            }
            let table_scope = Scope {
                parent: table_list_item.or(scope.parent),
                ..scope
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
        if facts.is_code_block(node)
            && let Some(code) = owned_source_texts
                .as_deref()
                .and_then(|sources| {
                    super::code::recognize_owned_known_block(dom, node, true, sources)
                })
                .or_else(|| super::code::recognize_known_block(dom, node, true))
        {
            let language = code.language.as_deref().map(Into::into);
            let text = code.into_text(owned_source_texts.as_deref_mut());
            builder.emit(
                scope.parent,
                SemanticKind::CodeBlock(CodeBlock { language, text }),
            )?;
            continue;
        }
        if tag == Tag::Code {
            builder.append_inline_code(scope.parent, &dom.text(node))?;
            continue;
        }
        if tag == Tag::Input
            && dom
                .attr(node, AttrName::Type)
                .is_some_and(|value| value.eq_ignore_ascii_case("checkbox"))
        {
            builder.emit(
                scope.parent,
                SemanticKind::TaskMarker(TaskMarker {
                    checked: dom.attr(node, AttrName::Checked).is_some(),
                    fallback_label: (!nearest_list_item_has_visible_text(
                        dom, node, &lists, &facts,
                    ))
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
                builder.emit(
                    scope.parent,
                    SemanticKind::Image(semantic_image(dom, node, source)),
                )?;
            } else {
                builder.append_prose(scope.parent, &alt)?;
            }
            continue;
        }
        if tag == Tag::Picture && images.is_synthetic(node) {
            if let Some(source) = images.source(node).map(Into::into) {
                builder.emit(
                    scope.parent,
                    SemanticKind::Image(semantic_image(dom, node, source)),
                )?;
            }
            continue;
        }
        if matches!(tag, Tag::Iframe | Tag::Video | Tag::Audio) {
            if let Some(media) = media.item(node) {
                if media_separators.contains(node) {
                    builder.append_normalized_prose(scope.parent, " ")?;
                }
                builder.emit(
                    scope.parent,
                    SemanticKind::Media(Media {
                        kind: media.kind,
                        source: media.source.clone(),
                        title: Some(media.title.clone()),
                    }),
                )?;
                if let Some(fallback) = media.fallback {
                    builder.append_normalized_prose(scope.parent, " ")?;
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
            .is_some_and(|parent| builder.is_block_group(parent));
        let semantic = if let Some(level) = facts.heading_level(node) {
            if !facts.heading_has_meaningful_content(node) {
                None
            } else if facts.has_block_descendant(node) {
                Some(SemanticKind::BlockGroup)
            } else {
                Some(SemanticKind::Heading { level })
            }
        } else if let Some(callout) = &callout {
            callout_kind(callout.kind).map(|kind| {
                SemanticKind::Callout(Callout {
                    kind,
                    title: Some(callout.title.clone()),
                })
            })
        } else if figures[node.index()] {
            Some(SemanticKind::Figure)
        } else if captions[node.index()] && scope.figure.is_some() {
            Some(SemanticKind::Figcaption)
        } else if let Some(list) = lists.container(node) {
            Some(SemanticKind::List(list))
        } else if lists.is_item(node) && scope.list.is_some() {
            Some(SemanticKind::ListItem)
        } else {
            match tag {
                Tag::Caption if scope.table.is_some() => Some(SemanticKind::TableCaption),
                Tag::P if facts.has_block_descendant(node) => Some(SemanticKind::BlockGroup),
                Tag::P | Tag::Address | Tag::Caption => Some(SemanticKind::Paragraph),
                Tag::Blockquote => dom
                    .attr(node, AttrName::DataCallout)
                    .and_then(callout_kind)
                    .map(|kind| SemanticKind::Callout(Callout { kind, title: None }))
                    .or(Some(SemanticKind::BlockQuote)),
                Tag::Li if scope.list.is_some() => Some(SemanticKind::ListItem),
                Tag::Table => Some(SemanticKind::Table(Table {
                    column_count: Some(0),
                })),
                Tag::Tr if scope.table.is_some() => Some(SemanticKind::TableRow),
                Tag::Td | Tag::Th if scope.row.is_some() => {
                    Some(SemanticKind::TableCell(TableCell {
                        header: tag == Tag::Th,
                        colspan: positive_u32(dom.attr(node, AttrName::ColSpan)).unwrap_or(1),
                        rowspan: positive_u32(dom.attr(node, AttrName::RowSpan)).unwrap_or(1),
                        alignment: dom.attr(node, AttrName::Align).and_then(table_alignment),
                    }))
                }
                Tag::Details => Some(SemanticKind::Details),
                Tag::Summary => Some(SemanticKind::Summary),
                Tag::Hr => Some(SemanticKind::ThematicBreak),
                Tag::Dl => Some(SemanticKind::DefinitionList),
                Tag::Dt if scope.definition_list.is_some() => Some(SemanticKind::DefinitionTerm),
                Tag::Dd if scope.definition_list.is_some() => {
                    Some(SemanticKind::DefinitionDescription)
                }
                Tag::Dt | Tag::Dd => Some(SemanticKind::Paragraph),
                Tag::Strong | Tag::B | Tag::Em | Tag::I | Tag::Del
                    if facts.has_block_descendant(node) =>
                {
                    Some(SemanticKind::BlockGroup)
                }
                Tag::Strong | Tag::B | Tag::Em | Tag::I | Tag::Del
                    if !facts.has_meaningful_content(node) =>
                {
                    None
                }
                Tag::Strong | Tag::B => Some(SemanticKind::Strong),
                Tag::Em | Tag::I => Some(SemanticKind::Emphasis),
                Tag::Del => Some(SemanticKind::Strikethrough),
                Tag::Br => Some(SemanticKind::HardBreak),
                Tag::A
                    if scope.link.is_none()
                        && !facts.has_block_descendant(node)
                        && facts.has_meaningful_content(node) =>
                {
                    dom.attr(node, AttrName::Href).and_then(|destination| {
                        let trimmed = trim_destination(destination);
                        let fragment_only = trimmed.starts_with('#') && trimmed.len() > 1;
                        context.link_destination(destination).map(|destination| {
                            SemanticKind::Link(Link {
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
                    Some(SemanticKind::BlockGroup)
                }
                _ => None,
            }
        };

        let Some(kind) = semantic else {
            if !is_block_tag(tag)
                && tag != Tag::Sup
                && inline_word_boundary_before(dom, node, &facts)
            {
                builder.append_normalized_prose(scope.parent, " ")?;
            }
            let mut transparent_scope = scope;
            if !is_block_tag(tag) && !facts.has_meaningful_content(node) {
                transparent_scope.preserve_isolated_whitespace = true;
            }
            push_children(dom, node, transparent_scope, &mut tasks);
            continue;
        };
        if builder.is_redundant_formatting(scope.parent, &kind) {
            push_children(dom, node, scope, &mut tasks);
            continue;
        }
        let semantic_leaf = matches!(
            kind,
            SemanticKind::CodeBlock(_)
                | SemanticKind::Image(_)
                | SemanticKind::HardBreak
                | SemanticKind::ThematicBreak
                | SemanticKind::FootnoteReference(_)
                | SemanticKind::TaskMarker(_)
                | SemanticKind::InlineMath(_)
                | SemanticKind::DisplayMath(_)
                | SemanticKind::Media(_)
        );
        let cell_span = match &kind {
            SemanticKind::TableCell(cell) => Some((cell.colspan, cell.rowspan)),
            _ => None,
        };
        let list_item = if scope.parent.is_some()
            && scope.parent == scope.list
            && !matches!(
                kind,
                SemanticKind::Figcaption
                    | SemanticKind::ListItem
                    | SemanticKind::FootnoteDefinition(_)
            ) {
            Some(builder.emit(scope.parent, SemanticKind::ListItem)?)
        } else {
            None
        };
        if let Some(list_item) = list_item {
            tasks.push(Task::Close { node: list_item });
        }
        let semantic_parent = if matches!(kind, SemanticKind::Figcaption) {
            scope.figure
        } else {
            list_item.or(scope.parent)
        };
        let direct_figure_wrapper = scope.figure.is_some()
            && scope.figure_wrapper.is_none()
            && semantic_parent == scope.figure
            && !matches!(kind, SemanticKind::Figcaption);
        let semantic_node = builder.emit(semantic_parent, kind)?;
        next_scope.parent = Some(semantic_node);
        if tag == Tag::Figure {
            next_scope.figure_wrapper = None;
        } else if direct_figure_wrapper {
            next_scope.figure_wrapper = Some(semantic_node);
        }
        if figures[node.index()]
            && images.is_synthetic(node)
            && let Some(source) = images.source(node).map(Into::into)
        {
            builder.emit(
                Some(semantic_node),
                SemanticKind::Image(semantic_image(dom, node, source)),
            )?;
        }
        if builder.is_list(semantic_node) {
            next_scope.list = Some(semantic_node);
        } else if !builder.is_list_item(semantic_node) && lists.container(node).is_none() {
            next_scope.list = scope.list;
        }
        match tag {
            Tag::Table => {
                next_scope.table = Some(semantic_node);
                next_scope.row = None;
                open_tables.push(OpenTable {
                    node: semantic_node,
                    analysis: TableAnalysis::default(),
                });
            }
            Tag::Tr => {
                next_scope.row = Some(semantic_node);
                if let Some(table) = scope.table {
                    open_tables
                        .last_mut()
                        .filter(|entry| entry.node == table)
                        .expect("semantic table scope has an analysis")
                        .analysis
                        .current_width = 0;
                }
            }
            Tag::Td | Tag::Th => {
                if let Some(table) = scope.table {
                    let (colspan, rowspan) = cell_span.unwrap_or((1, 1));
                    let analysis = &mut open_tables
                        .last_mut()
                        .filter(|entry| entry.node == table)
                        .expect("semantic table scope has an analysis")
                        .analysis;
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
            tasks.push(Task::Close {
                node: semantic_node,
            });
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
                    let paragraph = builder.emit(Some(semantic_node), SemanticKind::Paragraph)?;
                    let strong = builder.emit(Some(paragraph), SemanticKind::Strong)?;
                    builder.append_prose(Some(strong), &callout.title)?;
                    builder.close(strong)?;
                    builder.close(paragraph)?;
                    push_children(dom, node, next_scope, &mut tasks);
                }
            } else if figures[node.index()] {
                push_figure_children(dom, node, next_scope, &captions, &mut tasks);
            } else {
                push_children(dom, node, next_scope, &mut tasks);
            }
        }
    }

    let document = builder.finish()?;
    #[cfg(any(test, debug_assertions))]
    document.validate()?;
    Ok(document)
}

fn capacity_plan_for_lowering(
    facts: &super::facts::SemanticFacts,
    tables: &super::tables::TableAnalysis,
) -> BuildCapacityPlan {
    let mut capacity = facts.capacity_plan();
    capacity.lists = capacity.lists.saturating_add(tables.listing_count());
    capacity.code_blocks = capacity.code_blocks.max(tables.gutter_table_count());
    capacity
}

fn media_separators(
    dom: &Dom,
    root: NodeId,
    media: &super::media::MediaAnalysis,
) -> (SparseNodeSet, SparseNodeSet) {
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

    let mut before_media = SparseNodeSet::new();
    let mut after_media = SparseNodeSet::new();
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
            if matches!(previous, PreviousInline::Media) && starts_word {
                after_media.push(node);
            }
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
            if matches!(previous, PreviousInline::Word | PreviousInline::Media) {
                before_media.push(node);
            }
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
    before_media.sort();
    after_media.sort();
    before_media.build_dense_index(dom.len());
    after_media.build_dense_index(dom.len());
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
                kind: SemanticKind::Paragraph,
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
                    kind: SemanticKind::Paragraph,
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
    builder: &mut SemanticTapeBuilder,
    tasks: &mut Vec<Task>,
) -> Result<(), BuildError> {
    let list = builder.emit(
        scope.parent,
        SemanticKind::List(List {
            kind: ListKind::Ordered,
            start: (start != 1).then_some(i64::from(start)),
        }),
    )?;
    let mut plans = Vec::new();
    let mut current_item = None;
    let mut expects_metadata = false;
    for &row in analysis.rows(table) {
        let cells = analysis.cells(row);
        if analysis.row_has_rank(row) {
            let primary = cells
                .iter()
                .copied()
                .skip(1)
                .filter(|&cell| analysis.meaningful_cell(cell))
                .collect();
            plans.push(ListingPlan::Item {
                primary,
                metadata: None,
            });
            current_item = Some(plans.len() - 1);
            expects_metadata = true;
        } else if !analysis.row_has_content(row) {
            continue;
        } else if expects_metadata {
            if let Some(index) = current_item {
                let metadata = cells
                    .iter()
                    .copied()
                    .filter(|&cell| analysis.meaningful_cell(cell))
                    .collect();
                if let Some(ListingPlan::Item {
                    metadata: current_metadata,
                    ..
                }) = plans.get_mut(index)
                {
                    *current_metadata = Some(metadata);
                }
            }
            expects_metadata = false;
        } else {
            let cells = cells
                .iter()
                .copied()
                .filter(|&cell| analysis.meaningful_cell(cell))
                .collect::<Vec<_>>();
            let kind = if cells.iter().all(|&cell| analysis.cell_is_phrasing(cell)) {
                SemanticKind::Paragraph
            } else {
                SemanticKind::BlockGroup
            };
            plans.push(ListingPlan::Group { kind, cells });
        }
    }
    let mut item_plans = Vec::new();
    let mut group_plans = Vec::new();
    for plan in plans {
        match plan {
            ListingPlan::Item { .. } => item_plans.push(plan),
            ListingPlan::Group { .. } => group_plans.push(plan),
        }
    }
    // A fallback row is outside the ordered list. Close the list before those
    // plans, while keeping all list items in the list's source-order stream.
    tasks.extend(
        group_plans
            .into_iter()
            .rev()
            .map(|plan| Task::Listing { list, plan, scope }),
    );
    tasks.push(Task::Close { node: list });
    tasks.extend(
        item_plans
            .into_iter()
            .rev()
            .map(|plan| Task::Listing { list, plan, scope }),
    );
    let _ = dom;
    Ok(())
}

fn append_cell_tasks(dom: &Dom, cells: &[NodeId], scope: Scope, ordered: &mut Vec<Task>) {
    let mut inserted = false;
    for &cell in cells {
        if inserted {
            ordered.push(Task::Prose {
                parent: scope.parent,
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
    builder: &mut SemanticTapeBuilder,
    tasks: &mut Vec<Task>,
) -> Result<(), BuildError> {
    let id = footnote_id(footnote_ids, label)?;
    let definition = builder.emit(scope.parent, SemanticKind::FootnoteDefinition(id))?;
    builder.define_footnote(id, label, definition)?;
    tasks.push(Task::Close { node: definition });
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
    facts: &super::facts::SemanticFacts,
) -> bool {
    facts.first_visible(node).is_some_and(char::is_alphanumeric)
        && dom.prev_sibling(node).is_some_and(|previous| {
            facts
                .last_visible(previous)
                .is_some_and(char::is_alphanumeric)
                && dom.tag(previous).is_some_and(|tag| !is_block_tag(tag))
        })
}

fn meaningful_inline_separator(
    dom: &Dom,
    node: NodeId,
    facts: &super::facts::SemanticFacts,
) -> bool {
    dom.prev_sibling(node)
        .is_some_and(|sibling| is_inline_dom_node(dom, sibling, facts))
        && dom
            .next_sibling(node)
            .is_some_and(|sibling| is_inline_dom_node(dom, sibling, facts))
}

fn is_inline_dom_node(dom: &Dom, node: NodeId, facts: &super::facts::SemanticFacts) -> bool {
    dom.text_node(node)
        .is_some_and(|text| text.chars().any(|character| !character.is_whitespace()))
        || dom.tag(node).is_some_and(|tag| {
            !facts.has_block_descendant(node)
                && !is_block_tag(tag)
                && !matches!(
                    tag,
                    Tag::Head | Tag::Script | Tag::Style | Tag::Template | Tag::Noscript
                )
        })
}

fn nearest_list_item_has_visible_text(
    dom: &Dom,
    node: NodeId,
    lists: &super::lists::ListAnalysis,
    facts: &super::facts::SemanticFacts,
) -> bool {
    dom.ancestors(node)
        .find(|&ancestor| dom.tag(ancestor) == Some(Tag::Li) || lists.is_item(ancestor))
        .is_some_and(|item| facts.has_visible_text(item))
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
            .filter(|roles| has_token(roles, "heading"))
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
        compile_document(&dom, dom.root(), &context, &CompileInputs::default()).unwrap()
    }

    #[test]
    fn owned_compilation_matches_borrowed_compilation() {
        let html = r##"<article><h2>Owned input</h2><p>Text with <a href="/safe">a link</a> and <a href="javascript:bad">unsafe text</a>.</p><pre><code class="language-rust">let value = 1;</code></pre><figure><img src="/image.png" alt="Diagram"><figcaption>Figure label</figcaption></figure><p>Equation <math><mi>x</mi></math>.<sup><a role="doc-noteref" href="#note">1</a></sup></p><aside id="note" role="doc-footnote">Note text.</aside></article>"##;
        let dom = Dom::parse_fragment(html, Tag::Div).unwrap();
        let root = dom.root();
        let base = Url::parse("https://example.test/base/").unwrap();
        let context = CompileContext::new(Some(base.clone()), Some(&base));
        let borrowed = compile_document(&dom, root, &context, &CompileInputs::default()).unwrap();

        let owned = compile_document_owned(dom, root, &context, CompileInputs::default()).unwrap();

        assert_eq!(owned.debug_tape(), borrowed.debug_tape());
        assert_eq!(owned.stats(), borrowed.stats());
    }

    #[test]
    fn owned_compilation_preserves_gutter_table_code() {
        let source = "fn main() { println!(\"owned\"); }\n".repeat(512);
        let html = format!(
            r#"<article><h2>Gutter code</h2><table class="lntable"><tbody><tr><td class="lntd"><pre><code><span class="lnt">1</span><span class="lnt">2</span></code></pre></td><td class="lntd"><pre><code class="language-rust">{source}</code></pre></td></tr></tbody></table></article>"#
        );
        let dom = Dom::parse_fragment(&html, Tag::Div).unwrap();
        let root = dom.root();
        let context = CompileContext::default();
        let borrowed = compile_document(&dom, root, &context, &CompileInputs::default()).unwrap();

        let owned = compile_document_owned(dom, root, &context, CompileInputs::default()).unwrap();

        assert_eq!(owned.debug_tape(), borrowed.debug_tape());
        assert_eq!(owned.stats(), borrowed.stats());
    }

    fn uses_ordinary_compiler(html: &str) -> bool {
        let dom = Dom::parse_fragment(html, Tag::Div).unwrap();
        super::super::ordinary::supports(&dom, dom.root())
    }

    #[test]
    fn capacity_depth_counts_semantic_containers_not_transparent_wrappers() {
        let wrappers = "<span>".repeat(1_024);
        let closing = "</span>".repeat(1_024);
        let html = format!("<p>{wrappers}text{closing}</p>");
        let dom = Dom::parse_fragment(&html, Tag::Div).unwrap();
        let root = dom.root();

        let ordinary =
            super::super::ordinary::ordinary_source_gate_with_retained_nodes(&dom, root, None)
                .unwrap();
        assert_eq!(ordinary.capacity.max_depth, 1);

        let evidence =
            super::super::facts::SourceEvidence::analyze(&dom, root, &NodeStateStore::new());
        let complex = analyze_complex_document(
            &dom,
            root,
            &CompileContext::default(),
            None,
            &evidence,
            None,
        );
        assert_eq!(complex.facts.capacity_plan().max_depth, 1);
    }

    fn compare_ordinary_and_complex(html: &str, base: Option<&str>) {
        let dom = Dom::parse_fragment(html, Tag::Div).unwrap();
        let base = base.map(|value| Url::parse(value).unwrap());
        let context = CompileContext::new(base.clone(), base.as_ref());
        let source_evidence =
            super::super::facts::SourceEvidence::analyze(&dom, dom.root(), &NodeStateStore::new());
        let ordinary = super::super::ordinary::compile(
            &dom,
            dom.root(),
            &context,
            super::super::ordinary::ordinary_source_gate(&dom, dom.root()).unwrap(),
        )
        .unwrap();
        let retained_nodes: Vec<_> = dom.descendants(dom.root()).collect();
        let retained_stream =
            super::super::RetainedStream::from_preorder(&dom, dom.root(), &retained_nodes);
        let retained = super::super::ordinary::compile_with_retained_nodes(
            &dom,
            dom.root(),
            &context,
            super::super::ordinary::ordinary_source_gate_with_retained_nodes(
                &dom,
                dom.root(),
                Some(&retained_stream),
            )
            .unwrap()
            .source_node_count,
            Some(&retained_nodes),
        )
        .unwrap();
        let complex =
            compile_complex_document(&dom, dom.root(), &context, None, &source_evidence, None)
                .unwrap();
        assert_eq!(ordinary.debug_tape(), complex.debug_tape());
        assert_eq!(ordinary.debug_tape(), retained.debug_tape());
    }

    #[test]
    fn ordinary_compiler_handles_common_inline_and_block_semantics() {
        let html = r#"<h2>Read <em>this <strong>guide</strong></em></h2><p>Use <code>x = 1</code> with <del>old</del> and <a href="/relative">relative</a> or <a href="https://elsewhere.test/page">absolute</a> links.</p><blockquote><p>Quoted text.</p></blockquote><ul><li>One</li><li>Two</li></ul><ol start="4"><li>Four</li></ol><pre>plain code
</pre>"#;
        assert!(uses_ordinary_compiler(html));
        let document = compile(html, Some("https://example.test/base/"));
        assert_eq!(
            document.debug_tape(),
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
            document.debug_tape(),
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
            document.debug_tape(),
            concat!("Paragraph\n", "  Text(\"onetwo three unsafe fallback\")\n",)
        );
    }

    #[test]
    fn ordinary_boundaries_do_not_move_before_merged_fallback_text() {
        let html = r#"<p><span>x</span><span><img src="javascript:bad" alt="b">c</span></p>"#;
        let dom = Dom::parse_fragment(html, Tag::Div).unwrap();
        let context = CompileContext::default();
        let source_evidence =
            super::super::facts::SourceEvidence::analyze(&dom, dom.root(), &NodeStateStore::new());
        let ordinary = super::super::ordinary::compile(
            &dom,
            dom.root(),
            &context,
            super::super::ordinary::ordinary_source_gate(&dom, dom.root()).unwrap(),
        )
        .unwrap();
        let complex =
            compile_complex_document(&dom, dom.root(), &context, None, &source_evidence, None)
                .unwrap();
        assert_eq!(ordinary.text(), "x bc");
        assert_eq!(ordinary.text(), complex.text());
    }

    #[test]
    fn ordinary_compiler_matches_complex_for_article_collections() {
        compare_ordinary_and_complex(
            r#"<article><h1>Archive design</h1><p>The guide explains how the archive stores each record.</p><section class="related-content-tout"><h2>Collection</h2><p>This collection is part of the guide.</p><a href="/archive">Open the archive</a></section><h2>Validation</h2><p>The validation step compares every stored record.</p></article>"#,
            Some("https://example.test/docs/page.html"),
        );
    }

    #[test]
    fn ordinary_gate_keeps_nonsemantic_classed_wrappers_on_the_fast_path() {
        assert!(uses_ordinary_compiler(
            r#"<article class="post-content"><p>A classed wrapper can still contain ordinary prose.</p></article>"#
        ));
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
            r#"<aside class="side-note">A sidenote.</aside>"#,
            r#"<aside class="reference-text">A reference.</aside>"#,
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
            "<details>stray text<summary>Nested</summary></details>",
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
            document.debug_tape(),
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
        assert_eq!(document.debug_tape(), "Paragraph\n  Text(\"Content.\")\n");
    }

    #[test]
    fn collapses_transparent_div_and_section_wrapper_chains() {
        let document = compile(
            "<div> \n <section>\n<div><p>Content.</p></div>\n</section> </div>",
            None,
        );
        assert_eq!(
            document.debug_tape(),
            "BlockGroup\n  Paragraph\n    Text(\"Content.\")\n"
        );
    }

    #[test]
    fn plain_prose_fast_path_does_not_bypass_source_semantics() {
        let document = compile(
            r#"<div class="admonition warning"><p>Warning</p><p>Take care.</p></div><div class="warning"><p>Warning</p><p>Also take care.</p></div><blockquote data-legible-callout="warning"><p>Another warning.</p></blockquote><p data-legible-math="inline" data-latex="x^2">x 2</p><div id="footnotes"><p id="fn1">A note.</p></div>"#,
            None,
        );
        let tree = document.debug_tape();
        assert_eq!(tree.matches("Callout(kind=Warning").count(), 3, "{tree}");
        assert!(tree.contains("DisplayMath(source=\"x^2\""), "{tree}");
        assert!(tree.contains("FootnoteDefinition"), "{tree}");
    }

    #[test]
    fn routes_an_explicit_blockquote_callout_to_semantic_compilation() {
        let document = compile(
            r#"<blockquote data-legible-callout="warning"><p>Warning text.</p></blockquote>"#,
            None,
        );
        assert!(document.debug_tape().starts_with("Callout(kind=Warning"));
    }

    #[test]
    fn compiles_common_semantic_shapes() {
        let document = compile(
            r##"<h2>Guide</h2><p>Hello <strong>world</strong><br><a href="/more">more</a>.</p><ol start="3"><li><code>x = 1</code></li></ol><details><summary>More</summary><p>Detail</p></details>"##,
            Some("https://example.test/page"),
        );
        assert_eq!(
            document.debug_tape(),
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
            document.debug_tape(),
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
        assert_eq!(document.footnotes.len(), 1);
    }

    #[test]
    fn compiles_source_callout_math_and_footnote_semantics_directly() {
        let document = compile(
            r##"<div class="admonition warning"><p class="admonition-title"><strong>Warning</strong></p><p>Take care.</p></div><input class="footref-toggle" type="checkbox"><p>Equation <math><msup><mi>x</mi><mn>2</mn></msup></math>.<sup class="footnote-reference"><a href="#fn1">1</a></sup></p><section class="footnotes"><ol><li id="fn1"><p>Source note. <a class="footnote-backref" href="#ref1">↩</a></p></li></ol></section>"##,
            None,
        );
        assert_eq!(
            document.debug_tape(),
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
        let document =
            compile_document(&dom, dom.root(), &context, &CompileInputs::default()).unwrap();
        assert!(
            document
                .debug_tape()
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
            document.operation_view(1),
            Some(super::super::SemanticItemView::Link(_))
        ));
    }

    #[test]
    fn compiles_highlighted_code_to_one_semantic_leaf() {
        let document = compile(
            r#"<div class="highlight language-rust"><pre><code><span data-line><span class="line-number">1</span><span>fn main() {</span></span><span data-line><span class="line-number">2</span><span>    run();</span></span><span data-line><span class="line-number">3</span><span>}</span></span></code></pre></div>"#,
            None,
        );
        assert_eq!(
            document.debug_tape(),
            "BlockGroup\n  CodeBlock(language=Some(\"rust\"), text=\"fn main() {\\n    run();\\n}\")\n"
        );
        assert_eq!(document.len(), 2);
    }

    #[test]
    fn preserves_spaces_from_empty_inline_wrappers() {
        let document = compile("<p>a<em> </em><span> </span>b</p>", None);
        assert_eq!(document.debug_tape(), "Paragraph\n  Text(\"a b\")\n");
    }

    #[test]
    fn compiles_normalized_media_and_rowspan_table_widths() {
        let document = compile(
            r#"<p><video src="movie.mp4" aria-label="Interview recording"></video></p><table><tr><td rowspan="2">A</td><td>B</td></tr><tr><td>C</td><td>D</td></tr></table>"#,
            None,
        );
        assert_eq!(
            document.debug_tape(),
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
            document.debug_tape(),
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
            document.debug_tape(),
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
            document.debug_tape(),
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
            document.debug_tape(),
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
            document.debug_tape(),
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
                .debug_tape()
                .lines()
                .filter(|line| line.trim() == "ListItem")
                .count(),
            1
        );
    }

    #[test]
    fn figure_captions_can_escape_wrappers_without_dropping_later_siblings() {
        let document = compile(
            "<figure><div><img src='plot.png' alt='Plot'><figcaption>Caption</figcaption><span>After</span></div></figure>",
            None,
        );
        document.validate().unwrap();
        let tree = document.debug_tape();
        assert!(tree.contains("Figcaption\n    Text(\"Caption\")"), "{tree}");
        assert!(tree.contains("Text(\"After\")"), "{tree}");
    }

    #[test]
    fn compiles_layout_and_data_tables_to_distinct_semantics() {
        let document = compile(
            r#"<table role="presentation"><caption><h3>Overview</h3></caption><tr><td><h2>Left</h2><p>Prose</p></td><td>Right</td></tr></table><table><tr><th>Name</th><th>Value</th></tr><tr><td>A</td><td>1</td></tr></table>"#,
            None,
        );
        let tree = document.debug_tape();
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
        let tree = document.debug_tape();
        assert!(tree.starts_with("List(kind=Ordered, start=None)"), "{tree}");
        assert!(tree.contains("BlockGroup\n  Paragraph"), "{tree}");
    }

    #[test]
    fn capacity_plan_includes_synthetic_table_payloads() {
        let html = r#"<table><tr><td>1.</td><td><a href="/one">First</a></td></tr><tr><td></td><td>Metadata</td></tr><tr><td>2.</td><td><a href="/two">Second</a></td></tr><tr><td></td><td>More metadata</td></tr><tr><td>3.</td><td><a href="/three">Third</a></td></tr><tr><td></td><td>Last metadata</td></tr></table><table class="lntable"><tbody><tr><td class="lntd"><pre><code><span class="lnt">1</span></code></pre></td><td class="lntd"><pre><code class="language-rust">let value = 1;</code></pre></td></tr></tbody></table>"#;
        let dom = Dom::parse_fragment(html, Tag::Div).unwrap();
        let root = dom.root();
        let evidence =
            super::super::facts::SourceEvidence::analyze(&dom, root, &NodeStateStore::new());
        let analysis = analyze_complex_document(
            &dom,
            root,
            &CompileContext::default(),
            None,
            &evidence,
            None,
        );
        let capacity = capacity_plan_for_lowering(&analysis.facts, &analysis.tables);

        assert_eq!(analysis.tables.listing_count(), 1);
        assert_eq!(analysis.tables.gutter_table_count(), 1);
        assert!(capacity.lists >= analysis.tables.listing_count());
        assert!(capacity.code_blocks >= analysis.tables.gutter_table_count());
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
                .debug_tape()
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
        let tree = aria_listing.debug_tape();
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
                .debug_tape()
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
            document.operation_view(0),
            Some(super::super::SemanticItemView::CodeBlock(_))
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
