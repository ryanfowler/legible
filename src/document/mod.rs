//! Semantic document intermediate representation.
//!
//! [`Document`] is a read-only semantic representation. It is not an HTML DOM or a
//! CommonMark syntax tree. Legible intentionally removes site chrome, source
//! classes and IDs, arbitrary CSS, and implementation wrappers. It normalizes
//! retained HTML structures into useful semantic nodes. This representation is
//! lossy: callers cannot reconstruct unsupported elements, source attributes,
//! wrapper structure, or source whitespace from it.
//!
//! Traverse roots and children to inspect structured extracted content:
//!
//! ```rust
//! use legible::{NodeKind, extract};
//!
//! let page = extract(
//!     r#"<main>
//!       <h1>API guide</h1>
//!       <p>Read the <a href="/reference">reference</a>.</p>
//!       <pre><code class="language-rust">fn main() {}</code></pre>
//!       <table><tr><th>Name</th></tr><tr><td>value</td></tr></table>
//!     </main>"#,
//!     Some("https://example.com/docs"),
//! )?;
//!
//! let document = page.document();
//! let mut nodes: Vec<_> = document.roots().rev().collect();
//! while let Some(node) = nodes.pop() {
//!     match node.kind() {
//!         NodeKind::Heading { level } => println!("h{level}: {}", node.text()),
//!         NodeKind::Paragraph => println!("paragraph: {}", node.text()),
//!         NodeKind::Link(link) => println!("link: {}", link.destination()),
//!         NodeKind::CodeBlock(code) => println!("code: {}", code.text()),
//!         NodeKind::Table(table) => println!("table: {:?}", table.column_count()),
//!         _ => {}
//!     }
//!     let children: Vec<_> = node.children().collect();
//!     nodes.extend(children.into_iter().rev());
//! }
//! # Ok::<(), legible::Error>(())
//! ```

#![allow(dead_code)]

mod builder;
mod callouts;
mod code;
mod compiler;
mod figures;
mod footnotes;
mod images;
mod lists;
mod math;
mod media;
pub(crate) mod stats;
mod tables;
mod text;
mod uri;
mod validate;

pub(crate) use builder::{BuildError, DocumentBuilder};
pub(crate) use code::{
    class_is_semantic_evidence as code_class_is_semantic_evidence,
    count_blocks as source_code_block_count,
    is_multiline_orphan_with_evidence as is_multiline_code_with_evidence,
    multiline_content as code_multiline_content,
};
pub(crate) use compiler::{CompileContext, compile_document};
pub(crate) use figures::class_is_semantic_evidence as figure_class_is_semantic_evidence;
pub(crate) use footnotes::{
    Definitions as ExternalFootnoteDefinitions, adopt_external as adopt_external_footnotes,
    collect_external as collect_external_footnotes,
};
pub(crate) use math::accessible_math_nodes;
pub(crate) fn media_cleanup_evidence(
    dom: &crate::dom::Dom,
    nodes: &[crate::dom::NodeId],
) -> (Vec<bool>, Vec<Option<crate::dom::NodeId>>) {
    media::cleanup_evidence(dom, nodes)
}

pub(crate) fn selected_image_sources_for_cleanup(
    dom: &crate::dom::Dom,
    nodes: &[crate::dom::NodeId],
) -> Vec<Option<Box<str>>> {
    images::analyze(dom, nodes, None).sources
}
pub(crate) use stats::DocumentStats;
pub(crate) fn callout_class_is_semantic_evidence(
    dom: &crate::dom::Dom,
    node: crate::dom::NodeId,
) -> bool {
    callouts::class_is_semantic_evidence(dom, node)
}
pub(crate) fn is_local_footnote_reference(
    dom: &crate::dom::Dom,
    node: crate::dom::NodeId,
    href: &str,
) -> bool {
    footnotes::is_local_reference(dom, node, href)
}
pub(crate) fn footnote_class_is_semantic_evidence(
    dom: &crate::dom::Dom,
    node: crate::dom::NodeId,
) -> bool {
    footnotes::class_is_semantic_evidence(dom, node)
}
pub(crate) fn math_class_is_semantic_evidence(
    dom: &crate::dom::Dom,
    node: crate::dom::NodeId,
) -> bool {
    math::class_is_semantic_evidence(dom, node)
}
pub(crate) fn math_source_is_protected(dom: &crate::dom::Dom, node: crate::dom::NodeId) -> bool {
    math::is_source_evidence(dom, node)
}
pub(crate) fn semantic_source_is_protected(
    dom: &crate::dom::Dom,
    node: crate::dom::NodeId,
) -> bool {
    callouts::is_source_evidence(dom, node)
        || footnotes::is_source_evidence(dom, node)
        || math::is_source_evidence(dom, node)
}
pub(crate) use tables::{
    class_is_semantic_evidence as table_class_is_semantic_evidence, repeated_listing_start,
};
pub(crate) use uri::{DestinationKind, safe_destination};

