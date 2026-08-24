//! The extracted page and lazy output builders.

use std::{fmt, io};

use crate::diagnostics::ExtractionDiagnostics;
use crate::document::Document;
use crate::metadata::{Metadata, MetadataDiagnostics};
use serde_json::Value;

struct IoFmtWriter<'a, W: ?Sized> {
    writer: &'a mut W,
    error: Option<io::Error>,
}

impl<'a, W: io::Write + ?Sized> IoFmtWriter<'a, W> {
    fn new(writer: &'a mut W) -> Self {
        Self {
            writer,
            error: None,
        }
    }

    fn finish(self, result: fmt::Result) -> io::Result<()> {
        if let Some(error) = self.error {
            return Err(error);
        }

        result.map_err(|_| io::Error::other("rendering failed"))
    }
}

impl<W: io::Write + ?Sized> fmt::Write for IoFmtWriter<'_, W> {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        if self.error.is_some() {
            return Err(fmt::Error);
        }

        if let Err(error) = self.writer.write_all(value.as_bytes()) {
            self.error = Some(error);
            return Err(fmt::Error);
        }

        Ok(())
    }

    fn write_char(&mut self, value: char) -> fmt::Result {
        let mut buffer = [0; 4];
        self.write_str(value.encode_utf8(&mut buffer))
    }
}

/// Relevant page content and metadata.
///
/// Output formats are rendered lazily from one private semantic representation.
/// Calling a render method more than once produces the same output.
pub struct ExtractedPage {
    metadata: Metadata,
    content: ExtractedContent,
    diagnostics: Option<ExtractionDiagnostics>,
    metadata_diagnostics: Option<MetadataDiagnostics>,
    structured_data: Option<Vec<Value>>,
}

/// Independently owned parts of an extracted page.
pub struct ExtractedPageParts {
    /// Discovered page metadata.
    pub metadata: Metadata,
    /// Extraction diagnostics when the extractor enabled them.
    pub diagnostics: Option<ExtractionDiagnostics>,
    /// Metadata provenance when the extractor enabled it.
    pub metadata_diagnostics: Option<MetadataDiagnostics>,
    /// Parsed JSON-LD items when the extractor retained them.
    pub structured_data: Option<Vec<Value>>,
    /// Extracted semantic content.
    pub content: ExtractedContent,
}

/// Extracted semantic content with lazy rendering and metrics.
///
/// This value owns the private semantic representation. It does not borrow the
/// page metadata or diagnostics.
pub struct ExtractedContent {
    document: Document,
}

impl ExtractedPage {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        document: Document,
        metadata: Metadata,
        diagnostics: Option<ExtractionDiagnostics>,
        metadata_diagnostics: Option<MetadataDiagnostics>,
        structured_data: Option<Vec<Value>>,
    ) -> Self {
        Self {
            metadata,
            content: ExtractedContent { document },
            diagnostics,
            metadata_diagnostics,
            structured_data,
        }
    }

    /// Returns discovered page metadata.
    pub fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    /// Consumes the page and returns its metadata.
    pub fn into_metadata(self) -> Metadata {
        self.metadata
    }

    /// Returns the extracted semantic content.
    pub fn content(&self) -> &ExtractedContent {
        &self.content
    }

    /// Consumes the page and returns its independently owned parts.
    pub fn into_parts(self) -> ExtractedPageParts {
        ExtractedPageParts {
            metadata: self.metadata,
            diagnostics: self.diagnostics,
            metadata_diagnostics: self.metadata_diagnostics,
            structured_data: self.structured_data,
            content: self.content,
        }
    }

    /// Returns extraction diagnostics when the extractor enabled them.
    pub fn diagnostics(&self) -> Option<&ExtractionDiagnostics> {
        self.diagnostics.as_ref()
    }

    /// Returns metadata provenance when the extractor enabled it.
    pub fn metadata_diagnostics(&self) -> Option<&MetadataDiagnostics> {
        self.metadata_diagnostics.as_ref()
    }

    /// Returns parsed JSON-LD items when the extractor retained them.
    ///
    /// The value is `None` when retention was disabled. It is `Some` with an
    /// empty slice when retention was enabled but no valid items were found.
    pub fn structured_data(&self) -> Option<&[Value]> {
        self.structured_data.as_deref()
    }

    /// Renders the extracted content as CommonMark.
    pub fn markdown(&self) -> String {
        self.markdown_builder().render()
    }

    /// Writes the extracted content as CommonMark.
    ///
    /// The method writes to any value that implements [`fmt::Write`]. It
    /// returns the writer error, when one occurs.
    pub fn write_markdown<W: fmt::Write>(&self, writer: &mut W) -> fmt::Result {
        self.markdown_builder().write(writer)
    }

    /// Writes the extracted content as CommonMark to an I/O writer.
    ///
    /// The method writes UTF-8 bytes to any value that implements
    /// [`std::io::Write`]. It returns the first I/O error.
    pub fn write_markdown_io<W: io::Write>(&self, writer: &mut W) -> io::Result<()> {
        let mut adapter = IoFmtWriter::new(writer);
        let result = self.markdown_builder().write(&mut adapter);
        adapter.finish(result)
    }

    /// Returns a Markdown output builder.
    pub fn markdown_builder(&self) -> MarkdownBuilder<'_> {
        MarkdownBuilder {
            document: &self.content.document,
            links: true,
            images: true,
            max_line_width: None,
        }
    }

    /// Renders the extracted content as normalized plain text.
    pub fn text(&self) -> String {
        let _phase =
            crate::instrumentation::PhaseGuard::new(crate::instrumentation::Phase::Rendering);
        self.content.document.text()
    }

    /// Writes the extracted content as normalized plain text.
    ///
    /// The method writes to any value that implements [`fmt::Write`]. It
    /// returns the writer error, when one occurs.
    pub fn write_text<W: fmt::Write>(&self, writer: &mut W) -> fmt::Result {
        let _phase =
            crate::instrumentation::PhaseGuard::new(crate::instrumentation::Phase::Rendering);
        self.content.document.write_text(writer)
    }

    /// Writes the extracted content as normalized plain text to an I/O writer.
    ///
    /// The method writes UTF-8 bytes to any value that implements
    /// [`std::io::Write`]. It returns the first I/O error.
    pub fn write_text_io<W: io::Write>(&self, writer: &mut W) -> io::Result<()> {
        let mut adapter = IoFmtWriter::new(writer);
        let result = self.write_text(&mut adapter);
        adapter.finish(result)
    }

    /// Renders the extracted content as canonical semantic HTML.
    ///
    /// The private semantic representation cannot contain active source elements,
    /// arbitrary attributes, or unsupported URI schemes.
    pub fn html(&self) -> String {
        self.html_builder().render()
    }

    /// Writes the extracted content as canonical semantic HTML.
    ///
    /// The method writes to any value that implements [`fmt::Write`]. It
    /// returns the writer error, when one occurs.
    pub fn write_html<W: fmt::Write>(&self, writer: &mut W) -> fmt::Result {
        self.html_builder().write(writer)
    }

    /// Writes the extracted content as canonical semantic HTML to an I/O writer.
    ///
    /// The method writes UTF-8 bytes to any value that implements
    /// [`std::io::Write`]. It returns the first I/O error.
    pub fn write_html_io<W: io::Write>(&self, writer: &mut W) -> io::Result<()> {
        let mut adapter = IoFmtWriter::new(writer);
        let result = self.html_builder().write(&mut adapter);
        adapter.finish(result)
    }

    /// Returns an HTML output builder.
    pub fn html_builder(&self) -> HtmlBuilder<'_> {
        HtmlBuilder {
            document: &self.content.document,
        }
    }

    /// Returns the number of words in the normalized extracted text.
    pub fn word_count(&self) -> usize {
        self.content.document.word_count()
    }

    /// Returns the number of characters in the normalized extracted text.
    pub fn text_length(&self) -> usize {
        self.content.document.text_length()
    }

    /// Returns the number of characters contributed by link content.
    pub fn link_text_length(&self) -> usize {
        self.content.document.link_text_length()
    }

    /// Returns the fraction of normalized text contributed by links.
    pub fn link_density(&self) -> f64 {
        self.content.document.link_density()
    }

    /// Returns the number of semantic paragraphs.
    pub fn paragraph_count(&self) -> usize {
        self.content.document.paragraph_count()
    }

    /// Returns the number of semantic headings.
    pub fn heading_count(&self) -> usize {
        self.content.document.heading_count()
    }

    /// Returns the number of semantic list items.
    pub fn list_item_count(&self) -> usize {
        self.content.document.list_item_count()
    }

    /// Returns the number of semantic code blocks.
    pub fn code_block_count(&self) -> usize {
        self.content.document.code_block_count()
    }

    /// Returns the number of semantic data tables.
    pub fn table_count(&self) -> usize {
        self.content.document.table_count()
    }

    /// Returns the number of semantic figures.
    pub fn figure_count(&self) -> usize {
        self.content.document.figure_count()
    }

    /// Returns the number of semantic images.
    pub fn image_count(&self) -> usize {
        self.content.document.image_count()
    }

    /// Returns the number of footnote references.
    pub fn footnote_reference_count(&self) -> usize {
        self.content.document.footnote_reference_count()
    }

    /// Returns the number of footnote definitions.
    pub fn footnote_definition_count(&self) -> usize {
        self.content.document.footnote_definition_count()
    }

    /// Returns the number of math expressions.
    pub fn math_count(&self) -> usize {
        self.content.document.math_count()
    }

    /// Returns the number of blocks with useful structural evidence.
    pub fn structured_block_count(&self) -> usize {
        self.content.document.structured_block_count()
    }

    /// Returns whether normalized text contains an alphanumeric character.
    pub fn has_alphanumeric_text(&self) -> bool {
        self.content.document.stats().has_alphanumeric_text
    }

    /// Returns the number of alphabetic characters in normalized text.
    pub fn alphabetic_chars(&self) -> usize {
        self.content.document.stats().alphabetic_chars
    }

    /// Returns the number of numeric characters in normalized text.
    pub fn digit_chars(&self) -> usize {
        self.content.document.stats().digit_chars
    }

    /// Returns whether the result contains contextual semantic structure.
    pub fn has_contextual_structure(&self) -> bool {
        self.content.document.has_contextual_structure()
    }

    #[cfg(test)]
    pub(crate) fn semantic_node_count(&self) -> usize {
        self.content.document.len()
    }

    #[cfg(test)]
    pub(crate) fn semantic_retained_bytes(&self) -> usize {
        self.content.document.retained_bytes_estimate()
    }

    /// Checks private semantic representation invariants for fuzz testing.
    #[doc(hidden)]
    #[cfg(feature = "fuzzing")]
    pub fn validate_document(&self) -> bool {
        self.content.document.validate().is_ok()
    }
}