pub(crate) fn semantic_normalization_counts(
    dom: &crate::dom::Dom,
    root: crate::dom::NodeId,
) -> (usize, usize, usize) {
    let nodes: Vec<_> = std::iter::once(root).chain(dom.descendants(root)).collect();
    let footnotes = footnotes::FootnoteAnalysis::analyze(dom, root);
    let math = math::MathAnalysis::analyze(dom, &nodes);
    let mut references = 0;
    let mut definitions = 0;
    let mut expressions = 0;
    for node in nodes {
        references += usize::from(footnotes.reference(node).is_some());
        definitions += usize::from(footnotes.definition(node).is_some());
        expressions += usize::from(math.value(node).is_some());
    }
    (references, definitions, expressions)
}

pub(crate) fn table_normalization_counts(
    dom: &crate::dom::Dom,
    root: crate::dom::NodeId,
) -> (usize, usize) {
    let nodes: Vec<_> = std::iter::once(root).chain(dom.descendants(root)).collect();
    let analysis = tables::TableAnalysis::analyze(dom, &nodes);
    (analysis.flattened_count(), analysis.semantic_table_count())
}

use std::fmt;

/// An index into a [`Document`] arena.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct DocumentNodeId(u32);

impl DocumentNodeId {
    fn index(self) -> usize {
        self.0 as usize
    }
}

/// The structured semantic content extracted from one page.
pub struct Document {
    nodes: Vec<ArenaNode>,
    roots: Vec<DocumentNodeId>,
    footnotes: Vec<FootnoteRecord>,
}

impl Document {
    /// Iterates over the top-level semantic nodes in document order.
    pub fn roots(&self) -> impl ExactSizeIterator<Item = DocumentNode<'_>> + DoubleEndedIterator {
        self.roots
            .iter()
            .copied()
            .map(|id| DocumentNode { document: self, id })
    }

    /// Returns the normalized text for the complete document.
    pub fn text(&self) -> String {
        stats::render_document_text(self)
    }

    /// Returns the number of characters in [`Self::text`].
    pub fn text_length(&self) -> usize {
        stats::measure_document(self).text_length
    }

    /// Returns the number of words in [`Self::text`].
    pub fn word_count(&self) -> usize {
        stats::measure_document(self).word_count
    }

    /// Resolves a footnote ID to its definition.
    pub fn footnote(&self, id: FootnoteId) -> Option<FootnoteDefinition<'_>> {
        self.footnote_record(id)
            .map(|definition| FootnoteDefinition {
                document: self,
                definition,
            })
    }

    /// Iterates over footnote definitions in semantic ID order.
    pub fn footnotes(&self) -> impl ExactSizeIterator<Item = FootnoteDefinition<'_>> {
        self.footnotes.iter().map(|definition| FootnoteDefinition {
            document: self,
            definition,
        })
    }

    pub(crate) fn root_ids(
        &self,
    ) -> impl ExactSizeIterator<Item = DocumentNodeId> + DoubleEndedIterator + '_ {
        self.roots.iter().copied()
    }

    pub(crate) fn node(&self, id: DocumentNodeId) -> Option<&ArenaNode> {
        self.nodes.get(id.index())
    }

    pub(crate) fn child_ids(&self, parent: DocumentNodeId) -> Children<'_> {
        Children {
            document: self,
            next: self.first_child(parent),
        }
    }

    pub(crate) fn first_child(&self, parent: DocumentNodeId) -> Option<DocumentNodeId> {
        self.node(parent).and_then(|node| node.first_child)
    }

    pub(crate) fn next_sibling(&self, node: DocumentNodeId) -> Option<DocumentNodeId> {
        self.node(node).and_then(|node| node.next_sibling)
    }

    pub(crate) fn footnote_record(&self, id: FootnoteId) -> Option<&FootnoteRecord> {
        self.footnotes
            .get(id.index())
            .filter(|definition| definition.id == id)
    }

    pub(crate) fn len(&self) -> usize {
        self.nodes.len()
    }

    pub(crate) fn validate(&self) -> Result<(), ValidationError> {
        validate::validate(self)
    }

    #[cfg(test)]
    pub(crate) fn debug_tree(&self) -> String {
        debug_tree(self)
    }
}