impl ExtractedContent {
    /// Renders the content as CommonMark.
    pub fn markdown(&self) -> String {
        self.markdown_builder().render()
    }

    /// Writes the content as CommonMark.
    pub fn write_markdown<W: fmt::Write>(&self, writer: &mut W) -> fmt::Result {
        self.markdown_builder().write(writer)
    }

    /// Writes the content as CommonMark to an I/O writer.
    pub fn write_markdown_io<W: io::Write>(&self, writer: &mut W) -> io::Result<()> {
        let mut adapter = IoFmtWriter::new(writer);
        let result = self.markdown_builder().write(&mut adapter);
        adapter.finish(result)
    }

    /// Returns a Markdown output builder.
    pub fn markdown_builder(&self) -> MarkdownBuilder<'_> {
        MarkdownBuilder {
            document: &self.document,
            links: true,
            images: true,
            max_line_width: None,
        }
    }

    /// Renders the content as normalized plain text.
    pub fn text(&self) -> String {
        let _phase =
            crate::instrumentation::PhaseGuard::new(crate::instrumentation::Phase::Rendering);
        self.document.text()
    }

    /// Writes the content as normalized plain text.
    pub fn write_text<W: fmt::Write>(&self, writer: &mut W) -> fmt::Result {
        let _phase =
            crate::instrumentation::PhaseGuard::new(crate::instrumentation::Phase::Rendering);
        self.document.write_text(writer)
    }

    /// Writes the content as normalized plain text to an I/O writer.
    pub fn write_text_io<W: io::Write>(&self, writer: &mut W) -> io::Result<()> {
        let mut adapter = IoFmtWriter::new(writer);
        let result = self.write_text(&mut adapter);
        adapter.finish(result)
    }

    /// Renders the content as canonical semantic HTML.
    pub fn html(&self) -> String {
        self.html_builder().render()
    }

    /// Writes the content as canonical semantic HTML.
    pub fn write_html<W: fmt::Write>(&self, writer: &mut W) -> fmt::Result {
        self.html_builder().write(writer)
    }

    /// Writes the content as canonical semantic HTML to an I/O writer.
    pub fn write_html_io<W: io::Write>(&self, writer: &mut W) -> io::Result<()> {
        let mut adapter = IoFmtWriter::new(writer);
        let result = self.html_builder().write(&mut adapter);
        adapter.finish(result)
    }

    /// Returns an HTML output builder.
    pub fn html_builder(&self) -> HtmlBuilder<'_> {
        HtmlBuilder {
            document: &self.document,
        }
    }

    /// Returns the number of words in the normalized content text.
    pub fn word_count(&self) -> usize {
        self.document.word_count()
    }

    /// Returns the number of characters in the normalized content text.
    pub fn text_length(&self) -> usize {
        self.document.text_length()
    }

    /// Returns the number of characters contributed by link content.
    pub fn link_text_length(&self) -> usize {
        self.document.link_text_length()
    }

    /// Returns the fraction of normalized text contributed by links.
    pub fn link_density(&self) -> f64 {
        self.document.link_density()
    }

    /// Returns the number of semantic paragraphs.
    pub fn paragraph_count(&self) -> usize {
        self.document.paragraph_count()
    }

    /// Returns the number of semantic headings.
    pub fn heading_count(&self) -> usize {
        self.document.heading_count()
    }

    /// Returns the number of semantic list items.
    pub fn list_item_count(&self) -> usize {
        self.document.list_item_count()
    }

    /// Returns the number of semantic code blocks.
    pub fn code_block_count(&self) -> usize {
        self.document.code_block_count()
    }

    /// Returns the number of semantic data tables.
    pub fn table_count(&self) -> usize {
        self.document.table_count()
    }

    /// Returns the number of semantic figures.
    pub fn figure_count(&self) -> usize {
        self.document.figure_count()
    }

    /// Returns the number of semantic images.
    pub fn image_count(&self) -> usize {
        self.document.image_count()
    }

    /// Returns the number of footnote references.
    pub fn footnote_reference_count(&self) -> usize {
        self.document.footnote_reference_count()
    }

    /// Returns the number of footnote definitions.
    pub fn footnote_definition_count(&self) -> usize {
        self.document.footnote_definition_count()
    }

    /// Returns the number of math expressions.
    pub fn math_count(&self) -> usize {
        self.document.math_count()
    }

    /// Returns the number of blocks with useful structural evidence.
    pub fn structured_block_count(&self) -> usize {
        self.document.structured_block_count()
    }

    /// Returns whether normalized text contains an alphanumeric character.
    pub fn has_alphanumeric_text(&self) -> bool {
        self.document.stats().has_alphanumeric_text
    }

    /// Returns the number of alphabetic characters in normalized text.
    pub fn alphabetic_chars(&self) -> usize {
        self.document.stats().alphabetic_chars
    }

    /// Returns the number of numeric characters in normalized text.
    pub fn digit_chars(&self) -> usize {
        self.document.stats().digit_chars
    }

    /// Returns whether the content contains contextual semantic structure.
    pub fn has_contextual_structure(&self) -> bool {
        self.document.has_contextual_structure()
    }

    /// Checks private semantic representation invariants for fuzz testing.
    #[doc(hidden)]
    #[cfg(feature = "fuzzing")]
    pub fn validate_document(&self) -> bool {
        self.document.validate().is_ok()
    }
}

/// Configures HTML rendering for extracted content.
///
/// Canonical HTML is safe by construction. The builder remains public for API
/// compatibility and for symmetry with [`MarkdownBuilder`].
pub struct HtmlBuilder<'a> {
    document: &'a Document,
}

impl HtmlBuilder<'_> {
    /// Renders canonical semantic HTML.
    pub fn render(self) -> String {
        let _phase =
            crate::instrumentation::PhaseGuard::new(crate::instrumentation::Phase::Rendering);
        crate::render::html::render_html(self.document, self.document.output_capacity_hint())
    }

    /// Writes canonical semantic HTML to `writer`.
    pub fn write<W: fmt::Write>(self, writer: &mut W) -> fmt::Result {
        let _phase =
            crate::instrumentation::PhaseGuard::new(crate::instrumentation::Phase::Rendering);
        crate::render::html::write_html(self.document, writer)
    }

    /// Writes canonical semantic HTML to an I/O writer.
    ///
    /// The method writes UTF-8 bytes to any value that implements
    /// [`std::io::Write`]. It returns the first I/O error.
    pub fn write_io<W: io::Write>(self, writer: &mut W) -> io::Result<()> {
        let mut adapter = IoFmtWriter::new(writer);
        let result = self.write(&mut adapter);
        adapter.finish(result)
    }
}