pub(crate) struct Children<'a> {
    document: &'a Document,
    next: Option<DocumentNodeId>,
}

impl Iterator for Children<'_> {
    type Item = DocumentNodeId;

    fn next(&mut self) -> Option<Self::Item> {
        let id = self.next?;
        self.next = self.document.node(id).and_then(|node| node.next_sibling);
        Some(id)
    }
}

/// One semantic node and its arena links.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ArenaNode {
    kind: NodeKind,
    first_child: Option<DocumentNodeId>,
    next_sibling: Option<DocumentNodeId>,
}

impl ArenaNode {
    pub(crate) fn kind(&self) -> &NodeKind {
        &self.kind
    }
}

/// A read-only view of one semantic node.
#[derive(Clone, Copy)]
pub struct DocumentNode<'a> {
    document: &'a Document,
    id: DocumentNodeId,
}

impl<'a> DocumentNode<'a> {
    /// Returns the semantic kind and its associated value.
    pub fn kind(self) -> &'a NodeKind {
        &self.document.nodes[self.id.index()].kind
    }

    /// Iterates over direct semantic children in document order.
    pub fn children(self) -> impl Iterator<Item = DocumentNode<'a>> + 'a {
        self.document.child_ids(self.id).map(|id| DocumentNode {
            document: self.document,
            id,
        })
    }

    /// Returns normalized text from this node and all its descendants.
    pub fn text(self) -> String {
        stats::render_node_text(self.document, self.id)
    }
}

/// The semantic meaning of a document node.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum NodeKind {
    Paragraph,
    /// A semantic block boundary with no more specific meaning.
    BlockGroup,
    Heading {
        level: u8,
    },
    BlockQuote,
    CodeBlock(CodeBlock),
    List(List),
    ListItem,
    Table(Table),
    TableCaption,
    TableRow,
    TableCell(TableCell),
    Figure,
    Figcaption,
    Details,
    Summary,
    ThematicBreak,
    DefinitionList,
    DefinitionTerm,
    DefinitionDescription,
    Callout(Callout),
    FootnoteDefinition(FootnoteId),
    Text(TextValue),
    Emphasis,
    Strong,
    Strikethrough,
    InlineCode(TextValue),
    Link(Link),
    Image(Image),
    HardBreak,
    FootnoteReference(FootnoteId),
    TaskMarker(TaskMarker),
    InlineMath(MathValue),
    DisplayMath(MathValue),
    Media(Media),
}

/// Canonical text stored by a semantic leaf node.
///
/// The wrapper keeps the retained storage format out of the public API.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TextValue(String);

impl TextValue {
    pub(crate) fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub(crate) fn as_mut_string(&mut self) -> &mut String {
        &mut self.0
    }

    /// Returns the canonical text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for TextValue {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl std::ops::Deref for TextValue {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.as_str()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CodeBlock {
    pub(crate) language: Option<Box<str>>,
    pub(crate) text: Box<str>,
}

impl CodeBlock {
    /// Returns the detected language, when available.
    pub fn language(&self) -> Option<&str> {
        self.language.as_deref()
    }