/// Configures Markdown rendering for extracted content.
///
/// Links and images are included by default. Line wrapping is disabled by
/// default. The builder consumes itself when it renders the result.
pub struct MarkdownBuilder<'a> {
    document: &'a Document,
    links: bool,
    images: bool,
    max_line_width: Option<usize>,
}

impl MarkdownBuilder<'_> {
    /// Controls whether links are rendered as links or plain text.
    pub fn links(mut self, enabled: bool) -> Self {
        self.links = enabled;
        self
    }

    /// Controls whether images are rendered.
    pub fn images(mut self, enabled: bool) -> Self {
        self.images = enabled;
        self
    }

    /// Sets the preferred maximum width of Markdown source lines.
    ///
    /// The renderer wraps prose at whitespace. It preserves the prefixes for
    /// lists, quotes, and footnotes. It does not split atomic Markdown content,
    /// such as URLs and code spans. It also does not wrap code blocks, math
    /// blocks, headings, or tables. These items can exceed the selected width.
    ///
    /// A width of `0` disables wrapping.
    pub fn max_line_width(mut self, width: usize) -> Self {
        self.max_line_width = (width > 0).then_some(width);
        self
    }

    /// Renders the configured Markdown output.
    pub fn render(self) -> String {
        let _phase =
            crate::instrumentation::PhaseGuard::new(crate::instrumentation::Phase::Rendering);
        crate::render::markdown::render_markdown(
            self.document,
            self.document.output_capacity_hint(),
            crate::render::markdown::MarkdownConfig {
                links: self.links,
                images: self.images,
                max_line_width: self.max_line_width,
            },
        )
    }

    /// Writes the configured Markdown output to `writer`.
    pub fn write<W: fmt::Write>(self, writer: &mut W) -> fmt::Result {
        let _phase =
            crate::instrumentation::PhaseGuard::new(crate::instrumentation::Phase::Rendering);
        crate::render::markdown::write_markdown(
            self.document,
            writer,
            crate::render::markdown::MarkdownConfig {
                links: self.links,
                images: self.images,
                max_line_width: self.max_line_width,
            },
        )
    }

    /// Writes the configured Markdown output to an I/O writer.
    ///
    /// The method writes UTF-8 bytes to any value that implements
    /// [`std::io::Write`]. It returns the first I/O error.
    pub fn write_io<W: io::Write>(self, writer: &mut W) -> io::Result<()> {
        let mut adapter = IoFmtWriter::new(writer);
        let result = self.write(&mut adapter);
        adapter.finish(result)
    }
}

#[cfg(test)]
mod tests {
    use std::{fmt, io};

    use crate::extract;

    #[test]
    fn outputs_are_lazy_and_deterministic() {
        let page = extract(
            "<main><p>Hello <a href='/world'>world</a>.</p><img src='image.png' alt='Image'></main>",
            Some("https://example.com/page"),
        )
        .unwrap();

        assert_eq!(page.markdown(), page.markdown());
        assert_eq!(page.text(), page.text());
        assert_eq!(page.html(), page.html());
        assert_eq!(page.text_length(), page.text().chars().count());
        assert_eq!(page.word_count(), 2);
        assert_eq!(page.image_count(), 1);
    }

    #[test]
    fn into_parts_separates_owned_results() {
        let page = extract(
            "<html><head><title>Page title</title></head><body><main><h1>Page title</h1><p>This page has enough meaningful content for extraction. It explains the subject with clear details and useful context for every reader.</p></main></body></html>",
            None,
        )
        .unwrap();

        let parts: crate::ExtractedPageParts = page.into_parts();

        assert!(parts.diagnostics.is_none());
        assert!(parts.metadata_diagnostics.is_none());
        assert!(parts.structured_data.is_none());
        let _metadata = parts.metadata;
        let content: crate::ExtractedContent = parts.content;
        assert!(content.markdown().contains("enough meaningful content"));
        assert!(content.word_count() > 10);
    }

    #[test]
    fn write_methods_match_string_methods() {
        let page = extract(
            "<main><p>Hello <a href='/world'>world</a>.</p><img src='image.png' alt='Image'></main>",
            Some("https://example.com/page"),
        )
        .unwrap();

        let mut markdown = String::new();
        page.write_markdown(&mut markdown).unwrap();
        assert_eq!(markdown, page.markdown());

        let mut text = String::new();
        page.write_text(&mut text).unwrap();
        assert_eq!(text, page.text());

        let mut html = String::new();
        page.write_html(&mut html).unwrap();
        assert_eq!(html, page.html());

        let mut configured = String::new();
        page.markdown_builder()
            .links(false)
            .images(false)
            .write(&mut configured)
            .unwrap();
        assert_eq!(
            configured,
            page.markdown_builder().links(false).images(false).render()
        );

        let mut markdown = Vec::new();
        page.write_markdown_io(&mut markdown).unwrap();
        assert_eq!(markdown, page.markdown().as_bytes());

        let mut text = Vec::new();
        page.write_text_io(&mut text).unwrap();
        assert_eq!(text, page.text().as_bytes());

        let mut html = Vec::new();
        page.write_html_io(&mut html).unwrap();
        assert_eq!(html, page.html().as_bytes());

        let mut configured = Vec::new();
        page.markdown_builder()
            .links(false)
            .images(false)
            .write_io(&mut configured)
            .unwrap();
        assert_eq!(
            configured,
            page.markdown_builder()
                .links(false)
                .images(false)
                .render()
                .as_bytes()
        );
    }