    /// Returns the normalized preformatted code.
    pub fn text(&self) -> &str {
        &self.text
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Link {
    pub(crate) destination: Box<str>,
    pub(crate) title: Option<Box<str>>,
}

impl Link {
    /// Returns the resolved, policy-validated destination.
    pub fn destination(&self) -> &str {
        &self.destination
    }

    /// Returns the optional link title.
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Image {
    pub(crate) source: Box<str>,
    pub(crate) alt: Box<str>,
    pub(crate) title: Option<Box<str>>,
    pub(crate) width: Option<u32>,
    pub(crate) height: Option<u32>,
}

impl Image {
    /// Returns the selected, policy-validated image source.
    pub fn source(&self) -> &str {
        &self.source
    }
    /// Returns the image alternative text.
    pub fn alt(&self) -> &str {
        &self.alt
    }
    /// Returns the optional image title.
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }
    /// Returns the declared width in pixels, when available.
    pub fn width(&self) -> Option<u32> {
        self.width
    }
    /// Returns the declared height in pixels, when available.
    pub fn height(&self) -> Option<u32> {
        self.height
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct List {
    pub(crate) kind: ListKind,
    pub(crate) start: Option<i64>,
}

impl List {
    /// Returns whether this list is ordered or unordered.
    pub fn kind(&self) -> ListKind {
        self.kind
    }
    /// Returns the first ordinal for an ordered list.
    pub fn start(&self) -> Option<i64> {
        self.start
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ListKind {
    Ordered,
    Unordered,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Table {
    /// Exact width when no row spans make grid placement output-specific.
    pub(crate) column_count: Option<u32>,
}

impl Table {
    /// Returns the exact column count when the semantic grid has one.
    pub fn column_count(&self) -> Option<u32> {
        self.column_count
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TableCell {
    pub(crate) header: bool,
    pub(crate) colspan: u32,
    pub(crate) rowspan: u32,
    pub(crate) alignment: Option<TableAlignment>,
}

impl TableCell {
    /// Returns true for a semantic header cell.
    pub fn is_header(&self) -> bool {
        self.header
    }
    /// Returns the column span. This value is at least one.
    pub fn colspan(&self) -> u32 {
        self.colspan
    }
    /// Returns the row span. This value is at least one.
    pub fn rowspan(&self) -> u32 {
        self.rowspan
    }
    /// Returns the normalized cell alignment.
    pub fn alignment(&self) -> Option<TableAlignment> {
        self.alignment
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TableAlignment {
    Left,
    Center,
    Right,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Callout {
    pub(crate) kind: CalloutKind,
    pub(crate) title: Option<Box<str>>,
}

impl Callout {
    /// Returns the normalized callout category.
    pub fn kind(&self) -> CalloutKind {
        self.kind
    }
    /// Returns the optional callout title.
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CalloutKind {
    Note,
    Warning,
    Tip,
    Important,
    Caution,
    Info,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaskMarker {
    pub(crate) checked: bool,
    pub(crate) fallback_label: Option<Box<str>>,
}

impl TaskMarker {
    /// Returns true when the task is checked.
    pub fn is_checked(&self) -> bool {
        self.checked
    }
    /// Returns text used by formats without task-list syntax.
    pub fn fallback_label(&self) -> Option<&str> {
        self.fallback_label.as_deref()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MathValue {
    pub(crate) source: Box<str>,
    pub(crate) format: MathFormat,
    pub(crate) fallback_text: Option<Box<str>>,
}

impl MathValue {
    /// Returns the recovered math source.
    pub fn source(&self) -> &str {
        &self.source
    }
    /// Returns the source format.
    pub fn format(&self) -> MathFormat {
        self.format
    }
    /// Returns an accessible text fallback, when available.
    pub fn fallback_text(&self) -> Option<&str> {
        self.fallback_text.as_deref()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MathFormat {
    Tex,
    Text,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Media {
    pub(crate) kind: MediaKind,
    pub(crate) source: Box<str>,
    pub(crate) title: Option<Box<str>>,
}

impl Media {
    /// Returns the media category.
    pub fn kind(&self) -> MediaKind {
        self.kind
    }
    /// Returns the selected, policy-validated source.
    pub fn source(&self) -> &str {
        &self.source
    }
    /// Returns the optional media title.
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MediaKind {
    Audio,
    Video,
    Embedded,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct FootnoteId(u32);

impl FootnoteId {
    fn index(self) -> usize {
        self.0 as usize
    }

    pub(crate) fn from_index(index: usize) -> Result<Self, BuildError> {
        u32::try_from(index)
            .map(Self)
            .map_err(|_| BuildError::CapacityExceeded)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct FootnoteRecord {
    pub(crate) id: FootnoteId,
    pub(crate) label: Box<str>,
    pub(crate) node: DocumentNodeId,
}

/// A resolved read-only footnote definition.
#[derive(Clone, Copy)]
pub struct FootnoteDefinition<'a> {
    document: &'a Document,
    definition: &'a FootnoteRecord,
}

impl<'a> FootnoteDefinition<'a> {
    /// Returns the opaque semantic footnote ID.
    pub fn id(self) -> FootnoteId {
        self.definition.id
    }
    pub(crate) fn label(self) -> &'a str {
        &self.definition.label
    }
    /// Returns the semantic definition node.
    pub fn node(self) -> DocumentNode<'a> {
        DocumentNode {
            document: self.document,
            id: self.definition.node,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ValidationError(Box<str>);

impl ValidationError {
    fn new(message: impl Into<Box<str>>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for ValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for ValidationError {}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::*;

    #[test]
    fn manual_documents_cover_initial_semantic_vocabulary() {
        let mut builder = DocumentBuilder::with_capacity(20);
        let paragraph = builder.append(None, NodeKind::Paragraph).unwrap();
        builder.append_prose(Some(paragraph), "nested ").unwrap();
        let emphasis = builder.append(Some(paragraph), NodeKind::Emphasis).unwrap();
        builder.append_prose(Some(emphasis), "formatting").unwrap();

        let list = builder
            .append(
                None,
                NodeKind::List(List {
                    kind: ListKind::Unordered,
                    start: None,
                }),
            )
            .unwrap();
        builder.append(Some(list), NodeKind::ListItem).unwrap();
        builder
            .append(
                None,
                NodeKind::CodeBlock(CodeBlock {
                    language: Some("rust".into()),
                    text: "fn main() {}\n".into(),
                }),
            )
            .unwrap();

        let table = builder
            .append(
                None,
                NodeKind::Table(Table {
                    column_count: Some(1),
                }),
            )
            .unwrap();
        let row = builder.append(Some(table), NodeKind::TableRow).unwrap();
        builder
            .append(
                Some(row),
                NodeKind::TableCell(TableCell {
                    header: true,
                    colspan: 1,
                    rowspan: 1,
                    alignment: None,
                }),
            )
            .unwrap();

        let figure = builder.append(None, NodeKind::Figure).unwrap();
        builder.append(Some(figure), NodeKind::Figcaption).unwrap();
        let footnote = FootnoteId::from_index(0).unwrap();
        builder
            .append(None, NodeKind::FootnoteReference(footnote))
            .unwrap();
        let definition = builder
            .append(None, NodeKind::FootnoteDefinition(footnote))
            .unwrap();
        builder
            .define_footnote(footnote, "note", definition)
            .unwrap();

        let document = builder.finish();
        document.validate().unwrap();
        assert_eq!(document.root_ids().count(), 7);
        let definition = document.footnote(footnote).unwrap();
        assert_eq!(definition.id(), footnote);
        assert!(matches!(
            definition.node().kind(),
            NodeKind::FootnoteDefinition(id) if *id == footnote
        ));
        assert_eq!(document.footnotes().len(), 1);
    }

    #[test]
    fn deeply_nested_semantic_documents_are_stack_safe() {
        const DEPTH: usize = 10_000;
        let mut builder = DocumentBuilder::with_capacity(DEPTH + 1);
        let mut parent = None;
        for _ in 0..DEPTH {
            parent = Some(builder.append(parent, NodeKind::BlockQuote).unwrap());
        }
        builder.append_prose(parent, "deep").unwrap();
        let document = builder.finish();
        document.validate().unwrap();

        let mut count = 0;
        let mut stack: Vec<_> = document.root_ids().collect();
        while let Some(node) = stack.pop() {
            count += 1;
            stack.extend(document.child_ids(node));
        }
        assert_eq!(count, DEPTH + 1);
    }

    #[test]
    fn prose_stays_under_its_requested_parent() {
        let mut builder = DocumentBuilder::with_capacity(3);
        builder.append_prose(None, "root").unwrap();
        let paragraph = builder.append(None, NodeKind::Paragraph).unwrap();
        builder.append_prose(Some(paragraph), "child").unwrap();
        assert_eq!(
            builder.finish().debug_tree(),
            "Text(\"root\")\nParagraph\n  Text(\"child\")\n"
        );

        let mut builder = DocumentBuilder::with_capacity(1);
        assert_eq!(
            builder.append_prose(Some(DocumentNodeId(u32::MAX)), "bad"),
            Err(BuildError::InvalidParent)
        );
    }

    #[test]
    fn validation_rejects_cycles_and_invalid_cells() {
        let mut builder = DocumentBuilder::with_capacity(2);
        let first = builder.append(None, NodeKind::Paragraph).unwrap();
        let second = builder.append(Some(first), NodeKind::Strong).unwrap();
        let mut cycle = builder.finish();
        cycle.nodes[second.index()].next_sibling = Some(second);
        assert!(cycle.validate().is_err());

        let mut builder = DocumentBuilder::with_capacity(1);
        builder
            .append(
                None,
                NodeKind::TableCell(TableCell {
                    header: false,
                    colspan: 0,
                    rowspan: 1,
                    alignment: None,
                }),
            )
            .unwrap();
        assert!(builder.finish().validate().is_err());
    }
}

#[cfg(test)]
fn debug_tree(document: &Document) -> String {
    enum Task {
        Node(DocumentNodeId, usize),
    }

    let mut output = String::new();
    let mut tasks = Vec::new();
    tasks.extend(document.roots.iter().rev().map(|&id| Task::Node(id, 0)));
    while let Some(Task::Node(id, depth)) = tasks.pop() {
        let Some(node) = document.node(id) else {
            continue;
        };
        output.push_str(&"  ".repeat(depth));
        write_kind(&mut output, &node.kind);
        output.push('\n');
        let children: Vec<_> = document.child_ids(id).collect();
        tasks.extend(
            children
                .into_iter()
                .rev()
                .map(|child| Task::Node(child, depth + 1)),
        );
    }
    output
}

#[cfg(test)]
fn write_kind(output: &mut String, kind: &NodeKind) {
    use std::fmt::Write as _;
    match kind {
        NodeKind::Text(value) => write!(output, "Text({:?})", value.as_str()).unwrap(),
        NodeKind::Heading { level } => write!(output, "Heading(level={level})").unwrap(),
        NodeKind::CodeBlock(code) => write!(
            output,
            "CodeBlock(language={:?}, text={:?})",
            code.language, code.text
        )
        .unwrap(),
        NodeKind::List(list) => {
            write!(output, "List(kind={:?}, start={:?})", list.kind, list.start).unwrap()
        }
        NodeKind::Table(table) => {
            write!(output, "Table(columns={:?})", table.column_count).unwrap()
        }
        NodeKind::TableCell(cell) => write!(
            output,
            "TableCell(header={}, colspan={}, rowspan={}, alignment={:?})",
            cell.header, cell.colspan, cell.rowspan, cell.alignment
        )
        .unwrap(),
        NodeKind::Link(link) => write!(
            output,
            "Link(destination={:?}, title={:?})",
            link.destination, link.title
        )
        .unwrap(),
        NodeKind::Image(image) => write!(
            output,
            "Image(source={:?}, alt={:?}, title={:?}, width={:?}, height={:?})",
            image.source, image.alt, image.title, image.width, image.height
        )
        .unwrap(),
        NodeKind::InlineCode(value) => write!(output, "InlineCode({:?})", value.as_str()).unwrap(),
        NodeKind::FootnoteReference(id) => write!(output, "FootnoteReference({})", id.0).unwrap(),
        NodeKind::FootnoteDefinition(id) => write!(output, "FootnoteDefinition({})", id.0).unwrap(),
        NodeKind::TaskMarker(marker) => write!(
            output,
            "TaskMarker(checked={}, fallback={:?})",
            marker.checked, marker.fallback_label
        )
        .unwrap(),
        NodeKind::InlineMath(value) | NodeKind::DisplayMath(value) => write!(
            output,
            "{}(source={:?}, format={:?}, fallback={:?})",
            if matches!(kind, NodeKind::InlineMath(_)) {
                "InlineMath"
            } else {
                "DisplayMath"
            },
            value.source,
            value.format,
            value.fallback_text
        )
        .unwrap(),
        NodeKind::Callout(callout) => write!(
            output,
            "Callout(kind={:?}, title={:?})",
            callout.kind, callout.title
        )
        .unwrap(),
        NodeKind::Media(media) => write!(
            output,
            "Media(kind={:?}, source={:?}, title={:?})",
            media.kind, media.source, media.title
        )
        .unwrap(),
        other => write!(output, "{other:?}").unwrap(),
    }
}