    #[test]
    fn write_methods_return_writer_errors() {
        struct FailingWriter;

        impl fmt::Write for FailingWriter {
            fn write_str(&mut self, _: &str) -> fmt::Result {
                Err(fmt::Error)
            }
        }

        let page = extract("<main><p>Content</p></main>", None).unwrap();
        assert!(page.write_markdown(&mut FailingWriter).is_err());
        assert!(page.write_text(&mut FailingWriter).is_err());
        assert!(page.write_html(&mut FailingWriter).is_err());
    }

    #[test]
    fn io_write_methods_return_io_errors() {
        struct FailingWriter;

        impl io::Write for FailingWriter {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::Error::new(io::ErrorKind::BrokenPipe, "test error"))
            }

            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }

        let page = extract("<main><p>Content</p></main>", None).unwrap();

        let error = page.write_markdown_io(&mut FailingWriter).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);

        let error = page.write_text_io(&mut FailingWriter).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);

        let error = page.write_html_io(&mut FailingWriter).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);

        let error = page
            .markdown_builder()
            .write_io(&mut FailingWriter)
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
    }

    #[test]
    fn write_methods_send_output_in_multiple_writes() {
        struct CountingWriter {
            output: String,
            writes: usize,
        }

        impl fmt::Write for CountingWriter {
            fn write_str(&mut self, value: &str) -> fmt::Result {
                self.writes += 1;
                self.output.push_str(value);
                Ok(())
            }
        }

        let page = extract(
            "<main><p>First paragraph.</p><p>Second paragraph.</p></main>",
            None,
        )
        .unwrap();

        let mut markdown = CountingWriter {
            output: String::new(),
            writes: 0,
        };
        page.write_markdown(&mut markdown).unwrap();
        assert_eq!(markdown.output, page.markdown());
        assert!(markdown.writes > 1);

        let mut html = CountingWriter {
            output: String::new(),
            writes: 0,
        };
        page.write_html(&mut html).unwrap();
        assert_eq!(html.output, page.html());
        assert!(html.writes > 1);

        let mut text = CountingWriter {
            output: String::new(),
            writes: 0,
        };
        page.write_text(&mut text).unwrap();
        assert_eq!(text.output, page.text());
        assert!(text.writes > 1);
    }

    #[test]
    fn first_markdown_render_does_not_initialize_stats() {
        let page = extract("<main><p>Markdown output.</p></main>", None).unwrap();

        assert!(!page.content.document.stats_initialized());
        assert_eq!(page.markdown(), "Markdown output.\n");
        assert!(!page.content.document.stats_initialized());
    }

    #[test]
    fn first_html_render_does_not_initialize_stats() {
        let page = extract("<main><p>HTML output.</p></main>", None).unwrap();

        assert!(!page.content.document.stats_initialized());
        assert_eq!(page.html(), "<div><p>HTML output.</p></div>");
        assert!(!page.content.document.stats_initialized());
    }

    #[test]
    fn first_text_render_initializes_stats_during_the_text_walk() {
        let page = extract("<main><p>Text output.</p></main>", None).unwrap();

        assert!(!page.content.document.stats_initialized());
        assert_eq!(page.text(), "Text output.");
        assert!(page.content.document.stats_initialized());
        assert_eq!(page.text_length(), 12);
        assert_eq!(page.word_count(), 2);
    }

    #[test]
    fn cached_stats_keep_text_rendering_deterministic() {
        let page = extract("<main><p>Cached text.</p></main>", None).unwrap();

        assert_eq!(page.word_count(), 2);
        assert_eq!(page.text(), "Cached text.");
        assert_eq!(page.text(), "Cached text.");
    }

    #[test]
    fn semantic_metrics_are_cached_on_the_document() {
        let page = extract("<main><p>Markdown only output.</p></main>", None).unwrap();

        let before = page.content.document.stats();
        assert!(page.markdown().contains("Markdown only output."));
        assert_eq!(page.content.document.stats(), before);
        assert_eq!(page.word_count(), 3);
    }

    #[test]
    fn text_statistics_include_structural_boundaries() {
        let page = extract(
            "<main><p>Hello</p><p>world</p><table><tr><td>one</td><td>two</td></tr></table><p>before<br>after</p></main>",
            None,
        )
        .unwrap();

        assert_eq!(page.text(), "Hello world one two before after");
        assert_eq!(page.text_length(), page.text().chars().count());
        assert_eq!(page.word_count(), 6);
    }

    #[test]
    fn markdown_builder_controls_links_and_images() {
        let page = extract(
            "<main><p>Hello <a href='/world'>world</a>.</p><img src='image.png' alt='Image'></main>",
            Some("https://example.com/page"),
        )
        .unwrap();
        let markdown = page.markdown_builder().links(false).images(false).render();

        assert!(markdown.contains("Hello world."));
        assert!(!markdown.contains("]("));
        assert!(!markdown.contains("!["));
    }

    #[test]
    fn markdown_builder_controls_line_width_for_all_writer_types() {
        let page = extract(
            "<main><p>Alpha beta gamma delta epsilon zeta.</p></main>",
            None,
        )
        .unwrap();
        let expected = "Alpha beta gamma\ndelta epsilon\nzeta.\n";

        assert_eq!(
            page.markdown_builder().max_line_width(18).render(),
            expected
        );

        let mut formatted = String::new();
        page.markdown_builder()
            .max_line_width(18)
            .write(&mut formatted)
            .unwrap();
        assert_eq!(formatted, expected);

        let mut bytes = Vec::new();
        page.markdown_builder()
            .max_line_width(18)
            .write_io(&mut bytes)
            .unwrap();
        assert_eq!(bytes, expected.as_bytes());

        assert_eq!(
            page.markdown_builder().max_line_width(0).render(),
            page.markdown()
        );
    }

    #[test]
    fn disabled_images_do_not_make_an_image_only_heading_visible() {
        let page = extract(
            "<main><h2><img src='diagram.png' alt='Diagram'></h2><p>Content.</p></main>",
            None,
        )
        .unwrap();
        let markdown = page.markdown_builder().images(false).render();

        assert_eq!(markdown, "Content.\n");
    }

    #[test]
    fn extracted_page_retains_only_the_private_semantic_representation() {
        let clutter = "<nav><span>irrelevant</span></nav>".repeat(200);
        let html = format!(
            "<html><body>{clutter}<main><p>This is the relevant page content.</p></main></body></html>"
        );
        let page = extract(&html, None).unwrap();

        assert!(
            page.content.document.len() < 10,
            "retained {} semantic nodes",
            page.content.document.len()
        );
        assert!(page.text().contains("relevant page content"));
    }

    #[test]
    fn canonical_html_cannot_include_active_source_content() {
        let page = extract(
            r##"<main><p onclick="alert(1)"><a href="java&#x0A;script:alert(1)" onfocus="x">bad</a>
            <a href="/safe">safe</a><img src="data:text/html,bad" onerror="x">
            <svg><script>alert(1)</script><circle onload="x"></circle></svg>
            <svg><a id="target"><text>click</text></a><animate href="#target" attributeName="href" values="javascript:alert(1)" fill="freeze"></animate></svg>
            <iframe srcdoc="<script>x</script>"></iframe></p></main>"##,
            Some("https://example.com/page"),
        )
        .unwrap();

        let html = page.html();
        assert!(!html.to_ascii_lowercase().contains("javascript:"));
        assert!(!html.contains("onclick="));
        assert!(!html.contains("onfocus="));
        assert!(!html.contains("onerror="));
        assert!(!html.contains("onload="));
        assert!(!html.contains("<script"));
        assert!(!html.contains("<animate"));
        assert!(!html.contains("attributeName="));
        assert!(!html.contains("values="));
        assert!(!html.contains("<iframe"));
        assert!(!html.contains("srcdoc="));
        assert!(!html.contains("data:text/html"));
        assert!(html.contains("href=\"https://example.com/safe\""));
        assert_eq!(html, page.html());
    }
}
